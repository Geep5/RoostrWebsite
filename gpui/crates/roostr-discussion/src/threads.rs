//! Pure projections of the conversations stored in an object's own DAG
//! (`src/lib/threads.ts`), plus the names and recipient discovery of
//! `src/lib/conversations.ts`.

use std::collections::{BTreeSet, HashSet};

use roostr_core::{AgentEndpoint, Fields, Object, QueryRow, State, Store, guest_agents};
use roostr_rt::Task;
use serde_json::json;

use crate::chat::{DISCUSSION, mailbox_entries};

/// One agent that may be @-addressed on an object.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ObjectAgentOption {
    pub endpoint: AgentEndpoint,
    /// The object's name (where the agent is reached).
    pub name: String,
    pub agent_name: String,
    pub icon: String,
}

impl ObjectAgentOption {
    /// One guest = object + agent: every guest on a task shares the task's object id.
    pub fn key(&self) -> String {
        format!("{}:{}", self.endpoint.object_id, self.endpoint.agent_id)
    }
}

/// Who may be @-addressed on `object`: every agent of its space (spawned
/// helpers aside), each as an endpoint on this object - its default agents
/// (its `agent` property) only decide who the composer starts tagged
/// (`seed_tags`). An agent's own page tags the agent (its home endpoint).
/// `records` are the agent objects to name.
pub(crate) fn object_agent_options(
    object_id: &str,
    type_key: &str,
    fields: &Fields,
    records: &[QueryRow],
    name_of: impl Fn(&str) -> String,
) -> Vec<ObjectAgentOption> {
    let space = if type_key == "channel" {
        object_id
    } else {
        roostr_core::field_str(fields, "channel")
    };
    let in_space = records
        .iter()
        .filter(|r| {
            roostr_core::field_str(&r.fields, "channel") == space
                && roostr_core::field_str(&r.fields, "spawn_parent").is_empty()
        })
        .map(|r| r.id.clone());
    let guests: Vec<String> = match type_key {
        "agent" => std::iter::once(object_id.to_string())
            .chain(guest_agents(fields))
            .collect(),
        _ => guest_agents(fields).into_iter().chain(in_space).collect(),
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for agent_id in guests {
        if !seen.insert(agent_id.clone()) {
            continue;
        }
        let Some(agent) = records.iter().find(|r| r.id == agent_id) else {
            continue;
        };
        let agent_name = match roostr_core::field_str(&agent.fields, "name") {
            "" => "Agent".to_string(),
            name => name.to_string(),
        };
        let name = match name_of(object_id) {
            n if n.is_empty() => agent_name.clone(),
            n => n,
        };
        out.push(ObjectAgentOption {
            endpoint: AgentEndpoint {
                object_id: object_id.to_string(),
                agent_id,
            },
            name,
            icon: roostr_core::field_str(&agent.fields, "iconEmoji").to_string(),
            agent_name,
        });
    }
    out.sort_by(|a, b| {
        locale_cmp(&a.agent_name, &b.agent_name)
            .then_with(|| a.endpoint.agent_id.cmp(&b.endpoint.agent_id))
    });
    out
}

/// Who the composer starts tagged: the tags of the message just sent, else
/// every agent the newest person's message that tagged anyone tagged (in
/// the order written), else the object's default agents (`defaults`, in
/// property order). `person_messages` are chronological. Empty when none
/// of them is an option here.
pub(crate) fn seed_tags(
    options: &[ObjectAgentOption],
    person_messages: &[&str],
    defaults: &[String],
    just_sent: Vec<ObjectAgentOption>,
) -> Vec<ObjectAgentOption> {
    if !just_sent.is_empty() {
        return just_sent;
    }
    for text in person_messages.iter().rev() {
        let mut tagged: Vec<(usize, &ObjectAgentOption)> = options
            .iter()
            .filter_map(|o| tag_at(text, &o.agent_name).map(|at| (at, o)))
            .collect();
        if !tagged.is_empty() {
            tagged.sort_by_key(|(at, _)| *at);
            return tagged.into_iter().map(|(_, o)| o.clone()).collect();
        }
    }
    defaults
        .iter()
        .filter_map(|id| options.iter().find(|o| &o.endpoint.agent_id == id))
        .cloned()
        .collect()
}

/// Where `text` first @-tags `name` as a whole name ("@Ann" is not in "@Anne").
fn tag_at(text: &str, name: &str) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    let label = format!("@{name}");
    text.match_indices(&label)
        .find(|(at, _)| {
            !text[at + label.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
        .map(|(at, _)| at)
}

/// `localeCompare` for names: case-insensitive first, then exact.
fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// How many exchange threads `objectThreads` lists: every conversation and
/// every mailbox thread on the object except the human discussion.
pub(crate) fn object_thread_count(object: &Object) -> usize {
    let mut ids: BTreeSet<&str> = object
        .conversations
        .iter()
        .flatten()
        .map(|c| c.id.as_str())
        .collect();
    let entries = mailbox_entries(object, None);
    let threads: Vec<&str> = entries.iter().map(|e| e.thread_id.as_str()).collect();
    ids.extend(threads);
    ids.remove(DISCUSSION);
    ids.len()
}

/// A display name for any id: object summaries, then spaces, then agents.
pub(crate) fn name_of(store: &Store, id: &str) -> String {
    if let Some(s) = store.summary(id)
        && !s.name.is_empty()
    {
        return s.name.clone();
    }
    if let Some(s) = store.space(id)
        && !s.name.is_empty()
    {
        return s.name.clone();
    }
    store.agent(id).map(|a| a.name.clone()).unwrap_or_default()
}

/// `endpointName`: "Object · Agent", or just the object when they agree.
pub(crate) fn endpoint_name(store: &Store, endpoint: &AgentEndpoint) -> String {
    let mut object_name = name_of(store, &endpoint.object_id);
    if object_name.is_empty() {
        object_name = endpoint.object_id.chars().take(8).collect();
    }
    let agent_name = if endpoint.agent_id.is_empty() {
        String::new()
    } else {
        name_of(store, &endpoint.agent_id)
    };
    if !agent_name.is_empty() && agent_name != object_name {
        format!("{object_name} · {agent_name}")
    } else {
        object_name
    }
}

/// `loadObjectAgents`: the agents of the object's space, as @-options.
pub(crate) fn load_object_agents(
    object: &Object,
    s: &State,
) -> Task<anyhow::Result<Vec<ObjectAgentOption>>> {
    let space_id = if object.type_key == "channel" {
        object.id.clone()
    } else {
        object.channel().to_string()
    };
    let rows = roostr_core::api::query_all(
        json!({ "type": "agent", "filters": [{ "key": "channel", "condition": "equal", "value": space_id }] }),
        s,
    );
    let (object_id, type_key, fields, object_name) = (
        object.id.clone(),
        object.type_key.clone(),
        object.fields.clone(),
        object.name().to_string(),
    );
    s.rt.spawn(async move |cx| {
        let rows = rows.await?;
        Ok(cx.read(|s| {
            object_agent_options(&object_id, &type_key, &fields, &rows, |id| {
                if id == object_id {
                    object_name.clone()
                } else {
                    name_of(&s.store, id)
                }
            })
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(value: serde_json::Value) -> Fields {
        serde_json::from_value(value).expect("fields fixture")
    }

    fn agent(id: &str, name: &str, extra: &[(&str, &str)]) -> QueryRow {
        let mut f = Fields::from([
            ("channel".to_string(), roostr_core::Value::string("space")),
            ("name".to_string(), roostr_core::Value::string(name)),
        ]);
        for (k, v) in extra {
            f.insert(k.to_string(), roostr_core::Value::string(*v));
        }
        QueryRow {
            id: id.into(),
            type_key: "agent".into(),
            fields: f,
            ..Default::default()
        }
    }

    fn options(
        id: &str,
        type_key: &str,
        f: &Fields,
        records: &[QueryRow],
    ) -> Vec<ObjectAgentOption> {
        object_agent_options(id, type_key, f, records, |id| id.to_string())
    }

    fn endpoint(object_id: &str, agent_id: &str) -> AgentEndpoint {
        AgentEndpoint {
            object_id: object_id.into(),
            agent_id: agent_id.into(),
        }
    }

    fn agents() -> [QueryRow; 4] {
        [
            agent("zed", "Zed", &[]),
            agent("amy", "Amy", &[]),
            agent("sub", "Sub", &[("spawn_parent", "amy")]),
            agent("foreign", "Foreign", &[("channel", "elsewhere")]),
        ]
    }

    #[test]
    fn every_agent_of_the_space_is_addressable_on_every_object() {
        let agents = agents();
        let ids = |id: &str, type_key: &str, f: &Fields| -> Vec<String> {
            options(id, type_key, f, &agents)
                .into_iter()
                .map(|o| o.endpoint.agent_id)
                .collect()
        };
        // No default agent: still everyone of the space, never a helper or another space's.
        let plain = fields(serde_json::json!({ "channel": { "stringValue": "space" } }));
        assert_eq!(ids("obj", "task", &plain), ["amy", "zed"]);
        let endpoints: Vec<AgentEndpoint> = options("obj", "task", &plain, &agents)
            .into_iter()
            .map(|o| o.endpoint)
            .collect();
        assert_eq!(endpoints, [endpoint("obj", "amy"), endpoint("obj", "zed")]);
        // A default agent is listed once, and a missing one not at all.
        let defaults = fields(serde_json::json!({
            "channel": { "stringValue": "space" },
            "agent": { "valuesValue": { "items": [{ "stringValue": "zed" }, { "stringValue": "missing" }] } },
        }));
        assert_eq!(ids("obj", "task", &defaults), ["amy", "zed"]);
        // The space page: its own agents.
        assert_eq!(ids("space", "channel", &Fields::new()), ["amy", "zed"]);
        // An agent's own page addresses the agent at home.
        let home: Vec<AgentEndpoint> = options("amy", "agent", &Fields::new(), &agents)
            .into_iter()
            .map(|o| o.endpoint)
            .collect();
        assert_eq!(home, [endpoint("amy", "amy")]);
    }

    #[test]
    fn the_composer_starts_with_what_was_just_sent_then_the_last_tags_then_the_default_agents() {
        let agents = [
            agent("ann", "Ann", &[]),
            agent("anne", "Anne", &[]),
            agent("zed", "Zed", &[]),
        ];
        let plain = fields(serde_json::json!({ "channel": { "stringValue": "space" } }));
        let opts = options("obj", "task", &plain, &agents);
        let ids = |tags: Vec<ObjectAgentOption>| -> Vec<String> {
            tags.into_iter().map(|o| o.endpoint.agent_id).collect()
        };
        let defaults = ["zed".to_string(), "ann".to_string()];
        // Nothing said yet: every default agent, in property order.
        assert_eq!(
            ids(seed_tags(&opts, &[], &defaults, Vec::new())),
            ["zed", "ann"]
        );
        // A person's tags win over the defaults, all of them, in the order written; "@Anne" is not "@Ann".
        let said = ["@Zed hi", "thanks", "@Anne and @Zed, look"];
        assert_eq!(
            ids(seed_tags(&opts, &said, &defaults, Vec::new())),
            ["anne", "zed"]
        );
        // The message just sent wins over both.
        let sent = vec![opts[0].clone()];
        assert_eq!(ids(seed_tags(&opts, &said, &defaults, sent)), ["ann"]);
        // No default agent and no tags: an empty composer.
        assert!(seed_tags(&opts, &["hello"], &[], Vec::new()).is_empty());
    }
}

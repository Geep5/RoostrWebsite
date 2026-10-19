//! Pure projections of the conversations stored in an object's own DAG
//! (`src/lib/threads.ts`), plus the names and recipient discovery of
//! `src/lib/conversations.ts`.

use std::collections::{BTreeSet, HashSet};

use gpui_kit::{App, Task};
use roostr_core::{AgentEndpoint, Fields, Object, QueryRow, Store, guest_agents};
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

/// Who may be @-addressed on `object`: the agents on its guest list (its
/// `agent` property), each as an endpoint on this object. A space's guest
/// list is its own agents; an agent's own page tags the agent (its home
/// endpoint). `records` are the agent objects to name.
pub(crate) fn object_agent_options(
    object_id: &str,
    type_key: &str,
    fields: &Fields,
    records: &[QueryRow],
    name_of: impl Fn(&str) -> String,
) -> Vec<ObjectAgentOption> {
    let guests: Vec<String> = match type_key {
        "channel" => records
            .iter()
            .filter(|r| {
                roostr_core::field_str(&r.fields, "channel") == object_id
                    && roostr_core::field_str(&r.fields, "spawn_parent").is_empty()
            })
            .map(|r| r.id.clone())
            .collect(),
        "agent" => std::iter::once(object_id.to_string())
            .chain(guest_agents(fields))
            .collect(),
        _ => guest_agents(fields),
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
    cx: &mut App,
) -> Task<anyhow::Result<Vec<ObjectAgentOption>>> {
    let space_id = if object.type_key == "channel" {
        object.id.clone()
    } else {
        object.channel().to_string()
    };
    let rows = roostr_core::api::query_all(
        json!({ "type": "agent", "filters": [{ "key": "channel", "condition": "equal", "value": space_id }] }),
        cx,
    );
    let (object_id, type_key, fields, object_name) = (
        object.id.clone(),
        object.type_key.clone(),
        object.fields.clone(),
        object.name().to_string(),
    );
    cx.spawn(async move |cx| {
        let rows = rows.await?;
        Ok(cx.update(|cx| {
            let store = Store::global(cx);
            let store = store.read(cx);
            object_agent_options(&object_id, &type_key, &fields, &rows, |id| {
                if id == object_id {
                    object_name.clone()
                } else {
                    name_of(store, id)
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

    #[test]
    fn addressable_agents_are_the_guest_list_and_a_space_lists_its_own_agents() {
        let agents = [
            agent("zed", "Zed", &[]),
            agent("amy", "Amy", &[]),
            agent("sub", "Sub", &[("spawn_parent", "amy")]),
            agent("foreign", "Foreign", &[("channel", "elsewhere")]),
        ];
        let list = fields(serde_json::json!({
            "agent": { "valuesValue": { "items": [{ "stringValue": "zed" }, { "stringValue": "amy" }, { "stringValue": "missing" }] } },
        }));
        let named: Vec<(String, String)> = options("obj", "task", &list, &agents)
            .into_iter()
            .map(|o| (o.endpoint.object_id, o.agent_name))
            .collect();
        assert_eq!(
            named,
            [("obj".into(), "Amy".into()), ("obj".into(), "Zed".into())]
        );
        // The pre-list single string still reads as a one-agent guest list.
        let single = fields(serde_json::json!({ "agent": { "stringValue": "zed" } }));
        let endpoints: Vec<AgentEndpoint> = options("obj", "task", &single, &agents)
            .into_iter()
            .map(|o| o.endpoint)
            .collect();
        assert_eq!(endpoints, [endpoint("obj", "zed")]);
        // No guests → nobody to address; nothing falls back to a space default.
        assert!(options("obj", "task", &Fields::new(), &agents).is_empty());
        // A space's roster is its own non-sub agents, addressed on the space.
        let roster: Vec<String> = options("space", "channel", &Fields::new(), &agents)
            .into_iter()
            .map(|o| o.endpoint.agent_id)
            .collect();
        assert_eq!(roster, ["amy", "zed"]);
        // An agent's own page addresses the agent at home, with no Agent property needed.
        let home: Vec<AgentEndpoint> = options("amy", "agent", &Fields::new(), &agents)
            .into_iter()
            .map(|o| o.endpoint)
            .collect();
        assert_eq!(home, [endpoint("amy", "amy")]);
    }
}

//! The right pane's properties section (`PropertiesPane.svelte`): the
//! object's properties as rows — a small icon + the name on the left
//! (muted), the value on the right. Clicking a row opens its editor in a
//! popover under the row; checkboxes toggle in place; multi-value rows carry
//! a hover × per value, single-value rows a hover × that removes the
//! property. Grouped "How it runs", then "Properties".

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use roostr_core::objects::{object_icon, relation_emoji};
use roostr_core::options::status_icon;
use roostr_core::relations::{current_space_id, is_reserved_key, object_space_id, space_relations};
use roostr_core::serving::{
    MachineRow, machine_name, resolve_serving, served_by_machine_id, serving_copy,
};
use roostr_core::types::{field_str, guest_agents, is_builtin_tool, is_locked_tool, repeat_of};
use roostr_core::{
    Fields, Object, QueryRow, RelationDef, Route, State, StoreEvent, Value, api, nav,
};
use roostr_rt::Task;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Glyph, Theme, checkbox, checkbox_display, glyph, motion, radius, ty};
use rune::{Color, Cursor, Cx, El, Span, col, el, rich, row, text};
use serde_json::json;

use crate::capability::{
    capability_status, capability_status_badge, capability_status_text, open_capability_status,
};
use crate::credential_ui::{credential_status, open_credential_status};
use crate::credentials::{
    CONNECT_TIMEOUT_MS, CONNECT_TIMEOUT_TEXT, COOKIE_META, COOKIE_PREFIX, POLL,
    credential_status_badge, credential_status_text, is_secret_field, now_ms, read_fields,
};
use crate::project_folder::{open_project_folder, project_folder};
use crate::repeat_view::{repeat_is_open, repeat_view};
use crate::suggest::{PropertyPick, PropertySuggest, PropertySuggestState, SuggestEvent};
use crate::value::{PropertyValue, commit_pending, date_label, number_text, object_ids};
use crate::widgets::{below, enter, icon_text_el, placeholder, pop_shadow, prefix, tip, ui_glyph};

/// Types that never have an agent of their own (`AGENTLESS_TYPES`).
const AGENTLESS_TYPES: &[&str] = &[
    "agent",
    "channel",
    "relation",
    "skill",
    "tool",
    "credential",
    "program",
    "typescript",
    "json",
    "proto",
    "pinned_fact",
    "milestone",
    "machine",
];
const AGENT_PRIORITY: &[&str] = &[
    "served_by",
    "repo_path",
    "agent",
    "model",
    "prompt",
    "skills",
    "tools",
    "credentials",
    "capability",
    "chat_color",
];
const CREDENTIAL_PROPS: &[(&str, &str, &str, &str)] = &[
    ("status", "Status", "status", ""),
    ("error", "Error", "longtext", ""),
    ("served_by", "Served by", "object", ""),
    ("account", "Account", "shorttext", "🪪"),
    ("service", "Service", "shorttext", "🧩"),
    ("description", "Description", "longtext", ""),
    ("login_url", "Login page", "url", "🔗"),
    ("session_host", "Signed-in host", "shorttext", "🌐"),
    ("session_cookie", "Signed-in cookie", "shorttext", "🍪"),
];
const CAPABILITY_PROPS: &[(&str, &str, &str, &str)] = &[
    ("status", "Status", "status", ""),
    ("error", "Error", "longtext", ""),
    ("served_by", "Served by", "object", ""),
    ("key", "Key", "shorttext", "🧩"),
    ("checked_at", "Checked", "date", ""),
    ("description", "Description", "longtext", ""),
];
/// The serving computer's harness writes these on a capability: they show, never edit.
const OWN_READONLY: &[&str] = &["served_by", "key", "checked_at"];
const JEV_KEYS: &[&str] = &["jev_answer", "jev_writes"];
const HOW_KEYS: &[&str] = &[
    "served_by",
    "repo_path",
    "agent",
    "model",
    "prompt",
    "skills",
    "tools",
    "credentials",
    "chat_color",
    "assignee",
    "check_first",
    "repeat_instructions",
    "error",
    "capability",
];
const SYSTEM_KEYS: &[&str] = &[
    "done",
    "due_date",
    "status",
    "tag",
    "description",
    "url",
    "email",
    "phone",
    "created_date",
    "modified_date",
    "createdDate",
    "modifiedDate",
];
const TOOL_KEYS: &[&str] = &["tool_inputs", "tool_builtin", "tool_version"];
/// Types with no Repeat. Queries, collections and types repeat: their run
/// hands the agent what they hold.
const NO_REPEAT: &[&str] = &["channel", "chat", "relation", "template", "agent", "tool"];
const REPEAT_KEY: &str = "__repeat__";
const SKILL_HELP: &str = "Give this Skill an **Answer** to make it a Jev Skill: its page becomes the question (a numbered list of levels for a Score, bullets for a Choice, `Yes:` / `No:` lines for Yes or no), and agents that have it in their Skills run it with `jev_score`, filling **Writes to** (default: this Skill's name). They need a TypeSafe credential.";

fn prop_rank(key: &str) -> u8 {
    if SYSTEM_KEYS.contains(&key) {
        0
    } else if TOOL_KEYS.contains(&key) {
        1
    } else if JEV_KEYS.contains(&key) {
        2
    } else {
        3
    }
}

/// `plain(v, format)` for list formats: the ids/texts a value holds.
fn plain_list(v: Option<&Value>) -> Vec<String> {
    let Some(v) = v else { return vec![] };
    if let Some(s) = &v.string_value {
        return if s.is_empty() {
            vec![]
        } else {
            vec![s.clone()]
        };
    }
    if v.values_value.is_some() {
        return v
            .items()
            .iter()
            .map(|i| {
                i.string_value
                    .clone()
                    .or_else(|| i.link_value.as_ref().map(|l| l.target_id.clone()))
                    .unwrap_or_default()
            })
            .filter(|s| !s.is_empty())
            .collect();
    }
    if let Some(l) = &v.link_value {
        return if l.target_id.is_empty() {
            vec![]
        } else {
            vec![l.target_id.clone()]
        };
    }
    if let Some(l) = &v.list_value {
        return l.values.clone();
    }
    vec![]
}

/// `String(plain(v))` for scalar formats.
fn plain_text(v: Option<&Value>) -> String {
    let Some(v) = v else { return String::new() };
    if let Some(s) = &v.string_value {
        return s.clone();
    }
    if let Some(n) = v.int_value {
        return n.to_string();
    }
    if let Some(f) = v.float_value {
        return number_text(f);
    }
    if let Some(b) = v.bool_value {
        return b.to_string();
    }
    if v.values_value.is_some() || v.link_value.is_some() || v.list_value.is_some() {
        return plain_list(Some(v)).join(", ");
    }
    String::new()
}

/// A Jev note on a value: text, tooltip, the Skill that set it.
struct Judged {
    text: String,
    title: String,
    skill_id: String,
}

struct ToolInfo {
    name: String,
    error: String,
    builtin: bool,
}

struct SkillInfo {
    name: String,
    key: String,
    machines: Vec<String>,
}

/// One object's pane state (GPUI's `PropertiesPane` entity).
#[derive(Default)]
struct PaneState {
    editing: Option<String>,
    suggest: Option<PropertySuggestState>,
    /// The row the user just added: it enters once it arrives.
    entering: Option<String>,
    view_keys: HashSet<String>,
    serving: Option<(String, bool)>,
    machines: Vec<MachineRow>,
    skills: HashMap<String, SkillInfo>,
    tools: HashMap<String, ToolInfo>,
    tools_key: String,
    credentials: HashMap<String, QueryRow>,
    credentials_key: String,
    gone_agents: HashSet<String>,
    missing_agents_key: String,
    /// The object state the async lookups were last run for.
    loaded_for: Option<(String, i64)>,
    asking: String,
    ask_error: String,
    connect_poll: Option<Task<()>>,
    connect_poll_error: String,
}

/// Every open pane, by object id.
#[derive(Default)]
struct Panes(HashMap<String, PaneState>);

fn pane_mut<'a>(s: &'a mut State, id: &str) -> Option<&'a mut PaneState> {
    s.slot::<Panes>().0.get_mut(id)
}

/// Opens `object_id`'s pane state (loads the object into the store cache
/// and starts its lookups). Idempotent.
pub fn open_pane(s: &mut State, object_id: &str) {
    s.store.ensure_object(object_id);
    s.slot::<Panes>()
        .0
        .entry(object_id.to_string())
        .or_default();
    refresh_lookups(s, object_id);
}

/// Drops `object_id`'s pane state (its polls stop).
pub(crate) fn forget(s: &mut State, object_id: &str) {
    s.slot::<Panes>().0.remove(object_id);
}

/// A reconnect may have changed anything: every pane re-runs its lookups.
pub(crate) fn register(s: &mut State) {
    s.on_store(on_store);
}

fn on_store(s: &mut State, ev: &StoreEvent) {
    if matches!(ev, StoreEvent::Reconnected) {
        for pane in s.slot::<Panes>().0.values_mut() {
            pane.loaded_for = None;
        }
    }
}

fn type_key_of(object: &Object, s: &State) -> String {
    if object.type_key != "template" {
        return object.type_key.clone();
    }
    let target = object.field_str("target_type");
    s.store
        .types
        .iter()
        .find(|t| t.id == target)
        .map_or_else(|| object.type_key.clone(), |t| t.key.clone())
}

/// Re-run the lookups that depend on the object (views, serving, skills, tools, credentials, agents).
fn refresh_lookups(s: &mut State, id: &str) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    let key = (object.id.clone(), object.updated_at);
    let Some(pane) = pane_mut(s, id) else { return };
    if pane.loaded_for.as_ref() == Some(&key) {
        return;
    }
    pane.loaded_for = Some(key);
    let space = object_space_id(&s.store, &object.fields);

    // Columns and filters of the views this object belongs to count as its
    // properties: a query filtering on Priority shows Priority on its objects.
    let views = api::query_all(
        json!({ "filters": [{ "key": "typeKey", "condition": "in", "value": ["query", "set", "collection"] }, { "key": "channel", "condition": "equal", "value": space }] }),
        s,
    );
    let (oid, type_key) = (object.id.clone(), object.type_key.clone());
    s.rt.spawn(async move |cx| {
        let mut keys = HashSet::new();
        if let Ok(views) = views.await {
            for v in views {
                if v.id == oid {
                    continue;
                }
                let items = |k: &str| {
                    v.fields
                        .get(k)
                        .map(|x| x.items().to_vec())
                        .unwrap_or_default()
                };
                let member = if v.type_key == "collection" {
                    items("collectionIds")
                        .iter()
                        .any(|i| i.as_str() == Some(oid.as_str()))
                } else {
                    items("setOf")
                        .iter()
                        .any(|i| i.as_str() == Some(type_key.as_str()))
                };
                if !member {
                    continue;
                }
                for i in items("viewRelations") {
                    let k = i
                        .string_value
                        .clone()
                        .or_else(|| {
                            i.entries()
                                .and_then(|e| e.get("key"))
                                .and_then(|k| k.string_value.clone())
                        })
                        .unwrap_or_default();
                    if !k.is_empty() {
                        keys.insert(k);
                    }
                }
                // A filter's property (only real properties make rows: `serving`
                // and the like match none).
                for rule in items("viewFilters") {
                    if let Some(k) = rule
                        .entries()
                        .and_then(|e| e.get("key"))
                        .and_then(|k| k.string_value.clone())
                        .filter(|k| !k.is_empty())
                    {
                        keys.insert(k);
                    }
                }
            }
        }
        cx.update(|s| {
            if let Some(p) = pane_mut(s, &oid) {
                p.view_keys = keys;
            }
        });
    })
    .detach();

    // Serving (served_by names the machine and carries the warning), then skills.
    let serving = resolve_serving(&object.id, s);
    let skills = api::query_all(json!({ "type": "skill" }), s);
    let caps = api::query_all(json!({ "type": "capability" }), s);
    let oid = object.id.clone();
    s.rt.spawn(async move |cx| {
        let resolved = serving.await;
        let machines = resolved
            .as_ref()
            .map(|(_, m)| m.clone())
            .unwrap_or_default();
        cx.update(|s| {
            if let Some(p) = pane_mut(s, &oid) {
                p.serving = resolved.as_ref().ok().map(|(sv, m)| {
                    let copy = serving_copy(sv, m);
                    (copy.text, copy.warning)
                });
                p.machines = machines.clone();
            }
        });
        let (Ok(skills), Ok(caps)) = (skills.await, caps.await) else {
            return;
        };
        let mut working: HashMap<String, Vec<String>> = HashMap::new();
        for c in &caps {
            let m = served_by_machine_id(&c.fields, &machines);
            let key = field_str(&c.fields, "key").to_string();
            if m.is_empty() || key.is_empty() || field_str(&c.fields, "status") != "active" {
                continue;
            }
            let name = machine_name(&machines, &m);
            let list = working.entry(key).or_default();
            if !list.contains(&name) {
                list.push(name);
            }
        }
        let map = skills
            .into_iter()
            .map(|sk| {
                let key = field_str(&sk.fields, "key").to_string();
                let name = match field_str(&sk.fields, "name") {
                    "" if key.is_empty() => "Skill".to_string(),
                    "" => key.clone(),
                    n => n.to_string(),
                };
                let machines = if key.is_empty() {
                    vec![]
                } else {
                    working.get(&key).cloned().unwrap_or_default()
                };
                (
                    sk.id,
                    SkillInfo {
                        name,
                        key,
                        machines,
                    },
                )
            })
            .collect();
        cx.update(|s| {
            if let Some(p) = pane_mut(s, &oid) {
                p.skills = map;
            }
        });
    })
    .detach();

    // The Tool objects the tools row links, so a broken one shows as such.
    let tool_ids = plain_list(object.field("tools"));
    let tools_key = tool_ids.join(",");
    let pane = pane_mut(s, id).expect("open pane");
    if tools_key != pane.tools_key || pane.tools.is_empty() {
        pane.tools_key = tools_key;
        let task = (!tool_ids.is_empty()).then(|| {
            api::query_all(
                json!({ "filters": [{ "key": "id", "condition": "in", "value": tool_ids }] }),
                s,
            )
        });
        let oid = object.id.clone();
        s.rt.spawn(async move |cx| {
            let rows = match task {
                Some(t) => t.await.unwrap_or_default(),
                None => vec![],
            };
            let map = rows
                .into_iter()
                .map(|r| {
                    let name = match field_str(&r.fields, "name") {
                        "" => "Tool".to_string(),
                        n => n.to_string(),
                    };
                    let info = ToolInfo {
                        name,
                        error: field_str(&r.fields, "error").to_string(),
                        builtin: is_builtin_tool(&r.fields),
                    };
                    (r.id, info)
                })
                .collect();
            cx.update(|s| {
                if let Some(p) = pane_mut(s, &oid) {
                    p.tools = map;
                }
            });
        })
        .detach();
    }

    // The credential objects the credentials row links, for their status.
    let cred_ids = plain_list(object.field("credentials"));
    let cred_key = cred_ids.join(",");
    let pane = pane_mut(s, id).expect("open pane");
    if cred_key != pane.credentials_key || pane.credentials.is_empty() {
        pane.credentials_key = cred_key;
        let task = (!cred_ids.is_empty()).then(|| {
            api::query_all(
                json!({ "filters": [{ "key": "id", "condition": "in", "value": cred_ids }] }),
                s,
            )
        });
        let oid = object.id.clone();
        s.rt.spawn(async move |cx| {
            let rows = match task {
                Some(t) => t.await.unwrap_or_default(),
                None => vec![],
            };
            cx.update(|s| {
                if let Some(p) = pane_mut(s, &oid) {
                    p.credentials = rows.into_iter().map(|r| (r.id.clone(), r)).collect();
                }
            });
        })
        .detach();
    }

    // Agents the agent row links that this device does not list: deleted, or not downloaded yet.
    let missing: Vec<String> = plain_list(object.field("agent"))
        .into_iter()
        .filter(|a| s.store.agent(a).is_none())
        .collect();
    let missing_key = missing.join(",");
    let pane = pane_mut(s, id).expect("open pane");
    if missing_key != pane.missing_agents_key {
        pane.missing_agents_key = missing_key;
        let ledger = s.store.fetch_object("__vanished__");
        let fetches: Vec<_> = missing
            .iter()
            .map(|a| (a.clone(), s.store.fetch_object(a)))
            .collect();
        let oid = object.id.clone();
        s.rt.spawn(async move |cx| {
            let ledger = ledger.await.map(|o| o.fields).unwrap_or_default();
            let mut gone = HashSet::new();
            for (a, fetch) in fetches {
                if ledger.contains_key(&format!("vanished:{a}")) {
                    gone.insert(a);
                    continue;
                }
                if let Ok(o) = fetch.await
                    && o.deleted
                {
                    gone.insert(a);
                }
            }
            cx.update(|s| {
                if let Some(p) = pane_mut(s, &oid) {
                    p.gone_agents = gone;
                }
            });
        })
        .detach();
    }

    // While a credential's sign-in window is open its computer writes the status: follow it.
    let is_credential = type_key_of(&object, s) == "credential";
    let connecting = is_credential && object.field_str("status") == "connecting";
    let pane = pane_mut(s, id).expect("open pane");
    if connecting && pane.connect_poll.is_none() {
        pane.connect_poll_error.clear();
        let oid = object.id.clone();
        let mut last: Fields = object.fields.clone();
        let deadline = now_ms() + CONNECT_TIMEOUT_MS;
        let task = s.rt.spawn(async move |cx| {
            loop {
                cx.timer(POLL).await;
                if now_ms() > deadline {
                    cx.update(|s| {
                        if let Some(p) = pane_mut(s, &oid) {
                            p.connect_poll_error = CONNECT_TIMEOUT_TEXT.into();
                            // This task is the one running: let it finish.
                            if let Some(t) = p.connect_poll.take() {
                                t.detach();
                            }
                        }
                    });
                    return;
                }
                let read = cx.update(|s| read_fields(&oid, s));
                let Some(fresh) = read.await else { continue };
                if ["status", "error", "checked_at", "auth"]
                    .iter()
                    .any(|k| fresh.get(*k) != last.get(*k))
                {
                    last = fresh.clone();
                    cx.update(|s| crate::credentials::changed(&oid, s));
                }
                if field_str(&fresh, "status") != "connecting" {
                    cx.update(|s| {
                        if let Some(t) = pane_mut(s, &oid).and_then(|p| p.connect_poll.take()) {
                            t.detach();
                        }
                    });
                    return;
                }
            }
        });
        pane_mut(s, id).expect("open pane").connect_poll = Some(task);
    } else if !connecting {
        pane.connect_poll = None;
    }
}

/// The credential's / capability's own rows (stand-in defs until the harness seeds them).
fn own_rows(object: &Object, type_key: &str, relations: &[RelationDef]) -> Vec<RelationDef> {
    let is_capability = type_key == "capability";
    if type_key != "credential" && !is_capability {
        return vec![];
    }
    let def = |key: &str, name: &str, format: &str, emoji: &str| {
        let mut rel = relations
            .iter()
            .find(|r| r.key == key)
            .cloned()
            .unwrap_or_else(|| RelationDef {
                id: format!("{type_key}-{key}"),
                key: key.into(),
                format: format.into(),
                name: name.into(),
                icon_emoji: (!emoji.is_empty()).then(|| emoji.to_string()),
                max_count: if format == "status" { 1 } else { 0 },
                ..Default::default()
            });
        if is_capability && OWN_READONLY.contains(&key) {
            rel.read_only = true;
        }
        rel
    };
    if is_capability {
        return CAPABILITY_PROPS
            .iter()
            .map(|(k, n, f, e)| def(k, n, f, e))
            .collect();
    }
    let mut rows: Vec<RelationDef> = CREDENTIAL_PROPS
        .iter()
        .map(|(k, n, f, e)| def(k, n, f, e))
        .collect();
    for key in object
        .fields
        .keys()
        .filter(|k| k.starts_with("key_") && *k != "key_fields")
    {
        let words = key["key_".len()..].replace('_', " ");
        let mut chars = words.chars();
        let name = chars
            .next()
            .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
            .unwrap_or_default();
        rows.push(def(key, &name, "shorttext", "🔑"));
    }
    for key in object
        .fields
        .keys()
        .filter(|k| k.starts_with(COOKIE_PREFIX) && *k != COOKIE_META)
    {
        rows.push(def(
            key,
            &format!("Cookie {}", &key[COOKIE_PREFIX.len()..]),
            "shorttext",
            "🍪",
        ));
    }
    if object.fields.contains_key(COOKIE_META) {
        rows.push(def(COOKIE_META, "Cookie details", "longtext", "🍪"));
    }
    rows
}

/// Featured order first, then the agent block, then the rest.
fn shown(
    pane: &PaneState,
    object: &Object,
    type_key: &str,
    relations: &[RelationDef],
    own_keys: &HashSet<String>,
) -> Vec<RelationDef> {
    let fields = &object.fields;
    let is_credential = type_key == "credential";
    let machine_bound = ["agent", "capability", "credential"].contains(&type_key);
    let agent_config: &[&str] = if type_key == "agent" {
        &[
            "prompt",
            "model",
            "skills",
            "tools",
            "credentials",
            "served_by",
            "repo_path",
            "chat_color",
        ]
    } else {
        &[]
    };
    let tool_config: &[&str] = if type_key == "tool" {
        &["description", "tool_inputs", "tool_builtin", "tool_version"]
    } else {
        &[]
    };
    let skill_config: &[&str] = if type_key == "skill" && field_str(fields, "key").is_empty() {
        JEV_KEYS
    } else {
        &[]
    };
    let repeating = repeat_of(fields).is_some();
    let has = |k: &str| fields.contains_key(k);
    let mut present: Vec<RelationDef> = relations
        .iter()
        .filter(|r| {
            let k = r.key.as_str();
            if own_keys.contains(k) || (is_credential && (k == "key_fields" || k == "secret")) {
                return false;
            }
            if is_reserved_key(k) {
                return false;
            }
            if agent_config.contains(&k) || tool_config.contains(&k) || skill_config.contains(&k) {
                return true;
            }
            match k {
                "error" => object.type_key != "template",
                _ if type_key == "channel" => k == "agent",
                "keep_all_files" | "autostart" => type_key == "machine" || has(k),
                // Where its agents work on it: shown once it has an agent.
                "repo_path" => !guest_agents(fields).is_empty() || has(k),
                "served_by" => machine_bound || has(k),
                "credentials" | "skills" | "tools" => has(k),
                "check_first" => repeating || has(k),
                // What each run asks of the agent: wherever runs happen
                // (a Repeat, or pushes into a Check first).
                "repeat_instructions" => repeating || has("check_first") || has(k),
                "agent" => !AGENTLESS_TYPES.contains(&type_key) || has(k),
                "model" => type_key == "agent" || has(k),
                _ => !r.hidden && (has(k) || pane.view_keys.contains(k)),
            }
        })
        .cloned()
        .collect();
    let featured: Vec<String> = fields
        .get("featuredRelations")
        .map(|v| {
            v.items()
                .iter()
                .filter_map(|i| i.string_value.clone())
                .collect()
        })
        .unwrap_or_default();
    let rank = |k: &str| featured.iter().position(|f| f == k).unwrap_or(999);
    let agent_rank = |k: &str| AGENT_PRIORITY.iter().position(|f| *f == k).unwrap_or(999);
    present.sort_by_key(|r| (rank(&r.key), agent_rank(&r.key)));
    present
}

/// The "Properties" group: the type's own rows, then the shown rows outside "How it runs",
/// the system properties first; within a rank they keep the space's relation order (the
/// backend's list order), as GPUI's pane did.
fn property_rows(own: &[RelationDef], shown: &[RelationDef]) -> Vec<RelationDef> {
    let mut props: Vec<RelationDef> = own
        .iter()
        .filter(|r| !HOW_KEYS.contains(&r.key.as_str()))
        .cloned()
        .collect();
    let mut rest: Vec<RelationDef> = shown
        .iter()
        .filter(|r| !HOW_KEYS.contains(&r.key.as_str()))
        .cloned()
        .collect();
    rest.sort_by_key(|r| prop_rank(&r.key));
    props.extend(rest);
    props
}

fn judged(s: &State, object: &Object, key: &str) -> Option<Judged> {
    let e = object.field("judged")?.entries()?.get(key)?.entries()?;
    let text = if let Some(p) = e.get("probability").and_then(|v| v.float_value) {
        format!("{}% yes", (p * 100.).round())
    } else {
        let c = e.get("confidence").and_then(|v| v.float_value)?;
        format!("{}% sure", (c * 100.).round())
    };
    let skill_id = e
        .get("skill")
        .and_then(|v| v.string_value.clone())
        .unwrap_or_default();
    let skill = s
        .store
        .summary(&skill_id)
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "a Jev Skill".into());
    let agent = e
        .get("agent")
        .and_then(|v| v.as_str())
        .and_then(|a| s.store.agent(a))
        .map(|a| a.name.clone());
    let at = e.get("at").and_then(|v| v.int_value).filter(|a| *a != 0);
    let title = format!(
        "Set by {skill}{}{}",
        agent.map(|a| format!(" ({a})")).unwrap_or_default(),
        at.map(|a| format!(" · {}", crate::repeat_view::fmt_long(a)))
            .unwrap_or_default()
    );
    Some(Judged {
        text,
        title,
        skill_id,
    })
}

fn ask_again(s: &mut State, id: &str, key: &str) {
    let Some(h) = s.harness.clone() else {
        if let Some(p) = pane_mut(s, id) {
            p.ask_error = "Roostr isn't running on this computer.".into();
        }
        return;
    };
    let Some(p) = pane_mut(s, id) else { return };
    p.asking = key.to_string();
    p.ask_error.clear();
    let body = json!({ "object": id, "key": key });
    let task =
        s.rt.background(async move { h.post_json("/jev/again", &body).await });
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            let Some(p) = pane_mut(s, &id) else { return };
            p.asking.clear();
            match result {
                Err(e) => p.ask_error = e.to_string(),
                Ok(_) => crate::credentials::changed(&id, s),
            }
        });
    })
    .detach();
}

fn save_value(s: &mut State, id: &str, key: &str, value: Value) {
    // served_by is a pin to a machine_id string, not a link to the machine object.
    let mut value = value;
    if key == "served_by" {
        let target = value
            .link_value
            .as_ref()
            .map(|l| l.target_id.clone())
            .or_else(|| {
                value
                    .items()
                    .first()
                    .and_then(|i| i.link_value.as_ref())
                    .map(|l| l.target_id.clone())
            })
            .unwrap_or_default();
        if !target.is_empty()
            && let Some(m) = s
                .get::<Panes>()
                .and_then(|p| p.0.get(id))
                .and_then(|p| p.machines.iter().find(|m| m.id == target))
        {
            value = Value::string(m.machine_id.clone());
        }
    }
    let task = api::note::set_field(id, key, value, s);
    task.detach_and_log_err(&s.rt);
}

/// Remove one value from a list property, or the property when it was the last.
fn remove_value(s: &mut State, id: &str, key: &str, value: &str) {
    if let Some(p) = pane_mut(s, id) {
        p.editing = None;
    }
    let Some(object) = s.store.object(id) else {
        return;
    };
    let items = object
        .field(key)
        .map(|v| v.items().to_vec())
        .unwrap_or_default();
    let next: Vec<Value> = items
        .iter()
        .filter(|i| {
            i.string_value
                .clone()
                .or_else(|| i.link_value.as_ref().map(|l| l.target_id.clone()))
                .unwrap_or_default()
                != value
        })
        .cloned()
        .collect();
    let task = if !next.is_empty() && items.len() > 1 {
        api::note::set_field(id, key, Value::values(next), s)
    } else {
        api::note::delete_field(id, key, s)
    };
    task.detach_and_log_err(&s.rt);
}

/// Close the open value box, saving what is typed (a text box otherwise
/// saves only when it loses focus): the box being edited commits, then
/// the box closes.
fn finish_editing(s: &mut State, id: &str) {
    commit_pending(s);
    if let Some(p) = pane_mut(s, id) {
        p.editing = None;
    }
}

fn remove_prop(s: &mut State, id: &str, key: &str) {
    if let Some(p) = pane_mut(s, id) {
        p.editing = None;
    }
    let task = api::note::delete_field(id, key, s);
    task.detach_and_log_err(&s.rt);
}

fn empty_value(format: &str) -> Value {
    match format {
        "checkbox" => Value::bool(false),
        "number" => Value::float(0.),
        "date" => Value::int(0),
        "tag" | "object" | "agent" | "credentials" | "skills" => Value::values(vec![]),
        _ => Value::string(""),
    }
}

fn open_suggest(s: &mut State, id: &str, exclude: Vec<String>) {
    let space = current_space_id(s);
    if let Some(p) = pane_mut(s, id) {
        p.suggest = Some(PropertySuggestState::new(space, exclude, vec![]));
    }
}

fn on_suggest(s: &mut State, id: &str, ev: SuggestEvent<PropertyPick>) {
    let Some(p) = pane_mut(s, id) else { return };
    p.suggest = None;
    if let SuggestEvent::Picked(PropertyPick::Relation(rel)) = ev {
        // Open the new row's editor so the value can be set straight away.
        p.editing = Some(rel.key.clone());
        p.entering = Some(rel.key.clone());
        let task = api::note::set_field(id, &rel.key, empty_value(&rel.format), s);
        task.detach_and_log_err(&s.rt);
    }
}

fn toggle_edit(s: &mut State, id: &str, key: &str, type_key: &str) {
    let Some(p) = pane_mut(s, id) else { return };
    if p.editing.as_deref() == Some(key) {
        p.editing = None;
        return;
    }
    p.editing = Some(key.to_string());
    if key == "status" && type_key == "credential" {
        open_credential_status(s, id);
    }
    if key == "status" && type_key == "capability" {
        open_capability_status(s, id);
    }
    if key == "repo_path" {
        open_project_folder(s, id);
    }
}

/// The hover reveal of a chip's or row's × (`.rm { opacity: 0; transition:
/// opacity 100ms }`, shown while the nearest `.hover_group()` is hovered).
fn reveal(e: El) -> El {
    e.group_opacity(0.0, 1.0)
}

/// A hover group whose reveal follows `HOVER_REVEAL` both ways.
fn reveal_group(e: El) -> El {
    e.hover_group()
        .hover_motion(motion::HOVER_REVEAL.anim(), motion::HOVER_REVEAL.anim())
}

/// A chip's ×: a 15pt round ghost button on the panel colour, pinned to the
/// chip's top-right corner and revealed by the chip's hover; its glyph turns
/// red while it is hovered.
fn chip_rm(t: &Theme, key: String, label: String, on_press: impl Fn(&mut State) + 'static) -> El {
    let button = row()
        .key(key)
        .role("button")
        .label(label.clone())
        .size(15.0, 15.0)
        .center()
        .radius(7.5)
        .fill(t.panel)
        .border(1.0, t.border)
        .hover(ghost_hover(t))
        .hover_group()
        .cursor(Cursor::Pointer)
        .on_press(on_press)
        .child(glyph(Glyph::Close, 9.0).color(t.muted).group_color(t.red));
    reveal(
        el().pin(None, Some(-6.0), Some(-6.0), None)
            .child(tip(button, label)),
    )
}

/// The kit ghost button's hover fill.
fn ghost_hover(t: &Theme) -> Color {
    if t.dark {
        t.kit.accent.alpha(0.5)
    } else {
        t.kit.accent
    }
}

/// A neutral chip (kit `Tag` look): plain text on a quiet outline, radius 10,
/// 0 8px, 12px on a 20px line; colour only for problems (`tone`). With
/// `tint`, the outline turns muted while the chip is hovered (chips that open
/// something).
fn chip(t: &Theme, key: String, tone: Color, tint: bool, lead: El, name: String) -> El {
    let mut c = row()
        .key(key)
        .items_center()
        .gap(4.0)
        .min_w(0.0)
        .shrink()
        .px(8.0)
        .radius(radius::MENU)
        .border(1.0, t.border)
        .child(lead)
        .child(
            text(name)
                .font(ty::ui(12.0).lh(20.0))
                .color(tone)
                .truncate()
                .shrink(),
        );
    if tint {
        c = reveal_group(c).child(reveal(
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radius(radius::MENU)
                .border(1.0, t.muted),
        ));
    }
    c
}

fn chip_icon(g: Glyph, color: Color) -> El {
    glyph(g, 12.0).color(color)
}

/// A status value: borderless, its option colour, 13px on a 22px line.
fn status_val(t: &Theme, label: String, color: &str, g: Glyph) -> El {
    let c = roostr_ui::tags::badge_colors(t, color);
    row()
        .items_center()
        .gap(5.0)
        .child(glyph(g, 14.0).color(c.icon))
        .child(text(label).font(ty::ui(13.0).lh(22.0)).color(c.fg))
}

/// The pane's tag pill (`.pill`) in the option's kit colours: 0 8px,
/// radius 10, 12px, 18px line, 1px border.
fn pane_pill(t: &Theme, label: String, color: &str) -> El {
    let paint = roostr_ui::tags::tag_colors(t, color)
        .unwrap_or_else(|| roostr_ui::tags::unknown_tag_paint(t.fg));
    row()
        .px(8.0)
        .radius(radius::MENU)
        .fill(paint.bg)
        .border(1.0, paint.border)
        .child(text(label).font(ty::ui(12.0).lh(18.0)).color(paint.fg))
}

/// The skill help's inline markdown (`**bold**`, `` `code` ``) as rich text.
fn help_text(t: &Theme, src: &str) -> El {
    let mut out = String::new();
    let mut spans = Vec::new();
    let mut rest = src;
    while !rest.is_empty() {
        let (mark, bold) = match (rest.find("**"), rest.find('`')) {
            (Some(b), Some(c)) if b < c => (b, true),
            (Some(b), None) => (b, true),
            (_, Some(c)) => (c, false),
            (None, None) => {
                out.push_str(rest);
                break;
            }
        };
        out.push_str(&rest[..mark]);
        let open = if bold { 2 } else { 1 };
        let after = &rest[mark + open..];
        let close = after
            .find(if bold { "**" } else { "`" })
            .unwrap_or(after.len());
        let start = out.len();
        out.push_str(&after[..close]);
        let span = Span::new(start, out.len());
        spans.push(if bold {
            span.weight(ty::SEMIBOLD)
        } else {
            Span {
                mono: true,
                bg: Some(t.kit.muted),
                bg_pad: 2.0,
                ..span
            }
        });
        rest = after.get(close + open..).unwrap_or("");
    }
    rich(out, spans)
        .font(ty::ui(12.0).lh(12.0 * 1.45))
        .color(t.muted)
}

/// The info alert under a key-less Skill's rows (kit `Alert::info`, xsmall).
fn skill_help(t: &Theme) -> El {
    let k = &t.kit;
    row()
        .key("skill-help")
        .role("alert")
        .items_start()
        .gap(6.0)
        .mx(4.0)
        .mt(6.0)
        .px(12.0)
        .py(6.0)
        .radius(radius::KIT)
        .fill(k.info.alpha(0.04))
        .border(1.0, k.info.alpha(0.3))
        .child(
            glyph(Glyph::Info, roostr_ui::rems(0.875))
                .color(t.muted)
                .mt(3.0),
        )
        .child(
            col()
                .key("skill-help-text")
                .grow(1.0)
                .shrink()
                .child(help_text(t, SKILL_HELP)),
        )
}

/// The properties section of `object_id` (the discussion drawer's
/// Properties column): content only, the host scrolls and pads it.
pub fn properties_pane(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
) -> El {
    let fallback = PaneState::default();
    let pane = match s.get::<Panes>().and_then(|p| p.0.get(object_id)) {
        Some(p) => p,
        None => {
            let id = object_id.to_string();
            crate::widgets::later(s, move |s| open_pane(s, &id));
            &fallback
        }
    };
    let Some(object) = s.store.object(object_id) else {
        return el();
    };
    if pane.loaded_for != Some((object.id.clone(), object.updated_at)) {
        let id = object_id.to_string();
        crate::widgets::later(s, move |s| refresh_lookups(s, &id));
    }
    let space = object_space_id(&s.store, &object.fields);
    let relations: Vec<RelationDef> = space_relations(&s.store.relations, &space)
        .into_iter()
        .cloned()
        .collect();
    let type_key = type_key_of(&object, s);
    let own = own_rows(&object, &type_key, &relations);
    let own_keys: HashSet<String> = own.iter().map(|r| r.key.clone()).collect();
    let shown = shown(pane, &object, &type_key, &relations, &own_keys);
    let locked = is_locked_tool(&object.fields);
    let can_repeat = !NO_REPEAT.contains(&object.type_key.as_str());
    let tag_seeds: Fields = relations
        .iter()
        .filter(|r| r.format == "tag")
        .filter_map(|r| object.field(&r.key).map(|v| (r.key.clone(), v.clone())))
        .collect();

    // Grouped display: "How it runs", then "Properties".
    let mut how: Vec<RelationDef> = own
        .iter()
        .chain(shown.iter())
        .filter(|r| HOW_KEYS.contains(&r.key.as_str()))
        .cloned()
        .collect();
    let take = |how: &mut Vec<RelationDef>, k: &str| {
        how.iter().position(|r| r.key == k).map(|i| how.remove(i))
    };
    let assignee = take(&mut how, "assignee");
    let check_first = take(&mut how, "check_first");
    // Repeat instructions show the moment Repeat is being set (its editor
    // open), not only once a rule is saved; a space missing the bundled
    // property (not synced yet) still gets the row.
    let editing_repeat = repeat_is_open(s, object_id);
    let instructions = take(&mut how, "repeat_instructions").or_else(|| {
        (can_repeat && (editing_repeat || repeat_of(&object.fields).is_some())).then(|| {
            relations
                .iter()
                .find(|r| r.key == "repeat_instructions")
                .cloned()
                .unwrap_or_else(|| RelationDef {
                    key: "repeat_instructions".into(),
                    name: "Repeat instructions".into(),
                    format: "longtext".into(),
                    ..Default::default()
                })
        })
    });
    let error = take(&mut how, "error");
    if can_repeat {
        how.push(RelationDef {
            key: REPEAT_KEY.into(),
            name: "Repeat".into(),
            ..Default::default()
        });
    }
    // Repeat instructions sit under Repeat: what each run should do.
    how.extend(instructions);
    how.extend(assignee);
    how.extend(check_first);
    how.extend(error);
    let props = property_rows(&own, &shown);

    let ctx = RowCx {
        object: &object,
        pane,
        type_key: &type_key,
        is_credential: type_key == "credential",
        is_capability: type_key == "capability",
        locked,
        space: &space,
        tag_seeds: &tag_seeds,
    };
    let mut root = col();
    for (label, rows) in [("How it runs", how), ("Properties", props)] {
        if rows.is_empty() {
            continue;
        }
        let mut group = col().pt(4.0).pb(6.0).child(
            el().px(6.0).pt(6.0).pb(4.0).child(
                text(label)
                    .font(ty::ui(12.0).weight(ty::MEDIUM))
                    .color(t.muted),
            ),
        );
        for rel in rows {
            group = group.child(render_row(s, cx, t, layer, &ctx, &rel));
        }
        root = root.child(group);
    }
    if object.type_key == "skill" && object.field_str("key").is_empty() {
        root = root.child(skill_help(t));
    }
    if !locked {
        let exclude: Vec<String> = shown.iter().map(|r| r.key.clone()).collect();
        let id = object.id.clone();
        root = root.child(
            row()
                .key("add-prop")
                .role("button")
                .label("Add property")
                .w_full()
                .padding(6.0)
                .radius(radius::SM)
                .hover(ghost_hover(t))
                .hover_group()
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut State| open_suggest(s, &id, exclude.clone()))
                .child(
                    text("＋ Add property")
                        .font(ty::ui(13.0))
                        .color(t.muted)
                        .group_color(t.fg),
                ),
        );
        if let Some(state) = &pane.suggest {
            let (lens_id, ev_id) = (object.id.clone(), object.id.clone());
            let menu = PropertySuggest::new("add-suggest", state)
                .on_state(move |s: &mut State| {
                    pane_mut(s, &lens_id)
                        .and_then(|p| p.suggest.as_mut())
                        .expect("the open suggest menu")
                })
                .on_event(move |s: &mut State, ev: SuggestEvent<PropertyPick>| {
                    on_suggest(s, &ev_id, ev)
                })
                .build(s, cx, t, layer);
            below(layer, "add-prop", 4.0, menu);
        }
    }
    root
}

/// What every row of one pane draws from.
struct RowCx<'a> {
    object: &'a Object,
    pane: &'a PaneState,
    type_key: &'a str,
    is_credential: bool,
    is_capability: bool,
    locked: bool,
    space: &'a str,
    tag_seeds: &'a Fields,
}

/// Compact display string for a row (`display`).
fn display(s: &State, object: &Object, rel: &RelationDef) -> String {
    let v = object.field(&rel.key);
    match rel.format.as_str() {
        "checkbox" => if v.and_then(|v| v.bool_value) == Some(true) {
            "✓"
        } else {
            "✗"
        }
        .into(),
        "date" => v
            .and_then(Value::as_number)
            .filter(|ms| *ms != 0.)
            .map(|ms| date_label(ms as i64))
            .unwrap_or_default(),
        "object" => plain_list(v)
            .iter()
            .map(|id| {
                s.store
                    .summary(id)
                    .map(|o| o.name.clone())
                    .filter(|n| !n.is_empty())
                    .or_else(|| {
                        s.store
                            .agent(id)
                            .map(|a| a.name.clone())
                            .filter(|n| !n.is_empty())
                    })
                    .unwrap_or_else(|| prefix(id, 6))
            })
            .collect::<Vec<_>>()
            .join(", "),
        "tag" => plain_list(v).join(", "),
        "longtext" => prefix(&plain_text(v), 60),
        _ => plain_text(v),
    }
}

fn render_row(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    c: &RowCx,
    rel: &RelationDef,
) -> El {
    let object = c.object;
    let oid = object.id.clone();
    if rel.key == REPEAT_KEY {
        return repeat_view(s, cx, t, layer, &oid);
    }
    let key = rel.key.clone();
    let v = object.field(&key);
    let editable = !c.locked && !rel.read_only;
    let secret = c.is_credential && is_secret_field(&key);
    let row_glyph = if secret {
        Glyph::PropKey
    } else {
        roostr_ui::prop_line_icon(&key, &rel.format)
    };
    let label = if rel.name.is_empty() {
        key.clone()
    } else {
        rel.name.clone()
    };
    let empty_hint = match key.as_str() {
        "served_by" => "No machine yet",
        "repo_path" => "No project folder",
        "chat_color" => "Default grey",
        "credentials" => "No credentials",
        "agent" => "Add agent",
        "skills" => "No skills",
        "tools" => "No tools",
        "check_first" => "None",
        "repeat_instructions" => "What each run should do",
        _ => match rel.format.as_str() {
            "status" => "Select option",
            "tag" => "Select options",
            "date" => "Select a date",
            _ => "Empty",
        },
    };
    let empty = || {
        if editable {
            placeholder(t, empty_hint)
        } else {
            text("—").font(ty::ui(13.0)).color(t.muted).opacity(0.6)
        }
    };
    let shown_text = display(s, object, rel);
    let row_emoji = relation_emoji(
        &s.store,
        rel,
        object_ids(v).first().map(String::as_str),
        object.field_str("channel"),
    );
    let editing = c.pane.editing.as_deref() == Some(key.as_str());

    // ── The value ──
    let mut value = row()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .flex_wrap()
        .items_center()
        .gaps(4.0, 6.0)
        .min_h(22.0);
    let remover = |item: &str| {
        let (id, key, item) = (oid.clone(), key.clone(), item.to_string());
        move |s: &mut State| remove_value(s, &id, &key, &item)
    };
    let open_object = |target: &str| {
        let target = target.to_string();
        move |s: &mut State| nav::navigate(Route::Object(target.clone()), s)
    };
    // Values the user adds from this row's open editor enter; values
    // arriving while it is closed don't.
    let chip_wrap = |e: El| if editing { enter(e) } else { e };
    let list = plain_list(v);
    if c.is_capability && key == "status" {
        let status = object.field_str("status");
        let (g, color) = capability_status_badge(status);
        value = value.child(status_val(t, capability_status_text(status), color, g));
    } else if c.is_credential && key == "status" {
        let status = object.field_str("status");
        let (g, color) = credential_status_badge(status);
        value = value.child(status_val(t, credential_status_text(status), color, g));
    } else if rel.format == "checkbox" {
        let on = v.and_then(|v| v.bool_value) == Some(true);
        let (id, k) = (oid.clone(), key.clone());
        value = value.child(
            checkbox(format!("chk-{key}"), on)
                .disabled(!editable)
                .tooltip(label.clone())
                .on_change(move |s: &mut State, on: bool| save_value(s, &id, &k, Value::bool(on)))
                .build(t),
        );
    } else if rel.format == "tag" {
        if list.is_empty() {
            value = value.child(empty());
        }
        for item in &list {
            let color = rel
                .options
                .iter()
                .find(|o| &o.text == item)
                .map(|o| o.color.clone())
                .unwrap_or_default();
            let mut wrap = reveal_group(row().key(format!("tag-{key}-{item}"))).child(pane_pill(
                t,
                item.clone(),
                &color,
            ));
            if editable {
                wrap = wrap.child(chip_rm(
                    t,
                    format!("rm-{key}-{item}"),
                    format!("Remove {item}"),
                    remover(item),
                ));
            }
            value = value.child(chip_wrap(wrap));
        }
    } else if rel.format == "status" {
        if shown_text.is_empty() {
            value = value.child(empty());
        } else {
            let color = rel
                .options
                .iter()
                .find(|o| o.text == shown_text)
                .map(|o| o.color.clone())
                .unwrap_or_default();
            value = value.child(status_val(
                t,
                shown_text.clone(),
                &color,
                ui_glyph(status_icon(&shown_text)),
            ));
        }
    } else if key == "served_by" {
        match &c.pane.serving {
            Some((line, warning)) => {
                let line = line.strip_prefix("served by ").unwrap_or(line).to_string();
                let serving = text(line)
                    .key("serving")
                    .font(ty::ui(13.0).lh(22.0))
                    .color(if *warning { t.red } else { t.fg })
                    .truncate()
                    .shrink();
                value = value.child(if *warning {
                    tip(serving, "Cannot be honoured")
                } else {
                    serving
                });
            }
            None => value = value.child(empty()),
        }
    } else if key == "credentials" {
        if list.is_empty() {
            value = value.child(empty());
        }
        for id in &list {
            let (name, tone, title) = match c.pane.credentials.get(id) {
                None => (
                    s.store
                        .summary(id)
                        .map(|o| o.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| format!("{}…", prefix(id, 8))),
                    "",
                    "Credential".to_string(),
                ),
                Some(row) => {
                    let status = match field_str(&row.fields, "status") {
                        "" => "missing",
                        st => st,
                    };
                    let name = [
                        field_str(&row.fields, "name"),
                        field_str(&row.fields, "service"),
                    ]
                    .into_iter()
                    .find(|x| !x.is_empty())
                    .unwrap_or("Credential")
                    .to_string();
                    let who = [
                        field_str(&row.fields, "account"),
                        field_str(&row.fields, "service"),
                    ]
                    .into_iter()
                    .find(|x| !x.is_empty())
                    .unwrap_or("");
                    let tone = if status == "active" {
                        "ok"
                    } else if status == "broken" {
                        "warn"
                    } else {
                        "pending"
                    };
                    (name, tone, format!("{who} · {}", status.replace('_', " ")))
                }
            };
            let warn = tone == "warn";
            let chip_el = chip(
                t,
                format!("credchip-{id}"),
                if warn { t.red } else { t.fg },
                true,
                chip_icon(Glyph::PropKey, if warn { t.red } else { t.muted }),
                name.clone(),
            )
            .cursor(Cursor::Pointer)
            .on_press(open_object(id));
            let mut wrap =
                reveal_group(row().key(format!("cred-{id}")).shrink()).child(tip(chip_el, title));
            if editable {
                wrap = wrap.child(chip_rm(
                    t,
                    format!("rm-{id}"),
                    format!("Remove {name}"),
                    remover(id),
                ));
            }
            value = value.child(chip_wrap(wrap));
        }
    } else if key == "agent" {
        if list.is_empty() {
            value = value.child(empty());
        }
        for id in &list {
            let a = s.store.agent(id);
            let gone = a.is_none() && c.pane.gone_agents.contains(id);
            let loading = a.is_none() && !gone;
            let title = match a {
                Some(a) => format!("{} · agent", a.name),
                None if gone => format!("Agent {}… (deleted — remove)", prefix(id, 8)),
                None => format!("Agent {}… (still loading on this device)", prefix(id, 8)),
            };
            let icon = match a {
                Some(a) if !a.icon.is_empty() => a.icon.clone(),
                Some(_) => "🤖".into(),
                None if gone => "⚠️".into(),
                None => "⏳".into(),
            };
            let name = match a {
                Some(a) if !a.name.is_empty() => a.name.clone(),
                _ if gone => format!("{}…", prefix(id, 8)),
                _ => "Loading…".into(),
            };
            let rm_label = format!("Remove {}", a.map_or("agent", |a| a.name.as_str()));
            let tone = if gone {
                t.red
            } else if loading {
                t.muted
            } else {
                t.fg
            };
            let mut chip_el = chip(
                t,
                format!("agentchip-{id}"),
                tone,
                false,
                roostr_ui::emoji_text(&icon, ty::ui(12.0)),
                name,
            );
            // The chip opens the agent; the row around it edits the list.
            if a.is_some() {
                chip_el = chip_el.cursor(Cursor::Pointer).on_press(open_object(id));
            }
            let mut wrap =
                reveal_group(row().key(format!("agent-{id}")).shrink()).child(tip(chip_el, title));
            if editable {
                wrap = wrap.child(chip_rm(t, format!("rm-{id}"), rm_label, remover(id)));
            }
            value = value.child(chip_wrap(wrap));
        }
    } else if key == "skills" || key == "tools" {
        if list.is_empty() {
            value = value.child(empty());
        }
        for id in &list {
            let (name, warn, title, g) = if key == "skills" {
                let sk = c.pane.skills.get(id);
                let ok = sk.is_some_and(|x| x.key.is_empty() || !x.machines.is_empty());
                let warn = sk.is_some_and(|x| !x.key.is_empty()) && !ok;
                let title = match sk {
                    None => "Skill".to_string(),
                    Some(x) if x.key.is_empty() => format!("{} · instructions", x.name),
                    Some(x) if !x.machines.is_empty() => {
                        format!("{} · working on {}", x.name, x.machines.join(", "))
                    }
                    Some(x) => format!("{} · not working on any computer yet", x.name),
                };
                (
                    sk.map_or_else(|| format!("{}…", prefix(id, 8)), |x| x.name.clone()),
                    warn,
                    title,
                    Glyph::PropSkill,
                )
            } else {
                let tl = c.pane.tools.get(id);
                let title = match tl {
                    None => "Tool".to_string(),
                    Some(x) if !x.error.is_empty() => format!("{} · {}", x.name, x.error),
                    Some(x) => format!(
                        "{} · {}",
                        x.name,
                        if x.builtin { "built-in tool" } else { "tool" }
                    ),
                };
                (
                    tl.map_or_else(|| format!("{}…", prefix(id, 8)), |x| x.name.clone()),
                    tl.is_some_and(|x| !x.error.is_empty()),
                    title,
                    Glyph::PropTool,
                )
            };
            let rm_label = format!("Remove {name}");
            let chip_el = chip(
                t,
                format!("{key}chip-{id}"),
                if warn { t.red } else { t.fg },
                false,
                chip_icon(g, if warn { t.red } else { t.muted }),
                name,
            )
            .cursor(Cursor::Pointer)
            .on_press(open_object(id));
            let mut wrap =
                reveal_group(row().key(format!("{key}-{id}")).shrink()).child(tip(chip_el, title));
            if editable {
                wrap = wrap.child(chip_rm(t, format!("rm-{id}"), rm_label, remover(id)));
            }
            value = value.child(chip_wrap(wrap));
        }
    } else if rel.format == "object" {
        if list.is_empty() {
            value = value.child(empty());
        }
        for id in &list {
            let o = s.store.summary(id);
            let a = if o.is_some() { None } else { s.store.agent(id) };
            let lead = match o {
                Some(o) if s.store.layout_of(&o.type_key) == "task" => {
                    checkbox_display(format!("obj-chk-{id}"), o.done == Some(true))
                        .xsmall()
                        .build(t)
                }
                _ => {
                    let icon = match a {
                        Some(a) if !a.icon.is_empty() => a.icon.clone(),
                        Some(_) => "🤖".into(),
                        None => object_icon(
                            &s.store,
                            o.map_or("", |o| o.icon.as_str()),
                            o.map_or("", |o| o.type_key.as_str()),
                            c.space,
                        ),
                    };
                    icon_text_el(s, &icon, 12.0, ty::ui(12.0))
                }
            };
            let name = o
                .map(|o| o.name.clone())
                .filter(|n| !n.is_empty())
                .or_else(|| a.map(|a| a.name.clone()).filter(|n| !n.is_empty()))
                .unwrap_or_else(|| "Untitled".into());
            let rm_label = format!(
                "Remove {}",
                if name == "Untitled" {
                    "link"
                } else {
                    name.as_str()
                }
            );
            let chip_el = chip(t, format!("objchip-{id}"), t.fg, true, lead, name)
                .cursor(Cursor::Pointer)
                .on_press(open_object(id));
            let mut wrap = reveal_group(row().key(format!("obj-{id}")).shrink()).child(chip_el);
            if editable {
                wrap = wrap.child(chip_rm(t, format!("rm-{id}"), rm_label, remover(id)));
            }
            value = value.child(chip_wrap(wrap));
        }
    } else if shown_text.is_empty() {
        value = value.child(empty());
    } else {
        if key == "chat_color"
            && let Some(hex) =
                roostr_ui::color::normalize_hex(&shown_text).and_then(|h| roostr_ui::color::hex(&h))
        {
            value = value.child(
                el().size(12.0, 12.0)
                    .radius(6.0)
                    .border(1.0, t.border)
                    .fill(hex),
            );
        }
        let overdue = rel.format == "date"
            && key == "due_date"
            && v.and_then(Value::as_number)
                .is_some_and(|ms| ms != 0. && (ms as i64) < now_ms())
            && object.field("done").and_then(|d| d.bool_value) != Some(true);
        let wrap = key == "error";
        let line = if secret {
            "••••••••".to_string()
        } else {
            shown_text.clone()
        };
        let mut val = text(line)
            .color(if wrap || overdue { t.red } else { t.fg })
            .min_w(0.0)
            .shrink();
        val = if wrap {
            val.font(ty::ui(13.0).lh(18.0)).py(2.0).wrap()
        } else {
            val.font(ty::ui(13.0).lh(22.0)).truncate()
        };
        value = value.child(val);
    }
    let judged = judged(s, object, &key);
    if let Some(j) = &judged {
        value = value.child(tip(
            el().key(format!("judged-{key}")).ml(6.0).child(
                text(j.text.clone())
                    .font(ty::ui(11.5).lh(22.0))
                    .color(t.muted),
            ),
            j.title.clone(),
        ));
    }

    // ── The row ──
    let row_removable = !((c.is_credential || c.is_capability) && key == "status")
        && if rel.format == "tag" || rel.format == "object" {
            key == "served_by" && !list.is_empty()
        } else {
            !shown_text.is_empty()
        };
    let name_icon = match row_emoji {
        Some(e) => row()
            .w(14.0)
            .justify_center()
            .child(text(e).font(ty::ui(13.0))),
        None => glyph(row_glyph, 14.0).color(t.muted),
    };
    let mut line = row()
        .key(format!("prop-row-{key}"))
        .items_start()
        .gap(10.0)
        .pl(6.0)
        .pr(24.0)
        .py(4.0)
        .radius(radius::SM)
        .child(
            row()
                .w_frac(0.4)
                .max_w(120.0)
                .min_w(0.0)
                // One line tall, whatever the value's height: the icon and
                // the name sit together beside its first line.
                .h(22.0)
                .items_center()
                .gap(7.0)
                .child(name_icon)
                .child(
                    text(label.clone())
                        .font(ty::ui(13.0).lh(22.0))
                        .color(t.muted)
                        .truncate()
                        .shrink(),
                ),
        )
        .child(value);
    if editable {
        let (id, k, tk) = (oid.clone(), key.clone(), c.type_key.to_string());
        line = line
            .hover(t.hover)
            .cursor(Cursor::Pointer)
            .on_press(move |s: &mut State| toggle_edit(s, &id, &k, &tk));
    }
    let mut wrap = reveal_group(col().key(format!("row-wrap-{key}"))).child(line);
    if row_removable && editable {
        let (id, k) = (oid.clone(), key.clone());
        let button = row()
            .key(format!("row-rm-{key}"))
            .role("button")
            .label("Remove property")
            .size(18.0, 18.0)
            .center()
            .radius(9.0)
            .hover(ghost_hover(t))
            .hover_group()
            .cursor(Cursor::Pointer)
            .on_press(move |s: &mut State| remove_prop(s, &id, &k))
            .child(glyph(Glyph::Close, 11.0).color(t.muted).group_color(t.red));
        wrap = wrap.child(reveal(
            el().pin(None, Some(6.0), Some(2.0), None)
                .child(tip(button, "Remove property")),
        ));
    }
    if editing {
        let width = cx
            .layout(&format!("row-wrap-{key}"))
            .map_or(300.0, |r| r[2]);
        let mut inner = Vec::new();
        let pop = render_pop(s, cx, t, &mut inner, c, rel, judged, width);
        below(layer, &format!("row-wrap-{key}"), 4.0, pop);
        layer.extend(inner);
    }
    if c.pane.entering.as_deref() == Some(key.as_str()) {
        wrap = enter(wrap);
    }
    wrap
}

/// A popover action: muted 11px text that tints to `hover` while hovered.
fn pop_action(t: &Theme, key: &str, label: String, hover: Color) -> El {
    row()
        .key(key.to_string())
        .role("button")
        .label(label.clone())
        .hover_group()
        .cursor(Cursor::Pointer)
        .child(
            text(label)
                .font(ty::ui(11.0))
                .color(t.muted)
                .group_color(hover),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_pop(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    c: &RowCx,
    rel: &RelationDef,
    judged: Option<Judged>,
    width: f32,
) -> El {
    let object = c.object;
    let oid = object.id.clone();
    let key = rel.key.clone();
    let label = if rel.name.is_empty() {
        key.clone()
    } else {
        rel.name.clone()
    };
    let mut actions = row().items_center().gap(12.0);
    if let Some(j) = &judged {
        let asking = c.pane.asking == key;
        let mut ask = pop_action(
            t,
            "ask",
            if asking { "Asking…" } else { "Ask again" }.into(),
            t.fg,
        );
        if asking {
            ask = ask.opacity(0.5);
        } else {
            let (id, k) = (oid.clone(), key.clone());
            ask = ask.on_press(move |s: &mut State| ask_again(s, &id, &k));
        }
        let skill = j.skill_id.clone();
        let question = pop_action(t, "question", "Edit question →".into(), t.fg)
            .on_press(move |s: &mut State| nav::navigate(Route::Object(skill.clone()), s));
        actions = actions.child(tip(ask, j.title.clone())).child(tip(
            question,
            "Open the Jev Skill whose page is the question",
        ));
    }
    if key != "done" && !((c.is_credential || c.is_capability) && key == "status") {
        let (id, k) = (oid.clone(), key.clone());
        let remove = pop_action(t, "remove", "Remove".into(), t.red)
            .on_press(move |s: &mut State| remove_prop(s, &id, &k));
        actions = actions.child(tip(remove, "Remove property"));
    }
    // Multi-line text: Enter is a new line, so Done saves and closes.
    if rel.format == "longtext" || key == "error" {
        let id = oid.clone();
        let done = pop_action(t, "done", "Done".into(), t.accent)
            .on_press(move |s: &mut State| finish_editing(s, &id));
        actions = actions.child(el().key("pop-done").child(tip(done, "Save and close")));
    }
    let head = row()
        .items_center()
        .justify_between()
        .gap(12.0)
        .child(text(label.to_uppercase()).font(ty::ui(11.0)).color(t.muted))
        .child(actions);
    let close_id = oid.clone();
    let mut pop = col()
        .key("prop-pop")
        .w(width)
        .gap(8.0)
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(radius::MENU)
        .padding(10.0)
        .box_shadow(pop_shadow())
        // A press outside the box (and the menus it opens after it) closes it, as GPUI's
        // `pane-backdrop` did; a scrim under a modal box would never see the press.
        .modal(true, move |s: &mut State| finish_editing(s, &close_id))
        .child(head);
    if !c.pane.ask_error.is_empty() && judged.is_some() {
        pop = pop.child(
            text(c.pane.ask_error.clone())
                .font(ty::ui(12.0))
                .color(t.red)
                .wrap()
                .mb(6.0),
        );
    }
    if c.is_capability && key == "status" {
        pop = pop.child(capability_status(s, cx, t, layer, &oid));
    } else if c.is_credential && key == "status" {
        pop = pop.child(credential_status(
            s,
            cx,
            t,
            layer,
            &oid,
            &c.pane.connect_poll_error,
        ));
    } else {
        let (id, k) = (oid.clone(), key.clone());
        pop = pop.child(
            PropertyValue::new(
                format!("pv-{oid}-{key}"),
                rel.clone(),
                object.field(&key).cloned(),
            )
            .space(object.field_str("channel").to_string())
            .seeds(c.tag_seeds.clone())
            .auto_open(true)
            .on_save(move |s: &mut State, v: Value| save_value(s, &id, &k, v))
            .build(s, cx, t, layer),
        );
        if key == "repo_path" {
            let target = oid.clone();
            let on_save: Rc<dyn Fn(&mut State, String)> =
                Rc::new(move |s: &mut State, path: String| {
                    let task = api::note::set_field(&target, "repo_path", Value::string(path), s);
                    task.detach_and_log_err(&s.rt);
                });
            pop = pop.child(project_folder(s, cx, t, layer, &oid, on_save));
        }
    }
    pop
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_system_tool_jev_then_the_rest() {
        assert_eq!(prop_rank("due_date"), 0);
        assert_eq!(prop_rank("tool_version"), 1);
        assert_eq!(prop_rank("jev_answer"), 2);
        assert_eq!(prop_rank("priority"), 3);
    }

    #[test]
    fn plain_values() {
        let links = Value::values(vec![Value::link("a", "agent"), Value::string("b")]);
        assert_eq!(plain_list(Some(&links)), ["a", "b"]);
        assert_eq!(plain_text(Some(&links)), "a, b");
        assert_eq!(plain_text(Some(&Value::float(2.0))), "2");
        assert!(plain_list(Some(&Value::string(""))).is_empty());
    }

    /// The Properties rows follow the backend's relation order within a rank (system
    /// properties first), whatever that order is; a duplicate key's lower-id def keeps the
    /// first one's place. GPUI showed this Matcherino task as Done, Status, Target List,
    /// Customer, Draft doc, Invovles for the daemon's order below.
    #[test]
    fn property_rows_keep_the_backend_relation_order() {
        let rel = |id: &str, key: &str, space: &str| RelationDef {
            id: id.into(),
            key: key.into(),
            name: key.into(),
            space: Some(space.into()),
            ..Default::default()
        };
        let backend = vec![
            rel("f07a", "target_list", "sp"),
            rel("bundled-done", "done", "sp"),
            rel("4f7e", "kb_status", "other"),
            rel("a341", "customer", "sp"),
            rel("e440", "draft_doc", "sp"),
            rel("380d", "invovles", "sp"),
            rel("bundled-status", "status", "sp"),
            rel("0001", "target_list", "sp"),
        ];
        let relations: Vec<RelationDef> = space_relations(&backend, "sp")
            .into_iter()
            .cloned()
            .collect();
        assert_eq!(
            relations[0].id, "0001",
            "the lower id wins in the first one's place"
        );
        let mut o = Object {
            type_key: "task".into(),
            ..Object::default()
        };
        o.fields.insert("customer".into(), Value::string("x"));
        o.fields.insert("draft_doc".into(), Value::string("x"));
        let pane = PaneState {
            view_keys: ["done", "status", "target_list", "invovles"]
                .map(String::from)
                .into(),
            ..PaneState::default()
        };
        let shown = shown(&pane, &o, "task", &relations, &HashSet::new());
        let keys: Vec<String> = property_rows(&[], &shown)
            .into_iter()
            .map(|r| r.key)
            .collect();
        assert_eq!(
            keys,
            [
                "done",
                "status",
                "target_list",
                "customer",
                "draft_doc",
                "invovles"
            ]
        );
    }

    #[test]
    fn credential_rows_name_their_keys_and_cookies() {
        let mut o = Object {
            type_key: "credential".into(),
            ..Object::default()
        };
        o.fields.insert("key_api_token".into(), Value::string("x"));
        o.fields.insert("key_fields".into(), Value::string("x"));
        o.fields
            .insert(format!("{COOKIE_PREFIX}sid"), Value::string("x"));
        let rows = own_rows(&o, "credential", &[]);
        assert!(
            rows.iter()
                .any(|r| r.key == "key_api_token" && r.name == "Api token")
        );
        assert!(!rows.iter().any(|r| r.key == "key_fields"));
        assert!(rows.iter().any(|r| r.name == "Cookie sid"));
        let caps = own_rows(&o, "capability", &[]);
        assert!(caps.iter().find(|r| r.key == "key").unwrap().read_only);
    }

    #[test]
    fn empty_values_by_format() {
        assert_eq!(empty_value("checkbox").bool_value, Some(false));
        assert!(empty_value("tag").values_value.is_some());
        assert_eq!(empty_value("url").string_value.as_deref(), Some(""));
    }
}

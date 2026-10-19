//! The right pane's properties section (`PropertiesPane.svelte`): the
//! object's properties as rows — a small icon + the name on the left
//! (muted), the value on the right. Clicking a row opens its editor in a
//! popover under the row; checkboxes toggle in place; multi-value rows carry
//! a hover × per value, single-value rows a hover × that removes the
//! property. Grouped "How it runs", then "Properties".

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::color::normalize_hex;
use roostr_core::harness::harness;
use roostr_core::io;
use roostr_core::objects::object_icon;
use roostr_core::options::{badge_colors, status_icon};
use roostr_core::relations::{current_space_id, is_reserved_key, object_space_id, space_relations};
use roostr_core::serving::{
    MachineRow, machine_name, resolve_serving, served_by_machine_id, serving_copy,
};
use roostr_core::types::{field_str, is_builtin_tool, is_locked_tool, repeat_of};
use roostr_core::ui::{CheckboxIcon, prop_line_icon};
use roostr_core::{
    Fields, Glyph, Object, QueryRow, RelationDef, Route, Store, StoreEvent, Value, api, nav, theme,
};
use serde_json::json;

use crate::capability::{CapabilityStatus, capability_status_badge, capability_status_text};
use crate::credential_ui::CredentialStatus;
use crate::credentials::{
    CONNECT_TIMEOUT_MS, CONNECT_TIMEOUT_TEXT, COOKIE_META, COOKIE_PREFIX, POLL,
    credential_status_badge, credential_status_text, is_secret_field, now_ms, read_fields,
};
use crate::project_folder::ProjectFolder;
use crate::repeat_view::RepeatView;
use crate::suggest::{PropertyPick, PropertySuggest, SuggestEvent};
use crate::value::{PropertyValue, date_label, number_text};
use crate::widgets::{backdrop, below, placeholder, pop_shadow, prefix, tip};

/// Types that never have an agent of their own (`AGENTLESS_TYPES`).
const AGENTLESS_TYPES: &[&str] = &[
    "agent",
    "channel",
    "relation",
    "type",
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
const NO_REPEAT: &[&str] = &[
    "channel",
    "chat",
    "type",
    "relation",
    "template",
    "query",
    "set",
    "collection",
    "agent",
    "tool",
];
const REPEAT_KEY: &str = "__repeat__";

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

/// The right pane's properties section for one object.
pub struct PropertiesPane {
    object_id: String,
    editing: Option<String>,
    suggest: Option<Entity<PropertySuggest>>,
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
    credential_status: Option<Entity<CredentialStatus>>,
    capability_status: Option<Entity<CapabilityStatus>>,
    project_folder: Option<Entity<ProjectFolder>>,
    repeat: Option<Entity<RepeatView>>,
    row_width: Rc<Cell<Pixels>>,
    _subs: Vec<Subscription>,
}

impl PropertiesPane {
    /// The properties of `object_id` (loads it into the store cache).
    pub fn new(object_id: String, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        store.update(cx, |s, cx| s.ensure_object(&object_id, cx));
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&store, |this: &mut Self, _, ev: &StoreEvent, cx| {
                if matches!(ev, StoreEvent::Reconnected) {
                    this.loaded_for = None;
                    cx.notify();
                }
            }),
        ];
        Self {
            object_id,
            editing: None,
            suggest: None,
            view_keys: HashSet::new(),
            serving: None,
            machines: vec![],
            skills: HashMap::new(),
            tools: HashMap::new(),
            tools_key: String::new(),
            credentials: HashMap::new(),
            credentials_key: String::new(),
            gone_agents: HashSet::new(),
            missing_agents_key: String::new(),
            loaded_for: None,
            asking: String::new(),
            ask_error: String::new(),
            connect_poll: None,
            connect_poll_error: String::new(),
            credential_status: None,
            capability_status: None,
            project_folder: None,
            repeat: None,
            row_width: Rc::new(Cell::new(px(300.))),
            _subs: subs,
        }
    }

    fn type_key(object: &Object, store: &Store) -> String {
        if object.type_key != "template" {
            return object.type_key.clone();
        }
        let target = object.field_str("target_type");
        store
            .types
            .iter()
            .find(|t| t.id == target)
            .map_or_else(|| object.type_key.clone(), |t| t.key.clone())
    }

    /// Re-run the lookups that depend on the object (views, serving, skills, tools, credentials, agents).
    fn refresh_lookups(&mut self, object: &Arc<Object>, cx: &mut Context<Self>) {
        let key = (object.id.clone(), object.updated_at);
        if self.loaded_for.as_ref() == Some(&key) {
            return;
        }
        self.loaded_for = Some(key);
        let store = Store::global(cx).read(cx);
        let space = object_space_id(store, &object.fields);

        // Columns of the views this object belongs to count as its properties.
        let views = api::query_all(
            json!({ "filters": [{ "key": "typeKey", "condition": "in", "value": ["query", "set", "collection"] }, { "key": "channel", "condition": "equal", "value": space }] }),
            cx,
        );
        let (id, type_key) = (object.id.clone(), object.type_key.clone());
        cx.spawn(async move |this, cx| {
            let mut keys = HashSet::new();
            if let Ok(views) = views.await {
                for v in views {
                    if v.id == id {
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
                            .any(|i| i.as_str() == Some(id.as_str()))
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
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.object_id == id {
                    this.view_keys = keys;
                    cx.notify();
                }
            });
        })
        .detach();

        // Serving (served_by names the machine and carries the warning), then skills.
        let serving = resolve_serving(&object.id, cx);
        let skills = api::query_all(json!({ "type": "skill" }), cx);
        let caps = api::query_all(json!({ "type": "capability" }), cx);
        cx.spawn(async move |this, cx| {
            let resolved = serving.await;
            let machines = resolved
                .as_ref()
                .map(|(_, m)| m.clone())
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.serving = resolved.as_ref().ok().map(|(s, m)| {
                    let copy = serving_copy(s, m);
                    (copy.text, copy.warning)
                });
                this.machines = machines.clone();
                cx.notify();
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
            let _ = this.update(cx, |this, cx| {
                this.skills = map;
                cx.notify();
            });
        })
        .detach();

        // The Tool objects the tools row links, so a broken one shows as such.
        let tool_ids = plain_list(object.field("tools"));
        let tools_key = tool_ids.join(",");
        if tools_key != self.tools_key || self.tools.is_empty() {
            self.tools_key = tools_key;
            let task = (!tool_ids.is_empty()).then(|| {
                api::query_all(
                    json!({ "filters": [{ "key": "id", "condition": "in", "value": tool_ids }] }),
                    cx,
                )
            });
            cx.spawn(async move |this, cx| {
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
                let _ = this.update(cx, |this, cx| {
                    this.tools = map;
                    cx.notify();
                });
            })
            .detach();
        }

        // The credential objects the credentials row links, for their status.
        let cred_ids = plain_list(object.field("credentials"));
        let cred_key = cred_ids.join(",");
        if cred_key != self.credentials_key || self.credentials.is_empty() {
            self.credentials_key = cred_key;
            let task = (!cred_ids.is_empty()).then(|| {
                api::query_all(
                    json!({ "filters": [{ "key": "id", "condition": "in", "value": cred_ids }] }),
                    cx,
                )
            });
            cx.spawn(async move |this, cx| {
                let rows = match task {
                    Some(t) => t.await.unwrap_or_default(),
                    None => vec![],
                };
                let _ = this.update(cx, |this, cx| {
                    this.credentials = rows.into_iter().map(|r| (r.id.clone(), r)).collect();
                    cx.notify();
                });
            })
            .detach();
        }

        // Agents the agent row links that this device does not list: deleted, or not downloaded yet.
        let missing: Vec<String> = plain_list(object.field("agent"))
            .into_iter()
            .filter(|id| store.agent(id).is_none())
            .collect();
        let missing_key = missing.join(",");
        if missing_key != self.missing_agents_key {
            self.missing_agents_key = missing_key;
            let s = Store::global(cx).read(cx);
            let ledger = s.fetch_object("__vanished__", cx);
            let fetches: Vec<_> = missing
                .iter()
                .map(|id| (id.clone(), s.fetch_object(id, cx)))
                .collect();
            cx.spawn(async move |this, cx| {
                let ledger = ledger.await.map(|o| o.fields).unwrap_or_default();
                let mut gone = HashSet::new();
                for (id, fetch) in fetches {
                    if ledger.contains_key(&format!("vanished:{id}")) {
                        gone.insert(id);
                        continue;
                    }
                    if let Ok(o) = fetch.await
                        && o.deleted
                    {
                        gone.insert(id);
                    }
                }
                let _ = this.update(cx, |this, cx| {
                    this.gone_agents = gone;
                    cx.notify();
                });
            })
            .detach();
        }

        // While a credential's sign-in window is open its computer writes the status: follow it.
        let is_credential = Self::type_key(object, store) == "credential";
        let connecting = is_credential && object.field_str("status") == "connecting";
        if connecting && self.connect_poll.is_none() {
            self.connect_poll_error.clear();
            let id = object.id.clone();
            let mut last: Fields = object.fields.clone();
            let deadline = now_ms() + CONNECT_TIMEOUT_MS;
            self.connect_poll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL).await;
                    if now_ms() > deadline {
                        let _ = this.update(cx, |this, cx| {
                            this.connect_poll_error = CONNECT_TIMEOUT_TEXT.into();
                            this.connect_poll = None;
                            cx.notify();
                        });
                        return;
                    }
                    let read = cx.update(|cx| read_fields(&id, cx));
                    let Some(fresh) = read.await else { continue };
                    if ["status", "error", "checked_at", "auth"]
                        .iter()
                        .any(|k| fresh.get(*k) != last.get(*k))
                    {
                        last = fresh.clone();
                        cx.update(|cx| crate::credentials::changed(&id, cx));
                    }
                    if field_str(&fresh, "status") != "connecting" {
                        let _ = this.update(cx, |this, _| this.connect_poll = None);
                        return;
                    }
                }
            }));
        } else if !connecting {
            self.connect_poll = None;
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
        &self,
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
                if agent_config.contains(&k)
                    || tool_config.contains(&k)
                    || skill_config.contains(&k)
                {
                    return true;
                }
                match k {
                    "error" => object.type_key != "template",
                    _ if type_key == "channel" => k == "agent",
                    "keep_all_files" | "autostart" => type_key == "machine" || has(k),
                    "served_by" => machine_bound || has(k),
                    "credentials" | "skills" | "tools" => has(k),
                    "check_first" => repeating || has(k),
                    "agent" => !AGENTLESS_TYPES.contains(&type_key) || has(k),
                    "model" => type_key == "agent" || has(k),
                    _ => !r.hidden && (has(k) || self.view_keys.contains(k)),
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

    fn judged(&self, object: &Object, key: &str, cx: &App) -> Option<Judged> {
        let e = object.field("judged")?.entries()?.get(key)?.entries()?;
        let text = if let Some(p) = e.get("probability").and_then(|v| v.float_value) {
            format!("{}% yes", (p * 100.).round())
        } else {
            let c = e.get("confidence").and_then(|v| v.float_value)?;
            format!("{}% sure", (c * 100.).round())
        };
        let store = Store::global(cx).read(cx);
        let skill_id = e
            .get("skill")
            .and_then(|v| v.string_value.clone())
            .unwrap_or_default();
        let skill = store
            .summary(&skill_id)
            .map(|s| s.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "a Jev Skill".into());
        let agent = e
            .get("agent")
            .and_then(|v| v.as_str())
            .and_then(|a| store.agent(a))
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

    fn ask_again(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(h) = harness(cx) else {
            self.ask_error = "Roostr isn't running on this computer.".into();
            cx.notify();
            return;
        };
        self.asking = key.to_string();
        self.ask_error.clear();
        let body = json!({ "object": self.object_id, "key": key });
        let task = io::spawn(cx, async move { h.post_json("/jev/again", &body).await });
        let id = self.object_id.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.ask_error = e.to_string();
                } else {
                    crate::credentials::changed(&id, cx);
                }
                this.asking.clear();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save_value(&mut self, key: &str, value: Value, cx: &mut App) {
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
                && let Some(m) = self.machines.iter().find(|m| m.id == target)
            {
                value = Value::string(m.machine_id.clone());
            }
        }
        api::note::set_field(&self.object_id, key, value, cx).detach_and_log_err(cx);
    }

    /// Remove one value from a list property, or the property when it was the last.
    fn remove_value(&mut self, object: &Object, key: &str, value: &str, cx: &mut Context<Self>) {
        self.editing = None;
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
        if !next.is_empty() && items.len() > 1 {
            api::note::set_field(&object.id, key, Value::values(next), cx).detach_and_log_err(cx);
        } else {
            api::note::delete_field(&object.id, key, cx).detach_and_log_err(cx);
        }
        cx.notify();
    }

    fn remove_prop(&mut self, key: &str, cx: &mut Context<Self>) {
        self.editing = None;
        api::note::delete_field(&self.object_id, key, cx).detach_and_log_err(cx);
        cx.notify();
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

    fn open_suggest(&mut self, exclude: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let space = current_space_id(cx);
        let suggest = cx.new(|cx| PropertySuggest::new(space, exclude, vec![], window, cx));
        let sub = cx.subscribe(&suggest, |this, _, ev: &SuggestEvent<PropertyPick>, cx| {
            this.suggest = None;
            if let SuggestEvent::Picked(PropertyPick::Relation(rel)) = ev {
                api::note::set_field(
                    &this.object_id,
                    &rel.key,
                    Self::empty_value(&rel.format),
                    cx,
                )
                .detach_and_log_err(cx);
                // Open the new row's editor so the value can be set straight away.
                this.editing = Some(rel.key.clone());
            }
            cx.notify();
        });
        self._subs.push(sub);
        self.suggest = Some(suggest);
        cx.notify();
    }

    fn toggle_edit(
        &mut self,
        key: &str,
        editable: bool,
        object: &Object,
        type_key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !editable {
            return;
        }
        if self.editing.as_deref() == Some(key) {
            self.editing = None;
        } else {
            self.editing = Some(key.to_string());
            if key == "status" && type_key == "credential" {
                let id = object.id.clone();
                self.credential_status = Some(cx.new(|cx| CredentialStatus::new(id, cx)));
            }
            if key == "status" && type_key == "capability" {
                let id = object.id.clone();
                self.capability_status = Some(cx.new(|cx| CapabilityStatus::new(id, cx)));
            }
            if key == "repo_path" && object.type_key == "agent" {
                let id = object.id.clone();
                let target = object.id.clone();
                self.project_folder = Some(cx.new(|cx| {
                    ProjectFolder::new(
                        id,
                        move |path, cx| {
                            api::note::set_field(&target, "repo_path", Value::string(path), cx)
                                .detach_and_log_err(cx)
                        },
                        cx,
                    )
                }));
            }
        }
        let _ = window;
        cx.notify();
    }
}

fn chip_rm(
    id: SharedString,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    group: SharedString,
) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .top(px(-6.))
        .right(px(-6.))
        .size(px(15.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .text_size(px(11.))
        .text_color(theme::muted())
        .cursor_pointer()
        .invisible()
        .group_hover(group, |s| s.visible())
        .hover(|s| s.text_color(theme::red()).bg(theme::hover()))
        .tooltip(tip(label))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child("×")
}

/// A neutral chip: plain text on a quiet outline; colour only for problems.
fn chip(id: SharedString) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(4.))
        .min_w_0()
        .max_w_full()
        .px(px(8.))
        .border_1()
        .border_color(theme::border())
        .rounded(theme::radius::menu())
        .text_size(px(12.))
        .line_height(px(20.))
        .text_color(theme::fg())
        .overflow_hidden()
        .whitespace_nowrap()
}

fn chip_icon(glyph: Glyph, color: Hsla) -> Icon {
    Icon::new(glyph).size(px(12.)).flex_none().text_color(color)
}

fn status_val(text: String, color: &str, glyph: Glyph) -> Div {
    let c = badge_colors(color);
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .text_color(c.fg)
        .child(Icon::new(glyph).size(px(14.)).text_color(c.icon))
        .child(text)
}

impl Render for PropertiesPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = Store::global(cx).read(cx).object(&self.object_id) else {
            return div().into_any_element();
        };
        self.refresh_lookups(&object, cx);
        let store = Store::global(cx).read(cx);
        let space = object_space_id(store, &object.fields);
        let relations: Vec<RelationDef> = space_relations(&store.relations, &space)
            .into_iter()
            .cloned()
            .collect();
        let type_key = Self::type_key(&object, store);
        let is_credential = type_key == "credential";
        let is_capability = type_key == "capability";
        let own_rows = Self::own_rows(&object, &type_key, &relations);
        let own_keys: HashSet<String> = own_rows.iter().map(|r| r.key.clone()).collect();
        let shown = self.shown(&object, &type_key, &relations, &own_keys);
        let locked = is_locked_tool(&object.fields);
        let can_repeat = !NO_REPEAT.contains(&object.type_key.as_str());
        let tag_seeds: Fields = relations
            .iter()
            .filter(|r| r.format == "tag")
            .filter_map(|r| object.field(&r.key).map(|v| (r.key.clone(), v.clone())))
            .collect();

        // Grouped display: "How it runs", then "Properties".
        let mut how: Vec<RelationDef> = own_rows
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
        let error = take(&mut how, "error");
        if can_repeat {
            how.push(RelationDef {
                key: REPEAT_KEY.into(),
                name: "Repeat".into(),
                ..Default::default()
            });
        }
        how.extend(assignee);
        how.extend(check_first);
        how.extend(error);
        let mut props: Vec<RelationDef> = own_rows
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

        let mut root = div().flex().flex_col().text_color(theme::fg());
        for (label, rows) in [("How it runs", how), ("Properties", props)] {
            if rows.is_empty() {
                continue;
            }
            let mut group = div().flex().flex_col().pt(px(4.)).pb(px(6.)).child(
                div()
                    .px(px(6.))
                    .pt(px(6.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::muted())
                    .child(label),
            );
            for rel in rows {
                group = group.child(self.render_row(
                    &object,
                    &rel,
                    &type_key,
                    is_credential,
                    is_capability,
                    locked,
                    &space,
                    &tag_seeds,
                    window,
                    cx,
                ));
            }
            root = root.child(group);
        }
        if object.type_key == "skill" && object.field_str("key").is_empty() {
            let b = |t: &'static str| div().font_weight(FontWeight::BOLD).child(t);
            let code = |t: &'static str| {
                div()
                    .font_family(theme::fonts::MONO)
                    .text_size(px(11.))
                    .child(t)
            };
            root = root.child(
                div()
                    .mx(px(4.))
                    .mt(px(6.))
                    .flex()
                    .flex_wrap()
                    .text_size(px(12.))
                    .line_height(relative(1.45))
                    .text_color(theme::muted())
                    .child("Give this Skill an ")
                    .child(b("Answer"))
                    .child(" to make it a Jev Skill: its page becomes the question (a numbered list of levels for a Score, bullets for a Choice, ")
                    .child(code("Yes:"))
                    .child(" / ")
                    .child(code("No:"))
                    .child(" lines for Yes or no), and agents that have it in their Skills run it with ")
                    .child(code("jev_score"))
                    .child(", filling ")
                    .child(b("Writes to"))
                    .child(" (default: this Skill's name). They need a TypeSafe credential."),
            );
        }
        if self.editing.is_some() {
            let this = cx.entity().downgrade();
            root = root.child(backdrop("pane-backdrop", move |_, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.editing = None;
                    cx.notify();
                });
            }));
        }
        if !locked {
            let exclude: Vec<String> = shown.iter().map(|r| r.key.clone()).collect();
            let mut add = div().relative().child(
                div()
                    .id("add-prop")
                    .w_full()
                    .p(px(6.))
                    .rounded(theme::radius::sm())
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_suggest(exclude.clone(), window, cx)
                    }))
                    .child("＋ Add property"),
            );
            if let Some(s) = &self.suggest {
                add = add.child(below(4., s.clone()));
            }
            root = root.child(add);
        }
        root.into_any_element()
    }
}

/// The pane's tag pill (`.pill`): 0 8px, radius 10, 12px, 20px line.
fn pane_pill(text: String, color: &str) -> Div {
    let (bg, fg) = roostr_core::options::tag_colors(color)
        .unwrap_or((roostr_core::options::unknown_tag_bg(), theme::fg()));
    div()
        .px(px(8.))
        .rounded(theme::radius::menu())
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .line_height(px(20.))
        .whitespace_nowrap()
        .child(text)
}

impl PropertiesPane {
    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &mut self,
        object: &Arc<Object>,
        rel: &RelationDef,
        type_key: &str,
        is_credential: bool,
        is_capability: bool,
        locked: bool,
        space: &str,
        tag_seeds: &Fields,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if rel.key == REPEAT_KEY {
            let id = object.id.clone();
            let repeat = self
                .repeat
                .get_or_insert_with(|| cx.new(|cx| RepeatView::new(id, window, cx)))
                .clone();
            return div().child(repeat).into_any_element();
        }
        let key = rel.key.clone();
        let v = object.field(&key);
        let editable = !locked && !rel.read_only;
        let group: SharedString = format!("row-{key}").into();
        let secret = is_credential && is_secret_field(&key);
        let glyph = if secret {
            Glyph::PropKey
        } else {
            prop_line_icon(&key, &rel.format)
        };
        let label = if rel.name.is_empty() {
            key.clone()
        } else {
            rel.name.clone()
        };
        let store = Store::global(cx).read(cx);
        let empty_hint = match key.as_str() {
            "served_by" => "No machine yet",
            "repo_path" => "No project folder",
            "chat_color" => "Default grey",
            "credentials" => "No credentials",
            "agent" => "Add agent",
            "skills" => "No skills",
            "tools" => "No tools",
            "check_first" => "None",
            _ => match rel.format.as_str() {
                "status" => "Select option",
                "tag" => "Select options",
                "date" => "Select a date",
                _ => "Empty",
            },
        };
        let empty = || {
            if editable {
                placeholder(group.clone(), empty_hint)
            } else {
                div().text_color(theme::muted()).opacity(0.6).child("—")
            }
        };
        let display = self.display(object, rel, store);
        let _object_id = object.id.clone();

        // ── The value ──
        let mut value = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(6.))
            .gap_y(px(4.))
            .min_h(px(22.))
            .text_color(theme::fg());
        let entity = cx.entity().downgrade();
        let remover = |id: String| {
            let entity = entity.clone();
            let key = key.clone();
            let object = object.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                let _ = entity.update(cx, |this, cx| this.remove_value(&object, &key, &id, cx));
            }
        };
        let open_object = |id: String| {
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                nav::navigate(Route::Object(id.clone()), cx);
            }
        };
        let list = plain_list(v);
        if is_capability && key == "status" {
            let status = object.field_str("status");
            let (glyph, color) = capability_status_badge(status);
            value = value.child(status_val(capability_status_text(status), color, glyph));
        } else if is_credential && key == "status" {
            let status = object.field_str("status");
            let (glyph, color) = credential_status_badge(status);
            value = value.child(status_val(credential_status_text(status), color, glyph));
        } else if rel.format == "checkbox" {
            let on = v.and_then(|v| v.bool_value) == Some(true);
            let k = key.clone();
            value = value.child(
                div()
                    .id(SharedString::from(format!("chk-{key}")))
                    .size(px(18.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::radius::xs())
                    .border_1()
                    .text_size(px(11.))
                    .text_color(theme::white())
                    .map(|d| {
                        if on {
                            d.bg(theme::accent()).border_color(theme::accent())
                        } else {
                            d.bg(theme::panel()).border_color(theme::border())
                        }
                    })
                    .when(editable, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.border_color(theme::accent()))
                    })
                    .tooltip(tip(label.clone()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if editable {
                            this.save_value(&k, Value::bool(!on), cx);
                        }
                    }))
                    .when(on, |d| d.child("✓")),
            );
        } else if rel.format == "tag" {
            if list.is_empty() {
                value = value.child(empty());
            }
            for t in &list {
                let color = rel
                    .options
                    .iter()
                    .find(|o| &o.text == t)
                    .map(|o| o.color.clone())
                    .unwrap_or_default();
                let g: SharedString = format!("tag-{key}-{t}").into();
                value = value.child(
                    div()
                        .id(g.clone())
                        .group(g.clone())
                        .relative()
                        .flex()
                        .child(pane_pill(t.clone(), &color))
                        .when(editable, |d| {
                            d.child(chip_rm(
                                format!("rm-{key}-{t}").into(),
                                format!("Remove {t}"),
                                remover(t.clone()),
                                g.clone(),
                            ))
                        }),
                );
            }
        } else if rel.format == "status" {
            if display.is_empty() {
                value = value.child(empty());
            } else {
                let color = rel
                    .options
                    .iter()
                    .find(|o| o.text == display)
                    .map(|o| o.color.clone())
                    .unwrap_or_default();
                value = value.child(status_val(display.clone(), &color, status_icon(&display)));
            }
        } else if key == "served_by" {
            match &self.serving {
                Some((text, warning)) => {
                    let text = text.strip_prefix("served by ").unwrap_or(text).to_string();
                    value = value.child(
                        div()
                            .id("serving")
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .when(*warning, |d| {
                                d.text_color(theme::red())
                                    .tooltip(tip("Cannot be honoured"))
                            })
                            .child(text),
                    );
                }
                None => value = value.child(empty()),
            }
        } else if key == "credentials" {
            if list.is_empty() {
                value = value.child(empty());
            }
            for id in &list {
                let (label, tone, title) = match self.credentials.get(id) {
                    None => (
                        store
                            .summary(id)
                            .map(|s| s.name.clone())
                            .filter(|n| !n.is_empty())
                            .unwrap_or_else(|| format!("{}…", prefix(id, 8))),
                        "",
                        "Credential".to_string(),
                    ),
                    Some(row) => {
                        let status = match field_str(&row.fields, "status") {
                            "" => "missing",
                            s => s,
                        };
                        let label = [
                            field_str(&row.fields, "name"),
                            field_str(&row.fields, "service"),
                        ]
                        .into_iter()
                        .find(|s| !s.is_empty())
                        .unwrap_or("Credential")
                        .to_string();
                        let who = [
                            field_str(&row.fields, "account"),
                            field_str(&row.fields, "service"),
                        ]
                        .into_iter()
                        .find(|s| !s.is_empty())
                        .unwrap_or("");
                        let tone = if status == "active" {
                            "ok"
                        } else if status == "broken" {
                            "warn"
                        } else {
                            "pending"
                        };
                        (label, tone, format!("{who} · {}", status.replace('_', " ")))
                    }
                };
                let warn = tone == "warn";
                let g: SharedString = format!("cred-{id}").into();
                value = value.child(
                    div()
                        .id(g.clone())
                        .group(g.clone())
                        .relative()
                        .flex()
                        .max_w_full()
                        .child(
                            chip(format!("credchip-{id}").into())
                                .when(warn, |d| d.text_color(theme::red()))
                                .cursor_pointer()
                                .hover(|s| s.border_color(theme::muted()))
                                .tooltip(tip(title))
                                .on_click(open_object(id.clone()))
                                .child(chip_icon(
                                    Glyph::PropKey,
                                    if warn { theme::red() } else { theme::muted() },
                                ))
                                .child(label.clone()),
                        )
                        .when(editable, |d| {
                            d.child(chip_rm(
                                format!("rm-{id}").into(),
                                format!("Remove {label}"),
                                remover(id.clone()),
                                g.clone(),
                            ))
                        }),
                );
            }
        } else if key == "agent" {
            if list.is_empty() {
                value = value.child(empty());
            }
            for id in &list {
                let a = store.agent(id);
                let gone = a.is_none() && self.gone_agents.contains(id);
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
                let g: SharedString = format!("agent-{id}").into();
                value = value.child(
                    div()
                        .id(g.clone())
                        .group(g.clone())
                        .relative()
                        .flex()
                        .max_w_full()
                        .child(
                            chip(format!("agentchip-{id}").into())
                                .when(gone, |d| d.text_color(theme::red()))
                                .when(loading, |d| d.text_color(theme::muted()))
                                .tooltip(tip(title))
                                .child(div().text_size(px(12.)).flex_none().child(icon))
                                .child(name),
                        )
                        .when(editable, |d| {
                            d.child(chip_rm(
                                format!("rm-{id}").into(),
                                rm_label,
                                remover(id.clone()),
                                g.clone(),
                            ))
                        }),
                );
            }
        } else if key == "skills" || key == "tools" {
            if list.is_empty() {
                value = value.child(empty());
            }
            for id in &list {
                let (name, warn, title, glyph) = if key == "skills" {
                    let sk = self.skills.get(id);
                    let ok = sk.is_some_and(|s| s.key.is_empty() || !s.machines.is_empty());
                    let warn = sk.is_some_and(|s| !s.key.is_empty()) && !ok;
                    let title = match sk {
                        None => "Skill".to_string(),
                        Some(s) if s.key.is_empty() => format!("{} · instructions", s.name),
                        Some(s) if !s.machines.is_empty() => {
                            format!("{} · working on {}", s.name, s.machines.join(", "))
                        }
                        Some(s) => format!("{} · not working on any computer yet", s.name),
                    };
                    (
                        sk.map_or_else(|| format!("{}…", prefix(id, 8)), |s| s.name.clone()),
                        warn,
                        title,
                        Glyph::PropSkill,
                    )
                } else {
                    let tl = self.tools.get(id);
                    let title = match tl {
                        None => "Tool".to_string(),
                        Some(t) if !t.error.is_empty() => format!("{} · {}", t.name, t.error),
                        Some(t) => format!(
                            "{} · {}",
                            t.name,
                            if t.builtin { "built-in tool" } else { "tool" }
                        ),
                    };
                    (
                        tl.map_or_else(|| format!("{}…", prefix(id, 8)), |t| t.name.clone()),
                        tl.is_some_and(|t| !t.error.is_empty()),
                        title,
                        Glyph::PropTool,
                    )
                };
                let g: SharedString = format!("{key}-{id}").into();
                let rm_label = format!("Remove {name}");
                value = value.child(
                    div()
                        .id(g.clone())
                        .group(g.clone())
                        .relative()
                        .flex()
                        .max_w_full()
                        .child(
                            chip(format!("{key}chip-{id}").into())
                                .when(warn, |d| d.text_color(theme::red()))
                                .tooltip(tip(title))
                                .child(chip_icon(
                                    glyph,
                                    if warn { theme::red() } else { theme::muted() },
                                ))
                                .child(name),
                        )
                        .when(editable, |d| {
                            d.child(chip_rm(
                                format!("rm-{id}").into(),
                                rm_label,
                                remover(id.clone()),
                                g.clone(),
                            ))
                        }),
                );
            }
        } else if rel.format == "object" {
            if list.is_empty() {
                value = value.child(empty());
            }
            for id in &list {
                let o = store.summary(id);
                let a = if o.is_some() { None } else { store.agent(id) };
                let lead: AnyElement = match o {
                    Some(o) if store.layout_of(&o.type_key) == "task" => {
                        let on = o.done == Some(true);
                        CheckboxIcon::new(on)
                            .size(px(13.))
                            .color(if on { theme::green() } else { theme::muted() })
                            .into_any_element()
                    }
                    _ => {
                        let icon = match a {
                            Some(a) if !a.icon.is_empty() => a.icon.clone(),
                            Some(_) => "🤖".into(),
                            None => object_icon(
                                store,
                                o.map_or("", |o| o.icon.as_str()),
                                o.map_or("", |o| o.type_key.as_str()),
                                space,
                            ),
                        };
                        div()
                            .text_size(px(12.))
                            .flex_none()
                            .child(icon)
                            .into_any_element()
                    }
                };
                let name = o
                    .map(|o| o.name.clone())
                    .filter(|n| !n.is_empty())
                    .or_else(|| a.map(|a| a.name.clone()).filter(|n| !n.is_empty()))
                    .unwrap_or_else(|| "Untitled".into());
                let g: SharedString = format!("obj-{id}").into();
                let rm_label = format!(
                    "Remove {}",
                    if name == "Untitled" {
                        "link"
                    } else {
                        name.as_str()
                    }
                );
                value = value.child(
                    div()
                        .id(g.clone())
                        .group(g.clone())
                        .relative()
                        .flex()
                        .max_w_full()
                        .child(
                            chip(format!("objchip-{id}").into())
                                .cursor_pointer()
                                .hover(|s| s.border_color(theme::muted()))
                                .on_click(open_object(id.clone()))
                                .child(lead)
                                .child(name),
                        )
                        .when(editable, |d| {
                            d.child(chip_rm(
                                format!("rm-{id}").into(),
                                rm_label,
                                remover(id.clone()),
                                g.clone(),
                            ))
                        }),
                );
            }
        } else if display.is_empty() {
            value = value.child(empty());
        } else {
            if key == "chat_color"
                && let Some(hex) = normalize_hex(&display).and_then(|h| roostr_core::color::hex(&h))
            {
                value = value.child(
                    div()
                        .flex_none()
                        .size(px(12.))
                        .rounded_full()
                        .border_1()
                        .border_color(theme::border())
                        .bg(hex),
                );
            }
            let overdue = rel.format == "date"
                && key == "due_date"
                && v.and_then(Value::as_number)
                    .is_some_and(|ms| ms != 0. && (ms as i64) < now_ms())
                && object.field("done").and_then(|d| d.bool_value) != Some(true);
            let wrap = key == "error";
            let text = if secret {
                "••••••••".to_string()
            } else {
                display.clone()
            };
            value = value.child(
                div()
                    .min_w_0()
                    .when(wrap, |d| d.line_height(px(18.)).py(px(2.)))
                    .when(!wrap, |d| {
                        d.overflow_hidden().whitespace_nowrap().text_ellipsis()
                    })
                    .when(wrap || overdue, |d| d.text_color(theme::red()))
                    .child(text),
            );
        }
        let judged = self.judged(object, &key, cx);
        if let Some(j) = &judged {
            value = value.child(
                div()
                    .id(SharedString::from(format!("judged-{key}")))
                    .ml(px(6.))
                    .text_size(px(11.5))
                    .text_color(theme::muted())
                    .whitespace_nowrap()
                    .tooltip(tip(j.title.clone()))
                    .child(j.text.clone()),
            );
        }

        // ── The row ──
        let row_removable = !((is_credential || is_capability) && key == "status")
            && if rel.format == "tag" || rel.format == "object" {
                key == "served_by" && !list.is_empty()
            } else {
                !display.is_empty()
            };
        let width = self.row_width.clone();
        let editing = self.editing.as_deref() == Some(key.as_str());
        let k2 = key.clone();
        let type_key_owned = type_key.to_string();
        let obj2 = object.clone();
        let mut wrap = div().relative().group(group.clone()).child(
            div()
                .id(SharedString::from(format!("row-{key}")))
                .flex()
                .items_start()
                .gap(px(10.))
                .pl(px(6.))
                .pr(px(24.))
                .py(px(4.))
                .rounded(theme::radius::sm())
                .text_size(px(13.))
                .line_height(px(22.))
                .when(editable, |d| {
                    d.cursor_pointer()
                        .group_hover(group.clone(), |s| s.bg(theme::hover()))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_edit(&k2, editable, &obj2, &type_key_owned, window, cx)
                }))
                .child(
                    div()
                        .flex_none()
                        .w(relative(0.4))
                        .max_w(px(120.))
                        .min_w_0()
                        .min_h(px(22.))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_color(theme::muted())
                        .child(match rel.icon_emoji.as_deref().filter(|e| !e.is_empty()) {
                            Some(e) => div()
                                .flex_none()
                                .w(px(14.))
                                .text_size(px(13.))
                                .text_center()
                                .child(e.to_string())
                                .into_any_element(),
                            None => Icon::new(glyph)
                                .size(px(14.))
                                .flex_none()
                                .text_color(theme::muted())
                                .into_any_element(),
                        })
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(label.clone()),
                        ),
                )
                .child(value),
        );
        wrap = wrap.child(
            canvas(
                move |bounds, _, _| width.set(bounds.size.width),
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
        if row_removable && editable {
            let k = key.clone();
            wrap = wrap.child(
                div()
                    .id(SharedString::from(format!("row-rm-{key}")))
                    .absolute()
                    .right(px(2.))
                    .top(px(6.))
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .invisible()
                    .group_hover(group.clone(), |s| s.visible())
                    .hover(|s| s.text_color(theme::red()).bg(theme::hover()))
                    .tooltip(tip("Remove property"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.remove_prop(&k, cx);
                    }))
                    .child("×"),
            );
        }
        if editing {
            wrap = wrap.child(below(
                4.,
                self.render_pop(
                    object,
                    rel,
                    judged,
                    is_credential,
                    is_capability,
                    space,
                    tag_seeds,
                    window,
                    cx,
                ),
            ));
        }
        wrap.into_any_element()
    }

    /// Compact display string for a row (`display`).
    fn display(&self, object: &Object, rel: &RelationDef, store: &Store) -> String {
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
                    store
                        .summary(id)
                        .map(|s| s.name.clone())
                        .filter(|n| !n.is_empty())
                        .or_else(|| {
                            store
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

    #[allow(clippy::too_many_arguments)]
    fn render_pop(
        &mut self,
        object: &Arc<Object>,
        rel: &RelationDef,
        judged: Option<Judged>,
        is_credential: bool,
        is_capability: bool,
        space: &str,
        tag_seeds: &Fields,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = rel.key.clone();
        let label = if rel.name.is_empty() {
            key.clone()
        } else {
            rel.name.clone()
        };
        let pop_rm = |id: SharedString, text: String| {
            div()
                .id(id)
                .text_size(px(11.))
                .text_color(theme::muted())
                .cursor_pointer()
                .child(text)
        };
        let mut head = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child(label.to_uppercase()),
            );
        let mut actions = div().flex().items_center().gap(px(12.));
        if let Some(j) = &judged {
            let asking = self.asking == key;
            let k = key.clone();
            let skill = j.skill_id.clone();
            actions = actions
                .child(
                    pop_rm(
                        "ask".into(),
                        if asking {
                            "Asking…".into()
                        } else {
                            "Ask again".into()
                        },
                    )
                    .hover(|s| s.text_color(theme::fg()))
                    .tooltip(tip(j.title.clone()))
                    .when(asking, |d| d.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !asking {
                            this.ask_again(&k, cx)
                        }
                    })),
                )
                .child(
                    pop_rm("question".into(), "Edit question →".into())
                        .hover(|s| s.text_color(theme::fg()))
                        .tooltip(tip("Open the Jev Skill whose page is the question"))
                        .on_click(move |_, _, cx| nav::navigate(Route::Object(skill.clone()), cx)),
                );
        }
        if key != "done" && !((is_credential || is_capability) && key == "status") {
            let k = key.clone();
            actions = actions.child(
                pop_rm("remove".into(), "Remove".into())
                    .hover(|s| s.text_color(theme::red()))
                    .tooltip(tip("Remove property"))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_prop(&k, cx))),
            );
        }
        head = head.child(actions);
        let mut pop = div()
            .id("pop")
            .w(self.row_width.get())
            .flex()
            .flex_col()
            .gap(px(8.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::menu())
            .p(px(10.))
            .shadow(pop_shadow())
            .text_color(theme::fg())
            .occlude()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(head);
        if !self.ask_error.is_empty() && judged.is_some() {
            pop = pop.child(
                div()
                    .mb(px(6.))
                    .text_size(px(12.))
                    .text_color(theme::red())
                    .child(self.ask_error.clone()),
            );
        }
        if is_capability && key == "status" {
            if let Some(c) = &self.capability_status {
                pop = pop.child(c.clone());
            }
        } else if is_credential && key == "status" {
            if let Some(c) = &self.credential_status {
                let err = self.connect_poll_error.clone();
                c.update(cx, |c, _| c.poll_error = err);
                pop = pop.child(c.clone());
            }
        } else {
            let this = cx.entity().downgrade();
            let k = key.clone();
            pop = pop.child(
                PropertyValue::new(
                    SharedString::from(format!("pv-{}-{key}", object.id)),
                    rel.clone(),
                    object.field(&key).cloned(),
                )
                .space(object.field_str("channel").to_string())
                .seeds(tag_seeds.clone())
                .auto_open(true)
                .on_save(move |v, _, cx| {
                    let _ = this.update(cx, |this, cx| this.save_value(&k, v.clone(), cx));
                }),
            );
            if key == "repo_path"
                && object.type_key == "agent"
                && let Some(f) = &self.project_folder
            {
                pop = pop.child(f.clone());
            }
        }
        let _ = space;
        pop.into_any_element()
    }
}

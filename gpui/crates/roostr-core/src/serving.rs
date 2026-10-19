//! Per-object serving (`src/lib/serving.ts`): which machine's harness does the
//! work for an object. The engine owns the rule; native asks the daemon
//! (`POST /api/serving`) instead of running the resolver in WASM, and this
//! module turns the answer into copy.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::Result;
use gpui_kit::{App, Task};
use serde::Deserialize;
use serde_json::json;

use crate::backend::Backend;
use crate::io;
use crate::store::{Store, query_all};
use crate::types::{Fields, QueryRow, field_str};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Serving {
    /// "" when nothing pins the object and no machine qualifies.
    #[serde(default)]
    pub machine_id: String,
    /// "self" | "pinned" | "pinned-uncapable" | "agent" | "agent-capable" | "capability" | "unsatisfied" | "unserved"
    #[serde(default)]
    pub reason: String,
    /// Catalog keys of the machine skills the object's Skills need.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Machine ids that have every one of `skills` working, sorted.
    #[serde(default)]
    pub candidates: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MachineRow {
    /// The machine object's id.
    pub id: String,
    /// The harness's stable machine id (`machine_id`).
    pub machine_id: String,
    pub name: String,
}

/// Type keys whose objects are never served by a machine on their own row.
pub const UNSERVED_TYPES: &[&str] = &["channel", "machine", "agent", "relation", "type", "skill"];

/// Catalog key → Skill object name, refreshed by every resolve.
static SKILL_NAMES: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// A catalog key's display name: its Skill object's name, else the key itself.
pub fn skill_label(key: &str) -> String {
    SKILL_NAMES
        .lock()
        .ok()
        .and_then(|m| m.get(key).cloned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| key.to_string())
}

pub fn machine_row_of(row: &QueryRow) -> MachineRow {
    MachineRow {
        id: row.id.clone(),
        machine_id: field_str(&row.fields, "machine_id").to_string(),
        name: field_str(&row.fields, "name").to_string(),
    }
}

/// Every machine object, raw and as [`MachineRow`]s.
pub fn fetch_machines(cx: &App) -> Task<Result<(Vec<QueryRow>, Vec<MachineRow>)>> {
    let backend = Store::global(cx).read(cx).backend();
    io::spawn(cx, async move {
        let rows = query_all(backend, json!({ "type": "machine" })).await?;
        let machines = rows.iter().map(machine_row_of).collect();
        Ok((rows, machines))
    })
}

async fn refresh_skill_names(backend: Arc<dyn Backend>) -> Result<()> {
    let names = query_all(backend, json!({ "type": "skill" }))
        .await?
        .into_iter()
        .filter_map(|s| {
            let key = field_str(&s.fields, "key").to_string();
            let name = field_str(&s.fields, "name").to_string();
            (!key.is_empty() && !name.is_empty()).then_some((key, name))
        })
        .collect();
    if let Ok(mut m) = SKILL_NAMES.lock() {
        *m = names;
    }
    Ok(())
}

async fn resolve(backend: Arc<dyn Backend>, ids: Vec<String>) -> Result<Vec<Serving>> {
    refresh_skill_names(backend.clone()).await?;
    let answer = backend.serving(&ids).await?;
    Ok(ids
        .iter()
        .map(|id| {
            answer
                .get(id)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default()
        })
        .collect())
}

/// Resolve one object against the live machine roster.
pub fn resolve_serving(object_id: &str, cx: &App) -> Task<Result<(Serving, Vec<MachineRow>)>> {
    let backend = Store::global(cx).read(cx).backend();
    let ids = vec![object_id.to_string()];
    io::spawn(cx, async move {
        let rows = query_all(backend.clone(), json!({ "type": "machine" })).await?;
        let serving = resolve(backend, ids).await?.pop().unwrap_or_default();
        Ok((serving, rows.iter().map(machine_row_of).collect()))
    })
}

/// Resolve many objects in one request; one entry per input id, in order.
pub fn resolve_many(object_ids: &[String], cx: &App) -> Task<Result<Vec<Serving>>> {
    if object_ids.is_empty() {
        return Task::ready(Ok(vec![]));
    }
    let backend = Store::global(cx).read(cx).backend();
    let ids = object_ids.to_vec();
    io::spawn(cx, resolve(backend, ids))
}

/// Human name of a machine id: its object's `name`, else the id's first 8 chars + "…".
pub fn machine_name(machines: &[MachineRow], machine_id: &str) -> String {
    if machine_id.is_empty() {
        return "no machine".into();
    }
    match machines
        .iter()
        .find(|m| m.machine_id == machine_id)
        .filter(|m| !m.name.is_empty())
    {
        Some(m) => m.name.clone(),
        None => format!("{}…", machine_id.chars().take(8).collect::<String>()),
    }
}

/// The machine id a `served_by` field names, however written: the machine_id
/// string, or a link / one-item list pointing at the machine's object.
pub fn served_by_machine_id(fields: &Fields, machines: &[MachineRow]) -> String {
    let Some(v) = fields.get("served_by") else {
        return String::new();
    };
    let first = v.items().first();
    let candidates = [
        v.string_value.as_deref(),
        v.link_value.as_ref().map(|l| l.target_id.as_str()),
        first.and_then(|f| f.string_value.as_deref()),
        first.and_then(|f| f.link_value.as_ref().map(|l| l.target_id.as_str())),
    ];
    let raw = candidates
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())
        .unwrap_or("");
    machines
        .iter()
        .find(|m| m.id == raw)
        .map_or_else(|| raw.to_string(), |m| m.machine_id.clone())
}

/// The sentence a chip or cell shows; `warning` when it cannot be honoured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServingCopy {
    pub text: String,
    pub warning: bool,
}

fn keys(serving: &Serving) -> String {
    serving
        .skills
        .iter()
        .map(|k| skill_label(k))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn serving_copy(serving: &Serving, machines: &[MachineRow]) -> ServingCopy {
    let copy = |text: String, warning| ServingCopy { text, warning };
    if serving.machine_id.is_empty() {
        let unsatisfied = serving.reason == "unsatisfied";
        let text = if unsatisfied {
            format!("no machine has {}", keys(serving))
        } else {
            "no machine yet".into()
        };
        return copy(text, unsatisfied);
    }
    let name = machine_name(machines, &serving.machine_id);
    match serving.reason.as_str() {
        "pinned" => copy(format!("served by {name} (pinned)"), false),
        "pinned-uncapable" => copy(
            format!("pinned to {name}, which lacks {}", keys(serving)),
            true,
        ),
        "agent-capable" | "capability" => {
            copy(format!("served by {name} for {}", keys(serving)), false)
        }
        "unsatisfied" => copy(format!("no machine has {}", keys(serving)), true),
        _ => copy(format!("served by {name}"), false),
    }
}

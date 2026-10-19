//! Types, properties and tools the UI refuses to delete (`src/lib/protected.ts`):
//! Roostr's own (`bundled`, `system`, `tool_builtin`) and any type/property a
//! query, template or property in the same space depends on. UI-level only.

use std::collections::HashMap;

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Task};
use serde_json::json;

use crate::api;
use crate::store::Store;
use crate::types::{Fields, Object, QueryRow, Value, field_str};

fn str_items(v: Option<&Value>) -> Vec<&str> {
    v.map(|v| {
        v.items()
            .iter()
            .filter_map(|i| i.as_str())
            .filter(|s| !s.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

fn map_keys(v: Option<&Value>) -> Vec<&str> {
    v.map(|v| {
        v.items()
            .iter()
            .filter_map(|i| i.entries()?.get("key")?.as_str())
            .filter(|s| !s.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

fn row_name(r: &QueryRow) -> &str {
    let name = field_str(&r.fields, "name");
    if name.is_empty() { "Untitled" } else { name }
}

fn flag(fields: &Fields, key: &str) -> bool {
    fields.get(key).and_then(|v| v.bool_value) == Some(true)
}

/// Why `def` (a type, relation or tool object) can't be deleted, or "" when it can.
pub fn deletion_blocker(def: &Object, cx: &mut App) -> Task<Result<String>> {
    if def.type_key == "tool" {
        return Task::ready(Ok(if flag(&def.fields, "tool_builtin") {
            "Built into Roostr".into()
        } else {
            String::new()
        }));
    }
    if def.type_key != "type" && def.type_key != "relation" {
        return Task::ready(Ok(String::new()));
    }
    if flag(&def.fields, "bundled") {
        return Task::ready(Ok("Built into Roostr".into()));
    }
    if flag(&def.fields, "system") {
        return Task::ready(Ok("A Roostr default".into()));
    }
    let key = def.field_str("key").to_string();
    let space = def.field_str("channel").to_string();
    let is_type = def.type_key == "type";
    let writers = (!is_type && !key.is_empty()).then(|| {
        // A Skill names the property its result is written to ("Writes to").
        api::query_all(
            json!({ "filters": [{ "key": "jev_writes", "condition": "equal", "value": key }, { "key": "channel", "condition": "equal", "value": space }] }),
            cx,
        )
    });
    let kinds = if is_type {
        json!(["query", "collection", "set", "template", "relation"])
    } else {
        json!(["query", "collection", "set"])
    };
    let rows = api::query_all(
        json!({ "filters": [{ "key": "typeKey", "condition": "in", "value": kinds }, { "key": "channel", "condition": "equal", "value": space }] }),
        cx,
    );
    let def_id = def.id.clone();
    cx.background_spawn(async move {
        if let Some(writers) = writers
            && let Some(w) = writers.await?.first()
        {
            return Ok(format!("Skill “{}” writes to it", row_name(w)));
        }
        for r in rows.await? {
            if r.id == def_id {
                continue;
            }
            if is_type {
                if r.type_key == "template" && field_str(&r.fields, "target_type") == def_id {
                    return Ok(format!("Used by template “{}”", row_name(&r)));
                }
                if r.type_key == "relation"
                    && str_items(r.fields.get("object_types")).contains(&def_id.as_str())
                {
                    return Ok(format!("Property “{}” is limited to it", row_name(&r)));
                }
                if !key.is_empty() && str_items(r.fields.get("setOf")).contains(&key.as_str()) {
                    return Ok(format!("Used by query “{}”", row_name(&r)));
                }
            } else if !key.is_empty()
                && (map_keys(r.fields.get("viewFilters")).contains(&key.as_str())
                    || map_keys(r.fields.get("viewSorts")).contains(&key.as_str())
                    || field_str(&r.fields, "viewGroupKey") == key
                    || field_str(&r.fields, "viewDateKey") == key)
            {
                return Ok(format!("Used by query “{}”", row_name(&r)));
            }
        }
        Ok(String::new())
    })
}

/// Blockers for a selection of object ids, keyed by id; absent ids are deletable.
pub fn blockers_for(ids: &[String], cx: &mut App) -> Task<Result<HashMap<String, String>>> {
    let store = Store::global(cx).read(cx);
    let defs: Vec<String> = ids
        .iter()
        .filter(|id| {
            store.relations.iter().any(|r| &r.id == *id)
                || store
                    .summaries
                    .iter()
                    .any(|s| &s.id == *id && (s.type_key == "type" || s.type_key == "tool"))
        })
        .cloned()
        .collect();
    cx.spawn(async move |cx| {
        let mut out = HashMap::new();
        for id in defs {
            let object = cx
                .update(|cx| Store::global(cx).read(cx).fetch_object(&id, cx))
                .await?;
            let why = cx.update(|cx| deletion_blocker(&object, cx)).await?;
            if !why.is_empty() {
                out.insert(id, why);
            }
        }
        Ok(out)
    })
}

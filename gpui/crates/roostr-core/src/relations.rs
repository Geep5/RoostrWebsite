//! Shared property/relation helpers (`src/lib/relations.ts`): reserved keys,
//! creatable formats, per-format empty values, and create-from-scratch.

use anyhow::Result;
use gpui_kit::{App, Task};

use crate::api;
use crate::nav::Navigator;
use crate::store::Store;
use crate::types::{Fields, RelationDef, Value, field_str};

/// Keys that are never user-facing properties.
pub const RESERVED_KEYS: &[&str] = &[
    "name",
    "iconEmoji",
    "iconImage",
    "setOf",
    "featuredRelations",
    "collectionIds",
    "links", // derived from link blocks
    "viewFilters",
    "viewSorts",
    "viewRelations",
    "channel",
    "pinnedIds",
    "pin_order", // a pinned object's sidebar position (pins.rs)
    "members",
    "keyId",
];

pub fn is_reserved_key(key: &str) -> bool {
    RESERVED_KEYS.contains(&key)
}

/// Creatable formats (Anytype RelationType minus File/Icon/Relations).
pub const CREATABLE_FORMATS: &[&str] = &[
    "shorttext",
    "longtext",
    "number",
    "status",
    "tag",
    "date",
    "checkbox",
    "url",
    "email",
    "phone",
    "object",
];

/// Sidebar/list glyph per relation format.
pub fn format_glyph(format: &str) -> &'static str {
    match format {
        "number" => "#",
        "status" => "◐",
        "tag" => "◧",
        "date" => "📅",
        "checkbox" => "☑",
        "url" => "🔗",
        "email" => "＠",
        "phone" => "☎",
        "object" => "▣",
        _ => "≡",
    }
}

/// The properties of `space_id`, one def per key (the lowest id wins when
/// two devices created the same key), in first-seen order.
pub fn space_relations<'a>(relations: &'a [RelationDef], space_id: &str) -> Vec<&'a RelationDef> {
    let mut out: Vec<&RelationDef> = Vec::new();
    for r in relations
        .iter()
        .filter(|r| r.space.as_deref().unwrap_or("") == space_id)
    {
        match out.iter_mut().find(|held| held.key == r.key) {
            Some(held) => {
                if r.id < held.id {
                    *held = r;
                }
            }
            None => out.push(r),
        }
    }
    out
}

/// The active space id, falling back to the default (oldest) space.
pub fn current_space_id(cx: &App) -> String {
    let space = Navigator::global(cx).read(cx).space().to_string();
    if !space.is_empty() {
        return space;
    }
    Store::global(cx)
        .read(cx)
        .channels
        .first()
        .map(|s| s.id.clone())
        .unwrap_or_default()
}

/// The space an object belongs to (the default space when unstamped).
pub fn object_space_id(store: &Store, fields: &Fields) -> String {
    let channel = field_str(fields, "channel");
    if !channel.is_empty() {
        return channel.to_string();
    }
    store
        .channels
        .first()
        .map(|s| s.id.clone())
        .unwrap_or_default()
}

pub fn empty_value_for(format: &str) -> Value {
    match format {
        "checkbox" => Value::bool(false),
        "tag" | "object" => Value::values(vec![]),
        "number" | "date" => Value::int(0),
        _ => Value::string(""),
    }
}

/// `slugKey`: lower-case, non-alphanumerics → `_`, trimmed; `prop_<ms>` when nothing is left.
pub fn slug_key(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    if out.is_empty() {
        format!("prop_{}", chrono::Utc::now().timestamp_millis())
    } else {
        out
    }
}

/// Create a relation object in `space_id` and refresh the store; resolves to
/// the def (the existing one when the space already has the key).
/// `object_types` (type object ids) apply to the `object` format only.
pub fn create_relation(
    name: &str,
    format: &str,
    object_types: &[String],
    space_id: &str,
    cx: &mut App,
) -> Task<Result<RelationDef>> {
    let key = slug_key(name);
    let store = Store::global(cx).read(cx);
    if let Some(existing) = space_relations(&store.relations, space_id)
        .into_iter()
        .find(|r| r.key == key)
    {
        return Task::ready(Ok(existing.clone()));
    }
    let limits: Vec<String> = if format == "object" {
        object_types.to_vec()
    } else {
        vec![]
    };
    let max_count = if format == "status" { 1 } else { 0 };
    let mut fields = Fields::new();
    fields.insert("channel".into(), Value::string(space_id));
    fields.insert("key".into(), Value::string(&key));
    fields.insert("name".into(), Value::string(name));
    fields.insert("format".into(), Value::string(format));
    fields.insert("hidden".into(), Value::bool(false));
    fields.insert("readOnly".into(), Value::bool(false));
    fields.insert("maxCount".into(), Value::int(max_count));
    fields.insert("options".into(), Value::values(vec![]));
    fields.insert("bundled".into(), Value::bool(false));
    if !limits.is_empty() {
        fields.insert(
            "object_types".into(),
            Value::values(limits.iter().map(Value::string).collect()),
        );
    }
    let create = api::note::create(name, Some("relation"), Some(fields), cx);
    let def = RelationDef {
        id: String::new(),
        key,
        format: format.to_string(),
        name: name.to_string(),
        space: Some(space_id.to_string()),
        max_count,
        object_types: (!limits.is_empty()).then_some(limits),
        ..RelationDef::default()
    };
    cx.spawn(async move |cx| {
        let id = create.await?;
        cx.update(|cx| Store::global(cx).update(cx, |store, cx| store.refresh_all(cx)));
        Ok(RelationDef { id, ..def })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug_key("Due Date"), "due_date");
        assert_eq!(slug_key("  --Team #2!! "), "team_2");
        assert_eq!(slug_key("Ünïcode x"), "n_code_x");
        assert!(slug_key("!!!").starts_with("prop_"));
    }

    /// A relation as `/api/relations` sends it, `options` patched in or out.
    fn relation(options: Option<serde_json::Value>) -> serde_json::Value {
        let mut wire =
            serde_json::json!({ "id": "r", "key": "status", "name": "Status", "format": "status" });
        if let Some(options) = options {
            wire["options"] = options;
        }
        wire
    }

    #[test]
    fn a_relation_with_no_options_list_gets_an_empty_one() {
        let [fixed]: [RelationDef; 1] =
            serde_json::from_value(serde_json::json!([relation(None)])).unwrap();
        assert!(fixed.options.is_empty());
        assert!(!fixed.options.iter().any(|o| o.text == "Done"));
    }

    #[test]
    fn a_wrongly_typed_options_list_is_replaced_not_trusted() {
        let wire = serde_json::json!([
            relation(Some(serde_json::Value::Null)),
            relation(Some(serde_json::json!({})))
        ]);
        let [a, b]: [RelationDef; 2] = serde_json::from_value(wire).unwrap();
        assert!(a.options.is_empty());
        assert!(b.options.is_empty());
    }

    #[test]
    fn real_options_arrive_exactly_as_sent() {
        let options = serde_json::json!([
            { "id": "1", "text": "Todo", "color": "grey", "orderId": "000000" },
            { "id": "2", "text": "Done", "color": "green", "orderId": "000001" },
        ]);
        let [fixed]: [RelationDef; 1] =
            serde_json::from_value(serde_json::json!([relation(Some(options.clone()))])).unwrap();
        assert_eq!(serde_json::to_value(&fixed.options).unwrap(), options);
        let texts: Vec<&str> = fixed.options.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, ["Todo", "Done"]);
    }
}

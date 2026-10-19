//! `viewFilters` → engine filter conversion (`src/lib/filters.ts`), shared by
//! the object page and the pinned sidebar widget so both show the same records.

use serde_json::{Value as Json, json};

use crate::types::{Fields, RelationDef, SYSTEM_TYPE_KEYS, field_str};

/// Filter keys the engine cannot evaluate: `serving` is host-computed and
/// applied client-side after the engine query.
pub const VIRTUAL_FILTER_KEYS: &[&str] = &["serving"];

/// A stored serving filter rule for the client-side pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServingFilter {
    pub condition: String,
    pub value: String,
}

fn rules(fields: &Fields) -> impl Iterator<Item = &Fields> {
    fields
        .get("viewFilters")
        .map(|v| v.items())
        .unwrap_or_default()
        .iter()
        .filter_map(|i| i.entries())
}

fn entry_str<'a>(e: &'a Fields, key: &str) -> Option<&'a str> {
    e.get(key).and_then(|v| v.as_str())
}

/// `servingFiltersOf`: the view's `serving` rules with a value.
pub fn serving_filters_of(fields: &Fields) -> Vec<ServingFilter> {
    rules(fields)
        .filter(|e| entry_str(e, "key") == Some("serving"))
        .filter_map(|e| {
            let value = e
                .get("value")
                .map(|v| v.items())
                .unwrap_or_default()
                .iter()
                .map(|i| i.as_str().unwrap_or(""))
                .find(|s| !s.is_empty())?
                .to_string();
            Some(ServingFilter {
                condition: entry_str(e, "condition").unwrap_or("equal").to_string(),
                value,
            })
        })
        .collect()
}

/// JavaScript `Number(s)` as JSON: blank is 0, junk is `null` (NaN stringifies to null).
pub fn js_number(s: &str) -> Json {
    let t = s.trim();
    if t.is_empty() {
        return json!(0);
    }
    match t.parse::<f64>() {
        Ok(n) if n.is_finite() && n.fract() == 0. && n.abs() < 9.0e15 => json!(n as i64),
        Ok(n) if n.is_finite() => json!(n),
        _ => Json::Null,
    }
}

/// `engineFiltersOf`: the engine filters of a set/query object's `viewFields`.
/// An unsourced query (empty `setOf`) also excludes the system types.
pub fn engine_filters_of(fields: &Fields, relations: &[RelationDef]) -> Vec<Json> {
    let mut out = Vec::new();
    for e in rules(fields) {
        let key = entry_str(e, "key").unwrap_or("");
        if VIRTUAL_FILTER_KEYS.contains(&key) || key.is_empty() {
            continue;
        }
        let condition = entry_str(e, "condition").unwrap_or("equal");
        let values: Vec<&str> = e
            .get("value")
            .map(|v| v.items())
            .unwrap_or_default()
            .iter()
            .filter_map(|i| i.as_str())
            .collect();
        let format = if key == "createdAt" || key == "updatedAt" {
            "number"
        } else {
            relations
                .iter()
                .find(|r| r.key == key)
                .map_or("shorttext", |r| r.format.as_str())
        };
        let value: Option<Json> = if format == "checkbox" {
            Some(json!(true))
        } else if matches!(condition, "in" | "notIn" | "allIn" | "exactIn") {
            Some(json!(values))
        } else if format == "number" || format == "date" {
            values.first().map(|v| js_number(v))
        } else {
            values.first().map(|v| json!(v))
        };
        let incomplete = match &value {
            None => true,
            Some(Json::String(s)) => s.is_empty(),
            Some(Json::Array(a)) => a.is_empty(),
            _ => false,
        };
        if !matches!(condition, "empty" | "notEmpty" | "exists")
            && format != "checkbox"
            && incomplete
        {
            continue; // incomplete rule - don't filter on it yet
        }
        let mut rule = serde_json::Map::new();
        rule.insert("key".into(), json!(key));
        rule.insert("condition".into(), json!(condition));
        if let Some(value) = value {
            rule.insert("value".into(), value);
        }
        out.push(Json::Object(rule));
    }
    if fields
        .get("setOf")
        .map(|v| v.items())
        .unwrap_or_default()
        .is_empty()
    {
        out.push(json!({ "key": "typeKey", "condition": "notIn", "value": SYSTEM_TYPE_KEYS }));
    }
    out
}

/// `spaceFilterOf`: a set/query only matches objects of its own space; the
/// default space also includes unstamped objects.
pub fn space_filter_of(fields: &Fields, default_space_id: &str) -> Json {
    let owning = field_str(fields, "channel");
    let owning = if owning.is_empty() {
        default_space_id
    } else {
        owning
    };
    if owning == default_space_id {
        json!({ "key": "channel", "condition": "in", "value": [owning, ""] })
    } else {
        json!({ "key": "channel", "condition": "equal", "value": owning })
    }
}

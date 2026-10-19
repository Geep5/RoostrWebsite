//! What a saved view's filters say about anything made inside it
//! (`src/lib/view-seeds.ts`): every equal / in / allIn filter on a property
//! becomes that property's value, so a record born in a filtered view
//! satisfies the view.

use crate::types::{Fields, RelationDef, Value};

/// `Number(s) || 0`-style parse: blank, junk or zero → `None`.
fn number(s: &str) -> Option<f64> {
    let t = s.trim();
    let n = if t.is_empty() {
        0.
    } else {
        t.parse::<f64>().ok()?
    };
    (n != 0. && n.is_finite()).then_some(n)
}

/// The fields the view (its `viewFilters`) gives a new object.
pub fn seeds_from_view(view_fields: &Fields, relations: &[RelationDef]) -> Fields {
    let mut fields = Fields::new();
    let rules = view_fields
        .get("viewFilters")
        .map(|v| v.items())
        .unwrap_or_default();
    for e in rules.iter().filter_map(Value::entries) {
        let key = e.get("key").and_then(Value::as_str).unwrap_or("");
        let condition = e
            .get("condition")
            .and_then(Value::as_str)
            .unwrap_or("equal");
        let values: Vec<&str> = e
            .get("value")
            .map(|v| v.items())
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if !matches!(condition, "equal" | "in" | "allIn") {
            continue;
        }
        let Some(rel) = relations.iter().find(|r| r.key == key) else {
            continue;
        };
        if rel.format == "checkbox" {
            // Equal means "checked" unless an explicit first value says otherwise.
            fields.insert(
                key.to_string(),
                Value::bool(values.first().is_none_or(|v| *v != "false")),
            );
            continue;
        }
        let Some(first) = values.first() else {
            continue;
        };
        let value = match rel.format.as_str() {
            "tag" | "status" => Value::values(values.iter().map(|v| Value::string(*v)).collect()),
            "number" => Value::float(number(first).unwrap_or(0.)),
            "date" => Value::int(
                number(first).map_or_else(|| chrono::Utc::now().timestamp_millis(), |n| n as i64),
            ),
            _ => Value::string(*first),
        };
        fields.insert(key.to_string(), value);
    }
    fields
}

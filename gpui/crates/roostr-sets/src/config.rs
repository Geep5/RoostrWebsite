//! The view configuration a query / collection / type object carries
//! (`viewFilters`, `viewSorts`, `viewRelations`, `viewType`, `viewGroupKey`,
//! `viewDateKey`) and the condition catalog of QueryControls.svelte.

use roostr_core::{Fields, RelationDef, Value};

/// One stored filter rule (`FilterRule`). Values are always strings.
/// `quick_option` is the engine's relative-date rule (`numberOfDaysAgo`,
/// value = the day count); empty for an absolute rule.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FilterRule {
    pub key: String,
    pub condition: String,
    pub quick_option: String,
    pub value: Vec<String>,
}

/// One stored sort rule (`SortRule`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SortRule {
    pub key: String,
    pub desc: bool,
    /// Where empty values sort: "start" | "end".
    pub empty_start: bool,
}

impl SortRule {
    pub fn type_str(&self) -> &'static str {
        if self.desc { "desc" } else { "asc" }
    }
}

/// A table column: a relation key and its width (Anytype viewRelation).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Col {
    pub key: String,
    pub width: f32,
}

pub(crate) const DEFAULT_WIDTH: f32 = 150.;
pub(crate) const MIN_WIDTH: f32 = 60.;
/// Name's share when it has no width of its own.
pub(crate) const NAME_MIN_WIDTH: f32 = 180.;
/// Anytype default grid columns for a fresh view.
const DEFAULT_COLUMNS: &[&str] = &["type", "updatedAt"];

fn str_items(v: Option<&Value>) -> Vec<String> {
    v.map(|v| {
        v.items()
            .iter()
            .filter_map(|i| i.string_value.clone())
            .collect()
    })
    .unwrap_or_default()
}

fn entry_str<'a>(e: &'a Fields, key: &str) -> Option<&'a str> {
    e.get(key).and_then(Value::as_str)
}

pub(crate) fn parse_filters(fields: &Fields) -> Vec<FilterRule> {
    let Some(v) = fields.get("viewFilters") else {
        return Vec::new();
    };
    v.items()
        .iter()
        .filter_map(Value::entries)
        .map(|e| FilterRule {
            key: entry_str(e, "key").unwrap_or("").to_string(),
            condition: entry_str(e, "condition").unwrap_or("equal").to_string(),
            quick_option: entry_str(e, "quickOption").unwrap_or("").to_string(),
            value: str_items(e.get("value")),
        })
        .collect()
}

pub(crate) fn parse_sorts(fields: &Fields) -> Vec<SortRule> {
    let Some(v) = fields.get("viewSorts") else {
        return Vec::new();
    };
    v.items()
        .iter()
        .filter_map(Value::entries)
        .map(|e| SortRule {
            key: entry_str(e, "key").unwrap_or("").to_string(),
            desc: entry_str(e, "type") == Some("desc"),
            empty_start: entry_str(e, "empty") == Some("start"),
        })
        .collect()
}

/// The object page's `viewSorts` (keyed rules only), as the views take them.
pub(crate) fn view_sorts(fields: &Fields) -> Vec<SortRule> {
    parse_sorts(fields)
        .into_iter()
        .filter(|s| !s.key.is_empty())
        .map(|s| SortRule {
            empty_start: false,
            ..s
        })
        .collect()
}

pub(crate) fn filters_value(rules: &[FilterRule]) -> Value {
    Value::values(
        rules
            .iter()
            .map(|f| {
                let mut e = Fields::new();
                e.insert("key".into(), Value::string(&f.key));
                e.insert("condition".into(), Value::string(&f.condition));
                if !f.quick_option.is_empty() {
                    e.insert("quickOption".into(), Value::string(&f.quick_option));
                }
                e.insert(
                    "value".into(),
                    Value::values(f.value.iter().map(Value::string).collect()),
                );
                Value::map(e)
            })
            .collect(),
    )
}

pub(crate) fn sorts_value(rules: &[SortRule]) -> Value {
    Value::values(
        rules
            .iter()
            .map(|s| {
                let mut e = Fields::new();
                e.insert("key".into(), Value::string(&s.key));
                e.insert("type".into(), Value::string(s.type_str()));
                e.insert(
                    "empty".into(),
                    Value::string(if s.empty_start { "start" } else { "end" }),
                );
                Value::map(e)
            })
            .collect(),
    )
}

/// The stored columns (`viewRelations`), defaults for a fresh view.
pub(crate) fn stored_columns(fields: &Fields) -> Vec<Col> {
    let mut out = Vec::new();
    if let Some(v) = fields.get("viewRelations") {
        for i in v.items() {
            if let Some(s) = &i.string_value {
                out.push(Col {
                    key: s.clone(),
                    width: DEFAULT_WIDTH,
                });
            } else if let Some(e) = i.entries() {
                // "name" carries the Name column's width: view state, not a column.
                if let Some(k) = entry_str(e, "key")
                    && !k.is_empty()
                    && k != "name"
                {
                    let width = e
                        .get("width")
                        .and_then(|w| w.int_value)
                        .map_or(DEFAULT_WIDTH, |w| w as f32);
                    out.push(Col {
                        key: k.to_string(),
                        width,
                    });
                }
            }
        }
    }
    if out.is_empty() {
        DEFAULT_COLUMNS
            .iter()
            .map(|k| Col {
                key: (*k).to_string(),
                width: DEFAULT_WIDTH,
            })
            .collect()
    } else {
        out
    }
}

/// The Name column's persisted width; `None` = auto.
pub(crate) fn stored_name_width(fields: &Fields) -> Option<f32> {
    let v = fields.get("viewRelations")?;
    for i in v.items() {
        if let Some(e) = i.entries()
            && entry_str(e, "key") == Some("name")
            && let Some(w) = e.get("width").and_then(|w| w.int_value)
            && (w as f32) >= MIN_WIDTH
        {
            return Some(w as f32);
        }
    }
    None
}

/// The visible relation keys a gallery card shows (viewRelations map entries, name excluded).
pub(crate) fn visible_keys(fields: &Fields) -> Vec<String> {
    let Some(v) = fields.get("viewRelations") else {
        return Vec::new();
    };
    v.items()
        .iter()
        .filter_map(|i| i.entries().and_then(|e| entry_str(e, "key")))
        .filter(|k| !k.is_empty() && *k != "name")
        .map(str::to_string)
        .collect()
}

pub(crate) fn columns_value(cols: &[Col], name_width: Option<f32>) -> Value {
    let mut items: Vec<Value> = cols
        .iter()
        .map(|c| {
            let mut e = Fields::new();
            e.insert("key".into(), Value::string(&c.key));
            e.insert("width".into(), Value::int(c.width.round() as i64));
            Value::map(e)
        })
        .collect();
    if let Some(w) = name_width {
        let mut e = Fields::new();
        e.insert("key".into(), Value::string("name"));
        e.insert("width".into(), Value::int(w.round() as i64));
        items.push(Value::map(e));
    }
    Value::values(items)
}

/// `setOf` source type keys.
pub(crate) fn sources(fields: &Fields) -> Vec<String> {
    str_items(fields.get("setOf"))
}

/// `collectionIds` members.
pub(crate) fn member_ids(fields: &Fields) -> Vec<String> {
    roostr_core::collection::members(fields)
}

pub(crate) fn field_or<'a>(fields: &'a Fields, key: &str, default: &'a str) -> &'a str {
    match fields.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => s,
        _ => default,
    }
}

pub(crate) fn view_type(fields: &Fields) -> &str {
    field_or(fields, "viewType", "table")
}

pub(crate) fn group_key(fields: &Fields) -> &str {
    field_or(fields, "viewGroupKey", "")
}

pub(crate) fn date_key(fields: &Fields) -> &str {
    field_or(fields, "viewDateKey", "createdDate")
}

// ── Condition catalog per relation format ─────────────────────

pub(crate) struct ConditionDef {
    pub id: &'static str,
    /// The engine quick option this condition sets (`""` = none).
    pub quick: &'static str,
    /// The menu row's label.
    pub label: &'static str,
    /// The condition button's label once chosen (the value follows it).
    pub chip: &'static str,
    /// Text after the value field (`days ago`).
    pub unit: &'static str,
    pub needs_value: bool,
}

impl ConditionDef {
    /// Is this the condition `f` stores?
    pub(crate) fn matches(&self, f: &FilterRule) -> bool {
        self.id == f.condition && self.quick == f.quick_option
    }
}

const fn c(id: &'static str, label: &'static str, needs_value: bool) -> ConditionDef {
    ConditionDef {
        id,
        quick: "",
        label,
        chip: label,
        unit: "",
        needs_value,
    }
}

/// The engine's relative-date quick option: the value is a day count.
pub(crate) const DAYS_AGO: &str = "numberOfDaysAgo";

/// A relative-date condition: `less` = before the start of the day N days
/// ago, `greaterOrEqual` = within the last N days (`quick_option_range`).
const fn days(
    id: &'static str,
    label: &'static str,
    chip: &'static str,
    unit: &'static str,
) -> ConditionDef {
    ConditionDef {
        id,
        quick: DAYS_AGO,
        label,
        chip,
        unit,
        needs_value: true,
    }
}

const TEXT_CONDITIONS: &[ConditionDef] = &[
    c("like", "contains", true),
    c("notLike", "doesn't contain", true),
    c("equal", "is", true),
    c("notEqual", "is not", true),
    c("in", "is any of", true),
    c("notIn", "is none of", true),
    c("empty", "is empty", false),
    c("notEmpty", "is not empty", false),
];
const NUMBER_CONDITIONS: &[ConditionDef] = &[
    c("equal", "=", true),
    c("notEqual", "≠", true),
    c("greater", ">", true),
    c("less", "<", true),
    c("greaterOrEqual", "≥", true),
    c("lessOrEqual", "≤", true),
    c("empty", "is empty", false),
    c("notEmpty", "is not empty", false),
];
/// Dates (and Created / Updated): the number comparisons plus the relative rules.
const DATE_CONDITIONS: &[ConditionDef] = &[
    c("equal", "=", true),
    c("notEqual", "≠", true),
    c("greater", ">", true),
    c("less", "<", true),
    c("greaterOrEqual", "≥", true),
    c("lessOrEqual", "≤", true),
    days("less", "is more than … days ago", "more than", "days ago"),
    days(
        "greaterOrEqual",
        "is within the last … days",
        "within the last",
        "days",
    ),
    c("empty", "is empty", false),
    c("notEmpty", "is not empty", false),
];
const SELECT_CONDITIONS: &[ConditionDef] = &[
    c("in", "has any of", true),
    c("allIn", "has all of", true),
    c("exactIn", "is exactly", true),
    c("notIn", "has none of", true),
    c("empty", "is empty", false),
    c("notEmpty", "is not empty", false),
];
/// Links: which objects it holds (by id), or whether it holds any.
const OBJECT_CONDITIONS: &[ConditionDef] = &[
    c("in", "has any of", true),
    c("allIn", "has all of", true),
    c("notIn", "has none of", true),
    c("empty", "is empty", false),
    c("notEmpty", "is not empty", false),
];
/// Repeat is a rule, not a value: the only question is whether there is one.
const REPEAT_CONDITIONS: &[ConditionDef] = &[
    c("notEmpty", "is recurring", false),
    c("empty", "is not recurring", false),
];
const CHECKBOX_CONDITIONS: &[ConditionDef] = &[
    c("equal", "is checked", false),
    c("notEqual", "is unchecked", false),
];
/// The serving resolver's reasons plus the two rolled-up states.
const SERVING_CONDITIONS: &[ConditionDef] =
    &[c("equal", "is", true), c("notEqual", "is not", true)];
pub(crate) const SERVING_OPTIONS: &[&str] = &[
    "attention",
    "ok",
    "pinned",
    "agent",
    "capability",
    "unsatisfied",
    "pinned-uncapable",
    "unserved",
];

/// Conditions that take a list of values.
pub(crate) fn multi_value(condition: &str) -> bool {
    matches!(condition, "in" | "notIn" | "allIn" | "exactIn")
}

fn find<'a>(relations: &'a [RelationDef], key: &str) -> Option<&'a RelationDef> {
    relations.iter().find(|r| r.key == key)
}

pub(crate) fn filter_format(relations: &[RelationDef], key: &str) -> String {
    if key == "type" || key == "id" {
        return "shorttext".into();
    }
    if key == "createdAt" || key == "updatedAt" {
        return "number".into();
    }
    find(relations, key).map_or_else(|| "shorttext".into(), |r| r.format.clone())
}

pub(crate) fn conditions_for(relations: &[RelationDef], key: &str) -> &'static [ConditionDef] {
    if key == "serving" {
        return SERVING_CONDITIONS;
    }
    if key == "repeat" {
        return REPEAT_CONDITIONS;
    }
    if key == "createdAt" || key == "updatedAt" {
        return DATE_CONDITIONS;
    }
    match filter_format(relations, key).as_str() {
        "date" => DATE_CONDITIONS,
        "number" => NUMBER_CONDITIONS,
        "tag" | "status" => SELECT_CONDITIONS,
        "checkbox" => CHECKBOX_CONDITIONS,
        "object" => OBJECT_CONDITIONS,
        _ => TEXT_CONDITIONS,
    }
}

/// The catalog entry rule `f` stores, if the key's catalog has it.
pub(crate) fn condition_of(
    relations: &[RelationDef],
    f: &FilterRule,
) -> Option<&'static ConditionDef> {
    conditions_for(relations, &f.key)
        .iter()
        .find(|c| c.matches(f))
}

pub(crate) fn options_for(relations: &[RelationDef], key: &str) -> Vec<String> {
    if key == "serving" {
        return SERVING_OPTIONS.iter().map(|s| (*s).to_string()).collect();
    }
    find(relations, key)
        .map(|r| r.options.iter().map(|o| o.text.clone()).collect())
        .unwrap_or_default()
}

/// An option's own tag colour name.
pub(crate) fn option_color(relations: &[RelationDef], key: &str, text: &str) -> String {
    find(relations, key)
        .and_then(|r| r.options.iter().find(|o| o.text == text))
        .map(|o| o.color.clone())
        .unwrap_or_default()
}

pub(crate) fn needs_value(relations: &[RelationDef], f: &FilterRule) -> bool {
    condition_of(relations, f).is_none_or(|c| c.needs_value)
}

/// Filterable keys: virtual keys + every non-hidden relation, unique.
pub(crate) fn filter_keys(relations: &[RelationDef]) -> Vec<String> {
    let mut out: Vec<String> = vec!["type".into()];
    for r in relations {
        if !r.hidden && r.key != "setOf" && r.key != "served_by" && !out.contains(&r.key) {
            out.push(r.key.clone());
        }
    }
    for k in ["createdAt", "updatedAt", "serving", "repeat"] {
        if !out.iter().any(|x| x == k) {
            out.push(k.into());
        }
    }
    out
}

pub(crate) fn label_of(relations: &[RelationDef], key: &str) -> String {
    match key {
        "serving" => "Served by".into(),
        "repeat" => "Repeat".into(),
        "type" => "Type".into(),
        "createdAt" => "Created".into(),
        "updatedAt" => "Updated".into(),
        _ => find(relations, key)
            .map(|r| r.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| key.to_string()),
    }
}

/// A property choice as the menu shows it: its emoji, then its name.
pub(crate) fn option_label(relations: &[RelationDef], key: &str) -> String {
    let glyph = match key {
        "type" => "▦".to_string(),
        "createdAt" | "updatedAt" => "🗓️".to_string(),
        "serving" => "🖥️".to_string(),
        "repeat" => "↻".to_string(),
        _ => find(relations, key)
            .and_then(|r| r.icon_emoji.clone())
            .unwrap_or_default(),
    };
    let label = label_of(relations, key);
    if glyph.is_empty() {
        label
    } else {
        format!("{glyph} {label}")
    }
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A property a board can group by or a calendar can place records on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ViewProp {
    pub key: String,
    pub name: String,
    pub emoji: String,
}

/// Does `v` hold a value (QueryControls `hasValue`)?
pub(crate) fn has_value(v: &Value) -> bool {
    if v.bool_value.is_some() {
        return true;
    }
    if let Some(vv) = &v.values_value {
        return !vv.items.is_empty();
    }
    if let Some(s) = &v.string_value {
        return !s.is_empty();
    }
    v.int_value.is_some_and(|n| n != 0) || v.float_value.is_some_and(|n| n != 0.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_round_trip() {
        let rules = vec![
            FilterRule {
                key: "tag".into(),
                condition: "in".into(),
                value: vec!["a".into(), "b".into()],
                ..FilterRule::default()
            },
            FilterRule {
                key: "updatedAt".into(),
                condition: "less".into(),
                quick_option: DAYS_AGO.into(),
                value: vec!["30".into()],
            },
        ];
        let mut fields = Fields::new();
        fields.insert("viewFilters".into(), filters_value(&rules));
        assert_eq!(parse_filters(&fields), rules);
    }

    /// Dates offer the relative rules beside the comparisons; plain numbers don't.
    #[test]
    fn dates_offer_relative_days() {
        let rule = FilterRule {
            key: "updatedAt".into(),
            condition: "less".into(),
            quick_option: DAYS_AGO.into(),
            value: vec!["30".into()],
        };
        assert_eq!(condition_of(&[], &rule).map(|c| c.chip), Some("more than"));
        let absolute = FilterRule {
            quick_option: String::new(),
            ..rule
        };
        assert_eq!(condition_of(&[], &absolute).map(|c| c.label), Some("<"));
        let num = RelationDef {
            key: "n".into(),
            format: "number".into(),
            ..RelationDef::default()
        };
        assert!(
            conditions_for(&[num], "n")
                .iter()
                .all(|c| c.quick.is_empty())
        );
    }

    #[test]
    fn columns_skip_name_entry() {
        let mut fields = Fields::new();
        fields.insert(
            "viewRelations".into(),
            columns_value(
                &[Col {
                    key: "status".into(),
                    width: 130.,
                }],
                Some(260.),
            ),
        );
        assert_eq!(
            stored_columns(&fields),
            vec![Col {
                key: "status".into(),
                width: 130.
            }]
        );
        assert_eq!(stored_name_width(&fields), Some(260.));
    }

    /// Filtering on an object property (Agent, Linked projects...) can ask
    /// which objects it holds - it offered only "is empty" / "is not empty".
    #[test]
    fn object_properties_filter_by_which_objects() {
        let rel = RelationDef {
            key: "agent".into(),
            format: "object".into(),
            ..RelationDef::default()
        };
        let ids: Vec<&str> = conditions_for(&[rel], "agent")
            .iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, ["in", "allIn", "notIn", "empty", "notEmpty"]);
    }
}

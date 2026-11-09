//! Select/multi-select options (`src/lib/options.ts`): Anytype's option
//! system. Options live on the relation object as `{id, text, color, orderId}`;
//! `color` is a palette name. Field values store the option TEXT, so rename
//! and delete rewrite every object carrying the value.
//!
//! The palette's paint (pill, badge and swatch colours) is the design
//! system's: it maps each [`Hue`] to the kit's colour scale.

use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use roostr_rt::Task;
use serde_json::json;

use crate::icons::Glyph;
use crate::types::{Fields, RelationDef, RelationOption, Value};
use crate::{State, api};

/// A hue of the kit's colour scales (Tailwind's names).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hue {
    Gray,
    Red,
    Orange,
    Amber,
    Yellow,
    Lime,
    Green,
    Emerald,
    Teal,
    Cyan,
    Sky,
    Blue,
    Indigo,
    Violet,
    Purple,
    Fuchsia,
    Pink,
    Rose,
}

/// One palette entry: a stored option colour `name` and the kit hue it
/// paints with. The tag pill is the kit's own `Tag::color` look (dark
/// theme): fill 950 at 50%, border 800 at 50%, text 300; the vivid tone is
/// 500 (swatches, badge icons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagColor {
    pub name: &'static str,
    pub hue: Hue,
}

const fn tc(name: &'static str, hue: Hue) -> TagColor {
    TagColor { name, hue }
}

/// The option palette, in the kit's hue order. The first ten names are
/// Anytype's (`grey`, `ice` = the kit's Gray and Sky), kept so stored options
/// keep their colour; the rest are the kit colours Anytype lacks.
pub const TAG_COLORS: &[TagColor] = &[
    tc("grey", Hue::Gray),
    tc("red", Hue::Red),
    tc("orange", Hue::Orange),
    tc("amber", Hue::Amber),
    tc("yellow", Hue::Yellow),
    tc("lime", Hue::Lime),
    tc("green", Hue::Green),
    tc("emerald", Hue::Emerald),
    tc("teal", Hue::Teal),
    tc("cyan", Hue::Cyan),
    tc("ice", Hue::Sky),
    tc("blue", Hue::Blue),
    tc("indigo", Hue::Indigo),
    tc("violet", Hue::Violet),
    tc("purple", Hue::Purple),
    tc("fuchsia", Hue::Fuchsia),
    tc("pink", Hue::Pink),
    tc("rose", Hue::Rose),
];

pub fn tag_color(name: &str) -> Option<&'static TagColor> {
    TAG_COLORS.iter().find(|c| c.name == name)
}

/// Status option text → badge glyph by keyword, so "Approved" and "done"
/// share a bucket. Unknown → a plain dot.
pub fn status_icon(text: &str) -> Glyph {
    static RULES: LazyLock<Vec<(Regex, Glyph)>> = LazyLock::new(|| {
        [
            (
                r"done|approved|complete|completed|resolved|shipped|closed",
                Glyph::BadgeCheck,
            ),
            (
                r"in progress|progress|doing|active|working|ongoing",
                Glyph::BadgeHalf,
            ),
            (
                r"pending|waiting|review|in review|on hold|paused",
                Glyph::BadgeClock,
            ),
            (
                r"cancel|cancelled|canceled|blocked|failed|rejected|dropped",
                Glyph::BadgeX,
            ),
            (
                r"not started|todo|to do|backlog|new|open",
                Glyph::BadgeDashed,
            ),
        ]
        .into_iter()
        .map(|(words, glyph)| {
            (
                Regex::new(&format!(r"(?-u:\b)(?:{words})(?-u:\b)")).expect("valid"),
                glyph,
            )
        })
        .collect()
    });
    let t = text.to_lowercase();
    RULES
        .iter()
        .find(|(re, _)| re.is_match(&t))
        .map_or(Glyph::BadgeDot, |(_, g)| *g)
}

/// The stored `options` value of a relation.
pub fn options_value(options: &[RelationOption]) -> Value {
    Value::values(
        options
            .iter()
            .map(|o| {
                let mut e = Fields::new();
                e.insert("id".into(), Value::string(&o.id));
                e.insert("text".into(), Value::string(&o.text));
                e.insert("color".into(), Value::string(&o.color));
                e.insert("orderId".into(), Value::string(&o.order_id));
                Value::map(e)
            })
            .collect(),
    )
}

fn save(relation_id: &str, options: &[RelationOption], s: &mut State) -> Task<Result<()>> {
    let set = api::note::set_field(relation_id, "options", options_value(options), s);
    s.rt.spawn(async move |cx| {
        set.await?;
        cx.update(|s| s.store.refresh_all());
        Ok(())
    })
}

/// Create an option with a random palette color (`addOption`).
pub fn add_option(rel: &RelationDef, text: &str, s: &mut State) -> Task<Result<RelationOption>> {
    let pick = uuid::Uuid::new_v4().as_bytes()[0] as usize % TAG_COLORS.len();
    let option = RelationOption {
        id: uuid::Uuid::new_v4().to_string(),
        text: text.trim().to_string(),
        color: TAG_COLORS[pick].name.to_string(),
        order_id: format!("{:06}", rel.options.len()),
    };
    let mut all = rel.options.clone();
    all.push(option.clone());
    let save = save(&rel.id, &all, s);
    s.rt.spawn(async move |_| {
        save.await?;
        Ok(option)
    })
}

pub fn set_option_color(
    rel: &RelationDef,
    id: &str,
    color: &str,
    s: &mut State,
) -> Task<Result<()>> {
    let all: Vec<RelationOption> = rel
        .options
        .iter()
        .map(|o| {
            if o.id == id {
                RelationOption {
                    color: color.to_string(),
                    ..o.clone()
                }
            } else {
                o.clone()
            }
        })
        .collect();
    save(&rel.id, &all, s)
}

/// Rewrite `rel.key` on every object whose value contains `text`, then save `next`.
fn rewrite(
    rel: &RelationDef,
    text: String,
    replace: Option<String>,
    next: Vec<RelationOption>,
    s: &mut State,
) -> Task<Result<()>> {
    // "in" = value-list intersects the set; "equal" would miss multi-tag objects.
    let query = api::query(
        json!({ "filters": [{ "key": rel.key, "condition": "in", "value": [text] }], "limit": 500 }),
        s,
    );
    let key = rel.key.clone();
    let rel_id = rel.id.clone();
    s.rt.spawn(async move |cx| {
        for row in query.await?.records {
            let items = row
                .fields
                .get(&key)
                .map(|v| v.items().to_vec())
                .unwrap_or_default();
            let items: Vec<Value> = match &replace {
                Some(clean) => items
                    .into_iter()
                    .map(|i| {
                        if i.string_value.as_deref() == Some(text.as_str()) {
                            Value::string(clean)
                        } else {
                            i
                        }
                    })
                    .collect(),
                None => items
                    .into_iter()
                    .filter(|i| i.string_value.as_deref() != Some(text.as_str()))
                    .collect(),
            };
            cx.update(|s| api::note::set_field(&row.id, &key, Value::values(items), s))
                .await?;
        }
        cx.update(|s| save(&rel_id, &next, s)).await
    })
}

/// Rename an option and rewrite the value on every carrying object. No-op
/// for an unknown id, blank or unchanged text.
pub fn rename_option(rel: &RelationDef, id: &str, next: &str, s: &mut State) -> Task<Result<()>> {
    let clean = next.trim().to_string();
    let Some(prev) = rel.options.iter().find(|o| o.id == id) else {
        return Task::ready(Ok(()));
    };
    if clean.is_empty() || prev.text == clean {
        return Task::ready(Ok(()));
    }
    let all = rel
        .options
        .iter()
        .map(|o| {
            if o.id == id {
                RelationOption {
                    text: clean.clone(),
                    ..o.clone()
                }
            } else {
                o.clone()
            }
        })
        .collect();
    rewrite(rel, prev.text.clone(), Some(clean), all, s)
}

/// Delete an option and strip it from every carrying object.
pub fn delete_option(rel: &RelationDef, id: &str, s: &mut State) -> Task<Result<()>> {
    let Some(prev) = rel.options.iter().find(|o| o.id == id) else {
        return Task::ready(Ok(()));
    };
    let all = rel.options.iter().filter(|o| o.id != id).cloned().collect();
    rewrite(rel, prev.text.clone(), None, all, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_keywords() {
        assert_eq!(status_icon("Approved"), Glyph::BadgeCheck);
        assert_eq!(status_icon("In Progress"), Glyph::BadgeHalf);
        assert_eq!(status_icon("In review"), Glyph::BadgeClock);
        assert_eq!(status_icon("Cancelled"), Glyph::BadgeX);
        assert_eq!(status_icon("Not started"), Glyph::BadgeDashed);
        assert_eq!(status_icon("doneness"), Glyph::BadgeDot);
        assert_eq!(status_icon("Something"), Glyph::BadgeDot);
    }

    #[test]
    fn stored_anytype_names_keep_a_colour() {
        for name in [
            "grey", "yellow", "orange", "red", "pink", "purple", "blue", "ice", "teal", "lime",
        ] {
            assert!(tag_color(name).is_some(), "{name}");
        }
    }
}

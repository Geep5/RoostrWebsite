//! Select/multi-select options (`src/lib/options.ts`): Anytype's option
//! system. Options live on the relation object as `{id, text, color, orderId}`;
//! `color` is a palette name. Field values store the option TEXT, so rename
//! and delete rewrite every object carrying the value.

use std::sync::LazyLock;

use anyhow::Result;
use gpui_kit::component::ColorName;
use gpui_kit::{App, AppContext as _, Hsla, Rgba, Task};
use regex::Regex;
use serde_json::json;

use crate::api;
use crate::icons::Glyph;
use crate::store::Store;
use crate::theme;
use crate::types::{Fields, RelationDef, RelationOption, Value};

/// One palette entry: a stored option colour `name` and the gpui-kit colour
/// it paints with. The tag pill is the kit's own `Tag::color` look (dark
/// theme): fill 950 at 50%, border 800 at 50%, text 300; `hex` is the vivid
/// 500 tone (swatches, badge icons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagColor {
    pub name: &'static str,
    pub kit: ColorName,
}

impl TagColor {
    pub fn hex(&self) -> Hsla {
        self.kit.scale(500)
    }
    pub fn bg(&self) -> Hsla {
        self.kit.scale(950).opacity(0.5)
    }
    pub fn border(&self) -> Hsla {
        self.kit.scale(800).opacity(0.5)
    }
    pub fn text(&self) -> Hsla {
        self.kit.scale(300)
    }
    /// `#rrggbb` of the vivid tone, as the web stores it.
    pub fn hex_str(&self) -> String {
        let c = self.hex().to_rgb();
        let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
        format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
    }
}

const fn tc(name: &'static str, kit: ColorName) -> TagColor {
    TagColor { name, kit }
}

/// The option palette, in the kit's hue order. The first ten names are
/// Anytype's (`grey`, `ice` = the kit's Gray and Sky), kept so stored options
/// keep their colour; the rest are the kit colours Anytype lacks.
pub const TAG_COLORS: &[TagColor] = &[
    tc("grey", ColorName::Gray),
    tc("red", ColorName::Red),
    tc("orange", ColorName::Orange),
    tc("amber", ColorName::Amber),
    tc("yellow", ColorName::Yellow),
    tc("lime", ColorName::Lime),
    tc("green", ColorName::Green),
    tc("emerald", ColorName::Emerald),
    tc("teal", ColorName::Teal),
    tc("cyan", ColorName::Cyan),
    tc("ice", ColorName::Sky),
    tc("blue", ColorName::Blue),
    tc("indigo", ColorName::Indigo),
    tc("violet", ColorName::Violet),
    tc("purple", ColorName::Purple),
    tc("fuchsia", ColorName::Fuchsia),
    tc("pink", ColorName::Pink),
    tc("rose", ColorName::Rose),
];

pub fn tag_color(name: &str) -> Option<&'static TagColor> {
    TAG_COLORS.iter().find(|c| c.name == name)
}

/// `colorHex`: the palette's vivid hex for `name`, else `name` itself, else grey's.
pub fn color_hex(name: &str) -> String {
    match tag_color(name) {
        Some(c) => c.hex_str(),
        None if !name.is_empty() => name.to_string(),
        None => TAG_COLORS[0].hex_str(),
    }
}

/// Tag pill colours for a palette name; `None` = unknown/default
/// (render [`unknown_tag_bg`] with primary text).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TagPaint {
    pub bg: Hsla,
    pub fg: Hsla,
    pub border: Hsla,
}

pub fn tag_colors(name: &str) -> Option<TagPaint> {
    tag_color(name).map(|c| TagPaint {
        bg: c.bg(),
        fg: c.text(),
        border: c.border(),
    })
}

/// The pill of an option with no (known) colour: neutral fill and border, `fg` text.
pub fn unknown_tag_paint(fg: Hsla) -> TagPaint {
    TagPaint {
        bg: unknown_tag_bg(),
        fg,
        border: unknown_tag_bg(),
    }
}

/// The kit `Tag::color` look (dark theme) for any colour, e.g. a person's chat
/// colour: its hue and saturation at the kit's 950 (fill, 50%), 800 (border,
/// 50%) and 300 (text) lightness.
pub fn tint_paint(color: Hsla) -> TagPaint {
    let at = |l: f32, a: f32| Hsla {
        h: color.h,
        s: color.s,
        l,
        a,
    };
    TagPaint {
        bg: at(0.16, 0.5),
        border: at(0.32, 0.5),
        fg: at(0.8, 1.),
    }
}

/// The pill background of an option with no (known) color: `rgba(255,255,255,0.09)`.
pub fn unknown_tag_bg() -> Hsla {
    Hsla {
        h: 0.,
        s: 0.,
        l: 1.,
        a: 0.09,
    }
}

/// Property badge colors (`badgeStyle`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BadgeColors {
    pub bg: Hsla,
    pub fg: Hsla,
    pub icon: Hsla,
}

/// `color-mix(in srgb, a p, b)`: per-channel interpolation in gamma sRGB.
pub fn mix_srgb(a: Hsla, p: f32, b: Hsla) -> Hsla {
    let a = a.to_rgb();
    let b = b.to_rgb();
    let m = |x: f32, y: f32| x * p + y * (1. - p);
    Rgba {
        r: m(a.r, b.r),
        g: m(a.g, b.g),
        b: m(a.b, b.b),
        a: m(a.a, b.a),
    }
    .into()
}

/// Soft tint of the option's vivid tone over the panel (18%), its pale label,
/// and the vivid tone for the icon. Unknown → muted 14% over panel, fg, muted.
pub fn badge_colors(name: &str) -> BadgeColors {
    match tag_color(name) {
        Some(c) => BadgeColors {
            bg: mix_srgb(c.hex(), 0.18, theme::panel()),
            fg: c.text(),
            icon: c.hex(),
        },
        None => BadgeColors {
            bg: mix_srgb(theme::muted(), 0.14, theme::panel()),
            fg: theme::fg(),
            icon: theme::muted(),
        },
    }
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

fn refresh(cx: &mut App) {
    Store::global(cx).update(cx, |store, cx| store.refresh_all(cx));
}

fn save(relation_id: &str, options: &[RelationOption], cx: &mut App) -> Task<Result<()>> {
    let set = api::note::set_field(relation_id, "options", options_value(options), cx);
    cx.spawn(async move |cx| {
        set.await?;
        cx.update(refresh);
        Ok(())
    })
}

/// Create an option with a random palette color (`addOption`).
pub fn add_option(rel: &RelationDef, text: &str, cx: &mut App) -> Task<Result<RelationOption>> {
    let pick = uuid::Uuid::new_v4().as_bytes()[0] as usize % TAG_COLORS.len();
    let option = RelationOption {
        id: uuid::Uuid::new_v4().to_string(),
        text: text.trim().to_string(),
        color: TAG_COLORS[pick].name.to_string(),
        order_id: format!("{:06}", rel.options.len()),
    };
    let mut all = rel.options.clone();
    all.push(option.clone());
    let save = save(&rel.id, &all, cx);
    cx.background_spawn(async move {
        save.await?;
        Ok(option)
    })
}

pub fn set_option_color(
    rel: &RelationDef,
    id: &str,
    color: &str,
    cx: &mut App,
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
    save(&rel.id, &all, cx)
}

/// Rewrite `rel.key` on every object whose value contains `text`, then save `next`.
fn rewrite(
    rel: &RelationDef,
    text: String,
    replace: Option<String>,
    next: Vec<RelationOption>,
    cx: &mut App,
) -> Task<Result<()>> {
    // "in" = value-list intersects the set; "equal" would miss multi-tag objects.
    let query = api::query(
        json!({ "filters": [{ "key": rel.key, "condition": "in", "value": [text] }], "limit": 500 }),
        cx,
    );
    let key = rel.key.clone();
    let rel_id = rel.id.clone();
    cx.spawn(async move |cx| {
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
            cx.update(|cx| api::note::set_field(&row.id, &key, Value::values(items), cx))
                .await?;
        }
        cx.update(|cx| save(&rel_id, &next, cx)).await
    })
}

/// Rename an option and rewrite the value on every carrying object. No-op
/// for an unknown id, blank or unchanged text.
pub fn rename_option(rel: &RelationDef, id: &str, next: &str, cx: &mut App) -> Task<Result<()>> {
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
    rewrite(rel, prev.text.clone(), Some(clean), all, cx)
}

/// Delete an option and strip it from every carrying object.
pub fn delete_option(rel: &RelationDef, id: &str, cx: &mut App) -> Task<Result<()>> {
    let Some(prev) = rel.options.iter().find(|o| o.id == id) else {
        return Task::ready(Ok(()));
    };
    let all = rel.options.iter().filter(|o| o.id != id).cloned().collect();
    rewrite(rel, prev.text.clone(), None, all, cx)
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
    fn badge_mix_is_srgb_interpolation() {
        let red = tag_color("red").unwrap().hex().to_rgb();
        let panel = theme::panel().to_rgb();
        let c = badge_colors("red").bg.to_rgb();
        // 0.18 * vivid red + 0.82 * panel
        let want = |v: f32, p: f32| v * 0.18 + p * 0.82;
        assert!((c.r - want(red.r, panel.r)).abs() < 0.003);
        assert!((c.g - want(red.g, panel.g)).abs() < 0.003);
        assert!((c.b - want(red.b, panel.b)).abs() < 0.003);
        assert_eq!(badge_colors("nope").fg, theme::fg());
    }

    #[test]
    fn color_hex_falls_back() {
        let teal = color_hex("teal");
        assert!(teal.len() == 7 && teal.starts_with('#'), "{teal}");
        assert_eq!(color_hex("#123456"), "#123456");
        assert_eq!(color_hex(""), color_hex("grey"));
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

//! The graph page's overlays: the finder's result list and the side panel (totals, a type's
//! property usage, a property's users, an object's values).

use std::collections::HashMap;

use chrono::TimeZone as _;
use roostr_core::nav::{self, Route};
use roostr_core::relations::space_relations;
use roostr_core::{RelationDef, State, Value};
use roostr_ui::ty::Weights as _;
use roostr_ui::{KitSize, Link, Progress, ShimmerText, Theme, outline_button, spaced_text, ty};
use rune::{Color, Cursor, El, ScrollSpec, col, el, row, text};

use crate::geom::hex;
use crate::model::GraphObject;
use crate::scene::{Built, Derived};
use crate::view::{Page, PageKey, fly_to, set_hit};

/// The stage's text colour, which GPUI's children inherited.
const INK: u32 = 0xe7e9ee;

/// A property as the graph names it everywhere: its emoji, then its name.
pub(crate) fn prop_label(rel: &RelationDef) -> String {
    let name = if rel.name.is_empty() {
        &rel.key
    } else {
        &rel.name
    };
    match rel.icon_emoji.as_deref().filter(|s| !s.is_empty()) {
        Some(icon) => format!("{icon} {name}"),
        None => name.clone(),
    }
}

fn label_or_key(rel: Option<&RelationDef>, key: &str) -> String {
    rel.map_or_else(|| key.to_string(), prop_label)
}

/// `Math.round(u * 100) + "%"`.
fn pct(u: f64) -> String {
    format!("{}%", (u * 100.).round())
}

/// JS `String(number)` for the values the panel shows.
fn js_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0. && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// `formatValue`: a stored value as readable text.
fn format_value(rel: Option<&RelationDef>, value: &Value, names: &HashMap<&str, &str>) -> String {
    let single = std::slice::from_ref(value);
    let items = value
        .values_value
        .as_ref()
        .map_or(single, |v| v.items.as_slice());
    let name_of = |id: &str| {
        names
            .get(id)
            .copied()
            .filter(|n| !n.is_empty())
            .unwrap_or("Untitled")
            .to_string()
    };
    let format = rel.map_or("", |r| r.format.as_str());
    items
        .iter()
        .map(|it| {
            if let Some(link) = &it.link_value {
                return name_of(&link.target_id);
            }
            if let Some(b) = it.bool_value {
                return if b { "Yes" } else { "No" }.to_string();
            }
            if let Some(n) = it.int_value {
                if format == "date" {
                    return chrono::Local
                        .timestamp_millis_opt(n)
                        .earliest()
                        .map_or_else(
                            || "Invalid Date".to_string(),
                            |d| d.format("%-m/%-d/%Y").to_string(),
                        );
                }
                return n.to_string();
            }
            if let Some(f) = it.float_value {
                return js_number(f);
            }
            let s = it.string_value.as_deref().unwrap_or("");
            if format == "object" {
                return name_of(s);
            }
            rel.and_then(|r| r.options.iter().find(|o| o.id == s))
                .map_or_else(|| s.to_string(), |o| o.text.clone())
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Text in `style` and `color`.
fn label(s: impl Into<String>, style: rune::TextStyle, color: u32) -> El {
    text(s.into()).font(style).color(hex(color, 1.))
}

/// `.kicker`: mono 10px, letter-spacing .12em, #7d8496.
fn kicker(t: &Theme, s: impl Into<String>) -> El {
    spaced_text(s, 0.12)
        .family(rune::Family::Mono)
        .size(10.)
        .color(hex(0x7d8496, 1.))
        .build(t)
}

fn dot(color: Color, size: f32) -> El {
    el().size(size, size).radius(size / 2.).fill(color)
}

/// `.note`: 12px / 1.5, #7d8496.
fn note(s: impl Into<String>) -> El {
    label(s, ty::ui(12.).lh(18.), 0x7d8496).wrap()
}

fn muted_small(s: impl Into<String>) -> El {
    label(s, ty::mono(11.), 0x8a90a0)
}

/// One usage bar row: [dot] name, kit progress bar, percentage.
fn bar_row(t: &Theme, dot_color: Option<Color>, name: String, u: f64, color: Color) -> El {
    let name_w = if dot_color.is_some() { 104. } else { 96. };
    let key = format!("usage-{name}");
    let mut r = row()
        .items_center()
        .gap(if dot_color.is_some() { 9. } else { 10. });
    if let Some(c) = dot_color {
        r = r.child(dot(c, 8.));
    }
    r.child(label(name, ty::ui(13.), 0xd6dae3).truncate().w(name_w))
        .child(
            el().grow(1.).min_w(0.).child(
                Progress::new(key)
                    .size(KitSize::XSmall)
                    .color(color)
                    .value(u as f32 * 100.)
                    .build(t),
            ),
        )
        .child(el().w(36.).justify_end().child(muted_small(pct(u))))
}

/// `.field`: a key/value row with a hairline above.
fn field(key: String, value: String) -> El {
    row()
        .w_full()
        .gap(10.)
        .py(6.)
        .border_sides([1., 0., 0., 0.], Color::WHITE.alpha(0.05))
        .child(
            el().w(90.)
                .pt(1.)
                .child(label(key, ty::mono(11.), 0x7d8496).truncate()),
        )
        .child(
            el().grow(1.)
                .min_w(0.)
                .child(label(value, ty::ui(13.), 0xdde1e9).wrap().w_full()),
        )
}

/// The finder's matches (or "No objects or properties match.").
pub(crate) fn found_list(page: &Page, key: &PageKey) -> El {
    let mut list = col()
        .key("graph-found")
        .w_full()
        .max_h(360.)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .fill(hex(0x14161c, 0.95))
        .border(1., hex(0x2a2f3a, 1.))
        .radius(8.)
        .padding(4.);
    if page.found.is_empty() {
        list = list.child(el().px(8.).py(6.).child(label(
            "No objects or properties match.",
            ty::ui(12.),
            0x7d8496,
        )));
    }
    for (i, f) in page.found.iter().enumerate() {
        let on = i == page.hit;
        let (k1, k2) = (key.clone(), key.clone());
        let mut r = row()
            .key(format!("graph-found-{i}"))
            .w_full()
            .items_center()
            .gap(8.)
            .radius(6.)
            .px(8.)
            .py(6.)
            .cursor(Cursor::Pointer)
            .on_hover(move |s: &mut State| set_hit(s, &k1, i))
            .on_press_tx(move |s: &mut State, tx| fly_to(s, &k2, i, tx))
            .child(dot(f.color, 8.))
            .child(
                el().grow(1.)
                    .min_w(0.)
                    .child(label(f.name.clone(), ty::ui(13.), 0xd6dae3).truncate()),
            )
            .child(label(f.sub.clone(), ty::mono(10.), 0x7d8496));
        if on {
            r = r.fill(hex(0x262b36, 1.));
        }
        list = list.child(r);
    }
    list
}

fn titled(icon: &str, name: &str) -> String {
    if icon.is_empty() {
        name.to_string()
    } else {
        format!("{icon} {name}")
    }
}

fn type_kicker(icon: &str, name: &str) -> String {
    titled(icon, &name.to_uppercase())
}

/// Lite: "N not loaded" and, unless the space is downloading already (then its progress,
/// shimmering while it runs), "Load this space fully".
fn shadow_row(s: &State, t: &Theme, n: u64, channel: &str) -> El {
    let loading = roostr_core::lite::status(s)
        .and_then(|st| st.loading(channel).map(roostr_core::lite_ui::loading_text));
    let space = channel.to_string();
    let r = row()
        .flex_wrap()
        .items_center()
        .gap(10.)
        .child(muted_small(roostr_core::lite_ui::shadow_label(n)));
    match loading {
        // One identity across progress updates, so the sweep runs on.
        Some(line) => r.child(
            ShimmerText::new("graph-space-loading", line)
                .font(ty::mono(11.))
                .color(hex(0x8a90a0, 1.))
                .build(t),
        ),
        None => r.child(
            outline_button("graph-load-space", roostr_core::lite_ui::LOAD_SPACE)
                .on_press(move |s: &mut State| roostr_core::lite_ui::load_space(&space, s))
                .build(t),
        ),
    }
}

/// The side panel: what is selected, else the space's totals.
pub(crate) fn side_panel(
    s: &State,
    b: &Built,
    d: &Derived,
    channel: &str,
    page: &Page,
    t: &Theme,
) -> El {
    let g = &b.g;
    let rels = space_relations(&s.store.relations, channel);
    let rel_by_key: HashMap<&str, &RelationDef> =
        rels.iter().map(|r| (r.key.as_str(), *r)).collect();
    let names: HashMap<&str, &str> = s
        .store
        .summaries
        .iter()
        .map(|o| (o.id.as_str(), o.name.as_str()))
        .collect();
    let total_props = g.props.len() + g.idle.len();

    let col0 = col().w_full().gap(14.);
    let body =
        if let (Some(ti), Some(o)) = (d.sel_type, d.sel_obj_ref(b)) {
            let ty_ = &g.types[ti];
            object_panel(
                t,
                b,
                d,
                ty_.color,
                &type_kicker(&ty_.icon, &ty_.name),
                o,
                &rel_by_key,
                &names,
                total_props,
                col0,
            )
        } else if let Some(ti) = d.sel_type {
            let ty_ = &g.types[ti];
            let mut usage = ty_.usage.clone();
            usage.sort_by(|a, b| b.1.total_cmp(&a.1));
            let unused: Vec<String> = g
                .props
                .iter()
                .filter(|p| !ty_.uses(&p.key))
                .map(|p| {
                    rel_by_key
                        .get(p.key.as_str())
                        .map_or_else(|| p.name.clone(), |r| prop_label(r))
                })
                .chain(g.idle.iter().map(|r| label_or_key(Some(r), &r.key)))
                .collect();
            let mut c = col0.child(
                row()
                    .w_full()
                    .items_center()
                    .gap(10.)
                    .child(dot(ty_.color, 12.))
                    .child(el().grow(1.).min_w(0.).child(
                        label(titled(&ty_.icon, &ty_.name), ty::ui(18.).semibold(), INK).wrap(),
                    ))
                    .child(muted_small(format!("{} objects", ty_.objects.len()))),
            );
            // Lite: the type's placeholders - clicking the ghost cluster lands here.
            if ty_.shadow > 0 {
                c = c.child(shadow_row(s, t, ty_.shadow, channel));
            }
            c.child(kicker(
                t,
                format!("IN USE · {} OF {}", ty_.usage.len(), total_props),
            ))
            .child(usage.iter().fold(col().w_full().gap(7.), |list, (key, u)| {
                list.child(bar_row(
                    t,
                    None,
                    label_or_key(rel_by_key.get(key.as_str()).copied(), key),
                    *u,
                    ty_.color,
                ))
            }))
            .child(kicker(t, "AVAILABLE, UNUSED"))
            .child(unused.into_iter().fold(
                row().w_full().flex_wrap().gap(5.),
                |chips, name| {
                    chips.child(
                        el().border(1., Color::WHITE.alpha(0.14))
                            .border_dash(2., 1.)
                            .radius(5.)
                            .px(6.)
                            .py(2.)
                            .child(label(name, ty::mono(11.), 0x6f7686)),
                    )
                },
            ))
        } else if let Some(pi) = d.sel_prop {
            let p = &g.props[pi];
            let mut users = p.users.clone();
            let weight = |t: usize| {
                g.types[t].usage_of(&p.key).unwrap_or(0.) * g.types[t].objects.len() as f64
            };
            users.sort_by(|a, b| weight(*b).total_cmp(&weight(*a)));
            let side = if p.targets.is_empty() {
                p.format.clone()
            } else {
                format!(
                    "→ {}",
                    p.targets
                        .iter()
                        .map(|&t| titled(&g.types[t].icon, &g.types[t].name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let n = g.types.len();
            col0.child(
                row()
                    .w_full()
                    .items_end()
                    .gap(10.)
                    .child(el().grow(1.).min_w(0.).child(
                        label(titled(&p.icon, &p.name), ty::mono(18.).semibold(), INK).wrap(),
                    ))
                    .child(muted_small(side)),
            )
            .child(kicker(t, format!("USED BY {} OF {} TYPES", users.len(), n)))
            .child(users.iter().fold(col().w_full().gap(7.), |list, &ti| {
                let ty_ = &g.types[ti];
                list.child(bar_row(
                    t,
                    Some(ty_.color),
                    titled(&ty_.icon, &ty_.name),
                    ty_.usage_of(&p.key).unwrap_or(0.),
                    ty_.color,
                ))
            }))
            .child(note(if users.len() == n {
                "Used by every type.".to_string()
            } else {
                format!(
                    "The other {} types can hold {} too; none of their objects do yet.",
                    n - users.len(),
                    p.name
                )
            }))
        } else {
            let total = |num: String, what: &'static str| {
                col()
                    .grow(1.)
                    .child(label(num, ty::ui(22.).medium(), INK))
                    .child(kicker(t, what))
            };
            let mut c = col0
                .child(
                    row()
                        .w_full()
                        .gap(8.)
                        .child(total(g.types.len().to_string(), "TYPES"))
                        .child(total(g.object_count.to_string(), "OBJECTS"))
                        .child(total(g.props.len().to_string(), "PROPERTIES")),
                )
                .child(note(
                    "Hover a type, property or object. Click to pin. Scroll or pinch to zoom. \
                 Double-click an object to open it.",
                ));
            if g.shadow_count > 0 {
                c = c.child(shadow_row(s, t, g.shadow_count, channel));
            }
            c
        };

    let panel = col()
        .key("graph-panel")
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .fill(hex(0x14171f, 0.86))
        .border(1., Color::WHITE.alpha(0.07))
        .radius(14.)
        .pt(18.)
        .px(18.)
        .pb(16.)
        .child(body);
    let stage_h = page.stage[3];
    if page.narrow() {
        panel
            .pin(Some(12.), Some(12.), Some(12.), None)
            .max_h(stage_h * 0.38)
    } else {
        panel
            .pin(None, Some(24.), Some(24.), None)
            .w(300.)
            .max_h((stage_h - 140.).max(0.))
    }
}

#[allow(clippy::too_many_arguments)]
fn object_panel(
    t: &Theme,
    b: &Built,
    d: &Derived,
    color: Color,
    kicker_text: &str,
    o: &GraphObject,
    rel_by_key: &HashMap<&str, &RelationDef>,
    names: &HashMap<&str, &str>,
    total_props: usize,
    col0: El,
) -> El {
    let fields = o.held.iter().fold(col().w_full(), |list, k| {
        let rel = rel_by_key.get(k.as_str()).copied();
        let value = o
            .fields
            .get(k)
            .map(|v| format_value(rel, v, names))
            .unwrap_or_default();
        list.child(field(label_or_key(rel, k), value))
    });
    let out: Vec<String> = d
        .out
        .iter()
        .map(|&(t, j)| b.object(t, j).name.clone())
        .collect();
    let inc: Vec<String> = d
        .inc
        .iter()
        .map(|&(t, j)| b.object(t, j).name.clone())
        .collect();
    let id = o.id.clone();
    let mut c = col0
        .child(
            col()
                .w_full()
                .gap(4.)
                .child(
                    row().items_center().gap(10.).child(dot(color, 8.)).child(
                        spaced_text(kicker_text, 0.1)
                            .family(rune::Family::Mono)
                            .size(11.)
                            .color(hex(0x8a90a0, 1.))
                            .build(t),
                    ),
                )
                .child(label(o.name.clone(), ty::ui(18.).semibold(), INK).wrap()),
        )
        .child(fields)
        .child(note(format!(
            "{} other properties available, empty",
            total_props.saturating_sub(o.held.len())
        )));
    if !out.is_empty() || !inc.is_empty() {
        let mut links = col().w_full();
        if !out.is_empty() {
            links = links.child(field("Links to".into(), out.join(", ")));
        }
        if !inc.is_empty() {
            links = links.child(field("Linked from".into(), inc.join(", ")));
        }
        c = c.child(links);
    }
    c.child(
        row().child(
            Link::new("graph-open-object")
                .label("Open object")
                .text_size(13.)
                .color(hex(0xc9cfe0, 1.))
                .on_press(move |s: &mut State| nav::navigate(Route::Object(id.clone()), s))
                .build(t),
        ),
    )
}

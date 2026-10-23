//! The graph page's overlays: the finder's result list and the side panel
//! (totals, a type's property usage, a property's users, an object's values).

use std::collections::HashMap;

use chrono::TimeZone as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::link::Link;
use gpui_kit::component::progress::Progress;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::nav::{self, Route};
use roostr_core::relations::space_relations;
use roostr_core::theme::fonts::MONO;
use roostr_core::ui::spaced_text;
use roostr_core::{RelationDef, Store, Value};

use crate::model::GraphObject;
use crate::paint::hex;
use crate::scene::{Built, Derived};
use crate::view::GraphView;

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

fn mono() -> Div {
    div().font_family(MONO)
}

/// `.kicker`: mono 10px, letter-spacing .12em, #7d8496.
fn kicker(text: impl Into<String>) -> Div {
    mono()
        .text_size(px(10.))
        .text_color(hex(0x7d8496, 1.))
        .child(spaced_text(text, 0.12))
}

fn dot(color: Hsla, size: f32) -> Div {
    div().size(px(size)).flex_none().rounded_full().bg(color)
}

/// `.note`: 12px / 1.5, #7d8496.
fn note(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(hex(0x7d8496, 1.))
        .child(text.into())
}

fn muted_small(text: impl Into<SharedString>) -> Div {
    mono()
        .text_size(px(11.))
        .text_color(hex(0x8a90a0, 1.))
        .child(text.into())
}

fn ellipsis(text: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(text.into())
}

/// One usage bar row: [dot] name, kit progress bar, percentage.
fn bar_row(dot_color: Option<Hsla>, name: String, u: f64, color: Hsla) -> Div {
    let name_w = if dot_color.is_some() { 104. } else { 96. };
    let id = ElementId::Name(format!("usage-{name}").into());
    div()
        .flex()
        .items_center()
        .gap(px(if dot_color.is_some() { 9. } else { 10. }))
        .text_size(px(13.))
        .children(dot_color.map(|c| dot(c, 8.)))
        .child(
            ellipsis(name)
                .w(px(name_w))
                .flex_none()
                .text_color(hex(0xd6dae3, 1.)),
        )
        .child(
            Progress::new(id)
                .xsmall()
                .flex_1()
                .color(color)
                .value(u as f32 * 100.),
        )
        .child(muted_small(pct(u)).w(px(36.)).flex_none().text_right())
}

/// `.field`: a key/value row with a hairline above.
fn field(key: String, value: String) -> Div {
    div()
        .flex()
        .gap(px(10.))
        .py(px(6.))
        .border_t_1()
        .border_color(hsla(0., 0., 1., 0.05))
        .text_size(px(13.))
        .child(
            mono()
                .w(px(90.))
                .flex_none()
                .pt(px(1.))
                .text_size(px(11.))
                .text_color(hex(0x7d8496, 1.))
                .overflow_hidden()
                .text_ellipsis()
                .child(key),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(hex(0xdde1e9, 1.))
                .child(value),
        )
}

/// The finder's matches (or "No objects or properties match.").
pub(crate) fn found_list(view: &GraphView, cx: &mut Context<GraphView>) -> AnyElement {
    let found = view.found_items();
    let mut list = div()
        .id("graph-found")
        .flex()
        .flex_col()
        .max_h(px(360.))
        .overflow_y_scroll()
        .bg(hex(0x14161c, 0.95))
        .border_1()
        .border_color(hex(0x2a2f3a, 1.))
        .rounded(px(8.))
        .p(px(4.));
    if found.is_empty() {
        list = list.child(
            div()
                .px(px(8.))
                .py(px(6.))
                .text_size(px(12.))
                .text_color(hex(0x7d8496, 1.))
                .child("No objects or properties match."),
        );
    }
    for (i, f) in found.iter().enumerate() {
        let on = i == view.hit_index();
        list = list.child(
            div()
                .id(("graph-found-row", i))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(6.))
                .px(px(8.))
                .py(px(6.))
                .text_size(px(13.))
                .text_color(hex(0xd6dae3, 1.))
                .cursor_pointer()
                .when(on, |d| d.bg(hex(0x262b36, 1.)))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.set_hit(i, cx);
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| this.fly_to(i, window, cx)))
                .child(dot(f.color, 8.))
                .child(ellipsis(f.name.clone()).flex_1())
                .child(
                    mono()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(hex(0x7d8496, 1.))
                        .child(f.sub.clone()),
                ),
        );
    }
    list.into_any_element()
}

/// The side panel: what is selected, else the space's totals.
pub(crate) fn side_panel(
    b: &Built,
    d: &Derived,
    channel: &str,
    narrow: bool,
    stage_h: Pixels,
    cx: &mut Context<GraphView>,
) -> AnyElement {
    let g = &b.g;
    let store = Store::global(cx).read(cx);
    let rels = space_relations(&store.relations, channel);
    let rel_by_key: HashMap<&str, &RelationDef> =
        rels.iter().map(|r| (r.key.as_str(), *r)).collect();
    let names: HashMap<&str, &str> = store
        .summaries
        .iter()
        .map(|s| (s.id.as_str(), s.name.as_str()))
        .collect();
    let total_props = g.props.len() + g.idle.len();

    let col = div().flex().flex_col().gap(px(14.));
    let body = if let (Some(t), Some(o)) = (d.sel_type, d.sel_obj_ref(b)) {
        let ty = &g.types[t];
        object_panel(
            b,
            d,
            ty.color,
            &type_kicker(&ty.icon, &ty.name),
            o,
            &rel_by_key,
            &names,
            total_props,
            col,
        )
    } else if let Some(t) = d.sel_type {
        let ty = &g.types[t];
        let mut usage = ty.usage.clone();
        usage.sort_by(|a, b| b.1.total_cmp(&a.1));
        let unused: Vec<String> = g
            .props
            .iter()
            .filter(|p| !ty.uses(&p.key))
            .map(|p| {
                rel_by_key
                    .get(p.key.as_str())
                    .map_or_else(|| p.name.clone(), |r| prop_label(r))
            })
            .chain(g.idle.iter().map(|r| label_or_key(Some(r), &r.key)))
            .collect();
        col.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(dot(ty.color, 12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(18.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(titled(&ty.icon, &ty.name)),
                )
                .child(muted_small(format!("{} objects", ty.objects.len()))),
        )
        // Lite: the type's placeholders - clicking the ghost cluster lands here.
        .when(ty.shadow > 0, |d| {
            d.child(shadow_row(ty.shadow, channel, cx))
        })
        .child(kicker(format!(
            "IN USE · {} OF {}",
            ty.usage.len(),
            total_props
        )))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(7.))
                .children(usage.iter().map(|(key, u)| {
                    bar_row(
                        None,
                        label_or_key(rel_by_key.get(key.as_str()).copied(), key),
                        *u,
                        ty.color,
                    )
                })),
        )
        .child(kicker("AVAILABLE, UNUSED"))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.))
                .children(unused.into_iter().map(|name| {
                    mono()
                        .text_size(px(11.))
                        .text_color(hex(0x6f7686, 1.))
                        .border_1()
                        .border_dashed()
                        .border_color(hsla(0., 0., 1., 0.14))
                        .rounded(px(5.))
                        .px(px(6.))
                        .py(px(2.))
                        .child(name)
                })),
        )
    } else if let Some(pi) = d.sel_prop {
        let p = &g.props[pi];
        let mut users = p.users.clone();
        let weight =
            |t: usize| g.types[t].usage_of(&p.key).unwrap_or(0.) * g.types[t].objects.len() as f64;
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
        col.child(
            div()
                .flex()
                .items_baseline()
                .gap(px(10.))
                .child(
                    mono()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(18.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(titled(&p.icon, &p.name)),
                )
                .child(muted_small(side)),
        )
        .child(kicker(format!("USED BY {} OF {} TYPES", users.len(), n)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(7.))
                .children(users.iter().map(|&t| {
                    let ty = &g.types[t];
                    bar_row(
                        Some(ty.color),
                        titled(&ty.icon, &ty.name),
                        ty.usage_of(&p.key).unwrap_or(0.),
                        ty.color,
                    )
                })),
        )
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
        let total = |num: String, label: &'static str| {
            div()
                .flex_1()
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(num),
                )
                .child(kicker(label))
        };
        col.child(
            div()
                .flex()
                .gap(px(8.))
                .child(total(g.types.len().to_string(), "TYPES"))
                .child(total(g.object_count.to_string(), "OBJECTS"))
                .child(total(g.props.len().to_string(), "PROPERTIES")),
        )
        .child(note("Hover a type, property or object. Click to pin. Scroll or pinch to zoom. Double-click an object to open it."))
        .when(g.shadow_count > 0, |d| d.child(shadow_row(g.shadow_count, channel, cx)))
    };

    let panel = div()
        .id("graph-panel")
        .occlude()
        .absolute()
        .overflow_y_scroll()
        .bg(hex(0x14171f, 0.86))
        .border_1()
        .border_color(hsla(0., 0., 1., 0.07))
        .rounded(px(14.))
        .pt(px(18.))
        .px(px(18.))
        .pb(px(16.))
        .child(body);
    if narrow {
        panel
            .left(px(12.))
            .right(px(12.))
            .top(px(12.))
            .max_h(relative(0.38))
            .into_any_element()
    } else {
        panel
            .top(px(24.))
            .right(px(24.))
            .w(px(300.))
            .max_h(stage_h - px(140.))
            .into_any_element()
    }
}

/// Lite: "N not loaded" and, unless the space is downloading already (then
/// its progress, shimmering while it runs), "Load this space fully".
fn shadow_row(n: u64, channel: &str, cx: &App) -> Div {
    let loading = roostr_core::lite::status(cx)
        .and_then(|s| s.loading(channel).map(roostr_core::lite_ui::loading_text));
    let space = channel.to_string();
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(10.))
        .child(muted_small(roostr_core::lite_ui::shadow_label(n)))
        .map(|d| match loading {
            Some(text) => d.child(
                // One identity across progress updates, so the sweep runs on.
                gpui_kit::component::shimmer::ShimmerText::new(text)
                    .id("graph-space-loading")
                    .font_family(MONO)
                    .text_size(px(11.))
                    .text_color(hex(0x8a90a0, 1.)),
            ),
            None => d.child(
                roostr_core::ui::outline_button(
                    "graph-load-space",
                    roostr_core::lite_ui::LOAD_SPACE,
                )
                .on_click(move |_, _, cx| roostr_core::lite_ui::load_space(&space, cx)),
            ),
        })
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

#[allow(clippy::too_many_arguments)]
fn object_panel(
    b: &Built,
    d: &Derived,
    color: Hsla,
    kicker_text: &str,
    o: &GraphObject,
    rel_by_key: &HashMap<&str, &RelationDef>,
    names: &HashMap<&str, &str>,
    total_props: usize,
    col: Div,
) -> Div {
    let fields = o.held.iter().map(|k| {
        let rel = rel_by_key.get(k.as_str()).copied();
        let value = o
            .fields
            .get(k)
            .map(|v| format_value(rel, v, names))
            .unwrap_or_default();
        field(label_or_key(rel, k), value)
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
    col.child(
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(dot(color, 8.))
                    .child(
                        mono()
                            .text_size(px(11.))
                            .text_color(hex(0x8a90a0, 1.))
                            .child(spaced_text(kicker_text, 0.1)),
                    ),
            )
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(o.name.clone()),
            ),
    )
    .child(div().children(fields))
    .child(note(format!(
        "{} other properties available, empty",
        total_props.saturating_sub(o.held.len())
    )))
    .when(!out.is_empty() || !inc.is_empty(), |c| {
        c.child(
            div()
                .when(!out.is_empty(), |d| {
                    d.child(field("Links to".into(), out.join(", ")))
                })
                .when(!inc.is_empty(), |d| {
                    d.child(field("Linked from".into(), inc.join(", ")))
                }),
        )
    })
    .child(
        Link::new("graph-open-object")
            .text_size(px(13.))
            .text_color(hex(0xc9cfe0, 1.))
            .child("Open object")
            .on_click(move |_, _, cx| nav::navigate(Route::Object(id.clone()), cx)),
    )
}

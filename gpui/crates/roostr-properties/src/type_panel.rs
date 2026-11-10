//! A type object's page body (`TypePanel.svelte`): its definition (agents
//! read it), layout (Page | Task) and templates, with Anytype's template
//! gallery overlay (mini previews, default badge, duplicate, delete, new).

use std::collections::{HashMap, HashSet};

use roostr_core::create::apply_template;
use roostr_core::types::style;
use roostr_core::{Fields, Object, Route, State, Value, api, nav};
use roostr_ui::theme::ty::Weights as _;
use roostr_ui::{
    Button, ButtonCustomVariant, Dialog, Glyph, Tag, Theme, glyph, radius, shadow, ty,
};
use rune::{
    Color, Cursor, Effect, El, ScrollSpec, Spring, SpringPair, Transition, col, el, row, text,
};
use serde_json::json;

use crate::widgets::{later, note, rule, tip};

/// The gallery card's chrome around its scrolling body: 18 + 22 padding and
/// the 2×1 border.
const GALLERY_CHROME: f32 = 42.0;

const LAYOUTS: [(&str, &str, &str); 2] = [
    ("page", "Page", "Title, icon, and blocks"),
    ("task", "Task", "Checkbox bound to Done"),
];

#[derive(Clone)]
struct Template {
    id: String,
    name: String,
}

#[derive(Clone)]
enum Line {
    Divider,
    Head(String),
    Check(String),
    List(String),
    Custom(String),
    Para(String),
}

/// One type panel's state (GPUI's `TypePanel` entity).
#[derive(Default)]
struct TypePanelState {
    templates: Vec<Template>,
    previews: HashMap<String, Vec<Line>>,
    /// The template gallery dialog is open.
    gallery: bool,
    /// "Delete this template?" is asked for this template id.
    confirm_delete: Option<String>,
}

/// Every open type panel, by type object id.
#[derive(Default)]
struct TypePanels(HashMap<String, TypePanelState>);

fn panel<'a>(s: &'a mut State, id: &str) -> Option<&'a mut TypePanelState> {
    s.slot::<TypePanels>().0.get_mut(id)
}

/// Opens the panel's state for the type object `object_id`: loads the
/// object and its templates. Idempotent.
pub fn open_type_panel(s: &mut State, object_id: &str) {
    if s.slot::<TypePanels>().0.contains_key(object_id) {
        return;
    }
    s.slot::<TypePanels>()
        .0
        .insert(object_id.to_string(), TypePanelState::default());
    s.store.ensure_object(object_id);
    load_templates(s, object_id, false);
}

/// Drops the panel's state for `object_id`.
pub(crate) fn forget(s: &mut State, object_id: &str) {
    s.slot::<TypePanels>().0.remove(object_id);
}

fn layout_of(object: &Object) -> &str {
    match object.field_str("layout") {
        "" => "page",
        l => l,
    }
}

fn set_layout(s: &mut State, object_id: &str, id: &str) {
    let Some(object) = s.store.object(object_id) else {
        return;
    };
    if id == layout_of(&object) {
        return;
    }
    let task = api::note::set_field(object_id, "layout", Value::string(id), s);
    s.rt.spawn(async move |cx| {
        if task.await.is_ok() {
            cx.update(|s| s.store.refresh_all());
        }
    })
    .detach();
}

fn load_templates(s: &mut State, object_id: &str, previews: bool) {
    let task = api::query(
        json!({ "type": "template", "filters": [{ "key": "target_type", "condition": "equal", "value": object_id }], "limit": 50 }),
        s,
    );
    let id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(res) = task.await else { return };
        let list: Vec<Template> = res
            .records
            .into_iter()
            .map(|r| Template {
                name: roostr_core::field_str(&r.fields, "name").to_string(),
                id: r.id,
            })
            .collect();
        cx.update(|s| {
            let Some(p) = panel(s, &id) else { return };
            p.templates = list;
            if previews {
                let missing: Vec<String> = p
                    .templates
                    .iter()
                    .filter(|t| !p.previews.contains_key(&t.id))
                    .map(|t| t.id.clone())
                    .collect();
                for tid in missing {
                    load_preview(s, &id, tid);
                }
            }
        });
    })
    .detach();
}

/// Marks `id` and every block under it.
fn mark(id: &str, tpl: &Object, skip: &mut HashSet<String>) {
    if !skip.insert(id.to_string()) {
        return;
    }
    if let Some(b) = tpl.block(id) {
        for c in &b.children_ids {
            mark(c, tpl, skip);
        }
    }
}

/// First content blocks of a template, flattened for the mini preview
/// (discussion blocks and what hangs under them left out).
fn preview_lines(tpl: &Object) -> Vec<Line> {
    let mut skip = HashSet::new();
    for b in &tpl.blocks {
        if b.content
            .custom
            .as_ref()
            .is_some_and(|c| c.content_type == "discussion")
        {
            mark(&b.id, tpl, &mut skip);
        }
    }
    let mut lines = Vec::new();
    for b in &tpl.blocks {
        if skip.contains(&b.id) || lines.len() >= 7 {
            continue;
        }
        if let Some(c) = &b.content.custom {
            lines.push(if c.content_type == "divider" {
                Line::Divider
            } else {
                Line::Custom(if c.content_type.is_empty() {
                    "block".into()
                } else {
                    c.content_type.clone()
                })
            });
        } else if let Some(t) = &b.content.text {
            let text = t.text.clone();
            lines.push(match t.style {
                style::HEADER1 | style::HEADER2 | style::HEADER3 => Line::Head(text),
                style::CHECKBOX => Line::Check(text),
                style::BULLET | style::NUMBERED => Line::List(text),
                _ => Line::Para(text),
            });
        }
    }
    lines
}

fn load_preview(s: &mut State, object_id: &str, template_id: String) {
    let task = s.store.fetch_object(&template_id);
    let id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let lines = match task.await {
            Ok(tpl) => preview_lines(&tpl),
            Err(_) => vec![],
        };
        cx.update(|s| {
            if let Some(p) = panel(s, &id) {
                p.previews.insert(template_id, lines);
            }
        });
    })
    .detach();
}

fn template_fields(object: &Object) -> Fields {
    let mut f = Fields::from([("target_type".to_string(), Value::string(object.id.clone()))]);
    let space = object.field_str("channel");
    if !space.is_empty() {
        f.insert("channel".into(), Value::string(space));
    }
    f
}

fn open_gallery(s: &mut State, object_id: &str) {
    open_type_panel(s, object_id);
    load_templates(s, object_id, true);
    if let Some(p) = panel(s, object_id) {
        p.gallery = true;
    }
}

fn close_gallery(s: &mut State, object_id: &str) {
    if let Some(p) = panel(s, object_id) {
        p.gallery = false;
    }
}

fn add_template(s: &mut State, object_id: &str) {
    let Some(object) = s.store.object(object_id) else {
        return;
    };
    close_gallery(s, object_id);
    let task = api::note::create(
        "New template",
        Some("template"),
        Some(template_fields(&object)),
        s,
    );
    s.rt.spawn(async move |cx| {
        if let Ok(id) = task.await {
            cx.update(|s| nav::navigate(Route::Object(id), s));
        }
    })
    .detach();
}

fn duplicate(s: &mut State, object_id: &str, t: Template) {
    let Some(object) = s.store.object(object_id) else {
        return;
    };
    let name = format!(
        "{} copy",
        if t.name.is_empty() {
            "Untitled"
        } else {
            &t.name
        }
    );
    let task = api::note::create(&name, Some("template"), Some(template_fields(&object)), s);
    let panel_id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(id) = task.await else { return };
        let apply = cx.update(|s| apply_template(&id, &t.id, s));
        let _ = apply.await;
        cx.update(|s| {
            if panel(s, &panel_id).is_none() {
                return;
            }
            load_templates(s, &panel_id, false);
            load_preview(s, &panel_id, id);
        });
    })
    .detach();
}

fn set_default(s: &mut State, object_id: &str, id: String) {
    let Some(object) = s.store.object(object_id) else {
        return;
    };
    let task = if object.field_str("default_template_id") == id {
        api::note::delete_field(object_id, "default_template_id", s)
    } else {
        api::note::set_field(object_id, "default_template_id", Value::string(id), s)
    };
    s.rt.spawn(async move |cx| {
        if task.await.is_ok() {
            cx.update(|s| s.store.refresh_all());
        }
    })
    .detach();
}

/// The "Delete this template?" question was answered Delete.
fn remove(s: &mut State, object_id: &str) {
    let Some(id) = panel(s, object_id).and_then(|p| p.confirm_delete.take()) else {
        return;
    };
    let is_default = s
        .store
        .object(object_id)
        .is_some_and(|o| o.field_str("default_template_id") == id);
    let panel_id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        if is_default {
            let t = cx.update(|s| api::note::delete_field(&panel_id, "default_template_id", s));
            let _ = t.await;
        }
        let t = cx.update(|s| api::note::delete(&id, s));
        let _ = t.await;
        cx.update(|s| {
            if panel(s, &panel_id).is_none() {
                return;
            }
            load_templates(s, &panel_id, false);
            s.store.refresh_all();
        });
    })
    .detach();
}

/// The window's size, from the overlay root every Roostr window mounts.
fn window_size(cx: &mut rune::Cx<State>) -> Option<[f32; 2]> {
    cx.layout_size("kit-root").filter(|[w, _]| *w > 0.0)
}

/// Type object page body: layout cards and the Templates chip; the gallery
/// dialog (and its delete question) go into `layer` while open.
pub fn type_panel(
    s: &State,
    cx: &mut rune::Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
) -> El {
    let state = s.get::<TypePanels>().and_then(|p| p.0.get(object_id));
    if state.is_none() {
        let id = object_id.to_string();
        later(s, move |s| open_type_panel(s, &id));
    }
    let Some(object) = s.store.object(object_id) else {
        return el().key("type-panel");
    };
    let layout = layout_of(&object).to_string();
    let mut layouts = row().gap(8.0);
    for (id, name, hint) in LAYOUTS {
        let selected = layout == id;
        let oid = object_id.to_string();
        layouts = layouts.child(
            Button::new(id)
                .outline()
                .selected(selected)
                .h_auto()
                .min_w(160.0)
                .px(14.0)
                .py(10.0)
                .radius(radius::MENU)
                .bg(Color::TRANSPARENT)
                .border_color(if selected { t.accent } else { t.border })
                .text_color(t.fg)
                .pointer()
                .on_press(move |s: &mut State| set_layout(s, &oid, id))
                // Title + hint sit left like the web card, not kit-centred.
                .child(
                    col()
                        .min_w(160.0 - 28.0)
                        .items_start()
                        .gap(2.0)
                        .child(
                            text(format!("{} {name}", if id == "task" { "✅" } else { "📄" }))
                                .font(ty::ui(14.0))
                                .color(t.fg),
                        )
                        .child(note(t, hint, 11.0)),
                )
                .build(t),
        );
    }
    let n = state.map_or(0, |p| p.templates.len());
    let size = window_size(cx);
    let narrow = size.is_some_and(|[w, _]| roostr_ui::layout::is_mobile(w));
    let chip = {
        let oid = object_id.to_string();
        Button::new("tpl-chip")
            .outline()
            .radius(999.0)
            .h_auto()
            .px(12.0)
            .py(4.0)
            .bg(Color::TRANSPARENT)
            .border_color(t.border)
            .pointer()
            .on_press(move |s: &mut State| open_gallery(s, &oid))
            .child(
                text(if n > 0 {
                    format!("⊞ Templates · {n}")
                } else {
                    "⊞ Templates".into()
                })
                .font(ty::ui(12.0))
                .color(t.muted)
                .group_color(t.fg),
            )
            .build(t)
            .hover_group()
    };
    if let Some(p) = state {
        if p.gallery {
            gallery(t, layer, object_id, &object, p, size);
        }
        if p.confirm_delete.is_some() {
            delete_question(t, layer, object_id);
        }
    }
    let mut root = col()
        .key("type-panel")
        .gap(24.0)
        .mt(16.0)
        .child(
            col()
                .items_start()
                .child(note(t, "LAYOUT", 11.0).mb(8.0))
                .child(layouts),
        )
        .child(row().mt(-8.0).child(chip));
    // `.type-panel`: `16px 16px 0` at ≤720px; flush with the page on desktop.
    if narrow {
        root = root.mx(16.0);
    }
    root
}

/// One line of a template's mini preview: 9px, line height 1.35, one line
/// with an ellipsis.
fn preview_line(line: impl Into<String>, size: f32) -> El {
    text(line.into())
        .font(ty::ui(size).lh(size * 1.35))
        .truncate()
        .w_full()
}

fn preview(t: &Theme, lines: &[Line]) -> El {
    let mut p = col()
        .w_full()
        .items_start()
        .gap(3.0)
        .h(108.0)
        .py(10.0)
        .px(12.0)
        .clip_soft(0.0);
    for l in lines {
        p = p.child(match l {
            Line::Divider => rule(t).my(2.0),
            Line::Head(s) => preview_line(if s.is_empty() { "Heading" } else { s }, 11.0)
                .font(ty::ui(11.0).lh(11.0 * 1.35).bold())
                .color(t.fg),
            Line::Check(s) => preview_line(format!("☐ {s}"), 9.0).color(t.fg),
            Line::List(s) => preview_line(format!("• {s}"), 9.0).color(t.fg),
            Line::Custom(s) => preview_line(format!("▨ {s}"), 9.0).color(t.muted),
            Line::Para(s) => preview_line(s.clone(), 9.0).color(t.fg),
        });
    }
    if lines.is_empty() {
        p = p.child(preview_line("Empty template", 9.0).color(t.muted));
    }
    p
}

/// A card action: transparent, `hover` fill; the glyph takes its hover
/// colour from the button.
fn card_action(
    t: &Theme,
    id: &str,
    label: &str,
    title: &str,
    fg: Color,
    hover_fg: Color,
    press: impl Fn(&mut State) + 'static,
) -> El {
    tip(
        Button::new(id)
            .custom(ButtonCustomVariant::new(t).hover(t.hover).active(t.hover))
            .radius(5.0)
            .h_auto()
            .px(5.0)
            .py(2.0)
            .pointer()
            .on_press(press)
            .child(
                text(label.to_string())
                    .font(ty::ui(12.0))
                    .color(fg)
                    .group_color(hover_fg),
            )
            .build(t)
            .hover_group(),
        title,
    )
}

fn template_card(
    t: &Theme,
    object_id: &str,
    tpl: &Template,
    lines: &[Line],
    is_default: bool,
) -> El {
    let g = format!("tg-{}", tpl.id);
    let open_id = tpl.id.clone();
    let preview = tip(
        roostr_ui::motion::pressable(col())
            .key("preview")
            .role("button")
            .w_full()
            .cursor(Cursor::Pointer)
            .on_press({
                let oid = object_id.to_string();
                move |s: &mut State| {
                    close_gallery(s, &oid);
                    nav::navigate(Route::Object(open_id.clone()), s);
                }
            })
            .child(preview(t, lines)),
        "Edit template",
    );
    let mut footer = row()
        .center_y()
        .gap(6.0)
        .px(10.0)
        .py(7.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
        .child(
            el().grow(1.0).min_w(0.0).child(
                text(if tpl.name.is_empty() {
                    "Untitled".to_string()
                } else {
                    tpl.name.clone()
                })
                .font(ty::ui(12.0))
                .color(t.fg)
                .truncate(),
            ),
        );
    if is_default {
        footer = footer.child(
            Tag::custom(Color::TRANSPARENT, t.accent, t.accent)
                .rounded(5.0)
                .px(5.0)
                .py(1.0)
                .text_size(10.0)
                .label("Default")
                .build(t),
        );
    }
    let star = {
        let (oid, id) = (object_id.to_string(), tpl.id.clone());
        card_action(
            t,
            "star",
            "★",
            if is_default {
                "Unset default"
            } else {
                "Set as default"
            },
            if is_default { t.accent } else { t.muted },
            t.fg,
            move |s| set_default(s, &oid, id.clone()),
        )
    };
    let dup = {
        let (oid, tpl) = (object_id.to_string(), tpl.clone());
        card_action(t, "dup", "⧉", "Duplicate", t.muted, t.fg, move |s| {
            duplicate(s, &oid, tpl.clone())
        })
    };
    let del = {
        let (oid, id) = (object_id.to_string(), tpl.id.clone());
        card_action(
            t,
            "del",
            "🗑",
            "Delete",
            t.muted,
            roostr_ui::color::rgb(0xe05555),
            move |s| {
                if let Some(p) = panel(s, &oid) {
                    p.confirm_delete = Some(id.clone());
                }
            },
        )
    };
    col()
        .key(g)
        .hover_group()
        .w(182.0)
        .border(1.0, if is_default { t.accent } else { t.border })
        .radius(radius::MENU)
        .fill(t.bg)
        .clip_rounded(radius::MENU)
        .child(preview)
        .child(footer)
        .child(
            row()
                .pin(None, Some(6.0), Some(6.0), None)
                .gap(2.0)
                .fill(t.panel)
                .border(1.0, t.border)
                .radius(7.0)
                .padding(2.0)
                .group_opacity(0.0, 1.0)
                .child(star)
                .child(dup)
                .child(del),
        )
}

/// The template gallery dialog: mini-preview cards (open, default ★,
/// duplicate, delete) and the New template card. It rises 8px into place
/// while the dim fades in, centred in the window.
fn gallery(
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
    object: &Object,
    p: &TypePanelState,
    size: Option<[f32; 2]>,
) {
    let default_id = object.field_str("default_template_id");
    let name = match object.field_str("name") {
        "" => "object",
        n => n,
    };
    let mut grid = row().flex_wrap().gap(12.0);
    for tpl in &p.templates {
        let lines = p.previews.get(&tpl.id).map_or(&[][..], Vec::as_slice);
        grid = grid.child(template_card(
            t,
            object_id,
            tpl,
            lines,
            default_id == tpl.id,
        ));
    }
    let new = {
        let oid = object_id.to_string();
        Button::new("tg-new")
            .outline()
            .w(182.0)
            .h_auto()
            .radius(radius::MENU)
            .border_color(t.border)
            .bg(t.bg)
            .pointer()
            .on_press(move |s: &mut State| add_template(s, &oid))
            .child(
                col()
                    .items_center()
                    .gap(6.0)
                    .child(glyph(Glyph::Plus, 22.0).color(t.muted).group_color(t.fg))
                    .child(
                        text("New template")
                            .font(ty::ui(13.0))
                            .color(t.muted)
                            .group_color(t.fg),
                    ),
            )
            .build(t)
            .hover_group()
            .min_h(140.0)
            .border_dash(4.0, 2.0)
    };
    grid = grid.child(new);
    let [vw, vh] = size.unwrap_or([1400.0, 900.0]);
    let close = {
        let oid = object_id.to_string();
        move |s: &mut State| close_gallery(s, &oid)
    };
    let body = col()
        .key("tg-modal")
        .max_h(vh * 0.78 - GALLERY_CHROME)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(
            col()
                .child(
                    row()
                        .center_y()
                        .justify_between()
                        .child(
                            text("⊞ Templates")
                                .font(ty::ui(16.0).bold())
                                .color(t.fg),
                        )
                        .child(
                            Button::new("tg-x")
                                .ghost()
                                .icon(Glyph::Close)
                                .text_color(t.muted)
                                .pointer()
                                .accessibility_label("Close")
                                .on_press(close.clone())
                                .build(t),
                        ),
                )
                .child(
                    note(
                        t,
                        format!(
                            "A default template pre-fills every new {name}; the New button's ▾ picks one per object."
                        ),
                        12.0,
                    )
                    .mt(4.0)
                    .mb(14.0),
                )
                .child(grid),
        );
    let mut card = col()
        .key("tg-dialog")
        .role("dialog")
        .w(640.0)
        .max_w(vw - 48.0)
        .pt(18.0)
        .px(22.0)
        .pb(22.0)
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(radius::MODAL)
        .transition(roostr_ui::motion::appear(8.0))
        .child(body);
    for sh in shadow::kit_xl() {
        card = card.box_shadow(sh);
    }
    layer.push(
        el().key("tg-overlay")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(t.kit.overlay)
            .transition(Transition {
                insertion: Effect::opacity(0.0),
                removal: Effect::IDENTITY,
                springs: SpringPair::new(Spring::new(0.18, 0.0), Spring::NONE),
                ..Transition::identity()
            }),
    );
    let card = roostr_ui::overlay::dismissable(card, "tg", close, layer);
    layer.push(
        col()
            .key("tg-host")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .items_center()
            .justify_center()
            .child(card),
    );
}

/// "Delete this template?" with Delete / Cancel (the GPUI window prompt).
fn delete_question(t: &Theme, layer: &mut Vec<El>, object_id: &str) {
    let cancel = {
        let oid = object_id.to_string();
        move |s: &mut State| {
            if let Some(p) = panel(s, &oid) {
                p.confirm_delete = None;
            }
        }
    };
    Dialog::new("tg-delete")
        .width(380.0)
        .close_button(false)
        .open(true)
        .centered()
        .on_close(cancel.clone())
        .padding(18.0)
        .radius(12.0)
        .fill(t.panel)
        .border_color(t.border)
        .child(
            text("Delete this template?")
                .font(ty::ui(13.0).lh(18.0))
                .color(t.fg)
                .wrap(),
        )
        .footer(
            roostr_ui::outline_button("tg-delete-cancel", "Cancel")
                .on_press(cancel)
                .build(t),
        )
        .footer({
            let oid = object_id.to_string();
            roostr_ui::danger_button("tg-delete-ok", "Delete")
                .on_press(move |s: &mut State| remove(s, &oid))
                .build(t)
                .default_button()
        })
        .build(t, layer);
}

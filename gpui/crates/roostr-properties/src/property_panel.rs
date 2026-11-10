//! A property (relation) object's page body (`PropertyPanel.svelte`): its
//! definition (agents read it) and every object holding a value for it,
//! each value rendered in the property's own format.

use std::collections::HashMap;

use roostr_core::dates::show_date;
use roostr_core::objects::object_icon;
use roostr_core::relations::current_space_id;
use roostr_core::types::field_str;
use roostr_core::{QueryRow, Route, State, StoreEvent, Value, api, nav};
use roostr_ui::theme::ty::Weights as _;
use roostr_ui::{TagPill, Textarea, TextareaState, Theme, motion, radius, ty};
use rune::{Color, Cursor, El, col, el, row, text};
use serde_json::json;

use crate::value::number_text;
use crate::widgets::{empty_note, later, prefix};

/// A row's height before one is drawn (14px text, 8px padding, border).
const ROW_H_GUESS: f32 = 38.0;

const PLACEHOLDER: &str = "What is this property for? Agents read this.";

/// One property panel's state (GPUI's `PropertyPanel` entity).
struct PropertyPanelState {
    definition: TextareaState,
    /// Typed since the last save: saved once the area loses focus.
    dirty: bool,
    /// The description the area last showed from the store (or saved).
    synced: Option<String>,
    /// A save on its way: the store still has the old description.
    saving: Option<String>,
    rows: Vec<QueryRow>,
    loaded: bool,
    loaded_key: Option<String>,
}

impl PropertyPanelState {
    fn new() -> Self {
        Self {
            definition: TextareaState::new().auto_grow(2, 20),
            dirty: false,
            synced: None,
            saving: None,
            rows: vec![],
            loaded: false,
            loaded_key: None,
        }
    }
}

/// Every open property panel, by relation object id.
#[derive(Default)]
struct PropertyPanels(HashMap<String, PropertyPanelState>);

fn panel<'a>(s: &'a mut State, id: &str) -> &'a mut PropertyPanelState {
    s.slot::<PropertyPanels>()
        .0
        .entry(id.to_string())
        .or_insert_with(PropertyPanelState::new)
}

/// Opens the panel's state for the relation object `object_id` and loads
/// the object; its holders load once the object is there. Idempotent.
pub fn open_property_panel(s: &mut State, object_id: &str) {
    if s.slot::<PropertyPanels>().0.contains_key(object_id) {
        return;
    }
    panel(s, object_id);
    s.store.ensure_object(object_id);
    sync(s, object_id);
}

/// Drops the panel's state for `object_id`.
pub(crate) fn forget(s: &mut State, object_id: &str) {
    s.slot::<PropertyPanels>().0.remove(object_id);
}

/// Reconnect reloads every panel's holders; blur saves the definition.
pub(crate) fn register(s: &mut State) {
    s.on_store(on_store);
    s.on_poll(poll);
}

fn on_store(s: &mut State, ev: &StoreEvent) {
    if matches!(ev, StoreEvent::Reconnected) {
        for p in s.slot::<PropertyPanels>().0.values_mut() {
            p.loaded_key = None;
        }
    }
}

/// Every open panel: a definition that lost focus with typed text is
/// saved (the web's onblur), the area follows the stored description while
/// not focused, and holders load for the property's key.
fn poll(s: &mut State) -> bool {
    let ids: Vec<String> = match s.get::<PropertyPanels>() {
        Some(p) if !p.0.is_empty() => p.0.keys().cloned().collect(),
        _ => return false,
    };
    let mut changed = false;
    for id in ids {
        changed |= sync(s, &id);
    }
    changed
}

/// One panel's poll work; true when what it shows changed.
fn sync(s: &mut State, id: &str) -> bool {
    let Some(object) = s.store.object(id) else {
        return false;
    };
    let mut changed = false;
    let p = panel(s, id);
    if p.dirty && !p.definition.is_focused() {
        p.dirty = false;
        changed |= save_definition(s, id);
    }
    let description = object.field_str("description").to_string();
    let p = panel(s, id);
    if p.synced.as_deref() != Some(description.as_str()) && !p.definition.is_focused() {
        match &p.saving {
            Some(draft) if *draft != description => {}
            _ => {
                p.saving = None;
                p.definition.set_value(description.clone());
                p.synced = Some(description);
                changed = true;
            }
        }
    }
    load(s, id) || changed
}

fn save_definition(s: &mut State, id: &str) -> bool {
    let Some(object) = s.store.object(id) else {
        return false;
    };
    let p = panel(s, id);
    let draft = p.definition.value().trim().to_string();
    if draft == object.field_str("description") {
        return false;
    }
    p.synced = Some(draft.clone());
    p.saving = Some(draft.clone());
    let task = api::note::set_field(id, "description", Value::string(draft), s);
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        if let Err(e) = task.await {
            log::error!("saving the definition of {id}: {e:#}");
            // Show the stored description again.
            cx.update(|s| {
                if let Some(p) = s.slot::<PropertyPanels>().0.get_mut(&id) {
                    p.saving = None;
                    p.synced = None;
                }
            });
        }
    })
    .detach();
    true
}

/// Loads the objects holding a value for the property, once per key.
fn load(s: &mut State, id: &str) -> bool {
    let Some(object) = s.store.object(id) else {
        return false;
    };
    let key = object.field_str("key").to_string();
    let p = panel(s, id);
    if key.is_empty() || p.loaded_key.as_deref() == Some(key.as_str()) {
        return false;
    }
    p.loaded_key = Some(key.clone());
    // A stamped property lists its own space's holders; a bundled one the open space's.
    let own = match object.field_str("channel") {
        "" => current_space_id(s),
        c => c.to_string(),
    };
    let default = s
        .store
        .channels
        .first()
        .map(|c| c.id.clone())
        .unwrap_or_default();
    let space_filter = if own == default {
        json!({ "key": "channel", "condition": "in", "value": [own, ""] })
    } else {
        json!({ "key": "channel", "condition": "equal", "value": own })
    };
    let task = api::query_all(
        json!({ "filters": [{ "key": key, "condition": "notEmpty" }, space_filter] }),
        s,
    );
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(records) = task.await else { return };
        let mut rows: Vec<QueryRow> = records
            .into_iter()
            .filter(|r| r.id != id && r.type_key != "relation" && r.type_key != "type")
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        cx.update(|s| {
            if let Some(p) = s.slot::<PropertyPanels>().0.get_mut(&id) {
                p.rows = rows;
                p.loaded = true;
            }
        });
    })
    .detach();
    false
}

/// Object ids a link value names.
fn linked_ids(v: Option<&Value>) -> Vec<String> {
    let Some(v) = v else { return vec![] };
    if let Some(l) = v.link_value.as_ref().filter(|l| !l.target_id.is_empty()) {
        return vec![l.target_id.clone()];
    }
    v.items()
        .iter()
        .filter_map(|i| i.link_value.as_ref().map(|l| l.target_id.clone()))
        .filter(|s| !s.is_empty())
        .collect()
}

fn option_texts(v: Option<&Value>) -> Vec<String> {
    let Some(v) = v else { return vec![] };
    if let Some(s) = &v.string_value {
        return if s.is_empty() {
            vec![]
        } else {
            vec![s.clone()]
        };
    }
    v.items()
        .iter()
        .filter_map(|i| i.string_value.clone())
        .filter(|s| !s.is_empty())
        .collect()
}

fn fmt(v: Option<&Value>, format: &str) -> String {
    let Some(v) = v else { return String::new() };
    if format == "repeat" {
        return String::new();
    }
    if let Some(b) = v.bool_value {
        return if b { "☑" } else { "☐" }.into();
    }
    if let Some(s) = &v.string_value {
        return s.clone();
    }
    if let Some(n) = v.int_value {
        return if format == "date" {
            show_date(n)
        } else {
            n.to_string()
        };
    }
    if let Some(f) = v.float_value {
        return number_text(f);
    }
    if v.values_value.is_some() {
        return v
            .items()
            .iter()
            .map(|i| {
                i.string_value
                    .clone()
                    .or_else(|| i.link_value.as_ref().map(|l| prefix(&l.target_id, 8)))
                    .unwrap_or_default()
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
    }
    if let Some(l) = &v.link_value {
        return prefix(&l.target_id, 8);
    }
    if let Some(l) = &v.list_value {
        return l.values.join(", ");
    }
    String::new()
}

/// A cell opening an object: the kit link in Roostr's look, the cell's own
/// text colour (fading to 0.8 on hover, as the kit link does), no underline.
fn roostr_link(t: &Theme, key: String, route: String, content: El) -> El {
    row()
        .key(key)
        .role("link")
        .hover_group()
        .cursor(Cursor::Pointer)
        .min_w(0.0)
        .on_press(move |s: &mut State| nav::navigate(Route::Object(route.clone()), s))
        .child(content.color(t.fg).group_color(t.fg.alpha(t.fg.a * 0.8)))
}

fn th(t: &Theme, label: &str) -> El {
    el().px(8.0).py(6.0).child(
        text(label.to_string())
            .font(ty::ui(12.0).medium())
            .color(t.muted),
    )
}

fn cols(t: &Theme, key: Option<String>, a: El, b: El, c: El) -> El {
    let mut r = row()
        .w_full()
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
        .border_layout()
        .child(el().w_frac(0.42).min_w(0.0).child(a))
        .child(el().w_frac(0.42).min_w(0.0).max_w(340.0).child(b))
        .child(el().grow(1.0).min_w(0.0).child(c));
    if let Some(k) = key {
        r = r.key(k);
    }
    r
}

/// Property object page body: definition + holders table. Only the rows
/// in (or a window's height around) the window are built: a common property
/// lists a thousand objects. Spacers keep the list's full height.
pub fn property_panel(
    s: &State,
    cx: &mut rune::Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
) -> El {
    let _ = layer;
    let state = s.get::<PropertyPanels>().and_then(|p| p.0.get(object_id));
    if state.is_none() {
        let id = object_id.to_string();
        later(s, move |s| open_property_panel(s, &id));
    }
    let Some(object) = s.store.object(object_id) else {
        return el().key("property-panel");
    };
    let fresh;
    let st = match state {
        Some(p) => p,
        None => {
            fresh = PropertyPanelState::new();
            &fresh
        }
    };
    let key = object.field_str("key").to_string();
    let format = match object.field_str("format") {
        "" => "shorttext",
        f => f,
    }
    .to_string();
    let options = s
        .store
        .relations
        .iter()
        .find(|r| r.id == object.id)
        .map(|r| r.options.clone())
        .unwrap_or_default();
    let chipped = format == "tag" || format == "status";
    let space = current_space_id(s);
    let n = st.rows.len();
    let focused = st.definition.is_focused();

    // The textarea itself (no wrapper): border only on hover and focus; it
    // blurs on any outside press and saves then (the web's onblur).
    let area = {
        let (lens_id, change_id) = (object_id.to_string(), object_id.to_string());
        Textarea::new("pp-definition", &st.definition)
            .on_edit(move |s: &mut State| &mut panel(s, &lens_id).definition)
            .on_change(move |s: &mut State, _| panel(s, &change_id).dirty = true)
            .placeholder(PLACEHOLDER)
            .appearance(false)
            .text_size(13.5)
            .text_color(t.fg)
            .px(0.0)
            .py(0.0)
            .build(t)
    };
    let mut definition = col()
        .key("pp-definition-box")
        .min_w(0.0)
        .ml(-8.0)
        .mb(8.0)
        .radius(radius::MD)
        .py(6.0)
        .px(8.0)
        .border(
            1.0,
            if focused {
                t.accent
            } else {
                Color::TRANSPARENT
            },
        )
        .child(area);
    if !focused {
        definition = definition.hover_group().child(
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radius(radius::MD)
                .border(1.0, t.border)
                .group_opacity(0.0, 1.0)
                .inert(),
        );
    }

    let mut panel_el = col()
        .key("property-panel")
        .px(48.0)
        .items_stretch()
        .child(definition)
        .child(
            row()
                .center_y()
                .gap(10.0)
                .my(13.0)
                .child(
                    row()
                        .px(8.0)
                        .py(2.0)
                        .radius(radius::SM)
                        .border(1.0, t.border)
                        .child(text(format.clone()).font(ty::ui(12.0)).color(t.fg)),
                )
                .child(if st.loaded {
                    // The count rolls as objects take or drop a value.
                    motion::rolling_text(
                        t,
                        format!("property-count-{object_id}"),
                        format!("{n} object{} with a value", if n == 1 { "" } else { "s" }),
                        n as f64,
                        ty::ui(13.0),
                        t.muted,
                    )
                } else {
                    text("Loading…").font(ty::ui(13.0)).color(t.muted)
                }),
        );
    if st.loaded && n == 0 {
        return panel_el
            .child(empty_note(t, "No objects have a value for this property yet.", 14.0).my(13.0));
    }
    if n == 0 {
        return panel_el;
    }

    let top_key = format!("pp-top-{object_id}");
    let view_h = cx
        .layout_with("kit-root", |r| r[3])
        .filter(|h| *h > 0.0)
        .unwrap_or(900.0);
    // Where row 0 is in the window, to 32pt.
    let top = cx
        .window_layout_with(&top_key, |r| (r[1] / 32.0).floor() as i32)
        .map_or(0.0, |q| q as f32 * 32.0);
    let window = |row_h: f32| {
        let first = (((-view_h - top) / row_h).floor().max(0.0) as usize).min(n);
        let last = ((((2.0 * view_h - top) / row_h).ceil().max(0.0)) as usize).clamp(first, n);
        (first, last)
    };
    let (first, _) = window(ROW_H_GUESS);
    let row_h = cx
        .layout_with(&format!("pp-r-{}", st.rows[first].id), |r| {
            (r[3] * 4.0).round() as i32
        })
        .map_or(ROW_H_GUESS, |q| q as f32 / 4.0)
        .max(1.0);
    let (first, last) = window(row_h);

    let mut table = col()
        .mt(8.0)
        .items_stretch()
        .child(cols(t, None, th(t, "Name"), th(t, "Value"), th(t, "Type")))
        .child(el().key(top_key).h(row_h * first as f32));
    for r in &st.rows[first..last] {
        let name = r
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .or_else(|| Some(field_str(&r.fields, "name").to_string()).filter(|n| !n.is_empty()))
            .unwrap_or_else(|| "Untitled".into());
        let icon = object_icon(
            &s.store,
            field_str(&r.fields, "iconEmoji"),
            &r.type_key,
            &space,
        );
        let name_cell = el().padding(8.0).child(roostr_link(
            t,
            format!("name-{}", r.id),
            r.id.clone(),
            text(format!("{icon} {name}"))
                .font(ty::ui(14.0).medium())
                .truncate(),
        ));
        let v = r.fields.get(&key);
        let ids = linked_ids(v);
        let mut value = row().padding(8.0).center_y().clip_soft(0.0);
        if !ids.is_empty() {
            for id in ids {
                let (name, icon, tk) = match s.store.summary(&id) {
                    Some(o) => (o.name.clone(), o.icon.clone(), o.type_key.clone()),
                    None => match s.store.agent(&id) {
                        Some(a) => (a.name.clone(), a.icon.clone(), "agent".into()),
                        None => (String::new(), String::new(), String::new()),
                    },
                };
                let label = format!(
                    "{} {}",
                    object_icon(&s.store, &icon, &tk, &space),
                    if name.is_empty() { "Untitled" } else { &name }
                );
                value = value.child(
                    roostr_link(
                        t,
                        format!("link-{}-{id}", r.id),
                        id.clone(),
                        text(label).font(ty::ui(14.0)),
                    )
                    .mr(10.0),
                );
            }
        } else if chipped {
            for o in option_texts(v) {
                let color = options
                    .iter()
                    .find(|x| x.text == o)
                    .map(|x| x.color.clone())
                    .unwrap_or_default();
                value = value.child(el().mr(4.0).child(TagPill::new(o, color).build(t)));
            }
        } else {
            value = value.child(
                text(fmt(v, &format))
                    .font(ty::ui(14.0))
                    .color(t.fg)
                    .truncate(),
            );
        }
        let type_cell = el().padding(8.0).child(
            text(r.type_key.clone())
                .font(ty::ui(14.0))
                .color(t.muted)
                .wrap(),
        );
        table = table.child(cols(
            t,
            Some(format!("pp-r-{}", r.id)),
            name_cell,
            value,
            type_cell,
        ));
    }
    table = table.child(el().h(row_h * (n - last) as f32));
    panel_el = panel_el.child(table);
    panel_el
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_values_like_the_web_table() {
        assert_eq!(fmt(None, "shorttext"), "");
        assert_eq!(fmt(Some(&Value::string("hi")), "shorttext"), "hi");
        assert_eq!(fmt(Some(&Value::string("x")), "repeat"), "");
        let n = Value {
            float_value: Some(2.5),
            ..Value::default()
        };
        assert_eq!(fmt(Some(&n), "number"), number_text(2.5));
        let b = Value {
            bool_value: Some(true),
            ..Value::default()
        };
        assert_eq!(fmt(Some(&b), "checkbox"), "☑");
        assert!(option_texts(Some(&Value::string(""))).is_empty());
        assert_eq!(option_texts(Some(&Value::string("Done"))), vec!["Done"]);
        assert!(linked_ids(None).is_empty());
    }
}

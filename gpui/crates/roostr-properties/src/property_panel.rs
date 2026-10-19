//! A property (relation) object's page body (`PropertyPanel.svelte`): its
//! definition (agents read it) and every object holding a value for it,
//! each value rendered in the property's own format.

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::dates::show_date;
use roostr_core::objects::object_icon;
use roostr_core::relations::current_space_id;
use roostr_core::types::field_str;
use roostr_core::ui::TagPill;
use roostr_core::{Object, QueryRow, Route, Store, StoreEvent, Value, api, nav, theme};
use serde_json::json;

use crate::value::number_text;
use crate::widgets::{prefix, text_area};

/// Property object page body: definition + holders table.
pub struct PropertyPanel {
    object_id: String,
    definition: Entity<TextareaState>,
    synced: Option<String>,
    rows: Vec<QueryRow>,
    loaded: bool,
    loaded_key: Option<String>,
    _subs: Vec<Subscription>,
}

impl PropertyPanel {
    /// The panel for the relation object `object_id`.
    pub fn new(object_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        store.update(cx, |s, cx| s.ensure_object(&object_id, cx));
        let definition = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 20)
                .placeholder("What is this property for? Agents read this.")
        });
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&store, |this: &mut Self, _, ev: &StoreEvent, cx| {
                if matches!(ev, StoreEvent::Reconnected) {
                    this.loaded_key = None;
                    cx.notify();
                }
            }),
            cx.subscribe(&definition, |this: &mut Self, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Blur) {
                    this.save_definition(cx);
                }
            }),
        ];
        Self {
            object_id,
            definition,
            synced: None,
            rows: vec![],
            loaded: false,
            loaded_key: None,
            _subs: subs,
        }
    }

    fn save_definition(&mut self, cx: &mut Context<Self>) {
        let Some(object) = Store::global(cx).read(cx).object(&self.object_id) else {
            return;
        };
        let draft = self.definition.read(cx).value().trim().to_string();
        if draft == object.field_str("description") {
            return;
        }
        self.synced = Some(draft.clone());
        api::note::set_field(&self.object_id, "description", Value::string(draft), cx)
            .detach_and_log_err(cx);
    }

    fn load(&mut self, object: &Object, cx: &mut Context<Self>) {
        let key = object.field_str("key").to_string();
        if key.is_empty() || self.loaded_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.loaded_key = Some(key.clone());
        // A stamped property lists its own space's holders; a bundled one the open space's.
        let own = match object.field_str("channel") {
            "" => current_space_id(cx),
            s => s.to_string(),
        };
        let default = Store::global(cx)
            .read(cx)
            .channels
            .first()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        let space_filter = if own == default {
            json!({ "key": "channel", "condition": "in", "value": [own, ""] })
        } else {
            json!({ "key": "channel", "condition": "equal", "value": own })
        };
        let task = api::query_all(
            json!({ "filters": [{ "key": key, "condition": "notEmpty" }, space_filter] }),
            cx,
        );
        let id = object.id.clone();
        cx.spawn(async move |this, cx| {
            let Ok(records) = task.await else { return };
            let mut rows: Vec<QueryRow> = records
                .into_iter()
                .filter(|r| r.id != id && r.type_key != "relation" && r.type_key != "type")
                .collect();
            rows.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
            let _ = this.update(cx, |this, cx| {
                this.rows = rows;
                this.loaded = true;
                cx.notify();
            });
        })
        .detach();
    }
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

impl Render for PropertyPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = Store::global(cx).read(cx).object(&self.object_id) else {
            return div();
        };
        self.load(&object, cx);
        let description = object.field_str("description").to_string();
        if self.synced.as_deref() != Some(description.as_str())
            && !self.definition.focus_handle(cx).is_focused(window)
        {
            self.definition
                .update(cx, |s, cx| s.set_value(description.clone(), window, cx));
            self.synced = Some(description);
        }
        let store = Store::global(cx).read(cx);
        let key = object.field_str("key").to_string();
        let format = match object.field_str("format") {
            "" => "shorttext",
            f => f,
        }
        .to_string();
        let options = store
            .relations
            .iter()
            .find(|r| r.id == object.id)
            .map(|r| r.options.clone())
            .unwrap_or_default();
        let chipped = format == "tag" || format == "status";
        let space = current_space_id(cx);
        let n = self.rows.len();
        let focused = self.definition.focus_handle(cx).is_focused(window);
        let mut panel = div()
            .px(px(48.))
            .flex()
            .flex_col()
            .text_color(theme::fg())
            .child(
                div()
                    .ml(px(-8.))
                    .mb(px(8.))
                    .group("def")
                    // A textarea blurs on any outside click (the web's onblur save).
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                        if this.definition.focus_handle(cx).is_focused(window) {
                            window.blur(cx);
                        }
                    }))
                    .child(
                        text_area(
                            &self.definition,
                            window,
                            cx,
                            transparent_black(),
                            theme::radius::md(),
                            (6., 8.),
                        )
                        .text_size(px(13.5))
                        .when(!focused, |d| {
                            d.border_color(transparent_black())
                                .group_hover("def", |s| s.border_color(theme::border()))
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .my(px(13.))
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child(
                        div()
                            .border_1()
                            .border_color(theme::border())
                            .rounded(theme::radius::sm())
                            .px(px(8.))
                            .py(px(2.))
                            .text_size(px(12.))
                            .text_color(theme::fg())
                            .child(format.clone()),
                    )
                    .child(if self.loaded {
                        format!("{n} object{} with a value", if n == 1 { "" } else { "s" })
                    } else {
                        "Loading…".into()
                    }),
            );
        if self.loaded && n == 0 {
            return panel.child(
                div()
                    .my(px(13.))
                    .text_color(theme::muted())
                    .child("No objects have a value for this property yet."),
            );
        }
        if n == 0 {
            return panel;
        }
        let th = |t: &'static str| {
            div()
                .px(px(8.))
                .py(px(6.))
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::muted())
                .child(t)
        };
        let cols = |a: AnyElement, b: AnyElement, c: AnyElement| {
            div()
                .flex()
                .border_b_1()
                .border_color(theme::border())
                .child(div().w(relative(0.42)).min_w_0().child(a))
                .child(div().w(relative(0.42)).min_w_0().max_w(px(340.)).child(b))
                .child(div().flex_1().min_w_0().child(c))
        };
        let mut table = div().mt(px(8.)).flex().flex_col().child(cols(
            th("Name").into_any_element(),
            th("Value").into_any_element(),
            th("Type").into_any_element(),
        ));
        for r in &self.rows {
            let rid = r.id.clone();
            let name = r
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .or_else(|| {
                    Some(field_str(&r.fields, "name").to_string()).filter(|n| !n.is_empty())
                })
                .unwrap_or_else(|| "Untitled".into());
            let name_cell = div()
                .id(SharedString::from(format!("name-{}", r.id)))
                .p(px(8.))
                .text_size(px(14.))
                .font_weight(FontWeight::MEDIUM)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .cursor_pointer()
                .hover(|s| s.underline())
                .on_click(move |_, _, cx| nav::navigate(Route::Object(rid.clone()), cx))
                .child(format!(
                    "{} {name}",
                    object_icon(
                        store,
                        field_str(&r.fields, "iconEmoji"),
                        &r.type_key,
                        &space
                    )
                ));
            let v = r.fields.get(&key);
            let ids = linked_ids(v);
            let mut value = div()
                .p(px(8.))
                .flex()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(14.));
            if !ids.is_empty() {
                for id in ids {
                    let (name, icon, tk) = match store.summary(&id) {
                        Some(s) => (s.name.clone(), s.icon.clone(), s.type_key.clone()),
                        None => match store.agent(&id) {
                            Some(a) => (a.name.clone(), a.icon.clone(), "agent".into()),
                            None => (String::new(), String::new(), String::new()),
                        },
                    };
                    let target = id.clone();
                    value = value.child(
                        div()
                            .id(SharedString::from(format!("link-{}-{id}", r.id)))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .mr(px(10.))
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(move |_, _, cx| {
                                nav::navigate(Route::Object(target.clone()), cx)
                            })
                            .child(object_icon(store, &icon, &tk, &space))
                            .child(if name.is_empty() {
                                "Untitled".to_string()
                            } else {
                                name
                            }),
                    );
                }
            } else if chipped {
                for t in option_texts(v) {
                    let color = options
                        .iter()
                        .find(|o| o.text == t)
                        .map(|o| o.color.clone())
                        .unwrap_or_default();
                    value = value.child(div().mr(px(4.)).child(TagPill::new(t, color)));
                }
            } else {
                value = value.child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(fmt(v, &format)),
                );
            }
            let type_cell = div()
                .p(px(8.))
                .text_size(px(14.))
                .text_color(theme::muted())
                .child(r.type_key.clone());
            table = table.child(cols(
                name_cell.into_any_element(),
                value.into_any_element(),
                type_cell.into_any_element(),
            ));
        }
        panel = panel.child(table);
        panel
    }
}

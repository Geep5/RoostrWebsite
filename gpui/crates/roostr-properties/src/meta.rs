//! The line under an object's title (`ObjectMeta.svelte`): its type (opens
//! the type), who runs a recurring object, a query's source types (each
//! removable, more addable) and how many objects link here (`backlinks.ts`).

use gpui_kit::component::Icon;
use gpui_kit::component::tag::Tag;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::objects::{builtin_glyph, kind_name, object_icon, type_glyph};
use roostr_core::options::badge_colors;
use roostr_core::relations::current_space_id;
use roostr_core::repeat::describe_repeat;
use roostr_core::types::{guest_agents, repeat_of};
use roostr_core::ui::brighten;
use roostr_core::{Fields, Glyph, Route, Store, StoreEvent, Value, api, lite_ui, nav, theme};
use serde_json::json;

use crate::suggest::{SuggestEvent, TypeSuggest};
use crate::widgets::{backdrop, below, pop_shadow, prefix, tip};

#[derive(Clone)]
struct Backlink {
    id: String,
    name: String,
    type_key: String,
    icon: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    Sources,
    Backlinks,
}

/// Type / repeat / query source / backlink badges under an object's title.
pub struct ObjectMeta {
    object_id: String,
    backlinks: Vec<Backlink>,
    open: Option<Open>,
    suggest: Option<Entity<TypeSuggest>>,
    _subs: Vec<Subscription>,
}

impl ObjectMeta {
    /// The badges of `object_id` (loads it into the store cache, fetches its backlinks).
    pub fn new(object_id: String, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        store.update(cx, |s, cx| s.ensure_object(&object_id, cx));
        let mut this = Self {
            object_id,
            backlinks: vec![],
            open: None,
            suggest: None,
            _subs: vec![
                cx.observe(&store, |_, _, cx| cx.notify()),
                // Lite: backlinks come from what this device holds; a refresh
                // (a space finished loading, objects fetched) may bring more.
                cx.subscribe(&store, |this, _, event: &StoreEvent, cx| {
                    if matches!(event, StoreEvent::Refreshed)
                        && roostr_core::lite::status(cx).is_some()
                    {
                        this.load_backlinks(cx);
                    }
                }),
            ],
        };
        this.load_backlinks(cx);
        this
    }

    /// Objects whose fields link to this one (every object is scanned: a cap would hide backlinks).
    fn load_backlinks(&mut self, cx: &mut Context<Self>) {
        let task = api::query_all(json!({}), cx);
        let id = self.object_id.clone();
        cx.spawn(async move |this, cx| {
            let Ok(records) = task.await else { return };
            let links = |fields: &Fields| {
                fields.iter().any(|(key, v)| {
                    if key == "channel" {
                        return false;
                    }
                    if v.link_value.as_ref().is_some_and(|l| l.target_id == id) {
                        return true;
                    }
                    v.items().iter().any(|i| {
                        i.link_value.as_ref().is_some_and(|l| l.target_id == id)
                            || (key == "collectionIds" && i.as_str() == Some(id.as_str()))
                    })
                })
            };
            let out: Vec<Backlink> = records
                .iter()
                .filter(|r| r.id != id && links(&r.fields))
                .map(|r| Backlink {
                    id: r.id.clone(),
                    name: Some(roostr_core::field_str(&r.fields, "name"))
                        .filter(|n| !n.is_empty())
                        .unwrap_or("Untitled")
                        .to_string(),
                    type_key: r.type_key.clone(),
                    icon: roostr_core::field_str(&r.fields, "iconEmoji").to_string(),
                })
                .collect();
            let _ = this.update(cx, |this, cx| {
                this.backlinks = out;
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle(&mut self, which: Open, window: &mut Window, cx: &mut Context<Self>) {
        self.open = if self.open == Some(which) {
            None
        } else {
            Some(which)
        };
        self.suggest = None;
        if self.open == Some(Open::Sources) {
            let Some(object) = Store::global(cx).read(cx).object(&self.object_id) else {
                return;
            };
            let keys = source_keys(&object.fields);
            let placeholder: SharedString = if keys.is_empty() {
                "Search types… (e.g. p → Person, Project)".into()
            } else {
                "Add another type…".into()
            };
            let space = object_space(&object.fields, cx);
            let suggest = cx.new(|cx| TypeSuggest::new(space, keys, placeholder, window, cx));
            let sub = cx.subscribe(
                &suggest,
                |this, _, ev: &SuggestEvent<String>, cx| match ev {
                    SuggestEvent::Picked(key) => {
                        let Some(object) = Store::global(cx).read(cx).object(&this.object_id)
                        else {
                            return;
                        };
                        let mut keys = source_keys(&object.fields);
                        keys.push(key.clone());
                        this.save_sources(keys, cx);
                    }
                    SuggestEvent::Dismissed => {
                        this.open = None;
                        this.suggest = None;
                        cx.notify();
                    }
                },
            );
            self._subs.push(sub);
            self.suggest = Some(suggest);
        }
        cx.notify();
    }

    fn save_sources(&mut self, keys: Vec<String>, cx: &mut Context<Self>) {
        if let Some(s) = &self.suggest {
            let exclude = keys.clone();
            s.update(cx, |s, cx| s.set_exclude(exclude, cx));
        }
        let v = Value::values(keys.into_iter().map(Value::string).collect());
        api::note::set_field(&self.object_id, "setOf", v, cx).detach_and_log_err(cx);
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.suggest = None;
        cx.notify();
    }
}

fn source_keys(fields: &Fields) -> Vec<String> {
    fields
        .get("setOf")
        .map(|v| {
            v.items()
                .iter()
                .filter_map(|i| i.string_value.clone())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The object's own space, else the open one.
fn object_space(fields: &Fields, cx: &App) -> String {
    match roostr_core::field_str(fields, "channel") {
        "" => current_space_id(cx),
        s => s.to_string(),
    }
}

/// An `ObjectMeta` badge: a kit `Tag` (h26, padding 0 10 0 6, gap 6,
/// radius 8, 13/500, neutral badge colours) inside the interactive shell
/// that carries its id, hover brighten, click and tooltip (`Tag` has none).
fn badge(
    id: impl Into<ElementId>,
    lead: AnyElement,
    text: impl Into<SharedString>,
    clickable: bool,
) -> Stateful<Div> {
    let c = badge_colors("");
    div()
        .id(id.into())
        .flex()
        .flex_none()
        .rounded(theme::radius::md())
        .bg(c.bg)
        .hover(move |s| s.bg(brighten(c.bg, 1.18)))
        .when(clickable, |d| d.cursor_pointer())
        .child(
            Tag::custom(transparent_black(), c.fg, transparent_black())
                .border_0()
                .rounded(theme::radius::md())
                .gap(px(6.))
                .h(px(26.))
                .py_0()
                .pl(px(6.))
                .pr(px(10.))
                .text_size(px(13.))
                .line_height(px(26.))
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap()
                .child(lead)
                .child(text.into()),
        )
}

fn emoji(e: String) -> AnyElement {
    div()
        .w(px(16.))
        .text_center()
        .text_size(px(14.))
        .child(e)
        .into_any_element()
}

fn glyph(g: Glyph) -> AnyElement {
    Icon::new(g)
        .size(px(16.))
        .text_color(badge_colors("").icon)
        .into_any_element()
}

fn meta_pop() -> Stateful<Div> {
    div()
        .id("meta-pop")
        .min_w(px(280.))
        .max_w(px(380.))
        .max_h(px(360.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(4.))
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .rounded(theme::radius::menu())
        .p(px(10.))
        .shadow(pop_shadow())
        .text_color(theme::fg())
        .occlude()
}

impl Render for ObjectMeta {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = Store::global(cx).read(cx).object(&self.object_id) else {
            return div();
        };
        let space = object_space(&object.fields, cx);
        let store = Store::global(cx).read(cx);
        let type_def = store
            .types
            .iter()
            .find(|t| t.key == object.type_key && t.space == space)
            .cloned();
        let type_icon = type_def
            .as_ref()
            .map(|t| t.icon.clone())
            .filter(|i| !i.is_empty())
            .unwrap_or_else(|| builtin_glyph(&object.type_key).to_string());
        let type_name = type_def
            .as_ref()
            .map(|t| t.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| kind_name(&object.type_key));
        let lead = || {
            if type_icon.is_empty() {
                glyph(Glyph::BadgeDot)
            } else {
                emoji(type_icon.clone())
            }
        };
        // `.meta`: `margin: 2px 0 14px 48px`, `0 16px 10px` at ≤720px.
        let narrow = roostr_core::layout::is_mobile(window);
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .when(narrow, |d| d.mb(px(10.)).mx(px(16.)))
            .when(!narrow, |d| d.mt(px(2.)).mb(px(14.)).ml(px(48.)))
            .text_size(px(13.));
        row = row.child(match &type_def {
            Some(t) => {
                let id = t.id.clone();
                badge("type", lead(), type_name, true)
                    .tooltip(tip("Type"))
                    .on_click(move |_, _, cx| nav::navigate(Route::Object(id.clone()), cx))
            }
            None => badge("type", lead(), type_name, false).tooltip(tip("Type")),
        });
        if let Some(rule) = repeat_of(&object.fields) {
            let mut ids = guest_agents(&object.fields);
            if let Some(a) = object.field("assignee") {
                ids.extend(guest_agents(&Fields::from([(
                    "agent".to_string(),
                    a.clone(),
                )])));
            }
            let desc = describe_repeat(&rule);
            match ids.first() {
                Some(id) => {
                    let a = store.agent(id);
                    let icon = a
                        .map(|a| a.icon.clone())
                        .filter(|i| !i.is_empty())
                        .unwrap_or_else(|| "🤖".into());
                    let name = a
                        .map(|a| a.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| format!("{}…", prefix(id, 8)));
                    let target = id.clone();
                    row = row.child(
                        badge("runner", emoji(icon), name.clone(), true)
                            .tooltip(tip(format!("{name} runs this · {desc}")))
                            .on_click(move |_, _, cx| {
                                nav::navigate(Route::Object(target.clone()), cx)
                            }),
                    );
                }
                None => {
                    row = row.child(
                        badge("runner", emoji("↻".into()), "Repeats", false).tooltip(tip(format!(
                            "Repeats · {desc} · no agent: add one to its Agent property"
                        ))),
                    );
                }
            }
        }
        if object.type_key == "query" {
            let keys = source_keys(&object.fields);
            let sources: Vec<(String, String, String, String)> = keys
                .iter()
                .map(|key| {
                    let t = store
                        .types
                        .iter()
                        .find(|x| &x.key == key && x.space == space);
                    let built_in = match key.as_str() {
                        "query" => "Query",
                        "collection" => "Collection",
                        _ => "",
                    };
                    let name = t
                        .map(|t| t.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| {
                            if built_in.is_empty() {
                                key.clone()
                            } else {
                                built_in.into()
                            }
                        });
                    let icon = t
                        .map(|t| t.icon.clone())
                        .filter(|i| !i.is_empty())
                        .unwrap_or_else(|| type_glyph(store, key, &space));
                    (
                        key.clone(),
                        t.map(|t| t.id.clone()).unwrap_or_default(),
                        name,
                        icon,
                    )
                })
                .collect();
            let (lead, text) = match sources.first() {
                Some(first) => (
                    emoji(first.3.clone()),
                    format!(
                        "Object {}: {}",
                        if sources.len() == 1 { "Type" } else { "Types" },
                        sources
                            .iter()
                            .map(|s| s.2.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ),
                None => (glyph(Glyph::BadgeDot), "All objects".to_string()),
            };
            let mut wrap = div().relative().flex().child(
                badge("sources", lead, text, true)
                    .tooltip(tip("What this query shows"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.toggle(Open::Sources, window, cx)),
                    ),
            );
            if self.open == Some(Open::Sources) {
                let mut pop = meta_pop();
                for (key, id, name, icon) in sources {
                    let main = div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex_none()
                                .size(px(40.))
                                .rounded(theme::radius::md())
                                .bg(theme::hover())
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(20.))
                                .child(icon),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .min_w_0()
                                .child(div().text_size(px(14.)).child("Object Type"))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(theme::muted())
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(name.clone()),
                                ),
                        );
                    let main = if id.is_empty() {
                        main.into_any_element()
                    } else {
                        div()
                            .id(SharedString::from(format!("src-{key}")))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.close(cx);
                                nav::navigate(Route::Object(id.clone()), cx);
                            }))
                            .child(main)
                            .into_any_element()
                    };
                    let k = key.clone();
                    pop = pop.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .p(px(8.))
                            .rounded(theme::radius::md())
                            .bg(theme::hl_med())
                            .child(main)
                            .child(
                                div()
                                    .id(SharedString::from(format!("src-rm-{key}")))
                                    .px(px(4.))
                                    .text_color(theme::muted())
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme::fg()))
                                    .tooltip(tip("Remove"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let Some(object) =
                                            Store::global(cx).read(cx).object(&this.object_id)
                                        else {
                                            return;
                                        };
                                        let keys = source_keys(&object.fields)
                                            .into_iter()
                                            .filter(|x| *x != k)
                                            .collect();
                                        this.save_sources(keys, cx);
                                    }))
                                    .child("×"),
                            ),
                    );
                }
                if let Some(s) = &self.suggest {
                    pop = pop.child(s.clone());
                }
                wrap = wrap.child(below("sources-pop", 6., pop));
            }
            row = row.child(wrap);
        }
        let n = self.backlinks.len();
        if n > 0 {
            let mut wrap = div().relative().flex().child(
                badge(
                    "backlinks",
                    glyph(Glyph::BadgeLink),
                    format!("{n} backlink{}", if n == 1 { "" } else { "s" }),
                    true,
                )
                .tooltip(tip("Backlinks"))
                .on_click(
                    cx.listener(|this, _, window, cx| this.toggle(Open::Backlinks, window, cx)),
                ),
            );
            if self.open == Some(Open::Backlinks) {
                let mut pop = meta_pop().child(
                    div()
                        .mb(px(4.))
                        .text_size(px(11.))
                        .text_color(theme::muted())
                        .child("LINKED FROM"),
                );
                for b in &self.backlinks {
                    let id = b.id.clone();
                    pop = pop.child(
                        div()
                            .id(SharedString::from(format!("bl-{}", b.id)))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(theme::radius::sm())
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.close(cx);
                                nav::navigate(Route::Object(id.clone()), cx);
                            }))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(18.))
                                    .text_center()
                                    .child(object_icon(store, &b.icon, &b.type_key, &space)),
                            )
                            .child(b.name.clone())
                            .child(
                                div()
                                    .ml_auto()
                                    .text_size(px(11.))
                                    .text_color(theme::muted())
                                    .child(b.type_key.clone()),
                            ),
                    );
                }
                wrap = wrap.child(below("backlinks-pop", 6., pop));
            }
            row = row.child(wrap);
        }
        // Lite: backlinks above are only what this device holds.
        if let Some(status) = roostr_core::lite::status(cx)
            && !status.is_full(&space)
        {
            let label = match status.loading(&space) {
                Some(load) => lite_ui::loading_text(load),
                None => lite_ui::LOAD_SPACE.to_string(),
            };
            let target = space.clone();
            row = row.child(
                badge("backlinks-lite", glyph(Glyph::BadgeLink), label, true)
                    .tooltip(tip(
                        "Backlinks from what this device holds; load the space for all of them",
                    ))
                    .on_click(move |_, _, cx| lite_ui::load_space(&target, cx)),
            );
        }
        if self.open.is_some() {
            let this = cx.entity().downgrade();
            row = row.child(backdrop("meta-backdrop", move |_, cx| {
                let _ = this.update(cx, |this, cx| this.close(cx));
            }));
        }
        row
    }
}

//! The sidebar (`aside.widgets`, `+layout.svelte:1376-1521`, CSS 1728-2414):
//! space head (settings link, search, New pill + menu), one card per pinned
//! object, then Recently edited, Types and Properties.

use gpui_kit::component::IconNamed as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::nav::{Navigator, Route, navigate};
use roostr_core::objects::{object_icon, type_glyph};
use roostr_core::relations::{format_glyph, is_reserved_key, space_relations};
use roostr_core::store::Store;
use roostr_core::types::{ObjectSummary, RelationDef, Space, Value, is_system_type};
use roostr_core::{Glyph, TypeDef, UiState, theme};
use roostr_properties::{PropertyPick, PropertySuggest, SuggestEvent};

use crate::create_menu::{CreateMenu, CreateMenuEvent};
use crate::modal::{Prompt, PromptEvent};
use crate::pinned::PinnedWidget;
use crate::rail::DragGhost;
use crate::tabs::checkbox_icon;
use crate::workspace::Workspace;

#[derive(Clone)]
pub(crate) struct WidgetDrag {
    id: String,
    icon: String,
    name: String,
}

/// `.obj-icon`: 20px wide, centred, accent (emoji keep their own colours).
pub(crate) fn obj_icon(icon: &str) -> Div {
    div()
        .flex_none()
        .w(px(20.))
        .flex()
        .justify_center()
        .text_color(theme::accent())
        .child(roostr_core::ui::icon_text(icon, px(16.)))
}

/// The Builtin mark: a muted lock pushed to the row's end.
fn builtin_mark(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .ml_auto()
        .pl(px(6.))
        .flex()
        .flex_none()
        .opacity(0.55)
        .child(
            svg()
                .path(Glyph::Lock.path())
                .size(px(11.))
                .text_color(theme::muted()),
        )
        .tooltip(crate::tip("Roostr default - can't be deleted"))
}

/// A sidebar row (`.item`): h26, padding 0 8, radius 6, 13px, hover/current hl-med.
fn item(id: impl Into<ElementId>, current: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(26.))
        .px(px(8.))
        .rounded(theme::radius::sm())
        .text_size(px(13.))
        .line_height(px(22.))
        .whitespace_nowrap()
        .overflow_hidden()
        .cursor_pointer()
        .map(|d| {
            if current {
                d.bg(theme::hl_med())
            } else {
                d.hover(|s| s.bg(theme::hl_med()))
            }
        })
}

fn open(id: String) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| navigate(Route::Object(id.clone()), cx)
}

impl Workspace {
    pub(crate) fn render_sidebar(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let card = div()
            .ml(px(6.))
            .flex_none()
            .w(px(width))
            .h_full()
            .flex()
            .flex_col()
            .rounded(theme::radius::pane())
            .line_height(relative(1.2))
            .bg(theme::bg())
            .overflow_hidden();
        let Some(current) = Self::current_space(cx) else {
            return card;
        };
        let open_id = Self::open_object_id(cx);
        card.child(self.render_space_head(&current, window, cx))
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.sidebar_scroll)
                    .px(px(8.))
                    .pt(px(8.))
                    .pb(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(self.render_pinned(&current, open_id.as_deref(), cx))
                    .child(self.render_recent(&current, open_id.as_deref(), cx))
                    .child(self.render_types(open_id.as_deref(), cx))
                    .child(self.render_props(open_id.as_deref(), window, cx)),
            )
    }

    fn render_space_head(
        &mut self,
        current: &Space,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let icon = if current.icon.is_empty() {
            "◍".to_string()
        } else {
            current.icon.clone()
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(8.))
            .pt(px(10.))
            .pb(px(6.))
            .bg(theme::bg())
            .child(
                div()
                    .id("space-head")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(theme::radius::sm())
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(px(22.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hl_med()))
                    .child(
                        div()
                            .flex_none()
                            .size(px(22.))
                            .mr(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme::radius::sm())
                            .overflow_hidden()
                            .text_size(px(17.))
                            .child(roostr_core::ui::icon_text(&icon, px(22.))),
                    )
                    .child(div().min_w_0().truncate().child(current.name.clone()))
                    .tooltip(crate::tip("Space settings"))
                    .on_click(open(current.id.clone())),
            )
            .child(
                div()
                    .id("head-search")
                    .flex_none()
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::radius::md())
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hl_med()).text_color(theme::fg()))
                    .child(
                        svg()
                            .path(Glyph::Search.path())
                            .size(px(17.))
                            .text_color(theme::muted()),
                    )
                    .map(|d| self.data_tip(d, "tip-search", "Search (⌘K)", cx))
                    .on_click(cx.listener(|this, _, window, cx| this.open_search(window, cx))),
            )
            .child(
                div().relative().child(
                    div()
                        .id("create-btn")
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .h(px(30.))
                        .px(px(7.))
                        .rounded(theme::radius::pill())
                        .bg(theme::hl_med())
                        .text_color(theme::muted())
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
                        .child(
                            svg()
                                .path(Glyph::Compose.path())
                                .size(px(15.))
                                .text_color(theme::muted()),
                        )
                        .child(
                            svg()
                                .path(Glyph::ChevronDown.path())
                                .size(px(11.))
                                .text_color(theme::muted()),
                        )
                        .tooltip(crate::tip("New object"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.create_menu.is_some() {
                                this.create_menu = None;
                                cx.notify();
                            } else {
                                this.open_create_menu(window, cx);
                            }
                        }))
                        .child(self.anchor_probe("create")),
                ),
            )
    }

    pub(crate) fn open_create_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.create_menu.is_some() {
            return;
        }
        let menu = cx.new(|cx| CreateMenu::new(window, cx));
        cx.subscribe_in(
            &menu,
            window,
            |this, _, event: &CreateMenuEvent, window, cx| match event {
                CreateMenuEvent::Close => {
                    this.create_menu = None;
                    cx.notify();
                }
                CreateMenuEvent::Create(kind) => this.sidebar_create(kind.clone(), window, cx),
            },
        )
        .detach();
        self.create_menu = Some(menu);
        cx.notify();
    }

    fn flip_widget(&mut self, id: &str, cx: &mut Context<Self>) {
        let collapsed = !self.widget_collapsed.get(id).copied().unwrap_or(false);
        self.widget_collapsed.insert(id.to_string(), collapsed);
        self.flip_arrow(format!("widget:{id}"));
        UiState::set(cx, "widget-collapsed", &self.widget_collapsed);
        cx.notify();
    }

    fn flip_section(&mut self, key: &str, cx: &mut Context<Self>) {
        let collapsed = !self.section_collapsed.get(key).copied().unwrap_or(false);
        self.section_collapsed.insert(key.to_string(), collapsed);
        self.flip_arrow(format!("section:{key}"));
        UiState::set(cx, "section-collapsed", &self.section_collapsed);
        cx.notify();
    }

    fn render_pinned(
        &mut self,
        current: &Space,
        open_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let pinned: Vec<ObjectSummary> = {
            let store = Store::global(cx).read(cx);
            current
                .pinned_ids
                .iter()
                .filter_map(|id| store.summary(id).cloned())
                .collect()
        };
        let active = Navigator::global(cx).read(cx).space().to_string();
        let dragging = cx.has_active_drag();
        let drag_id = self.widget_drag.clone().filter(|_| dragging);
        let over = self.widget_over.clone().filter(|_| dragging);
        let mut out = Vec::new();
        for p in pinned {
            let id = p.id.clone();
            let collapsed = self.widget_collapsed.get(&id).copied().unwrap_or(false);
            let icon = object_icon(Store::global(cx).read(cx), &p.icon, &p.type_key, &active);
            let name = if p.name.is_empty() {
                "Untitled".to_string()
            } else {
                p.name.clone()
            };
            let is_current = open_id == Some(id.as_str());
            let body = (!collapsed).then(|| {
                self.pinned_views
                    .entry(id.clone())
                    .or_insert_with(|| cx.new(|cx| PinnedWidget::new(id.clone(), cx)))
                    .clone()
            });
            if let Some(view) = &body {
                let side_width = self.side_width;
                view.update(cx, |w, _| w.side_width = side_width);
            }
            let over_here = over
                .as_ref()
                .filter(|(o, _)| *o == id)
                .map(|(_, bottom)| *bottom);
            let payload = WidgetDrag {
                id: id.clone(),
                icon: icon.clone(),
                name: name.clone(),
            };
            let card = div()
                .id(crate::eid("widget", &id))
                .relative()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::border())
                .rounded(theme::radius::menu())
                .p(px(8.))
                .when(drag_id.as_deref() == Some(&id), |d| d.opacity(0.4))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .id(crate::eid("widget-collapse", &id))
                                .group(SharedString::from(format!("widget-collapse:{id}")))
                                .flex_none()
                                .w(px(16.))
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .opacity(0.6)
                                .cursor_pointer()
                                .hover(|s| s.opacity(1.))
                                .child(
                                    self.arrow(&format!("widget:{id}"), !collapsed, 7.)
                                        .text_color(theme::muted())
                                        .group_hover(format!("widget-collapse:{id}"), |s| {
                                            s.text_color(theme::fg())
                                        }),
                                )
                                .on_click(cx.listener({
                                    let id = id.clone();
                                    move |this, _, _, cx| this.flip_widget(&id, cx)
                                })),
                        )
                        .child(
                            div()
                                .id(crate::eid("widget-head", &id))
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .h(px(28.))
                                .pl(px(2.))
                                .pr(px(8.))
                                .rounded(theme::radius::sm())
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .line_height(px(22.))
                                .overflow_hidden()
                                .cursor_pointer()
                                .map(|d| {
                                    if is_current {
                                        d.bg(theme::hl_med())
                                    } else {
                                        d.hover(|s| s.bg(theme::hl_med()))
                                    }
                                })
                                .child(obj_icon(&icon))
                                .child(div().min_w_0().truncate().child(name.clone()))
                                .on_click(open(id.clone())),
                        ),
                )
                .children(body)
                .when_some(over_here, |d, bottom| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(12.))
                            .right(px(12.))
                            .h(px(2.))
                            .rounded(px(2.))
                            .bg(theme::accent())
                            .map(|l| {
                                if bottom {
                                    l.bottom(px(-5.))
                                } else {
                                    l.top(px(-5.))
                                }
                            }),
                    )
                })
                .on_drag(payload, |payload, _, _, cx| {
                    let payload = payload.clone();
                    let ghost = move || {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .px(px(8.))
                            .h(px(28.))
                            .rounded(theme::radius::menu())
                            .bg(theme::panel())
                            .border_1()
                            .border_color(theme::border())
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::fg())
                            .opacity(0.8)
                            .child(obj_icon(&payload.icon))
                            .child(payload.name.clone())
                            .into_any_element()
                    };
                    cx.new(|_| DragGhost(Box::new(ghost)))
                })
                .on_drag_move(cx.listener({
                    let id = id.clone();
                    move |this, e: &DragMoveEvent<WidgetDrag>, _, cx| {
                        let dragged = e.drag(cx).id.clone();
                        if this.widget_drag.as_deref() != Some(&dragged) {
                            this.widget_drag = Some(dragged.clone());
                            cx.notify();
                        }
                        if dragged == id {
                            return;
                        }
                        if e.bounds.contains(&e.event.position) {
                            let bottom = e.event.position.y > e.bounds.center().y;
                            if this.widget_over != Some((id.clone(), bottom)) {
                                this.widget_over = Some((id.clone(), bottom));
                                cx.notify();
                            }
                        } else if this.widget_over.as_ref().is_some_and(|(o, _)| *o == id) {
                            this.widget_over = None;
                            cx.notify();
                        }
                    }
                }))
                .on_drop(cx.listener(|this, payload: &WidgetDrag, _, cx| {
                    let target = this.widget_over.take();
                    this.widget_drag = None;
                    cx.notify();
                    let (Some((target, bottom)), Some(space)) = (target, Self::current_space(cx))
                    else {
                        return;
                    };
                    roostr_core::pins::move_pin(&space, &payload.id, &target, bottom, cx)
                        .detach_and_log_err(cx);
                }));
            out.push(card.into_any_element());
        }
        out
    }

    fn section(
        &self,
        key: &'static str,
        label: &'static str,
        add: Option<AnyElement>,
        rows: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = !self.section_collapsed.get(key).copied().unwrap_or(false);
        let mut body = div()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::menu())
            .px(px(8.))
            .py(px(4.))
            .flex()
            .flex_col();
        for (i, row) in rows.into_iter().enumerate() {
            body = body.child(
                div()
                    .when(i > 0, |d| d.border_t_1().border_color(theme::border()))
                    .child(row),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(4.))
                    .child(
                        div()
                            .id(crate::eid("section", key))
                            .group(SharedString::from(format!("section:{key}")))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .pt(px(2.))
                            .px(px(8.))
                            .pb(px(6.))
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(px(18.))
                            .text_color(theme::muted())
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme::fg()))
                            .child(
                                div().opacity(0.7).child(
                                    self.arrow(&format!("section:{key}"), open, 7.)
                                        .text_color(theme::muted())
                                        .group_hover(format!("section:{key}"), |s| {
                                            s.text_color(theme::fg())
                                        }),
                                ),
                            )
                            .child(roostr_core::ui::spaced_text(label.to_uppercase(), 0.05))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.flip_section(key, cx)),
                            ),
                    )
                    .children(add),
            )
            .when(open, |d| d.child(body))
    }

    fn section_add(id: &'static str, tip: &'static str) -> Stateful<Div> {
        div()
            .id(id)
            .relative()
            .flex_none()
            .size(px(20.))
            .mr(px(2.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::radius::xs())
            .text_size(px(12.))
            .line_height(px(12.))
            .text_color(theme::muted())
            .opacity(0.5)
            .cursor_pointer()
            .hover(|s| s.opacity(1.).bg(theme::hl_med()).text_color(theme::fg()))
            .child("＋")
            .tooltip(crate::tip(tip))
    }

    fn render_recent(
        &mut self,
        current: &Space,
        open_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Div {
        let active = Navigator::global(cx).read(cx).space().to_string();
        let rows: Vec<AnyElement> = {
            let store = Store::global(cx).read(cx);
            let recent: Vec<&ObjectSummary> = store
                .summaries
                .iter()
                .filter(|s| {
                    !current.pinned_ids.contains(&s.id)
                        && !is_system_type(&s.type_key)
                        && s.channel_id == current.id
                })
                .take(8)
                .collect();
            if recent.is_empty() {
                vec![none("Nothing yet")]
            } else {
                recent
                    .into_iter()
                    .map(|r| {
                        let name = if r.name.is_empty() {
                            "Untitled".to_string()
                        } else {
                            r.name.clone()
                        };
                        let lead = if store.layout_of(&r.type_key) == "task" {
                            let done = r.done == Some(true);
                            let id = r.id.clone();
                            div()
                                .id(crate::eid("row-check", &r.id))
                                .flex()
                                .flex_none()
                                .mr(px(6.))
                                .text_color(if done {
                                    theme::accent()
                                } else {
                                    theme::muted()
                                })
                                .child(checkbox_icon(done, 16.))
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    let task = roostr_core::api::note::set_field(
                                        &id,
                                        "done",
                                        Value::bool(!done),
                                        cx,
                                    );
                                    cx.spawn(async move |cx| {
                                        if task.await.is_ok() {
                                            cx.update(|cx| {
                                                Store::global(cx)
                                                    .update(cx, |s, cx| s.refresh_all(cx))
                                            });
                                        }
                                    })
                                    .detach();
                                })
                                .into_any_element()
                        } else {
                            obj_icon(&object_icon(store, &r.icon, &r.type_key, &active))
                                .into_any_element()
                        };
                        item(crate::eid("recent", &r.id), open_id == Some(r.id.as_str()))
                            .child(lead)
                            .child(div().min_w_0().truncate().child(name))
                            .on_click(open(r.id.clone()))
                            .into_any_element()
                    })
                    .collect()
            }
        };
        self.section("recent", "Recently edited", None, rows, cx)
    }

    fn render_types(&mut self, open_id: Option<&str>, cx: &mut Context<Self>) -> Div {
        let space_id = Self::space_id(cx);
        let rows: Vec<AnyElement> = {
            let store = Store::global(cx).read(cx);
            let active = Navigator::global(cx).read(cx).space().to_string();
            store
                .types
                .iter()
                .filter(|t| t.space == space_id)
                .map(|t| {
                    let icon = if t.icon.is_empty() {
                        type_glyph(store, &t.key, &active)
                    } else {
                        t.icon.clone()
                    };
                    let name = if t.name.is_empty() {
                        t.key.clone()
                    } else {
                        t.name.clone()
                    };
                    let current = open_id == Some(t.id.as_str());
                    div()
                        .rounded(theme::radius::sm())
                        .when(current, |d| d.bg(theme::hover()))
                        .child(
                            item(crate::eid("type", &t.id), false)
                                .child(obj_icon(&icon))
                                .child(div().min_w_0().truncate().child(name))
                                .when(t.builtin, |d| {
                                    d.child(builtin_mark(crate::eid("type-lock", &t.id)))
                                })
                                .on_click(open(t.id.clone())),
                        )
                        .into_any_element()
                })
                .collect()
        };
        let add = Self::section_add("new-type", "New type")
            .on_click(cx.listener(|this, _, window, cx| this.new_type(window, cx)))
            .into_any_element();
        self.section("types", "Types", Some(add), rows, cx)
    }

    fn render_props(
        &mut self,
        open_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let space_id = Self::space_id(cx);
        let rows: Vec<AnyElement> = {
            let store = Store::global(cx).read(cx);
            let mut props: Vec<&RelationDef> = space_relations(&store.relations, &space_id)
                .into_iter()
                .filter(|r| !r.hidden && !is_reserved_key(&r.key))
                .collect();
            let label = |r: &RelationDef| {
                if r.name.is_empty() {
                    r.key.to_lowercase()
                } else {
                    r.name.to_lowercase()
                }
            };
            props.sort_by_key(|r| label(r));
            if props.is_empty() {
                vec![none("No properties yet")]
            } else {
                props
                    .into_iter()
                    .map(|r| {
                        let icon = r
                            .icon_emoji
                            .clone()
                            .filter(|i| !i.is_empty())
                            .unwrap_or_else(|| format_glyph(&r.format).to_string());
                        let name = if r.name.is_empty() {
                            r.key.clone()
                        } else {
                            r.name.clone()
                        };
                        item(crate::eid("prop", &r.id), open_id == Some(r.id.as_str()))
                            .child(obj_icon(&icon))
                            .child(div().min_w_0().truncate().child(name))
                            .when(store.builtin_ids.contains(&r.id), |d| {
                                d.child(builtin_mark(crate::eid("prop-lock", &r.id)))
                            })
                            .on_click(open(r.id.clone()))
                            .into_any_element()
                    })
                    .collect()
            }
        };
        let add = Self::section_add("new-prop", "New property")
            .child(self.anchor_probe("new-prop"))
            .on_click(cx.listener(|this, _, window, cx| this.open_new_property(window, cx)))
            .into_any_element();
        let _ = window;
        self.section("props", "Properties", Some(add), rows, cx)
    }

    pub(crate) fn open_new_property(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.new_prop.is_some() {
            self.new_prop = None;
            cx.notify();
            return;
        }
        let space_id = Self::space_id(cx);
        let suggest =
            cx.new(|cx| PropertySuggest::new(space_id, Vec::new(), Vec::new(), window, cx));
        cx.subscribe(
            &suggest,
            |this, _, event: &SuggestEvent<PropertyPick>, cx| {
                this.new_prop = None;
                cx.notify();
                if let SuggestEvent::Picked(PropertyPick::Relation(rel)) = event {
                    navigate(Route::Object(rel.id.clone()), cx);
                }
            },
        )
        .detach();
        suggest.focus_handle(cx).focus(window, cx);
        self.new_prop = Some(suggest);
        cx.notify();
    }

    /// `newType`: name prompt; per-space key clash check; restore a binned
    /// type with that key, else create it; open it.
    pub(crate) fn new_type(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.new(|cx| Prompt::ask("Type name:", window, cx));
        cx.subscribe_in(
            &prompt,
            window,
            |this, _, event: &PromptEvent, window, cx| {
                this.prompt = None;
                cx.notify();
                let PromptEvent::Submit(name) = event else {
                    return;
                };
                let name = name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                let key = type_key_of(&name);
                if key.is_empty() {
                    return;
                }
                let space = Self::space_id(cx);
                let clash =
                    Store::global(cx).read(cx).types.iter().any(|t: &TypeDef| {
                        t.key == key && (t.space == space || t.space.is_empty())
                    });
                if clash {
                    let alert = cx.new(|cx| {
                        Prompt::alert(
                            format!("A type with key \"{key}\" already exists in this space."),
                            window,
                            cx,
                        )
                    });
                    cx.subscribe(&alert, |this, _, _: &PromptEvent, cx| {
                        this.prompt = None;
                        cx.notify();
                    })
                    .detach();
                    this.prompt = Some(alert);
                    return;
                }
                let binned = roostr_core::api::query_all(
                    serde_json::json!({ "type": "type", "includeDeleted": true }),
                    cx,
                );
                cx.spawn(async move |_, cx| {
                    let rows = binned.await.unwrap_or_default();
                    let found = rows.into_iter().find(|r| {
                        r.deleted == Some(true)
                            && roostr_core::types::field_str(&r.fields, "key") == key
                            && roostr_core::types::field_str(&r.fields, "channel") == space
                    });
                    let id = if let Some(row) = found {
                        let task = cx.update(|cx| roostr_core::api::note::restore(&row.id, cx));
                        if task.await.is_err() {
                            return;
                        }
                        row.id
                    } else {
                        let mut fields = roostr_core::types::Fields::new();
                        fields.insert("channel".into(), Value::string(space));
                        fields.insert("key".into(), Value::string(key));
                        fields.insert("layout".into(), Value::string("page"));
                        let task = cx.update(|cx| {
                            roostr_core::api::note::create(&name, Some("type"), Some(fields), cx)
                        });
                        let Ok(id) = task.await else { return };
                        id
                    };
                    cx.update(|cx| {
                        Store::global(cx).update(cx, |s, cx| s.refresh_all(cx));
                        navigate(Route::Object(id), cx);
                    });
                })
                .detach();
            },
        )
        .detach();
        self.prompt = Some(prompt);
        cx.notify();
    }
}

/// `.none`: the empty-section note.
fn none(text: &'static str) -> AnyElement {
    div()
        .text_size(px(12.))
        .text_color(theme::muted())
        .px(px(8.))
        .child(text)
        .into_any_element()
}

/// Type key from a name: lowercase, runs of non-alphanumerics → `_`, trimmed.
fn type_key_of(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

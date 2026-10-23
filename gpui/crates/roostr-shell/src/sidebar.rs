//! The sidebar (`aside.widgets`, `+layout.svelte:1376-1521`, CSS 1728-2414):
//! space head (settings link, search, New pill + menu), one card per pinned
//! object, then Recently edited, Types and Properties.

use gpui_kit::component::button::Button;
use gpui_kit::component::empty::Empty;
use gpui_kit::component::group_box::GroupBox;
use gpui_kit::component::sidebar::{SidebarItem, SidebarMenuItem};
use gpui_kit::component::{Icon, Sizable as _, Size};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::motion::{self, HoverMotion};
use roostr_core::nav::{Navigator, Route, navigate};
use roostr_core::objects::{object_icon, type_glyph};
use roostr_core::relations::{format_glyph, is_reserved_key, space_relations};
use roostr_core::store::Store;
use roostr_core::types::{ObjectSummary, RelationDef, Space, Value, is_system_type};
use roostr_core::{Glyph, TypeDef, UiState, theme, ui};
use roostr_properties::{PropertyPick, PropertySuggest, SuggestEvent};

use crate::Tip as _;
use crate::create_menu::{CreateMenu, CreateMenuEvent};
use crate::list_motion::{self, PinMotion, Pins};
use crate::modal::{Prompt, PromptEvent};
use crate::pinned::PinnedWidget;
use crate::rail::DragGhost;
use crate::workspace::{self, Workspace};

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

/// The Builtin mark (`.builtin-mark`): a muted 11px lock pushed to the
/// row's end, explaining itself in a tooltip.
pub(crate) fn builtin_mark(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .ml_auto()
        .pl(px(6.))
        .flex()
        .flex_none()
        .opacity(0.55)
        .child(
            Icon::new(Glyph::Lock)
                .size(px(11.))
                .text_color(theme::muted()),
        )
        .tooltip(ui::tooltip("Roostr default - can't be deleted"))
}

/// A sidebar row (`.item`) as gpui-kit's [`SidebarMenuItem`], held to
/// Roostr's h26, padding 0 8, radius 6, 13px (hover and current are the
/// kit's sidebar accent, `hl-med`). The label leaves 26px for the row's
/// lead, which [`Row`] lays over it.
fn item(label: impl Into<SharedString>, current: bool) -> SidebarMenuItem {
    SidebarMenuItem::new(label)
        .active(current)
        .label_style(StyleRefinement::default().pl(px(26.)).truncate())
        .min_h(px(26.))
        .max_h(px(26.))
        .py_0()
        .px(px(8.))
        .rounded(theme::radius::sm())
        .text_size(px(13.))
        .line_height(px(22.))
        .cursor_pointer()
}

/// A sidebar row to draw: the kit item needs the window and app, so the
/// rows are gathered under the store borrow and drawn after it.
struct Row {
    id: ElementId,
    /// What wraps the row (the Types list's current fill and selector).
    outer: Div,
    item: SidebarMenuItem,
    /// The object icon, or a task's checkbox (clickable on its own).
    lead: AnyElement,
}

impl Row {
    fn render(self, window: &mut Window, cx: &mut App) -> Div {
        self.outer
            .relative()
            .child(SidebarItem::render(self.item, self.id, window, cx))
            .child(
                div()
                    .absolute()
                    .left(px(8.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(self.lead),
            )
    }

    /// `rows` drawn, or the section's `none` note when there are none.
    fn render_all(
        rows: Vec<Row>,
        none_text: &'static str,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        if rows.is_empty() {
            return vec![none(none_text)];
        }
        rows.into_iter()
            .map(|row| row.render(window, cx).into_any_element())
            .collect()
    }
}

fn open(id: String) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| navigate(Route::Object(id.clone()), cx)
}

/// Write a task's `done` (a row's checkbox), then refresh the lists.
pub(crate) fn set_done(id: &str, done: bool, cx: &mut App) {
    let task = roostr_core::api::note::set_field(id, "done", Value::bool(done), cx);
    cx.spawn(async move |cx| {
        if task.await.is_ok() {
            cx.update(|cx| Store::global(cx).update(cx, |s, cx| s.refresh_all(cx)));
        }
    })
    .detach();
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
        let pins = list_motion::pins("pins", &current.id, &current.pinned_ids, window, cx);
        let pinned = self.render_pinned(&current, open_id.as_deref(), &pins, window, cx);
        let recent = self.render_recent(&current, open_id.as_deref(), window, cx);
        let types = self.render_types(open_id.as_deref(), window, cx);
        let props = self.render_props(open_id.as_deref(), window, cx);
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
                    .children(pinned)
                    .child(pins.follow("section:recent", recent))
                    .child(pins.follow("section:types", types))
                    .child(pins.follow("section:props", props)),
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
                crate::fill_button("space-head", theme::hl_med(), cx)
                    .flex_1()
                    .min_w_0()
                    .h_auto()
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(theme::radius::sm())
                    .text_color(theme::fg())
                    .accessibility_label(current.name.clone())
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_start()
                            .gap(px(10.))
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(px(22.))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(22.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(theme::radius::sm())
                                    .overflow_hidden()
                                    .text_size(px(17.))
                                    .child(roostr_core::ui::icon_text(&icon, px(22.))),
                            )
                            .child(div().min_w_0().truncate().child(current.name.clone())),
                    )
                    .tip("Space settings")
                    .on_click(open(current.id.clone())),
            )
            .child(
                crate::fill_button("head-search", theme::hl_med(), cx)
                    .size(px(30.))
                    .rounded(theme::radius::md())
                    .child(Icon::new(Glyph::Search).size(px(17.)))
                    .accessibility_label("Search")
                    .tooltip_with_action("Search", &crate::Search, None)
                    .tooltip_show_delay(crate::TIP_DELAY)
                    .on_click(cx.listener(|this, _, window, cx| this.open_search(window, cx))),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .child(
                        crate::fill_button("create-btn", theme::hover(), cx)
                            .h(px(30.))
                            .px(px(7.))
                            .rounded(theme::radius::pill())
                            .bg(theme::hl_med())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(3.))
                                    .child(Icon::new(Glyph::Compose).size(px(15.)))
                                    .child(Icon::new(Glyph::ChevronDown).size(px(11.))),
                            )
                            .accessibility_label("New object")
                            .tip("New object")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.create_menu.is_some() {
                                    this.create_menu = None;
                                    cx.notify();
                                } else {
                                    this.open_create_menu(window, cx);
                                }
                            })),
                    )
                    .child(self.anchor_probe("create")),
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

    fn flip_section(&mut self, key: &str, cx: &mut Context<Self>) {
        let collapsed = !self.section_collapsed.get(key).copied().unwrap_or(false);
        self.section_collapsed.insert(key.to_string(), collapsed);
        UiState::set(cx, "section-collapsed", &self.section_collapsed);
        cx.notify();
    }

    fn render_pinned(
        &mut self,
        current: &Space,
        open_id: Option<&str>,
        pins: &Pins,
        window: &mut Window,
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
        // Lite: the vault or this space not ready yet, or the space loading.
        if let Some(notice) = roostr_core::lite_ui::ready_notice(&current.id, cx) {
            out.push(div().px(px(8.)).child(notice).into_any_element());
        }
        for p in pinned {
            let id = p.id.clone();
            let collapsed = self.pin_collapsed(&id, cx);
            // Only this device's own fold moves (Svelte popped the body); one
            // sync brings in snaps.
            let own = self.pin_folds.contains_key(&id);
            let fold = list_motion::fold(
                crate::eid("widget-fold", &id),
                !collapsed,
                own.then_some(motion::CONTROL),
                true,
                window,
                cx,
            );
            let icon = object_icon(Store::global(cx).read(cx), &p.icon, &p.type_key, &active);
            let name = if p.name.is_empty() {
                "Untitled".to_string()
            } else {
                p.name.clone()
            };
            let is_current = open_id == Some(id.as_str());
            let body = fold.shown().then(|| {
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
            let collapse_group = SharedString::from(format!("widget-collapse:{id}"));
            let card = div()
                .id(crate::eid("widget", &id))
                .relative()
                .when(drag_id.as_deref() == Some(&id), |d| d.opacity(0.4))
                .child(
                    GroupBox::new()
                        .id(crate::eid("widget-box", &id))
                        .content_style(
                            StyleRefinement::default()
                                .bg(theme::panel())
                                .border_1()
                                .border_color(theme::border())
                                .rounded(theme::radius::menu())
                                .p(px(8.))
                                .gap_0(),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .child(
                                    // The fold arrow: no fill, fg and full
                                    // opacity under the pointer.
                                    crate::fill_button(
                                        crate::eid("widget-collapse", &id),
                                        transparent_black(),
                                        cx,
                                    )
                                    .group(collapse_group.clone())
                                    .w(px(16.))
                                    .h(px(24.))
                                    .opacity(0.6)
                                    .group_hover(collapse_group, |s| s.opacity(1.))
                                    .accessibility_label("Fold")
                                    .child(
                                        workspace::arrow_at(fold.turn(), 7.)
                                            .text_color(theme::muted())
                                            .group_hover(format!("widget-collapse:{id}"), |s| {
                                                s.text_color(theme::fg())
                                            }),
                                    )
                                    .on_click(cx.listener({
                                        let id = id.clone();
                                        move |this, _, _, cx| this.toggle_pin_collapsed(&id, cx)
                                    })),
                                )
                                .child(
                                    crate::fill_button(
                                        crate::eid("widget-head", &id),
                                        theme::hl_med(),
                                        cx,
                                    )
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(28.))
                                    .pl(px(2.))
                                    .pr(px(8.))
                                    .rounded(theme::radius::sm())
                                    .text_color(theme::fg())
                                    .when(is_current, |d| d.bg(theme::hl_med()))
                                    .accessibility_label(name.clone())
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .justify_start()
                                            .gap(px(6.))
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .line_height(px(22.))
                                            .child(obj_icon(&icon))
                                            .child(div().min_w_0().truncate().child(name.clone())),
                                    )
                                    .on_click(open(id.clone())),
                                ),
                        )
                        .children(body.map(|view| fold.wrap(view))),
                )
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
                .on_drop(cx.listener({
                    let pins = pins.state.clone();
                    move |this, payload: &WidgetDrag, _, cx| {
                        let target = this.widget_over.take();
                        this.widget_drag = None;
                        cx.notify();
                        let (Some((target, bottom)), Some(space)) =
                            (target, Self::current_space(cx))
                        else {
                            return;
                        };
                        PinMotion::arm(&pins, cx);
                        roostr_core::pins::move_pin(&space, &payload.id, &target, bottom, cx)
                            .detach_and_log_err(cx);
                    }
                }));
            out.push(pins.card(&id, card, window, cx));
        }
        out
    }

    fn section(
        &self,
        key: &'static str,
        label: &'static str,
        add: Option<AnyElement>,
        rows: Vec<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = !self.section_collapsed.get(key).copied().unwrap_or(false);
        // A long section only fades: sliding it would lay out every row each frame.
        let fold = list_motion::fold(
            crate::eid("section-fold", key),
            open,
            Some(motion::CONTROL),
            rows.len() <= 30,
            window,
            cx,
        );
        // `.section-name` (`transition: color 0.15s`).
        let (hover, hovered) = motion::hover(
            crate::eid("section-hover", key),
            motion::CONTROL,
            window,
            cx,
        );
        let tint = motion::mix(theme::muted(), theme::fg(), hover);
        let body = GroupBox::new()
            .id(crate::eid("section-box", key))
            .content_style(
                StyleRefinement::default()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .rounded(theme::radius::menu())
                    .px(px(8.))
                    .py(px(4.))
                    .gap_0(),
            )
            .children(rows.into_iter().enumerate().map(|(i, row)| {
                div()
                    .when(i > 0, |d| d.border_t_1().border_color(theme::border()))
                    .child(row)
            }));
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
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .pt(px(2.))
                            .px(px(8.))
                            .pb(px(6.))
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(px(18.))
                            .text_color(tint)
                            .cursor_pointer()
                            .child(
                                div()
                                    .opacity(0.7)
                                    .child(workspace::arrow_at(fold.turn(), 7.).text_color(tint)),
                            )
                            .child(roostr_core::ui::spaced_text(label.to_uppercase(), 0.05))
                            .on_hover(move |over, _, cx| HoverMotion::set(&hovered, *over, cx))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.flip_section(key, cx)),
                            ),
                    )
                    .children(add),
            )
            .when(fold.shown(), |d| d.child(fold.wrap(body)))
    }

    /// `.section-add`: a 20px ＋, half faded until the pointer is over it.
    fn section_add(id: &'static str, tip: &'static str, cx: &App) -> Button {
        let group = SharedString::from(format!("section-add:{id}"));
        crate::fill_button(id, theme::hl_med(), cx)
            .group(group.clone())
            .relative()
            .size(px(20.))
            .mr(px(2.))
            .rounded(theme::radius::xs())
            .opacity(0.5)
            .group_hover(group, |s| s.opacity(1.))
            .child(Icon::new(Glyph::Plus).size(px(12.)))
            .accessibility_label(tip)
            .tip(tip)
    }

    fn render_recent(
        &mut self,
        current: &Space,
        open_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let active = Navigator::global(cx).read(cx).space().to_string();
        let rows: Vec<Row> = {
            let store = Store::global(cx).read(cx);
            store
                .summaries
                .iter()
                .filter(|s| {
                    !current.pinned_ids.contains(&s.id)
                        && !is_system_type(&s.type_key)
                        && s.channel_id == current.id
                })
                .take(8)
                .map(|r| {
                    let name = if r.name.is_empty() {
                        "Untitled".to_string()
                    } else {
                        r.name.clone()
                    };
                    let lead = if store.layout_of(&r.type_key) == "task" {
                        let id = r.id.clone();
                        ui::checkbox(crate::eid("row-check", &r.id), r.done == Some(true))
                            .with_size(Size::Medium)
                            .flex_none()
                            .mr(px(6.))
                            .on_change(move |done: &bool, _, cx| {
                                cx.stop_propagation();
                                set_done(&id, *done, cx);
                            })
                            .into_any_element()
                    } else {
                        obj_icon(&object_icon(store, &r.icon, &r.type_key, &active))
                            .into_any_element()
                    };
                    Row {
                        id: crate::eid("recent", &r.id),
                        outer: div(),
                        item: item(name, open_id == Some(r.id.as_str()))
                            .on_click(open(r.id.clone())),
                        lead,
                    }
                })
                .collect()
        };
        let rows = Row::render_all(rows, "Nothing yet", window, cx);
        self.section("recent", "Recently edited", None, rows, window, cx)
    }

    fn render_types(
        &mut self,
        open_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let space_id = Self::space_id(cx);
        let rows: Vec<Row> = {
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
                    let selector = format!("type:{}", t.id);
                    let lock = crate::eid("type-lock", &t.id);
                    Row {
                        id: crate::eid("type", &t.id),
                        outer: div()
                            .rounded(theme::radius::sm())
                            .when(current, |d| d.bg(theme::hover()))
                            .debug_selector(move || selector),
                        item: item(name, false)
                            .when(t.builtin, |d| {
                                d.suffix(move |_, _| builtin_mark(lock.clone()))
                            })
                            .on_click(open(t.id.clone())),
                        lead: obj_icon(&icon).into_any_element(),
                    }
                })
                .collect()
        };
        let rows = rows
            .into_iter()
            .map(|row| row.render(window, cx).into_any_element())
            .collect();
        let add = Self::section_add("new-type", "New type", cx)
            .on_click(cx.listener(|this, _, window, cx| this.new_type(window, cx)))
            .into_any_element();
        self.section("types", "Types", Some(add), rows, window, cx)
    }

    fn render_props(
        &mut self,
        open_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let space_id = Self::space_id(cx);
        let rows: Vec<Row> = {
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
                    let lock = crate::eid("prop-lock", &r.id);
                    Row {
                        id: crate::eid("prop", &r.id),
                        outer: div(),
                        item: item(name, open_id == Some(r.id.as_str()))
                            .when(store.builtin_ids.contains(&r.id), |d| {
                                d.suffix(move |_, _| builtin_mark(lock.clone()))
                            })
                            .on_click(open(r.id.clone())),
                        lead: obj_icon(&icon).into_any_element(),
                    }
                })
                .collect()
        };
        let rows = Row::render_all(rows, "No properties yet", window, cx);
        let add = Self::section_add("new-prop", "New property", cx)
            .child(self.anchor_probe("new-prop"))
            .on_click(cx.listener(|this, _, window, cx| this.open_new_property(window, cx)))
            .into_any_element();
        self.section("props", "Properties", Some(add), rows, window, cx)
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

/// `.none`: the empty-section note, as gpui-kit's [`Empty`] reduced to a
/// muted 12px line.
fn none(text: &'static str) -> AnyElement {
    Empty::new()
        .flex_none()
        .items_start()
        .gap_0()
        .p_0()
        .px(px(8.))
        .rounded(px(0.))
        .text_size(px(12.))
        .text_color(theme::muted())
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

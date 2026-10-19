//! The sidebar's New menu (`createMenuBody`, `+layout.svelte:896-905`, logic
//! 149-207, CSS 1796-1857): a type search over the space's Default type,
//! Query, Collection and the space's other types; ↑/↓ highlight, Enter creates.

use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::create::{creatable_types, default_type_key};
use roostr_core::objects::type_glyph;
use roostr_core::store::Store;
use roostr_core::{Navigator, theme, ui};

use crate::workspace::Workspace;

pub(crate) fn init(_: &mut App) {}

pub(crate) enum CreateMenuEvent {
    /// Closed (Escape) without creating.
    Close,
    /// Create an object of this kind ("query", "collection" or a type key).
    Create(String),
}

#[derive(Clone, PartialEq, Eq, Copy)]
enum Group {
    Default,
    Fixed,
    Type,
}

struct Choice {
    key: String,
    icon: String,
    name: String,
    group: Group,
}

pub(crate) struct CreateMenu {
    input: Entity<InputState>,
    filter: String,
    /// Highlighted choice (-1 = none: the first ↓ lands on the first).
    active: i32,
    default_key: String,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CreateMenuEvent> for CreateMenu {}

impl CreateMenu {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search types…"));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscriptions = vec![cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter = input.read(cx).value().to_string();
                this.active = -1;
                cx.notify();
            }
        })];
        let space = Workspace::space_id(cx);
        let fetch = default_type_key(&space, cx);
        cx.spawn(async move |this, cx| {
            let key = fetch.await;
            let _ = this.update(cx, |this, cx| {
                if Workspace::space_id(cx) == space {
                    this.default_key = key;
                    cx.notify();
                }
            });
        })
        .detach();
        Self {
            input,
            filter: String::new(),
            active: -1,
            default_key: "note".into(),
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn choices(&self, cx: &App) -> Vec<Choice> {
        let types = creatable_types(cx);
        let store = Store::global(cx).read(cx);
        let space = Navigator::global(cx).read(cx).space().to_string();
        let def = types.iter().position(|t| t.key == self.default_key);
        let mut all = Vec::new();
        if let Some(ix) = def {
            let t = &types[ix];
            all.push(Choice {
                key: t.key.clone(),
                icon: t.icon.clone(),
                name: t.name.clone(),
                group: Group::Default,
            });
        }
        all.push(Choice {
            key: "query".into(),
            icon: type_glyph(store, "query", &space),
            name: "Query".into(),
            group: Group::Fixed,
        });
        all.push(Choice {
            key: "collection".into(),
            icon: type_glyph(store, "collection", &space),
            name: "Collection".into(),
            group: Group::Fixed,
        });
        for (i, t) in types.iter().enumerate() {
            if Some(i) != def {
                all.push(Choice {
                    key: t.key.clone(),
                    icon: t.icon.clone(),
                    name: t.name.clone(),
                    group: Group::Type,
                });
            }
        }
        let q = self.filter.trim().to_lowercase();
        if q.is_empty() {
            return all;
        }
        all.into_iter()
            .filter(|c| c.name.to_lowercase().contains(&q) || c.key.to_lowercase().contains(&q))
            .collect()
    }

    fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.choices(cx).len() as i32;
        if n == 0 {
            return;
        }
        self.active = if down {
            (self.active + 1) % n
        } else if self.active <= 0 {
            n - 1
        } else {
            self.active - 1
        };
        cx.notify();
    }

    fn create_active(&mut self, cx: &mut Context<Self>) {
        let choices = self.choices(cx);
        if let Some(c) = choices.get(self.active.max(0) as usize) {
            cx.emit(CreateMenuEvent::Create(c.key.clone()));
        }
    }
}

impl Render for CreateMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `.m-create-menu button`: bigger touch rows on a phone.
        let mobile = roostr_core::layout::is_mobile(window);
        let choices = self.choices(cx);
        let mut list = div().flex().flex_col();
        let mut child_ix = 0;
        let mut active_child = None;
        for (i, c) in choices.iter().enumerate() {
            if i > 0 && c.group != choices[i - 1].group {
                list = list.child(ui::menu_separator());
                child_ix += 1;
            }
            if i as i32 == self.active {
                active_child = Some(child_ix);
            }
            let key = c.key.clone();
            list = list.child(
                div()
                    .id(("create-choice", i))
                    .flex()
                    .flex_none()
                    .items_center()
                    .map(|d| {
                        if mobile {
                            d.gap(px(10.))
                                .px(px(10.))
                                .py(px(11.))
                                .rounded(theme::radius::md())
                                .text_size(px(15.))
                        } else {
                            d.gap(px(8.))
                                .px(px(8.))
                                .py(px(6.))
                                .rounded(theme::radius::sm())
                                .text_size(px(13.))
                        }
                    })
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(i as i32 == self.active, |d| d.bg(theme::hover()))
                    .hover(|s| s.bg(theme::hover()))
                    .child(crate::sidebar::obj_icon(&c.icon))
                    .child(c.name.clone())
                    .when(c.group == Group::Default, |d| {
                        d.child(
                            div()
                                .ml_auto()
                                .pl(px(12.))
                                .text_size(px(11.))
                                .text_color(theme::muted())
                                .child("Default"),
                        )
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.active != i as i32 {
                            this.active = i as i32;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(CreateMenuEvent::Create(key.clone()))
                    })),
            );
            child_ix += 1;
        }
        if choices.is_empty() {
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(12.5))
                    .text_color(theme::muted())
                    .child("No matching type"),
            );
        }
        if let Some(ix) = active_child {
            self.scroll.scroll_to_item(ix);
        }
        ui::menu_panel()
            .id("create-menu")
            .line_height(relative(1.2))
            .min_w(px(184.))
            // `.m-create-menu`: the docked sheet's width, radius 14, no shadow.
            .when(mobile, |d| {
                d.w_full()
                    .rounded(theme::radius::modal())
                    .shadow(Vec::new())
            })
            .max_h_full()
            .occlude()
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                this.step(true, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                this.step(false, cx);
            }))
            .capture_action(cx.listener(|this, _: &Enter, _, cx| {
                cx.stop_propagation();
                this.create_active(cx);
            }))
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(CreateMenuEvent::Close);
            }))
            .child(type_search(&self.input).mx_0().mt_0().mb(px(4.)))
            .child(
                div()
                    .id("create-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(list),
            )
    }
}

/// `.type-search`: margin 4×6, padding 5×8, 13px, bg `--bg`, 1px border, radius 6.
pub(crate) fn type_search(input: &Entity<InputState>) -> Div {
    div()
        .flex_none()
        .mx(px(6.))
        .my(px(4.))
        .px(px(8.))
        .py(px(5.))
        .bg(theme::bg())
        .border_1()
        .border_color(theme::border())
        .rounded(theme::radius::sm())
        .text_size(px(13.))
        .child(
            Input::new(input)
                .appearance(false)
                .text_size(px(13.))
                .h(px(16.))
                .px_0()
                .py_0(),
        )
}

impl Workspace {
    /// `sidebarCreate`: close the menu, then create in the active space.
    pub(crate) fn sidebar_create(
        &mut self,
        kind: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_menu = None;
        cx.notify();
        let ch = Self::space_id(cx);
        let task = match kind.as_str() {
            "collection" => roostr_core::create::create_collection(&ch, cx),
            "query" => roostr_core::create::create_query(&ch, cx),
            _ => roostr_core::create::create_typed(&kind, &ch, "", cx),
        };
        cx.spawn_in(window, async move |this, cx| {
            if let Err(error) = task.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.show_alert(error.to_string(), window, cx)
                });
            }
        })
        .detach();
    }
}

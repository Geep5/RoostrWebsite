//! The sidebar's New menu (`createMenuBody`, `+layout.svelte:896-905`, logic
//! 149-207, CSS 1796-1857): a type search over the space's Default type,
//! Query, Collection and the space's other types; ↑/↓ highlight, Enter creates.
//! The choices are a gpui-kit [`Command`] list (one group per kind of
//! choice, separated) under the menu's own type search.

use gpui_kit::component::IndexPath;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::empty::Empty;
use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::tag::Tag;
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
    /// The choice list (rows, hover, click, scrolling to the highlight).
    list: Entity<CommandState>,
    /// Each choice's path in the list, in choice order.
    paths: Vec<IndexPath>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CreateMenuEvent> for CreateMenu {}

impl CreateMenu {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search types…"));
        input.update(cx, |input, cx| input.focus(window, cx));
        // Nothing highlighted until ↓ or a hover.
        let list = cx.new(|cx| {
            let mut list = CommandState::new(window, cx);
            list.set_selected_index(None, window, cx);
            list
        });
        let subscriptions = vec![cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.filter = input.read(cx).value().to_string();
                    this.active = -1;
                    this.list
                        .update(cx, |list, cx| list.set_selected_index(None, window, cx));
                    cx.notify();
                }
            },
        )];
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
            list,
            paths: Vec::new(),
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

    fn step(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        // The list highlights it and scrolls it into view.
        let path = self.paths.get(self.active as usize).copied();
        self.list
            .update(cx, |list, cx| list.set_selected_index(path, window, cx));
        cx.notify();
    }

    /// The list's highlight moved (a hover): follow it.
    fn follow(&mut self, path: IndexPath, cx: &mut Context<Self>) {
        if let Some(ix) = self.paths.iter().position(|p| *p == path) {
            self.active = ix as i32;
            cx.notify();
        }
    }

    /// A choice clicked in the list.
    fn pick(&mut self, path: IndexPath, cx: &mut Context<Self>) {
        if let Some(ix) = self.paths.iter().position(|p| *p == path) {
            self.active = ix as i32;
            self.create_active(cx);
        }
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
        let this = cx.entity().downgrade();
        let mut list = Command::new(&self.list)
            .searchable(false)
            .bordered(false)
            .max_h(px(1_000_000.))
            .flex_1()
            .min_h_0()
            // Out past the list's own 4px inset: rows sit on the menu's padding.
            .mx(px(-4.))
            .my(px(-4.))
            .bg(transparent_black())
            .text_color(theme::fg())
            // Rows round at `radius::sm` (the kit's 6); on a phone at
            // 12 − the 4px inset = `radius::md`.
            .when(mobile, |d| d.rounded(px(12.)))
            .on_select({
                let this = this.clone();
                move |path, _, cx| {
                    let _ = this.update(cx, |this, cx| this.follow(path, cx));
                }
            })
            .on_confirm(move |path, _, cx| {
                let _ = this.update(cx, |this, cx| this.pick(path, cx));
            })
            .empty(|_, _, _| {
                Empty::new()
                    .flex_none()
                    .items_start()
                    .gap_0()
                    .p_0()
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(0.))
                    .text_size(px(12.5))
                    .text_color(theme::muted())
                    .child("No matching type")
            });
        let mut paths = Vec::with_capacity(choices.len());
        let mut section = 0;
        let mut group = CommandGroup::new();
        let mut row = 0;
        for (i, c) in choices.iter().enumerate() {
            if i > 0 && c.group != choices[i - 1].group {
                list = list.group(group).separator();
                group = CommandGroup::new();
                section += 1;
                row = 0;
            }
            paths.push(IndexPath::new(row).section(section));
            row += 1;
            let (icon, name, default) = (c.icon.clone(), c.name.clone(), c.group == Group::Default);
            group = group.item(CommandItem::new().label(c.name.clone()).child(move |_, _| {
                // The kit row pads 8×6; a phone's rows are 10×11.
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .map(|d| {
                        if mobile {
                            d.gap(px(10.)).px(px(2.)).py(px(5.)).text_size(px(15.))
                        } else {
                            d.gap(px(8.)).text_size(px(13.))
                        }
                    })
                    .child(crate::sidebar::obj_icon(&icon))
                    .child(name.clone())
                    .when(default, |d| {
                        d.child(
                            Tag::custom(transparent_black(), theme::muted(), transparent_black())
                                .border_0()
                                .ml_auto()
                                .pl(px(12.))
                                .pr_0()
                                .py_0()
                                .text_size(px(11.))
                                .child("Default"),
                        )
                    })
            }));
        }
        if !choices.is_empty() {
            list = list.group(group);
        }
        self.paths = paths;
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
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                cx.stop_propagation();
                this.step(true, window, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                cx.stop_propagation();
                this.step(false, window, cx);
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
            .child(list)
    }
}

/// `.type-search`: the kit input at margin 4×6, padding 5×8, 13px, bg
/// `--bg`, 1px border, radius 6 (h28).
pub(crate) fn type_search(input: &Entity<InputState>) -> Input {
    Input::new(input)
        .flex_none()
        .w_auto()
        .h(px(28.))
        .mx(px(6.))
        .my(px(4.))
        .px(px(8.))
        .py(px(5.))
        .bg(theme::bg())
        .border_color(theme::border())
        .rounded(theme::radius::sm())
        .text_size(px(13.))
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

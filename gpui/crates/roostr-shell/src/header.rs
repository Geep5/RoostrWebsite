//! The page header (`.main-col > header`, `+layout.svelte:1524-1612`, CSS
//! 2749-2801): back/forward, the path button (opens search), the graph link,
//! the More menu with every item and submenu, the pane button with its badge,
//! and the delete-property modal (1623-1638).

use gpui_kit::component::IconNamed as _;
use gpui_kit::component::input::{Enter, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::nav::{Navigator, Route, navigate, open_tab};
use roostr_core::objects::{is_image_icon, object_icon, type_glyph};
use roostr_core::store::Store;
use roostr_core::types::{Block, Fields, RelationDef, Value, pos};
use roostr_core::{Glyph, PaneState, TypeDef, api, theme, ui};
use serde_json::json;

use crate::modal::{dialog_appear, outline_button, overlay, small_card};
use crate::workspace::Workspace;

pub(crate) fn init(_: &mut App) {}

/// The open More menu's state.
pub(crate) struct MoreMenu {
    show_collections: bool,
    /// The Change type submenu's search, while open.
    type_pick: Option<Entity<InputState>>,
    type_filter: String,
    _subscription: Option<Subscription>,
}

/// The delete-property confirmation.
pub(crate) struct DeleteProp {
    rel: RelationDef,
    /// Objects holding a value (`None` while counting).
    usage: Option<usize>,
    deleting: bool,
}

/// `.hbtn`: 28×28, radius 7, muted, hover `--hover`.
fn hbtn(id: impl Into<ElementId>) -> Stateful<Div> {
    hbtn_base(id).hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
}

/// `.hbtn` without its hover, for buttons whose hover adds to it (GPUI takes
/// one hover style per element).
fn hbtn_base(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::radius::tab())
        .text_color(theme::muted())
        .text_size(px(16.))
        .cursor_pointer()
}

fn glyph(glyph: Glyph, size: f32) -> Svg {
    svg()
        .path(glyph.path())
        .size(px(size))
        .flex_none()
        .text_color(theme::muted())
}

/// A More-menu row: gap 8, padding 7×10, radius 6, 13px; hover accent + white
/// (danger: red text, hover red + white).
fn more_item(
    id: impl Into<ElementId>,
    icon: Option<Glyph>,
    label: impl Into<SharedString>,
    danger: bool,
) -> Stateful<Div> {
    let color = if danger { theme::red() } else { theme::fg() };
    more_row(id, danger)
        .when_some(icon, |d, icon| {
            d.child(
                svg()
                    .path(icon.path())
                    .size(px(16.))
                    .flex_none()
                    .text_color(color),
            )
        })
        .child(label.into())
}

/// A More-menu row's frame, before its icon and label.
fn more_row(id: impl Into<ElementId>, danger: bool) -> Stateful<Div> {
    let color = if danger { theme::red() } else { theme::fg() };
    let hover_bg = if danger {
        theme::red()
    } else {
        theme::accent()
    };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .py(px(7.))
        .rounded(theme::radius::sm())
        .text_size(px(13.))
        .text_color(color)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg).text_color(theme::white()))
}

/// `.menu-sep`.
fn menu_sep() -> Div {
    div()
        .h(px(1.))
        .mx(px(6.))
        .my(px(4.))
        .flex_none()
        .bg(theme::border())
}

fn chev() -> Div {
    div().ml_auto().flex().opacity(0.6).child(
        svg()
            .path(Glyph::ChevronRight.path())
            .size(px(14.))
            .text_color(theme::fg()),
    )
}

/// `.menu-none`: muted 12px note with an optional leading icon.
fn menu_none(id: impl Into<ElementId>, lock: bool, text: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .py(px(6.))
        .text_size(px(12.))
        .text_color(theme::muted())
        .when(lock, |d| d.child(glyph(Glyph::Lock, 16.)))
        .child(text.into())
}

fn submenu() -> Div {
    div()
        .flex()
        .flex_col()
        .border_l_2()
        .border_color(theme::border())
        .ml(px(12.))
}

fn refresh(cx: &mut App) {
    Store::global(cx).update(cx, |s, cx| s.refresh_all(cx));
}

impl Workspace {
    pub(crate) fn render_header(
        &mut self,
        wide: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let object_id = Self::open_object_id(cx);
        let route = Navigator::global(cx).read(cx).route().clone();
        let (path_icon, path_name, graph_path) = self.header_path(&route, cx);
        let (has_more, pane_closed, count) = {
            let store = Store::global(cx).read(cx);
            let has_more = object_id.as_ref().is_some_and(|id| {
                store.relations.iter().any(|r| r.id == *id)
                    || store.summary(id).is_some()
                    || store.space(id).is_some()
            });
            let pane = PaneState::global(cx).read(cx);
            // The pane button opens Chat (Properties without one) while it is collapsed.
            let primary = pane.primary();
            let closed = (pane.available && !pane.is_open(primary)).then_some(primary);
            (has_more, closed, pane.count)
        };
        let path = div()
            .id("path")
            .flex()
            .items_center()
            .gap(px(8.))
            .max_w(relative(0.6))
            .min_w_0()
            .px(px(10.))
            .py(px(4.))
            .rounded(theme::radius::md())
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::fg())
            .cursor_pointer()
            .hover(|s| s.bg(theme::hover()))
            .map(|d| {
                if graph_path {
                    d.child(
                        svg()
                            .path(Glyph::Graph.path())
                            .size(px(15.))
                            .flex_none()
                            .text_color(theme::fg()),
                    )
                } else if is_image_icon(&path_icon) {
                    d.child(ui::emoji_or_image_icon(&path_icon, px(18.)))
                } else {
                    d.child(
                        div()
                            .flex_none()
                            .text_size(px(15.))
                            .child(path_icon.clone()),
                    )
                }
            })
            .child(div().min_w_0().truncate().child(path_name))
            .map(|d| self.data_tip(d, "tip-path", "Search (⌘K)", cx))
            .on_click(cx.listener(|this, _, window, cx| this.open_search(window, cx)));

        let graph_focus = object_id.clone();
        let right = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .child(
                hbtn("graph-link")
                    .child(glyph(Glyph::Graph, 16.))
                    .map(|d| self.data_tip(d, "tip-graph", "Graph", cx))
                    .on_click(move |_, _, cx| {
                        navigate(
                            Route::Graph {
                                focus: graph_focus.clone(),
                            },
                            cx,
                        )
                    }),
            )
            .when(has_more, |d| d.child(self.render_more(window, cx)))
            .when_some(pane_closed, |d, primary| {
                let badge = if count > 99 {
                    "99+".to_string()
                } else {
                    count.to_string()
                };
                d.child(
                    hbtn_base("pane-open")
                        .size(px(30.))
                        .border_1()
                        .border_color(theme::border())
                        .text_size(px(14.))
                        .hover(|s| {
                            s.bg(theme::hover())
                                .border_color(theme::muted())
                                .text_color(theme::fg())
                        })
                        .child(glyph(Glyph::Pane, 16.))
                        .when(count > 0, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .top(px(-4.))
                                    .right(px(-4.))
                                    .min_w(px(16.))
                                    .h(px(16.))
                                    .px(px(4.))
                                    .rounded(px(8.))
                                    .bg(theme::accent())
                                    .text_color(theme::white())
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .line_height(px(16.))
                                    .flex()
                                    .justify_center()
                                    .child(badge),
                            )
                        })
                        .map(|d| self.data_tip(d, "tip-pane", "Chat & properties", cx))
                        .on_click(move |_, _, cx| {
                            PaneState::global(cx).update(cx, |p, cx| p.set_open(primary, true, cx))
                        }),
                )
            });

        div()
            .w_full()
            .mx_auto()
            .when(!wide, |d| d.max_w(px(952.)))
            .flex()
            .items_center()
            .gap(px(8.))
            .pt(px(18.))
            .px(px(16.))
            .pb(px(8.))
            .border_b_1()
            .line_height(relative(1.2))
            .border_color(theme::border())
            .bg(theme::bg())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        hbtn("back")
                            .child(glyph(Glyph::NavBack, 16.))
                            .map(|d| self.data_tip(d, "tip-back", "Back", cx))
                            .on_click(|_, _, cx| {
                                Navigator::global(cx).update(cx, |n, cx| n.back(cx))
                            }),
                    )
                    .child(
                        hbtn("forward")
                            .child(glyph(Glyph::NavForward, 16.))
                            .map(|d| self.data_tip(d, "tip-forward", "Forward", cx))
                            .on_click(|_, _, cx| {
                                Navigator::global(cx).update(cx, |n, cx| n.forward(cx))
                            }),
                    ),
            )
            .child(div().flex_1().min_w_0().flex().justify_center().child(path))
            .child(right)
    }

    /// `headerPath` of `route`: (icon, name, is the graph).
    pub(crate) fn header_path(&self, route: &Route, cx: &App) -> (String, String, bool) {
        let store = Store::global(cx).read(cx);
        let active = Navigator::global(cx).read(cx).space().to_string();
        let current = Self::current_space(cx);
        match route {
            Route::Object(id) => {
                if let Some(s) = store.summary(id) {
                    let name = if s.name.is_empty() {
                        "Untitled".into()
                    } else {
                        s.name.clone()
                    };
                    (
                        object_icon(store, &s.icon, &s.type_key, &active),
                        name,
                        false,
                    )
                } else if let Some(c) = store.space(id) {
                    (
                        if c.icon.is_empty() {
                            "◍".into()
                        } else {
                            c.icon.clone()
                        },
                        c.name.clone(),
                        false,
                    )
                } else {
                    ("▨".into(), "…".into(), false)
                }
            }
            Route::Graph { .. } => (
                String::new(),
                format!("Graph — {}", current.map(|c| c.name).unwrap_or_default()),
                true,
            ),
            _ => match current {
                Some(c) => (
                    if c.icon.is_empty() {
                        "◍".into()
                    } else {
                        c.icon
                    },
                    c.name,
                    false,
                ),
                None => ("◍".into(), "glon".into(), false),
            },
        }
    }

    pub(crate) fn toggle_more(&mut self, cx: &mut Context<Self>) {
        self.more = if self.more.is_some() {
            None
        } else {
            Some(MoreMenu {
                show_collections: false,
                type_pick: None,
                type_filter: String::new(),
                _subscription: None,
            })
        };
        cx.notify();
    }

    fn render_more(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Div {
        div().relative().child(
            hbtn("more")
                .child("⋯")
                .child(self.anchor_probe("more"))
                .map(|d| self.data_tip(d, "tip-more", "More", cx))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_more(cx))),
        )
    }

    /// The mobile More menu's first row: the graph around the object.
    fn more_graph(&self, id: &str, cx: &Context<Self>) -> Stateful<Div> {
        let focus = id.to_string();
        more_row("more-graph", false)
            .debug_selector(|| "more-graph".into())
            .child(
                svg()
                    .path(Glyph::Graph.path())
                    .size(px(14.))
                    .mr(px(2.))
                    .flex_none()
                    .text_color(theme::fg()),
            )
            .child("Graph")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.more = None;
                cx.notify();
                navigate(
                    Route::Graph {
                        focus: Some(focus.clone()),
                    },
                    cx,
                );
            }))
    }

    pub(crate) fn render_more_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(id) = Self::open_object_id(cx) else {
            return div().id("more-menu");
        };
        // A phone's menu leads with Graph (its header has no graph link)
        // instead of Open in new tab (it has no tabs).
        let mobile = roostr_core::layout::is_mobile(window);
        let blocker = if self.delete_blocker.0 == id {
            self.delete_blocker.1.clone()
        } else {
            String::new()
        };
        let store = Store::global(cx).read(cx);
        let relation = store.relations.iter().find(|r| r.id == id).cloned();
        let summary = store.summary(&id).cloned();
        let space = if summary.is_none() {
            store.space(&id).cloned()
        } else {
            None
        };
        let panel = ui::menu_panel()
            .id("more-menu")
            .line_height(relative(1.2))
            .min_w(px(234.))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let blocked_line = |blocker: &str| {
            menu_none("more-blocked", true, format!("Can't delete: {blocker}")).tooltip(crate::tip(
                "Types and properties Roostr or your queries rely on can't be deleted",
            ))
        };

        if let Some(rel) = relation {
            let panel = if mobile {
                panel.child(self.more_graph(&id, cx)).child(menu_sep())
            } else {
                panel
            };
            return panel.child(if blocker.is_empty() {
                more_item("delete-prop", Some(Glyph::Trash), "Delete property", true)
                    .on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.open_delete_property(rel.clone(), cx)
                        }),
                    )
                    .into_any_element()
            } else {
                blocked_line(&blocker).into_any_element()
            });
        }

        if let Some(summary) = summary {
            let active = Navigator::global(cx).read(cx).space().to_string();
            let owning = roostr_core::pins::space_of(store, &summary.channel_id);
            let pinned = owning
                .as_ref()
                .is_some_and(|s| s.pinned_ids.contains(&summary.id));
            let collections_space = if !summary.channel_id.is_empty() {
                summary.channel_id.clone()
            } else if !active.is_empty() {
                active.clone()
            } else {
                store
                    .channels
                    .first()
                    .map(|c| c.id.clone())
                    .unwrap_or_default()
            };
            let collections: Vec<(String, String, String)> = store
                .summaries
                .iter()
                .filter(|s| {
                    s.type_key == "collection" && s.id != id && s.channel_id == collections_space
                })
                .map(|s| {
                    let name = if s.name.is_empty() {
                        "Untitled".to_string()
                    } else {
                        s.name.clone()
                    };
                    (
                        s.id.clone(),
                        object_icon(store, &s.icon, &s.type_key, &active),
                        name,
                    )
                })
                .collect();
            let can_retype = !matches!(summary.type_key.as_str(), "query" | "collection" | "set");
            let space_id = Self::space_id(cx);
            let filter = self
                .more
                .as_ref()
                .map(|m| m.type_filter.trim().to_lowercase())
                .unwrap_or_default();
            let type_choices: Vec<TypeDef> = store
                .types
                .iter()
                .filter(|t| {
                    t.space == space_id
                        && t.key != summary.type_key
                        && (filter.is_empty()
                            || if t.name.is_empty() { &t.key } else { &t.name }
                                .to_lowercase()
                                .contains(&filter))
                })
                .cloned()
                .collect();
            let type_icons: Vec<String> = type_choices
                .iter()
                .map(|t| {
                    if t.icon.is_empty() {
                        type_glyph(store, &t.key, &active)
                    } else {
                        t.icon.clone()
                    }
                })
                .collect();
            let show_collections = self.more.as_ref().is_some_and(|m| m.show_collections);
            let type_pick = self.more.as_ref().and_then(|m| m.type_pick.clone());
            let raw_filter = self
                .more
                .as_ref()
                .map(|m| m.type_filter.trim().to_string())
                .unwrap_or_default();

            let open_id = id.clone();
            let pin_id = id.clone();
            let mut panel = panel
                .child(if mobile {
                    self.more_graph(&id, cx)
                } else {
                    more_item(
                        "open-tab",
                        Some(Glyph::ExternalLink),
                        "Open in new tab",
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.more = None;
                        open_tab(Route::Object(open_id.clone()), true, cx);
                    }))
                })
                .child(
                    more_item(
                        "pin",
                        Some(if pinned { Glyph::StarOff } else { Glyph::Star }),
                        if pinned {
                            "Unpin from space"
                        } else {
                            "Pin to space"
                        },
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.more = None;
                        if let Some(space) = &owning {
                            roostr_core::pins::set_pinned(&pin_id, space, !pinned, cx)
                                .detach_and_log_err(cx);
                        }
                        cx.notify();
                    })),
                )
                .child(
                    more_item(
                        "add-collection",
                        Some(Glyph::FolderPlus),
                        "Add to collection",
                        false,
                    )
                    .child(chev())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(more) = &mut this.more {
                            more.show_collections = !more.show_collections;
                        }
                        cx.notify();
                    })),
                )
                .when(show_collections, |d| {
                    let mut sub = submenu();
                    if collections.is_empty() {
                        sub = sub.child(menu_none(
                            "no-collections",
                            false,
                            "No collections in this space",
                        ));
                    }
                    for (cid, icon, name) in collections {
                        let target = id.clone();
                        sub = sub.child(
                            more_item(
                                crate::eid("collection", &cid),
                                None,
                                format!("{icon} {name}"),
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.more = None;
                                    cx.notify();
                                    add_to_collection(&target, &cid, cx);
                                },
                            )),
                        );
                    }
                    d.child(sub)
                })
                .when(can_retype, |d| {
                    d.child(
                        more_item(
                            "change-type",
                            Some(Glyph::ArrowLeftRight),
                            "Change type",
                            false,
                        )
                        .child(chev())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_type_pick(window, cx)),
                        ),
                    )
                })
                .when_some(type_pick, |d, input| {
                    let first = type_choices.first().map(|t| t.key.clone());
                    let target = id.clone();
                    let mut sub = submenu()
                        .id("type-pick")
                        .capture_action(cx.listener(move |this, _: &Enter, _, cx| {
                            cx.stop_propagation();
                            if let Some(key) = &first {
                                this.more = None;
                                cx.notify();
                                change_type(&target, key, cx);
                            }
                        }))
                        .child(crate::create_menu::type_search(&input));
                    for (t, icon) in type_choices.iter().zip(type_icons) {
                        let key = t.key.clone();
                        let target = id.clone();
                        let name = if t.name.is_empty() {
                            t.key.clone()
                        } else {
                            t.name.clone()
                        };
                        sub = sub.child(
                            more_item(
                                crate::eid("type-choice", &t.id),
                                None,
                                format!("{icon} {name}"),
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.more = None;
                                    cx.notify();
                                    change_type(&target, &key, cx);
                                },
                            )),
                        );
                    }
                    if type_choices.is_empty() {
                        sub = sub.child(menu_none(
                            "no-type",
                            false,
                            format!("No type matches “{raw_filter}”"),
                        ));
                    }
                    d.child(sub)
                })
                .child(
                    more_item("duplicate", Some(Glyph::Copy), "Duplicate", false).on_click(
                        cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                this.more = None;
                                cx.notify();
                                duplicate_object(&id, cx);
                            }
                        }),
                    ),
                );
            if store.space(&summary.channel_id).is_some() {
                let copy_id = id.clone();
                panel = panel.child(
                    more_item("copy-id", Some(Glyph::Hash), "Copy ID", false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.more = None;
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_id.clone()));
                            cx.notify();
                        }),
                    ),
                );
            }
            return panel.child(menu_sep()).child(if blocker.is_empty() {
                let bin_id = id.clone();
                more_item("bin", Some(Glyph::Trash), "Move to bin", true)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.more = None;
                        cx.notify();
                        move_to_bin(&bin_id, cx);
                    }))
                    .into_any_element()
            } else {
                blocked_line(&blocker).into_any_element()
            });
        }

        if space.is_some() {
            let open_id = id.clone();
            return panel
                .child(if mobile {
                    self.more_graph(&id, cx)
                } else {
                    more_item(
                        "open-tab",
                        Some(Glyph::ExternalLink),
                        "Open in new tab",
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.more = None;
                        open_tab(Route::Object(open_id.clone()), true, cx);
                    }))
                })
                .child(
                    more_item("copy-id", Some(Glyph::Hash), "Copy ID", false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.more = None;
                            cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                            cx.notify();
                        }),
                    ),
                );
        }
        panel
    }

    fn toggle_type_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(more) = &mut self.more else { return };
        more.type_filter.clear();
        if more.type_pick.take().is_some() {
            more._subscription = None;
            cx.notify();
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search types..."));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(more) = &mut this.more
            {
                more.type_filter = input.read(cx).value().to_string();
                cx.notify();
            }
        });
        more.type_pick = Some(input);
        more._subscription = Some(subscription);
        cx.notify();
    }

    fn open_delete_property(&mut self, rel: RelationDef, cx: &mut Context<Self>) {
        self.more = None;
        self.delete_prop = Some(DeleteProp {
            rel: rel.clone(),
            usage: None,
            deleting: false,
        });
        let query = api::query(
            json!({ "filters": [{ "key": rel.key, "condition": "exists" }], "limit": 1000 }),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let Ok(res) = query.await else { return };
            let usage = res
                .records
                .iter()
                .filter(|r| r.id != rel.id && r.type_key != "relation" && r.type_key != "type")
                .count();
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = &mut this.delete_prop
                    && d.rel.id == rel.id
                {
                    d.usage = Some(usage);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Strip the value from every object holding it (paging until none),
    /// then vanish the relation.
    fn delete_property(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &mut self.delete_prop else {
            return;
        };
        if d.deleting {
            return;
        }
        d.deleting = true;
        let rel = d.rel.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result: anyhow::Result<()> = async {
                loop {
                    let query = cx.update(|cx| {
                        api::query(json!({ "filters": [{ "key": rel.key, "condition": "exists" }], "limit": 1000 }), cx)
                    });
                    let res = query.await?;
                    let victims: Vec<String> = res
                        .records
                        .into_iter()
                        .filter(|r| r.id != rel.id && r.type_key != "relation" && r.type_key != "type")
                        .map(|r| r.id)
                        .collect();
                    if victims.is_empty() {
                        break;
                    }
                    for victim in victims {
                        cx.update(|cx| api::note::delete_field(&victim, &rel.key, cx)).await?;
                    }
                }
                cx.update(|cx| api::note::vanish(std::slice::from_ref(&rel.id), cx)).await?;
                Ok(())
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = &mut this.delete_prop {
                    d.deleting = false;
                }
                if result.is_ok() {
                    this.delete_prop = None;
                    refresh(cx);
                    navigate(Route::Home, cx);
                } else if let Err(error) = result {
                    log::warn!("delete property failed: {error:#}");
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_delete_prop(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(d) = &self.delete_prop else {
            return div().into_any_element();
        };
        let name = if d.rel.name.is_empty() {
            d.rel.key.clone()
        } else {
            d.rel.name.clone()
        };
        let usage = match d.usage {
            None => "…".to_string(),
            Some(n) => format!("{n} object{}", if n == 1 { "" } else { "s" }),
        };
        let deleting = d.deleting;
        let enter = dialog_appear("prop-del-appear", window, cx);
        deferred(
            overlay("prop-del-overlay", enter.target().alpha())
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if !this.delete_prop.as_ref().is_some_and(|d| d.deleting) {
                            this.delete_prop = None;
                            cx.notify();
                        }
                    }),
                )
                .child(
                    enter
                        .apply(small_card(window))
                        .id("prop-del-modal")
                        .gap_0()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .mb(px(8.))
                                .text_size(px(16.))
                                .font_weight(FontWeight::BOLD)
                                .child(format!("Delete \"{name}\"?")),
                        )
                        .child(
                            div().mb(px(14.)).text_size(px(13.)).line_height(relative(1.5)).text_color(theme::muted()).child(
                                format!(
                                    "This deletes the property and removes its value from {usage} — on every device, forever. This cannot be undone."
                                ),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.))
                                .child(
                                    ui::disabled(outline_button("prop-del-cancel", "Cancel", false), deleting).when(
                                        !deleting,
                                        |b| {
                                            b.on_click(cx.listener(|this, _, _, cx| {
                                                this.delete_prop = None;
                                                cx.notify();
                                            }))
                                        },
                                    ),
                                )
                                .child(
                                    ui::disabled(
                                        outline_button(
                                            "prop-del-ok",
                                            if deleting { "Deleting…" } else { "Delete property" },
                                            !deleting,
                                        )
                                        .when(deleting, |b| b.border_color(theme::red()).text_color(theme::red())),
                                        deleting,
                                    )
                                    .when(!deleting, |b| {
                                        b.on_click(cx.listener(|this, _, _, cx| this.delete_property(cx)))
                                    }),
                                ),
                        ),
                ),
        )
        .with_priority(10)
        .into_any_element()
    }
}

/// `addToCollection`: append the object to the collection's members.
fn add_to_collection(object_id: &str, collection_id: &str, cx: &mut App) {
    let fetch = Store::global(cx).read(cx).fetch_object(collection_id, cx);
    let (object_id, collection_id) = (object_id.to_string(), collection_id.to_string());
    cx.spawn(async move |cx| {
        let Ok(col) = fetch.await else { return };
        let ids: Vec<String> = col
            .field("collectionIds")
            .map(|v| {
                v.items()
                    .iter()
                    .filter_map(|i| i.string_value.clone())
                    .collect()
            })
            .unwrap_or_default();
        if !ids.contains(&object_id) {
            let items = ids
                .iter()
                .chain(std::iter::once(&object_id))
                .map(|id| Value::string(id.clone()))
                .collect();
            let task = cx.update(|cx| {
                api::note::set_field(&collection_id, "collectionIds", Value::values(items), cx)
            });
            let _ = task.await;
        }
        cx.update(refresh);
    })
    .detach();
}

fn change_type(object_id: &str, type_key: &str, cx: &mut App) {
    let task = api::note::set_type(object_id, type_key, cx);
    cx.spawn(async move |cx| {
        let _ = task.await;
        cx.update(refresh);
    })
    .detach();
}

fn move_to_bin(object_id: &str, cx: &mut App) {
    let task = api::note::delete(object_id, cx);
    cx.spawn(async move |cx| {
        if task.await.is_ok() {
            cx.update(|cx| {
                refresh(cx);
                navigate(Route::Home, cx);
            });
        }
    })
    .detach();
}

/// `duplicateObject`: same fields (name + " copy"), the whole block tree in
/// DFS order (roots appended, children INNER-appended), then open it.
fn duplicate_object(object_id: &str, cx: &mut App) {
    let fetch = Store::global(cx).read(cx).fetch_object(object_id, cx);
    cx.spawn(async move |cx| {
        let Ok(src) = fetch.await else { return };
        let base = if src.name().is_empty() {
            "Untitled"
        } else {
            src.name()
        };
        let name = format!("{base} copy");
        let mut fields: Fields = src.fields.clone();
        fields.insert("name".into(), Value::string(name.clone()));
        let type_key = src.type_key.clone();
        let task = cx.update(|cx| api::note::create(&name, Some(&type_key), Some(fields), cx));
        let Ok(id) = task.await else { return };
        let mut parent_of = std::collections::HashMap::new();
        for b in &src.blocks {
            for c in &b.children_ids {
                parent_of.insert(c.clone(), b.id.clone());
            }
        }
        for b in &src.blocks {
            let parent = parent_of.get(&b.id).cloned();
            let block = Block {
                children_ids: Vec::new(),
                ..b.clone()
            };
            let position = if parent.is_some() {
                pos::INNER
            } else {
                pos::NONE
            };
            let task = cx.update(|cx| {
                api::note::block_add(&id, &block, parent.as_deref().unwrap_or(""), position, cx)
            });
            if task.await.is_err() {
                return;
            }
        }
        cx.update(|cx| navigate(Route::Object(id), cx));
    })
    .detach();
}

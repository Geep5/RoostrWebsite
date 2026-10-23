//! The page header (`.main-col > header`, `+layout.svelte:1524-1612`, CSS
//! 2749-2801): back/forward, the path button (opens search), the graph link,
//! the More menu with every item and submenu, the pane button with its badge,
//! and the delete-property modal (1623-1638).

use gpui_kit::component::breadcrumb::{Breadcrumb, BreadcrumbItem};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{Disableable as _, Icon, IconNamed as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::nav::{Navigator, Route, navigate, open_tab};
use roostr_core::objects::{is_image_icon, object_icon, type_glyph};
use roostr_core::store::Store;
use roostr_core::types::{Block, Fields, RelationDef, Value, pos};
use roostr_core::{Glyph, PaneState, api, theme, ui};
use serde_json::json;

use crate::modal::{dialog_appear, outline_button, overlay, small_card};
use crate::workspace::Workspace;

pub(crate) fn init(_: &mut App) {}

/// The open More menu: gpui-kit's [`PopupMenu`] with its Change type flyout.
pub(crate) struct MoreMenu {
    menu: Entity<PopupMenu>,
    /// What `menu` was last filled from.
    model: Option<MoreModel>,
    /// The Change type flyout, refilled as its search changes.
    types: Entity<PopupMenu>,
    type_search: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

/// What the More menu shows (a change refills it).
#[derive(Clone, PartialEq)]
struct MoreModel {
    id: String,
    /// A phone leads with Graph (its header has no graph link) instead of
    /// Open in new tab (it has no tabs).
    mobile: bool,
    /// Why the object can't be deleted (empty: it can).
    blocker: String,
    kind: MoreKind,
}

#[derive(Clone, PartialEq)]
enum MoreKind {
    Property,
    Object {
        channel_id: String,
        pinned: bool,
        /// The space's collections: (id, "icon name").
        collections: Vec<(String, String)>,
        can_retype: bool,
        copy_id: bool,
    },
    Space,
}

/// A Change type choice.
struct TypeChoice {
    key: String,
    icon: String,
    name: String,
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
            // The path is one crumb (the open object, space or graph); the
            // whole pill opens search, so the crumb itself takes no click.
            .child(
                Breadcrumb::new()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(px(14.))
                    .child(
                        BreadcrumbItem::new(path_name)
                            .min_w_0()
                            .truncate()
                            .text_color(theme::fg()),
                    ),
            )
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

    /// Open or close the More menu. The menu is gpui-kit's [`PopupMenu`]
    /// (keyboard: arrows, Enter, Escape, ← → into the flyouts); it is filled
    /// from [`MoreModel`] and refilled when that changes (a delete blocker
    /// resolving, a pin toggling elsewhere).
    pub(crate) fn toggle_more(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.more.take().is_some() {
            cx.notify();
            return;
        }
        let menu = PopupMenu::build(window, cx, |menu, _, _| menu.min_w(px(234.)));
        let types = PopupMenu::build(window, cx, |menu, _, _| {
            menu.min_w(px(220.)).max_h(px(360.)).scrollable(true)
        });
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search types..."));
        let subscriptions = vec![
            // Escape, an item, or (desktop) a click outside closes it. A
            // phone has no click-away: its ⋯ toggles, and items close it.
            cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
                if window.last_input_was_keyboard() || !roostr_core::layout::is_mobile(window) {
                    this.more = None;
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.fill_type_menu(window, cx),
                    // Enter takes the first match.
                    InputEvent::PressEnter { .. } => {
                        let filter = input.read(cx).value().to_string();
                        let Some(id) = Self::open_object_id(cx) else {
                            return;
                        };
                        if let Some(choice) = type_choices(&id, &filter, cx).into_iter().next() {
                            this.more = None;
                            cx.notify();
                            change_type(&id, &choice.key, cx);
                        }
                    }
                    _ => {}
                },
            ),
        ];
        menu.focus_handle(cx).focus(window, cx);
        self.more = Some(MoreMenu {
            menu,
            model: None,
            types,
            type_search: input,
            _subscriptions: subscriptions,
        });
        self.fill_type_menu(window, cx);
        cx.notify();
    }

    fn render_more(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Div {
        div().relative().child(
            hbtn("more")
                .child("⋯")
                .child(self.anchor_probe("more"))
                .map(|d| self.data_tip(d, "tip-more", "More", cx))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_more(window, cx))),
        )
    }

    /// What the More menu shows for the open object, `None` without one.
    fn more_model(&self, mobile: bool, cx: &App) -> Option<MoreModel> {
        let id = Self::open_object_id(cx)?;
        let blocker = if self.delete_blocker.0 == id {
            self.delete_blocker.1.clone()
        } else {
            String::new()
        };
        let store = Store::global(cx).read(cx);
        let kind = if store.relations.iter().any(|r| r.id == id) {
            MoreKind::Property
        } else if let Some(summary) = store.summary(&id) {
            let active = Navigator::global(cx).read(cx).space().to_string();
            let pinned = roostr_core::pins::space_of(store, &summary.channel_id)
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
            let collections = store
                .summaries
                .iter()
                .filter(|s| {
                    s.type_key == "collection" && s.id != id && s.channel_id == collections_space
                })
                .map(|s| {
                    let name = if s.name.is_empty() {
                        "Untitled"
                    } else {
                        s.name.as_str()
                    };
                    let icon = object_icon(store, &s.icon, &s.type_key, &active);
                    (s.id.clone(), format!("{icon} {name}"))
                })
                .collect();
            MoreKind::Object {
                channel_id: summary.channel_id.clone(),
                pinned,
                collections,
                can_retype: !matches!(summary.type_key.as_str(), "query" | "collection" | "set"),
                copy_id: store.space(&summary.channel_id).is_some(),
            }
        } else if store.space(&id).is_some() {
            MoreKind::Space
        } else {
            return None;
        };
        Some(MoreModel {
            id,
            mobile,
            blocker,
            kind,
        })
    }

    /// The open More menu, refilled first if what it shows changed.
    pub(crate) fn render_more_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let model = self.more_model(roostr_core::layout::is_mobile(window), cx);
        let workspace = cx.entity().downgrade();
        let Some(more) = &mut self.more else {
            return div().id("more-menu");
        };
        let Some(model) = model else {
            return div().id("more-menu");
        };
        if more.model.as_ref() != Some(&model) {
            more.model = Some(model.clone());
            let types = more.types.clone();
            more.menu.update(cx, |menu, cx| {
                menu.rebuild(window, cx, move |menu, window, cx| {
                    more_items(menu, &model, &workspace, &types, window, cx)
                })
            });
        }
        div()
            .id("more-menu")
            .line_height(relative(1.2))
            .child(more.menu.clone())
    }

    /// Fill the Change type flyout: its search, then the space's other
    /// types matching it.
    fn fill_type_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(more) = &self.more else { return };
        let Some(id) = Self::open_object_id(cx) else {
            return;
        };
        let input = more.type_search.clone();
        let filter = input.read(cx).value().trim().to_string();
        let choices = type_choices(&id, &filter, cx);
        let workspace = cx.entity().downgrade();
        more.types.update(cx, |menu, cx| {
            let menu_focus = menu.focus_handle(cx);
            menu.rebuild(window, cx, move |mut menu, _, _| {
                // The search takes the keys while the flyout shows: at once
                // under the pointer, once → enters it from the keyboard.
                menu = menu.item(
                    PopupMenuItem::element(move |window, cx| {
                        let focused = input.read(cx).focus_handle(cx).is_focused(window);
                        if !focused
                            && (!window.last_input_was_keyboard() || menu_focus.is_focused(window))
                        {
                            let input = input.clone();
                            window.defer(cx, move |window, cx| {
                                input.update(cx, |input, cx| input.focus(window, cx))
                            });
                        }
                        crate::create_menu::type_search(&input).flex_1().mx_0()
                    })
                    .disabled(true),
                );
                if choices.is_empty() {
                    return menu.label(format!("No type matches “{filter}”"));
                }
                for choice in choices {
                    let target = id.clone();
                    let workspace = workspace.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{} {}", choice.icon, choice.name)).on_click(
                            move |_, _, cx| {
                                close_more(&workspace, cx);
                                change_type(&target, &choice.key, cx);
                            },
                        ),
                    );
                }
                menu
            })
        });
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
                                    outline_button("prop-del-cancel", "Cancel", false, cx)
                                        .disabled(deleting)
                                        .when(deleting, |b| b.opacity(0.5))
                                        .when(!deleting, |b| {
                                            b.on_click(cx.listener(|this, _, _, cx| {
                                                this.delete_prop = None;
                                                cx.notify();
                                            }))
                                        }),
                                )
                                .child(
                                    outline_button(
                                        "prop-del-ok",
                                        if deleting { "Deleting…" } else { "Delete property" },
                                        !deleting,
                                        cx,
                                    )
                                    .when(deleting, |b| {
                                        b.border_color(theme::red())
                                            .text_color(theme::red())
                                            .opacity(0.5)
                                    })
                                    .disabled(deleting)
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

/// Close the More menu from one of its items.
fn close_more(workspace: &WeakEntity<Workspace>, cx: &mut App) {
    let _ = workspace.update(cx, |this, cx| {
        this.more = None;
        cx.notify();
    });
}

const BLOCKED_TIP: &str = "Types and properties Roostr or your queries rely on can't be deleted";

/// A red row with the bin glyph (Move to bin, Delete property).
fn danger_item(label: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| div().text_color(theme::red()).child(label))
        .icon(Icon::new(Glyph::Trash).text_color(theme::red()))
}

/// The disabled row saying why the object can't be deleted; it still
/// explains itself in a tooltip.
fn blocked_item(blocker: &str) -> PopupMenuItem {
    let text = SharedString::from(format!("Can't delete: {blocker}"));
    PopupMenuItem::element(move |_, _| {
        div()
            .id("more-blocked")
            .flex_1()
            .text_size(px(12.))
            .child(text.clone())
            .tooltip(crate::tip(BLOCKED_TIP))
    })
    .icon(Icon::new(Glyph::Lock))
    .disabled(true)
}

/// The More menu's rows for `m`.
fn more_items(
    menu: PopupMenu,
    m: &MoreModel,
    workspace: &WeakEntity<Workspace>,
    types: &Entity<PopupMenu>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let id = m.id.clone();
    let lead = if m.mobile {
        let (ws, focus) = (workspace.clone(), id.clone());
        Some(
            PopupMenuItem::element(|_, _| {
                div().debug_selector(|| "more-graph".into()).child("Graph")
            })
            .icon(Icon::new(Glyph::Graph))
            .on_click(move |_, _, cx| {
                close_more(&ws, cx);
                navigate(
                    Route::Graph {
                        focus: Some(focus.clone()),
                    },
                    cx,
                );
            }),
        )
    } else if matches!(m.kind, MoreKind::Property) {
        None
    } else {
        let (ws, target) = (workspace.clone(), id.clone());
        Some(
            PopupMenuItem::new("Open in new tab")
                .icon(Icon::new(Glyph::ExternalLink))
                .on_click(move |_, _, cx| {
                    close_more(&ws, cx);
                    open_tab(Route::Object(target.clone()), true, cx);
                }),
        )
    };
    let copy_id = {
        let (ws, id) = (workspace.clone(), id.clone());
        PopupMenuItem::new("Copy ID")
            .icon(Icon::new(Glyph::Hash))
            .on_click(move |_, _, cx| {
                close_more(&ws, cx);
                cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
            })
    };
    let menu = menu.when_some(lead, |menu, lead| menu.item(lead));
    match &m.kind {
        MoreKind::Property => menu.separator().item(if m.blocker.is_empty() {
            let ws = workspace.clone();
            danger_item("Delete property").on_click(move |_, _, cx| {
                let rel = Store::global(cx)
                    .read(cx)
                    .relations
                    .iter()
                    .find(|r| r.id == id)
                    .cloned();
                if let Some(rel) = rel {
                    let _ = ws.update(cx, |this, cx| this.open_delete_property(rel, cx));
                }
            })
        } else {
            blocked_item(&m.blocker)
        }),
        MoreKind::Object {
            channel_id,
            pinned,
            collections,
            can_retype,
            copy_id: has_copy_id,
        } => {
            let pinned = *pinned;
            let (ws, target, channel) = (workspace.clone(), id.clone(), channel_id.clone());
            let mut menu = menu.item(
                PopupMenuItem::new(if pinned {
                    "Unpin from space"
                } else {
                    "Pin to space"
                })
                .icon(Icon::new(if pinned { Glyph::StarOff } else { Glyph::Star }))
                .on_click(move |_, _, cx| {
                    close_more(&ws, cx);
                    let space = roostr_core::pins::space_of(Store::global(cx).read(cx), &channel);
                    if let Some(space) = space {
                        roostr_core::pins::set_pinned(&target, &space, !pinned, cx)
                            .detach_and_log_err(cx);
                    }
                }),
            );
            let (ws, target, entries) = (workspace.clone(), id.clone(), collections.clone());
            menu = menu.submenu_with_icon(
                Some(Icon::new(Glyph::FolderPlus)),
                "Add to collection",
                window,
                cx,
                move |mut sub, _, _| {
                    if entries.is_empty() {
                        return sub.label("No collections in this space");
                    }
                    for (cid, label) in &entries {
                        let (ws, target, cid) = (ws.clone(), target.clone(), cid.clone());
                        sub = sub.item(PopupMenuItem::new(label.clone()).on_click(
                            move |_, _, cx| {
                                close_more(&ws, cx);
                                add_to_collection(&target, &cid, cx);
                            },
                        ));
                    }
                    sub
                },
            );
            if *can_retype {
                menu = menu.item(
                    PopupMenuItem::submenu("Change type", types.clone())
                        .icon(Icon::new(Glyph::ArrowLeftRight)),
                );
            }
            let (ws, target) = (workspace.clone(), id.clone());
            menu = menu.item(
                PopupMenuItem::new("Duplicate")
                    .icon(Icon::new(Glyph::Copy))
                    .on_click(move |_, _, cx| {
                        close_more(&ws, cx);
                        duplicate_object(&target, cx);
                    }),
            );
            if *has_copy_id {
                menu = menu.item(copy_id);
            }
            menu.separator().item(if m.blocker.is_empty() {
                let ws = workspace.clone();
                danger_item("Move to bin").on_click(move |_, _, cx| {
                    close_more(&ws, cx);
                    move_to_bin(&id, cx);
                })
            } else {
                blocked_item(&m.blocker)
            })
        }
        MoreKind::Space => menu.item(copy_id),
    }
}

/// The space's types an object can change to, matching `filter`.
fn type_choices(object_id: &str, filter: &str, cx: &App) -> Vec<TypeChoice> {
    let store = Store::global(cx).read(cx);
    let Some(summary) = store.summary(object_id) else {
        return Vec::new();
    };
    let active = Navigator::global(cx).read(cx).space().to_string();
    let space_id = Workspace::space_id(cx);
    let filter = filter.trim().to_lowercase();
    store
        .types
        .iter()
        .filter(|t| t.space == space_id && t.key != summary.type_key)
        .filter_map(|t| {
            let name = if t.name.is_empty() {
                t.key.clone()
            } else {
                t.name.clone()
            };
            (filter.is_empty() || name.to_lowercase().contains(&filter)).then(|| TypeChoice {
                key: t.key.clone(),
                icon: if t.icon.is_empty() {
                    type_glyph(store, &t.key, &active)
                } else {
                    t.icon.clone()
                },
                name,
            })
        })
        .collect()
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

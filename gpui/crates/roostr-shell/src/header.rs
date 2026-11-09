//! The page header (`.main-col > header`, `+layout.svelte:1524-1612`, CSS
//! 2749-2801): back/forward, the path button (opens search), the graph link,
//! the More menu with every item and submenu, the pane button with its badge,
//! and the delete-property modal (1623-1638).

use roostr_core::cover;
use roostr_core::nav::{Route, navigate, open_tab};
use roostr_core::objects::{object_icon, type_glyph};
use roostr_core::types::{Block, Fields, RelationDef, Value, pos};
use roostr_core::{State, api};
use roostr_ui::{
    CountBadge, Glyph, Input, PopupMenu, PopupMenuItem, Theme, danger_button, disabled, glyph,
    icon_button, outline_button, radius, ty, ty::Weights as _,
};
use rune::{Cursor, Cx, El, Tx, col, el, row, text};
use serde_json::json;

use crate::shell::{Shell, current_space, open_object_id, shell, space_id};

/// The delete-property confirmation.
pub(crate) struct DeleteProp {
    rel: RelationDef,
    /// Objects holding a value (`None` while counting).
    usage: Option<usize>,
    deleting: bool,
}

/// Which of the More menu's flyouts is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Flyout {
    Collections,
    Types,
}

/// The More menu's flyout (a slot: only the header reads it).
#[derive(Default)]
struct MoreFlyout(Option<Flyout>);

fn refresh(s: &mut State) {
    s.store.refresh_all();
}

/// The page header for the active route (`wide`: queries, sets and
/// collections run full width; others a 952 pt column).
pub(crate) fn view(
    s: &State,
    _cx: &mut Cx<State>,
    t: &Theme,
    wide: bool,
    _layer: &mut Vec<El>,
) -> El {
    let object_id = open_object_id(s);
    let route = s.nav.route().clone();
    let (path_icon, path_name, graph_path) = header_path(s, &route);
    let has_more = object_id.as_ref().is_some_and(|id| {
        s.store.relations.iter().any(|r| r.id == *id)
            || s.store.summary(id).is_some()
            || s.store.space(id).is_some()
    });
    // The pane button opens Chat (Properties without one) while it is collapsed.
    let primary = s.pane.primary();
    let pane_closed = (s.pane.available && !s.pane.is_open(primary)).then_some(primary);

    // The path pill: the whole pill opens search; the crumb itself takes no press.
    let icon = if graph_path {
        glyph(Glyph::Graph, 15.).color(t.fg)
    } else if roostr_ui::is_image_icon(&path_icon) {
        crate::shell::emoji_or_image(s, &path_icon, 18.)
    } else {
        text(path_icon).font(ty::ui(15.))
    };
    let path = crate::shell::fill("path", t.hover)
        .shrink()
        .min_w(0.)
        .px(10.)
        .py(4.)
        .gap(8.)
        .radius(radius::MD)
        .label(path_name.clone())
        .child(icon)
        .child(
            text(path_name)
                .font(ty::ui(14.).semibold())
                .color(t.fg)
                .truncate()
                .shrink()
                .min_w(0.),
        )
        .on_press(crate::search::open);
    let path = roostr_ui::tooltip(path, "Search (⌘K)");

    let graph_focus = object_id.clone();
    let mut right = row().gap(2.).items_center().child(
        icon_button("graph-link", Glyph::Graph)
            .accessibility_label("Graph")
            .tooltip("Graph")
            .on_press(move |s: &mut State| {
                navigate(
                    Route::Graph {
                        focus: graph_focus.clone(),
                    },
                    s,
                )
            })
            .build(t),
    );
    if has_more {
        right = right.child(more_button(t));
    }
    if let Some(primary) = pane_closed {
        // `.hbtn` at 30 pt with a border (muted under the pointer) and the
        // pane's message count (hidden at 0, capped at 99+).
        let button = row()
            .key("pane-open")
            .size(30., 30.)
            .center()
            .radius(radius::TAB)
            .border(1., t.border)
            .border_layout()
            .cursor(Cursor::Pointer)
            .hover(t.hover)
            .press(t.hover)
            .hover_group()
            .label("Chat & properties")
            .child(crate::shell::fill_glyph(t, Glyph::Pane, 16.))
            .on_press(move |s: &mut State| s.pane.set_open(primary, true, &mut s.ui));
        right = right.child(
            CountBadge::new(roostr_ui::tooltip(button, "Chat & properties"))
                .count(s.pane.count)
                .max(99)
                .color(t.accent)
                .build(t),
        );
    }

    let mut header = row()
        .key("page-header")
        .w_full()
        .items_center()
        .gap(8.)
        .h(roostr_ui::HEADER_H)
        .pt(18.)
        .px(16.)
        .pb(8.)
        .border_sides([0., 0., 1., 0.], t.border)
        .border_layout()
        .fill(t.bg)
        .child(
            row()
                .gap(2.)
                .items_center()
                .child(
                    icon_button("back", Glyph::NavBack)
                        .accessibility_label("Back")
                        .tooltip("Back")
                        .on_press(|s: &mut State| s.nav.back(&mut s.ui))
                        .build(t),
                )
                .child(
                    icon_button("forward", Glyph::NavForward)
                        .accessibility_label("Forward")
                        .tooltip("Forward")
                        .on_press(|s: &mut State| s.nav.forward(&mut s.ui))
                        .build(t),
                ),
        )
        .child(row().grow(1.).min_w(0.).justify_center().child(path))
        .child(right);
    if !wide {
        header = header.max_w(952.);
    }
    header
}

/// The ⋯ button (`more`): the menu anchors under it.
fn more_button(t: &Theme) -> El {
    roostr_ui::Button::new("more")
        .ghost()
        .accessibility_label("More")
        .tooltip("More")
        .child(text("⋯").font(ty::ui(16.)).color(t.muted))
        .on_press(toggle_more)
        .build(t)
        .size(28., 28.)
        .radius(radius::TAB)
        .cursor(Cursor::Pointer)
}

/// `headerPath` of `route`: (icon, name, is the graph).
pub(crate) fn header_path(s: &State, route: &Route) -> (String, String, bool) {
    let store = &s.store;
    let active = s.nav.space();
    let current = current_space(s);
    let space_icon = |icon: &str| {
        if icon.is_empty() {
            "◍".to_string()
        } else {
            icon.to_string()
        }
    };
    match route {
        Route::Object(id) => {
            if let Some(o) = store.summary(id) {
                (
                    object_icon(store, &o.icon, &o.type_key, active),
                    crate::shell::or_untitled(&o.name),
                    false,
                )
            } else if let Some(c) = store.space(id) {
                (space_icon(&c.icon), c.name.clone(), false)
            } else {
                ("▨".into(), "…".into(), false)
            }
        }
        Route::Graph { .. } => (
            String::new(),
            format!(
                "Graph — {}",
                current.map(|c| c.name.clone()).unwrap_or_default()
            ),
            true,
        ),
        _ => match current {
            Some(c) => (space_icon(&c.icon), c.name.clone(), false),
            None => ("◍".into(), "glon".into(), false),
        },
    }
}

/// Open or close the More menu.
pub(crate) fn toggle_more(s: &mut State) {
    let shell = s.slot::<Shell>();
    shell.more = !shell.more;
    if shell.more {
        shell.type_search = roostr_ui::InputState::new();
        s.slot::<MoreFlyout>().0 = None;
    }
}

fn close_more(s: &mut State) {
    s.slot::<Shell>().more = false;
    s.slot::<MoreFlyout>().0 = None;
}

const BLOCKED_TIP: &str = "Types and properties Roostr or your queries rely on can't be deleted";

/// A red row with the bin glyph (Move to bin, Delete property).
fn danger_item(t: &Theme, key: &str, label: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(
        row()
            .key(key.to_string())
            .gap(8.)
            .items_center()
            .child(glyph(Glyph::Trash, 12.).color(t.red))
            .child(text(label).font(ty::ui(14.)).color(t.red)),
    )
}

/// A plain row with a leading glyph.
fn item(label: &str, g: Glyph) -> PopupMenuItem {
    PopupMenuItem::new(label.to_string()).icon(g)
}

/// The disabled row saying why the object can't be deleted; it still
/// explains itself in a tooltip.
fn blocked_item(t: &Theme, blocker: &str) -> PopupMenuItem {
    let line = row()
        .key("more-blocked")
        .gap(8.)
        .items_center()
        .child(glyph(Glyph::Lock, 12.).color(t.kit.muted_foreground))
        .child(
            text(format!("Can't delete: {blocker}"))
                .font(ty::ui(12.))
                .color(t.kit.muted_foreground)
                .wrap()
                .shrink(),
        );
    PopupMenuItem::element(roostr_ui::tooltip(line, BLOCKED_TIP)).disabled(true)
}

/// The open More menu (desktop and phone), anchored under the ⋯ keyed
/// `more`. A desktop press outside closes it; a phone's ⋯ toggles it.
pub(crate) fn more_overlay(
    s: &State,
    _cx: &mut Cx<State>,
    t: &Theme,
    mobile: bool,
    layer: &mut Vec<El>,
) {
    let Some(menu) = more_menu(s, t, mobile) else {
        return;
    };
    let mut flyouts = Vec::new();
    let panel = menu.panel(t, "more-menu", &mut flyouts);
    let panel = roostr_ui::overlay::anchored(panel, "more", roostr_ui::Placement::BottomEnd, 4.);
    if mobile {
        layer.push(panel);
    } else {
        let panel = roostr_ui::overlay::dismissable(panel, "more-menu", close_more, layer);
        layer.push(panel);
    }
    layer.extend(flyouts);
}

/// The More menu's rows for the open object, `None` without one.
fn more_menu(s: &State, t: &Theme, mobile: bool) -> Option<PopupMenu> {
    let id = open_object_id(s)?;
    let sh = shell(s)?;
    let flyout = s.get::<MoreFlyout>().and_then(|f| f.0);
    let blocker = if sh.delete_blocker.0 == id {
        sh.delete_blocker.1.clone()
    } else {
        String::new()
    };
    let store = &s.store;
    let mut menu = PopupMenu::new().min_w(234.);
    // A phone leads with Graph (its header has no graph link) instead of
    // Open in new tab (it has no tabs).
    let target = id.clone();
    menu = menu.item(if mobile {
        PopupMenuItem::element(
            row()
                .key("more-graph")
                .gap(8.)
                .items_center()
                .child(glyph(Glyph::Graph, 12.).color(t.kit.foreground))
                .child(text("Graph").font(ty::ui(14.)).color(t.kit.foreground)),
        )
        .on_press(move |s: &mut State| {
            close_more(s);
            navigate(
                Route::Graph {
                    focus: Some(target.clone()),
                },
                s,
            );
        })
    } else {
        item("Open in new tab", Glyph::ExternalLink).on_press(move |s: &mut State| {
            close_more(s);
            open_tab(Route::Object(target.clone()), true, s);
        })
    });
    // A phone has no hover: its cover actions live here.
    if mobile && let Some(object) = store.object(&id).filter(|o| cover::can_have_cover(o)) {
        let has_cover = cover::cover_file(&object.fields).is_some();
        let target = id.clone();
        let label = if has_cover { "Cover" } else { "Add cover" };
        menu = menu.separator().item(
            PopupMenuItem::element(
                row()
                    .key("more-cover")
                    .gap(8.)
                    .items_center()
                    .child(
                        glyph(
                            if has_cover {
                                Glyph::Palette
                            } else {
                                Glyph::Plus
                            },
                            12.,
                        )
                        .color(t.kit.foreground),
                    )
                    .child(text(label).font(ty::ui(14.)).color(t.kit.foreground)),
            )
            .on_press(move |s: &mut State| {
                close_more(s);
                cover::ask(&target, s);
            }),
        );
        menu = menu.separator();
    }
    let copy_id = {
        let id = id.clone();
        item("Copy ID", Glyph::Hash).on_press_tx(move |s: &mut State, tx: &mut Tx| {
            close_more(s);
            tx.copy(id.clone());
        })
    };
    if store.relations.iter().any(|r| r.id == id) {
        let delete = if blocker.is_empty() {
            let target = id.clone();
            danger_item(t, "more-delete-prop", "Delete property").on_press(move |s: &mut State| {
                let rel = s.store.relations.iter().find(|r| r.id == target).cloned();
                if let Some(rel) = rel {
                    open_delete_property(s, rel);
                }
            })
        } else {
            blocked_item(t, &blocker)
        };
        return Some(menu.item(copy_id).separator().item(delete));
    }
    if let Some(summary) = store.summary(&id) {
        let active = s.nav.space();
        let pinned = roostr_core::pins::space_of(store, &summary.channel_id)
            .is_some_and(|sp| sp.pinned_ids.contains(&summary.id));
        let (target, channel) = (id.clone(), summary.channel_id.clone());
        menu = menu.item(
            item(
                if pinned {
                    "Unpin from space"
                } else {
                    "Pin to space"
                },
                if pinned { Glyph::StarOff } else { Glyph::Star },
            )
            .on_press(move |s: &mut State| {
                close_more(s);
                if let Some(space) = roostr_core::pins::space_of(&s.store, &channel) {
                    roostr_core::pins::set_pinned(&target, &space, !pinned, s)
                        .detach_and_log_err(&s.rt);
                }
            }),
        );
        // Add to collection: the space's collections.
        let collections_space = if !summary.channel_id.is_empty() {
            summary.channel_id.clone()
        } else {
            space_id(s)
        };
        let mut sub = PopupMenu::new();
        let collections: Vec<(String, String)> = store
            .summaries
            .iter()
            .filter(|o| {
                o.type_key == "collection" && o.id != id && o.channel_id == collections_space
            })
            .map(|o| {
                let icon = object_icon(store, &o.icon, &o.type_key, active);
                (
                    o.id.clone(),
                    format!("{icon} {}", crate::shell::or_untitled(&o.name)),
                )
            })
            .collect();
        if collections.is_empty() {
            sub = sub.label("No collections in this space");
        }
        for (cid, label) in collections {
            let target = id.clone();
            sub = sub.item(PopupMenuItem::new(label).on_press(move |s: &mut State| {
                close_more(s);
                add_to_collection(s, &target, &cid);
            }));
        }
        menu = menu.item(
            PopupMenuItem::submenu(
                "Add to collection",
                sub,
                flyout == Some(Flyout::Collections),
            )
            .icon(Glyph::FolderPlus)
            .on_toggle(|s: &mut State, open: bool| {
                s.slot::<MoreFlyout>().0 = open.then_some(Flyout::Collections);
            }),
        );
        if !matches!(summary.type_key.as_str(), "query" | "collection" | "set") {
            menu = menu.item(
                PopupMenuItem::submenu(
                    "Change type",
                    type_menu(s, t, &id, sh),
                    flyout == Some(Flyout::Types),
                )
                .icon(Glyph::ArrowLeftRight)
                .on_toggle(|s: &mut State, open: bool| {
                    s.slot::<MoreFlyout>().0 = open.then_some(Flyout::Types);
                    if open {
                        s.slot::<Shell>().type_search.focus();
                    }
                }),
            );
        }
        let target = id.clone();
        menu = menu.item(
            item("Duplicate", Glyph::Copy).on_press(move |s: &mut State| {
                close_more(s);
                duplicate_object(s, &target);
            }),
        );
        if store.space(&summary.channel_id).is_some() {
            menu = menu.item(copy_id);
        }
        let delete = if blocker.is_empty() {
            let target = id.clone();
            danger_item(t, "more-bin", "Move to bin").on_press(move |s: &mut State| {
                close_more(s);
                move_to_bin(s, &target);
            })
        } else {
            blocked_item(t, &blocker)
        };
        return Some(menu.separator().item(delete));
    }
    if store.space(&id).is_some() {
        return Some(menu.item(copy_id));
    }
    None
}

/// A Change type choice.
struct TypeChoice {
    key: String,
    icon: String,
    name: String,
}

/// The Change type flyout: its search (which takes the keys while the
/// flyout shows), then the space's other types matching it.
fn type_menu(s: &State, t: &Theme, id: &str, sh: &Shell) -> PopupMenu {
    let filter = sh.type_search.value().trim().to_string();
    let choices = type_choices(s, id, &filter);
    // `.type-search`: the kit input at margin 4×6, padding 5×8, 13px, bg
    // `--bg`, 1px border, radius 6 (h28).
    let target = id.to_string();
    let search = crate::create_menu::type_search(
        t,
        Input::new("type-search", &sh.type_search).placeholder("Search types..."),
    )
    .on_edit(|s: &mut State| &mut s.slot::<Shell>().type_search)
    // Enter takes the first match.
    .on_submit(move |s: &mut State, _: &mut Tx| {
        let filter = s.slot::<Shell>().type_search.value().trim().to_string();
        if let Some(choice) = type_choices(s, &target, &filter).into_iter().next() {
            close_more(s);
            change_type(s, &target, &choice.key);
        }
    })
    .build(t);
    let mut menu = PopupMenu::new()
        .min_w(220.)
        .max_h(360.)
        .item(PopupMenuItem::element(el().w_full().mx(-2.).my(4.).child(search)).disabled(false));
    if choices.is_empty() {
        return menu.label(format!("No type matches “{filter}”"));
    }
    for choice in choices {
        let target = id.to_string();
        let key = choice.key.clone();
        // The New menu's row: its icon column, 8 pt gap, 13 pt name.
        menu = menu.item(
            PopupMenuItem::element(
                row()
                    .gap(8.)
                    .py(3.)
                    .items_center()
                    .child(crate::shell::obj_icon(s, t, &choice.icon))
                    .child(text(choice.name).font(ty::ui(13.)).color(t.kit.foreground)),
            )
            .on_press(move |s: &mut State| {
                close_more(s);
                change_type(s, &target, &key);
            }),
        );
    }
    menu
}

/// The space's types an object can change to, matching `filter`.
fn type_choices(s: &State, object_id: &str, filter: &str) -> Vec<TypeChoice> {
    let store = &s.store;
    let Some(summary) = store.summary(object_id) else {
        return Vec::new();
    };
    let active = s.nav.space();
    let space = space_id(s);
    let filter = filter.trim().to_lowercase();
    store
        .types
        .iter()
        .filter(|t| t.space == space && t.key != summary.type_key)
        .filter_map(|t| {
            let name = if t.name.is_empty() {
                t.key.clone()
            } else {
                t.name.clone()
            };
            (filter.is_empty() || name.to_lowercase().contains(&filter)).then(|| TypeChoice {
                key: t.key.clone(),
                icon: if t.icon.is_empty() {
                    type_glyph(store, &t.key, active)
                } else {
                    t.icon.clone()
                },
                name,
            })
        })
        .collect()
}

/// `addToCollection`: append the object to the collection's members.
fn add_to_collection(s: &mut State, object_id: &str, collection_id: &str) {
    let fetch = s.store.fetch_object(collection_id);
    let (object_id, collection_id) = (object_id.to_string(), collection_id.to_string());
    s.rt.spawn(async move |cx| {
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
            let task = cx.update(|s| {
                api::note::set_field(&collection_id, "collectionIds", Value::values(items), s)
            });
            let _ = task.await;
        }
        cx.update(refresh);
    })
    .detach();
}

fn change_type(s: &mut State, object_id: &str, type_key: &str) {
    let task = api::note::set_type(object_id, type_key, s);
    s.rt.spawn(async move |cx| {
        let _ = task.await;
        cx.update(refresh);
    })
    .detach();
}

fn move_to_bin(s: &mut State, object_id: &str) {
    let task = api::note::delete(object_id, s);
    s.rt.spawn(async move |cx| {
        if task.await.is_ok() {
            cx.update(|s| {
                refresh(s);
                navigate(Route::Home, s);
            });
        }
    })
    .detach();
}

/// `duplicateObject`: same fields (name + " copy"), the whole block tree in
/// DFS order (roots appended, children INNER-appended), then open it.
fn duplicate_object(s: &mut State, object_id: &str) {
    let fetch = s.store.fetch_object(object_id);
    s.rt.spawn(async move |cx| {
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
        let task = cx.update(|s| api::note::create(&name, Some(&type_key), Some(fields), s));
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
            let task = cx.update(|s| {
                api::note::block_add(&id, &block, parent.as_deref().unwrap_or(""), position, s)
            });
            if task.await.is_err() {
                return;
            }
        }
        cx.update(|s| navigate(Route::Object(id), s));
    })
    .detach();
}

// ------------------------------------------------------------ delete property

fn open_delete_property(s: &mut State, rel: RelationDef) {
    close_more(s);
    s.slot::<Shell>().delete_prop = Some(DeleteProp {
        rel: rel.clone(),
        usage: None,
        deleting: false,
    });
    let query = api::query(
        json!({ "filters": [{ "key": rel.key, "condition": "exists" }], "limit": 1000 }),
        s,
    );
    s.rt.spawn(async move |cx| {
        let Ok(res) = query.await else { return };
        let usage = res
            .records
            .iter()
            .filter(|r| r.id != rel.id && r.type_key != "relation" && r.type_key != "type")
            .count();
        cx.update(|s| {
            if let Some(d) = &mut s.slot::<Shell>().delete_prop
                && d.rel.id == rel.id
            {
                d.usage = Some(usage);
            }
        });
    })
    .detach();
}

fn cancel_delete(s: &mut State) {
    let shell = s.slot::<Shell>();
    if !shell.delete_prop.as_ref().is_some_and(|d| d.deleting) {
        shell.delete_prop = None;
    }
}

/// Strip the value from every object holding it (paging until none),
/// then vanish the relation.
fn delete_property(s: &mut State) {
    let Some(d) = &mut s.slot::<Shell>().delete_prop else {
        return;
    };
    if d.deleting {
        return;
    }
    d.deleting = true;
    let rel = d.rel.clone();
    s.rt.spawn(async move |cx| {
        let result: anyhow::Result<()> = async {
            loop {
                let query = cx.update(|s| {
                    api::query(
                        json!({ "filters": [{ "key": rel.key, "condition": "exists" }], "limit": 1000 }),
                        s,
                    )
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
                    cx.update(|s| api::note::delete_field(&victim, &rel.key, s))
                        .await?;
                }
            }
            cx.update(|s| api::note::vanish(std::slice::from_ref(&rel.id), s))
                .await?;
            Ok(())
        }
        .await;
        cx.update(|s| {
            let shell = s.slot::<Shell>();
            if let Some(d) = &mut shell.delete_prop {
                d.deleting = false;
            }
            match result {
                Ok(()) => {
                    shell.delete_prop = None;
                    refresh(s);
                    navigate(Route::Home, s);
                }
                Err(error) => log::warn!("delete property failed: {error:#}"),
            }
        });
    })
    .detach();
}

/// The delete-property confirmation (`.prop-del-modal`) while it is open. A
/// press outside or Escape cancels, except while deleting.
pub(crate) fn delete_overlay(s: &State, t: &Theme, layer: &mut Vec<El>) {
    let Some(d) = shell(s).and_then(|sh| sh.delete_prop.as_ref()) else {
        return;
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
    let body = col()
        .key("prop-del-modal")
        .child(
            text(format!("Delete \"{name}\"?"))
                .font(ty::ui(16.).bold())
                .color(t.fg)
                .mb(8.),
        )
        .child(
            text(format!(
                "This deletes the property and removes its value from {usage} — on every device, forever. This cannot be undone."
            ))
            .font(ty::ui(13.).lh(13. * 1.5))
            .color(t.muted)
            .wrap()
            .mb(14.),
        )
        .child(
            row()
                .justify_end()
                .gap(8.)
                .child(
                    disabled(outline_button("prop-del-cancel", "Cancel"), d.deleting)
                        .on_press(|s: &mut State| s.slot::<Shell>().delete_prop = None)
                        .build(t),
                )
                .child(
                    disabled(
                        danger_button(
                            "prop-del-ok",
                            if d.deleting {
                                "Deleting…"
                            } else {
                                "Delete property"
                            },
                        ),
                        d.deleting,
                    )
                    .on_press(delete_property)
                    .build(t),
                ),
        );
    let dismiss: Option<fn(&mut State)> = (!d.deleting).then_some(cancel_delete as fn(&mut State));
    crate::modal::small_dialog(t, "prop-del", body, dismiss, layer);
}

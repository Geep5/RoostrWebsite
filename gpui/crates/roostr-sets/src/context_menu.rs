//! Right-click menu for records in a view (RowContextMenu.svelte): open,
//! open in new tab, change type, edit properties, add to / remove from a
//! collection, move to bin. Bulk by construction: it acts on every id given.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::Icon;
use gpui_kit::component::empty::Empty;
use gpui_kit::component::input::{Escape as InputEscape, Input, InputEvent, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::nav::{self, Route};
use roostr_core::objects::{object_icon, type_glyph};
use roostr_core::protected::blockers_for;
use roostr_core::relations::{is_reserved_key, space_relations};
use roostr_core::{Fields, Glyph, RelationDef, Store, Value, api, theme};
use roostr_properties::PropertyValue;

use crate::common;
use crate::config;

/// What the menu tells its host.
pub(crate) enum MenuEvent {
    /// Close the menu (and clear the host's selection).
    Close,
    /// Records changed: reload.
    Changed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fly {
    Types,
    Cols,
    Props,
}

/// Multi-value formats merge across records; everything else overwrites.
fn is_merge(rel: &RelationDef) -> bool {
    rel.format == "tag" || rel.format == "object"
}

fn list_of(fields: Option<&Fields>, key: &str) -> Vec<String> {
    fields
        .and_then(|f| f.get(key))
        .map(|v| {
            v.items()
                .iter()
                .filter_map(|i| i.string_value.clone())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) struct RowContextMenu {
    position: Point<Pixels>,
    ids: Vec<String>,
    space_id: String,
    /// Collections only: "Remove from collection" severs membership.
    collection: Option<String>,
    /// The view's visible relation keys head the Edit-properties list.
    relation_keys: Vec<String>,
    /// Row fields the host already holds.
    fields_of: HashMap<String, Fields>,
    fly: Option<Fly>,
    type_filter: Entity<InputState>,
    prop_filter: Entity<InputState>,
    edit_key: String,
    edit_value: Option<Value>,
    blocked: HashMap<String, String>,
    /// The kit menu of top-level rows, and the protected-id count it was
    /// built for (rebuilt when that changes).
    menu: Entity<PopupMenu>,
    menu_blocked: Option<usize>,
    /// Set by a flyout row's click so the menu's dismiss keeps it open.
    keep_open: Rc<Cell<bool>>,
    /// The menu's bounds on the last frame and each flyout row's: a flyout
    /// sits against the menu, level with its row.
    menu_bounds: common::BoundsCell,
    fly_rows: [common::BoundsCell; 3],
    _subs: Vec<Subscription>,
}

impl EventEmitter<MenuEvent> for RowContextMenu {}

impl RowContextMenu {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        position: Point<Pixels>,
        ids: Vec<String>,
        space_id: String,
        collection: Option<String>,
        relation_keys: Vec<String>,
        fields_of: HashMap<String, Fields>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let type_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search types..."));
        let prop_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter properties..."));
        let menu = PopupMenu::build(window, cx, |menu, _, _| menu);
        let subs = vec![
            cx.subscribe_in(
                &type_filter,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        if let Some(key) = this.filtered_types(cx).first().map(|t| t.0.clone()) {
                            this.retype(key, cx);
                        }
                    }
                    _ => cx.notify(),
                },
            ),
            cx.observe(&prop_filter, |_, _, cx| cx.notify()),
            cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
                this.menu_dismissed(window, cx)
            }),
        ];
        // Keyboard focus starts in the menu: arrows, Enter and Escape.
        window.focus(&menu.focus_handle(cx), cx);
        let task = blockers_for(&ids, cx);
        cx.spawn(async move |this, cx| {
            if let Ok(blocked) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.blocked = blocked;
                    cx.notify();
                });
            }
        })
        .detach();
        Self {
            position,
            ids,
            space_id,
            collection,
            relation_keys,
            fields_of,
            fly: None,
            type_filter,
            prop_filter,
            edit_key: String::new(),
            edit_value: None,
            blocked: HashMap::new(),
            menu,
            menu_blocked: None,
            keep_open: Rc::new(Cell::new(false)),
            menu_bounds: common::bounds_cell(),
            fly_rows: [
                common::bounds_cell(),
                common::bounds_cell(),
                common::bounds_cell(),
            ],
            _subs: subs,
        }
    }

    fn suffix(&self) -> String {
        if self.ids.len() > 1 {
            format!(" ({})", self.ids.len())
        } else {
            String::new()
        }
    }

    fn deletable(&self) -> Vec<String> {
        self.ids
            .iter()
            .filter(|id| !self.blocked.contains_key(*id))
            .cloned()
            .collect()
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(MenuEvent::Close);
    }

    /// Space types plus bundled ones, filtered by the search box: (key, label).
    fn filtered_types(&self, cx: &App) -> Vec<(String, String)> {
        let q = self.type_filter.read(cx).value().trim().to_lowercase();
        let store = Store::global(cx).read(cx);
        store
            .types
            .iter()
            .filter(|t| t.space.is_empty() || t.space == self.space_id)
            .filter(|t| {
                q.is_empty()
                    || (if t.name.is_empty() { &t.key } else { &t.name })
                        .to_lowercase()
                        .contains(&q)
            })
            .map(|t| {
                let icon = if t.icon.is_empty() {
                    type_glyph(store, &t.key, &self.space_id)
                } else {
                    t.icon.clone()
                };
                (
                    t.key.clone(),
                    format!(
                        "{} {}",
                        icon,
                        if t.name.is_empty() { &t.key } else { &t.name }
                    ),
                )
            })
            .collect()
    }

    fn prop_rels(&self, cx: &App) -> Vec<RelationDef> {
        let store = Store::global(cx).read(cx);
        let all: Vec<&RelationDef> = space_relations(&store.relations, &self.space_id)
            .into_iter()
            .filter(|r| !r.hidden && !r.read_only && !is_reserved_key(&r.key))
            .collect();
        let head: Vec<&RelationDef> = self
            .relation_keys
            .iter()
            .filter_map(|k| all.iter().find(|r| r.key == *k).copied())
            .collect();
        let rest = all
            .iter()
            .filter(|r| !head.iter().any(|h| h.id == r.id))
            .copied();
        head.iter().copied().chain(rest).cloned().collect()
    }

    fn open_fly(&mut self, kind: Fly, window: &mut Window, cx: &mut Context<Self>) {
        if kind == Fly::Types {
            self.type_filter
                .update(cx, |s, cx| s.set_value("", window, cx));
        }
        if kind == Fly::Props {
            self.edit_key.clear();
            self.edit_value = None;
        }
        self.fly = if self.fly == Some(kind) {
            None
        } else {
            Some(kind)
        };
        if self.fly == Some(Fly::Types) {
            self.type_filter.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let targets = self.ids.clone();
        self.fly = None;
        self.close(cx);
        // One object opens in place; a multi-selection opens each as a tab.
        if targets.len() == 1 {
            common::open_object(&targets[0], cx);
            return;
        }
        for id in targets {
            nav::open_tab(Route::Object(id), true, cx);
        }
    }

    fn open_tabs(&mut self, cx: &mut Context<Self>) {
        let targets = self.ids.clone();
        self.close(cx);
        for id in targets {
            common::open_object_tab(&id, cx);
        }
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        let Some(collection) = self.collection.clone() else {
            return;
        };
        let targets = self.ids.clone();
        self.close(cx);
        let task = common::remove_members(&collection, &targets, cx);
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            let _ = this.update(cx, |_, cx| cx.emit(MenuEvent::Changed));
        })
        .detach();
    }

    fn bin(&mut self, cx: &mut Context<Self>) {
        let targets = self.deletable();
        self.close(cx);
        cx.spawn(async move |this, cx| {
            for id in targets {
                let task = cx.update(|cx| api::note::delete(&id, cx));
                let _ = task.await;
            }
            let _ = this.update(cx, |_, cx| cx.emit(MenuEvent::Changed));
        })
        .detach();
    }

    /// Retype in place: blocks and fields survive.
    fn retype(&mut self, type_key: String, cx: &mut Context<Self>) {
        let targets = self.ids.clone();
        self.close(cx);
        cx.spawn(async move |this, cx| {
            for id in targets {
                let task = cx.update(|cx| api::note::set_type(&id, &type_key, cx));
                let _ = task.await;
            }
            let _ = this.update(cx, |_, cx| cx.emit(MenuEvent::Changed));
        })
        .detach();
    }

    fn add_to(&mut self, collection_id: String, cx: &mut Context<Self>) {
        let targets = self.ids.clone();
        self.close(cx);
        let fetch = Store::global(cx).read(cx).fetch_object(&collection_id, cx);
        cx.spawn(async move |this, cx| {
            if let Ok(col) = fetch.await {
                let have = config::member_ids(&col.fields);
                let mut merged = have.clone();
                for id in targets {
                    if !merged.contains(&id) {
                        merged.push(id);
                    }
                }
                if merged.len() != have.len() {
                    let task = cx.update(|cx| common::set_members(&collection_id, &merged, cx));
                    let _ = task.await;
                }
            }
            let _ = this.update(cx, |_, cx| cx.emit(MenuEvent::Changed));
        })
        .detach();
    }

    /// Fields of every target: the host's rows, else fetched.
    #[allow(clippy::result_large_err)]
    fn values_of(&self, cx: &App) -> Task<Vec<Option<Fields>>> {
        let store = Store::global(cx).read(cx);
        let tasks: Vec<Result<Fields, Task<anyhow::Result<roostr_core::Object>>>> = self
            .ids
            .iter()
            .map(|id| match self.fields_of.get(id) {
                Some(f) => Ok(f.clone()),
                None => Err(store.fetch_object(id, cx)),
            })
            .collect();
        cx.background_spawn(async move {
            let mut out = Vec::new();
            for t in tasks {
                out.push(match t {
                    Ok(f) => Some(f),
                    Err(task) => task.await.ok().map(|o| o.fields),
                });
            }
            out
        })
    }

    /// Open one property: the initial value is the cross-record intersection.
    fn open_prop(&mut self, key: String, cx: &mut Context<Self>) {
        self.edit_key = key.clone();
        self.edit_value = None;
        cx.notify();
        let Some(rel) = self.prop_rels(cx).into_iter().find(|r| r.key == key) else {
            return;
        };
        let values = self.values_of(cx);
        cx.spawn(async move |this, cx| {
            let all = values.await;
            let initial = if is_merge(&rel) {
                let mut inter = list_of(all.first().and_then(Option::as_ref), &key);
                for f in all.iter().skip(1) {
                    let l = list_of(f.as_ref(), &key);
                    inter.retain(|v| l.contains(v));
                }
                Some(Value::values(
                    inter.into_iter().map(Value::string).collect(),
                ))
            } else {
                let first = all
                    .first()
                    .and_then(|f| f.as_ref().and_then(|f| f.get(&key).cloned()));
                if all
                    .iter()
                    .skip(1)
                    .all(|f| f.as_ref().and_then(|f| f.get(&key).cloned()) == first)
                {
                    first
                } else {
                    None
                }
            };
            let _ = this.update(cx, |this, cx| {
                if this.edit_key == key {
                    this.edit_value = initial;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn apply_prop(&mut self, rel: RelationDef, v: Value, cx: &mut Context<Self>) {
        let targets = self.ids.clone();
        let base: Vec<String> = self
            .edit_value
            .as_ref()
            .map(|e| {
                e.items()
                    .iter()
                    .filter_map(|i| i.string_value.clone())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let values = self.values_of(cx);
        // Back to the property list; the menu stays open for the next one.
        self.edit_key.clear();
        self.edit_value = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            if is_merge(&rel) {
                let next: Vec<String> = v
                    .items()
                    .iter()
                    .filter_map(|i| i.string_value.clone())
                    .filter(|s| !s.is_empty())
                    .collect();
                let added: Vec<String> =
                    next.iter().filter(|x| !base.contains(x)).cloned().collect();
                let removed: Vec<String> =
                    base.iter().filter(|x| !next.contains(x)).cloned().collect();
                let all = values.await;
                for (id, fields) in targets.iter().zip(all) {
                    let cur = list_of(fields.as_ref(), &rel.key);
                    let mut merged: Vec<String> = cur
                        .iter()
                        .filter(|x| !removed.contains(x))
                        .cloned()
                        .collect();
                    merged.extend(added.iter().filter(|x| !cur.contains(x)).cloned());
                    let value = Value::values(merged.into_iter().map(Value::string).collect());
                    let task = cx.update(|cx| api::note::set_field(id, &rel.key, value, cx));
                    let _ = task.await;
                }
            } else {
                for id in &targets {
                    let task = cx.update(|cx| api::note::set_field(id, &rel.key, v.clone(), cx));
                    let _ = task.await;
                }
            }
            let _ = this.update(cx, |_, cx| cx.emit(MenuEvent::Changed));
        })
        .detach();
    }

    fn render_fly(
        &mut self,
        kind: Fly,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        // `.ctx-fly`: max-height min(340px, 60vh) (content-box: + padding and border).
        let max_h = (window.viewport_size().height * 0.6).min(px(340.)) + px(10.);
        let mut fly = div()
            .id("ctx-fly")
            .flex()
            .flex_col()
            .w(px(250.))
            .max_h(max_h)
            .overflow_y_scroll()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(px(10.))
            .p(px(4.))
            .text_size(px(13.))
            .text_color(theme::fg())
            .shadow(menu_shadow())
            .occlude();
        let mut list = div().flex().flex_col();
        match kind {
            Fly::Types => {
                fly = fly.child(
                    div()
                        .mx(px(6.))
                        .mt(px(4.))
                        .mb(px(6.))
                        // The Input binds Escape itself: catch its action first.
                        .capture_action(cx.listener(|this, _: &InputEscape, window, cx| {
                            cx.stop_propagation();
                            if this.type_filter.read(cx).value().is_empty() {
                                this.fly = None;
                                // Keep keyboard focus in the menu: the next Escape closes it.
                                window.focus(&this.menu.focus_handle(cx), cx);
                            } else {
                                this.type_filter
                                    .update(cx, |s, cx| s.set_value("", window, cx));
                            }
                            cx.notify();
                        }))
                        .child(Input::new(&self.type_filter).text_size(px(13.))),
                );
                let types = self.filtered_types(cx);
                if types.is_empty() {
                    let q = self.type_filter.read(cx).value().trim().to_string();
                    list = list.child(none_row(format!("No type matches “{q}”")));
                }
                for (key, label) in types {
                    list = list.child(
                        menu_item(SharedString::from(format!("type-{key}")))
                            .child(label)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.retype(key.clone(), cx)),
                            ),
                    );
                }
            }
            Fly::Cols => {
                let store = Store::global(cx).read(cx);
                let cols: Vec<(String, String)> = store
                    .summaries
                    .iter()
                    .filter(|s| {
                        s.type_key == "collection"
                            && s.channel_id == self.space_id
                            && !self.ids.contains(&s.id)
                    })
                    .map(|s| {
                        let icon = object_icon(store, &s.icon, &s.type_key, &self.space_id);
                        (
                            s.id.clone(),
                            format!(
                                "{icon} {}",
                                if s.name.is_empty() {
                                    "Untitled"
                                } else {
                                    &s.name
                                }
                            ),
                        )
                    })
                    .collect();
                if cols.is_empty() {
                    list = list.child(none_row("No collections in this space".into()));
                }
                for (id, label) in cols {
                    list = list.child(
                        menu_item(SharedString::from(format!("col-{id}")))
                            .child(label)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.add_to(id.clone(), cx)),
                            ),
                    );
                }
            }
            Fly::Props => {
                let rels = self.prop_rels(cx);
                if let Some(rel) = rels.iter().find(|r| r.key == self.edit_key).cloned() {
                    let label = format!(
                        "{}{}",
                        rel.icon_emoji
                            .as_deref()
                            .filter(|e| !e.is_empty())
                            .map(|e| format!("{e} "))
                            .unwrap_or_default(),
                        if rel.name.is_empty() {
                            &rel.key
                        } else {
                            &rel.name
                        }
                    );
                    let entity = cx.entity().downgrade();
                    let save_rel = rel.clone();
                    fly = fly
                        .child(
                            menu_item("ctx-back")
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(9.))
                                        .child(Icon::new(Glyph::ChevronLeft).size(px(14.)))
                                        .child(label),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.edit_key.clear();
                                    this.edit_value = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div().px(px(6.)).pt(px(2.)).pb(px(8.)).child(
                                PropertyValue::new(
                                    SharedString::from(format!("ctx-edit-{}", rel.key)),
                                    rel,
                                    self.edit_value.clone(),
                                )
                                .auto_open(true)
                                .on_save(move |v, _, cx| {
                                    let v = v.clone();
                                    let rel = save_rel.clone();
                                    let _ =
                                        entity.update(cx, |this, cx| this.apply_prop(rel, v, cx));
                                }),
                            ),
                        );
                    return fly;
                }
                if rels.len() > 6 {
                    fly = fly.child(
                        div()
                            .mx(px(6.))
                            .mt(px(4.))
                            .mb(px(6.))
                            .child(Input::new(&self.prop_filter).text_size(px(13.))),
                    );
                }
                let q = self.prop_filter.read(cx).value().trim().to_lowercase();
                let filtered: Vec<RelationDef> = rels
                    .into_iter()
                    .filter(|r| {
                        q.is_empty()
                            || (if r.name.is_empty() { &r.key } else { &r.name })
                                .to_lowercase()
                                .contains(&q)
                    })
                    .collect();
                if filtered.is_empty() {
                    list = list.child(none_row("No properties".into()));
                }
                for r in filtered {
                    let key = r.key.clone();
                    let label = format!(
                        "{}{}",
                        r.icon_emoji
                            .as_deref()
                            .filter(|e| !e.is_empty())
                            .map(|e| format!("{e} "))
                            .unwrap_or_default(),
                        if r.name.is_empty() { &r.key } else { &r.name }
                    );
                    list = list.child(
                        menu_item(SharedString::from(format!("prop-{key}")))
                            .child(label)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_prop(key.clone(), cx)),
                            ),
                    );
                }
            }
        }
        fly.child(list)
    }
}

fn menu_shadow() -> Vec<BoxShadow> {
    vec![common::shadow(hsla(0., 0., 0., 0.4), 0., 8., 24., 0.)]
}

/// A flyout row (`.ctx-item`), the kit list row.
fn menu_item(id: impl Into<ElementId>) -> ListItem {
    ListItem::new(id)
        .gap(px(9.))
        .w_full()
        .px(px(10.))
        .py(px(7.))
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(theme::fg())
        .cursor_pointer()
}

/// A flyout's "nothing here" line, the kit empty state.
fn none_row(text: String) -> Empty {
    common::empty_state(text, 12.).px(px(8.)).py(px(4.))
}

/// One top-level row's content (`.ctx-item`: 16px icon, 9px gap, 13px label,
/// 7×10 padding). It spans the kit row edge to edge (the kit pads 8px) so the
/// open-flyout highlight covers the whole row.
fn row_body(glyph: Glyph, label: SharedString) -> Div {
    div()
        .flex()
        .flex_1()
        .items_center()
        .gap(px(9.))
        .mx(px(-8.))
        .px(px(10.))
        .py(px(7.))
        .rounded(px(6.))
        .text_size(px(13.))
        .child(Icon::new(glyph).size(px(16.)))
        .child(label)
}

impl RowContextMenu {
    /// Fill the kit menu: Open, Open in new tab, the three flyout rows,
    /// Remove from collection, a separator, then Move to bin or the locked
    /// "Can't delete" line. Rebuilt when the protected ids arrive.
    fn build_menu(&self, menu: PopupMenu, this: &WeakEntity<Self>) -> PopupMenu {
        let suffix = self.suffix();
        let act = |f: fn(&mut Self, &mut Context<Self>)| {
            let this = this.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let _ = this.update(cx, |this, cx| f(this, cx));
            }
        };
        let plain = |glyph: Glyph, label: String| {
            let label = SharedString::from(label);
            PopupMenuItem::element(move |_, _| row_body(glyph, label.clone()))
        };
        let fly_row = |glyph: Glyph, label: String, kind: Fly| {
            let label = SharedString::from(label);
            let (this, keep_open, cell) = (
                this.clone(),
                self.keep_open.clone(),
                self.fly_rows[kind as usize].clone(),
            );
            let open_this = this.clone();
            PopupMenuItem::element(move |_, cx| {
                let open = this.upgrade().is_some_and(|e| e.read(cx).fly == Some(kind));
                row_body(glyph, label.clone())
                    .relative()
                    .when(open, |d| d.bg(theme::hover()))
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .text_color(theme::muted())
                            .child(Icon::new(Glyph::ChevronRight).size(px(14.))),
                    )
                    .child(common::track_bounds(&cell))
            })
            .on_click(move |_, window, cx| {
                // A flyout row toggles its flyout; the menu stays open.
                keep_open.set(true);
                let _ = open_this.update(cx, |this, cx| this.open_fly(kind, window, cx));
            })
        };

        let mut menu = menu
            .min_w(px(200.))
            .item(plain(Glyph::ArrowUpRight, format!("Open{suffix}")).on_click(act(Self::open)))
            .item(
                plain(Glyph::ExternalLink, format!("Open in new tab{suffix}"))
                    .on_click(act(Self::open_tabs)),
            )
            .item(fly_row(
                Glyph::ArrowLeftRight,
                format!("Change type{suffix}"),
                Fly::Types,
            ))
            .item(fly_row(
                Glyph::SlidersHorizontal,
                format!("Edit properties{suffix}"),
                Fly::Props,
            ))
            .item(fly_row(
                Glyph::FolderPlus,
                format!("Add to collection{suffix}"),
                Fly::Cols,
            ));
        if self.collection.is_some() {
            menu = menu.item(
                plain(
                    Glyph::FolderMinus,
                    format!("Remove from collection{suffix}"),
                )
                .on_click(act(Self::remove)),
            );
        }
        menu = menu.separator();
        let deletable = self.deletable();
        if deletable.is_empty() {
            let why = SharedString::from(format!(
                "Can't delete: {}",
                self.blocked.values().next().cloned().unwrap_or_default()
            ));
            menu.item(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .id("ctx-locked")
                        .flex()
                        .flex_1()
                        .items_center()
                        .gap(px(9.))
                        .mx(px(-8.))
                        .px(px(10.))
                        .py(px(7.))
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .tooltip(roostr_core::ui::tooltip(
                            "Types and properties Roostr or your queries rely on can't be deleted",
                        ))
                        .child(Icon::new(Glyph::Lock).size(px(16.)))
                        .child(why.clone())
                })
                .disabled(true),
            )
        } else {
            let label = SharedString::from(format!(
                "Move to bin{}{}",
                if deletable.len() > 1 {
                    format!(" ({})", deletable.len())
                } else {
                    String::new()
                },
                if self.blocked.is_empty() {
                    String::new()
                } else {
                    format!(" - {} protected kept", self.blocked.len())
                }
            ));
            menu.item(
                PopupMenuItem::element(move |_, _| {
                    row_body(Glyph::Trash, label.clone()).text_color(theme::red())
                })
                .on_click(act(Self::bin)),
            )
        }
    }

    /// The kit menu closed itself. A flyout row asked to stay open; a press
    /// outside is the backdrop's to handle (it must not close the menu while
    /// a flyout's own popovers take clicks); a key (Escape, Enter on a row)
    /// closes it.
    fn menu_dismissed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.keep_open.replace(false) {
            return;
        }
        if window.last_input_was_keyboard() {
            self.close(cx);
        }
    }
}

impl Render for RowContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.menu_blocked != Some(self.blocked.len()) {
            self.menu_blocked = Some(self.blocked.len());
            let this = cx.weak_entity();
            let menu = self.menu.clone();
            let state = &*self;
            menu.update(cx, |menu, cx| {
                menu.rebuild(window, cx, |menu, _, _| state.build_menu(menu, &this))
            });
        }
        let viewport = window.viewport_size();
        let pos = point(
            self.position.x.min(viewport.width - px(230.)),
            self.position.y,
        );
        let flyout = self.fly.map(|kind| {
            let content = self.render_fly(kind, window, cx);
            // Each flyout enters as it opens (switching flyouts mounts the next).
            let content = common::appear(("ctx-fly", kind as usize), content, window, cx);
            // Flush against the menu's right edge, centred on its row
            // (estimated 320px tall); flipped to the menu's left edge within
            // 10px of the window edge, clamped to the window.
            let menu = self.menu_bounds.get();
            let row = self.fly_rows[kind as usize].get();
            let flip = menu.right() + px(250.) > viewport.width - px(10.);
            let x = if flip {
                menu.left() - px(5.)
            } else {
                menu.right() + px(5.)
            };
            anchored()
                .position(point(x, row.top() + px(15.5 - 160.)))
                .anchor(if flip {
                    Anchor::TopRight
                } else {
                    Anchor::TopLeft
                })
                .snap_to_window_with_margin(px(8.))
                .child(content)
        });
        // In-tree overlays (inputs keep their keyboard actions); the data view
        // paints this menu after everything else.
        div()
            // The click-away backdrop.
            .child(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("ctx-backdrop")
                        .w(viewport.width)
                        .h(viewport.height)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.close(cx)),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| this.close(cx)),
                        ),
                ),
            )
            .child(
                anchored()
                    .position(pos)
                    .snap_to_window_with_margin(px(8.))
                    .child(common::appear(
                        "ctx-menu",
                        div()
                            .relative()
                            .child(common::track_bounds(&self.menu_bounds))
                            .child(self.menu.clone()),
                        window,
                        cx,
                    )),
            )
            .children(flyout)
    }
}

//! Query/set/collection/type data views: table, kanban, calendar, gallery,
//! query controls (the `.dataview` part of the object page).
//!
//! Every data view is keyed by its object's id: its state (controls, the active
//! record view, the collection picker, the row menu) lives in the [`Sets`] slot,
//! and [`view`] draws it from there. [`register`] installs the store listener
//! (reloads on commits) and the per-frame sync (the query follows the object).

mod calendar;
mod common;
mod config;
mod context_menu;
mod controls;
mod gallery;
mod kanban;
mod layout_icon;
mod table;
mod views;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use roostr_core::create::default_type_key;
use roostr_core::filters::space_filter_of;
use roostr_core::relations::object_space_id;
use roostr_core::{Object, State, StoreEvent, Value, api, field_str, lite_ui};
use roostr_rt::Task;
use roostr_ui::{Button, Glyph, Input, InputState, Theme, outline_button, ty};
use rune::{Cx, Drag, El, Key, KeyEvent, Tx, col, el, row, text};
use serde_json::json;

use crate::views::ViewInput;

/// Which kind of object a data view shows records for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataViewKind {
    /// A query / set: records matching its sources and filters.
    Query,
    /// A collection: its hand-picked members.
    Collection,
    /// A type page: every object of the type.
    Type,
}

/// The record view a data view shows (its object's `viewType`).
pub(crate) enum Active {
    Table(table::Table),
    Gallery(gallery::Gallery),
    Kanban(kanban::Kanban),
    Calendar(calendar::Calendar),
}

impl Active {
    fn kind(&self) -> &'static str {
        match self {
            Self::Table(_) => "table",
            Self::Gallery(_) => "gallery",
            Self::Kanban(_) => "kanban",
            Self::Calendar(_) => "calendar",
        }
    }

    fn shows(&self, id: &str) -> bool {
        match self {
            Self::Table(v) => v.row_ids().any(|r| r == id),
            Self::Gallery(v) => v.row_ids().any(|r| r == id),
            Self::Kanban(v) => v.row_ids().any(|r| r == id),
            Self::Calendar(v) => v.row_ids().any(|r| r == id),
        }
    }
}

/// Every open data view, by object id.
#[derive(Default)]
pub(crate) struct Sets {
    pub views: HashMap<String, DataView>,
}

/// One data view: controls + the active record view for a query/set,
/// collection or type object.
pub(crate) struct DataView {
    pub id: String,
    pub kind: DataViewKind,
    pub controls: controls::Controls,
    pub active: Option<Active>,
    /// Live search text from the controls row.
    pub search: String,
    pub input: ViewInput,
    /// The collection bar's "+ Add object" picker.
    pub picking: bool,
    pub pick_query: InputState,
    pub candidates: Vec<(String, String, String)>,
    /// The open row context menu (table, gallery, kanban).
    pub menu: Option<context_menu::RowMenu>,
    /// The object, space and relation count the query was last computed from.
    seen: Option<Arc<Object>>,
    seen_space: String,
    seen_relations: usize,
    /// Commits burst: one reload after they settle.
    reload: Option<Task<()>>,
    /// Lite: collection members last asked of the relays.
    fetched: Vec<String>,
}

impl DataView {
    fn new(id: String, kind: DataViewKind) -> Self {
        Self {
            controls: controls::Controls::new(kind),
            id,
            kind,
            active: None,
            search: String::new(),
            input: ViewInput {
                body: None,
                sorts: Vec::new(),
                updated_at: 0,
            },
            picking: false,
            pick_query: InputState::new(),
            candidates: Vec::new(),
            menu: None,
            seen: None,
            seen_space: String::new(),
            seen_relations: 0,
            reload: None,
            fetched: Vec::new(),
        }
    }

    pub(crate) fn collection_id(&self) -> Option<String> {
        (self.kind == DataViewKind::Collection).then(|| self.id.clone())
    }

    pub(crate) fn table(&self) -> Option<&table::Table> {
        match &self.active {
            Some(Active::Table(t)) => Some(t),
            _ => None,
        }
    }

    pub(crate) fn table_mut(&mut self) -> Option<&mut table::Table> {
        match &mut self.active {
            Some(Active::Table(t)) => Some(t),
            _ => None,
        }
    }
}

pub(crate) fn dv<'a>(s: &'a State, id: &str) -> Option<&'a DataView> {
    s.get::<Sets>().and_then(|sets| sets.views.get(id))
}

pub(crate) fn dv_mut<'a>(s: &'a mut State, id: &str) -> Option<&'a mut DataView> {
    s.slot::<Sets>().views.get_mut(id)
}

/// Install the data views' listeners (replaces GPUI's `init`): a commit to a
/// view's object or to a record on screen reloads it, and every frame each
/// open view follows its object (filters, sorts, view type, members).
pub fn register(s: &mut State) {
    s.on_store(on_store);
    s.on_poll(on_poll);
}

/// Open the data view of `object_id` (idempotent): loads the object into the
/// store cache and starts its records loading once it is there. [`view`] opens
/// a view it finds closed by itself; hosts call this to start loading early.
pub fn open(s: &mut State, object_id: &str, kind: DataViewKind) {
    if dv(s, object_id).is_some_and(|v| v.kind == kind) {
        return;
    }
    s.store.ensure_object(object_id);
    s.slot::<Sets>().views.insert(
        object_id.to_string(),
        DataView::new(object_id.to_string(), kind),
    );
    controls::open(s, object_id);
    sync(s, object_id);
}

/// Drop the data view of `object_id` (the page showing it went away).
pub fn close(s: &mut State, object_id: &str) {
    s.slot::<Sets>().views.remove(object_id);
}

/// Escape inside a data view: close the collection picker, the controls'
/// menus, the row menu, the cell editor and the selection. True when anything
/// was open.
pub fn dismiss(s: &mut State, object_id: &str) -> bool {
    let Some(v) = dv_mut(s, object_id) else {
        return false;
    };
    let mut any = std::mem::take(&mut v.picking);
    any |= v.controls.dismiss();
    any |= v.menu.take().is_some();
    any |= match &mut v.active {
        Some(Active::Table(t)) => t.dismiss(),
        Some(Active::Gallery(g)) => g.dismiss(),
        Some(Active::Kanban(k)) => k.dismiss(),
        _ => false,
    };
    any
}

/// The window's `on_key` hook for keys nothing focused took: ← / → scroll the
/// table under the pointer sideways (never while a text field has focus: the
/// field keeps them), Escape dismisses what an open view shows.
pub fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    table::on_window_key(s, ev, tx)
}

/// GPUI's `page_mouse_down`: the object page forwards drags that start on
/// the page around the data view (its margins, the space below the table),
/// so the table's rubber band can start there, like the web's page-wide
/// selection provider. Attach to the page column:
/// `.on_drag(move |s, d, tx| roostr_sets::page_drag(s, &id, d, tx))`.
/// Presses with cmd/ctrl/shift, other buttons and non-table views are
/// ignored; a plain click (no drag) clears the selection.
pub fn page_drag(s: &mut State, object_id: &str, drag: &Drag, tx: &mut Tx) {
    table::page_drag(s, object_id, drag, tx);
}

fn on_store(s: &mut State, event: &StoreEvent) {
    let ids: Vec<String> = s.get::<Sets>().map_or_else(Vec::new, |sets| {
        sets.views
            .values()
            .filter(|v| match event {
                StoreEvent::Committed(id) => {
                    *id == v.id || v.active.as_ref().is_some_and(|a| a.shows(id))
                }
                StoreEvent::Reconnected => true,
                StoreEvent::Refreshed | StoreEvent::Live(_) => false,
            })
            .map(|v| v.id.clone())
            .collect()
    });
    for id in ids {
        schedule_reload(s, &id);
    }
}

/// Bursts collapse into one reload, 250ms after the last commit.
fn schedule_reload(s: &mut State, id: &str) {
    let view = id.to_string();
    let wait = s.rt.timer(Duration::from_millis(250));
    let task = s.rt.spawn(async move |cx| {
        wait.await;
        cx.update(|s| reload(s, &view));
    });
    if let Some(v) = dv_mut(s, id) {
        v.reload = Some(task);
    }
}

/// Reload the active view's records (a refresh keeps the rows scrolled in).
pub(crate) fn reload(s: &mut State, id: &str) {
    let Some(v) = dv_mut(s, id) else { return };
    v.reload = None;
    match v.active.as_ref().map(Active::kind) {
        Some("table") => table::reload(s, id),
        Some("gallery") => gallery::load(s, id),
        Some("kanban") => kanban::load(s, id),
        Some("calendar") => calendar::load(s, id),
        _ => {}
    }
}

fn on_poll(s: &mut State) -> bool {
    let ids: Vec<String> = s
        .get::<Sets>()
        .map_or_else(Vec::new, |sets| sets.views.keys().cloned().collect());
    let mut changed = false;
    for id in ids {
        changed |= sync(s, &id);
        changed |= controls::poll(s, &id);
        changed |= table::poll(s, &id);
    }
    changed
}

/// Recompute the record query when the object, the space or the relations
/// moved, hand it to the active view and swap the view when `viewType`
/// changed. True when anything changed.
fn sync(s: &mut State, id: &str) -> bool {
    let Some(object) = s.store.object(id) else {
        return false;
    };
    let space = roostr_core::relations::current_space_id(s);
    let relations_n = s.store.relations.len();
    let Some(v) = dv(s, id) else { return false };
    let same = v.seen.as_ref().is_some_and(|o| Arc::ptr_eq(o, &object))
        && v.seen_space == space
        && v.seen_relations == relations_n;
    if same {
        return false;
    }
    let (kind, search) = (v.kind, v.search.clone());
    if let Some(v) = dv_mut(s, id) {
        v.seen = Some(object.clone());
        v.seen_space = space;
        v.seen_relations = relations_n;
    }
    lite(s, id, kind, &object);
    apply_input(s, id, &object, kind, &search);
    true
}

/// The record query for the view's current object and search; swaps or
/// updates the active record view.
fn apply_input(s: &mut State, id: &str, object: &Object, kind: DataViewKind, search: &str) {
    let relations = common::scoped_relations(s, object);
    let body = common::table_body(s, object, kind, search, &relations);
    let input = ViewInput {
        body: body.clone(),
        sorts: config::view_sorts(&object.fields),
        updated_at: object.updated_at,
    };
    let view_type = config::view_type(&object.fields).to_string();
    let Some(v) = dv_mut(s, id) else { return };
    v.controls.set_body(body);
    let swap = v.active.as_ref().map(Active::kind) != Some(view_type.as_str());
    let changed = input != v.input;
    v.input = input.clone();
    if swap {
        v.menu = None;
        v.active = Some(match view_type.as_str() {
            "gallery" => Active::Gallery(gallery::Gallery::new(input)),
            "kanban" => Active::Kanban(kanban::Kanban::new(input)),
            "calendar" => Active::Calendar(calendar::Calendar::new(input)),
            _ => Active::Table(table::Table::new(input)),
        });
        reload_fresh(s, id);
    } else if changed {
        match view_type.as_str() {
            "gallery" => gallery::set_input(s, id, input),
            "kanban" => kanban::set_input(s, id, input),
            "calendar" => calendar::set_input(s, id, input),
            _ => table::set_input(s, id, input),
        }
    }
}

/// A view just made loads from the top.
fn reload_fresh(s: &mut State, id: &str) {
    match dv(s, id).and_then(|v| v.active.as_ref()).map(Active::kind) {
        Some("table") => table::load(s, id, false),
        Some("gallery") => gallery::load(s, id),
        Some("kanban") => kanban::load(s, id),
        Some("calendar") => calendar::load(s, id),
        _ => {}
    }
}

/// Lite: an opened query/set or type page over a space not held whole
/// downloads that space (the refresh when it lands reloads the rows); a
/// collection fetches its members.
fn lite(s: &mut State, id: &str, kind: DataViewKind, object: &Object) {
    if roostr_core::lite::status(s).is_none() {
        return;
    }
    if kind == DataViewKind::Collection {
        let ids = config::member_ids(&object.fields);
        let Some(v) = dv_mut(s, id) else { return };
        if !ids.is_empty() && ids != v.fetched {
            v.fetched = ids.clone();
            lite_ui::fetch(ids, s);
        }
        return;
    }
    let space = object_space_id(&s.store, &object.fields);
    lite_ui::ensure_space(&space, s);
}

/// The controls' search text changed (live filter).
pub(crate) fn set_search(s: &mut State, id: &str, q: String) {
    let Some(v) = dv_mut(s, id) else { return };
    if v.search == q {
        return;
    }
    v.search = q.clone();
    let kind = v.kind;
    let Some(object) = s.store.object(id) else {
        return;
    };
    apply_input(s, id, &object, kind, &q);
}

/// Table-view New: the record joins the view (collection membership included)
/// and the table reloads with it.
pub(crate) fn on_record_created(s: &mut State, id: &str, record: String) {
    let Some(v) = dv(s, id) else { return };
    let join = (v.kind == DataViewKind::Collection).then(|| {
        let mut members = s
            .store
            .object(id)
            .map(|o| config::member_ids(&o.fields))
            .unwrap_or_default();
        members.push(record);
        common::set_members(id, &members, s)
    });
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        if let Some(join) = join
            && let Err(error) = join.await
        {
            log::error!("add to collection: {error:#}");
        }
        cx.update(|s| table::reload(s, &view));
    })
    .detach();
}

// ── Collection bar ───────────────────────────────────────────────

fn toggle_picker(s: &mut State, id: &str) {
    let Some(v) = dv_mut(s, id) else { return };
    if v.picking {
        v.picking = false;
        return;
    }
    v.pick_query.clear();
    let Some(object) = s.store.object(id) else {
        return;
    };
    let default = s
        .store
        .channels
        .first()
        .map(|c| c.id.clone())
        .unwrap_or_default();
    // Self-contained spaces: only this space's objects are addable.
    let task = api::query(
        json!({"filters": [space_filter_of(&object.fields, &default)], "limit": 200}),
        s,
    );
    let members = config::member_ids(&object.fields);
    let current = object.id.clone();
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(res) = task.await else { return };
        const HIDDEN: &[&str] = &[
            "program",
            "typescript",
            "json",
            "proto",
            "relation",
            "collection",
            "query",
            "set",
            "type",
            "template",
            "agent",
            "skill",
        ];
        let candidates = res
            .records
            .iter()
            .filter(|r| {
                r.id != current
                    && !members.contains(&r.id)
                    && !HIDDEN.contains(&r.type_key.as_str())
            })
            .map(|r| {
                let name = field_str(&r.fields, "name");
                let name = if name.is_empty() {
                    r.id.chars().take(8).collect()
                } else {
                    name.to_string()
                };
                (r.id.clone(), name, r.type_key.clone())
            })
            .collect();
        cx.update(|s| {
            if let Some(v) = dv_mut(s, &view) {
                v.candidates = candidates;
                v.picking = true;
                v.pick_query.focus();
            }
        });
    })
    .detach();
}

/// Anytype collection New: create an object of the space's Default type and link it.
fn new_in_collection(s: &mut State, id: &str) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    let ch = field_str(&object.fields, "channel").to_string();
    let members = config::member_ids(&object.fields);
    let type_task = default_type_key(&ch, s);
    let collection = id.to_string();
    s.rt.spawn(async move |cx| {
        let type_key = type_task.await;
        let mut fields = roostr_core::Fields::new();
        if !ch.is_empty() {
            fields.insert("channel".into(), Value::string(&ch));
        }
        let create = cx.update(|s| roostr_core::create::from_type("", &type_key, fields, s));
        let new_id = match create.await {
            Ok(new_id) => new_id,
            Err(error) => {
                log::error!("new in collection: {error:#}");
                return;
            }
        };
        let mut next = members;
        next.push(new_id.clone());
        let join = cx.update(|s| common::set_members(&collection, &next, s));
        if let Err(error) = join.await {
            log::error!("add to collection: {error:#}");
        }
        cx.update(|s| common::open_object(&new_id, s));
    })
    .detach();
}

fn add_member(s: &mut State, id: &str, member: String) {
    if let Some(v) = dv_mut(s, id) {
        v.picking = false;
    }
    let mut members = s
        .store
        .object(id)
        .map(|o| config::member_ids(&o.fields))
        .unwrap_or_default();
    members.push(member);
    let task = common::set_members(id, &members, s);
    task.detach_and_log_err(&s.rt);
}

fn collection_bar(s: &State, t: &Theme, v: &DataView, members: usize) -> El {
    let id = v.id.clone();
    let new = {
        let id = id.clone();
        common::new_button(&format!("{id}/col-new"), false)
            .on_press(move |s: &mut State| new_in_collection(s, &id))
            .build(t)
    };
    let add = {
        let id = id.clone();
        outline_button(format!("{id}/col-add"), "+ Add object")
            .bg(t.panel)
            .px(12.0)
            .py(6.0)
            .on_press(move |s: &mut State| toggle_picker(s, &id))
            .build(t)
    };
    let mut bar = row().center_y().gap(12.0).mb(12.0).child(new).child(add);
    if members > 0 {
        bar = bar.child(
            text(format!("{members} object(s)"))
                .font(ty::ui(12.0))
                .color(t.muted),
        );
    }
    let mut out = col().child(bar);
    if v.picking {
        let q = v.pick_query.value().trim().to_lowercase();
        let shown: Vec<&(String, String, String)> = v
            .candidates
            .iter()
            .filter(|c| q.is_empty() || c.1.to_lowercase().contains(&q))
            .collect();
        let mut list = col().gap(4.0);
        for (cand, name, type_key) in &shown {
            let (view, pick) = (id.clone(), cand.clone());
            list = list.child(
                common::vrow(t, &format!("{id}/pick-{cand}"), false)
                    .fill(t.panel)
                    .radius(8.0)
                    .px(12.0)
                    .child(
                        row()
                            .gap(4.0)
                            .center_y()
                            .child(text(name.clone()).font(ty::ui(13.0)).color(t.fg))
                            .child(text(type_key.clone()).font(ty::ui(12.0)).color(t.muted)),
                    )
                    .on_press(move |s: &mut State| add_member(s, &view, pick.clone())),
            );
        }
        if shown.is_empty() {
            let line = if v.candidates.is_empty() {
                "Nothing to add."
            } else {
                "No matches."
            };
            list = list.child(common::empty_state(t, line, 12.0));
        }
        let list = col()
            .key(format!("{id}/picker-list"))
            .max_h(260.0)
            .min_h(0.0)
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .child(list);
        let query = {
            let view = id.clone();
            Input::new(format!("{id}/pick-query"), &v.pick_query)
                .placeholder("Search objects…")
                .text_size(13.0)
                .on_edit(move |s: &mut State| {
                    &mut s
                        .slot::<Sets>()
                        .views
                        .get_mut(&view)
                        .expect("the picker's data view is open")
                        .pick_query
                })
                .build(t)
        };
        let close = {
            let view = id.clone();
            Button::new(format!("{id}/picker-close"))
                .ghost()
                .small()
                .icon(Glyph::Close)
                .text_color(t.muted)
                .pointer()
                .tooltip("Close (Esc)")
                .on_press(move |s: &mut State| {
                    if let Some(v) = dv_mut(s, &view) {
                        v.picking = false;
                    }
                })
                .build(t)
        };
        out = out.child(
            col()
                .gap(8.0)
                .border(1.0, t.border)
                .radius(10.0)
                .padding(10.0)
                .mb(14.0)
                .child(
                    row()
                        .gap(8.0)
                        .center_y()
                        .child(el().grow(1.0).min_w(0.0).child(query))
                        .child(close),
                )
                .child(list),
        );
    }
    let _ = s;
    out
}

/// The data view of `object_id`: the collection bar (collections), the lite
/// space notice, the controls row and its panels, then the active record view
/// (table, gallery, kanban or calendar). Menus and popovers go into `layer`;
/// mount it with [`roostr_ui::root`]. A view not yet open opens itself.
pub fn view(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
    kind: DataViewKind,
) -> El {
    let Some(v) = dv(s, object_id).filter(|v| v.kind == kind) else {
        let id = object_id.to_string();
        s.rt.spawn(async move |cx| cx.update(|s| open(s, &id, kind)))
            .detach();
        return el();
    };
    let Some(object) = s.store.object(object_id) else {
        return el();
    };
    let id = object_id.to_string();
    let members = config::member_ids(&object.fields).len();
    let mut chrome = col().key(format!("{id}/chrome")).w_full();
    if kind == DataViewKind::Collection {
        chrome = chrome.child(collection_bar(s, t, v, members));
    } else {
        // Lite: the space downloading (or its button, after a failed try).
        let space = object_space_id(&s.store, &object.fields);
        if let Some(notice) = lite_ui::space_notice(&space, s) {
            chrome = chrome.child(common::notice_row(t, &format!("{id}/notice"), notice));
        }
    }
    chrome = chrome.child(controls::view(s, cx, t, layer, v));
    let records = if v.input.body.is_some() {
        match &v.active {
            Some(Active::Table(_)) => table::view(s, cx, t, layer, v),
            Some(Active::Gallery(_)) => gallery::view(s, cx, t, layer, v),
            Some(Active::Kanban(_)) => kanban::view(s, cx, t, layer, v),
            Some(Active::Calendar(_)) => calendar::view(s, cx, t, layer, v),
            None => el(),
        }
    } else if kind == DataViewKind::Collection {
        text("Empty collection — add objects.")
            .font(ty::ui(12.0))
            .color(t.muted)
    } else {
        el()
    };
    if v.menu.is_some() {
        context_menu::view(s, cx, t, layer, v);
    }
    let escape = id.clone();
    let band = id.clone();
    col()
        .key(format!("{id}/dataview"))
        .w_full()
        .focusable(true)
        .no_focus_ring()
        .on_key(move |s: &mut State, ev: &KeyEvent, tx: &mut Tx| {
            if ev.key == Key::Escape {
                return dismiss(s, &escape);
            }
            table::on_view_key(s, &escape, ev, tx)
        })
        // Anytype's rubber band arms anywhere in the view below its chrome;
        // interactive parts (buttons, the header, the entry row) take their
        // own presses first.
        .on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| table::marquee(s, &band, d, tx))
        .child(chrome)
        .child(records)
}

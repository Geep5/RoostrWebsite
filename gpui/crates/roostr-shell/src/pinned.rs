//! A pinned object's sidebar widget body (`lib/components/PinnedWidget.svelte`):
//! sets render a mini version of their current view (rows, gallery cards,
//! kanban group counts, a mini month); any other object lists what it links to.
//!
//! Each pin's state lives in the [`Pinned`] slot keyed by object id: Shell's
//! poll keeps it to the current space's open pins ([`sync`]), store events
//! reload it ([`register`]), and the sidebar / space home draw [`widget`]
//! inside their own card.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use chrono::{Datelike, Local, NaiveDate, TimeZone};
use roostr_core::filters::{engine_filters_of, space_filter_of};
use roostr_core::nav::{Route, navigate};
use roostr_core::objects::object_icon;
use roostr_core::relations::{object_space_id, space_relations};
use roostr_core::types::{Object, QueryRow, RelationDef, Value, field_str};
use roostr_core::{State, StoreEvent, api, collection, lite_ui};
use roostr_rt::Task;
use roostr_ui::color::mix_srgb;
use roostr_ui::ty::{self, Weights as _};
use roostr_ui::{ShimmerText, Skeleton, Theme, motion, radius};
use rune::{Cursor, Cx, El, TextStyle, col, el, row, text};
use serde_json::{Value as Json, json};

use crate::drag::{self, Axis};
use crate::shell::{fill, hover_fg, icon_text};

/// Rows a list widget fetches: all of them, short of a query matching thousands.
const LIST_FETCH: usize = 500;
const GALLERY_LIMIT: usize = 8;
const DOW: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
/// A list widget's row: the 24px line and the 1px below it.
const ROW_PITCH: f32 = 25.;
/// How far past the window a list draws rows, in window heights each way.
const OVERSCAN: f32 = 1.;

/// Every pinned widget's state, by pinned object id.
#[derive(Default)]
pub(crate) struct Pinned {
    widgets: HashMap<String, Widget>,
}

#[derive(Default)]
struct Widget {
    obj: Option<Arc<Object>>,
    rows: Vec<QueryRow>,
    month_off: i32,
    seq: u64,
    /// Lite: the query/set's space, while it is not held whole (its rows
    /// would be partial); it downloads and the widget shows the progress line.
    partial_space: Option<String>,
    /// Lite: ids last asked of the relays (collection members, link
    /// targets), so the refresh after the fetch does not ask again.
    fetched: Vec<String>,
    /// The running load (dropping the widget cancels it).
    load: Option<Task<()>>,
}

/// Listeners: widgets reload on store events. Their rows are other objects:
/// any of them may have synced in or changed since (a phone's first history
/// walk runs under it), so every collection refresh - throttled, after
/// commits - reloads them all; a commit of the pinned object reloads its own.
pub(crate) fn register(s: &mut State) {
    s.on_store(on_store);
}

fn on_store(s: &mut State, event: &StoreEvent) {
    let ids: Vec<String> = match s.get::<Pinned>() {
        Some(p) => p
            .widgets
            .keys()
            .filter(|id| match event {
                StoreEvent::Committed(oid) => oid == *id,
                StoreEvent::Refreshed | StoreEvent::Reconnected => true,
                StoreEvent::Live(_) => false,
            })
            .cloned()
            .collect(),
        None => return,
    };
    for id in ids {
        load(s, &id);
    }
}

/// Keep widgets for exactly `ids` (start loading new ones, drop the rest).
/// True when anything changed.
pub(crate) fn sync(s: &mut State, ids: &[String]) -> bool {
    let pinned = s.slot::<Pinned>();
    let before = pinned.widgets.len();
    pinned.widgets.retain(|id, _| ids.contains(id));
    let mut changed = pinned.widgets.len() != before;
    let new: Vec<String> = ids
        .iter()
        .filter(|id| !pinned.widgets.contains_key(*id))
        .cloned()
        .collect();
    for id in new {
        s.slot::<Pinned>()
            .widgets
            .insert(id.clone(), Widget::default());
        load(s, &id);
        changed = true;
    }
    changed
}

/// The widget has nothing to preview (no object yet; notes, pages: no rows,
/// while a calendar always shows its month) and no space downloading: no body
/// at all.
pub(crate) fn is_empty(s: &State, id: &str) -> bool {
    let Some(w) = s.get::<Pinned>().and_then(|p| p.widgets.get(id)) else {
        return true;
    };
    let Some(obj) = &w.obj else {
        return true;
    };
    let calendar = is_set(obj) && obj.field_str("viewType") == "calendar";
    w.rows.is_empty() && !calendar && w.partial_space.is_none()
}

/// The widget of `id`, if it is still kept and its load `seq` is the latest.
fn current<'a>(s: &'a mut State, id: &str, seq: u64) -> Option<&'a mut Widget> {
    s.slot::<Pinned>()
        .widgets
        .get_mut(id)
        .filter(|w| w.seq == seq)
}

/// Fetch the pinned object, then its rows.
fn load(s: &mut State, id: &str) {
    let fetch = s.store.fetch_object(id);
    let Some(w) = s.slot::<Pinned>().widgets.get_mut(id) else {
        return;
    };
    w.seq += 1;
    let seq = w.seq;
    let id_ = id.to_string();
    let task = s.rt.spawn(async move |cx| {
        let object = match fetch.await {
            Ok(object) => Arc::new(object),
            Err(_) => {
                cx.update(|s| {
                    if let Some(w) = current(s, &id_, seq) {
                        w.obj = None;
                    }
                });
                return;
            }
        };
        let body = cx.update(|s| {
            lite(s, &id_, &object);
            query_body(s, &object)
        });
        let rows = match body {
            Some(body) => cx
                .update(|s| api::query(body, s))
                .await
                .ok()
                .map(|res| res.records),
            None => Some(Vec::new()),
        };
        cx.update(|s| {
            let Some(w) = current(s, &id_, seq) else {
                return;
            };
            match rows {
                Some(mut rows) => {
                    if let Some(members) = manual_order(&object) {
                        collection::sort_by_members(&mut rows, &members);
                    }
                    w.obj = Some(object);
                    w.rows = rows;
                }
                None => w.obj = None,
            }
        });
    });
    if let Some(w) = s.slot::<Pinned>().widgets.get_mut(id) {
        w.load = Some(task);
    }
}

/// Lite: a query/set over a space not held whole downloads that space
/// (once; the refresh when it lands re-evaluates); a collection or a link
/// widget fetches the ids it shows.
fn lite(s: &mut State, id: &str, o: &Object) {
    if roostr_core::lite::status(s).is_none() {
        if let Some(w) = s.slot::<Pinned>().widgets.get_mut(id) {
            w.partial_space = None;
        }
        return;
    }
    let ids = if o.type_key == "collection" {
        members(o)
    } else if is_set(o) {
        let space = object_space_id(&s.store, &o.fields);
        let partial = if lite_ui::space_held(&space, s) {
            None
        } else {
            lite_ui::ensure_space(&space, s);
            Some(space)
        };
        if let Some(w) = s.slot::<Pinned>().widgets.get_mut(id) {
            w.partial_space = partial;
        }
        return;
    } else {
        link_targets(o)
    };
    let Some(w) = s.slot::<Pinned>().widgets.get_mut(id) else {
        return;
    };
    if !ids.is_empty() && ids != w.fetched {
        w.fetched = ids.clone();
        lite_ui::fetch(ids, s);
    }
}

fn is_set(o: &Object) -> bool {
    matches!(o.type_key.as_str(), "query" | "set" | "collection")
}

fn space_rels(s: &State, o: &Object) -> Vec<RelationDef> {
    space_relations(&s.store.relations, &object_space_id(&s.store, &o.fields))
        .into_iter()
        .cloned()
        .collect()
}

/// A collection's member ids.
fn members(o: &Object) -> Vec<String> {
    collection::members(&o.fields)
}

/// A collection in its own order (no view sorts): its member ids, which its
/// items show in and drag to reorder.
fn manual_order(o: &Object) -> Option<Vec<String>> {
    let sorted = o.field("viewSorts").is_some_and(|v| {
        v.items().iter().any(|i| {
            i.entries()
                .and_then(|e| e.get("key").and_then(|k| k.as_str()).map(|k| !k.is_empty()))
                .unwrap_or(false)
        })
    });
    (o.type_key == "collection" && !sorted).then(|| members(o))
}

/// What a non-set object links to (the tree widget's rows).
fn link_targets(o: &Object) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    let mut add = |id: &str| {
        if !id.is_empty() && !targets.iter().any(|t| t == id) {
            targets.push(id.to_string());
        }
    };
    for (key, v) in &o.fields {
        if key == "channel" {
            continue;
        }
        if let Some(link) = v.link_value.as_ref().filter(|l| !l.target_id.is_empty()) {
            add(&link.target_id);
        } else if v.values_value.is_some() {
            for item in v.items() {
                if let Some(link) = item.link_value.as_ref().filter(|l| !l.target_id.is_empty()) {
                    add(&link.target_id);
                } else if key == "collectionIds"
                    && let Some(s) = &item.string_value
                {
                    add(s);
                }
            }
        }
    }
    targets
}

/// The widget's query (`None` = nothing to ask: no members / no links).
fn query_body(s: &State, o: &Object) -> Option<Json> {
    if is_set(o) {
        let members = members(o);
        let mut filters = engine_filters_of(&o.fields, &space_rels(s, o));
        filters.push(space_filter_of(
            &o.fields,
            s.store.channels.first().map_or("", |c| c.id.as_str()),
        ));
        let mut body = if o.type_key == "collection" {
            if members.is_empty() {
                return None;
            }
            let mut all = vec![json!({ "key": "id", "condition": "in", "value": members })];
            all.extend(filters);
            json!({ "filters": all })
        } else {
            json!({ "setId": o.id, "filters": filters })
        };
        let sorts: Vec<Json> = o
            .field("viewSorts")
            .map(|v| {
                v.items()
                    .iter()
                    .filter_map(|i| {
                        let e = i.entries()?;
                        let key = e.get("key").and_then(|v| v.as_str()).unwrap_or("");
                        if key.is_empty() {
                            return None;
                        }
                        let kind = if e.get("type").and_then(|v| v.as_str()) == Some("asc") {
                            "asc"
                        } else {
                            "desc"
                        };
                        Some(json!({ "key": key, "type": kind, "emptyPlacement": "end" }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let sorts = if sorts.is_empty() {
            vec![json!({ "key": "updatedAt", "type": "desc", "emptyPlacement": "end" })]
        } else {
            sorts
        };
        let shown: Vec<&str> = [
            "name",
            "iconEmoji",
            "done",
            o.field_str("viewGroupKey"),
            o.field_str("viewDateKey"),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
        let map = body.as_object_mut().expect("object body");
        map.insert("sorts".into(), json!(sorts));
        // Every row shows (no "+N more"); a cap only against a query matching thousands.
        map.insert("limit".into(), json!(LIST_FETCH));
        map.insert("fields".into(), json!(shown));
        return Some(body);
    }
    // Tree widget: outbound links.
    let targets = link_targets(o);
    if targets.is_empty() {
        return None;
    }
    let limit = targets.len();
    Some(
        json!({ "filters": [{ "key": "id", "condition": "in", "value": targets }], "limit": limit }),
    )
}

struct Cell {
    n: u32,
    cur: bool,
    today: bool,
    marked: bool,
}

/// The navigable Monday-first month: label, year and padded cells.
fn month(rows: &[QueryRow], date_key: &str, month_off: i32) -> (String, i32, Vec<Cell>) {
    let now = Local::now().date_naive();
    let total = now.year() * 12 + now.month0() as i32 + month_off;
    let (y, m0) = (total.div_euclid(12), total.rem_euclid(12) as u32);
    let first = NaiveDate::from_ymd_opt(y, m0 + 1, 1).expect("valid month");
    let next_first = if m0 == 11 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m0 + 2, 1)
    }
    .expect("valid month");
    let days = (next_first - first).num_days() as u32;
    let prev_days = (first
        - first
            .pred_opt()
            .expect("has previous day")
            .with_day(1)
            .expect("day 1"))
    .num_days() as u32;
    let ts_of = |r: &QueryRow| -> i64 {
        match date_key {
            "createdDate" => r.created_at,
            "modifiedDate" => r.updated_at,
            key => r
                .fields
                .get(key)
                .and_then(|v| v.int_value.or(v.float_value.map(|f| f as i64)))
                .unwrap_or(0),
        }
    };
    let mut marked = HashSet::new();
    for r in rows {
        let ts = ts_of(r);
        if ts == 0 {
            continue;
        }
        if let Some(d) = Local.timestamp_millis_opt(ts).earliest() {
            let d = d.date_naive();
            if d.year() == y && d.month0() == m0 {
                marked.insert(d.day());
            }
        }
    }
    let offset = first.weekday().num_days_from_monday();
    let is_this = y == now.year() && m0 == now.month0();
    let mut cells = Vec::new();
    for i in 0..offset {
        cells.push(Cell {
            n: prev_days - offset + 1 + i,
            cur: false,
            today: false,
            marked: false,
        });
    }
    for i in 1..=days {
        cells.push(Cell {
            n: i,
            cur: true,
            today: is_this && i == now.day(),
            marked: marked.contains(&i),
        });
    }
    let mut next = 1;
    while cells.len() % 7 != 0 {
        cells.push(Cell {
            n: next,
            cur: false,
            today: false,
            marked: false,
        });
        next += 1;
    }
    (first.format("%B").to_string(), y, cells)
}

fn open(id: String) -> impl Fn(&mut State) + 'static {
    move |s: &mut State| navigate(Route::Object(id.clone()), s)
}

/// Step the calendar widget of `id` by `delta` months.
fn step_month(id: String, delta: i32) -> impl Fn(&mut State) + 'static {
    move |s: &mut State| {
        if let Some(w) = s.slot::<Pinned>().widgets.get_mut(&id) {
            w.month_off += delta;
        }
    }
}

/// The widget body of pin `id`, `width` the width of the column the body sits
/// in (inside its card's padding and border; the gallery fits its columns to it).
pub(crate) fn widget(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    id: &str,
    width: f32,
    layer: &mut Vec<El>,
) -> El {
    let Some(w) = s.get::<Pinned>().and_then(|p| p.widgets.get(id)) else {
        return el();
    };
    let Some(obj) = w.obj.clone() else {
        return el();
    };
    let set = is_set(&obj);
    let view_type = if set && !obj.field_str("viewType").is_empty() {
        obj.field_str("viewType")
    } else {
        "table"
    };
    // Lite: the space is downloading - its progress (`lite_ui`'s text,
    // shimmering) over skeleton rows instead of partial rows.
    if let Some(space) = &w.partial_space {
        return match lite_ui::loading_line(space, s) {
            Some(line) => col()
                .px(8.)
                .gap(8.)
                .pb(6.)
                .child(
                    el().py(6.).child(
                        ShimmerText::new(format!("pin-load-{id}"), line)
                            .font(ty::ui(12.))
                            .color(t.muted)
                            .build(t),
                    ),
                )
                .child(Skeleton::new().build(t).h(10.).w_frac(0.8).radius(5.))
                .child(Skeleton::new().build(t).h(10.).w_frac(0.55).radius(5.)),
            None => {
                let mut out = col().px(8.);
                if let Some(notice) = lite_ui::space_notice(space, s) {
                    out = out.child(crate::sidebar::notice_row(t, notice, &format!("pin-{id}")));
                }
                out
            }
        };
    }
    if w.rows.is_empty() && view_type != "calendar" {
        return el();
    }
    let current_id = crate::shell::open_object_id(s).unwrap_or_default();
    let active = s.nav.space().to_string();
    let group_key = obj.field_str("viewGroupKey").to_string();
    let date_key = if obj.field_str("viewDateKey").is_empty() {
        "createdDate".to_string()
    } else {
        obj.field_str("viewDateKey").to_string()
    };
    let group_rel = space_rels(s, &obj).into_iter().find(|r| r.key == group_key);
    let row_icon = |r: &QueryRow| {
        object_icon(
            &s.store,
            field_str(&r.fields, "iconEmoji"),
            &r.type_key,
            &active,
        )
    };
    let row_name = |r: &QueryRow| crate::shell::or_untitled(field_str(&r.fields, "name"));
    let body = col().gap(1.).pt(2.);

    if view_type == "kanban"
        && let Some(rel) = group_rel
    {
        return kanban(t, id, &w.rows, &rel, &group_key, body);
    }
    if view_type == "calendar" {
        return body.child(calendar(t, id, &w.rows, &date_key, w.month_off));
    }
    if view_type == "gallery" {
        let shown: Vec<&QueryRow> = w.rows.iter().take(GALLERY_LIMIT).collect();
        // As many columns (up to three) as leave every card's content room
        // (the grid's 8 pt right padding off the body's column, 8 pt gaps).
        let inner = width - 8.;
        let needs: Vec<f32> = shown
            .iter()
            .map(|r| {
                2. + 24.
                    + icon_width(&row_icon(r), ty::ui(15.).medium())
                    + 8.
                    + text_width(&row_name(r), ty::ui(13.).medium())
            })
            .collect();
        // Never more columns than cards: two cards share the width, not two thirds of it.
        let cols = gallery_columns(inner, &needs).min(shown.len().max(1));
        let drag_list = manual_order(&obj).map(|_| items_list(id));
        let list = drag_list.as_deref();
        item_ghost(s, t, id, list, &w.rows, layer);
        let mut items: Vec<(Option<String>, El)> = shown
            .iter()
            .map(|r| {
                let card = card(t, format!("w-card:{}", r.id), r.id == current_id)
                    .child(icon_text(s, &row_icon(r), 15., ty::ui(15.).medium()).color(t.fg))
                    .child(
                        text(row_name(r))
                            .font(ty::ui(13.).medium())
                            .color(t.fg)
                            .truncate()
                            .shrink()
                            .min_w(0.),
                    );
                (Some(r.id.clone()), item(s, card, id, &r.id, list, Axis::X))
            })
            .collect();
        if w.rows.len() > GALLERY_LIMIT {
            items.push((
                None,
                card(t, "w-card-more".into(), false)
                    .justify_center()
                    .border_dash(2., 1.)
                    .child(
                        text(format!("+{} more", w.rows.len() - GALLERY_LIMIT))
                            .font(ty::ui(13.))
                            .color(t.muted)
                            .truncate(),
                    )
                    .on_press(open(id.to_string())),
            ));
        }
        let mut grid = col().gap(8.).pt(4.).pr(8.).pb(6.);
        let mut items = items.into_iter().peekable();
        while items.peek().is_some() {
            let mut line = row().gap(8.);
            for _ in 0..cols {
                line = line.child(match items.next() {
                    Some((row_id, item)) => {
                        let mut cell = row().grow(1.).min_w(0.).child(item);
                        if let Some(mark) = row_id.and_then(|r| drop_line(s, t, list, &r, Axis::X))
                        {
                            cell = cell.child(mark);
                        }
                        cell
                    }
                    None => el().grow(1.),
                });
            }
            grid = grid.child(line);
        }
        return body.child(grid);
    }

    // Only the rows in and near the window are drawn, spacers standing for
    // the rest: a list of hundreds would be laid out whole on every view
    // run. The list's window position is read in steps of half a window, so
    // scrolling re-runs the view only every half window; the drawn rows
    // (one window of overscan each way) always cover the visible ones. Until
    // the list has been placed once, every row is drawn.
    let n = w.rows.len();
    let list_key = format!("w-rows-{id}");
    let view_h = crate::window_height(s);
    let drawn = if view_h > 0. {
        let step = view_h / 2.;
        cx.window_layout_with(&list_key, move |r| (r[1] / step).round() as i64)
            .map_or(0..n, |q| rows_near(n, q as f32 * step, view_h, OVERSCAN))
    } else {
        0..n
    };
    let drag_list = manual_order(&obj).map(|_| items_list(id));
    let list = drag_list.as_deref();
    item_ghost(s, t, id, list, &w.rows, layer);
    let mut rows = col()
        .key(list_key)
        .child(el().h(ROW_PITCH * drawn.start as f32));
    for r in &w.rows[drawn.clone()] {
        let current = r.id == current_id;
        let row_line = line(t, format!("w-row-{}", r.id), current)
            .child(list_text(
                t,
                icon_text(s, &row_icon(r), 11., ty::ui(11.)),
                current,
            ))
            .child(
                list_text(t, text(row_name(r)).font(ty::ui(12.)), current)
                    .truncate()
                    .shrink()
                    .min_w(0.),
            );
        let mut cell = col()
            .pb(1.)
            .child(item(s, row_line, id, &r.id, list, Axis::Y));
        if let Some(mark) = drop_line(s, t, list, &r.id, Axis::Y) {
            cell = cell.child(mark);
        }
        rows = rows.child(cell);
    }
    rows = rows.child(el().h(ROW_PITCH * (n - drawn.end) as f32));
    body.child(rows)
}

/// The drag list of pin `pin`'s items.
fn items_list(pin: &str) -> String {
    format!("pin-items:{pin}")
}

/// Item `item` of pin `pin` (`el`, its row or card) opens on a press. In a
/// collection in its own order (`list`, its items' drag list) it also drags -
/// on the phone after a long press - to reorder the members; the dragged one fades.
fn item(s: &State, el: El, pin: &str, item: &str, list: Option<&str>, axis: Axis) -> El {
    let Some(list) = list else {
        return el.on_press(open(item.to_string()));
    };
    let (open_id, press_list) = (item.to_string(), list.to_string());
    let mut el = el.on_press(move |s: &mut State| {
        // A press that lifted the item is never navigation.
        if drag::take_swallowed_press(s, &press_list) || drag::list(s, &press_list).is_some() {
            return;
        }
        navigate(Route::Object(open_id.clone()), s)
    });
    if drag::dragging(s, list) == Some(item) {
        el = el.opacity(0.4);
    }
    let pin = pin.to_string();
    let drop = move |s: &mut State, dragged: &str, target: &str, after: bool| {
        reorder(s, &pin, dragged, target, after)
    };
    if crate::is_mobile(s) {
        crate::mobile::held_item(el, list, item, axis, drop)
    } else {
        drag::item(el, list, item, axis, drop)
    }
}

/// The 2 pt accent line where the item dragged in `list` lands beside `item`:
/// across the 1 pt gap above or below a row (`Axis::Y`), or down the 8 pt gap
/// left or right of a gallery card (`Axis::X`). Its cell carries it, outside
/// the row's clip.
fn drop_line(s: &State, t: &Theme, list: Option<&str>, item: &str, axis: Axis) -> Option<El> {
    let after = drag::over(s, list?, item)?;
    let mark = match (axis, after) {
        (Axis::Y, false) => el().pin(Some(4.), Some(-1.5), Some(4.), None).h(2.),
        (Axis::Y, true) => el().pin(Some(4.), None, Some(4.), Some(-0.5)).h(2.),
        (Axis::X, false) => el().pin(Some(-5.), Some(4.), None, Some(4.)).w(2.),
        (Axis::X, true) => el().pin(None, Some(4.), Some(-5.), Some(4.)).w(2.),
    };
    Some(
        mark.key(format!("w-drop:{item}"))
            .radius(1.)
            .fill(t.accent)
            .inert(),
    )
}

/// The floating copy of the item dragged in `list` (one of `rows`).
fn item_ghost(
    s: &State,
    t: &Theme,
    pin: &str,
    list: Option<&str>,
    rows: &[QueryRow],
    layer: &mut Vec<El>,
) {
    let Some(list) = list else { return };
    let Some(r) = drag::dragging(s, list).and_then(|d| rows.iter().find(|r| r.id == d)) else {
        return;
    };
    let icon = object_icon(
        &s.store,
        field_str(&r.fields, "iconEmoji"),
        &r.type_key,
        s.nav.space(),
    );
    let ghost = row()
        .key(format!("w-ghost:{pin}"))
        .items_center()
        .gap(6.)
        .px(8.)
        .h(28.)
        .radius(radius::MENU)
        .fill(t.panel)
        .border(1., t.border)
        .opacity(0.8)
        .child(icon_text(s, &icon, 12., ty::ui(12.)).color(t.fg))
        .child(
            text(crate::shell::or_untitled(field_str(&r.fields, "name")))
                .font(ty::ui(12.).medium())
                .color(t.fg),
        );
    drag::ghost(s, list, ghost, layer);
}

/// Item `dragged` of collection pin `pin` let go beside `target`: the
/// widget shows the new order at once and the collection's member list
/// (`collectionIds`) takes it. A drop in place writes nothing.
fn reorder(s: &mut State, pin: &str, dragged: &str, target: &str, after: bool) {
    let Some(w) = s.slot::<Pinned>().widgets.get_mut(pin) else {
        return;
    };
    let Some(next) = w
        .obj
        .as_deref()
        .and_then(manual_order)
        .and_then(|members| collection::moved(&members, dragged, target, after))
    else {
        return;
    };
    collection::sort_by_members(&mut w.rows, &next);
    let value = Value::values(next.iter().map(Value::string).collect());
    if let Some(obj) = &mut w.obj {
        Arc::make_mut(obj)
            .fields
            .insert("collectionIds".into(), value.clone());
    }
    api::note::set_field(pin, "collectionIds", value, s).detach_and_log_err(&s.rt);
}

/// `.w-line`: a kit list row, 24px, 12px muted (fg under the pointer and
/// while current, the current one on `hl-med`). Its content goes in as
/// children (gap 6).
fn line(t: &Theme, key: String, current: bool) -> El {
    let mut e = row()
        .key(key)
        .role("listitem")
        .h(24.)
        .px(8.)
        .center_y()
        .gap(6.)
        .min_w(0.)
        .radius(radius::SM)
        .clip_rounded(radius::SM)
        .cursor(Cursor::Pointer)
        .hover(t.kit.list_hover)
        .hover_group();
    if current {
        e = e.fill(t.hl_med);
    }
    motion::pressable_row(e)
}

/// Text in a [`line`]: muted, fg while the line is hovered or current.
fn list_text(t: &Theme, e: El, current: bool) -> El {
    if current {
        e.color(t.fg)
    } else {
        hover_fg(e, t)
    }
}

/// A kanban widget: one line per non-empty group with its count.
fn kanban(
    t: &Theme,
    id: &str,
    rows: &[QueryRow],
    rel: &RelationDef,
    group_key: &str,
    body: El,
) -> El {
    let checkbox = rel.format == "checkbox";
    let vals_of = |r: &QueryRow| -> Vec<String> {
        let Some(v) = r.fields.get(group_key) else {
            return vec![];
        };
        if checkbox {
            return vec![if v.bool_value == Some(true) {
                "true".into()
            } else {
                "false".into()
            }];
        }
        if let Some(s) = &v.string_value {
            return if s.is_empty() {
                vec![]
            } else {
                vec![s.clone()]
            };
        }
        if v.values_value.is_some() {
            return v
                .items()
                .iter()
                .filter_map(|i| i.string_value.clone())
                .filter(|s| !s.is_empty())
                .collect();
        }
        vec![]
    };
    let defs: Vec<(String, String)> = if checkbox {
        vec![
            ("false".into(), "Not done".into()),
            ("true".into(), "Done".into()),
        ]
    } else {
        let mut d: Vec<(String, String)> = rel
            .options
            .iter()
            .map(|o| (o.text.clone(), o.text.clone()))
            .collect();
        let label = if rel.name.is_empty() {
            group_key.to_string()
        } else {
            rel.name.clone()
        };
        d.push((String::new(), format!("No {label}")));
        d
    };
    let mut body = body;
    for (i, (gid, name)) in defs.into_iter().enumerate() {
        let count = rows
            .iter()
            .filter(|r| {
                let vals = vals_of(r);
                if checkbox {
                    vals.first() == Some(&gid) || (gid == "false" && vals.is_empty())
                } else if gid.is_empty() {
                    vals.is_empty()
                } else {
                    vals.contains(&gid)
                }
            })
            .count();
        if count == 0 {
            continue;
        }
        body = body.child(
            line(t, format!("w-group-{i}"), false)
                .child(
                    hover_fg(text(name).font(ty::ui(12.)), t)
                        .truncate()
                        .grow(1.)
                        .shrink()
                        .min_w(0.),
                )
                .child(motion::rolling_count(
                    t,
                    format!("w-group-n:{id}:{gid}"),
                    count,
                    ty::ui(11.),
                    t.muted,
                ))
                .on_press(open(id.to_string())),
        );
    }
    body
}

/// A mini month: title (opens the set) with ‹ › month steps over the
/// weekday head and the day grid (weekends shaded, today accent and bold,
/// days with records dotted).
fn calendar(t: &Theme, id: &str, rows: &[QueryRow], date_key: &str, month_off: i32) -> El {
    let (label, year, cells) = month(rows, date_key, month_off);
    let weeks = cells.len() / 7;
    let mut grid = col()
        .key("w-cal-grid")
        .cursor(Cursor::Pointer)
        .on_press(open(id.to_string()));
    let mut head_row = row();
    for (i, d) in DOW.iter().enumerate() {
        let mut c = row()
            .grow(1.)
            .py(4.)
            .justify_center()
            .child(text(*d).font(ty::ui(11.)).color(t.muted));
        if i >= 5 {
            c = c.fill(t.hl_light);
        }
        if i == 5 {
            c = c.corners([8., 0., 0., 0.]);
        }
        if i == 6 {
            c = c.corners([0., 8., 0., 0.]);
        }
        head_row = head_row.child(c);
    }
    grid = grid.child(head_row);
    for w in 0..weeks {
        let last = w + 1 == weeks;
        let mut week = row();
        for (i, c) in cells[w * 7..w * 7 + 7].iter().enumerate() {
            let (color, font) = if c.today {
                (t.accent, ty::ui(12.5).bold())
            } else if c.cur {
                (t.fg, ty::ui(12.5))
            } else {
                (t.muted, ty::ui(12.5))
            };
            let mut cell = row()
                .grow(1.)
                .py(5.)
                .justify_center()
                .child(text(c.n.to_string()).font(font).color(color));
            if i >= 5 {
                cell = cell.fill(t.hl_light);
            }
            if last && i == 5 {
                cell = cell.corners([0., 0., 0., 8.]);
            }
            if last && i == 6 {
                cell = cell.corners([0., 0., 8., 0.]);
            }
            if !c.cur {
                cell = cell.opacity(0.6);
            }
            if c.marked {
                cell = cell.child(
                    row()
                        .pin(Some(0.), None, Some(0.), Some(1.))
                        .justify_center()
                        .child(el().size(3., 3.).radius(1.5).fill(t.accent)),
                );
            }
            week = week.child(cell);
        }
        grid = grid.child(week);
    }
    let nav_btn = |key: &'static str, glyph: &'static str, delta: i32| {
        motion::pressable(fill(key, rune::Color::TRANSPARENT))
            .px(5.)
            .py(2.)
            .role("button")
            .label(if delta < 0 {
                "Previous month"
            } else {
                "Next month"
            })
            .child(hover_fg(text(glyph).font(ty::ui(16.).lh(16.)), t))
            .on_press(step_month(id.to_string(), delta))
    };
    let title_font = ty::ui(15.).bold();
    col()
        .pt(2.)
        .pr(8.)
        .pb(6.)
        .child(
            row()
                .center_y()
                .justify_between()
                .pt(2.)
                .pr(2.)
                .pb(6.)
                .pl(6.)
                .child(
                    motion::pressable(fill("w-cal-title", rune::Color::TRANSPARENT))
                        .role("button")
                        .child(text(label).font(title_font).color(t.fg))
                        .child(text(year.to_string()).font(title_font).color(t.fg).ml(4.))
                        .on_press(open(id.to_string())),
                )
                .child(
                    row()
                        .gap(2.)
                        .child(nav_btn("w-cal-prev", "‹", -1))
                        .child(nav_btn("w-cal-next", "›", 1)),
                ),
        )
        .child(grid)
}

/// The pinned gallery's columns: three, two or one, the most that give every card (`needs`,
/// its content's width) room in `inner` points with 8 pt gaps.
fn gallery_columns(inner: f32, needs: &[f32]) -> usize {
    (1..=3)
        .rev()
        .find(|&n| {
            let each = (inner - 8. * (n - 1) as f32) / n as f32;
            needs.iter().all(|need| *need <= each + 0.5)
        })
        .unwrap_or(1)
}

/// A gallery card: icon and name on `hl-light` in a 1px border that
/// strengthens under the pointer (`hl-med` and a stronger border while
/// current). Callers add the content and the press.
fn card(t: &Theme, key: String, current: bool) -> El {
    let (fill_color, border) = if current {
        (t.hl_med, mix_srgb(t.border, 0.4, t.fg))
    } else {
        (t.hl_light, t.border)
    };
    let mut e = row()
        .key(key)
        .center_y()
        .gap(8.)
        .min_w(0.)
        .grow(1.)
        .px(12.)
        .py(11.)
        .radius(radius::MENU)
        .fill(fill_color)
        .border(1., border)
        .cursor(Cursor::Pointer)
        .hover_group();
    if !current {
        // The hover border, drawn over the resting one while the card is hovered. It sits
        // on the card's own edge: one point outside, the fold's clip cut its left side off.
        e = e.child(
            el().pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .radius(radius::MENU)
                .border(1., mix_srgb(t.border, 0.55, t.fg))
                .group_opacity(0., 1.)
                .inert(),
        );
    }
    motion::pressable(e)
}

/// The rows of an `n`-row list whose first row's top is at window y `top`
/// that are on a `view_h` tall window or within `overscan` windows of it.
fn rows_near(n: usize, top: f32, view_h: f32, overscan: f32) -> Range<usize> {
    let over = view_h * overscan;
    let first = (((-over - top) / ROW_PITCH).floor().max(0.) as usize).min(n);
    let last = (((view_h + over - top) / ROW_PITCH).ceil().max(0.) as usize).clamp(first, n);
    first..last
}

/// Laid-out width of `text` in `font`.
fn text_width(text: &str, font: TextStyle) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    rune::text::measure(text, &[], font, None).0
}

/// Width of a row icon at `font`: an image icon is a `font.size` square.
fn icon_width(icon: &str, font: TextStyle) -> f32 {
    if roostr_ui::is_image_icon(icon) {
        font.size
    } else {
        text_width(icon, font)
    }
}

#[cfg(test)]
mod tests {
    use super::gallery_columns;

    #[test]
    fn the_gallery_takes_the_most_columns_its_cards_fit() {
        // 400 pt: thirds are 128, halves 196.
        assert_eq!(gallery_columns(400., &[120., 100.]), 3);
        assert_eq!(gallery_columns(400., &[120., 150.]), 2);
        assert_eq!(gallery_columns(400., &[120., 250.]), 1);
        assert_eq!(gallery_columns(400., &[]), 3);
    }

    use super::*;

    #[test]
    fn rows_near_covers_the_window_and_its_overscan() {
        // 300 rows, list top at the window top, an 800pt window: two windows of rows.
        assert_eq!(rows_near(300, 0., 800., OVERSCAN), 0..64);
        assert_eq!(rows_near(300, 0., 800., 0.), 0..32);
        // Scrolled to the end: the last rows, none above the overscan.
        let top = 800. - 300. * ROW_PITCH;
        let near = rows_near(300, top, 800., OVERSCAN);
        assert_eq!(near.end, 300);
        assert!(near.start > 200);
        // Far below the window: nothing.
        assert_eq!(rows_near(300, 5000., 800., OVERSCAN), 0..0);
    }

    #[test]
    fn a_half_window_step_always_covers_the_visible_rows() {
        let view_h = 800.;
        let step = view_h / 2.;
        for top in (-8000..800).step_by(7) {
            let top = top as f32;
            let q = (top / step).round();
            let drawn = rows_near(300, q * step, view_h, OVERSCAN);
            let visible = rows_near(300, top, view_h, 0.);
            assert!(
                drawn.start <= visible.start && drawn.end >= visible.end,
                "{top}"
            );
        }
    }

    #[test]
    fn link_targets_skip_the_channel_and_repeats() {
        use roostr_core::types::{LinkValue, Value};
        let link = |id: &str| Value {
            link_value: Some(LinkValue {
                target_id: id.into(),
                ..LinkValue::default()
            }),
            ..Value::default()
        };
        let o = Object {
            fields: [
                ("channel".to_string(), link("sp-1")),
                ("a".to_string(), link("x")),
                ("b".to_string(), link("x")),
                ("c".to_string(), link("y")),
            ]
            .into(),
            ..Object::default()
        };
        assert_eq!(link_targets(&o), vec!["x".to_string(), "y".to_string()]);
    }
}

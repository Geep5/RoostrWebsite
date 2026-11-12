//! The dataview grid (SetTable.svelte): Name first, then the view's relation
//! columns (`viewRelations`), then "+". Inline cell editing, sort by header,
//! column resize / reorder / hide, row multi-select (cmd / shift / marquee),
//! paging, the Served by and Repeat virtual columns, the "+ New Object" entry
//! row, and the row context menu.
//!
//! The page scrolls the table: only the rows in and near the window are built
//! (spacers stand in for the rest), from row heights measured as they lay out.
//! Drawing never writes state; handlers and layout callbacks keep the geometry
//! the rubber band, the cell editor and paging need.

use std::collections::HashMap;
use std::rc::Rc;

use roostr_core::dates::show_date;
use roostr_core::objects::{object_icon, relation_emoji, type_icon};
use roostr_core::protected::blockers_for;
use roostr_core::relations::{current_space_id, object_space_id};
use roostr_core::repeat::describe_repeat;
use roostr_core::serving::{MachineRow, fetch_machines, machine_name, resolve_many, serving_copy};
use roostr_core::{QueryRow, RelationDef, State, Value, api, field_str, is_locked_tool, repeat_of};
use roostr_properties::{
    CredentialCell, FlowEvent, FlowItem, PropertyFlow, PropertyFlowState, PropertyValue,
    credential_cell_badge, credential_status_badge, credential_status_text, reconnect_target,
};
use roostr_rt::Task;
use roostr_ui::color::{rgb, rgba};
use roostr_ui::motion;
use roostr_ui::overlay::{self, Placement};
use roostr_ui::tags::badge_colors;
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    Button, Glyph, Input, InputState, ScrollArea, ScrollbarMode, Theme, checkbox, checkbox_display,
    danger_button, glyph, ty,
};
use rune::{
    Color, Cursor, Cx, Drag, El, Key, KeyEvent, Phase, PointerButton, PointerPress, Shadow, Tx,
    col, el, row, text,
};
use serde_json::{Value as Json, json};

use crate::common::{self, engine_body, tag_pill};
use crate::config::{self, Col, DEFAULT_WIDTH, MIN_WIDTH, NAME_MIN_WIDTH};
use crate::gallery::js_number_string;
use crate::views::ViewInput;
use crate::{DataView, dv, dv_mut};

/// Built-in columns that aren't stored fields.
const SPECIALS: &[(&str, &str)] = &[
    ("type", "Type"),
    ("createdAt", "Created"),
    ("updatedAt", "Updated"),
    ("serving", "Served by"),
    ("repeat", "Repeat"),
];
/// Icons for the built-in columns, matching the filter and sort menus.
fn special_emoji(key: &str) -> Option<&'static str> {
    match key {
        "type" => Some("▦"),
        "createdAt" | "updatedAt" => Some("🗓️"),
        "serving" => Some("🖥️"),
        "repeat" => Some("↻"),
        _ => None,
    }
}
/// Rows per page.
const PAGE: usize = 100;
/// A body row's height before one is drawn (13px text, 7px padding, border).
const ROW_H_GUESS: f32 = 33.;
/// Rows drawn beyond the window, as a share of its height on each side: a
/// wheel step or a fling between frames lands on rows already drawn.
const OVERSCAN: f32 = 1.;
/// The view re-runs as the table's top crosses a band this tall (and draws a
/// band more on each side), not on every frame of a scroll.
const BAND: f32 = 256.;
/// The window's height before rune-kit's root has laid out.
const WINDOW_H_GUESS: f32 = 900.;
/// Room under the last row for the horizontal scrollbar of a too-wide table.
const TABLE_SCROLLBAR_ROOM: f32 = 10.;
/// The header row (35 with its rule).
const HEADER_H: f32 = 35.;
/// What every row draws whatever the columns - and its space (`channel`):
/// a cell's editor offers and creates linked objects in the row's space.
const ROW_FIELDS: &[&str] = &[
    "name",
    "channel",
    "iconEmoji",
    "done",
    "status",
    "error",
    "service",
    "served_by",
    "check_requested",
    "checked_at",
];
/// The Served by column edits `served_by`.
const SERVING_KEY: &str = "served_by";
const PLUS_W: f32 = 32.;
/// How far ← / → scroll a hovered table: about one column.
const ARROW_STEP: f32 = 160.;
/// A selected row's fill and the rubber band's.
const SELECTED: u32 = 0x377aff40;
const BAND_FILL: u32 = 0x377aff1f;
const BAND_BORDER: u32 = 0x377aff8c;
/// The error / warning text colour.
const WARN: u32 = 0xf55522;

#[derive(Clone, Debug)]
struct ServingInfo {
    text: String,
    label: String,
    warning: bool,
    reason: String,
    machine_id: String,
    icon: String,
}

fn serving_attention(info: Option<&ServingInfo>) -> bool {
    info.is_some_and(|i| i.warning || i.reason == "unserved")
}

fn serving_match(condition: &str, value: &str, info: Option<&ServingInfo>) -> bool {
    let hit = match value {
        "attention" => serving_attention(info),
        "ok" => info.is_some() && !serving_attention(info),
        _ => info.is_some_and(|i| i.reason == value),
    };
    if condition == "notEqual" { !hit } else { hit }
}

/// Cannot be honoured first, then unserved, then fine.
fn serving_rank(info: Option<&ServingInfo>) -> i32 {
    match info {
        None => 1,
        Some(i) if i.warning => 0,
        Some(i) if !i.machine_id.is_empty() => 2,
        Some(_) => 1,
    }
}

/// A live resize gesture: `None` column = the Name column.
struct Resize {
    col: Option<usize>,
    start_w: f32,
    snapshot: Vec<Col>,
}

/// A header drag (column reorder): where it came from, the gap it would drop
/// into, and the pointer for its ghost.
struct ColDrag {
    from: usize,
    label: String,
    gap: Option<usize>,
    /// The drop line's x in the table's own points.
    gap_x: Option<f32>,
    /// The pointer (window) and where in the cell it grabbed.
    at: [f32; 2],
    grab: [f32; 2],
}

/// The rubber band, in window points.
struct Marquee {
    x0: [f32; 2],
    x1: [f32; 2],
}

pub(crate) struct Table {
    input: ViewInput,
    rows: Vec<QueryRow>,
    total: usize,
    page_query: Option<Json>,
    loading_more: bool,
    generation: u64,
    selected: Vec<String>,
    anchor: String,
    marquee: Option<Marquee>,
    drag_moved: bool,
    cell_edit: Option<(String, String)>,
    sel_blocked: HashMap<String, String>,
    override_sort: Option<(String, bool)>,
    local_cols: Option<Vec<Col>>,
    name_local: Option<f32>,
    adding: Option<PropertyFlowState>,
    col_drag: Option<ColDrag>,
    resize: Option<Resize>,
    serving_by_id: HashMap<String, ServingInfo>,
    machines_now: Vec<MachineRow>,
    pending_new: bool,
    new_input: InputState,
    /// The entry field has had focus since it opened (its blur commits).
    new_focused: bool,
    committing: bool,
    /// Each drawn row's height as it laid out, and their mean for the rest.
    heights: HashMap<String, f32>,
    row_h: f32,
    /// The rows block's top and the table's left edge, in window points, and
    /// the horizontal viewport's rect (the visible width of the table).
    rows_top: f32,
    table_left: f32,
    clip: [f32; 4],
    /// The table's laid-out width (the content the viewport scrolls).
    table_w: f32,
    /// The pointer is over the table (← / → scroll it).
    hovered: bool,
}

impl Table {
    pub(crate) fn new(input: ViewInput) -> Self {
        Self {
            input,
            rows: Vec::new(),
            total: 0,
            page_query: None,
            loading_more: false,
            generation: 0,
            selected: Vec::new(),
            anchor: String::new(),
            marquee: None,
            drag_moved: false,
            cell_edit: None,
            sel_blocked: HashMap::new(),
            override_sort: None,
            local_cols: None,
            name_local: None,
            adding: None,
            col_drag: None,
            resize: None,
            serving_by_id: HashMap::new(),
            machines_now: Vec::new(),
            pending_new: false,
            new_input: InputState::new(),
            new_focused: false,
            committing: false,
            heights: HashMap::new(),
            row_h: ROW_H_GUESS,
            rows_top: 0.,
            table_left: 0.,
            clip: [0.; 4],
            table_w: 0.,
            hovered: false,
        }
    }

    pub(crate) fn row_ids(&self) -> impl Iterator<Item = &str> {
        self.rows.iter().map(|r| r.id.as_str())
    }

    /// Escape: close the cell editor, clear the selection.
    pub(crate) fn dismiss(&mut self) -> bool {
        let any = self.cell_edit.is_some() || !self.selected.is_empty() || self.adding.is_some();
        self.cell_edit = None;
        self.adding = None;
        self.selected.clear();
        self.sel_blocked.clear();
        any
    }

    fn sort_key_dir(&self) -> (String, bool) {
        if let Some((k, d)) = &self.override_sort {
            return (k.clone(), *d);
        }
        match self.input.sorts.first() {
            Some(s) => (s.key.clone(), s.desc),
            None => ("updatedAt".into(), true),
        }
    }

    /// Row `i`'s height: measured, else the mean of those measured.
    fn height_of(&self, r: &QueryRow) -> f32 {
        self.heights.get(&r.id).copied().unwrap_or(self.row_h)
    }

    /// The rows' tops from the rows block's top (one longer: the last is the
    /// bottom of the last row).
    /// The horizontal viewport's offset and its furthest, from where the
    /// table and the viewport last laid out (the arrows and the scrollbar
    /// wheel step from it).
    fn hscroll(&self) -> (f32, f32) {
        let max = (self.table_w - self.clip[2]).max(0.);
        ((self.clip[0] - self.table_left).clamp(0., max), max)
    }

    fn tops(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.rows.len() + 1);
        let mut y = 0.;
        out.push(0.);
        for r in &self.rows {
            y += self.height_of(r);
            out.push(y);
        }
        out
    }
}

fn table<'a>(s: &'a State, id: &str) -> Option<&'a Table> {
    dv(s, id).and_then(DataView::table)
}

fn table_mut<'a>(s: &'a mut State, id: &str) -> Option<&'a mut Table> {
    dv_mut(s, id).and_then(DataView::table_mut)
}

fn stored_cols(s: &State, id: &str) -> Vec<Col> {
    s.store
        .object(id)
        .map(|o| config::stored_columns(&o.fields))
        .unwrap_or_default()
}

fn stored_name_width(s: &State, id: &str) -> Option<f32> {
    s.store
        .object(id)
        .and_then(|o| config::stored_name_width(&o.fields))
}

fn cols_of(s: &State, id: &str, t: &Table) -> Vec<Col> {
    t.local_cols.clone().unwrap_or_else(|| stored_cols(s, id))
}

fn cols(s: &State, id: &str) -> Vec<Col> {
    table(s, id).map_or_else(Vec::new, |t| cols_of(s, id, t))
}

pub(crate) fn set_input(s: &mut State, id: &str, input: ViewInput) {
    let Some(t) = table_mut(s, id) else { return };
    // The table reloads on its body and sorts (the page's own refresh reloads it too).
    let reload = input.body != t.input.body || input.sorts != t.input.sorts;
    t.input = input;
    if reload {
        load(s, id, false);
    }
}

// ── Loading ──────────────────────────────────────────────────────

/// The engine sorts and filters; the table fetches a page at a time. Only
/// what the engine can't compute (Served by, a sort by next repeat) needs
/// every row. `keep_count`: a refresh keeps the rows already scrolled in.
pub(crate) fn load(s: &mut State, id: &str, keep_count: bool) {
    let columns: Vec<String> = cols(s, id).into_iter().map(|c| c.key).collect();
    let Some(t) = table_mut(s, id) else { return };
    let Some(body) = t.input.body.clone() else {
        return;
    };
    t.generation += 1;
    let generation = t.generation;
    let rules = common::body_serving_rules(&body);
    let (key, desc) = t.sort_key_dir();
    let dir = if desc { "desc" } else { "asc" };
    let serving_sort = key == "serving";
    let repeat_sort = key == "repeat";
    let sorts: Vec<Json> = if serving_sort || repeat_sort {
        vec![json!({"key": "updatedAt", "type": "desc", "emptyPlacement": "end"})]
    } else if t.override_sort.is_some() {
        vec![json!({"key": key, "type": dir, "emptyPlacement": "end"})]
    } else if !t.input.sorts.is_empty() {
        t.input
            .sorts
            .iter()
            .map(|s| json!({"key": s.key, "type": s.type_str(), "emptyPlacement": "end"}))
            .collect()
    } else {
        vec![json!({"key": "updatedAt", "type": "desc", "emptyPlacement": "end"})]
    };
    // A collection in its own order: the engine can't sort by it, so every row.
    let members = t
        .input
        .members
        .clone()
        .filter(|_| t.override_sort.is_none());
    let needs_every_row = members.is_some()
        || serving_sort
        || repeat_sort
        || !rules.is_empty()
        || columns.iter().any(|c| c == "serving");
    let view = id.to_string();
    if !needs_every_row {
        let mut fields: Vec<String> = ROW_FIELDS.iter().map(|s| (*s).to_string()).collect();
        for c in &columns {
            if !fields.contains(c) {
                fields.push(c.clone());
            }
        }
        let query = engine_body(&body, json!({"sorts": sorts, "fields": fields}));
        let limit = if keep_count {
            PAGE.max(t.rows.len())
        } else {
            PAGE
        };
        let task = api::query(engine_body(&query, json!({"offset": 0, "limit": limit})), s);
        s.rt.spawn(async move |cx| {
            let res = match task.await {
                Ok(res) => res,
                Err(error) => {
                    log::error!("table rows: {error:#}");
                    return;
                }
            };
            cx.update(|s| {
                let Some(t) = table_mut(s, &view) else { return };
                if t.generation != generation {
                    return;
                }
                t.page_query = Some(query);
                t.rows = res.records;
                t.total = res.total as usize;
                after_rows(t);
            });
        })
        .detach();
        return;
    }
    t.page_query = None;
    let task = api::query(engine_body(&body, json!({"sorts": sorts})), s);
    let with_serving = columns.iter().any(|c| c == "serving") || !rules.is_empty() || serving_sort;
    s.rt.spawn(async move |cx| {
        let mut all = match task.await {
            Ok(res) => res.records,
            Err(error) => {
                log::error!("table rows: {error:#}");
                return;
            }
        };
        if let Some(members) = &members {
            roostr_core::collection::sort_by_members(&mut all, members);
        }
        if repeat_sort {
            // One-off objects last either way.
            all.sort_by(|a, b| {
                let x = repeat_of(&a.fields).map(|r| r.next);
                let y = repeat_of(&b.fields).map(|r| r.next);
                match (x, y) {
                    (Some(x), Some(y)) => {
                        if desc {
                            y.cmp(&x)
                        } else {
                            x.cmp(&y)
                        }
                    }
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (Some(_), None) => std::cmp::Ordering::Less,
                }
            });
        }
        let mut serving: Option<(HashMap<String, ServingInfo>, Vec<MachineRow>)> = None;
        if with_serving {
            let machines_task = cx.read(fetch_machines);
            let (machine_rows, machines) = match machines_task.await {
                Ok(m) => m,
                Err(error) => {
                    log::error!("machines: {error:#}");
                    return;
                }
            };
            let ids: Vec<String> = all.iter().map(|r| r.id.clone()).collect();
            let resolve = cx.read(|s| resolve_many(&ids, s));
            let resolved = resolve.await.unwrap_or_default();
            let icons: Vec<String> = cx.read(|s| {
                let space = current_space_id(s);
                resolved
                    .iter()
                    .map(|r| {
                        if r.machine_id.is_empty() {
                            return String::new();
                        }
                        let emoji = machine_rows
                            .iter()
                            .find(|m| field_str(&m.fields, "machine_id") == r.machine_id)
                            .map(|m| field_str(&m.fields, "iconEmoji").to_string())
                            .unwrap_or_default();
                        object_icon(&s.store, &emoji, "machine", &space)
                    })
                    .collect()
            });
            let mut map = HashMap::new();
            for (i, r) in all.iter().enumerate() {
                let sv = resolved.get(i).cloned().unwrap_or_default();
                let copy = serving_copy(&sv, &machines);
                let label = if sv.machine_id.is_empty() {
                    "No machine".to_string()
                } else {
                    machine_name(&machines, &sv.machine_id)
                };
                map.insert(
                    r.id.clone(),
                    ServingInfo {
                        text: copy.text,
                        label,
                        warning: copy.warning,
                        reason: sv.reason.clone(),
                        machine_id: sv.machine_id.clone(),
                        icon: icons.get(i).cloned().unwrap_or_default(),
                    },
                );
            }
            if !rules.is_empty() {
                all.retain(|r| {
                    rules
                        .iter()
                        .all(|(c, v)| serving_match(c, v, map.get(&r.id)))
                });
            }
            if serving_sort {
                let d = if desc { -1 } else { 1 };
                all.sort_by(|a, b| {
                    let ra = serving_rank(map.get(&a.id));
                    let rb = serving_rank(map.get(&b.id));
                    (d * (ra - rb))
                        .cmp(&0)
                        .then(b.updated_at.cmp(&a.updated_at))
                });
            }
            serving = Some((map, machines));
        }
        cx.update(|s| {
            let Some(t) = table_mut(s, &view) else { return };
            if t.generation != generation {
                return;
            }
            if let Some((map, machines)) = serving {
                t.serving_by_id = map;
                t.machines_now = machines;
            }
            t.total = all.len();
            t.rows = all;
            after_rows(t);
        });
    })
    .detach();
}

/// The next page of a paged view, in the same order.
fn load_more(s: &mut State, id: &str) {
    let Some(t) = table_mut(s, id) else { return };
    let Some(query) = t.page_query.clone() else {
        return;
    };
    if t.loading_more || t.rows.len() >= t.total {
        return;
    }
    t.loading_more = true;
    let generation = t.generation;
    let offset = t.rows.len();
    let task = api::query(
        engine_body(&query, json!({"offset": offset, "limit": PAGE})),
        s,
    );
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        let res = task.await;
        cx.update(|s| {
            let Some(t) = table_mut(s, &view) else { return };
            t.loading_more = false;
            if t.generation != generation {
                return;
            }
            match res {
                Ok(res) => {
                    for r in res.records {
                        if !t.rows.iter().any(|x| x.id == r.id) {
                            t.rows.push(r);
                        }
                    }
                    t.total = res.total as usize;
                }
                Err(error) => log::error!("table page: {error:#}"),
            }
        });
    })
    .detach();
}

fn after_rows(t: &mut Table) {
    let ids: Vec<&str> = t.rows.iter().map(|r| r.id.as_str()).collect();
    t.selected.retain(|s| ids.contains(&s.as_str()));
    t.heights.retain(|k, _| ids.contains(&k.as_str()));
}

/// After a change: the same view, the same rows scrolled into view.
pub(crate) fn reload(s: &mut State, id: &str) {
    load(s, id, true);
}

/// Await a mutation, then reload.
fn after(s: &mut State, id: &str, task: Task<anyhow::Result<roostr_core::JsonMap>>) {
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        if let Err(error) = task.await {
            log::error!("{error:#}");
        }
        cx.update(|s| reload(s, &view));
    })
    .detach();
}

/// Per frame: the entry row's blur commits it (GPUI's `InputEvent::Blur`).
pub(crate) fn poll(s: &mut State, id: &str) -> bool {
    let Some(t) = table_mut(s, id) else {
        return false;
    };
    if !t.pending_new {
        return false;
    }
    if t.new_input.is_focused() {
        t.new_focused = true;
        return false;
    }
    if !t.new_focused {
        return false;
    }
    let empty = t.new_input.value().trim().is_empty();
    commit_new(s, id, empty);
    true
}

// ── Selection ────────────────────────────────────────────────────

fn set_selection(s: &mut State, id: &str, ids: Vec<String>) {
    let Some(t) = table_mut(s, id) else { return };
    if ids == t.selected {
        return;
    }
    t.selected = ids.clone();
    let task = blockers_for(&ids, s);
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        if let Ok(map) = task.await {
            cx.update(|s| {
                if let Some(t) = table_mut(s, &view)
                    && t.selected == ids
                {
                    t.sel_blocked = map;
                }
            });
        }
    })
    .detach();
}

/// The row menu closed: its selection goes with it.
pub(crate) fn clear_selection(s: &mut State, id: &str) {
    set_selection(s, id, Vec::new());
}

fn toggle_row(s: &mut State, id: &str, row: &str) {
    let Some(t) = table_mut(s, id) else { return };
    t.anchor = row.to_string();
    let next = if t.selected.iter().any(|x| x == row) {
        t.selected.iter().filter(|x| *x != row).cloned().collect()
    } else {
        let mut n = t.selected.clone();
        n.push(row.to_string());
        n
    };
    set_selection(s, id, next);
}

fn range_rows(s: &mut State, id: &str, row: &str) {
    let Some(t) = table(s, id) else { return };
    let anchor = if t.anchor.is_empty() {
        t.selected.first().cloned().unwrap_or_default()
    } else {
        t.anchor.clone()
    };
    let a = t.rows.iter().position(|r| r.id == anchor);
    let b = t.rows.iter().position(|r| r.id == row);
    let (Some(a), Some(b)) = (a, b) else {
        toggle_row(s, id, row);
        return;
    };
    let next = t.rows[a.min(b)..=a.max(b)]
        .iter()
        .map(|r| r.id.clone())
        .collect();
    set_selection(s, id, next);
}

/// A row press: cmd/ctrl toggles it, shift extends to it, a plain press
/// clears a selection or opens the row; the middle button opens it in a tab.
fn on_row_press(s: &mut State, id: &str, row: &str, tx: &mut Tx) {
    if tx.button() == PointerButton::Middle {
        common::open_object_tab(row, s);
        return;
    }
    let m = tx.mods();
    if m.cmd || m.ctrl {
        toggle_row(s, id, row);
        return;
    }
    if m.shift {
        range_rows(s, id, row);
        return;
    }
    let Some(t) = table_mut(s, id) else { return };
    // The press that ends a drag must not clear what the drag just selected.
    if t.drag_moved {
        t.drag_moved = false;
        return;
    }
    if !t.selected.is_empty() {
        set_selection(s, id, Vec::new());
        return;
    }
    common::open_object(row, s);
}

/// The rubber band: a press-and-drag anywhere in the data view below its
/// chrome (and from the page around it, [`crate::page_drag`]) selects the
/// rows it crosses. Presses with cmd/ctrl/shift, other buttons and presses on
/// the header don't arm it. A plain press on the whitespace around the rows
/// clears the selection.
pub(crate) fn marquee(s: &mut State, id: &str, d: &Drag, tx: &mut Tx) {
    if tx.button() != PointerButton::Primary {
        return;
    }
    let [x, y] = d.window_point();
    let Some(t) = table_mut(s, id) else { return };
    match d.phase {
        Phase::Down => {
            let m = tx.mods();
            // Above the rows: the controls, the header.
            if m.cmd || m.ctrl || m.shift || y < t.rows_top {
                return;
            }
            t.marquee = Some(Marquee {
                x0: [x, y],
                x1: [x, y],
            });
            t.drag_moved = false;
        }
        Phase::Move => {
            let Some(m) = &mut t.marquee else { return };
            m.x1 = [x, y];
            let (x0, x1) = (m.x0, m.x1);
            if (x1[0] - x0[0]).abs() + (x1[1] - x0[1]).abs() < 4. {
                return;
            }
            t.drag_moved = true;
            let (top, bottom) = (x0[1].min(x1[1]), x0[1].max(x1[1]));
            let (left, right) = (x0[0].min(x1[0]), x0[0].max(x1[0]));
            let tops = t.tops();
            let row_left = t.clip[0];
            let row_right = t.clip[0] + t.clip[2];
            let hit: Vec<String> = t
                .rows
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    let (rt, rb) = (t.rows_top + tops[*i], t.rows_top + tops[i + 1]);
                    rb >= top && rt <= bottom && row_right >= left && row_left <= right
                })
                .map(|(_, r)| r.id.clone())
                .collect();
            set_selection(s, id, hit);
        }
        Phase::Up => {
            let Some(m) = t.marquee.take() else { return };
            // A plain click on the whitespace around the rows clears, like a plain row click.
            let tops = t.tops();
            let on_row = m.x0[1] >= t.rows_top
                && m.x0[1] <= t.rows_top + tops[t.rows.len()]
                && m.x0[0] >= t.clip[0]
                && m.x0[0] <= t.clip[0] + t.clip[2];
            if !t.drag_moved && !t.selected.is_empty() && !on_row {
                set_selection(s, id, Vec::new());
            }
        }
    }
}

/// [`crate::page_drag`]: a press on the page around the data view.
pub(crate) fn page_drag(s: &mut State, id: &str, d: &Drag, tx: &mut Tx) {
    marquee(s, id, d, tx);
}

// ── Cells ────────────────────────────────────────────────────────

fn is_editable(relations: &[RelationDef], key: &str) -> bool {
    // Repeat is a rule edited in the object's Repeat row, not a cell value.
    key != "repeat" && common::find_rel(relations, key).is_some_and(|r| !r.read_only)
}

fn format_of<'a>(relations: &'a [RelationDef], key: &str) -> &'a str {
    common::find_rel(relations, key).map_or("", |r| r.format.as_str())
}

/// A press on cell `key` of `record`: a checkbox toggles, any other opens
/// (or moves, or closes) its editor.
fn on_cell_press(s: &mut State, id: &str, record: &str, key: &str, checkbox: bool) {
    let Some(t) = table_mut(s, id) else { return };
    // The release ending a row-drag must not open an editor.
    if t.drag_moved {
        t.drag_moved = false;
        return;
    }
    if checkbox {
        // Checkbox cells toggle on press, no menu.
        let cur = t
            .rows
            .iter()
            .find(|r| r.id == record)
            .and_then(|r| r.fields.get(key))
            .and_then(|v| v.bool_value)
            == Some(true);
        let task = api::note::set_field(record, key, Value::bool(!cur), s);
        after(s, id, task);
        return;
    }
    let same = t
        .cell_edit
        .as_ref()
        .is_some_and(|(r, k)| r == record && k == key);
    let open = !same;
    if t.cell_edit.is_some() {
        roostr_properties::commit_pending(s);
    }
    if let Some(t) = table_mut(s, id) {
        t.cell_edit = open.then(|| (record.to_string(), key.to_string()));
    }
}

fn close_editor(s: &mut State, id: &str) {
    roostr_properties::commit_pending(s);
    if let Some(t) = table_mut(s, id) {
        t.cell_edit = None;
    }
}

fn cell_save(s: &mut State, id: &str, v: Value) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    let relations = common::scoped_relations(s, &object);
    let Some(t) = table_mut(s, id) else { return };
    let Some((record, key)) = t.cell_edit.clone() else {
        return;
    };
    let f = format_of(
        &relations,
        if key == "serving" { SERVING_KEY } else { &key },
    );
    // Single-value editors close on save; tag/object stay open for more picks.
    if f != "tag" && f != "object" {
        t.cell_edit = None;
    }
    let task = if key == "serving" {
        // The picker links a Computer object; served_by holds its machine_id.
        let target = v
            .link_value
            .as_ref()
            .map(|l| l.target_id.clone())
            .or_else(|| {
                v.items()
                    .first()
                    .and_then(|i| i.link_value.as_ref().map(|l| l.target_id.clone()))
            })
            .unwrap_or_default();
        let machine_id = t
            .machines_now
            .iter()
            .find(|m| m.id == target)
            .map(|m| m.machine_id.clone())
            .unwrap_or_default();
        if machine_id.is_empty() {
            api::note::delete_field(&record, SERVING_KEY, s)
        } else {
            api::note::set_field(&record, SERVING_KEY, Value::string(machine_id), s)
        }
    } else {
        api::note::set_field(&record, &key, v, s)
    };
    after(s, id, task);
}

fn cell_text(s: &State, t: &Table, r: &QueryRow, key: &str, relations: &[RelationDef]) -> String {
    match key {
        "serving" => {
            return t
                .serving_by_id
                .get(&r.id)
                .map(|s| s.label.clone())
                .unwrap_or_default();
        }
        "repeat" => {
            return repeat_of(&r.fields)
                .map(|rule| describe_repeat(&rule))
                .unwrap_or_default();
        }
        "type" => return common::type_name(s, &r.type_key),
        "createdAt" => {
            return if r.created_at != 0 {
                show_date(r.created_at)
            } else {
                String::new()
            };
        }
        "updatedAt" => {
            return if r.updated_at != 0 {
                show_date(r.updated_at)
            } else {
                String::new()
            };
        }
        _ => {}
    }
    let Some(v) = r.fields.get(key) else {
        return String::new();
    };
    let format = format_of(relations, key);
    // Object values come as a list, a bare link, or a bare id: all read as names.
    if format == "object" || v.link_value.is_some() {
        return common::link_names(s, v);
    }
    if let Some(s) = &v.string_value {
        return s.clone();
    }
    if let Some(b) = v.bool_value {
        return if b { "✓".into() } else { String::new() };
    }
    if let Some(n) = v.int_value {
        return if format == "date" {
            show_date(n)
        } else {
            n.to_string()
        };
    }
    if let Some(f) = v.float_value {
        return js_number_string(f);
    }
    if v.values_value.is_some() {
        return v
            .items()
            .iter()
            .map(|i| i.string_value.clone().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(", ");
    }
    if let Some(l) = &v.list_value {
        return l.values.join(", ");
    }
    String::new()
}

fn col_name(key: &str, relations: &[RelationDef]) -> String {
    SPECIALS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, n)| (*n).to_string())
        .or_else(|| common::find_rel(relations, key).map(|r| r.name.clone()))
        .unwrap_or_else(|| key.to_string())
}

fn toggle_sort(s: &mut State, id: &str, key: &str) {
    let Some(t) = table_mut(s, id) else { return };
    let (cur, desc) = t.sort_key_dir();
    t.override_sort = Some(if cur == key {
        (key.to_string(), !desc)
    } else {
        (key.to_string(), false)
    });
    load(s, id, false);
}

// ── Columns ──────────────────────────────────────────────────────

fn persist_view(s: &mut State, id: &str, next: Vec<Col>, name_w: Option<f32>) {
    let task = api::note::set_field(id, "viewRelations", config::columns_value(&next, name_w), s);
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        if let Err(error) = task.await {
            log::error!("save columns: {error:#}");
        }
        cx.update(|s| {
            // Wait for the refetched view object before dropping the local state.
            if let Some(t) = table_mut(s, &view) {
                t.local_cols = None;
                t.name_local = None;
            }
            reload(s, &view);
        });
    })
    .detach();
}

fn save_columns(s: &mut State, id: &str, next: Vec<Col>) {
    let name_w = stored_name_width(s, id);
    if let Some(t) = table_mut(s, id) {
        t.local_cols = Some(next.clone());
    }
    persist_view(s, id, next, name_w);
}

fn drop_column(s: &mut State, id: &str) {
    let Some(t) = table_mut(s, id) else { return };
    let Some(drag) = t.col_drag.take() else {
        return;
    };
    let Some(gap) = drag.gap else { return };
    let mut next = cols(s, id);
    if drag.from >= next.len() {
        return;
    }
    let moved = next.remove(drag.from);
    next.insert(if gap > drag.from { gap - 1 } else { gap }, moved);
    save_columns(s, id, next);
}

/// A header drag: the gap under the pointer among the columns (laid out
/// from `starts`, each column's left edge in the table's points, and its
/// width), never beside the column itself.
fn column_drag_move(s: &mut State, id: &str, x: f32, starts: &[(f32, f32)]) {
    let Some(t) = table_mut(s, id) else { return };
    let Some(drag) = &mut t.col_drag else { return };
    let local = x - t.table_left;
    let n = starts.len();
    let mut gap = None;
    for (i, (left, w)) in starts.iter().enumerate() {
        if local >= *left && local < left + w {
            let after = local > left + w / 2.;
            gap = Some(if after { i + 1 } else { i });
            break;
        }
    }
    // Over the "+" column: after the last.
    if gap.is_none()
        && let Some((left, w)) = starts.last()
        && local >= left + w
    {
        gap = Some(n);
    }
    let Some(gap) = gap else { return };
    if gap == drag.from || gap == drag.from + 1 {
        drag.gap = None;
        drag.gap_x = None;
    } else {
        drag.gap = Some(gap);
        drag.gap_x = Some(if gap == n {
            starts.last().map_or(0., |(l, w)| l + w)
        } else {
            starts[gap].0
        });
    }
}

// ── Entry row ────────────────────────────────────────────────────

/// The accent New of a table view: the in-table entry row, focused.
pub(crate) fn begin_new(s: &mut State, id: &str) {
    let Some(t) = table_mut(s, id) else { return };
    t.pending_new = true;
    t.new_focused = false;
    t.new_input.clear();
    t.new_input.focus();
}

fn commit_new(s: &mut State, id: &str, cancel: bool) {
    let Some(t) = table_mut(s, id) else { return };
    if t.committing {
        return;
    }
    let name = t.new_input.value().trim().to_string();
    t.pending_new = false;
    t.new_focused = false;
    t.new_input.clear();
    if cancel {
        return;
    }
    // The entry row's record: created as the view creates records (`oncreate`).
    t.committing = true;
    let task = crate::controls::create_record(s, id, name, None);
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        task.await;
        cx.update(|s| {
            if let Some(t) = table_mut(s, &view) {
                t.committing = false;
            }
            load(s, &view, false);
        });
    })
    .detach();
}

// ── Context menu ─────────────────────────────────────────────────

fn open_menu(s: &mut State, id: &str, row: &str, at: PointerPress) {
    let columns: Vec<String> = cols(s, id).into_iter().map(|c| c.key).collect();
    let Some(t) = table(s, id) else { return };
    let targets = if t.selected.iter().any(|x| x == row) {
        t.selected.clone()
    } else {
        vec![row.to_string()]
    };
    let fields: HashMap<String, roostr_core::Fields> = t
        .rows
        .iter()
        .filter(|r| targets.contains(&r.id))
        .map(|r| (r.id.clone(), r.fields.clone()))
        .collect();
    crate::context_menu::open(s, id, targets, (at.x, at.y), fields, columns);
}

fn delete_selected(s: &mut State, id: &str) {
    let Some(t) = table(s, id) else { return };
    let ids: Vec<String> = t
        .selected
        .iter()
        .filter(|x| !t.sel_blocked.contains_key(*x))
        .cloned()
        .collect();
    set_selection(s, id, Vec::new());
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        for record in ids {
            let task = cx.update(|s| api::note::delete(&record, s));
            if let Err(error) = task.await {
                log::error!("delete {record}: {error:#}");
            }
        }
        cx.update(|s| reload(s, &view));
    })
    .detach();
}

/// The property flow's list: the view's columns (on), then the specials not shown (off).
fn flow_items(s: &State, id: &str, cols: &[Col]) -> Vec<FlowItem> {
    let Some(object) = s.store.object(id) else {
        return Vec::new();
    };
    let relations = common::scoped_relations(s, &object);
    let mut items: Vec<FlowItem> = cols
        .iter()
        .map(|c| {
            let sp = SPECIALS.iter().find(|(k, _)| *k == c.key);
            let rel = common::find_rel(&relations, &c.key);
            FlowItem {
                key: c.key.clone(),
                name: sp
                    .map(|(_, n)| (*n).to_string())
                    .or_else(|| rel.map(|r| r.name.clone()))
                    .unwrap_or_else(|| c.key.clone()),
                format: rel
                    .map(|r| r.format.clone())
                    .unwrap_or_else(|| "shorttext".into()),
                on: true,
                emoji: special_emoji(&c.key)
                    .map(str::to_string)
                    .or_else(|| rel.and_then(|r| r.icon_emoji.clone())),
            }
        })
        .collect();
    for (k, n) in SPECIALS {
        if !cols.iter().any(|c| c.key == *k) {
            items.push(FlowItem {
                key: (*k).into(),
                name: (*n).into(),
                format: "shorttext".into(),
                on: false,
                emoji: special_emoji(k).map(str::to_string),
            });
        }
    }
    items
}

fn toggle_adding(s: &mut State, id: &str) {
    let Some(t) = table_mut(s, id) else { return };
    if t.adding.take().is_some() {
        return;
    }
    let Some(object) = s.store.object(id) else {
        return;
    };
    let items = flow_items(s, id, &cols(s, id));
    let space = object_space_id(&s.store, &object.fields);
    if let Some(t) = table_mut(s, id) {
        t.adding = Some(PropertyFlowState::new(
            items,
            vec![SERVING_KEY.to_string()],
            space,
        ));
    }
}

fn on_flow(s: &mut State, id: &str, event: FlowEvent) {
    match event {
        FlowEvent::Toggle(key, on) => {
            let mut next = cols(s, id);
            if on {
                next.push(Col {
                    key,
                    width: DEFAULT_WIDTH,
                });
            } else {
                next.retain(|c| c.key != key);
            }
            let items = flow_items(s, id, &next);
            if let Some(flow) = table_mut(s, id).and_then(|t| t.adding.as_mut()) {
                flow.set_items(items);
            }
            save_columns(s, id, next);
        }
        FlowEvent::Add(rel) => {
            let mut next = cols(s, id);
            next.push(Col {
                key: rel.key.clone(),
                width: DEFAULT_WIDTH,
            });
            save_columns(s, id, next);
        }
        FlowEvent::Close => {
            if let Some(t) = table_mut(s, id) {
                t.adding = None;
            }
        }
    }
}

// ── Keys ─────────────────────────────────────────────────────────

/// ← / → with no modifiers: the table under the pointer scrolls sideways.
fn arrow_step(ev: &KeyEvent) -> Option<f32> {
    let m = ev.mods;
    if m.cmd || m.ctrl || m.alt || m.shift || m.logo {
        return None;
    }
    match ev.key {
        Key::Left => Some(-ARROW_STEP),
        Key::Right => Some(ARROW_STEP),
        _ => None,
    }
}

fn scroll_by(t: &Table, id: &str, dx: f32, tx: &mut Tx) -> bool {
    let (at, max) = t.hscroll();
    if max <= 0. {
        return false;
    }
    let x = (at + dx).clamp(0., max);
    if x == at {
        return false;
    }
    tx.scroll_to_x(format!("{id}/table-scroll"), x);
    true
}

/// The window's keys nothing focused took: the hovered table scrolls with ←
/// / →; Escape closes every open cell editor.
pub(crate) fn on_window_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    // A text field keeps its arrows even when it ignores one (→ at the end of the text).
    if tx.focused().is_some() && ev.key != Key::Escape {
        return false;
    }
    let Some(sets) = s.get::<crate::Sets>() else {
        return false;
    };
    if ev.key == Key::Escape {
        let ids: Vec<String> = sets
            .views
            .values()
            .filter(|v| v.table().is_some_and(|t| t.cell_edit.is_some()))
            .map(|v| v.id.clone())
            .collect();
        for id in &ids {
            close_editor(s, id);
        }
        return !ids.is_empty();
    }
    let Some(dx) = arrow_step(ev) else {
        return false;
    };
    sets.views
        .values()
        .find_map(|v| v.table().filter(|t| t.hovered).map(|t| (v.id.clone(), t)))
        .is_some_and(|(id, t)| scroll_by(t, &id, dx, tx))
}

/// Keys while the data view has focus (a press in it focused it).
pub(crate) fn on_view_key(s: &mut State, id: &str, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(dx) = arrow_step(ev) else {
        return false;
    };
    table(s, id).is_some_and(|t| t.hovered && scroll_by(t, id, dx, tx))
}

// ── Rendering ────────────────────────────────────────────────────

/// Column widths for this frame: the Name column takes the slack (never less
/// than its minimum), or its own width; fixed-layout tables spread any excess
/// over every column in proportion.
fn layout_widths(avail: f32, name_w: Option<f32>, cols: &[Col]) -> (f32, Vec<f32>, f32) {
    let sum: f32 = cols.iter().map(|c| c.width).sum();
    match name_w {
        None => {
            let name = (avail - sum - PLUS_W).max(NAME_MIN_WIDTH);
            (name, cols.iter().map(|c| c.width).collect(), PLUS_W)
        }
        Some(nw) => {
            let total = nw + sum + PLUS_W;
            let scale = if avail > total { avail / total } else { 1. };
            (
                nw * scale,
                cols.iter().map(|c| c.width * scale).collect(),
                PLUS_W * scale,
            )
        }
    }
}

/// What every view run reads once: the theme, the view's id, its relations.
struct Ctx<'a> {
    th: &'a Theme,
    id: &'a str,
    relations: &'a [RelationDef],
    space: &'a str,
}

/// The kit ghost button's hover fill: the accent at half strength (dark), solid (light).
fn ghost_hover(th: &Theme) -> Color {
    if th.dark {
        th.kit.accent.alpha(th.kit.accent.a * 0.5)
    } else {
        th.kit.accent
    }
}

/// A column title (`.th-label`), drawn as the kit ghost button that sorts by
/// it: muted 13/500, the optional property emoji, the text ellipsized. The
/// whole title is also the column's drag source (reorder).
fn sort_head(c: &Ctx, s: &State, key: String, icon: Option<String>, label: String) -> El {
    let th = c.th;
    let ghost_hover = ghost_hover(th);
    let mut content = row().grow(1.0).min_w(0.0).center_y();
    if let Some(icon) = icon {
        content = content.child(el().mr(5.0).child(common::icon(s, &icon, ty::ui(12.0))));
    }
    content = content.child(
        text(label)
            .font(ty::ui(13.0).medium())
            .color(th.muted)
            .group_color(th.kit.accent_foreground)
            .truncate()
            .shrink(),
    );
    row()
        .key(key)
        .role("button")
        .min_w(0.0)
        .px(10.0)
        .py(6.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .hover_group()
        .hover(ghost_hover)
        .child(content)
}

/// The resize grip on a header cell's right edge: the accent at 25% while
/// the header cell is hovered, 60% under the pointer.
fn resize_grip(c: &Ctx, key: String, col: Option<usize>, start_w: f32) -> El {
    let id = c.id.to_string();
    el().key(key)
        .pin(None, Some(0.), Some(0.), Some(0.))
        .w(11.0)
        .cursor(Cursor::ResizeColumn)
        .hover(c.th.accent.alpha(0.6))
        .help("Drag to resize")
        .child(
            el().w_full()
                .h_full()
                .fill(c.th.accent.alpha(0.25))
                .group_opacity(0.0, 1.0),
        )
        .on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| {
            resize_drag(s, &id, col, start_w, d)
        })
}

fn resize_drag(s: &mut State, id: &str, col: Option<usize>, start_w: f32, d: &Drag) {
    match d.phase {
        Phase::Down => {
            let snapshot = cols(s, id);
            let name_w = stored_name_width(s, id);
            let Some(t) = table_mut(s, id) else { return };
            let start_w = match col {
                Some(_) => start_w,
                None => t.name_local.or(name_w).unwrap_or(start_w),
            };
            t.resize = Some(Resize {
                col,
                start_w,
                snapshot,
            });
        }
        Phase::Move => {
            let Some(t) = table_mut(s, id) else { return };
            let Some(resize) = &t.resize else { return };
            let w = (resize.start_w + d.dx).max(MIN_WIDTH);
            match resize.col {
                Some(idx) => {
                    let mut snap = resize.snapshot.clone();
                    if let Some(c) = snap.get_mut(idx) {
                        c.width = w;
                    }
                    t.local_cols = Some(snap);
                }
                None => t.name_local = Some(w.round()),
            }
        }
        Phase::Up => {
            let Some(t) = table_mut(s, id) else { return };
            let Some(resize) = t.resize.take() else {
                return;
            };
            if d.cancelled() {
                t.local_cols = None;
                t.name_local = None;
                return;
            }
            match resize.col {
                Some(_) => {
                    let next = t.local_cols.clone().unwrap_or(resize.snapshot);
                    save_columns(s, id, next);
                }
                None => {
                    let name_w = t.name_local;
                    let next = cols(s, id);
                    persist_view(s, id, next, name_w);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn header_cell(
    c: &Ctx,
    s: &State,
    i: usize,
    col: &Col,
    width: f32,
    sort: &(String, bool),
    dragging: bool,
    starts: Rc<Vec<(f32, f32)>>,
) -> El {
    let th = c.th;
    let label = col_name(&col.key, c.relations);
    let icon = common::find_rel(c.relations, &col.key).and_then(|r| {
        let space = r.space.clone().unwrap_or_default();
        relation_emoji(&s.store, r, None, &space)
    });
    let arrow = if sort.0 == col.key {
        if sort.1 { "↓" } else { "↑" }
    } else {
        ""
    };
    let (id, key) = (c.id.to_string(), col.key.clone());
    let sort_key = col.key.clone();
    let drag_label = label.clone();
    let mut head = sort_head(
        c,
        s,
        format!("head-{}", col.key),
        icon,
        format!("{label} {arrow}"),
    )
    .on_press(move |s: &mut State| toggle_sort(s, &id, &sort_key));
    if dragging {
        head = head.opacity(0.55);
    }
    // The title is the sort toggle and the drag source (reorder).
    let id = c.id.to_string();
    head = head.on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| {
        let [x, y] = d.window_point();
        match d.phase {
            Phase::Down => {
                if let Some(t) = table_mut(s, &id) {
                    t.col_drag = Some(ColDrag {
                        from: i,
                        label: drag_label.clone(),
                        gap: None,
                        gap_x: None,
                        at: [x, y],
                        grab: [d.x, d.y],
                    });
                }
            }
            Phase::Move => {
                if let Some(drag) = table_mut(s, &id).and_then(|t| t.col_drag.as_mut()) {
                    drag.at = [x, y];
                }
                if d.dx.abs() + d.dy.abs() >= 4. {
                    column_drag_move(s, &id, x, &starts);
                }
            }
            Phase::Up => {
                if d.cancelled() || d.dx.abs() + d.dy.abs() < 4. {
                    if let Some(t) = table_mut(s, &id) {
                        t.col_drag = None;
                    }
                    return;
                }
                drop_column(s, &id);
            }
        }
    });
    let hide_id = c.id.to_string();
    let hide = el()
        .pin(None, Some(0.), Some(8.), Some(0.))
        .center_y()
        .group_opacity(0.0, 1.0)
        .child(
            Button::new(format!("hide-{}", col.key))
                .ghost()
                .xsmall()
                .icon(Glyph::Close)
                .bg(th.bg)
                .text_color(th.muted)
                .pointer()
                .tooltip("Hide column")
                .on_press(move |s: &mut State| {
                    let next = cols(s, &hide_id)
                        .into_iter()
                        .filter(|x| x.key != key)
                        .collect();
                    save_columns(s, &hide_id, next);
                })
                .build(th),
        );
    let mut cell = row()
        .key(format!("th-{}", col.key))
        .hover_group()
        .w(width)
        .h_full()
        .center_y()
        .clip_rounded(0.0)
        .child(head)
        .child(hide)
        .child(resize_grip(
            c,
            format!("grip-{}", col.key),
            Some(i),
            col.width,
        ));
    if dragging {
        cell = cell.fill(th.accent.alpha(0.14));
    }
    cell
}

#[allow(clippy::too_many_arguments)]
fn body_cell(
    c: &Ctx,
    s: &State,
    cx: &mut Cx<State>,
    layer: &mut Vec<El>,
    t: &Table,
    r: &QueryRow,
    key: &str,
    width: f32,
    dragging: bool,
) -> El {
    let th = c.th;
    let base = || {
        let e = row()
            .w(width)
            .center_y()
            .gap(4.0)
            .px(10.0)
            .py(7.0)
            .clip_rounded(0.0);
        if dragging {
            e.fill(th.accent.alpha(0.14))
        } else {
            e
        }
    };
    let cell_key = format!("cell-{}-{}", r.id, key);
    let target = reconnect_target(r, key);
    if r.type_key == "credential" && key == "status" {
        // The harness writes a credential's status: shown in words, never hand-edited here.
        let st = field_str(&r.fields, "status").to_string();
        if let Some(target) = target {
            let (label, g, color) = credential_cell_badge(r, s);
            let colors = badge_colors(th, color);
            let name = field_str(&r.fields, "name");
            let tip = format!(
                "Reconnect {}: {label}",
                if name.is_empty() { "credential" } else { name }
            );
            return base().key(cell_key).child(
                CredentialCell::new(format!("cred-{}", r.id), r.clone(), target, tip)
                    .child(
                        row()
                            .center_y()
                            .gap(5.0)
                            .child(glyph(g, 14.0).color(colors.icon))
                            .child(text(label).font(ty::ui(13.0)).color(colors.fg)),
                    )
                    .build(s, cx, th, layer),
            );
        }
        let (g, color) = credential_status_badge(&st);
        let colors = badge_colors(th, color);
        return base().key(cell_key).child(
            row()
                .center_y()
                .gap(5.0)
                .child(glyph(g, 14.0).color(colors.icon))
                .child(
                    text(credential_status_text(&st))
                        .font(ty::ui(13.0))
                        .color(colors.fg),
                ),
        );
    }
    if let Some(target) = target
        && key == "error"
    {
        let line = cell_text(s, t, r, key, c.relations);
        return base().key(cell_key).child(
            CredentialCell::new(
                format!("err-{}", r.id),
                r.clone(),
                target,
                format!("Reconnect: {line}"),
            )
            .child(text(line).font(ty::ui(13.0)).color(rgb(WARN)).truncate())
            .build(s, cx, th, layer),
        );
    }
    let locked = is_locked_tool(&r.fields);
    if is_editable(c.relations, key) && !locked {
        let fmt = format_of(c.relations, key);
        let rel = common::find_rel(c.relations, key);
        let checkbox = fmt == "checkbox";
        let (id, record, k) = (c.id.to_string(), r.id.clone(), key.to_string());
        let mut cell = base()
            .key(cell_key)
            .cursor(Cursor::Pointer)
            .hover(th.hl_light)
            .on_press(move |s: &mut State| on_cell_press(s, &id, &record, &k, checkbox));
        cell = with_marquee(cell, c.id, cx);
        // Held, it shrinks; not while its row has a cell editor open.
        if !t.cell_edit.as_ref().is_some_and(|(row, _)| *row == r.id) {
            cell = motion::pressable_row(cell);
        }
        match fmt {
            "tag" | "status" => {
                // Inline-block pills on the text baseline grow the CSS line box to ~23px.
                let items = common::string_items(r.fields.get(key));
                if !items.is_empty() {
                    cell = cell.min_h(37.0);
                }
                for item in items {
                    let color = rel
                        .and_then(|rel| rel.options.iter().find(|o| o.text == item))
                        .map_or("", |o| o.color.as_str());
                    cell = cell.child(tag_pill(th, &item, color, false, th.fg));
                }
            }
            "object" => {
                let ids = r.fields.get(key).map(common::link_ids).unwrap_or_default();
                if !ids.is_empty() {
                    cell = cell.min_h(37.0);
                }
                for target in &ids {
                    let open = target.clone();
                    // A pill opens its object; the cell around it opens the editor.
                    cell = cell.child(
                        el().key(format!("pill-{}-{key}-{target}", r.id))
                            .cursor(Cursor::Pointer)
                            .on_press(move |s: &mut State| common::open_object(&open, s))
                            .child(common::object_pill(s, th, target, c.space)),
                    );
                }
            }
            "checkbox" => {
                // Display only: the press reaches the cell, which toggles it.
                let on = r.fields.get(key).and_then(|v| v.bool_value) == Some(true);
                cell = cell
                    .min_h(37.0)
                    .child(checkbox_display(format!("cbx-{}-{key}", r.id), on).build(th));
            }
            _ => {
                let line = cell_text(s, t, r, key, c.relations);
                let warn = key == "error" && !line.is_empty();
                cell = cell.child(
                    text(line)
                        .font(ty::ui(13.0))
                        .color(if warn { rgb(WARN) } else { th.fg })
                        .truncate()
                        .shrink(),
                );
            }
        }
        return cell;
    }
    // Specials and read-only columns.
    let muted = matches!(key, "type" | "createdAt" | "updatedAt");
    let serving = t.serving_by_id.get(&r.id);
    let warn = key == "serving" && serving.is_some_and(|s| s.warning);
    let ink = if warn {
        rgb(WARN)
    } else if muted {
        th.muted
    } else {
        th.fg
    };
    let lead = if key == "type" {
        Some(type_icon(&s.store, &r.type_key, c.space)).filter(|s| !s.is_empty())
    } else if key == "serving" {
        serving.map(|s| s.icon.clone()).filter(|s| !s.is_empty())
    } else {
        None
    };
    let line = cell_text(s, t, r, key, c.relations);
    let mut cell = base().key(cell_key);
    if let Some(icon) = lead {
        cell = cell.child(el().mr(2.0).child(common::icon(s, &icon, ty::ui(13.0))));
    }
    cell = cell.child(text(line).font(ty::ui(13.0)).color(ink).truncate().shrink());
    if key == "serving" {
        if let Some(sv) = serving {
            cell = cell.help(sv.text.clone());
        }
        if !locked {
            let (id, record) = (c.id.to_string(), r.id.clone());
            cell = cell
                .cursor(Cursor::Pointer)
                .hover(th.hl_light)
                .on_press(move |s: &mut State| on_cell_press(s, &id, &record, "serving", false));
            cell = with_marquee(cell, c.id, cx);
            if !t.cell_edit.as_ref().is_some_and(|(row, _)| *row == r.id) {
                cell = motion::pressable_row(cell);
            }
        }
    }
    cell
}

/// The rubber band also arms from the rows' own pressables. Not on a touch screen: an element
/// that drags takes the finger, so the list under it would never scroll.
fn with_marquee(e: El, id: &str, cx: &Cx<State>) -> El {
    if crate::touch_screen(cx) {
        return e;
    }
    let id = id.to_string();
    e.on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| marquee(s, &id, d, tx))
}

/// The cell editor (roostr-properties `PropertyValue` at the cell): anchored
/// under the cell's left edge (+4), pulled left so it stays inside the
/// table's visible width (the page column clips anything past it). A press
/// outside it closes it, or moves it to the editable cell pressed.
#[allow(clippy::too_many_arguments)]
fn cell_pop(
    c: &Ctx,
    s: &State,
    cx: &mut Cx<State>,
    layer: &mut Vec<El>,
    t: &Table,
    name_width: f32,
    widths: &[f32],
    cols: &[Col],
) -> Option<El> {
    let th = c.th;
    let (record, key) = t.cell_edit.clone()?;
    let edit_key = if key == "serving" {
        SERVING_KEY
    } else {
        key.as_str()
    };
    let rel = common::find_rel(c.relations, edit_key)?.clone();
    let row = t.rows.iter().find(|r| r.id == record)?;
    let ci = cols.iter().position(|col| col.key == key)?;
    let wide = rel.format == "longtext" || rel.key == "error";
    let seeds = s
        .store
        .object(c.id)
        .map(|o| roostr_core::view_seeds::seeds_from_view(&o.fields, c.relations))
        .unwrap_or_default();
    let pop_key = format!("{}/cell-pop", c.id);
    // Where the cell starts in the table, and the table's visible part
    // (in the table's points: it scrolls sideways under the clip).
    let cell_left = name_width + widths[..ci].iter().sum::<f32>();
    let (at, _) = t.hscroll();
    let pop_w = cx.layout_with(&pop_key, |r| r[2]).unwrap_or(0.).max(260.);
    let mut left = cell_left;
    if t.clip[2] > 0. {
        left = left.min(at + t.clip[2] - pop_w).max(at);
    }
    let win_w = cx.layout_with("kit-root", |r| r[2]).unwrap_or(f32::MAX);
    let id = c.id.to_string();
    let editor = PropertyValue::new(
        format!("pop-{record}-{key}"),
        rel,
        row.fields.get(edit_key).cloned(),
    )
    .space(field_str(&row.fields, "channel").to_string())
    .seeds(seeds)
    .auto_open(true)
    .inline_menus(true)
    .on_save(move |s: &mut State, v: Value| cell_save(s, &id, v))
    .build(s, cx, th, layer);
    let mut pop = col()
        .key(pop_key)
        .fill(th.panel)
        .border(1.0, th.border)
        .radius(10.0)
        .padding(8.0)
        .box_shadow(roostr_ui::shadow::menu())
        .child(editor);
    if wide {
        // `.cell-pop.wide`: 440px, `max-width: calc(100vw - 24px)` (content box).
        pop = pop.w(458.0_f32.min(win_w - 24. + 18.));
    }
    let escape = c.id.to_string();
    let pop = pop.on_key(move |s: &mut State, ev: &KeyEvent, _: &mut Tx| {
        if ev.key == Key::Escape {
            close_editor(s, &escape);
            return true;
        }
        false
    });
    let anchor = format!("{}/pop-anchor", c.id);
    // Enters per cell: another cell's editor mounts afresh.
    let pop = overlay::anchored(
        pop.key(format!("{}/cell-pop-{record}-{key}", c.id)),
        &anchor,
        Placement::BottomStart,
        4.,
    );
    // The press-away layer under it: an editable cell takes the editor, any
    // other press closes it.
    let id = c.id.to_string();
    layer.push(
        el().key(format!("{}/cell-scrim", c.id))
            .pin(Some(0.), Some(0.), Some(0.), Some(0.))
            .on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| {
                if d.phase == Phase::Down {
                    let [x, y] = d.window_point();
                    press_outside_editor(s, &id, x, y);
                }
            }),
    );
    layer.push(pop);
    // The anchor rides in the edited row, `left` in from the table's start.
    let ri = t.rows.iter().position(|r| r.id == record)?;
    let tops = t.tops();
    Some(
        el().key(anchor)
            .absolute(left, tops[ri + 1] - 1.)
            .size(0., 1.),
    )
}

/// A press outside the open cell editor: on an editable cell, the editor
/// moves there (a checkbox toggles); anywhere else it closes.
fn press_outside_editor(s: &mut State, id: &str, x: f32, y: f32) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    let relations = common::scoped_relations(s, &object);
    let all = cols(s, id);
    let name_w = s
        .get::<crate::Sets>()
        .and_then(|_| table(s, id))
        .and_then(|t| t.name_local)
        .or(config::stored_name_width(&object.fields));
    let Some(t) = table(s, id) else { return };
    let (name_width, widths, _) = layout_widths(t.clip[2], name_w, &all);
    let hit = (|| {
        if x < t.clip[0] || x > t.clip[0] + t.clip[2] {
            return None;
        }
        let tops = t.tops();
        let ri = (0..t.rows.len())
            .find(|&i| y >= t.rows_top + tops[i] && y < t.rows_top + tops[i + 1])?;
        let local = x - t.table_left;
        // The pinned Name column covers the cells scrolled under it.
        if x < t.clip[0] + name_width {
            return None;
        }
        let mut left = name_width;
        for (ci, w) in widths.iter().enumerate() {
            if local >= left && local < left + w {
                let key = &all[ci].key;
                let row = &t.rows[ri];
                let editable = if key == "serving" {
                    !is_locked_tool(&row.fields)
                } else {
                    is_editable(&relations, key)
                        && !is_locked_tool(&row.fields)
                        && reconnect_target(row, key).is_none()
                        && !(row.type_key == "credential" && key == "status")
                };
                return editable.then(|| {
                    (
                        row.id.clone(),
                        key.clone(),
                        format_of(&relations, key) == "checkbox",
                    )
                });
            }
            left += w;
        }
        None
    })();
    match hit {
        Some((record, key, checkbox)) => on_cell_press(s, id, &record, &key, checkbox),
        None => close_editor(s, id),
    }
}

/// The table of data view `v`: header, the rows near the window, the entry
/// row and "＋ New Object", the "+" column button, the selection bar, and
/// (into `layer`) the cell editor, the column flow, the drag ghost and the
/// rubber band.
pub(crate) fn view(
    s: &State,
    cx: &mut Cx<State>,
    th: &Theme,
    layer: &mut Vec<El>,
    v: &DataView,
) -> El {
    let Some(t) = v.table() else { return el() };
    let Some(object) = s.store.object(&v.id) else {
        return el();
    };
    let id = v.id.as_str();
    let relations = common::scoped_relations(s, &object);
    let space = current_space_id(s);
    let c = Ctx {
        th,
        id,
        relations: &relations,
        space: &space,
    };
    let all_cols = cols_of(s, id, t);
    let n = all_cols.len();
    let name_w = t.name_local.or(config::stored_name_width(&object.fields));
    let scroll_key = format!("{id}/table-scroll");
    let avail = cx.layout_with(&scroll_key, |r| r[2]).unwrap_or(0.);
    let (name_width, widths, plus_w) = layout_widths(avail, name_w, &all_cols);
    let table_w = name_width + widths.iter().sum::<f32>() + plus_w;
    let sort = t.sort_key_dir();
    // Each column's left edge and width in the table's points (reorder targets).
    let starts: Rc<Vec<(f32, f32)>> = Rc::new({
        let mut x = name_width;
        widths
            .iter()
            .map(|w| {
                let at = (x, *w);
                x += w;
                at
            })
            .collect()
    });

    // Header.
    let name_arrow = if sort.0 == "name" {
        if sort.1 { "↓" } else { "↑" }
    } else {
        ""
    };
    let pin_shadow = Shadow {
        x: 8.,
        blur: 16.,
        spread: -8.,
        color: Color::BLACK.alpha(0.6),
        ..Shadow::default()
    };
    let sort_id = id.to_string();
    // The Name column stays put while the table scrolls sideways, opaque over
    // the cells scrolled under it, its shadow showing once it is pinned.
    let name_th = row()
        .key("th-name")
        .hover_group()
        .w(name_width)
        .h_full()
        .center_y()
        .clip_rounded(0.0)
        .fill(th.bg)
        .sticky_left(0.)
        .stuck_shadow(pin_shadow)
        .child(
            sort_head(
                &c,
                s,
                "head-name".into(),
                None,
                format!("Name {name_arrow}"),
            )
            .on_press(move |s: &mut State| toggle_sort(s, &sort_id, "name")),
        )
        .child(resize_grip(&c, "grip-name".into(), None, name_width));
    let mut header = row()
        .key(format!("{id}/table-head"))
        .h(HEADER_H)
        .border_sides([0., 0., 1., 0.], th.border)
        .border_layout()
        .child(name_th);
    let dragging = t
        .col_drag
        .as_ref()
        .filter(|d| d.gap.is_some() || d.gap_x.is_some());
    for (i, col) in all_cols.iter().enumerate() {
        let on = t.col_drag.as_ref().is_some_and(|d| d.from == i);
        header = header.child(header_cell(
            &c,
            s,
            i,
            col,
            widths[i],
            &sort,
            on,
            starts.clone(),
        ));
    }
    // The "+" column's place (its button is pinned on top, see below).
    header = header.child(el().w(plus_w).h_full());

    // Body rows: only those in (or near) the window are built - the page
    // scrolls the table, and building every loaded row (hundreds, each a row
    // of cells) on every change lagged. Spacers stand in for the rest.
    let rows_key = format!("{id}/rows");
    let n_rows = t.rows.len();
    let tops = t.tops();
    let win_h = cx
        .layout_with("kit-root", |r| r[3])
        .unwrap_or(WINDOW_H_GUESS);
    let band = cx
        .window_layout_with(&rows_key, |r| (r[1] / BAND).floor() as i32)
        .unwrap_or(0);
    let (first, last) = {
        let top = band as f32 * BAND;
        let over = win_h * OVERSCAN + BAND;
        let first = tops[1..]
            .iter()
            .position(|b| top + b >= -over)
            .unwrap_or(n_rows);
        let last = tops[..n_rows]
            .iter()
            .rposition(|y| top + y <= win_h + over)
            .map_or(first, |i| i + 1)
            .max(first);
        (first, last)
    };
    let mut rows_el = col().key(rows_key.clone()).on_layout_tx({
        let id = id.to_string();
        move |s: &mut State, r: [f32; 4], tx: &mut Tx| {
            if let Some(t) = table_mut(s, &id) {
                t.rows_top = r[1];
            }
            tx.live_only();
        }
    });
    rows_el = rows_el.child(el().h(tops[first]));
    for r in &t.rows[first..last] {
        rows_el = rows_el.child(body_row(
            &c, s, cx, layer, t, r, &all_cols, &widths, name_width, plus_w, pin_shadow,
        ));
    }
    rows_el = rows_el.child(el().h(tops[n_rows] - tops[last]));
    if t.page_query.is_some() && t.rows.len() < t.total {
        // The next page loads as this comes within a screen of view.
        rows_el = rows_el.child(
            row()
                .key(format!("{id}/loading-more"))
                .px(10.0)
                .py(7.0)
                .border_sides([0., 0., 1., 0.], th.border)
                .border_layout()
                .child(
                    text(format!("Loading more… {} of {}", t.rows.len(), t.total))
                        .font(ty::ui(13.0))
                        .color(th.fg),
                ),
        );
        let near = band as f32 * BAND + tops[n_rows] < win_h + 800.;
        if near && !t.loading_more {
            let id = id.to_string();
            s.rt.spawn(async move |cx| cx.update(|s| load_more(s, &id)))
                .detach();
        }
    }
    // The entry row and "＋ New Object".
    if t.pending_new {
        let (submit, key_id, lens) = (id.to_string(), id.to_string(), id.to_string());
        rows_el = rows_el.child(
            el().py(4.0).child(
                row()
                    .key(format!("{id}/new-entry"))
                    .center_y()
                    .gap(6.0)
                    .max_w(444.0)
                    .border(2.0, th.accent)
                    .border_layout()
                    .radius(8.0)
                    .px(10.0)
                    .py(8.0)
                    .child(
                        Input::new(format!("{id}/new-input"), &t.new_input)
                            .placeholder("Enter name")
                            .appearance(false)
                            .text_size(13.0)
                            .on_edit(move |s: &mut State| {
                                &mut table_mut(s, &lens)
                                    .expect("the entry row's table is open")
                                    .new_input
                            })
                            .on_submit(move |s: &mut State, _: &mut Tx| {
                                commit_new(s, &submit, false)
                            })
                            .on_key(move |s: &mut State, ev: &KeyEvent, _: &mut Tx| {
                                if ev.key == Key::Escape {
                                    commit_new(s, &key_id, true);
                                    return true;
                                }
                                false
                            })
                            .build(th),
                    ),
            ),
        );
    }
    let new_id = id.to_string();
    rows_el = rows_el.child(
        // The kit ghost button, full width, its label at the start.
        row()
            .key(format!("{id}/new-object"))
            .role("button")
            .w_full()
            .px(8.0)
            .py(10.0)
            .radius(roostr_ui::radius::KIT)
            .center_y()
            .cursor(Cursor::Pointer)
            .focusable(true)
            .hover_group()
            .hover(ghost_hover(th))
            .child(
                text("＋ New Object")
                    .font(ty::ui(14.0))
                    .color(th.muted)
                    .group_color(th.kit.accent_foreground),
            )
            .on_press(move |s: &mut State| begin_new(s, &new_id)),
    );

    let mut table_el = col()
        .key(format!("{id}/table"))
        .w(table_w)
        .on_layout_tx({
            let id = id.to_string();
            move |s: &mut State, r: [f32; 4], tx: &mut Tx| {
                if let Some(t) = table_mut(s, &id) {
                    t.table_left = r[0];
                    t.table_w = r[2];
                }
                tx.live_only();
            }
        })
        .child(header)
        .child(rows_el);
    if let Some(x) = dragging.and_then(|d| d.gap_x) {
        table_el = table_el.child(
            el().pin(Some(x - 1.5), Some(0.), None, Some(0.))
                .w(3.0)
                .radius(2.0)
                .fill(th.accent)
                .box_shadow(Shadow {
                    spread: 3.,
                    color: th.accent.alpha(0.22),
                    ..Shadow::default()
                }),
        );
    }
    if let Some(anchor) = cell_pop(&c, s, cx, layer, t, name_width, &widths, &all_cols) {
        table_el = table_el.child(el().pin(Some(0.), Some(HEADER_H), None, None).child(anchor));
    }
    // The table overflows its pane sideways: its columns, as laid out for
    // this pane width, are wider than the pane.
    let too_wide = avail > 0. && table_w > avail + 0.5;
    if too_wide {
        table_el = table_el.pb(TABLE_SCROLLBAR_ROOM);
    }
    // The wheel over the horizontal scrollbar strip scrolls the table
    // sideways (either wheel axis), not the page.
    let wheel_id = id.to_string();
    table_el = table_el.on_wheel(move |s: &mut State, e: rune::WheelEvent, tx: &mut Tx| {
        let Some(t) = table(s, &wheel_id) else {
            return false;
        };
        let (at, max) = t.hscroll();
        if max <= 0. || e.y < t.clip[3] - TABLE_SCROLLBAR_ROOM {
            return false;
        }
        let d = if e.dy.abs() > e.dx.abs() { e.dy } else { e.dx };
        let x = (at - d).clamp(0., max);
        tx.scroll_to_x(format!("{wheel_id}/table-scroll"), x);
        true
    });

    let mode = if too_wide {
        ScrollbarMode::Always
    } else {
        ScrollbarMode::system()
    };
    let scroller = ScrollArea::horizontal(scroll_key, table_el)
        .mode(mode)
        .w_full()
        .build(th);

    // The "+" header button, pinned to the right edge.
    let plus_id = id.to_string();
    let add_key = format!("{id}/add-column");
    let plus_button = Button::new(add_key.clone())
        .outline()
        .icon(Glyph::Plus)
        .square(26.0)
        .radius(7.0)
        .border_color(th.border)
        .text_color(th.fg)
        .pointer()
        .tooltip("Add column")
        .on_press(move |s: &mut State| toggle_adding(s, &plus_id))
        .build(th);
    let plus = row()
        .key(format!("{id}/plus"))
        .pin(None, Some(0.), Some(0.), None)
        .w(40.0)
        .h(HEADER_H)
        .center()
        .fill(th.bg)
        .border_sides([0., 0., 1., 0.], th.border)
        .box_shadow(Shadow {
            x: -8.,
            blur: 16.,
            spread: -8.,
            color: Color::BLACK.alpha(0.6),
            ..Shadow::default()
        })
        .child(plus_button);
    if let Some(flow) = &t.adding {
        // Opens leftward, over the table: the + sits at the table's right edge.
        let (lens, ev) = (id.to_string(), id.to_string());
        let surface = PropertyFlow::new(format!("{id}/flow"), flow)
            .on_state(move |s: &mut State| {
                table_mut(s, &lens)
                    .and_then(|t| t.adding.as_mut())
                    .expect("the column flow is open")
            })
            .on_event(move |s: &mut State, e: FlowEvent| on_flow(s, &ev, e))
            .build(s, cx, th, layer);
        layer.push(overlay::anchored(
            surface,
            &add_key,
            Placement::BottomEnd,
            4.,
        ));
    }

    let hover_id = id.to_string();
    let mut root = col().key(format!("{id}/table-root")).w_full().child(
        el().key(format!("{id}/table-wrap"))
            .w_full()
            // The viewport's rect: the table's visible part.
            .on_layout_tx({
                let id = id.to_string();
                move |s: &mut State, r: [f32; 4], tx: &mut Tx| {
                    if let Some(t) = table_mut(s, &id) {
                        t.clip = r;
                    }
                    tx.live_only();
                }
            })
            .on_hover_change(move |s: &mut State, inside: bool, tx: &mut Tx| {
                if let Some(t) = table_mut(s, &hover_id) {
                    t.hovered = inside;
                }
                tx.live_only();
            })
            .child(scroller)
            .child(plus),
    );
    if too_wide {
        root = root.child(
            el().key(format!("{id}/set-table-scrollbar"))
                .h(0.)
                .w_full()
                .a11y_hidden(),
        );
    }

    if !t.selected.is_empty() {
        root = root.child(row().w_full().justify_center().child(selection_bar(&c, t)));
    }
    if t.rows.is_empty() {
        root = root.child(el().px(10.0).py(16.0).child(common::empty_state(
            th,
            "No objects match.",
            13.0,
        )));
    }
    if let Some(m) = t.marquee.as_ref().filter(|_| t.drag_moved) {
        let (x, y) = (m.x0[0].min(m.x1[0]), m.x0[1].min(m.x1[1]));
        let (w, h) = ((m.x1[0] - m.x0[0]).abs(), (m.x1[1] - m.x0[1]).abs());
        layer.push(
            el().key(format!("{id}/marquee"))
                .absolute(x, y)
                .size(w, h)
                .fill(rgba(BAND_FILL))
                .border(1.0, rgba(BAND_BORDER))
                .radius(2.0)
                .inert(),
        );
    }
    if let Some(drag) = dragging {
        layer.push(
            row()
                .key(format!("{id}/col-ghost"))
                .absolute(drag.at[0] - drag.grab[0], drag.at[1] - drag.grab[1])
                .px(10.0)
                .py(6.0)
                .radius(6.0)
                .fill(th.panel)
                .border(1.0, th.border)
                .opacity(0.9)
                .inert()
                .child(text(drag.label.clone()).font(ty::ui(13.0)).color(th.muted)),
        );
    }
    let _ = n;
    root
}

#[allow(clippy::too_many_arguments)]
fn body_row(
    c: &Ctx,
    s: &State,
    cx: &mut Cx<State>,
    layer: &mut Vec<El>,
    t: &Table,
    r: &QueryRow,
    cols: &[Col],
    widths: &[f32],
    name_width: f32,
    plus_w: f32,
    pin_shadow: Shadow,
) -> El {
    let th = c.th;
    let id = c.id;
    let selected = t.selected.iter().any(|x| *x == r.id);
    let task = s.store.layout_of(&r.type_key) == "task";
    let done = r.fields.get("done").and_then(|v| v.bool_value) == Some(true);
    let lead = if task {
        let (view, rid) = (id.to_string(), r.id.clone());
        el().mr(6.0).child(
            checkbox(format!("done-{}", r.id), done)
                .tab_stop(false)
                .on_change(move |s: &mut State, done: bool| {
                    let task = api::note::set_field(&rid, "done", Value::bool(done), s);
                    after(s, &view, task);
                })
                .build(th),
        )
    } else {
        let glyph = object_icon(
            &s.store,
            field_str(&r.fields, "iconEmoji"),
            &r.type_key,
            c.space,
        );
        el().mr(4.0)
            .child(common::icon(s, &glyph, ty::ui(13.0).weight(550.)))
    };
    // Opaque, so the cells scrolled under it don't show through; the row's
    // selection and hover are painted again on top of that.
    let wash = if selected {
        el().pin(Some(0.), Some(0.), Some(0.), Some(0.))
            .fill(rgba(SELECTED))
    } else {
        el().pin(Some(0.), Some(0.), Some(0.), Some(0.))
            .fill(th.hover)
            .group_opacity(0.0, 1.0)
    };
    let (press_id, press_row) = (id.to_string(), r.id.clone());
    let (menu_id, menu_row) = (id.to_string(), r.id.clone());
    let name_cell = row()
        .key(format!("name-{}", r.id))
        .w(name_width)
        .self_stretch()
        .fill(th.bg)
        .sticky_left(0.)
        .stuck_shadow(pin_shadow)
        .cursor(Cursor::Pointer)
        .pointer_buttons([PointerButton::Middle])
        .on_press_tx(move |s: &mut State, tx: &mut Tx| on_row_press(s, &press_id, &press_row, tx))
        .on_secondary_press(move |s: &mut State, p: PointerPress, _: &mut Tx| {
            open_menu(s, &menu_id, &menu_row, p)
        })
        .child(wash)
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .center_y()
                .px(10.0)
                .py(7.0)
                .clip_rounded(0.0)
                .child(lead)
                .child(
                    text(common::row_name(r))
                        .font(ty::ui(13.0).weight(550.))
                        .color(th.fg)
                        .truncate()
                        .shrink(),
                ),
        );
    // Rune scales the element a press lands on, so the row's pressables
    // (its Name, its cells) each shrink while held; none while the row has a
    // cell editor open.
    let editing = t.cell_edit.as_ref().is_some_and(|(row, _)| *row == r.id);
    let mut name_cell = with_marquee(name_cell, id, cx);
    if !editing {
        name_cell = motion::pressable_row(name_cell);
    }
    let mut line = row()
        .key(format!("row-{}", r.id))
        .hover_group()
        .border_sides([0., 0., 1., 0.], th.border)
        .border_layout()
        .cursor(Cursor::Pointer)
        .child(name_cell);
    line = if selected {
        line.fill(rgba(SELECTED))
    } else {
        line.hover(th.hover)
    };
    for (i, col) in cols.iter().enumerate() {
        let on = t
            .col_drag
            .as_ref()
            .is_some_and(|d| d.from == i && d.gap.is_some());
        line = line.child(body_cell(c, s, cx, layer, t, r, &col.key, widths[i], on));
    }
    line = line.child(el().w(plus_w));
    let (press_id, press_row) = (id.to_string(), r.id.clone());
    let (menu_id, menu_row) = (id.to_string(), r.id.clone());
    let (h_id, h_row) = (id.to_string(), r.id.clone());
    let line = line
        .pointer_buttons([PointerButton::Middle])
        .on_press_tx(move |s: &mut State, tx: &mut Tx| on_row_press(s, &press_id, &press_row, tx))
        .on_secondary_press(move |s: &mut State, p: PointerPress, _: &mut Tx| {
            open_menu(s, &menu_id, &menu_row, p)
        })
        .on_layout_tx(move |s: &mut State, rect: [f32; 4], tx: &mut Tx| {
            measured(s, &h_id, &h_row, rect[3], tx)
        });
    with_marquee(line, id, cx)
}

/// A drawn row laid out at `h`: kept for the window, the band and the
/// editor; the view re-runs only when a height changed.
fn measured(s: &mut State, id: &str, row: &str, h: f32, tx: &mut Tx) {
    let Some(t) = table_mut(s, id) else { return };
    let old = t.heights.insert(row.to_string(), h);
    if old.is_some_and(|o| (o - h).abs() < 0.5) {
        tx.live_only();
        return;
    }
    let n = t.heights.len().max(1) as f32;
    t.row_h = t.heights.values().sum::<f32>() / n;
}

fn selection_bar(c: &Ctx, t: &Table) -> El {
    let th = c.th;
    let id = c.id;
    let deletable = t
        .selected
        .iter()
        .filter(|x| !t.sel_blocked.contains_key(*x))
        .count();
    let mut bar = row()
        .key(format!("{id}/sel-bar"))
        .center_y()
        .gap(12.0)
        .mt(8.0)
        .fill(th.panel)
        .border(1.0, th.border)
        .radius(10.0)
        .px(12.0)
        .py(6.0)
        .box_shadow(roostr_ui::shadow::gpui(4.0, 16.0, 0.0, 0.4))
        .child(motion::rolling_text(
            th,
            format!("{id}/sel-count"),
            format!("{} selected", t.selected.len()),
            t.selected.len() as f64,
            ty::ui(13.0),
            th.fg,
        ));
    if deletable > 0 {
        let label = if t.sel_blocked.is_empty() {
            "Delete".to_string()
        } else {
            format!(
                "Delete {deletable} ({} protected kept)",
                t.sel_blocked.len()
            )
        };
        let del = id.to_string();
        bar = bar.child(
            danger_button(format!("{id}/sel-del"), label)
                .py(2.0)
                .px(8.0)
                .on_press(move |s: &mut State| delete_selected(s, &del))
                .build(th),
        );
    } else {
        let why = t.sel_blocked.values().next().cloned().unwrap_or_default();
        bar = bar.child(
            text(format!("Can't delete: {why}"))
                .key(format!("{id}/sel-locked"))
                .font(ty::ui(12.0))
                .color(th.muted)
                .help("Types and properties Roostr or your queries rely on can't be deleted"),
        );
    }
    let clear = id.to_string();
    bar.child(
        Button::new(format!("{id}/sel-clear"))
            .ghost()
            .xsmall()
            .icon(Glyph::Close)
            .accessibility_label("Clear selection")
            .text_color(th.muted)
            .pointer()
            .on_press(move |s: &mut State| set_selection(s, &clear, Vec::new()))
            .build(th),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_takes_the_slack_or_scales_with_the_rest() {
        let cols = vec![
            Col {
                key: "a".into(),
                width: 150.,
            },
            Col {
                key: "b".into(),
                width: 100.,
            },
        ];
        let (name, widths, plus) = layout_widths(1000., None, &cols);
        assert_eq!((name, plus), (1000. - 250. - PLUS_W, PLUS_W));
        assert_eq!(widths, vec![150., 100.]);
        // Too narrow: Name keeps its minimum and the table scrolls.
        assert_eq!(layout_widths(300., None, &cols).0, NAME_MIN_WIDTH);
        // A stored Name width: wider panes spread the excess in proportion.
        let (name, widths, _) = layout_widths(2. * (200. + 250. + PLUS_W), Some(200.), &cols);
        assert_eq!(name, 400.);
        assert_eq!(widths, vec![300., 200.]);
    }

    #[test]
    fn serving_filters_match_the_rolled_up_states() {
        let warn = ServingInfo {
            text: String::new(),
            label: String::new(),
            warning: true,
            reason: "pinned-uncapable".into(),
            machine_id: "m".into(),
            icon: String::new(),
        };
        assert!(serving_match("equal", "attention", Some(&warn)));
        assert!(!serving_match("equal", "ok", Some(&warn)));
        assert!(serving_match("notEqual", "ok", Some(&warn)));
        assert!(serving_match("equal", "pinned-uncapable", Some(&warn)));
        assert_eq!(serving_rank(Some(&warn)), 0);
        assert_eq!(serving_rank(None), 1);
    }
}

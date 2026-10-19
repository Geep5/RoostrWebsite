//! The dataview grid (SetTable.svelte): Name first, then the view's relation
//! columns (`viewRelations`), then "+". Inline cell editing, sort by header,
//! column resize / reorder / hide, row multi-select (cmd / shift / marquee),
//! paging, the Served by and Repeat virtual columns, the "+ New Object" entry
//! row, and the row context menu.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::Icon;
use gpui_kit::component::input::{Escape as InputEscape, Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::dates::show_date;
use roostr_core::objects::{object_icon, type_icon};
use roostr_core::options::badge_colors;
use roostr_core::protected::blockers_for;
use roostr_core::relations::current_space_id;
use roostr_core::repeat::describe_repeat;
use roostr_core::serving::{MachineRow, fetch_machines, machine_name, resolve_many, serving_copy};
use roostr_core::ui::{CheckboxIcon, tooltip};
use roostr_core::{
    QueryRow, RelationDef, Store, Value, api, field_str, is_locked_tool, repeat_of, theme,
};
use roostr_properties::{
    CredentialCell, FlowEvent, FlowItem, PropertyFlow, PropertyValue, credential_cell_badge,
    credential_status_badge, credential_status_text, reconnect_target,
};
use serde_json::{Value as Json, json};

use crate::common::{
    self, BoundsCell, bounds_cell, cached_object, engine_body, tag_pill, track_bounds,
};
use crate::config::{self, Col, DEFAULT_WIDTH, MIN_WIDTH, NAME_MIN_WIDTH};
use crate::context_menu::{MenuEvent, RowContextMenu};
use crate::gallery::js_number_string;
use crate::views::{ViewInput, ViewMsg};

/// What the table asks of the data view.
pub(crate) enum TableEvent {
    /// The entry row committed a name: create a record (`oncreate`).
    Create(String),
}

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
/// What every row draws whatever the columns.
const ROW_FIELDS: &[&str] = &[
    "name",
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

/// Payload of a header drag (column reorder).
#[derive(Clone)]
struct ColumnDrag {
    idx: usize,
    label: SharedString,
}

impl Render for ColumnDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .text_size(px(13.))
            .text_color(theme::muted())
            .opacity(0.9)
            .child(self.label.clone())
    }
}

/// A live resize gesture: `None` column = the Name column.
struct Resize {
    col: Option<usize>,
    start_x: Pixels,
    start_w: f32,
    snapshot: Vec<Col>,
}

struct Marquee {
    x0: Point<Pixels>,
    x1: Point<Pixels>,
}

pub(crate) struct SetTable {
    object_id: String,
    collection: Option<String>,
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
    editable_down: bool,
    sel_blocked: HashMap<String, String>,
    ctx_menu: Option<Entity<RowContextMenu>>,
    override_sort: Option<(String, bool)>,
    local_cols: Option<Vec<Col>>,
    name_local: Option<f32>,
    adding: Option<Entity<PropertyFlow>>,
    drag_col: Option<usize>,
    gap_idx: Option<usize>,
    gap_x: Option<Pixels>,
    resize: Option<Resize>,
    serving_by_id: HashMap<String, ServingInfo>,
    machines_now: Vec<MachineRow>,
    pending_new: bool,
    new_input: Entity<InputState>,
    committing: bool,
    row_bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    table_bounds: BoundsCell,
    scroll_bounds: BoundsCell,
    pop_bounds: BoundsCell,
    plus_bounds: BoundsCell,
    name_th_bounds: BoundsCell,
    header_bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<TableEvent> for SetTable {}
impl EventEmitter<ViewMsg> for SetTable {}

impl SetTable {
    pub(crate) fn new(
        object_id: String,
        collection: Option<String>,
        input: ViewInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let new_input = cx.new(|cx| InputState::new(window, cx).placeholder("Enter name"));
        let subs = vec![cx.subscribe_in(
            &new_input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.commit_new(false, window, cx),
                InputEvent::Blur => {
                    let empty = input.read(cx).value().trim().is_empty();
                    if this.pending_new {
                        this.commit_new(empty, window, cx);
                    }
                }
                _ => {}
            },
        )];
        let mut this = Self {
            object_id,
            collection,
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
            editable_down: false,
            sel_blocked: HashMap::new(),
            ctx_menu: None,
            override_sort: None,
            local_cols: None,
            name_local: None,
            adding: None,
            drag_col: None,
            gap_idx: None,
            gap_x: None,
            resize: None,
            serving_by_id: HashMap::new(),
            machines_now: Vec::new(),
            pending_new: false,
            new_input,
            committing: false,
            row_bounds: Rc::new(RefCell::new(Vec::new())),
            table_bounds: bounds_cell(),
            scroll_bounds: bounds_cell(),
            pop_bounds: bounds_cell(),
            plus_bounds: bounds_cell(),
            name_th_bounds: bounds_cell(),
            header_bounds: Rc::new(RefCell::new(Vec::new())),
            scroll: ScrollHandle::new(),
            _subs: subs,
        };
        this.load(false, cx);
        this
    }

    pub(crate) fn set_input(&mut self, input: ViewInput, cx: &mut Context<Self>) {
        // The table reloads on its body and sorts (the page's own refresh reloads it too).
        if input.body != self.input.body || input.sorts != self.input.sorts {
            self.input = input;
            self.load(false, cx);
        } else {
            self.input = input;
        }
    }

    pub(crate) fn row_ids(&self) -> impl Iterator<Item = &str> {
        self.rows.iter().map(|r| r.id.as_str())
    }

    fn stored_cols(&self, cx: &App) -> Vec<Col> {
        cached_object(&self.object_id, cx)
            .map(|o| config::stored_columns(&o.fields))
            .unwrap_or_default()
    }

    fn stored_name_width(&self, cx: &App) -> Option<f32> {
        cached_object(&self.object_id, cx).and_then(|o| config::stored_name_width(&o.fields))
    }

    fn cols(&self, cx: &App) -> Vec<Col> {
        self.local_cols
            .clone()
            .unwrap_or_else(|| self.stored_cols(cx))
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

    // ── Loading ──────────────────────────────────────────────────

    /// The engine sorts and filters; the table fetches a page at a time. Only
    /// what the engine can't compute (Served by, a sort by next repeat) needs
    /// every row. `keep_count`: a refresh keeps the rows already scrolled in.
    pub(crate) fn load(&mut self, keep_count: bool, cx: &mut Context<Self>) {
        let Some(body) = self.input.body.clone() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let rules = common::body_serving_rules(&body);
        let (key, desc) = self.sort_key_dir();
        let dir = if desc { "desc" } else { "asc" };
        let serving_sort = key == "serving";
        let repeat_sort = key == "repeat";
        let sorts: Vec<Json> = if serving_sort || repeat_sort {
            vec![json!({"key": "updatedAt", "type": "desc", "emptyPlacement": "end"})]
        } else if self.override_sort.is_some() {
            vec![json!({"key": key, "type": dir, "emptyPlacement": "end"})]
        } else if !self.input.sorts.is_empty() {
            self.input
                .sorts
                .iter()
                .map(|s| json!({"key": s.key, "type": s.type_str(), "emptyPlacement": "end"}))
                .collect()
        } else {
            vec![json!({"key": "updatedAt", "type": "desc", "emptyPlacement": "end"})]
        };
        let columns: Vec<String> = self.cols(cx).into_iter().map(|c| c.key).collect();
        let needs_every_row = serving_sort
            || repeat_sort
            || !rules.is_empty()
            || columns.iter().any(|c| c == "serving");
        if !needs_every_row {
            let mut fields: Vec<String> = ROW_FIELDS.iter().map(|s| (*s).to_string()).collect();
            for c in &columns {
                if !fields.contains(c) {
                    fields.push(c.clone());
                }
            }
            let query = engine_body(&body, json!({"sorts": sorts, "fields": fields}));
            let limit = if keep_count {
                PAGE.max(self.rows.len())
            } else {
                PAGE
            };
            let task = api::query(
                engine_body(&query, json!({"offset": 0, "limit": limit})),
                cx,
            );
            cx.spawn(async move |this, cx| {
                let Ok(res) = task.await else { return };
                let _ = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    this.page_query = Some(query);
                    this.rows = res.records;
                    this.total = res.total as usize;
                    this.after_rows(cx);
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        self.page_query = None;
        let task = api::query(engine_body(&body, json!({"sorts": sorts})), cx);
        let with_serving =
            columns.iter().any(|c| c == "serving") || !rules.is_empty() || serving_sort;
        cx.spawn(async move |this, cx| {
            let Ok(res) = task.await else { return };
            let mut all = res.records;
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
                let machines_task = cx.update(|cx| fetch_machines(cx));
                let Ok((machine_rows, machines)) = machines_task.await else {
                    return;
                };
                let ids: Vec<String> = all.iter().map(|r| r.id.clone()).collect();
                let resolve = cx.update(|cx| resolve_many(&ids, cx));
                let resolved = resolve.await.unwrap_or_default();
                let icons: Vec<String> = cx.update(|cx| {
                    let store = Store::global(cx).read(cx);
                    let space = current_space_id(cx);
                    resolved
                        .iter()
                        .map(|s| {
                            if s.machine_id.is_empty() {
                                return String::new();
                            }
                            let emoji = machine_rows
                                .iter()
                                .find(|m| field_str(&m.fields, "machine_id") == s.machine_id)
                                .map(|m| field_str(&m.fields, "iconEmoji").to_string())
                                .unwrap_or_default();
                            object_icon(store, &emoji, "machine", &space)
                        })
                        .collect()
                });
                let mut map = HashMap::new();
                for (i, r) in all.iter().enumerate() {
                    let s = resolved.get(i).cloned().unwrap_or_default();
                    let copy = serving_copy(&s, &machines);
                    let label = if s.machine_id.is_empty() {
                        "No machine".to_string()
                    } else {
                        machine_name(&machines, &s.machine_id)
                    };
                    map.insert(
                        r.id.clone(),
                        ServingInfo {
                            text: copy.text,
                            label,
                            warning: copy.warning,
                            reason: s.reason.clone(),
                            machine_id: s.machine_id.clone(),
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
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if let Some((map, machines)) = serving {
                    this.serving_by_id = map;
                    this.machines_now = machines;
                }
                this.total = all.len();
                this.rows = all;
                this.after_rows(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The next page of a paged view, in the same order.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.page_query.clone() else {
            return;
        };
        if self.loading_more || self.rows.len() >= self.total {
            return;
        }
        self.loading_more = true;
        let generation = self.generation;
        let task = api::query(
            engine_body(&query, json!({"offset": self.rows.len(), "limit": PAGE})),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                if this.generation != generation {
                    return;
                }
                if let Ok(res) = res {
                    for r in res.records {
                        if !this.rows.iter().any(|x| x.id == r.id) {
                            this.rows.push(r);
                        }
                    }
                    this.total = res.total as usize;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn after_rows(&mut self, _cx: &mut Context<Self>) {
        let ids: Vec<&str> = self.rows.iter().map(|r| r.id.as_str()).collect();
        self.selected.retain(|s| ids.contains(&s.as_str()));
    }

    /// After a change: the same view, the same rows scrolled into view.
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.load(true, cx);
    }

    // ── Selection ────────────────────────────────────────────────

    fn set_selection(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if ids == self.selected {
            return;
        }
        self.selected = ids;
        let task = blockers_for(&self.selected, cx);
        cx.spawn(async move |this, cx| {
            if let Ok(map) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.sel_blocked = map;
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn toggle_row(&mut self, id: &str, cx: &mut Context<Self>) {
        self.anchor = id.to_string();
        let next = if self.selected.iter().any(|x| x == id) {
            self.selected.iter().filter(|x| *x != id).cloned().collect()
        } else {
            let mut n = self.selected.clone();
            n.push(id.to_string());
            n
        };
        self.set_selection(next, cx);
    }

    fn range_rows(&mut self, id: &str, cx: &mut Context<Self>) {
        let anchor = if self.anchor.is_empty() {
            self.selected.first().cloned().unwrap_or_default()
        } else {
            self.anchor.clone()
        };
        let a = self.rows.iter().position(|r| r.id == anchor);
        let b = self.rows.iter().position(|r| r.id == id);
        let (Some(a), Some(b)) = (a, b) else {
            self.toggle_row(id, cx);
            return;
        };
        let next = self.rows[a.min(b)..=a.max(b)]
            .iter()
            .map(|r| r.id.clone())
            .collect();
        self.set_selection(next, cx);
    }

    fn on_row_click(&mut self, e: &ClickEvent, id: &str, cx: &mut Context<Self>) {
        let m = e.modifiers();
        if m.platform || m.control {
            self.toggle_row(id, cx);
            return;
        }
        if m.shift {
            self.range_rows(id, cx);
            return;
        }
        // The click that ends a drag must not clear what the drag just selected.
        if self.drag_moved {
            self.drag_moved = false;
            return;
        }
        if !self.selected.is_empty() {
            self.set_selection(Vec::new(), cx);
            return;
        }
        common::open_object(id, cx);
    }

    /// Escape: close the cell editor, clear the selection, close the menu.
    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        let any = self.cell_edit.is_some() || !self.selected.is_empty() || self.ctx_menu.is_some();
        self.cell_edit = None;
        self.ctx_menu = None;
        self.set_selection(Vec::new(), cx);
        cx.notify();
        any
    }

    /// Arm the rubber band (the data view forwards presses from its whole area).
    pub(crate) fn start_marquee(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.marquee = Some(Marquee {
            x0: position,
            x1: position,
        });
        self.drag_moved = false;
        cx.notify();
    }

    fn marquee_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(m) = &mut self.marquee else { return };
        m.x1 = position;
        let (x0, x1) = (m.x0, m.x1);
        if (x1.x - x0.x).abs() + (x1.y - x0.y).abs() < px(4.) {
            return;
        }
        self.drag_moved = true;
        let rect = Bounds::from_corners(
            point(x0.x.min(x1.x), x0.y.min(x1.y)),
            point(x0.x.max(x1.x), x0.y.max(x1.y)),
        );
        let bounds = self.row_bounds.borrow();
        let hit: Vec<String> = self
            .rows
            .iter()
            .zip(bounds.iter())
            .filter(|(_, b)| {
                b.bottom() >= rect.top()
                    && b.top() <= rect.bottom()
                    && b.right() >= rect.left()
                    && b.left() <= rect.right()
            })
            .map(|(r, _)| r.id.clone())
            .collect();
        drop(bounds);
        self.set_selection(hit, cx);
        cx.notify();
    }

    fn end_marquee(&mut self, cx: &mut Context<Self>) {
        // A plain click on the whitespace around the rows clears, like a plain row click.
        if let Some(m) = self.marquee.take()
            && !self.drag_moved
            && !self.selected.is_empty()
            && !self.row_bounds.borrow().iter().any(|b| b.contains(&m.x0))
        {
            self.set_selection(Vec::new(), cx);
        }
        cx.notify();
    }

    // ── Cells ────────────────────────────────────────────────────

    fn is_editable(relations: &[RelationDef], key: &str) -> bool {
        // Repeat is a rule edited in the object's Repeat row, not a cell value.
        key != "repeat" && common::find_rel(relations, key).is_some_and(|r| !r.read_only)
    }

    fn format_of(relations: &[RelationDef], key: &str) -> String {
        common::find_rel(relations, key)
            .map(|r| r.format.clone())
            .unwrap_or_default()
    }

    fn on_cell_click(
        &mut self,
        record: &str,
        key: &str,
        relations: &[RelationDef],
        cx: &mut Context<Self>,
    ) {
        // The mouseup ending a row-drag must not open an editor.
        if self.drag_moved {
            self.drag_moved = false;
            return;
        }
        if Self::format_of(relations, key) == "checkbox" {
            // Checkbox cells toggle on click, no menu.
            let cur = self
                .rows
                .iter()
                .find(|r| r.id == record)
                .and_then(|r| r.fields.get(key))
                .and_then(|v| v.bool_value)
                == Some(true);
            let task = api::note::set_field(record, key, Value::bool(!cur), cx);
            self.after(task, cx);
            return;
        }
        let same = self
            .cell_edit
            .as_ref()
            .is_some_and(|(r, k)| r == record && k == key);
        self.cell_edit = if same {
            None
        } else {
            Some((record.to_string(), key.to_string()))
        };
        cx.notify();
    }

    /// Await a mutation, then reload.
    fn after(&mut self, task: Task<anyhow::Result<roostr_core::JsonMap>>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                log::error!("{error:#}");
            }
            let _ = this.update(cx, |this, cx| this.reload(cx));
        })
        .detach();
    }

    fn cell_save(&mut self, v: Value, relations: &[RelationDef], cx: &mut Context<Self>) {
        let Some((record, key)) = self.cell_edit.clone() else {
            return;
        };
        let f = Self::format_of(relations, if key == "serving" { SERVING_KEY } else { &key });
        // Single-value editors close on save; tag/object stay open for more picks.
        if f != "tag" && f != "object" {
            self.cell_edit = None;
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
            let machine_id = self
                .machines_now
                .iter()
                .find(|m| m.id == target)
                .map(|m| m.machine_id.clone())
                .unwrap_or_default();
            if machine_id.is_empty() {
                api::note::delete_field(&record, SERVING_KEY, cx)
            } else {
                api::note::set_field(&record, SERVING_KEY, Value::string(machine_id), cx)
            }
        } else {
            api::note::set_field(&record, &key, v, cx)
        };
        self.after(task, cx);
        cx.notify();
    }

    fn cell_text(&self, r: &QueryRow, key: &str, relations: &[RelationDef], cx: &App) -> String {
        match key {
            "serving" => {
                return self
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
            "type" => return common::type_name(&r.type_key, cx),
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
        let format = Self::format_of(relations, key);
        // Object values come as a list, a bare link, or a bare id: all read as names.
        if format == "object" || v.link_value.is_some() {
            return common::link_names(v, cx);
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

    fn toggle_sort(&mut self, key: &str, cx: &mut Context<Self>) {
        let (cur, desc) = self.sort_key_dir();
        self.override_sort = Some(if cur == key {
            (key.to_string(), !desc)
        } else {
            (key.to_string(), false)
        });
        self.load(false, cx);
        cx.notify();
    }

    // ── Columns ──────────────────────────────────────────────────

    fn persist_view(
        &mut self,
        next: Vec<Col>,
        name_w: Option<f32>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let task = api::note::set_field(
            &self.object_id,
            "viewRelations",
            config::columns_value(&next, name_w),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            let _ = this.update(cx, |this, cx| {
                // Wait for the refetched view object before dropping the local state.
                this.local_cols = None;
                this.name_local = None;
                this.reload(cx);
            });
        })
    }

    fn save_columns(&mut self, next: Vec<Col>, cx: &mut Context<Self>) {
        self.local_cols = Some(next.clone());
        let name_w = self.stored_name_width(cx);
        self.persist_view(next, name_w, cx).detach();
        cx.notify();
    }

    fn drop_column(&mut self, cx: &mut Context<Self>) {
        let from = self.drag_col.take();
        let gap = self.gap_idx.take();
        self.gap_x = None;
        cx.notify();
        let (Some(from), Some(gap)) = (from, gap) else {
            return;
        };
        let mut next = self.cols(cx);
        if from >= next.len() {
            return;
        }
        let moved = next.remove(from);
        next.insert(if gap > from { gap - 1 } else { gap }, moved);
        self.save_columns(next, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn column_drag_over(
        &mut self,
        i: usize,
        from: usize,
        header: Bounds<Pixels>,
        x: Pixels,
        force_after: bool,
        n: usize,
        cx: &mut Context<Self>,
    ) {
        let after = force_after || x > header.left() + header.size.width / 2.;
        let gap = if force_after {
            n
        } else if after {
            i + 1
        } else {
            i
        };
        self.drag_col = Some(from);
        if gap == from || gap == from + 1 {
            self.gap_idx = None;
            self.gap_x = None;
        } else {
            self.gap_idx = Some(gap);
            let edge = if force_after {
                header.left()
            } else if after {
                header.right()
            } else {
                header.left()
            };
            self.gap_x = Some(edge - self.table_bounds.get().left());
        }
        cx.notify();
    }

    fn mouse_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(resize) = &self.resize {
            let w = (resize.start_w + f32::from(position.x - resize.start_x)).max(MIN_WIDTH);
            match resize.col {
                Some(idx) => {
                    let mut snap = resize.snapshot.clone();
                    if let Some(c) = snap.get_mut(idx) {
                        c.width = w;
                    }
                    self.local_cols = Some(snap);
                }
                None => self.name_local = Some(w.round()),
            }
            cx.notify();
            return;
        }
        if self.marquee.is_some() {
            self.marquee_move(position, cx);
        }
    }

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        if let Some(resize) = self.resize.take() {
            match resize.col {
                Some(_) => {
                    let cols = self.local_cols.clone().unwrap_or(resize.snapshot);
                    self.save_columns(cols, cx);
                }
                None => {
                    let cols = self.cols(cx);
                    let name_w = self.name_local;
                    self.persist_view(cols, name_w, cx).detach();
                }
            }
            return;
        }
        if self.marquee.is_some() {
            self.end_marquee(cx);
        }
    }

    // ── Entry row ────────────────────────────────────────────────

    pub(crate) fn begin_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_new = true;
        self.new_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn commit_new(&mut self, cancel: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.committing {
            return;
        }
        let name = self.new_input.read(cx).value().trim().to_string();
        self.pending_new = false;
        self.new_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
        if cancel {
            return;
        }
        cx.emit(TableEvent::Create(name));
    }

    /// The data view finished creating the entry row's record.
    pub(crate) fn created(&mut self, cx: &mut Context<Self>) {
        self.committing = false;
        self.load(false, cx);
    }

    pub(crate) fn set_committing(&mut self, committing: bool) {
        self.committing = committing;
    }

    // ── Context menu ─────────────────────────────────────────────

    fn open_menu(
        &mut self,
        id: String,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = if self.selected.contains(&id) {
            self.selected.clone()
        } else {
            vec![id]
        };
        let Some(object) = cached_object(&self.object_id, cx) else {
            return;
        };
        let space = field_str(&object.fields, "channel").to_string();
        let columns: Vec<String> = self.cols(cx).into_iter().map(|c| c.key).collect();
        let fields: HashMap<String, roostr_core::Fields> = self
            .rows
            .iter()
            .filter(|r| targets.contains(&r.id))
            .map(|r| (r.id.clone(), r.fields.clone()))
            .collect();
        let collection = self.collection.clone();
        let menu = cx.new(|cx| {
            RowContextMenu::new(
                position, targets, space, collection, columns, fields, window, cx,
            )
        });
        cx.subscribe(&menu, |this, _, event: &MenuEvent, cx| {
            match event {
                MenuEvent::Close => {
                    this.ctx_menu = None;
                    this.set_selection(Vec::new(), cx);
                }
                MenuEvent::Changed => {
                    this.reload(cx);
                    cx.emit(ViewMsg::Changed);
                }
            }
            cx.notify();
        })
        .detach();
        self.ctx_menu = Some(menu);
        cx.notify();
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .selected
            .iter()
            .filter(|id| !self.sel_blocked.contains_key(*id))
            .cloned()
            .collect();
        self.set_selection(Vec::new(), cx);
        cx.spawn(async move |this, cx| {
            for id in ids {
                let _ = cx.update(|cx| api::note::delete(&id, cx)).await;
            }
            let _ = this.update(cx, |this, cx| this.reload(cx));
        })
        .detach();
    }

    /// The property flow's list: the view's columns (on), then the specials not shown (off).
    fn flow_items(&self, cols: &[Col], cx: &App) -> Vec<FlowItem> {
        let Some(object) = cached_object(&self.object_id, cx) else {
            return Vec::new();
        };
        let relations = common::scoped_relations(&object, cx);
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

    fn open_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.adding.take().is_some() {
            cx.notify();
            return;
        }
        let Some(object) = cached_object(&self.object_id, cx) else {
            return;
        };
        let items = self.flow_items(&self.cols(cx), cx);
        let space =
            roostr_core::relations::object_space_id(Store::global(cx).read(cx), &object.fields);
        let flow =
            cx.new(|cx| PropertyFlow::new(items, vec![SERVING_KEY.to_string()], space, window, cx));
        cx.subscribe(&flow, |this, flow, event: &FlowEvent, cx| {
            match event {
                FlowEvent::Toggle(key, on) => {
                    let mut cols = this.cols(cx);
                    if *on {
                        cols.push(Col {
                            key: key.clone(),
                            width: DEFAULT_WIDTH,
                        });
                    } else {
                        cols.retain(|c| c.key != *key);
                    }
                    let items = this.flow_items(&cols, cx);
                    flow.update(cx, |f, cx| f.set_items(items, cx));
                    this.save_columns(cols, cx);
                }
                FlowEvent::Add(rel) => {
                    let mut cols = this.cols(cx);
                    cols.push(Col {
                        key: rel.key.clone(),
                        width: DEFAULT_WIDTH,
                    });
                    this.save_columns(cols, cx);
                }
                FlowEvent::Close => this.adding = None,
            }
            cx.notify();
        })
        .detach();
        self.adding = Some(flow);
        cx.notify();
    }
}

// ── Rendering ───────────────────────────────────────────────────

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

impl SetTable {
    #[allow(clippy::too_many_arguments)]
    fn header_cell(
        &self,
        i: usize,
        c: &Col,
        width: f32,
        relations: &[RelationDef],
        sort: &(String, bool),
        n: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = Self::col_name(&c.key, relations);
        let icon = common::find_rel(relations, &c.key)
            .and_then(|r| r.icon_emoji.clone())
            .filter(|e| !e.is_empty());
        let arrow = if sort.0 == c.key {
            if sort.1 { "↓" } else { "↑" }
        } else {
            ""
        };
        let group: SharedString = format!("th-{}", c.key).into();
        let key = c.key.clone();
        let key2 = c.key.clone();
        let dragging = self.drag_col == Some(i);
        let headers = self.header_bounds.clone();
        let headers2 = self.header_bounds.clone();
        div()
            .id(SharedString::from(format!("th-{}", c.key)))
            .group(group.clone())
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .when(dragging, |d| d.bg(theme::accent().opacity(0.14)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_drag(
                ColumnDrag {
                    idx: i,
                    label: label.clone().into(),
                },
                |d, _, _, cx| cx.new(|_| d.clone()),
            )
            .on_drag_move(
                cx.listener(move |this, e: &DragMoveEvent<ColumnDrag>, _, cx| {
                    let Some(b) = headers.borrow().get(i + 1).copied() else {
                        return;
                    };
                    if b.contains(&e.event.position) {
                        let from = e.drag(cx).idx;
                        this.column_drag_over(i, from, b, e.event.position.x, false, n, cx);
                    }
                }),
            )
            .on_drop(cx.listener(|this, _: &ColumnDrag, _, cx| this.drop_column(cx)))
            .child(
                div()
                    .id(SharedString::from(format!("head-{}", c.key)))
                    .flex()
                    .items_center()
                    .max_w_full()
                    .px(px(10.))
                    .py(px(6.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::muted())
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(dragging, |d| d.opacity(0.55))
                    .hover(|s| s.text_color(theme::fg()))
                    .when_some(icon, |d, icon| {
                        d.child(div().flex_none().mr(px(5.)).text_size(px(12.)).child(icon))
                    })
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(format!("{label} {arrow}")),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_sort(&key, cx))),
            )
            .child(
                div()
                    .id(SharedString::from(format!("hide-{}", c.key)))
                    .absolute()
                    .right(px(8.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .px(px(5.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(theme::bg())
                            .text_size(px(13.))
                            .text_color(theme::muted())
                            .opacity(0.)
                            .group_hover(group.clone(), |s| s.opacity(1.))
                            .hover(|s| s.text_color(theme::red()))
                            .child("×"),
                    )
                    .tooltip(tooltip("Hide column"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let next = this
                            .cols(cx)
                            .into_iter()
                            .filter(|x| x.key != key2)
                            .collect();
                        this.save_columns(next, cx);
                    })),
            )
            .child(self.resize_grip(
                SharedString::from(format!("grip-{}", c.key)),
                group,
                Some(i),
                c.width,
                cx,
            ))
            .child(
                canvas(
                    move |b, _, _| {
                        let mut h = headers2.borrow_mut();
                        if h.len() <= i + 1 {
                            h.resize(i + 2, Bounds::default());
                        }
                        h[i + 1] = b;
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .into_any_element()
    }

    fn resize_grip(
        &self,
        id: SharedString,
        group: SharedString,
        col: Option<usize>,
        start_w: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let name_bounds = self.name_th_bounds.clone();
        div()
            .id(id)
            .absolute()
            .top_0()
            .right_0()
            .w(px(11.))
            .h_full()
            .cursor_col_resize()
            .group_hover(group, |s| s.bg(theme::accent().opacity(0.25)))
            .hover(|s| s.bg(theme::accent().opacity(0.6)))
            .tooltip(tooltip("Drag to resize"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let start_w = match col {
                        Some(_) => start_w,
                        None => this
                            .name_local
                            .or(this.stored_name_width(cx))
                            .unwrap_or(f32::from(name_bounds.get().size.width)),
                    };
                    this.resize = Some(Resize {
                        col,
                        start_x: e.position.x,
                        start_w,
                        snapshot: this.cols(cx),
                    });
                }),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn body_cell(
        &self,
        r: &QueryRow,
        c: &str,
        width: f32,
        relations: &[RelationDef],
        dragging: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let base = || {
            div()
                .flex_none()
                .w(px(width))
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(10.))
                .py(px(7.))
                .overflow_hidden()
                .whitespace_nowrap()
                .when(dragging, |d| d.bg(theme::accent().opacity(0.14)))
        };
        let target = reconnect_target(r, c);
        if r.type_key == "credential" && c == "status" {
            // The harness writes a credential's status: shown in words, never hand-edited here.
            let st = field_str(&r.fields, "status").to_string();
            if let Some(target) = target {
                let (text, glyph, color) = credential_cell_badge(r, cx);
                let colors = badge_colors(color);
                let name = field_str(&r.fields, "name");
                let label = format!(
                    "Reconnect {}: {text}",
                    if name.is_empty() { "credential" } else { name }
                );
                return base()
                    .child(
                        CredentialCell::new(
                            SharedString::from(format!("cred-{}", r.id)),
                            r.clone(),
                            target,
                            label.into(),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .text_color(colors.fg)
                                .child(Icon::new(glyph).size(px(14.)).text_color(colors.icon))
                                .child(text),
                        ),
                    )
                    .into_any_element();
            }
            let (glyph, color) = credential_status_badge(&st);
            let colors = badge_colors(color);
            return base()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .text_color(colors.fg)
                        .child(Icon::new(glyph).size(px(14.)).text_color(colors.icon))
                        .child(credential_status_text(&st)),
                )
                .into_any_element();
        }
        if let Some(target) = target
            && c == "error"
        {
            let text = self.cell_text(r, c, relations, cx);
            return base()
                .child(
                    CredentialCell::new(
                        SharedString::from(format!("err-{}", r.id)),
                        r.clone(),
                        target,
                        format!("Reconnect: {text}").into(),
                    )
                    .child(
                        div()
                            .text_color(rgb(0xf55522))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(text),
                    ),
                )
                .into_any_element();
        }
        let locked = is_locked_tool(&r.fields);
        if Self::is_editable(relations, c) && !locked {
            let fmt = Self::format_of(relations, c);
            let rel = common::find_rel(relations, c).cloned();
            let record = r.id.clone();
            let key = c.to_string();
            let rels = relations.to_vec();
            let mut cell = base()
                .id(SharedString::from(format!("cell-{}-{}", r.id, c)))
                .cursor_pointer()
                .hover(|s| s.bg(theme::hl_light()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.editable_down = true),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.on_cell_click(&record, &key, &rels, cx)
                }));
            match fmt.as_str() {
                "tag" | "status" => {
                    // Inline-block pills on the text baseline grow the CSS line box to ~23px.
                    let items = common::string_items(r.fields.get(c));
                    if !items.is_empty() {
                        cell = cell.min_h(px(37.));
                    }
                    for t in items {
                        let color = rel
                            .as_ref()
                            .and_then(|rel| rel.options.iter().find(|o| o.text == t))
                            .map(|o| o.color.clone())
                            .unwrap_or_default();
                        cell = cell.child(tag_pill(t, &color, false));
                    }
                }
                "checkbox" => {
                    let on = r.fields.get(c).and_then(|v| v.bool_value) == Some(true);
                    cell = cell.min_h(px(37.)).child(
                        CheckboxIcon::new(on).size(px(20.)).color(if on {
                            theme::fg()
                        } else {
                            theme::muted()
                        }),
                    );
                }
                _ => {
                    let text = self.cell_text(r, c, relations, cx);
                    let warn = c == "error" && !text.is_empty();
                    cell = cell.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .when(warn, |d| d.text_color(rgb(0xf55522)))
                            .child(text),
                    );
                }
            }
            return cell.into_any_element();
        }
        // Specials and read-only columns.
        let muted = matches!(c, "type" | "createdAt" | "updatedAt");
        let serving = self.serving_by_id.get(&r.id).cloned();
        let warn = c == "serving" && serving.as_ref().is_some_and(|s| s.warning);
        let space = current_space_id(cx);
        let lead = if c == "type" {
            Some(type_icon(Store::global(cx).read(cx), &r.type_key, &space))
                .filter(|s| !s.is_empty())
        } else if c == "serving" {
            serving
                .as_ref()
                .map(|s| s.icon.clone())
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        let text = self.cell_text(r, c, relations, cx);
        let mut cell = base()
            .id(SharedString::from(format!("cell-{}-{}", r.id, c)))
            .when(muted, |d| d.text_color(theme::muted()))
            .when(warn, |d| d.text_color(rgb(0xf55522)))
            .when_some(lead, |d, icon| {
                d.child(div().flex_none().mr(px(2.)).child(icon))
            })
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(text),
            );
        if c == "serving" {
            if let Some(s) = &serving {
                cell = cell.tooltip(tooltip(s.text.clone()));
            }
            if !locked {
                let record = r.id.clone();
                let rels = relations.to_vec();
                cell = cell
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hl_light()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.editable_down = true),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.on_cell_click(&record, "serving", &rels, cx)
                    }));
            }
        }
        cell.into_any_element()
    }

    /// The cell editor popover (PropertyValue at the cell).
    fn cell_pop(
        &self,
        relations: &[RelationDef],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (record, key) = self.cell_edit.clone()?;
        let edit_key = if key == "serving" {
            SERVING_KEY
        } else {
            key.as_str()
        };
        let rel = common::find_rel(relations, edit_key)?.clone();
        let row = self.rows.iter().find(|r| r.id == record)?;
        let wide = rel.format == "longtext" || rel.key == "error";
        let seeds = cached_object(&self.object_id, cx)
            .map(|o| roostr_core::view_seeds::seeds_from_view(&o.fields, relations))
            .unwrap_or_default();
        let entity = cx.entity().downgrade();
        let rels = relations.to_vec();
        let pop_bounds = self.pop_bounds.clone();
        Some(
            div()
                .relative()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::border())
                .rounded(px(10.))
                .p(px(8.))
                .shadow(roostr_core::ui::menu_shadow())
                // `.cell-pop.wide`: 440px, `max-width: calc(100vw - 24px)` (content box).
                .when(wide, |d| {
                    d.w(px(458.).min(window.viewport_size().width - px(24.) + px(18.)))
                })
                .occlude()
                .child(track_bounds(&self.pop_bounds))
                .child(
                    PropertyValue::new(
                        SharedString::from(format!("pop-{record}-{key}")),
                        rel.clone(),
                        row.fields.get(&rel.key).cloned(),
                    )
                    .space(field_str(&row.fields, "channel").to_string())
                    .seeds(seeds)
                    .auto_open(true)
                    .on_save(move |v, _, cx| {
                        let v = v.clone();
                        let rels = rels.clone();
                        let _ = entity.update(cx, |this, cx| this.cell_save(v, &rels, cx));
                    }),
                )
                .on_mouse_down_out(cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    if pop_bounds.get().contains(&e.position) {
                        return;
                    }
                    // A click on an editable cell moves/toggles the editor itself.
                    this.editable_down = false;
                    cx.defer_in_window_or(move |this: &mut Self, cx: &mut Context<Self>| {
                        if !this.editable_down {
                            this.cell_edit = None;
                            cx.notify();
                        }
                        this.editable_down = false;
                    });
                }))
                .into_any_element(),
        )
    }
}

trait DeferExt<T> {
    fn defer_in_window_or(&mut self, f: impl FnOnce(&mut T, &mut Context<T>) + 'static);
}

impl<T: 'static> DeferExt<T> for Context<'_, T> {
    /// Run `f` on this entity once the current event has finished dispatching.
    fn defer_in_window_or(&mut self, f: impl FnOnce(&mut T, &mut Context<T>) + 'static) {
        let entity = self.entity().downgrade();
        self.defer(move |cx| {
            let _ = entity.update(cx, f);
        });
    }
}

impl Render for SetTable {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = cached_object(&self.object_id, cx) else {
            return div();
        };
        let relations = common::scoped_relations(&object, cx);
        let cols = self.cols(cx);
        let n = cols.len();
        let name_w = self
            .name_local
            .or(config::stored_name_width(&object.fields));
        let avail = f32::from(self.scroll_bounds.get().size.width);
        let (name_width, widths, plus_w) = layout_widths(avail, name_w, &cols);
        let table_w = name_width + widths.iter().sum::<f32>() + plus_w;
        let sort = self.sort_key_dir();

        // Header.
        let name_group: SharedString = "th-name".into();
        let name_arrow = if sort.0 == "name" {
            if sort.1 { "↓" } else { "↑" }
        } else {
            ""
        };
        let name_bounds = self.name_th_bounds.clone();
        let mut header = div()
            .flex()
            .h(px(35.))
            .border_b_1()
            .border_color(theme::border())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("th-name")
                    .group(name_group.clone())
                    .relative()
                    .flex_none()
                    .w(px(name_width))
                    .h_full()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child(track_bounds(&name_bounds))
                    .child(
                        div()
                            .id("head-name")
                            .px(px(10.))
                            .py(px(6.))
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::muted())
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme::fg()))
                            .child(format!("Name {name_arrow}"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_sort("name", cx))),
                    )
                    .child(self.resize_grip("grip-name".into(), name_group, None, 0., cx)),
            );
        for (i, c) in cols.iter().enumerate() {
            header = header.child(self.header_cell(i, c, widths[i], &relations, &sort, n, cx));
        }
        // The "+" column's drop target (its button is pinned on top, see below).
        let plus_bounds = self.plus_bounds.clone();
        header = header.child(
            div()
                .id("th-plus-drop")
                .flex_none()
                .w(px(plus_w))
                .h_full()
                .on_drag_move(
                    cx.listener(move |this, e: &DragMoveEvent<ColumnDrag>, _, cx| {
                        if e.bounds.contains(&e.event.position) {
                            let from = e.drag(cx).idx;
                            let b = e.bounds;
                            let _ = &plus_bounds;
                            this.column_drag_over(
                                n.saturating_sub(1),
                                from,
                                b,
                                e.event.position.x,
                                true,
                                n,
                                cx,
                            );
                        }
                    }),
                )
                .on_drop(cx.listener(|this, _: &ColumnDrag, _, cx| this.drop_column(cx))),
        );

        // Body rows.
        let mut rows_el = div().flex().flex_col();
        let row_bounds = self.row_bounds.clone();
        let n_rows = self.rows.len();
        rows_el = rows_el.on_children_prepainted(move |bounds, _, _| {
            *row_bounds.borrow_mut() = bounds.into_iter().take(n_rows).collect();
        });
        let space = current_space_id(cx);
        for r in &self.rows {
            let selected = self.selected.contains(&r.id);
            let id = r.id.clone();
            let id_aux = r.id.clone();
            let id_ctx = r.id.clone();
            let task = Store::global(cx).read(cx).layout_of(&r.type_key) == "task";
            let done = r.fields.get("done").and_then(|v| v.bool_value) == Some(true);
            let lead: AnyElement = if task {
                let rid = r.id.clone();
                div()
                    .id(SharedString::from(format!("done-{}", r.id)))
                    .flex_none()
                    .flex()
                    .items_center()
                    .mr(px(6.))
                    .cursor_pointer()
                    .text_color(if done {
                        theme::accent()
                    } else {
                        theme::muted()
                    })
                    .hover(|s| if done { s } else { s.text_color(theme::fg()) })
                    .child(CheckboxIcon::new(done).size(px(18.)).color(if done {
                        theme::accent()
                    } else {
                        theme::muted()
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let task = api::note::set_field(&rid, "done", Value::bool(!done), cx);
                        this.after(task, cx);
                    }))
                    .into_any_element()
            } else {
                let icon = object_icon(
                    Store::global(cx).read(cx),
                    field_str(&r.fields, "iconEmoji"),
                    &r.type_key,
                    &space,
                );
                div().flex_none().mr(px(4.)).child(icon).into_any_element()
            };
            let mut row = div()
                .id(SharedString::from(format!("row-{}", r.id)))
                .flex()
                .border_b_1()
                .border_color(theme::border())
                .text_size(px(13.))
                .cursor_pointer()
                .when(selected, |d| d.bg(rgba(0x377aff40)))
                .when(!selected, |d| d.hover(|s| s.bg(theme::hover())))
                .on_click(
                    cx.listener(move |this, e: &ClickEvent, _, cx| this.on_row_click(e, &id, cx)),
                )
                .on_mouse_down(MouseButton::Middle, move |_, _, cx| {
                    common::open_object_tab(&id_aux, cx)
                })
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_menu(id_ctx.clone(), e.position, window, cx)
                    }),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(name_width))
                        .flex()
                        .items_center()
                        .px(px(10.))
                        .py(px(7.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_weight(FontWeight(550.))
                        .child(lead)
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(common::row_name(r)),
                        ),
                );
            for (i, c) in cols.iter().enumerate() {
                row = row.child(self.body_cell(
                    r,
                    &c.key,
                    widths[i],
                    &relations,
                    self.drag_col == Some(i),
                    cx,
                ));
            }
            row = row.child(div().flex_none().w(px(plus_w)));
            rows_el = rows_el.child(row);
        }
        if self.page_query.is_some() && self.rows.len() < self.total {
            // The next page loads as this comes within a screen of view.
            let entity = cx.entity().downgrade();
            rows_el = rows_el.child(
                div()
                    .relative()
                    .px(px(10.))
                    .py(px(7.))
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(px(13.))
                    .child(format!(
                        "Loading more… {} of {}",
                        self.rows.len(),
                        self.total
                    ))
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                if bounds.top() < window.viewport_size().height + px(800.) {
                                    let entity = entity.clone();
                                    cx.defer(move |cx| {
                                        let _ = entity.update(cx, |this, cx| this.load_more(cx));
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            );
        }
        // The entry row and "+ New Object".
        if self.pending_new {
            rows_el = rows_el.child(
                div()
                    .py(px(4.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .max_w(px(444.))
                            .border_2()
                            .border_color(theme::accent())
                            .rounded(px(8.))
                            .px(px(10.))
                            .py(px(8.))
                            // The Input binds Escape itself: catch its action first.
                            .capture_action(cx.listener(|this, _: &InputEscape, window, cx| {
                                cx.stop_propagation();
                                this.commit_new(true, window, cx);
                            }))
                            .child(
                                Input::new(&self.new_input)
                                    .appearance(false)
                                    .text_size(px(13.)),
                            ),
                    ),
            );
        }
        rows_el = rows_el.child(
            div()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("new-object")
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .w_full()
                        .px(px(8.))
                        .py(px(10.))
                        .text_size(px(14.))
                        .text_color(theme::muted())
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme::fg()))
                        .child("＋ New Object")
                        .on_click(cx.listener(|this, _, window, cx| this.begin_new(window, cx))),
                ),
        );

        let table = div()
            .relative()
            .w(px(table_w))
            .child(track_bounds(&self.table_bounds))
            .child(header)
            .child(rows_el)
            .when_some(self.gap_x, |d, x| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(x - px(1.5))
                        .w(px(3.))
                        .rounded(px(2.))
                        .bg(theme::accent())
                        .shadow(vec![common::shadow(
                            theme::accent().opacity(0.22),
                            0.,
                            0.,
                            0.,
                            3.,
                        )]),
                )
            });

        // The "+" header button, pinned to the right edge (sticky).
        let plus_open = self.adding.is_some();
        let plus = div()
            .absolute()
            .top_0()
            .right_0()
            .w(px(40.))
            .h(px(35.))
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::bg())
            .border_b_1()
            .border_color(theme::border())
            .shadow(vec![common::shadow(
                hsla(0., 0., 0., 0.6),
                -8.,
                0.,
                8.,
                -8.,
            )])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .relative()
                    .child(track_bounds(&self.plus_bounds))
                    .child(
                        div()
                            .id("add-column")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(26.))
                            .border_1()
                            .border_color(theme::border())
                            .rounded(px(7.))
                            .text_size(px(17.))
                            .text_color(theme::fg())
                            .cursor_pointer()
                            .hover(|s| {
                                s.border_color(theme::accent())
                                    .text_color(theme::accent())
                                    .bg(rgba(0x377aff1f))
                            })
                            .tooltip(tooltip("Add column"))
                            .child("+")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_adding(window, cx)),
                            ),
                    )
                    .when_some(self.adding.clone().filter(|_| plus_open), |d, flow| {
                        d.child(common::floating(common::Place::BelowLeft, 4., flow))
                    }),
            );

        let entity = cx.entity().downgrade();
        // Registered every frame (cheap no-ops when idle) so a press and the
        // moves that follow it are tracked even before the next frame paints.
        let active_gesture = true;
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .text_color(theme::fg())
            .child(
                div()
                    .relative()
                    .child(track_bounds(&self.scroll_bounds))
                    .child(
                        div()
                            .id("set-table-scroll")
                            .overflow_x_scroll()
                            .track_scroll(&self.scroll)
                            .child(table),
                    )
                    .child(plus),
            )
            // Global pointer tracking while a resize or marquee runs.
            .when(active_gesture, |d| {
                d.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let e1 = entity.clone();
                            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble {
                                    let _ =
                                        e1.update(cx, |this, cx| this.mouse_move(e.position, cx));
                                }
                            });
                            let e2 = entity.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble {
                                    let _ = e2.update(cx, |this, cx| this.mouse_up(cx));
                                }
                            });
                        },
                    )
                    .absolute()
                    .size_0(),
                )
            });

        if let Some(pop) = self.cell_pop(&relations, window, cx) {
            // Anchored at the cell's bottom-left (+4px).
            let cell = self.cell_edit.as_ref().and_then(|(rid, key)| {
                let ri = self.rows.iter().position(|r| r.id == *rid)?;
                let rb = *self.row_bounds.borrow().get(ri)?;
                let ci = cols.iter().position(|c| c.key == *key)?;
                let left = rb.left() + px(name_width + widths[..ci].iter().sum::<f32>());
                Some(point(left, rb.bottom() + px(4.)))
            });
            if let Some(pos) = cell {
                // In-tree: the editor's inputs need their keyboard actions.
                root = root.child(
                    anchored()
                        .position(pos)
                        .snap_to_window_with_margin(px(12.))
                        .child(pop),
                );
            }
        }

        if !self.selected.is_empty() {
            let deletable = self
                .selected
                .iter()
                .filter(|id| !self.sel_blocked.contains_key(*id))
                .count();
            let mut bar = div()
                .id("sel-bar")
                .flex()
                .items_center()
                .gap(px(12.))
                .mt(px(8.))
                .mx_auto()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::border())
                .rounded(px(10.))
                .px(px(12.))
                .py(px(6.))
                .text_size(px(13.))
                .shadow(vec![common::shadow(hsla(0., 0., 0., 0.4), 0., 4., 16., 0.)])
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(format!("{} selected", self.selected.len()));
            if deletable > 0 {
                let label = if self.sel_blocked.is_empty() {
                    "Delete".to_string()
                } else {
                    format!(
                        "Delete {deletable} ({} protected kept)",
                        self.sel_blocked.len()
                    )
                };
                bar = bar.child(
                    div()
                        .id("sel-del")
                        .text_color(rgb(0xe05555))
                        .cursor_pointer()
                        .child(label)
                        .on_click(cx.listener(|this, _, _, cx| this.delete_selected(cx))),
                );
            } else {
                let why = self
                    .sel_blocked
                    .values()
                    .next()
                    .cloned()
                    .unwrap_or_default();
                bar = bar.child(
                    div()
                        .id("sel-locked")
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .tooltip(tooltip(
                            "Types and properties Roostr or your queries rely on can't be deleted",
                        ))
                        .child(format!("Can't delete: {why}")),
                );
            }
            bar = bar.child(
                div()
                    .id("sel-clear")
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .child("✕")
                    .on_click(cx.listener(|this, _, _, cx| this.set_selection(Vec::new(), cx))),
            );
            root = root.child(div().flex().child(bar));
        }
        if self.rows.is_empty() {
            root = root.child(
                div()
                    .px(px(10.))
                    .py(px(16.))
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child("No objects match."),
            );
        }
        if let Some(m) = &self.marquee
            && self.drag_moved
        {
            let origin = point(m.x0.x.min(m.x1.x), m.x0.y.min(m.x1.y));
            let size = size((m.x1.x - m.x0.x).abs(), (m.x1.y - m.x0.y).abs());
            root = root.child(
                deferred(
                    anchored().position(origin).child(
                        div()
                            .w(size.width)
                            .h(size.height)
                            .bg(rgba(0x377aff1f))
                            .border_1()
                            .border_color(rgba(0x377aff8c))
                            .rounded(px(2.)),
                    ),
                )
                .with_priority(1),
            );
        }
        let _ = window;
        root
    }
}

impl SetTable {
    /// The open row menu; the data view paints it above everything.
    pub(crate) fn ctx_menu(&self) -> Option<Entity<RowContextMenu>> {
        self.ctx_menu.clone()
    }
}

//! The dataview controls row (QueryControls.svelte): view chip, Group by /
//! Date chip, search, filter / sort panels, the view settings menu (layout
//! tiles + property list), and the accent New button (+ template picker).
//!
//! All of it is drawn from [`Controls`], kept in the view's [`DataView`];
//! handlers re-borrow it by the view's id.

use std::collections::{HashMap, HashSet};
use std::f32::consts::PI;
use std::rc::Rc;

use roostr_core::filters::space_filter_of;
use roostr_core::relations::{create_relation, current_space_id, format_glyph, object_space_id};
use roostr_core::{Object, RelationDef, State, Value, api, field_str};
use roostr_properties::KeyPicker;
use roostr_rt::Task;
use roostr_ui::motion::{self, CONTROL, HOVER_REVEAL};
use roostr_ui::overlay::{self, Placement};
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    Button, DropdownMenu, Glyph, Input, InputState, KitSize, PopupMenu, PopupMenuItem, Theme,
    Toggle, glyph, ty,
};
use rune::{
    Align, Color, Cursor, Cx, Drag, Effect, El, Key, KeyEvent, Phase, ScrollSpec, Side, Spring,
    SpringPair, Transition, Tx, Vec2, col, el, row, text,
};
use serde_json::{Value as Json, json};

use crate::common::{self, menu_surface, menu_title, separator, tag_pill, vrow, vrow_content};
use crate::config::{self, FilterRule, SortRule, ViewProp};
use crate::layout_icon::layout_icon;
use crate::{DataView, DataViewKind, dv, dv_mut};

/// A KeyPicker label function.
type KeyLabel = Rc<dyn Fn(&str, &State) -> String>;

const LAYOUTS: &[(&str, &str)] = &[
    ("table", "Table"),
    ("gallery", "Gallery"),
    ("kanban", "Kanban"),
    ("calendar", "Calendar"),
];
const USED_SAMPLE: u64 = 500;
const SYSTEM_DATES: &[(&str, &str)] = &[
    ("createdDate", "Created date"),
    ("modifiedDate", "Modified date"),
];
/// Lens handlers only run from elements of an open view.
const OPEN: &str = "the controls' data view is open";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    None,
    Filter,
    Sort,
}

/// Inputs of one filter rule's value editor. `*_dirty`: typed since the
/// last commit (a blur commits it, see [`poll`]).
#[derive(Default)]
struct RuleInputs {
    single: InputState,
    multi: InputState,
    single_dirty: bool,
    multi_dirty: bool,
}

impl RuleInputs {
    fn new(first: String) -> Self {
        Self {
            single: InputState::with_value(first),
            ..Self::default()
        }
    }
}

/// All controls-row UI state of one data view.
pub(crate) struct Controls {
    kind: DataViewKind,
    /// The view's record query (ranks the properties its records use first).
    body: Option<Json>,
    search_open: bool,
    search: InputState,
    open: Panel,
    sort_more: Option<usize>,
    sort_drag: Option<usize>,
    sort_over: Option<usize>,
    /// Window rects of the sort rules (drop targets of a sort drag).
    sort_rects: HashMap<usize, [f32; 4]>,
    settings_open: bool,
    prop_list_open: bool,
    chip_open: bool,
    used_keys: HashSet<String>,
    creating: bool,
    type_templates: Vec<(String, String)>,
    tpl_pick_open: bool,
    /// "＋ New … property" name entry (the web's `prompt`).
    new_prop: Option<InputState>,
    cond_open: Option<usize>,
    rule_inputs: HashMap<usize, RuleInputs>,
    /// Each object filter's search box (its values are picked, not typed).
    object_search: HashMap<usize, InputState>,
    /// How many rules the open panel showed before the last add: rules past
    /// it were just added and enter (opening the panel, re-keying, removing
    /// or reordering a rule does not animate).
    rules_base: usize,
}

impl Controls {
    pub(crate) fn new(kind: DataViewKind) -> Self {
        Self {
            kind,
            body: None,
            search_open: false,
            search: InputState::new(),
            open: Panel::None,
            sort_more: None,
            sort_drag: None,
            sort_over: None,
            sort_rects: HashMap::new(),
            settings_open: false,
            prop_list_open: false,
            chip_open: false,
            used_keys: HashSet::new(),
            creating: false,
            type_templates: Vec::new(),
            tpl_pick_open: false,
            new_prop: None,
            cond_open: None,
            rule_inputs: HashMap::new(),
            object_search: HashMap::new(),
            rules_base: 0,
        }
    }

    pub(crate) fn set_body(&mut self, body: Option<Json>) {
        self.body = body;
    }

    /// Close the floating menus (Escape).
    pub(crate) fn dismiss(&mut self) -> bool {
        let any = self.settings_open
            || self.chip_open
            || self.tpl_pick_open
            || self.sort_more.is_some()
            || self.cond_open.is_some();
        self.settings_open = false;
        self.chip_open = false;
        self.prop_list_open = false;
        self.tpl_pick_open = false;
        self.sort_more = None;
        self.cond_open = None;
        self.new_prop = None;
        any
    }
}

fn controls_mut<'a>(s: &'a mut State, id: &str) -> Option<&'a mut Controls> {
    dv_mut(s, id).map(|v| &mut v.controls)
}

// ── Templates on New (type pages) ─────────────────────────────

/// The view was just inserted: a type page loads its templates (New's chevron).
pub(crate) fn open(s: &mut State, id: &str) {
    if dv(s, id).is_none_or(|v| v.kind != DataViewKind::Type) {
        return;
    }
    let task = api::query(
        json!({"type": "template", "filters": [{"key": "target_type", "condition": "equal", "value": id}], "limit": 50}),
        s,
    );
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(res) = task.await else { return };
        let list = res
            .records
            .iter()
            .map(|r| (r.id.clone(), field_str(&r.fields, "name").to_string()))
            .collect();
        cx.update(|s| {
            if let Some(c) = controls_mut(s, &view) {
                c.type_templates = list;
            }
        });
    })
    .detach();
}

// ── Saving the view config ───────────────────────────────────

fn save_filters(s: &mut State, id: &str, next: Vec<FilterRule>) {
    let task = api::note::set_field(id, "viewFilters", config::filters_value(&next), s);
    task.detach_and_log_err(&s.rt);
}

fn save_sorts(s: &mut State, id: &str, next: Vec<SortRule>) {
    let task = api::note::set_field(id, "viewSorts", config::sorts_value(&next), s);
    task.detach_and_log_err(&s.rt);
}

/// A change that adds no rule: every rule shown from now on stays put.
fn settle_rules(s: &mut State, id: &str, len: usize) {
    if let Some(c) = controls_mut(s, id) {
        c.rules_base = len;
    }
}

fn update_filter(
    s: &mut State,
    id: &str,
    idx: usize,
    key: Option<String>,
    condition: Option<String>,
    value: Option<Vec<String>>,
) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    let relations = common::scoped_relations(s, &object);
    let mut next = config::parse_filters(&object.fields);
    let Some(rule) = next.get_mut(idx) else {
        return;
    };
    if let Some(v) = value {
        rule.value = v;
    }
    if let Some(c) = condition.clone() {
        rule.condition = c;
    }
    // Reset condition/value when the key's format changes the catalog.
    if let Some(k) = key {
        rule.condition = config::conditions_for(&relations, &k)[0].id.to_string();
        rule.key = k;
        rule.value.clear();
    }
    // A one-value condition keeps one value.
    if let Some(c) = condition
        && !config::multi_value(&c)
        && rule.value.len() > 1
    {
        rule.value.truncate(1);
    }
    if let Some(c) = controls_mut(s, id) {
        c.rule_inputs.remove(&idx);
        c.object_search.remove(&idx);
    }
    settle_rules(s, id, next.len());
    save_filters(s, id, next);
}

/// The stored values of filter rule `i`.
fn stored_values(s: &State, id: &str, i: usize) -> Vec<String> {
    s.store
        .object(id)
        .and_then(|o| {
            config::parse_filters(&o.fields)
                .get(i)
                .map(|f| f.value.clone())
        })
        .unwrap_or_default()
}

/// The value inputs of rule `i`, created on first use with the stored value.
fn rule_inputs<'a>(s: &'a mut State, id: &str, i: usize) -> &'a mut RuleInputs {
    let first = stored_values(s, id, i)
        .into_iter()
        .next()
        .unwrap_or_default();
    dv_mut(s, id)
        .expect(OPEN)
        .controls
        .rule_inputs
        .entry(i)
        .or_insert_with(|| RuleInputs::new(first))
}

/// One-value input: Enter or blur stores what it holds.
fn commit_single(s: &mut State, id: &str, i: usize) {
    let inputs = rule_inputs(s, id, i);
    inputs.single_dirty = false;
    let text = inputs.single.value().to_string();
    let current = stored_values(s, id, i);
    let next = if text.is_empty() { vec![] } else { vec![text] };
    if next != current {
        update_filter(s, id, i, None, None, Some(next));
    }
}

/// Several-values input: a comma, Enter or blur adds what was typed.
fn commit_multi(s: &mut State, id: &str, i: usize) {
    let inputs = rule_inputs(s, id, i);
    inputs.multi_dirty = false;
    let add = inputs
        .multi
        .value()
        .trim_end_matches(',')
        .trim()
        .to_string();
    inputs.multi.clear();
    if add.is_empty() {
        return;
    }
    let mut current = stored_values(s, id, i);
    if !current.contains(&add) {
        current.push(add);
        update_filter(s, id, i, None, None, Some(current));
    }
}

/// Per frame: commit the filter value inputs that lost focus with typed text
/// (GPUI's `InputEvent::Blur`), and keep an unfocused one-value input showing
/// the stored value. True when anything changed.
pub(crate) fn poll(s: &mut State, id: &str) -> bool {
    let Some(v) = dv(s, id) else { return false };
    if v.controls.rule_inputs.is_empty() {
        return false;
    }
    let (mut singles, mut multis) = (Vec::new(), Vec::new());
    for (i, r) in &v.controls.rule_inputs {
        if r.single_dirty && !r.single.is_focused() {
            singles.push(*i);
        }
        if r.multi_dirty && !r.multi.is_focused() {
            multis.push(*i);
        }
    }
    let mut changed = !singles.is_empty() || !multis.is_empty();
    for i in singles {
        commit_single(s, id, i);
    }
    for i in multis {
        if dv(s, id).is_some_and(|v| v.controls.rule_inputs.contains_key(&i)) {
            commit_multi(s, id, i);
        }
    }
    let filters = s
        .store
        .object(id)
        .map(|o| config::parse_filters(&o.fields))
        .unwrap_or_default();
    let Some(c) = controls_mut(s, id) else {
        return changed;
    };
    for (i, r) in c.rule_inputs.iter_mut() {
        let stored = filters
            .get(*i)
            .and_then(|f| f.value.first().cloned())
            .unwrap_or_default();
        if !r.single.is_focused() && !r.single_dirty && r.single.value() != stored {
            r.single.set_value(stored);
            changed = true;
        }
    }
    changed
}

// ── View layout ──────────────────────────────────────────────

/// Property keys this view's objects - or its source types' objects - hold a value for.
fn load_used_keys(s: &mut State, id: &str) {
    let Some(v) = dv(s, id) else { return };
    let (kind, body) = (v.kind, v.controls.body.clone());
    let Some(object) = s.store.object(id) else {
        return;
    };
    let type_key = field_str(&object.fields, "key").to_string();
    let types: Vec<String> = if kind == DataViewKind::Type {
        if type_key.is_empty() {
            vec![]
        } else {
            vec![type_key]
        }
    } else {
        config::sources(&object.fields)
    };
    let body_task = body
        .as_ref()
        .map(|b| api::query(common::engine_body(b, json!({"limit": USED_SAMPLE})), s));
    let types_task = (!types.is_empty()).then(|| {
        let space = space_filter_of(&object.fields, &current_space_id(s));
        api::query(
            json!({"filters": [{"key": "typeKey", "condition": "in", "value": types}, space], "limit": USED_SAMPLE}),
            s,
        )
    });
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        let mut keys = HashSet::new();
        for task in [body_task, types_task].into_iter().flatten() {
            if let Ok(res) = task.await {
                for r in res.records {
                    for (k, v) in r.fields {
                        if config::has_value(&v) {
                            keys.insert(k);
                        }
                    }
                }
            }
        }
        cx.update(|s| {
            if let Some(c) = controls_mut(s, &view) {
                c.used_keys = keys;
            }
        });
    })
    .detach();
}

fn group_choices(
    used_keys: &HashSet<String>,
    relations: &[RelationDef],
) -> (Vec<ViewProp>, Vec<ViewProp>) {
    let rank = |f: &str| match f {
        "status" => Some(0),
        "tag" => Some(1),
        "checkbox" => Some(2),
        _ => None,
    };
    let mut all: Vec<&RelationDef> = relations
        .iter()
        .filter(|r| !r.hidden && rank(&r.format).is_some())
        .collect();
    all.sort_by(|a, b| {
        rank(&a.format).cmp(&rank(&b.format)).then_with(|| {
            let an = if a.name.is_empty() { &a.key } else { &a.name };
            let bn = if b.name.is_empty() { &b.key } else { &b.name };
            an.to_lowercase().cmp(&bn.to_lowercase())
        })
    });
    let props: Vec<ViewProp> = all.into_iter().map(view_prop).collect();
    props.into_iter().partition(|p| used_keys.contains(&p.key))
}

fn date_choices(
    used_keys: &HashSet<String>,
    relations: &[RelationDef],
) -> (Vec<ViewProp>, Vec<ViewProp>) {
    let mut dates: Vec<ViewProp> = relations
        .iter()
        .filter(|r| {
            !r.hidden && r.format == "date" && !SYSTEM_DATES.iter().any(|(k, _)| *k == r.key)
        })
        .map(view_prop)
        .collect();
    dates.sort_by_key(|p| p.name.to_lowercase());
    let stamps = SYSTEM_DATES.iter().map(|(key, name)| {
        relations
            .iter()
            .find(|r| r.key == *key)
            .map(view_prop)
            .unwrap_or_else(|| ViewProp {
                key: (*key).into(),
                name: (*name).into(),
                emoji: format_glyph("date").into(),
            })
    });
    let (used, rest): (Vec<ViewProp>, Vec<ViewProp>) =
        dates.into_iter().partition(|p| used_keys.contains(&p.key));
    (used.into_iter().chain(stamps).collect(), rest)
}

/// Layout tile click. Kanban and calendar open their property list every time.
fn pick_layout(s: &mut State, id: &str, v: &'static str) {
    let Some(c) = controls_mut(s, id) else { return };
    c.prop_list_open = false;
    let choosing = v == "kanban" || v == "calendar";
    let save = api::note::set_field(id, "viewType", Value::string(v), s);
    if choosing {
        load_used_keys(s, id);
    }
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        let _ = save.await;
        cx.update(|s| {
            let Some(c) = controls_mut(s, &view) else {
                return;
            };
            let used_keys = c.used_keys.clone();
            if v == "kanban"
                && let Some(object) = s.store.object(&view)
            {
                // A board needs a group property: the top of the list stands in, saved.
                let relations = common::scoped_relations(s, &object);
                let (used, rest) = group_choices(&used_keys, &relations);
                let group = config::group_key(&object.fields).to_string();
                let all: Vec<ViewProp> = used.into_iter().chain(rest).collect();
                if let Some(first) = all.first()
                    && !all.iter().any(|p| p.key == group)
                {
                    let task =
                        api::note::set_field(&view, "viewGroupKey", Value::string(&first.key), s);
                    task.detach_and_log_err(&s.rt);
                }
            }
            if let Some(c) = controls_mut(s, &view) {
                c.prop_list_open = choosing;
            }
        });
    })
    .detach();
}

fn pick_prop(s: &mut State, id: &str, key: String) {
    let Some(c) = controls_mut(s, id) else { return };
    c.prop_list_open = false;
    c.chip_open = false;
    c.new_prop = None;
    let Some(object) = s.store.object(id) else {
        return;
    };
    let field = if config::view_type(&object.fields) == "calendar" {
        "viewDateKey"
    } else {
        "viewGroupKey"
    };
    let task = api::note::set_field(id, field, Value::string(key), s);
    task.detach_and_log_err(&s.rt);
}

/// The relation format "＋ New … property" creates for the view's layout.
fn new_prop_format(object: &Object) -> &'static str {
    if config::view_type(&object.fields) == "calendar" {
        "date"
    } else {
        "tag"
    }
}

/// "＋ New … property": name entry, then create the relation and pick it.
fn begin_new_prop(s: &mut State, id: &str) {
    let Some(c) = controls_mut(s, id) else { return };
    let mut input = InputState::new();
    input.focus();
    c.new_prop = Some(input);
}

fn submit_new_prop(s: &mut State, id: &str) {
    let Some(c) = controls_mut(s, id) else { return };
    let name = c
        .new_prop
        .take()
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let Some(object) = s.store.object(id) else {
        return;
    };
    let format = new_prop_format(&object);
    let space = object_space_id(&s.store, &object.fields);
    let task = create_relation(&name, format, &[], &space, s);
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        if let Ok(rel) = task.await {
            cx.update(|s| pick_prop(s, &view, rel.key));
        }
    })
    .detach();
}

// ── New record ───────────────────────────────────────────────

/// Create a record inheriting the view: filters seed fields, calendar
/// seeds the date, the page's space is stamped. `template`: `None` =
/// the type's default, `Some(None)` = Blank, `Some(Some(id))` = that one.
pub(crate) fn create_record(
    s: &mut State,
    id: &str,
    name: String,
    template: Option<Option<String>>,
) -> Task<()> {
    let Some(v) = dv(s, id) else {
        return Task::ready(());
    };
    if v.controls.creating {
        return Task::ready(());
    }
    let kind = v.kind;
    let Some(object) = s.store.object(id) else {
        return Task::ready(());
    };
    let relations = common::scoped_relations(s, &object);
    let channel = field_str(&object.fields, "channel").to_string();
    let sources = config::sources(&object.fields);
    let type_key = if kind == DataViewKind::Type {
        Some(field_str(&object.fields, "key"))
            .filter(|k| !k.is_empty())
            .unwrap_or("note")
            .to_string()
    } else {
        sources.first().cloned().unwrap_or_else(|| "note".into())
    };
    let view_type = config::view_type(&object.fields).to_string();
    let date_key = config::date_key(&object.fields).to_string();
    let seeds = roostr_core::view_seeds::seeds_from_view(&object.fields, &relations);
    let default_tpl = if kind == DataViewKind::Type {
        field_str(&object.fields, "default_template_id").to_string()
    } else {
        s.store
            .types
            .iter()
            .find(|t| t.key == type_key)
            .map(|t| t.default_template_id.clone())
            .unwrap_or_default()
    };
    if let Some(c) = controls_mut(s, id) {
        c.creating = true;
    }
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        let result: anyhow::Result<Option<String>> = async {
            // An agent named in the entry row is a row like any other.
            if type_key == "agent" && !name.trim().is_empty() {
                cx.update(|s| roostr_core::create::new_agent(&channel, &name, seeds.clone(), s))
                    .await?;
                return Ok(None);
            }
            // Infra types are never bare records.
            if matches!(type_key.as_str(), "machine" | "agent" | "file") {
                cx.update(|s| roostr_core::create::create_typed(&type_key, &channel, &name, s))
                    .await?;
                return Ok(None);
            }
            let mut fields = seeds;
            if !channel.is_empty() {
                fields.insert("channel".into(), Value::string(&channel));
            }
            if view_type == "calendar" && !date_key.is_empty() {
                fields.insert(date_key.clone(), Value::int(config::now_ms()));
            }
            let id = cx
                .update(|s| api::note::create(&name, Some(&type_key), Some(fields), s))
                .await?;
            let tpl = match template {
                Some(explicit) => explicit.unwrap_or_default(),
                None => default_tpl,
            };
            if !tpl.is_empty() {
                let _ = cx
                    .update(|s| roostr_core::create::apply_template(&id, &tpl, s))
                    .await;
            }
            Ok(Some(id))
        }
        .await;
        cx.update(|s| {
            let Some(c) = controls_mut(s, &view) else {
                return;
            };
            c.creating = false;
            match result {
                // Anytype's table New edits the record in place; other views open it.
                Ok(Some(id)) if view_type == "table" => crate::on_record_created(s, &view, id),
                Ok(Some(id)) => common::open_object(&id, s),
                Ok(None) => {}
                Err(error) => log::error!("create record: {error:#}"),
            }
        });
    })
}

fn new_record(s: &mut State, id: &str) {
    let table = s
        .store
        .object(id)
        .is_some_and(|o| config::view_type(&o.fields) == "table");
    if table {
        crate::table::begin_new(s, id);
    } else {
        create_record(s, id, String::new(), None).detach();
    }
}

fn toggle_search(s: &mut State, id: &str) {
    let Some(c) = controls_mut(s, id) else { return };
    c.search_open = !c.search_open;
    if c.search_open {
        c.search.focus();
    } else {
        c.search.clear();
        crate::set_search(s, id, String::new());
    }
}

fn toggle_panel(s: &mut State, id: &str, panel: Panel) {
    let len = s
        .store
        .object(id)
        .map(|o| match panel {
            Panel::Filter => config::parse_filters(&o.fields).len(),
            Panel::Sort => config::parse_sorts(&o.fields).len(),
            Panel::None => 0,
        })
        .unwrap_or_default();
    let Some(c) = controls_mut(s, id) else { return };
    c.open = if c.open == panel { Panel::None } else { panel };
    c.rules_base = len;
}

fn sort_reorder(s: &mut State, id: &str) {
    let Some(c) = controls_mut(s, id) else { return };
    let (Some(from), Some(to)) = (c.sort_drag.take(), c.sort_over.take()) else {
        c.sort_drag = None;
        c.sort_over = None;
        return;
    };
    if from == to {
        return;
    }
    let Some(object) = s.store.object(id) else {
        return;
    };
    let mut next = config::parse_sorts(&object.fields);
    if from >= next.len() || to >= next.len() {
        return;
    }
    let moved = next.remove(from);
    next.insert(to, moved);
    settle_rules(s, id, next.len());
    save_sorts(s, id, next);
}

/// A sort drag: the rule dragged dims, the rule under the pointer shows the
/// drop line, the release moves it there.
fn sort_drag(s: &mut State, id: &str, i: usize, d: &mut Drag) {
    match d.phase {
        Phase::Down => {}
        Phase::Move => {
            let [_, y] = d.window_point();
            let Some(c) = controls_mut(s, id) else { return };
            let over = c
                .sort_rects
                .iter()
                .find(|(_, r)| y >= r[1] && y <= r[1] + r[3])
                .map(|(j, _)| *j);
            if let Some(over) = over
                && (c.sort_over != Some(over) || c.sort_drag.is_none())
            {
                c.sort_drag = Some(i);
                c.sort_over = Some(over);
            }
        }
        Phase::Up => {
            if d.cancelled() {
                if let Some(c) = controls_mut(s, id) {
                    c.sort_drag = None;
                    c.sort_over = None;
                }
            } else {
                sort_reorder(s, id);
            }
        }
    }
}

// ── Rendering ────────────────────────────────────────────────

/// A rule just added enters: opacity 0 → 1 while it drops 4px over 150ms;
/// a removal is immediate. Every rule slides to its new place when the
/// rules reorder or a sibling comes or goes (Rune's layout motion).
fn rule_enter() -> Transition {
    Transition {
        insertion: Effect {
            offset: Vec2::new(0.0, -4.0),
            ..Effect::opacity(0.0)
        },
        removal: Effect::IDENTITY,
        springs: SpringPair::new(CONTROL.spring(), Spring::NONE),
        ..Transition::identity()
    }
}

/// Rule `rule_id`'s row, keyed by its identity so it follows the rule.
fn rule_row(id: &str, rule_id: &str, entering: bool, row: El) -> El {
    let e = el().key(format!("{id}/{rule_id}")).child(row);
    if entering {
        e.transition(rule_enter())
    } else {
        e
    }
}

/// A controls-row toggle (`.cbtn`): the kit toggle, on = the accent glyph.
fn cbtn(t: &Theme, key: String, g: Glyph, on: bool) -> Toggle {
    let toggle = Toggle::new(key)
        .checked(on)
        .icon(g)
        .size(KitSize::Px(20.0))
        .w(28.0)
        .h(28.0)
        .min_w(28.0)
        .p(0.0)
        .radius(6.0)
        .pointer()
        .text_color(if on { t.accent } else { t.muted });
    // On reads as the accent glyph alone, not the kit's pressed fill.
    if on {
        toggle.bg(Color::TRANSPARENT)
    } else {
        toggle
    }
}

/// gpui-kit's `dropdown_caret`: a medium chevron in the label colour at 75%.
fn caret(ink: Color) -> El {
    glyph(Glyph::KitChevronDown, KitSize::Medium.icon_px())
        .color(ink.alpha(ink.a * 0.75))
        .a11y_hidden()
}

/// A property's emoji column (min 18 wide, 6 before the name).
fn emoji_col(s: &State, emoji: &str, size: f32) -> El {
    row()
        .min_w(18.0)
        .mr(6.0)
        .center()
        .child(common::icon(s, emoji, ty::ui(size)))
}

fn prop_caption(
    s: &State,
    t: &Theme,
    current: Option<&ViewProp>,
    current_key: &str,
    size: f32,
    ink: Color,
) -> El {
    match current {
        Some(p) => row()
            .center_y()
            .min_w(0.0)
            .child(emoji_col(s, &p.emoji, size))
            .child(
                text(p.name.clone())
                    .font(ty::ui(size))
                    .color(ink)
                    .truncate()
                    .min_w(0.0),
            ),
        None => text(if current_key.is_empty() {
            "Select".to_string()
        } else {
            current_key.to_string()
        })
        .font(ty::ui(size))
        .color(if ink == t.fg { t.fg } else { ink }),
    }
}

/// The board/calendar property list: used properties, a divider, the rest, "＋ New … property".
#[allow(clippy::too_many_arguments)]
fn prop_list(
    s: &State,
    t: &Theme,
    v: &DataView,
    title: &str,
    calendar: bool,
    used: &[ViewProp],
    rest: &[ViewProp],
    current_key: &str,
) -> El {
    let id = &v.id;
    let row_of = |p: &ViewProp| {
        let (view, key) = (id.clone(), p.key.clone());
        vrow(t, &format!("{id}/plist-{}", p.key), false)
            .child(vrow_content(
                row()
                    .center_y()
                    .min_w(0.0)
                    .child(emoji_col(s, &p.emoji, 13.0))
                    .child(common::label(t, p.name.clone())),
                (p.key == current_key).then(|| text("✓").font(ty::ui(13.0)).color(t.accent)),
            ))
            .on_press(move |s: &mut State| pick_prop(s, &view, key.clone()))
    };
    let mut list = col().gap(2.0);
    for p in used {
        list = list.child(row_of(p));
    }
    if !used.is_empty() && !rest.is_empty() {
        list = list.child(separator(t));
    }
    for p in rest {
        list = list.child(row_of(p));
    }
    let scroll = col()
        .key(format!("{id}/plist-scroll"))
        .items_stretch()
        .max_h(320.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(list);
    let format = if calendar { "date" } else { "tag" };
    let new_entry = match &v.controls.new_prop {
        Some(input) => {
            let (lens, submit) = (id.clone(), id.clone());
            el().px(4.0).child(
                Input::new(format!("{id}/plist-new-name"), input)
                    .placeholder(format!("New {format} property name:"))
                    .text_size(13.0)
                    .on_edit(move |s: &mut State| {
                        dv_mut(s, &lens)
                            .expect(OPEN)
                            .controls
                            .new_prop
                            .get_or_insert_with(InputState::new)
                    })
                    .on_submit(move |s: &mut State, _: &mut Tx| submit_new_prop(s, &submit))
                    .build(t),
            )
        }
        None => {
            let view = id.clone();
            vrow(t, &format!("{id}/plist-new"), false)
                .child(common::label(t, format!("＋ New {format} property…")))
                .on_press(move |s: &mut State| begin_new_prop(s, &view))
        }
    };
    menu_surface(t)
        .w(274.0)
        .child(menu_title(t, title))
        .child(scroll)
        .child(separator(t))
        .child(new_entry)
}

/// The rules panel box (`.panel`).
fn panel_box(t: &Theme) -> El {
    col()
        .items_stretch()
        .gap(8.0)
        .border(1.0, t.border)
        .border_layout()
        .radius(10.0)
        .padding(10.0)
        .mb(12.0)
}

/// A muted text action of a rules panel ("+ Add filter", "Delete sort"): a
/// kit ghost button at its own width.
fn add_row(t: &Theme, key: String, label: &str, f: impl Fn(&mut State) + 'static) -> El {
    Button::new(key)
        .ghost()
        .xsmall()
        // The kit sizes a label itself (12px at xsmall): the text sits in a child.
        .child(text(label.to_string()).font(ty::ui(13.0)).color(t.muted))
        .accessibility_label(label.to_string())
        .text_color(t.muted)
        .pointer()
        .on_press(f)
        .build(t)
        .self_start()
}

/// A muted kit ghost "×" removing a value, a rule or a sort.
fn x_button(t: &Theme, key: String, label: &str) -> Button {
    Button::new(key)
        .ghost()
        .xsmall()
        .icon(Glyph::Close)
        .accessibility_label(label.to_string())
        .text_color(t.muted)
        .pointer()
}

/// A kit menu row with `.vrow` content: the label, and an accent ✓ when `checked`.
fn kit_row(t: &Theme, label: impl Into<String>, checked: bool) -> PopupMenuItem {
    PopupMenuItem::element(
        row()
            .grow(1.0)
            .center_y()
            .justify_between()
            .gap(12.0)
            .py(6.0)
            .child(text(label.into()).font(ty::ui(13.0)).color(t.fg))
            .children(checked.then(|| text("✓").font(ty::ui(13.0)).color(t.accent))),
    )
}

/// A kit menu's title row (`.ptitle`): [`menu_title`] as an inert item.
fn kit_title(t: &Theme, title: &str) -> PopupMenuItem {
    PopupMenuItem::element(menu_title(t, title).px(0.0)).disabled(true)
}

fn filter_panel(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    v: &DataView,
    object: &Object,
    relations: &Rc<Vec<RelationDef>>,
) -> El {
    let id = &v.id;
    let c = &v.controls;
    let filters = config::parse_filters(&object.fields);
    let keys = config::filter_keys(relations);
    let ids = rule_ids("filter", filters.iter().map(|f| f.key.as_str()));
    let mut panel = panel_box(t);
    for ((i, f), rule_id) in filters.iter().enumerate().zip(&ids) {
        let rels = relations.clone();
        let label: KeyLabel = Rc::new(move |k, _| config::option_label(&rels, k));
        let view = id.clone();
        let mut rule = row().flex_wrap().center_y().gap(8.0).child(
            KeyPicker::new(format!("{id}/fkey-{i}"), f.key.clone(), keys.clone(), label)
                .on_pick(move |s: &mut State, key: &str| {
                    update_filter(s, &view, i, Some(key.to_string()), None, None)
                })
                .build(s, cx, t, layer),
        );
        rule = rule.child(condition_select(t, layer, v, i, f, relations));
        if config::needs_value(relations, f) && config::filter_format(relations, &f.key) == "object"
        {
            rule = rule.child(object_values(s, t, v, i, f, relations, object));
        } else if config::needs_value(relations, f) {
            let options = config::options_for(relations, &f.key);
            if !options.is_empty() {
                let mut tags = row().flex_wrap().gap(6.0);
                for opt in options {
                    let on = f.value.contains(&opt);
                    let color = config::option_color(relations, &f.key, &opt);
                    let current = f.value.clone();
                    let (view, o) = (id.clone(), opt.clone());
                    let mut tag = motion::pressable(row())
                        .key(format!("{id}/ftag-{i}-{opt}"))
                        .role("button")
                        .checked(on)
                        .label(opt.clone())
                        .radius(999.0)
                        .border(1.0, if on { t.accent } else { Color::TRANSPARENT })
                        .border_layout()
                        .opacity(if on { 1.0 } else { 0.45 })
                        .cursor(Cursor::Pointer)
                        .child(
                            tag_pill(t, &opt, &color, false, t.fg)
                                .px(10.0)
                                .radius(999.0),
                        )
                        .on_press(move |s: &mut State| {
                            let next = if current.contains(&o) {
                                current.iter().filter(|v| **v != o).cloned().collect()
                            } else {
                                let mut n = current.clone();
                                n.push(o.clone());
                                n
                            };
                            update_filter(s, &view, i, None, None, Some(next))
                        });
                    if !on {
                        tag = tag.hover(t.kit.accent);
                    }
                    tags = tags.child(tag);
                }
                rule = rule.child(tags);
            } else if config::multi_value(&f.condition) {
                // Several values: every one is shown and removable.
                let mut multi = row().flex_wrap().center_y().gap(6.0);
                for (j, value) in f.value.iter().enumerate() {
                    let current = f.value.clone();
                    let view = id.clone();
                    multi = multi.child(
                        row()
                            .center_y()
                            .gap(4.0)
                            .pl(10.0)
                            .pr(4.0)
                            .py(2.0)
                            .radius(999.0)
                            .fill(t.border)
                            .border(1.0, t.border)
                            .border_layout()
                            .child(text(value.clone()).font(ty::ui(12.0)).color(t.fg))
                            .child(
                                x_button(t, format!("{id}/fval-x-{i}-{j}"), "Remove value")
                                    .on_press(move |s: &mut State| {
                                        let next = current
                                            .iter()
                                            .enumerate()
                                            .filter(|(k, _)| *k != j)
                                            .map(|(_, v)| v.clone())
                                            .collect();
                                        update_filter(s, &view, i, None, None, Some(next))
                                    })
                                    .build(t),
                            ),
                    );
                }
                let placeholder = if f.value.is_empty() {
                    "value"
                } else {
                    "add another"
                };
                let fresh = InputState::new();
                let state = c.rule_inputs.get(&i).map_or(&fresh, |r| &r.multi);
                let (lens, change, submit) = (id.clone(), id.clone(), id.clone());
                multi = multi.child(
                    el().w(140.0).child(
                        Input::new(format!("{id}/fval-multi-{i}"), state)
                            .placeholder(placeholder)
                            .text_size(13.0)
                            .on_edit(move |s: &mut State| &mut rule_inputs(s, &lens, i).multi)
                            .on_change(move |s: &mut State, _: &mut Tx| {
                                let inputs = rule_inputs(s, &change, i);
                                inputs.multi_dirty = true;
                                if inputs.multi.value().ends_with(',') {
                                    commit_multi(s, &change, i);
                                }
                            })
                            .on_submit(move |s: &mut State, _: &mut Tx| commit_multi(s, &submit, i))
                            .build(t),
                    ),
                );
                rule = rule.child(multi);
            } else {
                let fresh = InputState::with_value(f.value.first().cloned().unwrap_or_default());
                let state = c.rule_inputs.get(&i).map_or(&fresh, |r| &r.single);
                let (lens, change, submit) = (id.clone(), id.clone(), id.clone());
                rule = rule.child(
                    el().w(160.0).child(
                        Input::new(format!("{id}/fval-{i}"), state)
                            .placeholder("value")
                            .text_size(13.0)
                            .on_edit(move |s: &mut State| &mut rule_inputs(s, &lens, i).single)
                            .on_change(move |s: &mut State, _: &mut Tx| {
                                rule_inputs(s, &change, i).single_dirty = true;
                            })
                            .on_submit(move |s: &mut State, _: &mut Tx| {
                                commit_single(s, &submit, i)
                            })
                            .build(t),
                    ),
                );
            }
        }
        let all = filters.clone();
        let view = id.clone();
        rule = rule.child(
            x_button(t, format!("{id}/fx-{i}"), "Remove filter")
                .on_press(move |s: &mut State| {
                    let next: Vec<FilterRule> = all
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != i)
                        .map(|(_, f)| f.clone())
                        .collect();
                    if let Some(c) = controls_mut(s, &view) {
                        c.rule_inputs.clear();
                        c.object_search.clear();
                    }
                    settle_rules(s, &view, next.len());
                    save_filters(s, &view, next);
                })
                .build(t),
        );
        panel = panel.child(rule_row(id, rule_id, i >= c.rules_base, rule));
    }
    let all = filters.clone();
    let view = id.clone();
    panel.child(add_row(
        t,
        format!("{id}/add-filter"),
        "+ Add filter",
        move |s: &mut State| {
            let mut next = all.clone();
            next.push(FilterRule {
                key: "name".into(),
                condition: "like".into(),
                value: vec![],
            });
            save_filters(s, &view, next);
        },
    ))
}

/// An object filter's values, as Anytype's (OptionSelect in the filter's
/// values menu): a "Filter Objects..." field over a list of the objects
/// the property may hold (its type limit, this space) - "Selected" first,
/// then "All values" - each row the object's icon and name; a click adds
/// or removes it. The engine matches links by id.
fn object_values(
    s: &State,
    t: &Theme,
    v: &DataView,
    i: usize,
    f: &FilterRule,
    relations: &[RelationDef],
    view_object: &Object,
) -> El {
    /// Rows in "All values" (Anytype's LIMIT).
    const LIMIT: usize = 40;
    let id = &v.id;
    let fresh = InputState::new();
    let search = v.controls.object_search.get(&i).unwrap_or(&fresh);
    let focused = search.is_focused();
    let q = search.value().trim().to_lowercase();
    let store = &s.store;
    let space = object_space_id(store, &view_object.fields);
    // The property's type limit, as type keys (its object_types are type ids).
    let allowed: Vec<String> = relations
        .iter()
        .find(|r| r.key == f.key)
        .and_then(|r| r.object_types.clone())
        .unwrap_or_default()
        .iter()
        .filter_map(|id| {
            store
                .types
                .iter()
                .find(|t| &t.id == id)
                .map(|t| t.key.clone())
        })
        .collect();
    // (id, name, the icon to show: its own, else its type's).
    let icon_of = |icon: &str, type_key: &str| {
        roostr_core::objects::object_icon(store, icon, type_key, &space)
    };
    let row_of = |id: &str| -> (String, String, String) {
        match store.summary(id) {
            Some(s) => (
                id.to_string(),
                s.name.clone(),
                icon_of(&s.icon, &s.type_key),
            ),
            None => store
                .agent(id)
                .map(|a| (id.to_string(), a.name.clone(), icon_of(&a.icon, "agent")))
                .unwrap_or_else(|| (id.to_string(), id.to_string(), String::new())),
        }
    };
    let selected: Vec<(String, String, String)> = f.value.iter().map(|id| row_of(id)).collect();
    let mut rest: Vec<(String, String, String)> = if allowed.iter().any(|k| k == "agent") {
        store
            .agents
            .iter()
            .filter(|a| a.channel.is_empty() || a.channel == space)
            .map(|a| (a.id.clone(), a.name.clone(), icon_of(&a.icon, "agent")))
            .collect()
    } else {
        store
            .summaries
            .iter()
            .filter(|s| s.channel_id == space || s.channel_id.is_empty())
            .filter(|s| allowed.is_empty() || allowed.contains(&s.type_key))
            .map(|s| (s.id.clone(), s.name.clone(), icon_of(&s.icon, &s.type_key)))
            .collect()
    };
    rest.retain(|(id, name, _)| {
        !f.value.contains(id)
            && !name.is_empty()
            && (q.is_empty() || name.to_lowercase().contains(&q))
    });
    rest.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    rest.truncate(LIMIT);
    let selected: Vec<_> = selected
        .into_iter()
        .filter(|(_, name, _)| q.is_empty() || name.to_lowercase().contains(&q))
        .collect();

    // `.sectionName`: 5px 16px, small, secondary, medium weight.
    let section = |name: &str| {
        el().px(16.0).py(5.0).child(
            text(name.to_string())
                .font(ty::ui(12.0).weight(ty::MEDIUM))
                .color(t.muted),
        )
    };
    let mut list = col().items_stretch();
    let empty = selected.is_empty() && rest.is_empty();
    for (on, rows) in [(true, selected), (false, rest)] {
        if rows.is_empty() {
            continue;
        }
        if !f.value.is_empty() {
            list = list.child(section(if on { "Selected" } else { "All values" }));
        }
        for (target, name, icon) in rows {
            let current = f.value.clone();
            let view = id.clone();
            let selector = format!("fobj-{}-{target}", if on { "on" } else { "add" });
            let mut inner = row()
                .grow(1.0)
                .h_full()
                .center_y()
                .gap(4.0)
                .pl(10.0)
                .pr(8.0)
                .radius(6.0)
                .hover(t.hover)
                .child(
                    row()
                        .w(20.0)
                        .center()
                        .child(common::icon(s, &icon, ty::ui(14.0))),
                )
                .child(
                    text(name.clone())
                        .font(ty::ui(14.0))
                        .color(t.fg)
                        .truncate()
                        .grow(1.0)
                        .min_w(0.0),
                );
            if on {
                inner = inner.child(glyph(Glyph::Check, 14.0).color(t.fg));
            }
            list = list.child(
                // `.item`: 32px, 6px 16px (18px left), the hover a rounded
                // fill inset 8px from the menu's edges.
                motion::pressable(row())
                    .key(selector)
                    .role("button")
                    .label(name)
                    .px(8.0)
                    .h(32.0)
                    .center_y()
                    .cursor(Cursor::Pointer)
                    .on_press(move |s: &mut State| {
                        let next = if current.contains(&target) {
                            current.iter().filter(|v| **v != target).cloned().collect()
                        } else {
                            let mut next = current.clone();
                            next.push(target.clone());
                            next
                        };
                        if let Some(search) =
                            controls_mut(s, &view).and_then(|c| c.object_search.get_mut(&i))
                        {
                            search.clear();
                        }
                        update_filter(s, &view, i, None, None, Some(next))
                    })
                    .child(inner),
            );
        }
    }
    if empty {
        list = list.child(
            el().px(16.0).py(6.0).child(
                text("Type to search options")
                    .font(ty::ui(13.0))
                    .color(t.muted),
            ),
        );
    }
    let lens = id.clone();
    col()
        .items_stretch()
        .w(280.0)
        .gap(4.0)
        .py(4.0)
        .fill(t.panel)
        .border(1.0, t.border)
        .border_layout()
        .radius(roostr_ui::radius::MENU)
        .child(
            // Anytype's filter field: filled, borderless, radius 10, 8px 12px.
            row()
                .mx(8.0)
                .mt(4.0)
                .center_y()
                .gap(6.0)
                .px(12.0)
                .py(8.0)
                .radius(roostr_ui::radius::MENU)
                .fill(if focused { t.hl_med } else { t.hl_light })
                .child(glyph(Glyph::KitSearch, 14.0).color(t.muted))
                .child(
                    el().grow(1.0).min_w(0.0).child(
                        Input::new(format!("{id}/fobj-search-{i}"), search)
                            .placeholder("Filter Objects...")
                            .appearance(false)
                            .text_size(13.0)
                            .px(0.0)
                            .py(0.0)
                            .h(17.0)
                            .on_edit(move |s: &mut State| {
                                dv_mut(s, &lens)
                                    .expect(OPEN)
                                    .controls
                                    .object_search
                                    .entry(i)
                                    .or_default()
                            })
                            .build(t),
                    ),
                ),
        )
        .child(
            col()
                .key(format!("{id}/fobj-list-{i}"))
                .items_stretch()
                .max_h(280.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list),
        )
}

/// The condition `<select>`: the current label, a dropdown of the key's catalog.
fn condition_select(
    t: &Theme,
    layer: &mut Vec<El>,
    v: &DataView,
    i: usize,
    f: &FilterRule,
    relations: &[RelationDef],
) -> El {
    let id = &v.id;
    let conds = config::conditions_for(relations, &f.key);
    let label = conds
        .iter()
        .find(|c| c.id == f.condition)
        .map_or(f.condition.as_str(), |c| c.label)
        .to_string();
    let open = v.controls.cond_open == Some(i);
    let toggle = id.clone();
    let button = Button::new(format!("{id}/fcond-{i}"))
        .outline()
        .selected(open)
        .h_auto()
        .px(8.0)
        .py(4.0)
        .radius(6.0)
        .bg(t.panel)
        .border_color(if open { t.accent } else { t.border })
        .text_color(t.fg)
        .pointer()
        .accessibility_label(label.clone())
        .child(text(label).font(ty::ui(13.0)).color(t.fg))
        .child(caret(t.fg))
        .on_press(move |s: &mut State| {
            if let Some(c) = controls_mut(s, &toggle) {
                c.cond_open = if c.cond_open == Some(i) {
                    None
                } else {
                    Some(i)
                };
            }
        })
        .build(t);
    let mut menu = PopupMenu::new().min_w(160.0);
    for c in conds {
        let (view, cond) = (id.clone(), c.id.to_string());
        menu = menu.item(kit_row(t, c.label, c.id == f.condition).on_press(
            move |s: &mut State| {
                if let Some(c) = controls_mut(s, &view) {
                    c.cond_open = None;
                }
                update_filter(s, &view, i, None, Some(cond.clone()), None)
            },
        ));
    }
    let close = id.clone();
    DropdownMenu::new(format!("{id}/fcond-menu-{i}"))
        .trigger(button)
        .menu(menu)
        .open(open)
        .placement(Placement::BottomStart)
        .gap(4.0)
        .on_close(move |s: &mut State| {
            if let Some(c) = controls_mut(s, &close)
                && c.cond_open == Some(i)
            {
                c.cond_open = None;
            }
        })
        .build(t, layer)
}

fn sort_panel(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    v: &DataView,
    object: &Object,
    relations: &Rc<Vec<RelationDef>>,
) -> El {
    let id = &v.id;
    let c = &v.controls;
    let sorts = config::parse_sorts(&object.fields);
    let keys = config::filter_keys(relations);
    let ids = rule_ids("sort", sorts.iter().map(|s| s.key.as_str()));
    let mut panel = panel_box(t).gap(2.0);
    if sorts.is_empty() {
        panel = panel.child(
            common::empty_state(t, "No sorts applied", 12.0)
                .px(4.0)
                .py(2.0),
        );
    }
    /// `sorts` with rule `i` changed by `f`.
    fn with(sorts: &[SortRule], i: usize, f: impl Fn(&SortRule) -> SortRule) -> Vec<SortRule> {
        sorts
            .iter()
            .enumerate()
            .map(|(j, x)| if j == i { f(x) } else { x.clone() })
            .collect()
    }
    for ((i, rule), rule_id) in sorts.iter().enumerate().zip(&ids) {
        let rels = relations.clone();
        let label: KeyLabel = Rc::new(move |k, _| config::option_label(&rels, k));
        let dragging = c.sort_drag == Some(i);
        let over = c.sort_over == Some(i) && c.sort_drag.is_some_and(|d| d != i);
        let more_open = c.sort_more == Some(i);

        let toggle = id.clone();
        let more_button = Button::new(format!("{id}/sort-more-{i}"))
            .ghost()
            .xsmall()
            .icon(Glyph::Ellipsis)
            .accessibility_label("More")
            .selected(more_open)
            .text_color(t.muted)
            .pointer()
            .on_press(move |s: &mut State| {
                if let Some(c) = controls_mut(s, &toggle) {
                    c.sort_more = if c.sort_more == Some(i) {
                        None
                    } else {
                        Some(i)
                    };
                }
            })
            .build(t);
        let pick = |empty_start: bool| {
            let (view, sorts) = (id.clone(), sorts.clone());
            move |s: &mut State| {
                let next = with(&sorts, i, |x| SortRule {
                    empty_start,
                    ..x.clone()
                });
                if let Some(c) = controls_mut(s, &view) {
                    c.sort_more = None;
                }
                save_sorts(s, &view, next);
            }
        };
        let close = id.clone();
        let more = DropdownMenu::new(format!("{id}/sort-more-menu-{i}"))
            .trigger(more_button)
            .menu(
                PopupMenu::new()
                    .min_w(174.0)
                    .item(kit_title(t, "Show empty"))
                    .item(kit_row(t, "At the top", rule.empty_start).on_press(pick(true)))
                    .item(kit_row(t, "At the bottom", !rule.empty_start).on_press(pick(false))),
            )
            .open(more_open)
            .placement(Placement::BottomEnd)
            .gap(4.0)
            .on_close(move |s: &mut State| {
                if let Some(c) = controls_mut(s, &close)
                    && c.sort_more == Some(i)
                {
                    c.sort_more = None;
                }
            })
            .build(t, layer);

        let (pick_view, all) = (id.clone(), sorts.clone());
        let key_picker = el()
            .border(1.0, t.border)
            .border_layout()
            .radius(8.0)
            .px(8.0)
            .py(3.0)
            .child(
                KeyPicker::new(
                    format!("{id}/skey-{i}"),
                    rule.key.clone(),
                    keys.clone(),
                    label,
                )
                .on_pick(move |s: &mut State, key: &str| {
                    let next = with(&all, i, |x| SortRule {
                        key: key.to_string(),
                        ..x.clone()
                    });
                    settle_rules(s, &pick_view, next.len());
                    save_sorts(s, &pick_view, next);
                })
                .build(s, cx, t, layer),
            );
        let (dir_view, all) = (id.clone(), sorts.clone());
        let direction = Button::new(format!("{id}/sort-dir-{i}"))
            .outline()
            .h_auto()
            .px(8.0)
            .py(3.0)
            .radius(8.0)
            .border_color(t.border)
            .bg(Color::TRANSPARENT)
            .text_color(t.fg)
            .pointer()
            .gap(4.0)
            .accessibility_label(if rule.desc { "Desc" } else { "Asc" })
            .child(
                // `.sort-arrow`: one ↑ turned 180° for Desc (`transition: transform 0.15s`).
                glyph(Glyph::SortArrow, 16.0)
                    .color(t.fg)
                    .rotate(if rule.desc { PI } else { 0.0 })
                    .rotate_motion(CONTROL.anim()),
            )
            .child(
                text(if rule.desc { "Desc" } else { "Asc" })
                    .font(ty::ui(13.0))
                    .color(t.fg),
            )
            .on_press(move |s: &mut State| {
                let next = with(&all, i, |x| SortRule {
                    desc: !x.desc,
                    ..x.clone()
                });
                save_sorts(s, &dir_view, next);
            })
            .build(t);
        let (del_view, all) = (id.clone(), sorts.clone());
        let delete = x_button(t, format!("{id}/sort-x-{i}"), "Delete sort")
            .on_press(move |s: &mut State| {
                let next: Vec<SortRule> = all
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, x)| x.clone())
                    .collect();
                settle_rules(s, &del_view, next.len());
                save_sorts(s, &del_view, next);
            })
            .build(t);

        let (rect_view, drag_view) = (id.clone(), id.clone());
        // `.dnd` and `.row-btns` reveal on the row's hover (`opacity 0.1s`).
        let mut item = row()
            .key(format!("{id}/sort-item-{i}"))
            .hover_group()
            .hover_motion(HOVER_REVEAL.anim(), HOVER_REVEAL.anim())
            .center_y()
            .gap(8.0)
            .px(4.0)
            .py(3.0)
            .radius(8.0)
            .on_layout_tx(move |s: &mut State, r: [f32; 4], tx: &mut Tx| {
                if let Some(c) = controls_mut(s, &rect_view) {
                    c.sort_rects.insert(i, r);
                }
                tx.live_only();
            })
            .on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| sort_drag(s, &drag_view, i, d))
            .child(
                text("⋮⋮")
                    .font(ty::ui(10.0))
                    .color(t.muted)
                    .cursor(Cursor::Grab)
                    .group_opacity(0.0, 0.7),
            )
            .child(key_picker)
            .child(direction)
            .child(el().grow(1.0))
            .child(
                row()
                    .center_y()
                    .gap(2.0)
                    .group_opacity(if more_open { 1.0 } else { 0.0 }, 1.0)
                    .child(more)
                    .child(delete),
            );
        if dragging {
            item = item.opacity(0.4);
        }
        if over {
            item = item.border_sides([2.0, 0.0, 0.0, 0.0], t.accent);
        }
        panel = panel.child(rule_row(id, rule_id, i >= c.rules_base, item));
    }
    let (add_view, all) = (id.clone(), sorts.clone());
    panel = panel.child(separator(t)).child(add_row(
        t,
        format!("{id}/add-sort"),
        "＋ Add sort",
        move |s: &mut State| {
            let mut next = all.clone();
            next.push(SortRule {
                key: "name".into(),
                desc: false,
                empty_start: false,
            });
            save_sorts(s, &add_view, next);
        },
    ));
    if !sorts.is_empty() {
        let view = id.clone();
        panel = panel.child(add_row(
            t,
            format!("{id}/clear-sort"),
            "Delete sort",
            move |s: &mut State| {
                settle_rules(s, &view, 0);
                save_sorts(s, &view, vec![]);
            },
        ));
    }
    panel
}

/// Each rule's identity under `prefix`: its property key, numbered among the
/// rules sharing it. Motion follows the rule when the rules reorder (Svelte
/// keyed them by index).
fn rule_ids<'a>(prefix: &str, keys: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    keys.into_iter()
        .map(|key| {
            let n = seen.entry(key).or_default();
            let id = format!("{prefix}-{key}#{n}");
            *n += 1;
            id
        })
        .collect()
}

fn view_prop(r: &RelationDef) -> ViewProp {
    ViewProp {
        key: r.key.clone(),
        name: if r.name.is_empty() {
            r.key.clone()
        } else {
            r.name.clone()
        },
        emoji: r
            .icon_emoji
            .clone()
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| format_glyph(&r.format).to_string()),
    }
}

/// The controls row and its open panel. Menus go into `layer`.
pub(crate) fn view(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    v: &DataView,
) -> El {
    let Some(object) = s.store.object(&v.id) else {
        return el();
    };
    let id = &v.id;
    let c = &v.controls;
    let relations = Rc::new(common::scoped_relations(s, &object));
    let view_type = config::view_type(&object.fields).to_string();
    let calendar = view_type == "calendar";
    let chooses_prop = view_type == "kanban" || calendar;
    let prop_title: &'static str = if calendar { "Date" } else { "Group by" };
    let (used, rest) = if calendar {
        date_choices(&c.used_keys, &relations)
    } else {
        group_choices(&c.used_keys, &relations)
    };
    let current_key = if calendar {
        config::date_key(&object.fields)
    } else {
        config::group_key(&object.fields)
    }
    .to_string();
    let current = used
        .iter()
        .chain(rest.iter())
        .find(|p| p.key == current_key)
        .cloned();
    let filters_on = !config::parse_filters(&object.fields).is_empty();
    let sorts_on = !config::parse_sorts(&object.fields).is_empty();
    let view_label = {
        let mut ch = view_type.chars();
        ch.next()
            .map(|f| f.to_uppercase().collect::<String>() + ch.as_str())
            .unwrap_or_default()
    };

    let mut bar = row()
        .w_full()
        .center_y()
        .gap(8.0)
        .mb(10.0)
        .child(el().grow(1.0));
    bar = bar.child(
        row()
            .center_y()
            .gap(5.0)
            .child(layout_icon(&view_type, 16.0, t.muted))
            .child(text(view_label).font(ty::ui(12.0)).color(t.muted)),
    );
    if chooses_prop {
        let title = if calendar {
            "Choose the property this calendar places records by"
        } else {
            "Choose the property this board groups by"
        };
        // `.pchip-title`: hidden at ≤720px.
        let mobile = cx
            .layout_with("kit-root", |r| roostr_ui::layout::is_mobile(r[2]))
            .unwrap_or(false);
        let mut caption = row().grow(1.0).min_w(0.0).center_y();
        if !mobile {
            caption = caption.child(
                text(format!("{prop_title}:"))
                    .font(ty::ui(12.0))
                    .color(t.muted)
                    .mr(6.0),
            );
        }
        caption = caption.child(prop_caption(
            s,
            t,
            current.as_ref(),
            &current_key,
            12.0,
            t.fg,
        ));
        let toggle = id.clone();
        let chip = Button::new(format!("{id}/prop-chip"))
            .outline()
            .selected(c.chip_open)
            .h(28.0)
            .px(8.0)
            .radius(8.0)
            .border_color(t.border)
            .bg(if c.chip_open {
                t.hl_med
            } else {
                Color::TRANSPARENT
            })
            .text_color(t.fg)
            .pointer()
            .tooltip(title)
            .accessibility_label(title)
            .child(caption)
            .child(caret(t.fg))
            .on_press(move |s: &mut State| {
                let Some(c) = controls_mut(s, &toggle) else {
                    return;
                };
                c.chip_open = !c.chip_open;
                c.settings_open = false;
                if c.chip_open {
                    load_used_keys(s, &toggle);
                }
            })
            .build(t)
            .shrink()
            .min_w(0.0);
        if c.chip_open {
            let list = prop_list(s, t, v, prop_title, calendar, &used, &rest, &current_key);
            let close = id.clone();
            let list = overlay::dismissable(
                list,
                &format!("{id}/prop-chip-list"),
                move |s: &mut State| {
                    if let Some(c) = controls_mut(s, &close) {
                        c.chip_open = false;
                        c.new_prop = None;
                    }
                },
                layer,
            );
            layer.push(overlay::anchored(
                list,
                &format!("{id}/prop-chip"),
                Placement::BottomStart,
                6.0,
            ));
        }
        bar = bar.child(row().min_w(0.0).max_w(260.0).child(chip));
    }
    if c.search_open {
        let (lens, change, escape) = (id.clone(), id.clone(), id.clone());
        bar = bar.child(
            el().w(202.0).child(
                Input::new(format!("{id}/dv-search-field"), &c.search)
                    .placeholder("Search")
                    .text_size(13.0)
                    .on_edit(move |s: &mut State| {
                        &mut dv_mut(s, &lens).expect(OPEN).controls.search
                    })
                    .on_change(move |s: &mut State, _: &mut Tx| {
                        let Some(c) = controls_mut(s, &change) else {
                            return;
                        };
                        let q = c.search.value().to_string();
                        crate::set_search(s, &change, q);
                    })
                    // The field takes Escape: it closes the search.
                    .on_key(move |s: &mut State, ev: &KeyEvent, _: &mut Tx| {
                        if ev.key == Key::Escape {
                            toggle_search(s, &escape);
                            true
                        } else {
                            false
                        }
                    })
                    .build(t),
            ),
        );
    }
    let (search_view, filter_view, sort_view) = (id.clone(), id.clone(), id.clone());
    bar = bar
        .child(motion::pressable(
            cbtn(t, format!("{id}/dv-search"), Glyph::DvSearch, c.search_open)
                .on_press(move |s: &mut State, _: bool| toggle_search(s, &search_view))
                .build(t),
        ))
        .child(motion::pressable(
            cbtn(
                t,
                format!("{id}/dv-filter"),
                Glyph::DvFilter,
                filters_on || c.open == Panel::Filter,
            )
            .on_press(move |s: &mut State, _: bool| toggle_panel(s, &filter_view, Panel::Filter))
            .build(t),
        ))
        .child(motion::pressable(
            cbtn(
                t,
                format!("{id}/dv-sort"),
                Glyph::DvSort,
                sorts_on || c.open == Panel::Sort,
            )
            .on_press(move |s: &mut State, _: bool| toggle_panel(s, &sort_view, Panel::Sort))
            .build(t),
        ));

    // View settings: layout tiles, then the layout's property row.
    let settings_view = id.clone();
    bar = bar.child(motion::pressable(
        cbtn(
            t,
            format!("{id}/dv-settings"),
            Glyph::DvSettings,
            c.settings_open,
        )
        .on_press(move |s: &mut State, _: bool| {
            if let Some(c) = controls_mut(s, &settings_view) {
                c.settings_open = !c.settings_open;
                c.prop_list_open = false;
                c.chip_open = false;
            }
        })
        .build(t),
    ));
    if c.settings_open {
        // The layout tiles: one kit toggle each, the current one checked.
        let mut tiles = row().key(format!("{id}/layouts")).w_full().gap(4.0);
        for (layout, label) in LAYOUTS {
            let active = view_type == *layout;
            let ink = if active { t.fg } else { t.muted };
            let mut icon = layout_icon(layout, 44.0, ink);
            let mut name = text(*label).font(ty::ui(12.0).lh(15.0)).color(ink);
            if !active {
                icon = icon.group_color(t.kit.accent_foreground);
                name = name.group_color(t.kit.accent_foreground);
            }
            let view = id.clone();
            let mut tile = motion::pressable(col())
                .key(format!("{id}/layout-{layout}"))
                .role("button")
                .checked(active)
                .label(*label)
                .grow(1.0)
                .min_w(0.0)
                .center()
                .gap(2.0)
                .border(1.0, if active { t.accent } else { Color::TRANSPARENT })
                .border_layout()
                .radius(8.0)
                .pt(4.0)
                .px(2.0)
                .pb(6.0)
                .cursor(Cursor::Pointer)
                .child(icon)
                .child(name)
                .on_press(move |s: &mut State| pick_layout(s, &view, layout));
            if !active {
                tile = tile.hover(t.kit.accent).hover_group();
            }
            tiles = tiles.child(tile);
        }
        let mut vmenu = menu_surface(t).min_w(314.0);
        if chooses_prop {
            vmenu = vmenu.child(
                tiles
                    .pb(6.0)
                    .mb(4.0)
                    .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
                    .border_layout(),
            );
            let toggle = id.clone();
            vmenu = vmenu.child(
                vrow(t, &format!("{id}/vsub-row"), c.prop_list_open)
                    .child(vrow_content(
                        common::label(t, prop_title),
                        Some(
                            row()
                                .center_y()
                                .child(prop_caption(
                                    s,
                                    t,
                                    current.as_ref(),
                                    &current_key,
                                    12.0,
                                    t.muted,
                                ))
                                .child(text("›").font(ty::ui(12.0)).color(t.muted).ml(4.0)),
                        ),
                    ))
                    .on_press(move |s: &mut State| {
                        let Some(c) = controls_mut(s, &toggle) else {
                            return;
                        };
                        c.prop_list_open = !c.prop_list_open;
                        if c.prop_list_open {
                            load_used_keys(s, &toggle);
                        }
                    }),
            );
        } else {
            vmenu = vmenu.child(tiles);
        }
        let close = id.clone();
        let vmenu = overlay::dismissable(
            vmenu,
            &format!("{id}/view-menu"),
            move |s: &mut State| {
                if let Some(c) = controls_mut(s, &close) {
                    c.settings_open = false;
                    c.prop_list_open = false;
                    c.new_prop = None;
                }
            },
            layer,
        );
        layer.push(overlay::anchored(
            vmenu,
            &format!("{id}/dv-settings"),
            Placement::BottomEnd,
            6.0,
        ));
        if chooses_prop && c.prop_list_open {
            // `.plist` (274 wide) opens beside its row: left of it when it
            // fits, else on its right (Rune flips the side).
            let list = prop_list(s, t, v, prop_title, calendar, &used, &rest, &current_key);
            layer.push(
                list.anchored(format!("{id}/vsub-row"), Side::Left, Align::Start, 12.0)
                    .transition(motion::appear(0.0))
                    .instant_changes(),
            );
        }
    }

    // New (+ template chevron on type pages).
    let split = c.kind == DataViewKind::Type && !c.type_templates.is_empty();
    let new_view = id.clone();
    let mut new_btn = common::new_button(&format!("{id}/new-btn"), c.creating)
        .on_press(move |s: &mut State| {
            if dv(s, &new_view).is_some_and(|v| !v.controls.creating) {
                new_record(s, &new_view)
            }
        })
        .build(t);
    if split {
        new_btn = new_btn.corners([8.0, 0.0, 0.0, 8.0]);
    }
    let mut new_wrap = row().child(new_btn);
    if split {
        let toggle = id.clone();
        new_wrap = new_wrap.child(
            Button::new(format!("{id}/new-chev"))
                .primary()
                .xsmall()
                .icon(Glyph::KitChevronDown)
                .accessibility_label("New from template…")
                .w(22.0)
                .h(28.0)
                .pointer()
                .tooltip("New from template…")
                .on_press(move |s: &mut State| {
                    if let Some(c) = controls_mut(s, &toggle) {
                        c.tpl_pick_open = !c.tpl_pick_open;
                    }
                })
                .build(t)
                .corners([0.0, 8.0, 8.0, 0.0])
                .border_sides([0.0, 0.0, 0.0, 1.0], roostr_ui::color::rgba(0xffffff40)),
        );
        let default_tpl = field_str(&object.fields, "default_template_id").to_string();
        let create = |tpl: Option<String>| {
            let view = id.clone();
            move |s: &mut State| {
                if let Some(c) = controls_mut(s, &view) {
                    c.tpl_pick_open = false;
                }
                create_record(s, &view, String::new(), Some(tpl.clone())).detach();
            }
        };
        let mut menu = PopupMenu::new()
            .min_w(170.0)
            .item(kit_row(t, "Blank", false).on_press(create(None)));
        for (tpl, name) in &c.type_templates {
            let label = format!(
                "{}{}",
                if name.is_empty() { "Untitled" } else { name },
                if *tpl == default_tpl { " ★" } else { "" }
            );
            menu = menu.item(kit_row(t, label, false).on_press(create(Some(tpl.clone()))));
        }
        let close = id.clone();
        new_wrap = DropdownMenu::new(format!("{id}/tpl-pick"))
            .trigger(new_wrap)
            .menu(menu)
            .open(c.tpl_pick_open)
            .placement(Placement::BottomEnd)
            .gap(4.0)
            .on_close(move |s: &mut State| {
                if let Some(c) = controls_mut(s, &close) {
                    c.tpl_pick_open = false;
                }
            })
            .build(t, layer);
    }
    bar = bar.child(new_wrap);

    let mut root = col().items_stretch().child(bar);
    match c.open {
        Panel::Filter => {
            root = root.child(filter_panel(s, cx, t, layer, v, &object, &relations));
        }
        Panel::Sort => {
            root = root.child(sort_panel(s, cx, t, layer, v, &object, &relations));
        }
        Panel::None => {}
    }
    root
}

#[cfg(test)]
mod tests {
    use super::rule_ids;

    #[test]
    fn rule_ids_follow_rules_through_a_reorder() {
        let ids = |keys: &[&str]| -> Vec<String> { rule_ids("sort", keys.iter().copied()) };
        assert_eq!(
            ids(&["name", "status", "name"]),
            ["sort-name#0", "sort-status#0", "sort-name#1"]
        );
        assert_eq!(ids(&["status", "name"]), ["sort-status#0", "sort-name#0"]);
    }
}

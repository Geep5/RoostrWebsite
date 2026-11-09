//! The one per-format value editor (`PropertyValue.svelte`): shorttext,
//! longtext, number, status, tag, date (calendar picker), checkbox,
//! url/email/phone (typed input + open link), object (link picker with type
//! limits, "+ New agent" / "+ New prompt"), and the `chat_color` hex. Emits
//! complete values through `on_save`; the caller owns persistence.
//!
//! Each instance keeps its editing state in the slot [`ValueEditors`] under
//! the builder's key. Typed text commits on Enter, when its box loses focus
//! (the poll listener [`register`] installs) and on [`commit_pending`].

use std::collections::HashMap;
use std::rc::Rc;

use roostr_core::color::normalize_hex;
use roostr_core::objects::object_icon;
use roostr_core::types::is_system_type;
use roostr_core::{Fields, RelationDef, Route, State, Value, api, lite_ui};
use roostr_ui::{
    Button, ButtonRounded, ColorPicker, ColorPickerState, Glyph, Input, InputState, ListItem,
    ScrollArea, Tag, TagPill, Textarea, TextareaState, Theme, checkbox, checkbox_display, glyph,
    radius, ty,
};
use rune::{Cx, El, Key, KeyEvent, Tx, col, el, row, text};

use crate::calendar::{CalendarEvent, CalendarPickerState, calendar_picker};
use crate::option_picker::{OptionHooks, OptionPickerState, option_picker, rename};
use crate::widgets::{
    FieldSkin, below, empty_note, field, icon_text_el, later, lens, note, pop_shadow, prefix,
    text_area, tip,
};

/// Receives the complete new value.
pub(crate) type SaveFn = Rc<dyn Fn(&mut State, Value)>;

/// The hex field's invalid-value border.
const HEX_ERROR_RED: u32 = 0xe5484d;
/// The chat colour swatch with no colour set.
const NO_CHAT_COLOR: u32 = 0x3a3a3e;

/// One property value: display + inline editor (web PropertyValue.svelte).
pub struct PropertyValue {
    key: String,
    relation: RelationDef,
    value: Option<Value>,
    space: String,
    seeds: Fields,
    auto_open: bool,
    read_only: bool,
    inline_menus: bool,
    on_save: Option<SaveFn>,
}

impl PropertyValue {
    /// The editor for `relation`'s `value` (None = not set), keyed by `key`
    /// (its state's key in [`ValueEditors`] and its root element's key).
    pub fn new(key: impl Into<String>, relation: RelationDef, value: Option<Value>) -> Self {
        Self {
            key: key.into(),
            relation,
            value,
            space: String::new(),
            seeds: Fields::new(),
            auto_open: false,
            read_only: false,
            inline_menus: false,
            on_save: None,
        }
    }

    /// Owning space of the object being edited: agent pickers offer only its agents,
    /// type-limited pickers only its objects.
    pub fn space(mut self, space_id: impl Into<String>) -> Self {
        self.space = space_id.into();
        self
    }

    /// Fields a "+ New agent" made here starts with (a view's filter values).
    pub fn seeds(mut self, seeds: Fields) -> Self {
        self.seeds = seeds;
        self
    }

    /// An editor opened on demand (property pane, table cell): an empty object
    /// property opens its picker at once.
    pub fn auto_open(mut self, auto_open: bool) -> Self {
        self.auto_open = auto_open;
        self
    }

    /// Show the value without editing affordances.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Open the link picker and the calendar in the editor's own flow rather
    /// than floating above the page: an editor that is itself a popover (a
    /// table cell's) closes on a press outside its bounds, which a floating
    /// menu is - picking from it closed the editor before anything was saved.
    pub fn inline_menus(mut self, inline: bool) -> Self {
        self.inline_menus = inline;
        self
    }

    /// Called with the complete new value.
    pub fn on_save(mut self, f: impl Fn(&mut State, Value) + 'static) -> Self {
        self.on_save = Some(Rc::new(f));
        self
    }

    /// The editor, keyed `key`; its menus go into `layer` unless
    /// [`Self::inline_menus`].
    pub fn build(self, s: &State, _cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) -> El {
        let auto_open = self.auto_open;
        let p = Rc::new(Props {
            key: self.key,
            rel: self.relation,
            value: self.value,
            space: self.space,
            seeds: self.seeds,
            read_only: self.read_only,
            inline_menus: self.inline_menus,
            on_save: self.on_save,
        });
        let ed = s.get::<ValueEditors>().and_then(|m| m.0.get(&p.key));
        if needs_sync(&p, ed) {
            let p = p.clone();
            later(s, move |s| sync(s, &p, auto_open));
        }
        let fallback;
        let ed = match ed {
            Some(ed) => ed,
            None => {
                fallback = ValueEditor::default();
                &fallback
            }
        };
        col()
            .key(p.key.clone())
            .w_full()
            .min_w(0.0)
            .child(body(s, t, layer, &p, ed))
    }
}

/// What one frame's builder said, shared by its handlers.
struct Props {
    key: String,
    rel: RelationDef,
    value: Option<Value>,
    space: String,
    seeds: Fields,
    read_only: bool,
    inline_menus: bool,
    on_save: Option<SaveFn>,
}

impl Props {
    fn format(&self) -> &str {
        self.rel.format.as_str()
    }

    fn save(&self, s: &mut State, v: Value) {
        if let Some(f) = &self.on_save {
            f(s, v);
        }
    }
}

/// Every [`PropertyValue`]'s editing state, by builder key.
#[derive(Default)]
pub struct ValueEditors(HashMap<String, ValueEditor>);

struct ValueEditor {
    /// The first sync ran (auto open, served_by machines).
    mounted: bool,
    /// The relation the state was made for (another one resets it).
    rel_id: String,
    rel_key: String,
    /// Single-line text box (text, number, url/email/phone, hex).
    text: InputState,
    /// Multi-line box (longtext, error), sized for `area_rows` (0 = not made yet).
    area: TextareaState,
    area_rows: usize,
    /// The stored text last put in the boxes (an external change replaces an unfocused box).
    synced: Option<String>,
    /// Typed and not committed yet: how to commit it.
    pending: Option<Rc<Props>>,
    hex_error: String,
    /// The `chat_color` swatch: the colour picker Settings uses.
    swatch: ColorPickerState,
    /// The open calendar (`None` = closed).
    calendar: Option<CalendarPickerState>,
    options: OptionPickerState,
    object_open: bool,
    object_query: InputState,
    /// The highlighted candidate (-1 = none yet).
    object_active: i32,
    /// "+ New agent" naming box (Some while naming).
    agent_name: Option<InputState>,
    creating_agent: bool,
    /// `served_by` may hold a machine_id: machine_id → Computer object id.
    machine_objects: HashMap<String, String>,
    machines_asked: bool,
    /// Width of the objects row (the link menu spans it).
    objects_width: f32,
}

impl Default for ValueEditor {
    fn default() -> Self {
        Self {
            mounted: false,
            rel_id: String::new(),
            rel_key: String::new(),
            text: InputState::new(),
            area: TextareaState::new(),
            area_rows: 0,
            synced: None,
            pending: None,
            hex_error: String::new(),
            swatch: ColorPickerState::new(),
            calendar: None,
            options: OptionPickerState::default(),
            object_open: false,
            object_query: InputState::new(),
            object_active: -1,
            agent_name: None,
            creating_agent: false,
            machine_objects: HashMap::new(),
            machines_asked: false,
            objects_width: 240.0,
        }
    }
}

impl ValueEditor {
    fn shown_id(&self, id: &str) -> String {
        self.machine_objects
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }
}

/// The editor keyed `key` (made on first use).
fn editor<'a>(s: &'a mut State, key: &str) -> &'a mut ValueEditor {
    s.slot::<ValueEditors>()
        .0
        .entry(key.to_string())
        .or_default()
}

/// The editor keyed `key` if it still exists.
fn existing<'a>(s: &'a mut State, key: &str) -> Option<&'a mut ValueEditor> {
    s.slot::<ValueEditors>().0.get_mut(key)
}

/// Installs the commit-on-blur poll listener.
pub(crate) fn register(s: &mut State) {
    s.on_poll(poll);
}

/// Drops the state of the editor keyed `key`.
pub fn forget(s: &mut State, key: &str) {
    s.slot::<ValueEditors>().0.remove(key);
}

/// Commits every value editor with typed, uncommitted text (GPUI's
/// `CommitValue` action): call before a host action that would lose it.
pub fn commit_pending(s: &mut State) {
    let pending: Vec<(String, Rc<Props>)> = s
        .slot::<ValueEditors>()
        .0
        .iter()
        .filter_map(|(k, ed)| ed.pending.clone().map(|p| (k.clone(), p)))
        .collect();
    for (key, p) in pending {
        commit_text(s, &key, &p);
    }
}

/// Text typed into a box that has lost focus commits (GPUI's `InputEvent::Blur`);
/// an option name box that lost focus renames.
fn poll(s: &mut State) -> bool {
    let Some(eds) = s.get::<ValueEditors>() else {
        return false;
    };
    let blurred: Vec<(String, Rc<Props>)> = eds
        .0
        .iter()
        .filter_map(|(k, ed)| {
            let p = ed.pending.as_ref()?;
            let focused = if is_longtext(&p.rel) {
                ed.area.is_focused()
            } else {
                ed.text.is_focused()
            };
            (!focused).then(|| (k.clone(), p.clone()))
        })
        .collect();
    let renames: Vec<_> = s
        .slot::<ValueEditors>()
        .0
        .values_mut()
        .filter_map(|ed| ed.options.blurred_rename())
        .collect();
    let changed = !blurred.is_empty() || !renames.is_empty();
    for (key, p) in blurred {
        commit_text(s, &key, &p);
    }
    for r in renames {
        rename(s, r);
    }
    changed
}

/// What a value's text box shows.
fn text_of(value: Option<&Value>) -> String {
    value
        .and_then(|v| v.string_value.clone())
        .unwrap_or_default()
}

fn number_of(value: Option<&Value>) -> Option<f64> {
    let v = value?;
    v.int_value.map(|n| n as f64).or(v.float_value)
}

/// JS `String(number)` for the values we store.
pub(crate) fn number_text(n: f64) -> String {
    if n.fract() == 0. && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// `items`: the values' strings (tag/status lists).
fn items_of(value: Option<&Value>) -> Vec<String> {
    value
        .map(|v| {
            v.items()
                .iter()
                .filter_map(|i| i.string_value.clone())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Object-format values: link lists (relation-stamped); legacy rows may be plain strings.
pub(crate) fn object_ids(value: Option<&Value>) -> Vec<String> {
    let Some(v) = value else { return vec![] };
    if let Some(link) = &v.link_value
        && !link.target_id.is_empty()
    {
        return vec![link.target_id.clone()];
    }
    if let Some(s) = &v.string_value
        && !s.is_empty()
    {
        return vec![s.clone()];
    }
    v.items()
        .iter()
        .map(|i| {
            i.link_value
                .as_ref()
                .map(|l| l.target_id.clone())
                .or_else(|| i.string_value.clone())
                .unwrap_or_default()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

fn list_value(xs: &[String]) -> Value {
    Value::values(xs.iter().map(|s| Value::string(s.clone())).collect())
}

/// `showDate(ms, {year: numeric, month: short, day: numeric})` → "Oct 18, 2026".
pub(crate) fn date_label(ms: i64) -> String {
    roostr_core::dates::show_date_with(ms, roostr_core::dates::DateStyle::MonthDayYear)
}

/// `Number(raw)` for a typed number ("" is not a value here: onchange of an emptied box is ignored).
fn parse_js_number(raw: &str) -> Option<f64> {
    let t = raw.trim();
    if t.is_empty() {
        return Some(0.);
    }
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn is_longtext(rel: &RelationDef) -> bool {
    rel.key != "chat_color" && (rel.format == "longtext" || rel.key == "error")
}

/// The textarea's minimum rows.
fn area_rows(rel: &RelationDef) -> usize {
    if rel.key == "error" { 5 } else { 3 }
}

/// The text the box shows for the stored value.
fn stored_text(p: &Props) -> String {
    match p.format() {
        "number" => number_of(p.value.as_ref())
            .map(number_text)
            .unwrap_or_default(),
        _ => text_of(p.value.as_ref()),
    }
}

/// The `chat_color` swatch's colour for the stored value.
fn chat_color(p: &Props) -> rune::Color {
    normalize_hex(&text_of(p.value.as_ref()))
        .as_deref()
        .and_then(roostr_ui::color::hex)
        .unwrap_or(roostr_ui::color::rgb(NO_CHAT_COLOR))
}

fn box_focused(p: &Props, ed: &ValueEditor) -> bool {
    if is_longtext(&p.rel) {
        ed.area.is_focused()
    } else {
        ed.text.is_focused()
    }
}

/// Whether the state lags what the builder says (see [`sync`]).
fn needs_sync(p: &Props, ed: Option<&ValueEditor>) -> bool {
    let Some(ed) = ed else { return true };
    if !ed.mounted || ed.rel_id != p.rel.id || ed.rel_key != p.rel.key {
        return true;
    }
    if p.rel.key == "served_by" && !ed.machines_asked {
        return true;
    }
    if is_longtext(&p.rel) && ed.area_rows != area_rows(&p.rel) {
        return true;
    }
    if p.rel.key == "chat_color" && ed.swatch.value() != Some(chat_color(p)) {
        return true;
    }
    ed.synced.as_deref() != Some(stored_text(p).as_str()) && !box_focused(p, ed)
}

/// Brings the editor's state up to the builder's props (GPUI's `set_props`):
/// first mount (auto-open), a new relation, the served_by machines, and
/// the boxes kept on the stored value unless one is being edited.
fn sync(s: &mut State, p: &Props, auto_open: bool) {
    let longtext = is_longtext(&p.rel);
    let stored = stored_text(p);
    let open_now = {
        let ed = editor(s, &p.key);
        let first = !ed.mounted;
        ed.mounted = true;
        if ed.rel_id != p.rel.id || ed.rel_key != p.rel.key {
            ed.rel_id = p.rel.id.clone();
            ed.rel_key = p.rel.key.clone();
            ed.synced = None;
            ed.options = OptionPickerState::default();
        }
        if longtext && ed.area_rows != area_rows(&p.rel) {
            ed.area_rows = area_rows(&p.rel);
            ed.area = TextareaState::new().auto_grow(ed.area_rows, 24);
            ed.synced = None;
        }
        if ed.synced.as_deref() != Some(stored.as_str()) {
            if longtext {
                if !ed.area.is_focused() {
                    ed.area.set_value(stored.clone());
                    ed.synced = Some(stored);
                }
            } else if !ed.text.is_focused() {
                ed.text.set_value(stored.clone());
                ed.synced = Some(stored);
            }
        }
        if p.rel.key == "chat_color" {
            let color = chat_color(p);
            if ed.swatch.value() != Some(color) {
                ed.swatch.set_value(color);
            }
        }
        let open_now = first
            && auto_open
            && p.format() == "object"
            && object_ids(p.value.as_ref()).is_empty()
            && !p.read_only;
        if open_now {
            ed.object_open = true;
            ed.object_query.focus();
        }
        open_now
    };
    if open_now && !p.space.is_empty() {
        lite_ui::ensure_space(&p.space, s);
    }
    let ed = editor(s, &p.key);
    if p.rel.key == "served_by" && !ed.machines_asked {
        ed.machines_asked = true;
        load_machines(s, &p.key);
    }
}

/// `onchange`: the box lost focus or took Enter with a changed value.
fn commit_text(s: &mut State, key: &str, p: &Props) {
    let Some(ed) = existing(s, key) else { return };
    ed.pending = None;
    let raw = if is_longtext(&p.rel) {
        ed.area.value().to_string()
    } else {
        ed.text.value().to_string()
    };
    if ed.synced.as_deref() == Some(raw.as_str()) {
        return;
    }
    ed.synced = Some(raw.clone());
    if p.rel.key == "chat_color" {
        save_hex(s, key, p, &raw);
        return;
    }
    match p.format() {
        "number" => {
            let Some(n) = parse_js_number(&raw) else {
                return;
            };
            let v = if n.fract() == 0. && n.abs() < 9.007_199_254_740_992e15 {
                Value::int(n as i64)
            } else {
                Value::float(n)
            };
            p.save(s, v);
        }
        "url" | "email" | "phone" => p.save(s, Value::string(raw.trim())),
        _ => p.save(s, Value::string(raw)),
    }
}

fn save_hex(s: &mut State, key: &str, p: &Props, raw: &str) {
    let hex = if raw.trim().is_empty() {
        Some(String::new())
    } else {
        normalize_hex(raw)
    };
    let Some(ed) = existing(s, key) else { return };
    match hex {
        None => ed.hex_error = "Use a hex color like #EDCF7F".into(),
        Some(hex) => {
            ed.hex_error.clear();
            p.save(s, Value::string(hex));
        }
    }
}

fn option_selection(p: &Props) -> (Vec<String>, bool) {
    let items = items_of(p.value.as_ref());
    if p.format() == "status" {
        let text = text_of(p.value.as_ref());
        let sel = if !items.is_empty() {
            items
        } else if !text.is_empty() {
            vec![text]
        } else {
            vec![]
        };
        (sel, false)
    } else {
        (items, true)
    }
}

/// An option picked: status selects one (or clears it), tag toggles.
fn pick_option(s: &mut State, p: &Props, t: &str) {
    let (selected, _) = option_selection(p);
    let next: Vec<String> = if p.format() == "status" {
        if selected.iter().any(|x| x == t) {
            vec![]
        } else {
            vec![t.to_string()]
        }
    } else if selected.iter().any(|x| x == t) {
        selected.into_iter().filter(|x| x != t).collect()
    } else {
        selected.into_iter().chain([t.to_string()]).collect()
    };
    p.save(s, list_value(&next));
}

fn toggle_calendar(s: &mut State, p: &Props) {
    let value = number_of(p.value.as_ref()).unwrap_or(0.) as i64;
    let ed = editor(s, &p.key);
    ed.calendar = match ed.calendar {
        Some(_) => None,
        None => Some(CalendarPickerState::new(value)),
    };
}

// ── Object links ──────────────────────────────────────────────

fn load_machines(s: &mut State, key: &str) {
    let task = roostr_core::serving::fetch_machines(s);
    let key = key.to_string();
    s.rt.spawn(async move |cx| {
        let Ok((_, machines)) = task.await else {
            return;
        };
        let map: HashMap<String, String> = machines
            .into_iter()
            .filter(|m| !m.machine_id.is_empty())
            .map(|m| (m.machine_id, m.id))
            .collect();
        cx.update(move |s| {
            if let Some(ed) = existing(s, &key) {
                ed.machine_objects = map;
            }
        });
    })
    .detach();
}

/// A picker candidate.
#[derive(Clone)]
struct Candidate {
    id: String,
    name: String,
    type_key: String,
    icon: String,
    done: Option<bool>,
}

/// Type keys the relation is limited to (Anytype relationFormatObjectTypes).
fn allowed_type_keys(s: &State, rel: &RelationDef) -> Vec<String> {
    rel.object_types
        .iter()
        .flatten()
        .filter_map(|id| {
            s.store
                .types
                .iter()
                .find(|t| &t.id == id)
                .map(|t| t.key.clone())
        })
        .filter(|k| !k.is_empty())
        .collect()
}

fn candidates(s: &State, p: &Props, ed: &ValueEditor) -> Vec<Candidate> {
    let store = &s.store;
    let ids = object_ids(p.value.as_ref());
    let q = ed.object_query.value().trim().to_lowercase();
    let allowed = allowed_type_keys(s, &p.rel);
    let wants_agents = allowed.iter().any(|k| k == "agent");
    let space = p.space.as_str();
    let mut pool: Vec<Candidate> = if wants_agents {
        store
            .agents
            .iter()
            .filter(|a| {
                !ids.contains(&a.id)
                    && (space.is_empty() || a.channel.is_empty() || a.channel == space)
            })
            .map(|a| Candidate {
                id: a.id.clone(),
                name: a.name.clone(),
                type_key: "agent".into(),
                icon: a.icon.clone(),
                done: None,
            })
            .collect()
    } else {
        store
            .summaries
            .iter()
            .filter(|o| {
                !ids.iter().any(|id| ed.shown_id(id) == o.id)
                    && (!is_system_type(&o.type_key) || allowed.contains(&o.type_key))
            })
            .filter(|o| allowed.is_empty() || allowed.contains(&o.type_key))
            // An object links within its own space: the picker offers that space's own.
            .filter(|o| space.is_empty() || o.channel_id.is_empty() || o.channel_id == space)
            .map(|o| Candidate {
                id: o.id.clone(),
                name: o.name.clone(),
                type_key: o.type_key.clone(),
                icon: o.icon.clone(),
                done: o.done,
            })
            .collect()
    };
    pool.retain(|c| q.is_empty() || c.name.to_lowercase().contains(&q));
    if !wants_agents && allowed.is_empty() {
        pool.truncate(8);
    }
    pool
}

/// "+ New agent" / "+ New prompt" offered for agent- / prompt-limited relations.
fn new_kind(s: &State, rel: &RelationDef) -> &'static str {
    let allowed = allowed_type_keys(s, rel);
    if allowed.iter().any(|k| k == "agent") {
        "agent"
    } else if allowed.iter().any(|k| k == "system_prompt") {
        "prompt"
    } else {
        ""
    }
}

/// The type a typed name becomes (relations other than agent / prompt):
/// the first type the relation is limited to, else a page.
fn create_type(s: &State, rel: &RelationDef) -> String {
    allowed_type_keys(s, rel)
        .into_iter()
        .next()
        .unwrap_or_else(|| "page".into())
}

/// `(name, type label)` when the picker should offer to create an object
/// named after the query: a non-empty query that names no candidate, on a
/// relation without its own "New agent / prompt" flow.
fn offer_create(s: &State, p: &Props, ed: &ValueEditor) -> Option<(String, String)> {
    if p.read_only || !new_kind(s, &p.rel).is_empty() {
        return None;
    }
    let name = ed.object_query.value().trim().to_string();
    if name.is_empty() {
        return None;
    }
    let lower = name.to_lowercase();
    if candidates(s, p, ed)
        .iter()
        .any(|c| c.name.trim().to_lowercase() == lower)
    {
        return None;
    }
    let key = create_type(s, &p.rel);
    let types = &s.store.types;
    let label = types
        .iter()
        .find(|t| t.key == key && (p.space.is_empty() || t.space == p.space))
        .or_else(|| types.iter().find(|t| t.key == key))
        .map(|t| t.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or(key);
    Some((name, label))
}

/// The space new objects go to: the editor's, else the first.
fn target_channel(s: &State, p: &Props) -> String {
    if p.space.is_empty() {
        s.store
            .channels
            .first()
            .map(|c| c.id.clone())
            .unwrap_or_default()
    } else {
        p.space.clone()
    }
}

/// Make an object named after the query in this space and link it.
fn create_from_query(s: &mut State, p: &Rc<Props>) {
    let Some(ed) = s.get::<ValueEditors>().and_then(|m| m.0.get(&p.key)) else {
        return;
    };
    let Some((name, _)) = offer_create(s, p, ed) else {
        return;
    };
    if ed.creating_agent {
        return;
    }
    let channel = target_channel(s, p);
    let fields = if channel.is_empty() {
        Fields::new()
    } else {
        Fields::from([("channel".to_string(), Value::string(channel))])
    };
    let type_key = create_type(s, &p.rel);
    editor(s, &p.key).creating_agent = true;
    let task = roostr_core::create::from_type(&name, &type_key, fields, s);
    let p = p.clone();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(move |s| {
            let Some(ed) = existing(s, &p.key) else {
                return;
            };
            ed.creating_agent = false;
            match result {
                Ok(id) => {
                    ed.object_open = false;
                    toggle_object(s, &p, &id);
                }
                Err(error) => log::error!("create: {error:#}"),
            }
        });
    })
    .detach();
}

fn toggle_object_menu(s: &mut State, p: &Props) {
    let ed = editor(s, &p.key);
    ed.object_open = !ed.object_open;
    ed.agent_name = None;
    if ed.object_open {
        ed.object_query.focus();
        // A Lite copy holds only what was opened: the picker downloads
        // the space so it can offer every candidate in it.
        if !p.space.is_empty() {
            lite_ui::ensure_space(&p.space, s);
        }
    }
}

fn close_object_menu(s: &mut State, key: &str) {
    if let Some(ed) = existing(s, key) {
        ed.object_open = false;
        ed.agent_name = None;
    }
}

fn pick_candidate(s: &mut State, p: &Props, id: &str) {
    editor(s, &p.key).object_open = false;
    toggle_object(s, p, id);
}

fn toggle_object(s: &mut State, p: &Props, id: &str) {
    let ids = object_ids(p.value.as_ref());
    // A single-value relation (System prompt, Served by) swaps its link; a list adds.
    let next: Vec<String> = if ids.iter().any(|x| x == id) {
        ids.into_iter().filter(|x| x != id).collect()
    } else if p.rel.max_count == 1 {
        vec![id.to_string()]
    } else {
        ids.into_iter().chain([id.to_string()]).collect()
    };
    editor(s, &p.key).object_query.clear();
    let v = Value::values(
        next.into_iter()
            .map(|t| Value::link(t, p.rel.key.clone()))
            .collect(),
    );
    p.save(s, v);
}

fn move_active(s: &mut State, p: &Props, down: bool) {
    let Some(ed) = s.get::<ValueEditors>().and_then(|m| m.0.get(&p.key)) else {
        return;
    };
    let n = candidates(s, p, ed).len() as i32;
    if n == 0 {
        return;
    }
    let ed = editor(s, &p.key);
    ed.object_active = if down {
        (ed.object_active + 1) % n
    } else if ed.object_active <= 0 {
        n - 1
    } else {
        ed.object_active - 1
    };
}

/// Enter in the query: the highlighted (else first) candidate, else create from the query.
fn enter_query(s: &mut State, p: &Rc<Props>) {
    let Some(ed) = s.get::<ValueEditors>().and_then(|m| m.0.get(&p.key)) else {
        return;
    };
    let found = candidates(s, p, ed);
    if let Some(c) = found.get(ed.object_active.max(0) as usize) {
        let id = c.id.clone();
        pick_candidate(s, p, &id);
    } else if offer_create(s, p, ed).is_some() {
        create_from_query(s, p);
    }
}

/// The menu's keys (GPUI captured MoveUp / MoveDown / Escape on the whole menu).
fn menu_key(s: &mut State, p: &Props, ev: &KeyEvent) -> bool {
    match ev.key {
        Key::Down => move_active(s, p, true),
        Key::Up => move_active(s, p, false),
        Key::Escape => {
            let ed = editor(s, &p.key);
            if ed.agent_name.is_some() {
                ed.agent_name = None;
            } else {
                close_object_menu(s, &p.key);
            }
        }
        _ => return false,
    }
    true
}

fn start_naming(s: &mut State, p: &Props) {
    let ed = editor(s, &p.key);
    let seed = ed.object_query.value().trim().to_string();
    let mut input = InputState::with_value(seed);
    input.focus();
    input.select_all();
    ed.agent_name = Some(input);
}

/// Make the agent / prompt here and link it.
fn add_new(s: &mut State, p: &Rc<Props>) {
    let Some(ed) = existing(s, &p.key) else {
        return;
    };
    let Some(input) = &ed.agent_name else { return };
    let name = input.value().trim().to_string();
    if ed.creating_agent || name.is_empty() {
        return;
    }
    ed.creating_agent = true;
    let channel = target_channel(s, p);
    let task = if new_kind(s, &p.rel) == "agent" {
        roostr_core::create::new_agent(&channel, &name, p.seeds.clone(), s)
    } else {
        let fields = (!channel.is_empty())
            .then(|| Fields::from([("channel".to_string(), Value::string(channel.clone()))]));
        api::note::create(
            &name,
            Some("system_prompt"),
            Some(fields.unwrap_or_default()),
            s,
        )
    };
    let p = p.clone();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(move |s| {
            let Some(ed) = existing(s, &p.key) else {
                return;
            };
            ed.creating_agent = false;
            match result {
                Ok(id) => {
                    ed.object_open = false;
                    ed.agent_name = None;
                    toggle_object(s, &p, &id);
                }
                Err(error) => log::error!("create: {error:#}"),
            }
        });
    })
    .detach();
}

// ── Drawing ───────────────────────────────────────────────────

fn body(s: &State, t: &Theme, layer: &mut Vec<El>, p: &Rc<Props>, ed: &ValueEditor) -> El {
    let format = p.format();
    if format == "repeat" {
        // Owned by Repeat on the object page; this only guards a stray mount.
        return el();
    }
    if format == "checkbox" {
        let checked = p.value.as_ref().and_then(|v| v.bool_value) == Some(true);
        if p.read_only {
            return checkbox_display("chk", checked).build(t);
        }
        let p = p.clone();
        return checkbox("chk", checked)
            .on_change(move |s: &mut State, next: bool| p.save(s, Value::bool(next)))
            .build(t);
    }
    if p.read_only {
        return read_only(s, t, p, ed);
    }
    match format {
        "date" => date(t, layer, p, ed),
        "status" | "tag" => {
            let (selected, multi) = option_selection(p);
            let key = p.key.clone();
            let alive_key = p.key.clone();
            let pick = p.clone();
            let hooks = OptionHooks {
                lens: lens(move |s: &mut State| &mut editor(s, &key).options),
                alive: Rc::new(move |s: &State| {
                    s.get::<ValueEditors>()
                        .is_some_and(|m| m.0.contains_key(&alive_key))
                }),
                on_pick: Rc::new(move |s: &mut State, text: String| pick_option(s, &pick, &text)),
            };
            col().w_full().child(option_picker(
                s,
                t,
                &p.rel,
                &selected,
                multi,
                &ed.options,
                hooks,
            ))
        }
        "url" | "email" | "phone" => {
            let value = text_of(p.value.as_ref());
            let href = match format {
                "email" => format!("mailto:{value}"),
                "phone" => format!("tel:{value}"),
                _ if value.starts_with("http://") || value.starts_with("https://") => value.clone(),
                _ => format!("https://{value}"),
            };
            let mut line = row()
                .items_center()
                .gap(6.0)
                .w_full()
                .child(text_field(t, p, ed).grow(1.0));
            if !value.is_empty() {
                line = line.child(
                    Button::new("open")
                        .ghost()
                        .xsmall()
                        .icon(Glyph::ArrowUpRight)
                        .icon_size(14.0)
                        .text_color(t.accent)
                        .tooltip("Open")
                        .accessibility_label("Open")
                        .on_press_tx(move |_: &mut State, tx: &mut Tx| {
                            if let Err(error) = tx.open_url(&href) {
                                log::error!("open {href}: {error:?}");
                            }
                        })
                        .build(t),
                );
            }
            line
        }
        "object" => objects(s, t, layer, p, ed),
        _ if p.rel.key == "chat_color" => chat_color_field(t, layer, p, ed),
        _ if is_longtext(&p.rel) => {
            let (key, mark) = (p.key.clone(), p.clone());
            let area = Textarea::new("area", &ed.area)
                .placeholder("Empty")
                .on_edit(move |s: &mut State| &mut editor(s, &key).area)
                .on_change(move |s: &mut State, _: &mut Tx| {
                    editor(s, &mark.key).pending = Some(mark.clone())
                });
            text_area(t, area, ed.area.is_focused(), t.bg, radius::SM, (5.0, 8.0))
        }
        _ => text_field(t, p, ed).w_full(),
    }
}

/// The single-line box: commits on Enter (and on blur, through the poll).
fn text_field(t: &Theme, p: &Rc<Props>, ed: &ValueEditor) -> El {
    let placeholder = match p.format() {
        _ if p.rel.key == "chat_color" => "#RRGGBB",
        "email" => "name@example.com",
        "phone" => "+1 555 0100",
        "url" => "example.com",
        _ => "Empty",
    };
    let (key, mark, submit) = (p.key.clone(), p.clone(), p.clone());
    let input = Input::new("text", &ed.text)
        .placeholder(placeholder)
        .on_edit(move |s: &mut State| &mut editor(s, &key).text)
        .on_change(move |s: &mut State, _: &mut Tx| {
            editor(s, &mark.key).pending = Some(mark.clone())
        })
        .on_submit(move |s: &mut State, _: &mut Tx| commit_text(s, &submit.key.clone(), &submit));
    field(t, input, ed.text.is_focused(), FieldSkin::Value)
}

fn date(t: &Theme, layer: &mut Vec<El>, p: &Rc<Props>, ed: &ValueEditor) -> El {
    let num = number_of(p.value.as_ref()).unwrap_or(0.) as i64;
    let label = if num != 0 {
        date_label(num)
    } else {
        "Pick a date".into()
    };
    let ink = if num == 0 { t.muted } else { t.fg };
    let toggle = p.clone();
    let button = Button::new("date-btn")
        .outline()
        .bg(t.bg)
        .border_color(t.border)
        .radius(radius::SM)
        .h_auto()
        .px(10.0)
        .py(5.0)
        .text_color(ink)
        .child(text(label).font(ty::ui(13.0)).color(ink))
        .on_press(move |s: &mut State| toggle_calendar(s, &toggle))
        .build(t);
    let anchor = format!("{}-date", p.key);
    let wrap = row().key(anchor.clone()).child(button);
    let Some(cal) = &ed.calendar else { return wrap };
    let key = p.key.clone();
    let on = p.clone();
    let surface = calendar_picker(
        t,
        "date-calendar",
        cal,
        lens(move |s: &mut State| {
            editor(s, &key)
                .calendar
                .get_or_insert_with(|| CalendarPickerState::new(0))
        }),
        Rc::new(move |s: &mut State, ev: CalendarEvent| {
            editor(s, &on.key).calendar = None;
            if let CalendarEvent::Pick(ms) = ev {
                on.save(s, Value::int(ms.unwrap_or(0)));
            }
        }),
    );
    if p.inline_menus {
        return col().items_start().gap(6.0).child(wrap).child(surface);
    }
    below(layer, &anchor, 6.0, surface);
    wrap
}

fn chat_color_field(t: &Theme, layer: &mut Vec<El>, p: &Rc<Props>, ed: &ValueEditor) -> El {
    let bad = !ed.hex_error.is_empty();
    let (key, on) = (p.key.clone(), p.clone());
    // The same picker as Settings → Chat color, kept in step with the value.
    let picker = ColorPicker::new(
        format!("{}-swatch", p.key),
        &ed.swatch,
        move |s: &mut State| &mut editor(s, &key).swatch,
    )
    .accessibility_label("Chat color picker")
    .on_change(move |s: &mut State, color: Option<rune::Color>| {
        let Some(color) = color else { return };
        let hex = roostr_ui::color::to_hex(color);
        let ed = editor(s, &on.key);
        ed.text.set_value(hex.clone());
        ed.synced = Some(hex.clone());
        save_hex(s, &on.key, &on, &hex);
    })
    .build(t, layer);
    let mut hex = text_field(t, p, ed);
    if bad {
        hex = hex.border(1.0, roostr_ui::color::rgb(HEX_ERROR_RED));
    }
    let mut hex = row().key("hex").grow(1.0).child(hex.grow(1.0));
    if bad {
        hex = tip(hex, ed.hex_error.clone());
    }
    row()
        .items_center()
        .gap(6.0)
        .w_full()
        .child(
            row()
                .w(34.0)
                .h(26.0)
                .padding(2.0)
                .center()
                .radius(radius::XS)
                .fill(t.panel)
                .border(1.0, t.border)
                .child(picker),
        )
        .child(hex)
}

/// What leads a linked object's name: an agent's emoji, a task's checkbox,
/// else the object's icon.
fn object_lead(s: &State, t: &Theme, p: &Props, id: &str, shown: &str, key: String) -> El {
    let store = &s.store;
    let summary = store.summary(shown);
    let agent = store.agent(id);
    if agent.is_some() || summary.is_some_and(|o| o.type_key == "agent") {
        let icon = agent
            .map(|a| a.icon.clone())
            .filter(|i| !i.is_empty())
            .or_else(|| summary.map(|o| o.icon.clone()).filter(|i| !i.is_empty()))
            .unwrap_or_else(|| "🤖".into());
        return icon_text_el(s, &icon, 12.0, ty::ui(12.0));
    }
    if let Some(o) = summary.filter(|o| store.layout_of(&o.type_key) == "task") {
        return checkbox_display(key, o.done == Some(true))
            .xsmall()
            .build(t);
    }
    let (icon, tk) = summary
        .map(|o| (o.icon.as_str(), o.type_key.as_str()))
        .unwrap_or(("", "note"));
    icon_text_el(
        s,
        &object_icon(store, icon, tk, &p.space),
        12.0,
        ty::ui(12.0),
    )
}

fn objects(s: &State, t: &Theme, layer: &mut Vec<El>, p: &Rc<Props>, ed: &ValueEditor) -> El {
    let store = &s.store;
    let ids = object_ids(p.value.as_ref());
    let paint = roostr_ui::tags::object_pill_paint(t, t.fg);
    let anchor = format!("{}-objects", p.key);
    let width_key = p.key.clone();
    let mut chips = row()
        .key(anchor.clone())
        .flex_wrap()
        .gap(4.0)
        .items_center()
        .on_layout(move |s: &mut State, r: [f32; 4]| {
            if let Some(ed) = existing(s, &width_key)
                && (ed.objects_width - r[2]).abs() > 0.5
            {
                ed.objects_width = r[2];
            }
        });
    for id in &ids {
        let shown = ed.shown_id(id);
        let name = store
            .summary(&shown)
            .map(|o| o.name.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| {
                store
                    .agent(&shown)
                    .map(|a| a.name.clone())
                    .filter(|n| !n.is_empty())
            })
            .unwrap_or_else(|| prefix(id, 8));
        let lead = object_lead(s, t, p, id, &shown, format!("chip-done-{id}"));
        let target = shown.clone();
        let mut chip = Tag::custom(paint.bg, paint.fg, paint.border)
            .rounded_full()
            .gap(2.0)
            .pl(8.0)
            .pr(4.0)
            .py(1.0)
            .text_size(12.0)
            .child(
                Button::new(format!("chip-{id}"))
                    .ghost()
                    .xsmall()
                    .h_auto()
                    .px(2.0)
                    .child(
                        row()
                            .gap(4.0)
                            .center_y()
                            .child(lead)
                            .child(text(name).font(ty::ui(12.0)).color(t.fg)),
                    )
                    .on_press(move |s: &mut State| {
                        roostr_core::nav::navigate(Route::Object(target.clone()), s)
                    })
                    .build(t),
            );
        if !p.read_only {
            let (remove, on) = (id.clone(), p.clone());
            chip = chip.child(
                Button::new(format!("rm-{id}"))
                    .ghost()
                    .xsmall()
                    .child(glyph(Glyph::Close, 11.0).color(t.muted))
                    .text_color(t.muted)
                    .tooltip("Remove")
                    .accessibility_label("Remove")
                    .on_press(move |s: &mut State| toggle_object(s, &on, &remove))
                    .build(t),
            );
        }
        chips = chips.child(chip.build(t));
    }
    let toggle = p.clone();
    chips = chips.child(
        Button::new("link-object")
            .outline()
            .xsmall()
            .rounded(ButtonRounded::Px(999.0))
            .border_color(t.border)
            .px(8.0)
            .child(text("+ link object").font(ty::ui(11.0)).color(t.fg))
            .on_press(move |s: &mut State| toggle_object_menu(s, &toggle))
            .build(t),
    );
    if !ed.object_open {
        return chips;
    }
    let menu = object_menu(s, t, p, ed);
    if p.inline_menus {
        return col().gap(6.0).child(chips).child(menu);
    }
    below(layer, &anchor, 6.0, menu);
    chips
}

/// The link picker: query, candidates, the space notice, create-from-query,
/// "+ New agent / prompt". Escape or a press outside closes it.
fn object_menu(s: &State, t: &Theme, p: &Rc<Props>, ed: &ValueEditor) -> El {
    let store = &s.store;
    let found = candidates(s, p, ed);
    let kind = new_kind(s, &p.rel);
    let query = {
        let (key, change, submit, keys) = (p.key.clone(), p.key.clone(), p.clone(), p.clone());
        Input::new("object-query", &ed.object_query)
            .placeholder("Search objects…")
            .on_edit(move |s: &mut State| &mut editor(s, &key).object_query)
            .on_change(move |s: &mut State, _: &mut Tx| editor(s, &change).object_active = -1)
            .on_submit(move |s: &mut State, _: &mut Tx| enter_query(s, &submit))
            .on_key(move |s: &mut State, ev: &KeyEvent, _: &mut Tx| menu_key(s, &keys, ev))
    };
    let mut menu = col().gap(2.0).padding(8.0).child(el().mb(4.0).child(field(
        t,
        query,
        ed.object_query.is_focused(),
        FieldSkin::Filter,
    )));
    for (i, c) in found.iter().enumerate() {
        let active = i as i32 == ed.object_active;
        let lead = if c.type_key == "agent" {
            let icon = if !c.icon.is_empty() {
                c.icon.clone()
            } else {
                store
                    .agent(&c.id)
                    .map(|a| a.icon.clone())
                    .filter(|i| !i.is_empty())
                    .unwrap_or_else(|| "🤖".into())
            };
            icon_text_el(s, &icon, 13.0, ty::ui(13.0))
        } else if store.layout_of(&c.type_key) == "task" {
            checkbox_display(format!("cand-done-{}", c.id), c.done == Some(true))
                .xsmall()
                .build(t)
        } else {
            icon_text_el(
                s,
                &object_icon(store, &c.icon, &c.type_key, &p.space),
                13.0,
                ty::ui(13.0),
            )
        };
        let name = if c.name.is_empty() {
            "Untitled".to_string()
        } else {
            c.name.clone()
        };
        let (hover, pick, id) = (p.key.clone(), p.clone(), c.id.clone());
        menu = menu.child(
            ListItem::new(format!("cand-{}", c.id))
                .px(6.0)
                .py(5.0)
                .radius(radius::SM)
                .text_size(13.0)
                .pointer()
                .selected(active)
                .child(
                    row()
                        .grow(1.0)
                        .min_w(0.0)
                        .center_y()
                        .gap(6.0)
                        .child(
                            row()
                                .grow(1.0)
                                .shrink()
                                .min_w(0.0)
                                .center_y()
                                .gap(4.0)
                                .child(lead)
                                .child(text(name).font(ty::ui(13.0)).color(t.fg).truncate()),
                        )
                        .child(text(c.type_key.clone()).font(ty::ui(11.0)).color(t.muted)),
                )
                .on_hover(move |s: &mut State| {
                    let ed = editor(s, &hover);
                    if ed.object_active != i as i32 {
                        ed.object_active = i as i32;
                    }
                })
                .on_press(move |s: &mut State| pick_candidate(s, &pick, &id))
                .build(t),
        );
    }
    // Lite, while the space downloads (or before): the list may be partial.
    if let Some(notice) = lite_ui::space_notice(&p.space, s) {
        let mut line = row()
            .key("picker-space")
            .flex_wrap()
            .items_center()
            .gap(10.0)
            .py(6.0)
            .child(text(notice.text).font(ty::ui(12.0)).color(t.muted).wrap());
        if let Some(action) = notice.action {
            let label = action.label();
            line = line.child(
                roostr_ui::outline_button("picker-space-action", label)
                    .on_press(move |s: &mut State| action.run(s))
                    .build(t),
            );
        }
        menu = menu.child(line);
    }
    if found.is_empty() {
        menu = menu.child(
            el().px(6.0)
                .py(4.0)
                .child(empty_note(t, "No matches", 11.0)),
        );
    }
    if let Some((name, label)) = offer_create(s, p, ed) {
        let creating = ed.creating_agent;
        let create = p.clone();
        let item = ListItem::new("create-from-query")
            .px(6.0)
            .py(5.0)
            .radius(radius::SM)
            .text_size(13.0)
            .text_color(t.accent)
            .disabled(creating)
            .label(if creating {
                "Creating…".to_string()
            } else {
                format!("＋ Create {label} “{name}”")
            });
        let item = if creating {
            item
        } else {
            item.pointer()
                .on_press(move |s: &mut State| create_from_query(s, &create))
        };
        menu = menu.child(
            el().mt(2.0)
                .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
                .opacity(if creating { 0.5 } else { 1.0 })
                .child(item.build(t)),
        );
    }
    if !kind.is_empty() {
        menu = menu.child(match &ed.agent_name {
            None => {
                let start = p.clone();
                el().mt(2.0)
                    .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
                    .child(
                        ListItem::new("new-kind")
                            .px(6.0)
                            .py(5.0)
                            .radius(radius::SM)
                            .text_size(13.0)
                            .pointer()
                            .label(format!("＋ New {kind}"))
                            .on_press(move |s: &mut State| start_naming(s, &start))
                            .build(t),
                    )
            }
            Some(input) => {
                let empty = input.value().trim().is_empty();
                let creating = ed.creating_agent;
                let (key, submit, keys, press) = (p.key.clone(), p.clone(), p.clone(), p.clone());
                let name = Input::new("new-name", input)
                    .placeholder(format!("Name the new {kind}"))
                    .disabled(creating)
                    .on_edit(move |s: &mut State| {
                        editor(s, &key)
                            .agent_name
                            .get_or_insert_with(InputState::new)
                    })
                    .on_submit(move |s: &mut State, _: &mut Tx| add_new(s, &submit))
                    .on_key(move |s: &mut State, ev: &KeyEvent, _: &mut Tx| menu_key(s, &keys, ev));
                row()
                    .mt(2.0)
                    .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
                    .pt(6.0)
                    .items_center()
                    .gap(6.0)
                    .child(field(t, name, input.is_focused(), FieldSkin::Filter).grow(1.0))
                    .child(
                        Button::new("create-new")
                            .outline()
                            .xsmall()
                            .rounded(ButtonRounded::Px(999.0))
                            .px(8.0)
                            .disabled(creating || empty)
                            .child(
                                text(if creating { "Creating…" } else { "Create" })
                                    .font(ty::ui(11.0))
                                    .color(t.fg),
                            )
                            .on_press(move |s: &mut State| add_new(s, &press))
                            .build(t),
                    )
            }
        });
    }
    let close = p.key.clone();
    col()
        .key("obj-menu")
        .w(ed.objects_width.max(240.0))
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(radius::MENU)
        .box_shadow(pop_shadow())
        .child(
            ScrollArea::vertical("obj-menu-scroll", menu)
                .max_h(318.0)
                .build(t),
        )
        .modal(true, move |s: &mut State| close_object_menu(s, &close))
}

/// Read-only display of the value.
fn read_only(s: &State, t: &Theme, p: &Props, ed: &ValueEditor) -> El {
    let muted_empty = || note(t, "Empty", 13.0);
    match p.format() {
        "status" | "tag" => {
            let (selected, _) = option_selection(p);
            if selected.is_empty() {
                return muted_empty();
            }
            let mut pills = row().flex_wrap().gap(4.0);
            for v in &selected {
                let color = p
                    .rel
                    .options
                    .iter()
                    .find(|o| &o.text == v)
                    .map(|o| o.color.clone())
                    .unwrap_or_default();
                pills = pills.child(TagPill::new(v.clone(), color).build(t));
            }
            pills
        }
        "object" => {
            let store = &s.store;
            let paint = roostr_ui::tags::object_pill_paint(t, t.fg);
            let mut pills = row().flex_wrap().gap(4.0);
            for id in object_ids(p.value.as_ref()) {
                let shown = ed.shown_id(&id);
                let name = store
                    .summary(&shown)
                    .map(|o| o.name.clone())
                    .filter(|n| !n.is_empty())
                    .or_else(|| store.agent(&shown).map(|a| a.name.clone()))
                    .unwrap_or_else(|| prefix(&id, 8));
                pills = pills.child(
                    Tag::custom(paint.bg, paint.fg, paint.border)
                        .rounded_full()
                        .px(8.0)
                        .py(1.0)
                        .text_size(12.0)
                        .child(text(name).font(ty::ui(12.0)).color(t.fg))
                        .build(t),
                );
            }
            pills
        }
        "date" => {
            let num = number_of(p.value.as_ref()).unwrap_or(0.) as i64;
            if num == 0 {
                return muted_empty();
            }
            text(date_label(num)).font(ty::ui(13.0)).color(t.fg)
        }
        _ => {
            let value = stored_text(p);
            if value.is_empty() {
                return muted_empty();
            }
            text(value).font(ty::ui(13.0)).color(t.fg).wrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_as_js_strings() {
        assert_eq!(number_text(3.0), "3");
        assert_eq!(number_text(-12.0), "-12");
        assert_eq!(number_text(2.5), "2.5");
        assert_eq!(parse_js_number(""), Some(0.0));
        assert_eq!(parse_js_number(" 42 "), Some(42.0));
        assert_eq!(parse_js_number("abc"), None);
    }

    #[test]
    fn object_ids_read_links_strings_and_lists() {
        assert!(object_ids(None).is_empty());
        assert_eq!(object_ids(Some(&Value::link("a", "k"))), ["a"]);
        assert_eq!(object_ids(Some(&Value::string("b"))), ["b"]);
        let list = Value::values(vec![Value::link("x", "k"), Value::string("y")]);
        assert_eq!(object_ids(Some(&list)), ["x", "y"]);
    }

    #[test]
    fn status_selection_reads_a_plain_string_too() {
        let p = Props {
            key: "k".into(),
            rel: RelationDef {
                format: "status".into(),
                ..RelationDef::default()
            },
            value: Some(Value::string("Doing")),
            space: String::new(),
            seeds: Fields::new(),
            read_only: false,
            inline_menus: false,
            on_save: None,
        };
        assert_eq!(option_selection(&p), (vec!["Doing".to_string()], false));
    }
}

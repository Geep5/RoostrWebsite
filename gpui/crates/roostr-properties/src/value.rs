//! The one per-format value editor (`PropertyValue.svelte`): shorttext,
//! longtext, number, status, tag, date (calendar picker), checkbox,
//! url/email/phone (typed input + open link), object (link picker with type
//! limits, "+ New agent" / "+ New prompt"), and the `chat_color` hex. Emits
//! complete values through `on_save`; the caller owns persistence.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::{Escape, InputEvent, InputState, MoveDown, MoveUp, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::color::normalize_hex;
use roostr_core::objects::object_icon;
use roostr_core::types::is_system_type;
use roostr_core::ui::{CheckboxIcon, TagPill};
use roostr_core::{Fields, RelationDef, Store, Value, api, theme};

use crate::calendar::{CalendarEvent, CalendarPicker};
use crate::option_picker::{OptionEvent, OptionPicker};
use crate::widgets::{FieldSkin, below, field, pop_shadow, prefix, text_area, tip};

pub(crate) type SaveFn = Rc<dyn Fn(&Value, &mut Window, &mut App)>;

/// One property value: display + inline editor (web PropertyValue.svelte).
#[derive(IntoElement)]
pub struct PropertyValue {
    id: ElementId,
    relation: RelationDef,
    value: Option<Value>,
    space: SharedString,
    seeds: Fields,
    auto_open: bool,
    read_only: bool,
    on_save: Option<SaveFn>,
}

impl PropertyValue {
    /// The editor for `relation`'s `value` (None = not set), keyed by `id`.
    pub fn new(id: impl Into<ElementId>, relation: RelationDef, value: Option<Value>) -> Self {
        Self {
            id: id.into(),
            relation,
            value,
            space: SharedString::default(),
            seeds: Fields::new(),
            auto_open: false,
            read_only: false,
            on_save: None,
        }
    }

    /// Owning space of the object being edited: agent pickers offer only its agents,
    /// type-limited pickers only its objects.
    pub fn space(mut self, space_id: impl Into<SharedString>) -> Self {
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

    /// Called with the complete new value.
    pub fn on_save(mut self, f: impl Fn(&Value, &mut Window, &mut App) + 'static) -> Self {
        self.on_save = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for PropertyValue {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |window, cx| {
            ValueEditor::new(window, cx)
        });
        state.update(cx, |s, cx| s.set_props(self, window, cx));
        div().w_full().min_w_0().child(state)
    }
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

/// A picker candidate.
#[derive(Clone)]
struct Candidate {
    id: String,
    name: String,
    type_key: String,
    icon: String,
    done: Option<bool>,
}

struct ValueEditor {
    rel: RelationDef,
    value: Option<Value>,
    space: String,
    seeds: Fields,
    read_only: bool,
    on_save: Option<SaveFn>,
    mounted: bool,
    /// Single-line text box (text, number, url/email/phone, hex).
    text: Entity<InputState>,
    /// Multi-line box (longtext, error), made on first use.
    area: Option<Entity<TextareaState>>,
    /// The stored text last put in the boxes (an external change replaces an unfocused box).
    synced: Option<String>,
    hex_error: String,
    date_open: bool,
    placeholder: &'static str,
    calendar: Option<Entity<CalendarPicker>>,
    options: Option<Entity<OptionPicker>>,
    object_open: bool,
    object_query: Entity<InputState>,
    /// The highlighted candidate (-1 = none yet).
    object_active: i32,
    /// "+ New agent" naming box (Some while naming).
    agent_name: Option<Entity<InputState>>,
    creating_agent: bool,
    /// `served_by` may hold a machine_id: machine_id → Computer object id.
    machine_objects: HashMap<String, String>,
    machines_asked: bool,
    /// Width of the objects row (the link menu spans it).
    objects_width: Rc<Cell<Pixels>>,
    _subs: Vec<Subscription>,
}

impl ValueEditor {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = cx.new(|cx| InputState::new(window, cx));
        let object_query = cx.new(|cx| InputState::new(window, cx).placeholder("Search objects…"));
        let subs = vec![
            cx.subscribe_in(&text, window, |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.commit_text(window, cx);
                }
            }),
            cx.subscribe_in(
                &object_query,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => {
                        this.object_active = -1;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        let candidates = this.candidates(cx);
                        if let Some(c) = candidates.get(this.object_active.max(0) as usize) {
                            let id = c.id.clone();
                            this.pick_candidate(id, window, cx);
                        }
                    }
                    _ => {}
                },
            ),
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
        ];
        Self {
            rel: RelationDef::default(),
            value: None,
            space: String::new(),
            seeds: Fields::new(),
            read_only: false,
            on_save: None,
            mounted: false,
            text,
            area: None,
            synced: None,
            hex_error: String::new(),
            date_open: false,
            placeholder: "",
            calendar: None,
            options: None,
            object_open: false,
            object_query,
            object_active: -1,
            agent_name: None,
            creating_agent: false,
            machine_objects: HashMap::new(),
            machines_asked: false,
            objects_width: Rc::new(Cell::new(px(240.))),
            _subs: subs,
        }
    }

    fn format(&self) -> &str {
        self.rel.format.as_str()
    }

    fn is_longtext(&self) -> bool {
        self.rel.key != "chat_color" && (self.rel.format == "longtext" || self.rel.key == "error")
    }

    /// The text the box shows for the stored value.
    fn stored_text(&self) -> String {
        match self.format() {
            "number" => number_of(self.value.as_ref())
                .map(number_text)
                .unwrap_or_default(),
            _ => text_of(self.value.as_ref()),
        }
    }

    fn set_props(&mut self, props: PropertyValue, window: &mut Window, cx: &mut Context<Self>) {
        let rel_changed = self.rel.id != props.relation.id || self.rel.key != props.relation.key;
        self.rel = props.relation;
        self.value = props.value;
        self.space = props.space.to_string();
        self.seeds = props.seeds;
        self.read_only = props.read_only;
        self.on_save = props.on_save;
        if rel_changed {
            self.synced = None;
            self.options = None;
        }
        if !self.mounted {
            self.mounted = true;
            if props.auto_open
                && self.format() == "object"
                && object_ids(self.value.as_ref()).is_empty()
                && !self.read_only
            {
                self.object_open = true;
                let q = self.object_query.clone();
                window.defer(cx, move |window, cx| {
                    q.update(cx, |s, cx| s.focus(window, cx))
                });
            }
        }
        if self.rel.key == "served_by" && !self.machines_asked {
            self.machines_asked = true;
            self.load_machines(cx);
        }
        // Keep the text boxes on the stored value unless one is being edited.
        let stored = self.stored_text();
        if self.synced.as_deref() != Some(stored.as_str()) {
            if self.is_longtext() {
                let rows = if self.rel.key == "error" { 5 } else { 3 };
                let area = self.area.get_or_insert_with(|| {
                    let area = cx.new(|cx| {
                        TextareaState::new(window, cx)
                            .auto_grow(rows, 24)
                            .placeholder("Empty")
                    });
                    let sub =
                        cx.subscribe_in(&area, window, |this, _, ev: &InputEvent, window, cx| {
                            if matches!(ev, InputEvent::Blur) {
                                this.commit_text(window, cx);
                            }
                        });
                    self._subs.push(sub);
                    area
                });
                if !area.focus_handle(cx).is_focused(window) {
                    area.update(cx, |s, cx| s.set_value(stored.clone(), window, cx));
                    self.synced = Some(stored);
                }
            } else if !self.text.focus_handle(cx).is_focused(window) {
                self.text
                    .update(cx, |s, cx| s.set_value(stored.clone(), window, cx));
                self.synced = Some(stored);
            }
        }
        let placeholder = match self.format() {
            _ if self.rel.key == "chat_color" => "#RRGGBB",
            "email" => "name@example.com",
            "phone" => "+1 555 0100",
            "url" => "example.com",
            _ => "Empty",
        };
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            self.text
                .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
        if let Some(picker) = &self.options {
            let (selected, multi) = self.option_selection();
            let rel = self.rel.clone();
            picker.update(cx, |p, cx| p.set_props(rel, selected, multi, cx));
        }
    }

    fn save(&self, value: Value, window: &mut Window, cx: &mut App) {
        if let Some(f) = &self.on_save {
            f(&value, window, cx);
        }
    }

    /// `onchange`: the box lost focus or took Enter with a changed value.
    fn commit_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = if self.is_longtext() {
            self.area
                .as_ref()
                .map(|a| a.read(cx).value().to_string())
                .unwrap_or_default()
        } else {
            self.text.read(cx).value().to_string()
        };
        if self.synced.as_deref() == Some(raw.as_str()) {
            return;
        }
        self.synced = Some(raw.clone());
        if self.rel.key == "chat_color" {
            self.save_hex(&raw, window, cx);
            return;
        }
        match self.format() {
            "number" => {
                let Some(n) = parse_js_number(&raw) else {
                    return;
                };
                let v = if n.fract() == 0. && n.abs() < 9.007_199_254_740_992e15 {
                    Value::int(n as i64)
                } else {
                    Value::float(n)
                };
                self.save(v, window, cx);
            }
            "url" | "email" | "phone" => self.save(Value::string(raw.trim()), window, cx),
            _ => self.save(Value::string(raw), window, cx),
        }
    }

    fn save_hex(&mut self, raw: &str, window: &mut Window, cx: &mut Context<Self>) {
        let hex = if raw.trim().is_empty() {
            Some(String::new())
        } else {
            normalize_hex(raw)
        };
        match hex {
            None => self.hex_error = "Use a hex color like #EDCF7F".into(),
            Some(hex) => {
                self.hex_error.clear();
                self.save(Value::string(hex), window, cx);
            }
        }
        cx.notify();
    }

    fn option_selection(&self) -> (Vec<String>, bool) {
        let items = items_of(self.value.as_ref());
        if self.format() == "status" {
            let text = text_of(self.value.as_ref());
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

    fn option_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<OptionPicker> {
        if let Some(p) = &self.options {
            return p.clone();
        }
        let (selected, multi) = self.option_selection();
        let rel = self.rel.clone();
        let picker = cx.new(|cx| OptionPicker::new(rel, selected, multi, window, cx));
        let sub = cx.subscribe_in(&picker, window, |this, _, ev: &OptionEvent, window, cx| {
            let OptionEvent::Pick(t) = ev;
            let (selected, _) = this.option_selection();
            let next: Vec<String> = if this.format() == "status" {
                if selected.contains(t) {
                    vec![]
                } else {
                    vec![t.clone()]
                }
            } else if selected.contains(t) {
                selected.into_iter().filter(|x| x != t).collect()
            } else {
                selected.into_iter().chain([t.clone()]).collect()
            };
            this.save(list_value(&next), window, cx);
        });
        self._subs.push(sub);
        self.options = Some(picker.clone());
        picker
    }

    fn open_calendar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.date_open {
            self.date_open = false;
            self.calendar = None;
            cx.notify();
            return;
        }
        let value = number_of(self.value.as_ref()).unwrap_or(0.) as i64;
        let cal = cx.new(|cx| CalendarPicker::new(value, window, cx));
        let sub = cx.subscribe_in(&cal, window, |this, _, ev: &CalendarEvent, window, cx| {
            this.date_open = false;
            this.calendar = None;
            if let CalendarEvent::Pick(ms) = ev {
                this.save(Value::int(ms.unwrap_or(0)), window, cx);
            }
            cx.notify();
        });
        self._subs.push(sub);
        self.calendar = Some(cal);
        self.date_open = true;
        cx.notify();
    }

    // ── Object links ──────────────────────────────────────────────

    fn load_machines(&mut self, cx: &mut Context<Self>) {
        let task = roostr_core::serving::fetch_machines(cx);
        cx.spawn(async move |this, cx| {
            let Ok((_, machines)) = task.await else {
                return;
            };
            let map = machines
                .into_iter()
                .filter(|m| !m.machine_id.is_empty())
                .map(|m| (m.machine_id, m.id))
                .collect();
            let _ = this.update(cx, |this, cx| {
                this.machine_objects = map;
                cx.notify();
            });
        })
        .detach();
    }

    fn shown_id(&self, id: &str) -> String {
        self.machine_objects
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    /// Type keys the relation is limited to (Anytype relationFormatObjectTypes).
    fn allowed_type_keys(&self, store: &Store) -> Vec<String> {
        self.rel
            .object_types
            .iter()
            .flatten()
            .filter_map(|id| {
                store
                    .types
                    .iter()
                    .find(|t| &t.id == id)
                    .map(|t| t.key.clone())
            })
            .filter(|k| !k.is_empty())
            .collect()
    }

    fn candidates(&self, cx: &App) -> Vec<Candidate> {
        let store = Store::global(cx).read(cx);
        let ids = object_ids(self.value.as_ref());
        let q = self.object_query.read(cx).value().trim().to_lowercase();
        let allowed = self.allowed_type_keys(store);
        let wants_agents = allowed.iter().any(|k| k == "agent");
        let space = self.space.as_str();
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
                .filter(|s| {
                    !ids.iter().any(|id| self.shown_id(id) == s.id)
                        && (!is_system_type(&s.type_key) || allowed.contains(&s.type_key))
                })
                .filter(|s| allowed.is_empty() || allowed.contains(&s.type_key))
                // What an agent links belongs to one space: the picker offers that space's own.
                .filter(|s| space.is_empty() || allowed.is_empty() || s.channel_id == space)
                .map(|s| Candidate {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    type_key: s.type_key.clone(),
                    icon: s.icon.clone(),
                    done: s.done,
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
    fn new_kind(&self, cx: &App) -> &'static str {
        let allowed = self.allowed_type_keys(Store::global(cx).read(cx));
        if allowed.iter().any(|k| k == "agent") {
            "agent"
        } else if allowed.iter().any(|k| k == "system_prompt") {
            "prompt"
        } else {
            ""
        }
    }

    fn toggle_object_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.object_open = !self.object_open;
        self.agent_name = None;
        if self.object_open {
            self.object_query.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    fn close_object_menu(&mut self, cx: &mut Context<Self>) {
        self.object_open = false;
        self.agent_name = None;
        cx.notify();
    }

    fn pick_candidate(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.object_open = false;
        self.toggle_object(&id, window, cx);
    }

    fn toggle_object(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let ids = object_ids(self.value.as_ref());
        // A single-value relation (System prompt, Served by) swaps its link; a list adds.
        let next: Vec<String> = if ids.iter().any(|x| x == id) {
            ids.into_iter().filter(|x| x != id).collect()
        } else if self.rel.max_count == 1 {
            vec![id.to_string()]
        } else {
            ids.into_iter().chain([id.to_string()]).collect()
        };
        self.object_query
            .update(cx, |s, cx| s.set_value("", window, cx));
        let key = self.rel.key.clone();
        let v = Value::values(
            next.into_iter()
                .map(|t| Value::link(t, key.clone()))
                .collect(),
        );
        self.save(v, window, cx);
        cx.notify();
    }

    fn move_active(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.candidates(cx).len() as i32;
        if n == 0 {
            return;
        }
        self.object_active = if down {
            (self.object_active + 1) % n
        } else if self.object_active <= 0 {
            n - 1
        } else {
            self.object_active - 1
        };
        cx.notify();
    }

    fn start_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kind = self.new_kind(cx);
        let seed = self.object_query.read(cx).value().trim().to_string();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(format!("Name the new {kind}"))
                .default_value(seed)
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.add_new(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        self._subs.push(sub);
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.agent_name = Some(input);
        cx.notify();
    }

    /// Make the agent / prompt here and link it.
    fn add_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.agent_name else {
            return;
        };
        let name = input.read(cx).value().trim().to_string();
        if self.creating_agent || name.is_empty() {
            return;
        }
        self.creating_agent = true;
        input.update(cx, |s, cx| s.set_disabled(true, cx));
        let store = Store::global(cx).read(cx);
        let channel = if self.space.is_empty() {
            store
                .channels
                .first()
                .map(|s| s.id.clone())
                .unwrap_or_default()
        } else {
            self.space.clone()
        };
        let task = if self.new_kind(cx) == "agent" {
            roostr_core::create::new_agent(&channel, &name, self.seeds.clone(), cx)
        } else {
            let fields = (!channel.is_empty())
                .then(|| Fields::from([("channel".to_string(), Value::string(channel.clone()))]));
            api::note::create(
                &name,
                Some("system_prompt"),
                Some(fields.unwrap_or_default()),
                cx,
            )
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.creating_agent = false;
                match result {
                    Ok(id) => {
                        this.object_open = false;
                        this.agent_name = None;
                        this.toggle_object(&id, window, cx);
                    }
                    Err(error) => {
                        log::error!("create: {error:#}");
                        if let Some(input) = &this.agent_name {
                            input.update(cx, |s, cx| s.set_disabled(false, cx));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_object(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let store = Store::global(cx).read(cx);
        let ids = object_ids(self.value.as_ref());
        let read_only = self.read_only;
        let mut row = div()
            .relative()
            .flex()
            .flex_wrap()
            .gap(px(4.))
            .items_center();
        for id in &ids {
            let shown = self.shown_id(id);
            let summary = store.summary(&shown);
            let agent = store.agent(id);
            let name = summary
                .map(|s| s.name.clone())
                .filter(|n| !n.is_empty())
                .or_else(|| {
                    store
                        .agent(&shown)
                        .map(|a| a.name.clone())
                        .filter(|n| !n.is_empty())
                })
                .unwrap_or_else(|| prefix(id, 8));
            let lead: AnyElement =
                if agent.is_some() || summary.is_some_and(|s| s.type_key == "agent") {
                    let icon = agent
                        .map(|a| a.icon.clone())
                        .filter(|i| !i.is_empty())
                        .or_else(|| summary.map(|s| s.icon.clone()).filter(|i| !i.is_empty()));
                    div()
                        .child(icon.unwrap_or_else(|| "🤖".into()))
                        .into_any_element()
                } else if let Some(s) = summary.filter(|s| store.layout_of(&s.type_key) == "task") {
                    checkbox_icon(s.done == Some(true), 14.).into_any_element()
                } else {
                    let (icon, tk) = summary
                        .map(|s| (s.icon.as_str(), s.type_key.as_str()))
                        .unwrap_or(("", "note"));
                    div()
                        .child(object_icon(store, icon, tk, &self.space))
                        .into_any_element()
                };
            let target = shown.clone();
            let remove = id.clone();
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .border_1()
                    .border_color(theme::border())
                    .rounded_full()
                    .pl(px(8.))
                    .pr(px(4.))
                    .py(px(1.))
                    .text_size(px(12.))
                    .child(
                        div()
                            .id(SharedString::from(format!("chip-{id}")))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme::accent()))
                            .on_click(move |_, _, cx| {
                                roostr_core::nav::navigate(
                                    roostr_core::Route::Object(target.clone()),
                                    cx,
                                )
                            })
                            .child(lead)
                            .child(name),
                    )
                    .when(!read_only, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("rm-{id}")))
                                .px(px(4.))
                                .text_size(px(11.))
                                .text_color(theme::muted())
                                .cursor_pointer()
                                .tooltip(tip("Remove"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.toggle_object(&remove, window, cx)
                                }))
                                .child("×"),
                        )
                    }),
            );
        }
        let width = self.objects_width.clone();
        row = row.child(
            canvas(
                move |bounds, _, _| width.set(bounds.size.width),
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
        if read_only {
            return row.into_any_element();
        }
        row = row.child(
            div()
                .id("link-object")
                .border_1()
                .border_color(theme::border())
                .rounded_full()
                .px(px(8.))
                .py(px(1.))
                .text_size(px(11.))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.toggle_object_menu(window, cx)))
                .child("+ link object"),
        );
        if self.object_open {
            let menu = self.render_object_menu(window, cx);
            row = row.child(below("object-menu", 6., menu));
        }
        row.into_any_element()
    }

    fn render_object_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let candidates = self.candidates(cx);
        let kind = self.new_kind(cx);
        let store = Store::global(cx).read(cx);
        let mut menu = div()
            .id("obj-menu")
            .w(self.objects_width.get().max(px(240.)))
            .max_h(px(320.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::menu())
            .p(px(8.))
            .shadow(pop_shadow())
            .text_color(theme::fg())
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_object_menu(cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                this.move_active(true, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                this.move_active(false, cx);
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                cx.stop_propagation();
                if this.agent_name.is_some() {
                    this.agent_name = None;
                    cx.notify();
                } else {
                    this.close_object_menu(cx);
                }
            }))
            .child(div().mb(px(4.)).child(field(
                &self.object_query,
                FieldSkin::Filter,
                window,
                cx,
            )));
        for (i, c) in candidates.iter().enumerate() {
            let active = i as i32 == self.object_active;
            let lead: AnyElement = if c.type_key == "agent" {
                let icon = if !c.icon.is_empty() {
                    c.icon.clone()
                } else {
                    store
                        .agent(&c.id)
                        .map(|a| a.icon.clone())
                        .filter(|i| !i.is_empty())
                        .unwrap_or_else(|| "🤖".into())
                };
                div().child(icon).into_any_element()
            } else if store.layout_of(&c.type_key) == "task" {
                checkbox_icon(c.done == Some(true), 15.).into_any_element()
            } else {
                div()
                    .child(object_icon(store, &c.icon, &c.type_key, &self.space))
                    .into_any_element()
            };
            let id = c.id.clone();
            menu = menu.child(
                div()
                    .id(SharedString::from(format!("cand-{}", c.id)))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(6.))
                    .py(px(5.))
                    .rounded(theme::radius::sm())
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme::hover()))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.object_active != i as i32 {
                            this.object_active = i as i32;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_candidate(id.clone(), window, cx)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(lead)
                            .child(div().min_w_0().overflow_hidden().text_ellipsis().child(
                                if c.name.is_empty() {
                                    "Untitled".to_string()
                                } else {
                                    c.name.clone()
                                },
                            )),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(c.type_key.clone()),
                    ),
            );
        }
        if candidates.is_empty() {
            menu = menu.child(
                div()
                    .px(px(6.))
                    .py(px(4.))
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child("No matches"),
            );
        }
        if !kind.is_empty() {
            match &self.agent_name {
                None => {
                    menu = menu.child(
                        div()
                            .id("new-kind")
                            .mt(px(2.))
                            .border_t_1()
                            .border_color(theme::border())
                            .px(px(6.))
                            .py(px(5.))
                            .rounded(theme::radius::sm())
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover()))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.start_naming(window, cx)),
                            )
                            .child(format!("＋ New {kind}")),
                    );
                }
                Some(input) => {
                    let empty = input.read(cx).value().trim().is_empty();
                    let creating = self.creating_agent;
                    menu = menu.child(
                        div()
                            .mt(px(2.))
                            .border_t_1()
                            .border_color(theme::border())
                            .pt(px(6.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(field(input, FieldSkin::Filter, window, cx).flex_1())
                            .child(
                                div()
                                    .id("create-new")
                                    .flex_none()
                                    .border_1()
                                    .border_color(theme::border())
                                    .rounded_full()
                                    .px(px(8.))
                                    .py(px(1.))
                                    .text_size(px(11.))
                                    .when(creating || empty, |d| d.opacity(0.5))
                                    .when(!(creating || empty), |d| d.cursor_pointer())
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.add_new(window, cx)),
                                    )
                                    .child(if creating { "Creating…" } else { "Create" }),
                            ),
                    );
                }
            }
        }
        menu.into_any_element()
    }
}

/// `Number(raw)` for a typed number ("" is not a value here: onchange of an emptied box is ignored).
fn parse_js_number(raw: &str) -> Option<f64> {
    let t = raw.trim();
    if t.is_empty() {
        return Some(0.);
    }
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// Anytype's checkbox glyph (`CheckboxIcon`): muted unchecked, fg checked.
pub(crate) fn checkbox_icon(checked: bool, size: f32) -> CheckboxIcon {
    CheckboxIcon::new(checked).size(px(size)).color(if checked {
        theme::fg()
    } else {
        theme::muted()
    })
}

impl Render for ValueEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let format = self.rel.format.clone();
        let read_only = self.read_only;
        if format == "repeat" {
            // Owned by Repeat on the object page; this only guards a stray mount.
            return div().into_any_element();
        }
        if format == "checkbox" {
            let checked = self.value.as_ref().and_then(|v| v.bool_value) == Some(true);
            return div()
                .id("chk")
                .flex()
                .text_color(if checked { theme::fg() } else { theme::muted() })
                .when(!read_only, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.text_color(theme::fg()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.save(Value::bool(!checked), window, cx)
                        }))
                })
                .child(checkbox_icon(checked, 20.))
                .into_any_element();
        }
        if read_only {
            return self.render_read_only(cx);
        }
        match format.as_str() {
            "date" => {
                let num = number_of(self.value.as_ref()).unwrap_or(0.) as i64;
                let label = if num != 0 {
                    date_label(num)
                } else {
                    "Pick a date".into()
                };
                div()
                    .relative()
                    .flex()
                    .child(
                        div()
                            .id("date-btn")
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::bg())
                            .rounded(theme::radius::sm())
                            .px(px(10.))
                            .py(px(5.))
                            .text_size(px(13.))
                            .text_color(if num == 0 {
                                theme::muted()
                            } else {
                                theme::fg()
                            })
                            .cursor_pointer()
                            .hover(|s| s.border_color(theme::accent()))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_calendar(window, cx)),
                            )
                            .child(label),
                    )
                    .when_some(
                        self.calendar.clone().filter(|_| self.date_open),
                        |d, cal| {
                            d.child(below(
                                "date-calendar",
                                6.,
                                div()
                                    .occlude()
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.date_open = false;
                                        this.calendar = None;
                                        cx.notify();
                                    }))
                                    .child(cal),
                            ))
                        },
                    )
                    .into_any_element()
            }
            "status" | "tag" => {
                let picker = self.option_picker(window, cx);
                div().w_full().child(picker).into_any_element()
            }
            "url" | "email" | "phone" => {
                let text = text_of(self.value.as_ref());
                let href = match format.as_str() {
                    "email" => format!("mailto:{text}"),
                    "phone" => format!("tel:{text}"),
                    _ if text.starts_with("http://") || text.starts_with("https://") => {
                        text.clone()
                    }
                    _ => format!("https://{text}"),
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .w_full()
                    .child(field(&self.text, FieldSkin::Value, window, cx).flex_1())
                    .when(!text.is_empty(), |d| {
                        d.child(
                            div()
                                .id("open")
                                .flex_none()
                                .text_size(px(14.))
                                .text_color(theme::accent())
                                .cursor_pointer()
                                .tooltip(tip("Open"))
                                .on_click(move |_, _, cx| cx.open_url(&href))
                                .child("↗"),
                        )
                    })
                    .into_any_element()
            }
            "object" => self.render_object(window, cx),
            _ if self.rel.key == "chat_color" => {
                let text = text_of(self.value.as_ref());
                let swatch = normalize_hex(&text).unwrap_or_else(|| "#3A3A3E".into());
                let bad = !self.hex_error.is_empty();
                let error = self.hex_error.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .w_full()
                    .child(
                        div()
                            .flex_none()
                            .w(px(26.))
                            .h(px(22.))
                            .border_1()
                            .border_color(theme::border())
                            .rounded(theme::radius::xs())
                            .p(px(2.))
                            .child(
                                div()
                                    .size_full()
                                    .rounded(px(2.))
                                    .bg(roostr_core::color::hex(&swatch).unwrap_or(theme::hover())),
                            ),
                    )
                    .child(
                        div()
                            .id("hex")
                            .flex_1()
                            .when(bad, |d| d.tooltip(tip(error)))
                            .child(
                                field(&self.text, FieldSkin::Value, window, cx)
                                    .when(bad, |d| d.border_color(rgb(0xe5484d))),
                            ),
                    )
                    .into_any_element()
            }
            _ if self.is_longtext() => match &self.area {
                Some(area) => {
                    text_area(area, window, cx, theme::bg(), theme::radius::sm(), (5., 8.))
                        .into_any_element()
                }
                None => div().into_any_element(),
            },
            _ => field(&self.text, FieldSkin::Value, window, cx)
                .w_full()
                .into_any_element(),
        }
    }
}

impl ValueEditor {
    /// Read-only display of the value.
    fn render_read_only(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let format = self.rel.format.as_str();
        let muted_empty = || {
            div()
                .text_size(px(13.))
                .text_color(theme::muted())
                .child("Empty")
                .into_any_element()
        };
        match format {
            "status" | "tag" => {
                let (selected, _) = self.option_selection();
                if selected.is_empty() {
                    return muted_empty();
                }
                let rel = self.rel.clone();
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .children(selected.iter().map(|t| {
                        let color = rel
                            .options
                            .iter()
                            .find(|o| &o.text == t)
                            .map(|o| o.color.clone())
                            .unwrap_or_default();
                        TagPill::new(t.clone(), color)
                    }))
                    .into_any_element()
            }
            "object" => {
                let mut window_less = self.render_object_static(cx);
                window_less = window_less.text_size(px(12.));
                window_less.into_any_element()
            }
            "date" => {
                let num = number_of(self.value.as_ref()).unwrap_or(0.) as i64;
                if num == 0 {
                    return muted_empty();
                }
                div()
                    .text_size(px(13.))
                    .child(date_label(num))
                    .into_any_element()
            }
            _ => {
                let text = self.stored_text();
                if text.is_empty() {
                    return muted_empty();
                }
                div().text_size(px(13.)).child(text).into_any_element()
            }
        }
    }

    fn render_object_static(&self, cx: &App) -> Div {
        let store = Store::global(cx).read(cx);
        let ids = object_ids(self.value.as_ref());
        div()
            .flex()
            .flex_wrap()
            .gap(px(4.))
            .children(ids.iter().map(|id| {
                let shown = self.shown_id(id);
                let name = store
                    .summary(&shown)
                    .map(|s| s.name.clone())
                    .filter(|n| !n.is_empty())
                    .or_else(|| store.agent(&shown).map(|a| a.name.clone()))
                    .unwrap_or_else(|| prefix(id, 8));
                div()
                    .border_1()
                    .border_color(theme::border())
                    .rounded_full()
                    .px(px(8.))
                    .py(px(1.))
                    .child(name)
            }))
    }
}

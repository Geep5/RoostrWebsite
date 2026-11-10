//! Anytype's full add-property flow as one cascading popover
//! (`PropertyFlow.svelte`): 1. the current properties with visibility
//! switches and "Add property"; 2. filter / pick an existing property or a
//! format to create; 3. Name + Type (custom format submenu) + Create.
//!
//! The host keeps the flow's state while it is open and places the panel; the
//! panel pushes its own outside-press scrim into `layer`.

use std::rc::{Rc, Weak};

use roostr_core::relations::{create_relation, is_reserved_key, space_relations};
use roostr_core::{RelationDef, State};
use roostr_ui::overlay::dismissable;
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    Button, Glyph, Input, InputState, ListItem, Switch, Theme, color, disabled as ui_disabled,
    glyph, motion, prop_icon, radius, ty,
};
use rune::{Align, Cx, El, Key, KeyEvent, ScrollSpec, Side, Tx, col, el, row, text};

use crate::credentials::{COOKIE_META, is_secret_field};
use crate::type_limit::{toggled, type_limit_picker};
use crate::widgets::{FieldSkin, Lens, empty_note, field, lens, pop_shadow, rule};

/// A stage-1 row: a current/available property and its toggle state.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowItem {
    pub key: String,
    pub name: String,
    pub format: String,
    pub on: bool,
    /// The property's own emoji; None = its format glyph.
    pub emoji: Option<String>,
}

/// What the flow reports to its host.
pub enum FlowEvent {
    /// A stage-1 switch flipped.
    Toggle(String, bool),
    /// A property picked or created in stages 2-3 (the flow closes after).
    Add(RelationDef),
    /// Escape from the list, a press outside, or done.
    Close,
}

type EventFn = Rc<dyn Fn(&mut State, FlowEvent)>;

/// Anytype getRelationTypes order, mapped to our formats.
const FORMATS: &[(&str, &str)] = &[
    ("object", "Object"),
    ("longtext", "Text"),
    ("number", "Number"),
    ("status", "Select"),
    ("tag", "Multi-select"),
    ("date", "Date"),
    ("checkbox", "Checkbox"),
    ("url", "URL"),
    ("email", "Email"),
    ("phone", "Phone"),
    ("shorttext", "Short text"),
];

fn format_name(format: &str) -> String {
    FORMATS
        .iter()
        .find(|f| f.0 == format)
        .map_or_else(|| format.to_string(), |f| f.1.to_string())
}

const SYSTEM_KEYS: &[&str] = &[
    "createdDate",
    "modifiedDate",
    "dueDate",
    "done",
    "tag",
    "status",
    "url",
    "email",
    "phone",
    "description",
    "error",
    "account",
    "service",
    "login_url",
    "session_host",
    "session_cookie",
    COOKIE_META,
];

fn is_system(key: &str) -> bool {
    SYSTEM_KEYS.contains(&key) || is_secret_field(key)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    List,
    Suggest,
    Edit,
}

/// The add-property flow's state, kept by the host while the flow is open.
#[derive(Clone)]
pub struct PropertyFlowState {
    items: Vec<FlowItem>,
    exclude: Vec<String>,
    space_id: String,
    stage: Stage,
    query: InputState,
    name: InputState,
    edit_format: Option<String>,
    format_menu: bool,
    edit_types: Vec<String>,
    /// Dropped with the host's state: a create answering after the flow closed stays quiet.
    alive: Rc<()>,
}

impl PropertyFlowState {
    /// `items`: stage-1 rows; `exclude`: keys this surface covers another way.
    pub fn new(items: Vec<FlowItem>, exclude: Vec<String>, space_id: String) -> Self {
        Self {
            items,
            exclude,
            space_id,
            stage: Stage::List,
            query: InputState::new(),
            name: InputState::new(),
            edit_format: None,
            format_menu: false,
            edit_types: vec![],
            alive: Rc::new(()),
        }
    }

    /// The host's rows changed (a switch was applied).
    pub fn set_items(&mut self, items: Vec<FlowItem>) {
        if self.items != items {
            self.items = items;
        }
    }

    /// (my properties, system properties, can create) for the current filter.
    fn sections(&self, relations: &[RelationDef]) -> (Vec<RelationDef>, Vec<RelationDef>, bool) {
        let q = self.query.value().trim().to_lowercase();
        let label = |r: &RelationDef| {
            if r.name.is_empty() {
                r.key.clone()
            } else {
                r.name.clone()
            }
        };
        let all: Vec<RelationDef> = space_relations(relations, &self.space_id)
            .into_iter()
            .filter(|r| {
                !r.hidden
                    && !is_reserved_key(&r.key)
                    && !self.items.iter().any(|i| i.key == r.key)
                    && !self.exclude.contains(&r.key)
            })
            .cloned()
            .collect();
        let exact = all.iter().any(|r| label(r).to_lowercase() == q);
        let matching: Vec<RelationDef> = all
            .into_iter()
            .filter(|r| q.is_empty() || label(r).to_lowercase().contains(&q))
            .collect();
        let (system, library): (Vec<_>, Vec<_>) =
            matching.into_iter().partition(|r| is_system(&r.key));
        (library, system, !q.is_empty() && !exact)
    }

    fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
        match stage {
            Stage::Suggest => self.query.focus(),
            Stage::Edit => self.name.focus(),
            Stage::List => {}
        }
    }

    fn open_edit(&mut self, name: String, format: Option<&str>) {
        self.name.set_value(name);
        self.edit_format = format.map(str::to_string);
        self.format_menu = false;
        self.set_stage(Stage::Edit);
    }
}

/// The flow's handlers: its state and the host's event sink.
#[derive(Clone)]
struct Hooks {
    lens: Lens<PropertyFlowState>,
    on_event: Option<EventFn>,
}

impl Hooks {
    fn emit(&self, s: &mut State, ev: FlowEvent) {
        if let Some(f) = &self.on_event {
            f(s, ev);
        }
    }

    fn add(&self, s: &mut State, rel: RelationDef) {
        self.emit(s, FlowEvent::Add(rel));
        self.emit(s, FlowEvent::Close);
    }

    fn escape(&self, s: &mut State) {
        let st = (self.lens)(s);
        match st.stage {
            Stage::Edit => st.set_stage(Stage::Suggest),
            Stage::Suggest => st.set_stage(Stage::List),
            Stage::List => self.emit(s, FlowEvent::Close),
        }
    }

    /// Enter in the filter: the first of my properties, else create the query.
    fn enter_query(&self, s: &mut State) {
        let st = (self.lens)(s).clone();
        let (library, _, can_create) = st.sections(&s.store.relations);
        if let Some(first) = library.into_iter().next() {
            self.add(s, first);
        } else if can_create {
            let q = st.query.value().trim().to_string();
            (self.lens)(s).open_edit(q, None);
        }
    }

    fn submit(&self, s: &mut State) {
        let st = (self.lens)(s);
        let name = st.name.value().trim().to_string();
        let Some(format) = st.edit_format.clone() else {
            return;
        };
        if name.is_empty() {
            return;
        }
        let (types, space) = (st.edit_types.clone(), st.space_id.clone());
        let alive: Weak<()> = Rc::downgrade(&st.alive);
        let task = create_relation(&name, &format, &types, &space, s);
        let hooks = self.clone();
        s.rt.spawn(async move |cx| match task.await {
            Ok(rel) => {
                if alive.upgrade().is_some() {
                    cx.update(|s| hooks.add(s, rel));
                }
            }
            Err(error) => log::error!("create property: {error:#}"),
        })
        .detach();
    }

    /// Escape in a field steps back; the field keeps every other key.
    fn field_key(&self) -> impl Fn(&mut State, &KeyEvent, &mut Tx) -> bool + 'static {
        let h = self.clone();
        move |s: &mut State, ev: &KeyEvent, _: &mut Tx| {
            if ev.key == Key::Escape {
                h.escape(s);
                true
            } else {
                false
            }
        }
    }
}

/// A flow list row (kit `ListItem`): 13px, 8px glyph gap, list hover.
fn flow_row(t: &Theme, key: impl Into<String>) -> ListItem {
    ListItem::new(key)
        .px(8.0)
        .py(5.0)
        .radius(radius::SM)
        .text_size(13.0)
        .text_color(t.fg)
}

/// A row's content: the 18px glyph column, name, then any trailing marks.
fn line() -> El {
    row().center_y().gap(8.0).grow(1.0).shrink().min_w(0.0)
}

/// The 18px glyph column: the property's emoji, else its format's line icon.
fn lead(t: &Theme, emoji: Option<String>, format: Option<&str>) -> El {
    let col_ = el().w(18.0).center();
    match emoji {
        Some(e) => col_.child(text(e).font(ty::ui(13.0)).color(t.fg)),
        None => col_.child(glyph(format.map_or(Glyph::PropText, prop_icon), 14.0).color(t.muted)),
    }
}

fn r_name(t: &Theme, name: impl Into<String>) -> El {
    text(name.into())
        .font(ty::ui(13.0))
        .color(t.fg)
        .truncate()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
}

fn chevron(t: &Theme) -> El {
    glyph(Glyph::ChevronRight, 14.0).color(t.muted)
}

fn sep(t: &Theme) -> El {
    el().w_full().py(4.0).child(rule(t))
}

fn section(t: &Theme, label: &'static str) -> El {
    el().px(8.0)
        .pt(4.0)
        .pb(2.0)
        .child(text(label).font(ty::ui(11.0)).color(t.muted))
}

/// Anytype's switch as the kit `Switch` (small: 28×16 track, 12px knob); the
/// track recolours to the accent over Rune's colour motion.
fn switch(t: &Theme, key: &str, on: bool) -> Switch {
    Switch::new(format!("switch-{key}"))
        .checked(on)
        .small()
        .tab_stop(false)
        .color(t.accent)
}

/// The cascading add-property popover (table "+" column).
pub struct PropertyFlow {
    key: String,
    state: PropertyFlowState,
    lens: Option<Lens<PropertyFlowState>>,
    on_event: Option<EventFn>,
}

impl PropertyFlow {
    pub fn new(key: impl Into<String>, state: &PropertyFlowState) -> Self {
        Self {
            key: key.into(),
            state: state.clone(),
            lens: None,
            on_event: None,
        }
    }

    /// Where the host keeps this flow's state.
    pub fn on_state(
        mut self,
        f: impl for<'a> Fn(&'a mut State) -> &'a mut PropertyFlowState + 'static,
    ) -> Self {
        self.lens = Some(lens(f));
        self
    }

    /// Toggle / Add (then Close) / Close; the host closes the flow on Close.
    pub fn on_event(mut self, f: impl Fn(&mut State, FlowEvent) + 'static) -> Self {
        self.on_event = Some(Rc::new(f));
        self
    }

    pub fn build(self, s: &State, _cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) -> El {
        let key = self.key.clone();
        let st = &self.state;
        let hooks = self.lens.clone().map(|lens| Hooks {
            lens,
            on_event: self.on_event.clone(),
        });
        // Every handler goes through the host's lens; without one the flow only draws.
        let on = |e: El, f: Box<dyn Fn(&Hooks, &mut State)>| match &hooks {
            Some(h) => {
                let h = h.clone();
                e.on_press(move |s: &mut State| f(&h, s))
            }
            None => e,
        };
        let mut flow = col()
            .key(key.clone())
            .w(300.0)
            .max_h(420.0)
            .min_h(0.0)
            .gap(4.0)
            .fill(t.panel)
            .border(1.0, t.border)
            .radius(radius::MENU)
            .padding(8.0)
            .box_shadow(pop_shadow())
            .items_stretch();
        let mut menu = None;
        match st.stage {
            Stage::List => {
                let mut list = col()
                    .key("flow-list")
                    .gap(1.0)
                    .min_h(0.0)
                    .shrink()
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .items_stretch();
                let mut rows = col().gap(1.0).items_stretch();
                for it in &st.items {
                    let mut sw = switch(t, &it.key, it.on);
                    if let Some(h) = &hooks {
                        let (h, k) = (h.clone(), it.key.clone());
                        sw = sw.on_change(move |s: &mut State, next: bool| {
                            h.emit(s, FlowEvent::Toggle(k.clone(), next))
                        });
                    }
                    rows = rows.child(
                        flow_row(t, format!("item-{}", it.key))
                            .child(
                                line()
                                    .child(lead(t, it.emoji.clone(), Some(&it.format)))
                                    .child(r_name(t, it.name.clone()))
                                    .child(sw.build(t)),
                            )
                            .build(t),
                    );
                }
                if st.items.is_empty() {
                    rows = rows.child(el().px(8.0).py(4.0).child(empty_note(
                        t,
                        "No properties yet.",
                        12.0,
                    )));
                }
                list = list.child(rows);
                let mut add = Button::new("add-row")
                    .ghost()
                    .h_auto()
                    .px(8.0)
                    .py(6.0)
                    .radius(radius::SM)
                    .text_color(t.fg)
                    .child(
                        line()
                            .child(glyph(Glyph::Plus, 14.0).color(t.fg))
                            .child(text("Add property").font(ty::ui(13.0)).color(t.fg)),
                    );
                if let Some(h) = &hooks {
                    let h = h.clone();
                    add = add.on_press(move |s: &mut State| (h.lens)(s).set_stage(Stage::Suggest));
                }
                flow = flow.child(list).child(sep(t)).child(add.build(t));
            }
            Stage::Suggest => {
                let (library, system, can_create) = st.sections(&s.store.relations);
                let q = st.query.value().trim().to_string();
                let ql = q.to_lowercase();
                let formats: Vec<_> = FORMATS
                    .iter()
                    .filter(|f| ql.is_empty() || f.1.to_lowercase().contains(&ql))
                    .collect();
                let mut input =
                    Input::new("flow-query", &st.query).placeholder("Filter or create…");
                if let Some(h) = &hooks {
                    let (l, h1) = (h.lens.clone(), h.clone());
                    input = input
                        .on_edit(move |s: &mut State| &mut l(s).query)
                        .on_submit(move |s: &mut State, _: &mut Tx| h1.enter_query(s))
                        .on_key(h.field_key());
                }
                flow = flow.child(field(t, input, st.query.is_focused(), FieldSkin::Filter));
                let mut scroll = col()
                    .key("flow-scroll")
                    .gap(1.0)
                    .min_h(0.0)
                    .shrink()
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .items_stretch();
                let rel_row = |rel: &RelationDef| {
                    let r = rel.clone();
                    let emoji = rel.icon_emoji.clone().filter(|e| !e.is_empty());
                    let name = if rel.name.is_empty() {
                        rel.key.clone()
                    } else {
                        rel.name.clone()
                    };
                    let item = flow_row(t, format!("rel-{}", rel.key))
                        .child(
                            line()
                                .child(lead(t, emoji, Some(&rel.format)))
                                .child(r_name(t, name)),
                        )
                        .build(t);
                    on(item, Box::new(move |h, s| h.add(s, r.clone())))
                };
                if !library.is_empty() {
                    scroll = scroll.child(section(t, "My properties"));
                    for rel in &library {
                        scroll = scroll.child(rel_row(rel));
                    }
                }
                if !system.is_empty() {
                    if !library.is_empty() {
                        scroll = scroll.child(sep(t));
                    }
                    scroll = scroll.child(section(t, "System properties"));
                    for rel in &system {
                        scroll = scroll.child(rel_row(rel));
                    }
                }
                if can_create || !formats.is_empty() {
                    if !library.is_empty() || !system.is_empty() {
                        scroll = scroll.child(sep(t));
                    }
                    scroll = scroll.child(section(t, "Create new"));
                    if can_create {
                        let name = q.clone();
                        let item = flow_row(t, "create")
                            .text_color(t.accent)
                            .label(format!("＋ Create property “{q}”"))
                            .build(t);
                        scroll = scroll.child(on(
                            item,
                            Box::new(move |h, s| (h.lens)(s).open_edit(name.clone(), None)),
                        ));
                    }
                    for f in formats {
                        let name = q.clone();
                        let id = f.0;
                        let item = flow_row(t, format!("fmt-{id}"))
                            .child(
                                line()
                                    .child(lead(t, None, Some(id)))
                                    .child(r_name(t, f.1))
                                    .child(chevron(t)),
                            )
                            .build(t);
                        scroll = scroll.child(on(
                            item,
                            Box::new(move |h, s| (h.lens)(s).open_edit(name.clone(), Some(id))),
                        ));
                    }
                }
                flow = flow.child(scroll);
            }
            Stage::Edit => {
                let fmt = st.edit_format.clone();
                let label = |l: &'static str| {
                    el().px(2.0)
                        .pt(2.0)
                        .child(text(l).font(ty::ui(11.0)).color(t.muted))
                };
                // Its own key is what the menu anchors to (an anchor names an
                // element's own key, never a path), so it carries the flow's.
                let type_row = Button::new(format!("{key}-type-row"))
                    .outline()
                    .h(30.0)
                    .px(8.0)
                    .radius(radius::MD)
                    .border_color(t.border)
                    .text_color(t.fg)
                    .child(
                        line()
                            .child(lead(t, None, fmt.as_deref()))
                            .child(r_name(
                                t,
                                fmt.as_deref().map_or_else(
                                    || "Select property type".to_string(),
                                    format_name,
                                ),
                            ))
                            .child(chevron(t)),
                    );
                let type_row = match &hooks {
                    Some(h) => {
                        let h = h.clone();
                        type_row.on_press(move |s: &mut State| {
                            let st = (h.lens)(s);
                            st.format_menu = !st.format_menu;
                        })
                    }
                    None => type_row,
                };
                if st.format_menu {
                    let mut m = col()
                        .key("type-menu")
                        .w(282.0)
                        .max_h(240.0)
                        .min_h(0.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .fill(t.panel)
                        .border(1.0, t.border)
                        .radius(radius::MENU)
                        .padding(4.0)
                        .box_shadow(pop_shadow())
                        .items_stretch();
                    let mut rows = col().items_stretch();
                    for f in FORMATS {
                        let active = fmt.as_deref() == Some(f.0);
                        let id = f.0;
                        let item = flow_row(t, format!("menu-{id}"))
                            .check_icon(Glyph::Check)
                            .confirmed(active)
                            .child(line().child(lead(t, None, Some(id))).child(r_name(t, f.1)))
                            .build(t);
                        let item = if active { item.fill(t.hl_med) } else { item };
                        rows = rows.child(on(
                            item,
                            Box::new(move |h, s| {
                                let st = (h.lens)(s);
                                st.edit_format = Some(id.to_string());
                                st.format_menu = false;
                            }),
                        ));
                    }
                    m = m.child(rows);
                    // Hangs under the Type button, above the rest of the form (the
                    // flow's last child); presses on it stay inside the flow.
                    menu = Some(
                        m.anchored(format!("{key}-type-row"), Side::Bottom, Align::Start, 4.0)
                            .transition(motion::appear(-4.0))
                            .instant_changes(),
                    );
                }
                let disabled = st.name.value().trim().is_empty() || fmt.is_none();
                let mut name = Input::new("flow-name", &st.name).placeholder("Property name");
                if let Some(h) = &hooks {
                    let (l, h1) = (h.lens.clone(), h.clone());
                    name = name
                        .on_edit(move |s: &mut State| &mut l(s).name)
                        .on_submit(move |s: &mut State, _: &mut Tx| h1.submit(s))
                        .on_key(h.field_key());
                }
                let mut form = col()
                    .gap(6.0)
                    .items_stretch()
                    .child(label("Name"))
                    .child(field(t, name, st.name.is_focused(), FieldSkin::Filter))
                    .child(label("Type"))
                    .child(type_row.build(t));
                if fmt.as_deref() == Some("object") {
                    let toggle: Rc<dyn Fn(&mut State, &str)> = match &hooks {
                        Some(h) => {
                            let l = h.lens.clone();
                            Rc::new(move |s: &mut State, id: &str| {
                                let st = l(s);
                                st.edit_types = toggled(&st.edit_types, id);
                            })
                        }
                        None => Rc::new(|_: &mut State, _: &str| {}),
                    };
                    form = form.child(type_limit_picker(
                        s,
                        t,
                        "type-limit",
                        &st.edit_types,
                        &st.space_id,
                        toggle,
                    ));
                }
                let mut submit = Button::new("submit")
                    .primary()
                    .accessibility_label("Create")
                    .h(32.0)
                    .radius(radius::MD)
                    .pointer()
                    .child(
                        text("Create")
                            .font(ty::ui(13.0).semibold())
                            .color(color::rgb(0x14161a)),
                    );
                if let Some(h) = &hooks {
                    let h = h.clone();
                    submit = submit.on_press(move |s: &mut State| h.submit(s));
                }
                let submit = ui_disabled(submit, disabled).build(t);
                let submit = if disabled {
                    submit.opacity(0.4)
                } else {
                    submit
                };
                form = form.child(col().items_stretch().pt(4.0).child(submit));
                flow = flow.child(form);
            }
        }
        if let Some(m) = menu {
            flow = flow.child(m);
        }
        // The popover drops in as it opens; it takes keys from its first frame.
        let flow = flow.transition(motion::appear(-4.0));
        let close = hooks.clone();
        let flow = dismissable(
            flow,
            &key,
            move |s: &mut State| {
                if let Some(h) = &close {
                    h.emit(s, FlowEvent::Close)
                }
            },
            layer,
        );
        // Escape with no field focused (the list): the modal's close above; in a field the
        // field's own key handler steps back first.
        flow
    }
}

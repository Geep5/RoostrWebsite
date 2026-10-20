//! Anytype's full add-property flow as one cascading popover
//! (`PropertyFlow.svelte`): 1. the current properties with visibility
//! switches and "Add property"; 2. filter / pick an existing property or a
//! format to create; 3. Name + Type (custom format submenu) + Create.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::input::{Escape, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::relations::{create_relation, is_reserved_key, space_relations};
use roostr_core::{RelationDef, Store, motion, theme};

use crate::credentials::{COOKIE_META, is_secret_field};
use crate::type_limit::{TypeLimitPicker, toggled};
use crate::widgets::{FieldSkin, field, pop_shadow};

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

/// Anytype getRelationTypes order, mapped to our formats.
const FORMATS: &[(&str, &str, &str)] = &[
    ("object", "Object", "◈"),
    ("longtext", "Text", "≡"),
    ("number", "Number", "#"),
    ("status", "Select", "◉"),
    ("tag", "Multi-select", "▤"),
    ("date", "Date", "▦"),
    ("checkbox", "Checkbox", "☑"),
    ("url", "URL", "⌁"),
    ("email", "Email", "@"),
    ("phone", "Phone", "☏"),
    ("shorttext", "Short text", "–"),
];

fn glyph_for(format: &str) -> &'static str {
    FORMATS.iter().find(|f| f.0 == format).map_or("≡", |f| f.2)
}

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    List,
    Suggest,
    Edit,
}

/// The cascading add-property popover (table "+" column).
pub struct PropertyFlow {
    items: Vec<FlowItem>,
    exclude: Vec<String>,
    space_id: String,
    stage: Stage,
    query: Entity<InputState>,
    name: Entity<InputState>,
    edit_format: Option<String>,
    format_menu: bool,
    edit_types: Vec<String>,
    /// Where the format submenu painted (it hangs outside the flow's own bounds).
    menu_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<FlowEvent> for PropertyFlow {}

impl PropertyFlow {
    /// `items`: stage-1 rows; `exclude`: keys this surface covers another way.
    pub fn new(
        items: Vec<FlowItem>,
        exclude: Vec<String>,
        space_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Filter or create…"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Property name"));
        let subs = vec![
            cx.subscribe_in(
                &query,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => {
                        let (library, _, can_create) = this.sections(cx);
                        if let Some(first) = library.into_iter().next() {
                            this.add(first, cx);
                        } else if can_create {
                            let q = this.query.read(cx).value().trim().to_string();
                            this.open_edit(q, None, window, cx);
                        }
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &name,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    _ => {}
                },
            ),
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
        ];
        Self {
            items,
            exclude,
            space_id,
            stage: Stage::List,
            query,
            name,
            edit_format: None,
            format_menu: false,
            edit_types: vec![],
            menu_bounds: Rc::new(Cell::new(None)),
            _subs: subs,
        }
    }

    /// The host's rows changed (a switch was applied).
    pub fn set_items(&mut self, items: Vec<FlowItem>, cx: &mut Context<Self>) {
        if self.items != items {
            self.items = items;
            cx.notify();
        }
    }

    /// (my properties, system properties, can create) for the current filter.
    fn sections(&self, cx: &App) -> (Vec<RelationDef>, Vec<RelationDef>, bool) {
        let store = Store::global(cx).read(cx);
        let q = self.query.read(cx).value().trim().to_lowercase();
        let label = |r: &RelationDef| {
            if r.name.is_empty() {
                r.key.clone()
            } else {
                r.name.clone()
            }
        };
        let all: Vec<RelationDef> = space_relations(&store.relations, &self.space_id)
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

    fn add(&mut self, rel: RelationDef, cx: &mut Context<Self>) {
        cx.emit(FlowEvent::Add(rel));
        cx.emit(FlowEvent::Close);
    }

    fn set_stage(&mut self, stage: Stage, window: &mut Window, cx: &mut Context<Self>) {
        self.stage = stage;
        match stage {
            Stage::Suggest => self.query.update(cx, |s, cx| s.focus(window, cx)),
            Stage::Edit => self.name.update(cx, |s, cx| s.focus(window, cx)),
            Stage::List => {}
        }
        cx.notify();
    }

    fn open_edit(
        &mut self,
        name: String,
        format: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        self.edit_format = format.map(str::to_string);
        self.format_menu = false;
        self.set_stage(Stage::Edit, window, cx);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        let Some(format) = self.edit_format.clone() else {
            return;
        };
        if name.is_empty() {
            return;
        }
        let task = create_relation(&name, &format, &self.edit_types, &self.space_id, cx);
        cx.spawn_in(window, async move |this, cx| match task.await {
            Ok(rel) => {
                let _ = this.update(cx, |this, cx| this.add(rel, cx));
            }
            Err(error) => log::error!("create property: {error:#}"),
        })
        .detach();
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.stage {
            Stage::Edit => self.set_stage(Stage::Suggest, window, cx),
            Stage::Suggest => self.set_stage(Stage::List, window, cx),
            Stage::List => cx.emit(FlowEvent::Close),
        }
    }
}

fn row(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(8.))
        .py(px(5.))
        .rounded(theme::radius::sm())
        .text_size(px(13.))
        .text_color(theme::fg())
}

fn pick_row(id: impl Into<ElementId>) -> Stateful<Div> {
    row(id)
        .w_full()
        .cursor_pointer()
        .hover(|s| s.bg(theme::hl_med()))
}

fn glyph(text: impl Into<SharedString>, emoji: bool) -> Div {
    div()
        .flex_none()
        .w(px(18.))
        .text_center()
        .text_color(if emoji { theme::fg() } else { theme::muted() })
        .child(text.into())
}

fn r_name(text: impl Into<SharedString>) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(text.into())
}

fn sep() -> Div {
    div().flex_none().h(px(1.)).bg(theme::border()).my(px(4.))
}

fn section(text: &'static str) -> Div {
    div()
        .px(px(8.))
        .pt(px(4.))
        .pb(px(2.))
        .text_size(px(11.))
        .text_color(theme::muted())
        .child(text)
}

/// Anytype's switch: 30×18 track, 14px knob. The knob slides and the track
/// recolours over 150ms (`transition: left 0.15s` / `background 0.15s`).
fn switch(key: &str, on: bool, window: &mut Window, cx: &mut App) -> Stateful<Div> {
    let p = motion::progress(
        SharedString::from(format!("switch-knob-{key}")),
        on,
        motion::CONTROL,
        window,
        cx,
    );
    div()
        .id(SharedString::from(format!("switch-{key}")))
        .flex_none()
        .relative()
        .w(px(30.))
        .h(px(18.))
        .rounded(px(9.))
        .bg(motion::mix(theme::hover(), theme::accent(), p))
        .cursor_pointer()
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(2. + 12. * p))
                .size(px(14.))
                .rounded_full()
                .bg(theme::white()),
        )
}

impl Render for PropertyFlow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut flow = div()
            .id("property-flow")
            .w(px(300.))
            .max_h(px(420.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::menu())
            .p(px(8.))
            .shadow(pop_shadow())
            .text_color(theme::fg())
            .occlude()
            .on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if this.format_menu
                    && this
                        .menu_bounds
                        .get()
                        .is_some_and(|b| b.contains(&ev.position))
                {
                    return;
                }
                cx.emit(FlowEvent::Close)
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.escape(window, cx);
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.escape(window, cx);
                }
            }));
        match self.stage {
            Stage::List => {
                let mut list = div()
                    .id("flow-list")
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .overflow_y_scroll();
                for it in &self.items {
                    let key = it.key.clone();
                    let on = it.on;
                    list = list.child(
                        row(SharedString::from(format!("item-{}", it.key)))
                            .child(glyph(
                                it.emoji
                                    .clone()
                                    .unwrap_or_else(|| glyph_for(&it.format).to_string()),
                                it.emoji.is_some(),
                            ))
                            .child(r_name(it.name.clone()))
                            .child(switch(&it.key, on, window, cx).on_click(cx.listener(
                                move |_, _, _, cx| cx.emit(FlowEvent::Toggle(key.clone(), !on)),
                            ))),
                    );
                }
                if self.items.is_empty() {
                    list = list.child(
                        div()
                            .px(px(8.))
                            .py(px(4.))
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .child("No properties yet."),
                    );
                }
                flow = flow.child(list).child(sep()).child(
                    div()
                        .id("add-row")
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(theme::radius::sm())
                        .text_size(px(13.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::hl_med()))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_stage(Stage::Suggest, window, cx)
                        }))
                        .child("＋ Add property"),
                );
            }
            Stage::Suggest => {
                let (library, system, can_create) = self.sections(cx);
                let q = self.query.read(cx).value().trim().to_string();
                let ql = q.to_lowercase();
                let formats: Vec<_> = FORMATS
                    .iter()
                    .filter(|f| ql.is_empty() || f.1.to_lowercase().contains(&ql))
                    .collect();
                flow = flow.child(field(&self.query, FieldSkin::Filter, window, cx));
                let mut scroll = div()
                    .id("flow-scroll")
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .min_h_0()
                    .overflow_y_scroll();
                let rel_row = |rel: &RelationDef, cx: &mut Context<Self>| {
                    let r = rel.clone();
                    let emoji = rel.icon_emoji.clone().filter(|e| !e.is_empty());
                    pick_row(SharedString::from(format!("rel-{}", rel.key)))
                        .on_click(cx.listener(move |this, _, _, cx| this.add(r.clone(), cx)))
                        .child(glyph(
                            emoji
                                .clone()
                                .unwrap_or_else(|| glyph_for(&rel.format).to_string()),
                            emoji.is_some(),
                        ))
                        .child(r_name(if rel.name.is_empty() {
                            rel.key.clone()
                        } else {
                            rel.name.clone()
                        }))
                };
                if !library.is_empty() {
                    scroll = scroll.child(section("My properties"));
                    for rel in &library {
                        scroll = scroll.child(rel_row(rel, cx));
                    }
                }
                if !system.is_empty() {
                    if !library.is_empty() {
                        scroll = scroll.child(sep());
                    }
                    scroll = scroll.child(section("System properties"));
                    for rel in &system {
                        scroll = scroll.child(rel_row(rel, cx));
                    }
                }
                if can_create || !formats.is_empty() {
                    if !library.is_empty() || !system.is_empty() {
                        scroll = scroll.child(sep());
                    }
                    scroll = scroll.child(section("Create new"));
                    if can_create {
                        let name = q.clone();
                        scroll = scroll.child(
                            pick_row("create")
                                .text_color(theme::accent())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_edit(name.clone(), None, window, cx)
                                }))
                                .child(format!("＋ Create property “{q}”")),
                        );
                    }
                    for f in formats {
                        let name = q.clone();
                        let id = f.0;
                        scroll = scroll.child(
                            pick_row(SharedString::from(format!("fmt-{id}")))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_edit(name.clone(), Some(id), window, cx)
                                }))
                                .child(glyph(f.2, false))
                                .child(r_name(f.1))
                                .child(div().text_color(theme::muted()).child("›")),
                        );
                    }
                }
                flow = flow.child(scroll);
            }
            Stage::Edit => {
                let fmt = self.edit_format.clone();
                let label = |t: &'static str| {
                    div()
                        .px(px(2.))
                        .pt(px(2.))
                        .text_size(px(11.))
                        .text_color(theme::muted())
                        .child(t)
                };
                let mut type_wrap = div().relative().child(
                    pick_row("type-row")
                        .border_1()
                        .border_color(theme::border())
                        .rounded(theme::radius::md())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.format_menu = !this.format_menu;
                            cx.notify();
                        }))
                        .child(glyph(fmt.as_deref().map_or("?", glyph_for), false))
                        .child(r_name(fmt.as_deref().map_or_else(
                            || "Select property type".to_string(),
                            format_name,
                        )))
                        .child(div().text_color(theme::muted()).child("›")),
                );
                if self.format_menu {
                    let mut menu = div()
                        .id("type-menu")
                        .w(px(282.))
                        .max_h(px(240.))
                        .overflow_y_scroll()
                        .bg(theme::panel())
                        .border_1()
                        .border_color(theme::border())
                        .rounded(theme::radius::menu())
                        .p(px(4.))
                        .shadow(pop_shadow())
                        .occlude()
                        .child({
                            let cell = self.menu_bounds.clone();
                            canvas(move |b, _, _| cell.set(Some(b)), |_, _, _, _| {})
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                        });
                    for f in FORMATS {
                        let active = fmt.as_deref() == Some(f.0);
                        let id = f.0;
                        menu = menu.child(
                            pick_row(SharedString::from(format!("menu-{id}")))
                                .when(active, |d| d.bg(theme::hl_med()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.edit_format = Some(id.to_string());
                                    this.format_menu = false;
                                    cx.notify();
                                }))
                                .child(glyph(f.2, false))
                                .child(r_name(f.1))
                                .when(active, |d| {
                                    d.child(div().text_color(theme::muted()).child("✓"))
                                }),
                        );
                    }
                    let menu =
                        motion::appear_frame("type-menu-appear", -4., window, cx).apply(menu);
                    type_wrap = type_wrap.child(
                        div().absolute().left_0().top(px(34.)).child(
                            deferred(anchored().snap_to_window_with_margin(px(8.)).child(menu))
                                .with_priority(3),
                        ),
                    );
                }
                let disabled = self.name.read(cx).value().trim().is_empty() || fmt.is_none();
                let mut form = div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(label("Name"))
                    .child(field(&self.name, FieldSkin::Filter, window, cx))
                    .child(label("Type"))
                    .child(type_wrap);
                if fmt.as_deref() == Some("object") {
                    let entity = cx.entity().downgrade();
                    form = form.child(TypeLimitPicker::new(
                        "limit",
                        self.edit_types.clone(),
                        self.space_id.clone(),
                        move |id, _, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                this.edit_types = toggled(&this.edit_types, id);
                                cx.notify();
                            });
                        },
                    ));
                }
                form = form.child(
                    div()
                        .id("submit")
                        .mt(px(4.))
                        .bg(theme::accent())
                        .rounded(theme::radius::md())
                        .py(px(7.))
                        .text_center()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0x14161a))
                        .when(disabled, |d| d.opacity(0.4))
                        .when(!disabled, |d| d.cursor_pointer())
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                        .child("Create"),
                );
                flow = flow.child(form);
            }
        }
        // The popover drops in as it opens; it takes keys from its first frame.
        motion::appear_frame("property-flow-appear", -4., window, cx).apply(flow)
    }
}

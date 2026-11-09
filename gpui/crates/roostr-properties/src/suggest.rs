//! Anytype's relationSuggest (`PropertySuggest.svelte`) and typeSuggest
//! (`TypeSuggest.svelte`) menus: one filter box over the space's properties
//! (with "Create property" and its format/type-limit steps) or types.
//!
//! Both are the kit `Command` palette: it owns the query field, the list,
//! hover/arrow highlight and Enter. Roostr supplies the rows (glyph + name),
//! its own filtering and ranking (`filterable(false)`), the create steps, and
//! dismisses on the first Escape (the palette alone would clear the query
//! first). The host keeps the menu's state while it is open and places the
//! panel; the panel pushes its own outside-press scrim into `layer`.

use std::rc::{Rc, Weak};

use roostr_core::objects::type_glyph;
use roostr_core::relations::{
    CREATABLE_FORMATS, create_relation, is_reserved_key, space_relations,
};
use roostr_core::{RelationDef, State, TypeDef};
use roostr_ui::overlay::dismissable;
use roostr_ui::{
    Command, CommandGroup, CommandItem, CommandState, IndexPath, Theme, glyph, prop_icon, radius,
    ty,
};
use rune::{Color, Cx, El, Key, KeyEvent, Tx, col, el, row, text};

use crate::type_limit::{toggled, type_limit_picker};
use crate::widgets::{Lens, act_button, empty_note, lens, pop_shadow};

/// What a suggest menu reports to its host.
pub enum SuggestEvent<T> {
    /// The person chose this.
    Picked(T),
    /// Escape, or a press outside the menu.
    Dismissed,
}

/// A PropertySuggest choice.
pub enum PropertyPick {
    /// An existing or just-created property.
    Relation(RelationDef),
    /// One of the host's non-relation `extras` (its key).
    Extra(String),
}

type EventFn<T> = Rc<dyn Fn(&mut State, SuggestEvent<T>)>;

fn format_name(format: &str) -> &str {
    match format {
        "shorttext" => "Text",
        "longtext" => "Long text",
        "number" => "Number",
        "status" => "Status",
        "tag" => "Tag",
        "date" => "Date",
        "checkbox" => "Checkbox",
        "url" => "URL",
        "email" => "Email",
        "phone" => "Phone",
        "object" => "Object",
        other => other,
    }
}

/// A row's lead (the property's emoji, else its format's line icon) and
/// name, with an optional muted note at the right end.
fn glyph_name(
    t: &Theme,
    ink: Color,
    emoji: Option<String>,
    format: &str,
    name: String,
    note: Option<String>,
) -> El {
    let lead = match emoji {
        Some(e) => el()
            .w(18.0)
            .center()
            .child(text(e).font(ty::ui(13.0)).color(ink)),
        None => el()
            .w(18.0)
            .center()
            .child(glyph(prop_icon(format), 14.0).color(t.muted)),
    };
    let mut line = row()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .center_y()
        .justify_between()
        .gap(10.0)
        .child(
            row()
                .shrink()
                .min_w(0.0)
                .center_y()
                .gap(8.0)
                .child(lead)
                .child(
                    text(name)
                        .font(ty::ui(13.0))
                        .color(ink)
                        .truncate()
                        .shrink()
                        .min_w(0.0),
                ),
        );
    if let Some(note) = note {
        line = line.child(text(note).font(ty::ui(11.0)).color(t.muted));
    }
    line
}

/// A palette row drawn by Roostr; `label` is what assistive tech reads.
fn row_item(label: impl Into<String>, body: El) -> CommandItem {
    CommandItem::new().label(label).child(body)
}

/// The highlighted row as the palette shows it: the state's while it is a row, else the first.
fn shown_highlight(
    command: &CommandState,
    targets: &[(IndexPath, impl Sized)],
) -> Option<IndexPath> {
    command
        .highlighted()
        .filter(|h| targets.iter().any(|(p, _)| p == h))
        .or_else(|| targets.first().map(|(p, _)| *p))
}

/// What a PropertySuggest row does.
#[derive(Clone)]
enum Target {
    Create,
    Format(&'static str),
    Extra(String),
    Relation(RelationDef),
}

/// The query and step the default highlight is placed for.
type Step = (String, bool, String);

/// A PropertySuggest menu's state, kept by the host while the menu is open.
#[derive(Clone)]
pub struct PropertySuggestState {
    space_id: String,
    exclude: Vec<String>,
    extras: Vec<(String, String)>,
    command: CommandState,
    creating: bool,
    create_format: String,
    create_types: Vec<String>,
    /// The query and step the default highlight was last placed for (None: place it).
    highlighted_for: Option<Step>,
    /// Dropped with the host's state: a create answering after the menu closed stays quiet.
    alive: Rc<()>,
}

impl PropertySuggestState {
    /// `exclude`: relation keys to hide (already present); `extras`: `(key, name)`
    /// non-relation options (system columns like Type/Created). The query field is
    /// focused on the menu's first frame.
    pub fn new(space_id: String, exclude: Vec<String>, extras: Vec<(String, String)>) -> Self {
        let mut command = CommandState::new();
        command.focus();
        Self {
            space_id,
            exclude,
            extras,
            command,
            creating: false,
            create_format: String::new(),
            create_types: Vec::new(),
            highlighted_for: None,
            alive: Rc::new(()),
        }
    }

    fn raw(&self) -> String {
        self.command.query().trim().to_string()
    }

    fn step(&self) -> Step {
        (self.raw(), self.creating, self.create_format.clone())
    }

    fn all(&self, s: &State) -> Vec<RelationDef> {
        space_relations(&s.store.relations, &self.space_id)
            .into_iter()
            .filter(|r| !r.hidden && !is_reserved_key(&r.key) && !self.exclude.contains(&r.key))
            .cloned()
            .collect()
    }

    fn filtered_extras(&self, q: &str) -> Vec<(String, String)> {
        self.extras
            .iter()
            .filter(|(k, n)| {
                !self.exclude.contains(k) && (q.is_empty() || n.to_lowercase().contains(q))
            })
            .cloned()
            .collect()
    }
}

fn label(r: &RelationDef) -> String {
    if r.name.is_empty() {
        r.key.clone()
    } else {
        r.name.clone()
    }
}

fn filtered(all: &[RelationDef], q: &str) -> Vec<RelationDef> {
    all.iter()
        .filter(|r| q.is_empty() || label(r).to_lowercase().contains(q))
        .cloned()
        .collect()
}

fn can_create(all: &[RelationDef], q: &str) -> bool {
    !q.is_empty() && !all.iter().any(|r| label(r).to_lowercase() == q)
}

/// Relation picker with search (+ "create property"): the menu panel.
pub struct PropertySuggest {
    key: String,
    state: PropertySuggestState,
    lens: Option<Lens<PropertySuggestState>>,
    on_event: Option<EventFn<PropertyPick>>,
}

impl PropertySuggest {
    pub fn new(key: impl Into<String>, state: &PropertySuggestState) -> Self {
        Self {
            key: key.into(),
            state: state.clone(),
            lens: None,
            on_event: None,
        }
    }

    /// Where the host keeps this menu's state.
    pub fn on_state(
        mut self,
        f: impl for<'a> Fn(&'a mut State) -> &'a mut PropertySuggestState + 'static,
    ) -> Self {
        self.lens = Some(lens(f));
        self
    }

    /// Picked / Dismissed; the host closes the menu here (drops the state).
    pub fn on_event(
        mut self,
        f: impl Fn(&mut State, SuggestEvent<PropertyPick>) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(f));
        self
    }

    pub fn build(self, s: &State, _cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) -> El {
        let st = &self.state;
        let raw = st.raw();
        let q = raw.to_lowercase();
        let all = st.all(s);
        let filtered = filtered(&all, &q);
        let extras = st.filtered_extras(&q);
        let can_create = can_create(&all, &q);
        let pick_format = can_create && st.creating && st.create_format.is_empty();
        let limits = can_create && st.creating && !st.create_format.is_empty();

        // Rows, one group per section; `targets` maps each row's path to its action.
        // Bodies are drawn once the highlighted row is known (it takes the accent ink).
        enum Body {
            Create(String),
            Format(&'static str),
            Extra(String),
            Relation(Option<String>, String, String),
        }
        let mut sections: Vec<(Option<String>, Vec<(String, Body, Target)>)> = Vec::new();
        if can_create && !st.creating {
            let text = format!("＋ Create property “{raw}”");
            sections.push((
                None,
                vec![(text.clone(), Body::Create(text), Target::Create)],
            ));
        }
        if pick_format {
            sections.push((
                Some(format!("Type of “{raw}”")),
                CREATABLE_FORMATS
                    .iter()
                    .map(|&f| {
                        (
                            format_name(f).to_string(),
                            Body::Format(f),
                            Target::Format(f),
                        )
                    })
                    .collect(),
            ));
        }
        if !extras.is_empty() {
            sections.push((
                Some("System".into()),
                extras
                    .into_iter()
                    .map(|(key, name)| (name.clone(), Body::Extra(name), Target::Extra(key)))
                    .collect(),
            ));
        }
        if !filtered.is_empty() {
            sections.push((
                Some("My properties".into()),
                filtered
                    .into_iter()
                    .map(|rel| {
                        let name = label(&rel);
                        let body = Body::Relation(
                            rel.icon_emoji.clone().filter(|e| !e.is_empty()),
                            name.clone(),
                            rel.format.clone(),
                        );
                        (name, body, Target::Relation(rel))
                    })
                    .collect(),
            ));
        }
        let mut targets: Vec<(IndexPath, Target)> = Vec::new();
        for (section, (_, rows)) in sections.iter().enumerate() {
            for (r, (_, _, target)) in rows.iter().enumerate() {
                targets.push((IndexPath::new(r).section(section), target.clone()));
            }
        }

        // Enter does what the web menu's did: the first matching property,
        // else "Create property", else the first format. The highlight is
        // placed once per query and step; arrows and hover move it from there.
        let step = st.step();
        let want = if pick_format {
            targets
                .iter()
                .find(|(_, t)| matches!(t, Target::Format(_)))
                .map(|(ix, _)| *ix)
        } else if !st.creating {
            targets
                .iter()
                .find(|(_, t)| matches!(t, Target::Relation(_)))
                .or_else(|| targets.iter().find(|(_, t)| matches!(t, Target::Create)))
                .map(|(ix, _)| *ix)
        } else {
            None
        };
        let place = st.highlighted_for.as_ref() != Some(&step) && want.is_some();
        let mut command = st.command.clone();
        if place {
            command.set_highlighted(want);
        }
        let current = shown_highlight(&command, &targets);

        let mut groups: Vec<CommandGroup> = Vec::new();
        for (section, (heading, rows)) in sections.into_iter().enumerate() {
            let mut group = CommandGroup::new();
            if let Some(h) = heading {
                group = group.label(h.to_uppercase());
            }
            for (r, (name, body, _)) in rows.into_iter().enumerate() {
                let selected = current == Some(IndexPath::new(r).section(section));
                let ink = if selected {
                    t.kit.accent_foreground
                } else {
                    t.fg
                };
                let body = match body {
                    Body::Create(text_) => text(text_).font(ty::ui(13.0)).color(t.accent),
                    Body::Format(f) => {
                        glyph_name(t, ink, None, f, format_name(f).to_string(), None)
                    }
                    Body::Extra(n) => text(n).font(ty::ui(13.0)).color(ink),
                    Body::Relation(emoji, n, format) => {
                        glyph_name(t, ink, emoji, &format, n, Some(format.clone()))
                    }
                };
                group = group.item(row_item(name, body));
            }
            groups.push(group);
        }

        let key = self.key.clone();
        let mut palette = Command::new(format!("{key}-command"), &command)
            .filterable(false)
            .bordered(false)
            .placeholder("Filter or create a property…")
            .max_h(280.0)
            .fill(t.panel)
            .text_color(t.fg)
            .empty(el().padding(6.0).child(empty_note(
                t,
                "No properties yet — type a name to create one.",
                12.0,
            )));
        for group in groups {
            palette = palette.group(group);
        }
        if limits && let Some(lens) = &self.lens {
            palette = palette.footer(limit_panel(s, t, st, &raw, lens, &self.on_event));
        }
        if let Some(lens) = self.lens.clone() {
            let on_event = self.on_event.clone();
            let targets = Rc::new(targets);
            let (l1, l2, l3, l4) = (lens.clone(), lens.clone(), lens.clone(), lens.clone());
            let (e1, e2) = (on_event.clone(), on_event.clone());
            let step_k = step.clone();
            palette = palette
                .on_state(move |s: &mut State| &mut l1(s).command)
                .on_select(move |s: &mut State, _, _: &mut Tx| {
                    let st = l2(s);
                    st.highlighted_for = Some(st.step());
                })
                .on_query(move |s: &mut State, _: &str, _: &mut Tx| {
                    l3(s).highlighted_for = None;
                })
                .on_key(
                    move |s: &mut State, ev: &KeyEvent, _: &mut Tx| match ev.key {
                        Key::Escape => {
                            emit(&e1, s, SuggestEvent::Dismissed);
                            true
                        }
                        // Step 3 has no row to confirm: Enter creates the property.
                        Key::Enter if limits => {
                            create(s, &l4, &e1);
                            true
                        }
                        Key::Up | Key::Down => {
                            // Arrows move from the placed highlight, not the palette's own.
                            let st = l4(s);
                            if st.highlighted_for.as_ref() != Some(&step_k) {
                                if want.is_some() {
                                    st.command.set_highlighted(want);
                                }
                                st.highlighted_for = Some(step_k.clone());
                            }
                            false
                        }
                        _ => false,
                    },
                )
                .on_confirm(move |s: &mut State, ix: IndexPath, _: &mut Tx| {
                    let Some(target) = targets
                        .iter()
                        .find(|(p, _)| *p == ix)
                        .map(|(_, t)| t.clone())
                    else {
                        return;
                    };
                    confirm(s, &lens, &e2, target);
                });
        }

        let panel = col()
            .key(key.clone())
            .w(280.0)
            .max_h(340.0)
            .fill(t.panel)
            .border(1.0, t.border)
            .radius(radius::MENU)
            .clip_rounded(radius::MENU)
            .padding(4.0)
            .box_shadow(pop_shadow())
            .items_stretch()
            .child(palette.build(t).shrink().min_h(0.0));
        let on_event = self.on_event.clone();
        dismissable(
            panel,
            &key,
            move |s: &mut State| emit(&on_event, s, SuggestEvent::Dismissed),
            layer,
        )
    }
}

fn emit<T>(on_event: &Option<EventFn<T>>, s: &mut State, ev: SuggestEvent<T>) {
    if let Some(f) = on_event {
        f(s, ev);
    }
}

fn confirm(
    s: &mut State,
    lens: &Lens<PropertySuggestState>,
    on_event: &Option<EventFn<PropertyPick>>,
    target: Target,
) {
    match target {
        Target::Create => {
            let st = lens(s);
            st.creating = true;
            st.highlighted_for = None;
        }
        Target::Format(f) => choose(s, lens, on_event, f),
        Target::Extra(key) => emit(on_event, s, SuggestEvent::Picked(PropertyPick::Extra(key))),
        Target::Relation(rel) => emit(
            on_event,
            s,
            SuggestEvent::Picked(PropertyPick::Relation(rel)),
        ),
    }
}

/// A new property's type is always asked: an Object then asks which types it may link.
fn choose(
    s: &mut State,
    lens: &Lens<PropertySuggestState>,
    on_event: &Option<EventFn<PropertyPick>>,
    format: &str,
) {
    let st = lens(s);
    st.create_format = format.to_string();
    st.highlighted_for = None;
    if format != "object" {
        create(s, lens, on_event);
    }
}

/// Creates the property named by the query; `Picked` follows when the daemon answers
/// (unless the host closed the menu by then).
fn create(
    s: &mut State,
    lens: &Lens<PropertySuggestState>,
    on_event: &Option<EventFn<PropertyPick>>,
) {
    let st = lens(s);
    let name = st.raw();
    let (format, types, space) = (
        st.create_format.clone(),
        st.create_types.clone(),
        st.space_id.clone(),
    );
    let alive: Weak<()> = Rc::downgrade(&st.alive);
    let task = create_relation(&name, &format, &types, &space, s);
    let on_event = on_event.clone();
    s.rt.spawn(async move |cx| match task.await {
        Ok(rel) => {
            if alive.upgrade().is_some() {
                cx.update(|s| {
                    emit(
                        &on_event,
                        s,
                        SuggestEvent::Picked(PropertyPick::Relation(rel)),
                    )
                });
            }
        }
        Err(error) => log::error!("create property: {error:#}"),
    })
    .detach();
}

/// The step-3 panel (an Object's type limits + Back / Create), shown under
/// the list once a format that needs it is chosen.
fn limit_panel(
    s: &State,
    t: &Theme,
    st: &PropertySuggestState,
    raw: &str,
    lens: &Lens<PropertySuggestState>,
    on_event: &Option<EventFn<PropertyPick>>,
) -> El {
    let (toggle, back, make) = (lens.clone(), lens.clone(), lens.clone());
    let on_event = on_event.clone();
    col()
        .key("limits")
        .gap(4.0)
        .padding(4.0)
        .items_stretch()
        .child(
            el().px(6.0).pt(6.0).pb(2.0).child(
                text(format!("Object “{raw}” links to").to_uppercase())
                    .font(ty::ui(10.0))
                    .color(t.muted),
            ),
        )
        .child(type_limit_picker(
            s,
            t,
            "type-limit",
            &st.create_types,
            &st.space_id,
            Rc::new(move |s: &mut State, id: &str| {
                let st = toggle(s);
                st.create_types = toggled(&st.create_types, id);
            }),
        ))
        .child(
            row()
                .gap(6.0)
                .justify_end()
                .pt(6.0)
                .child(
                    act_button(t, "back", "Back", false)
                        .on_press(move |s: &mut State| {
                            let st = back(s);
                            st.create_format.clear();
                            st.highlighted_for = None;
                        })
                        .build(t),
                )
                .child(
                    act_button(t, "create-it", "Create", false)
                        .on_press(move |s: &mut State| create(s, &make, &on_event))
                        .build(t),
                ),
        )
}

/// A TypeSuggest menu's state, kept by the host while the menu is open.
#[derive(Clone)]
pub struct TypeSuggestState {
    space_id: String,
    exclude: Vec<String>,
    command: CommandState,
    placeholder: String,
}

impl TypeSuggestState {
    /// The types of `space_id` (plus Query and Collection) minus `exclude` keys. The query
    /// field is focused on the menu's first frame.
    pub fn new(space_id: String, exclude: Vec<String>, placeholder: String) -> Self {
        let mut command = CommandState::new();
        command.focus();
        Self {
            space_id,
            exclude,
            command,
            placeholder,
        }
    }

    /// Replace the excluded keys (the host's list changed).
    pub fn set_exclude(&mut self, exclude: Vec<String>) {
        if self.exclude != exclude {
            self.exclude = exclude;
        }
    }

    /// The space's types, name/key prefix matches first, then the rest that contain the query.
    fn matches(&self, s: &State) -> Vec<TypeDef> {
        let store = &s.store;
        let q = self.command.query().trim().to_lowercase();
        let space = if self.space_id.is_empty() {
            store
                .channels
                .first()
                .map(|s| s.id.clone())
                .unwrap_or_default()
        } else {
            self.space_id.clone()
        };
        // Queries and collections are built-in kinds, not type objects, but a query can list them.
        let view_kind = |key: &str, name: &str| TypeDef {
            id: String::new(),
            key: key.into(),
            name: name.into(),
            icon: type_glyph(store, key, &space),
            layout: "page".into(),
            default_template_id: String::new(),
            space: String::new(),
            builtin: true,
        };
        let pool: Vec<TypeDef> = store
            .types
            .iter()
            .filter(|t| t.space == space)
            .cloned()
            .chain([
                view_kind("query", "Query"),
                view_kind("collection", "Collection"),
            ])
            .filter(|t| !self.exclude.contains(&t.key))
            .collect();
        if q.is_empty() {
            return pool;
        }
        let starts: Vec<TypeDef> = pool
            .iter()
            .filter(|t| t.name.to_lowercase().starts_with(&q) || t.key.starts_with(&q))
            .cloned()
            .collect();
        let contains = pool.into_iter().filter(|t| {
            !starts.iter().any(|s| s.id == t.id && s.key == t.key)
                && (t.name.to_lowercase().contains(&q) || t.key.contains(&q))
        });
        starts.iter().cloned().chain(contains).collect()
    }
}

/// Type picker with filter input: the menu body (the host gives it a surface).
pub struct TypeSuggest {
    key: String,
    state: TypeSuggestState,
    lens: Option<Lens<TypeSuggestState>>,
    on_event: Option<EventFn<String>>,
}

impl TypeSuggest {
    pub fn new(key: impl Into<String>, state: &TypeSuggestState) -> Self {
        Self {
            key: key.into(),
            state: state.clone(),
            lens: None,
            on_event: None,
        }
    }

    /// Where the host keeps this menu's state.
    pub fn on_state(
        mut self,
        f: impl for<'a> Fn(&'a mut State) -> &'a mut TypeSuggestState + 'static,
    ) -> Self {
        self.lens = Some(lens(f));
        self
    }

    /// Picked(type key) / Dismissed; the host closes the menu here (drops the state).
    pub fn on_event(mut self, f: impl Fn(&mut State, SuggestEvent<String>) + 'static) -> Self {
        self.on_event = Some(Rc::new(f));
        self
    }

    pub fn build(self, s: &State, _cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) -> El {
        let st = &self.state;
        let matches = st.matches(s);
        let paths: Vec<(IndexPath, ())> = (0..matches.len())
            .map(|r| (IndexPath::new(r), ()))
            .collect();
        let current = shown_highlight(&st.command, &paths);
        let keys: Rc<Vec<String>> = Rc::new(matches.iter().map(|t| t.key.clone()).collect());
        let items: Vec<CommandItem> = matches
            .iter()
            .enumerate()
            .map(|(r, ty_)| {
                let icon = if ty_.icon.is_empty() {
                    type_glyph(&s.store, &ty_.key, &st.space_id)
                } else {
                    ty_.icon.clone()
                };
                let name = if ty_.name.is_empty() {
                    ty_.key.clone()
                } else {
                    ty_.name.clone()
                };
                let ink = if current == Some(IndexPath::new(r)) {
                    t.kit.accent_foreground
                } else {
                    t.fg
                };
                row_item(
                    name.clone(),
                    row()
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .center_y()
                        .gap(8.0)
                        .child(
                            el().w(20.0)
                                .center()
                                .child(text(icon).font(ty::ui(13.0)).color(ink)),
                        )
                        .child(
                            text(name)
                                .font(ty::ui(13.0))
                                .color(ink)
                                .truncate()
                                .grow(1.0)
                                .shrink()
                                .min_w(0.0),
                        )
                        .child(text(ty_.key.clone()).font(ty::ui(11.0)).color(t.muted)),
                )
            })
            .collect();
        let key = self.key.clone();
        let mut palette = Command::new(format!("{key}-command"), &st.command)
            .filterable(false)
            .bordered(false)
            .placeholder(st.placeholder.clone())
            .max_h(240.0)
            .fill(t.panel)
            .text_color(t.fg)
            .items(items)
            .empty(
                el().px(8.0)
                    .py(6.0)
                    .child(empty_note(t, "No matching types.", 12.0)),
            );
        if let Some(lens) = self.lens.clone() {
            let (e1, e2) = (self.on_event.clone(), self.on_event.clone());
            palette = palette
                .on_state(move |s: &mut State| &mut lens(s).command)
                .on_key(move |s: &mut State, ev: &KeyEvent, _: &mut Tx| {
                    if ev.key == Key::Escape {
                        emit(&e1, s, SuggestEvent::Dismissed);
                        return true;
                    }
                    false
                })
                .on_confirm(move |s: &mut State, ix: IndexPath, _: &mut Tx| {
                    if let Some(k) = keys.get(ix.row).cloned() {
                        emit(&e2, s, SuggestEvent::Picked(k));
                    }
                });
        }
        let body = col()
            .key(key.clone())
            .min_w(240.0)
            .items_stretch()
            .child(palette.build(t).shrink().min_h(0.0));
        let on_event = self.on_event.clone();
        dismissable(
            body,
            &key,
            move |s: &mut State| emit(&on_event, s, SuggestEvent::Dismissed),
            layer,
        )
    }
}

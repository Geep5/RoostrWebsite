//! List rows and the command palette: gpui-kit's `ListItem` ([`ListItem`]) and its `Command`
//! family ([`Command`], [`CommandGroup`], [`CommandItem`], [`CommandState`]): a search field
//! over a filtered, keyboard-navigable list of items in titled groups. gpui-kit's
//! `SearchableList` (the list inside `Select` and `Combobox`) is [`crate::Select`]'s dropdown.
//!
//! The query and the highlighted item live in a [`CommandState`] in the app state; the items
//! are built each view run (their rows are elements), and the palette binds to the state with
//! [`Command::on_state`]:
//!
//! ```ignore
//! Command::new("search", &s.palette)
//!     .placeholder("Search types…")
//!     .group(CommandGroup::new().label("Types").item(CommandItem::new().label("Page")))
//!     .on_state(|s: &mut S| &mut s.palette)
//!     .on_confirm(|s: &mut S, path: IndexPath, _| s.pick(path))
//!     .build(t)
//! ```

use std::any::Any;
use std::rc::Rc;

use rune::{Color, Cursor, El, Key, KeyEvent, Tx, col, el, row, text};

use crate::icons::{Glyph, glyph};
use crate::input::{InputState, lens};
use crate::overlay::Acts;
use crate::theme::{Theme, radius, rems, ty};
use crate::ty::Weights as _;
use crate::{Input, KitSize};

/// gpui-kit's `ListItem`: a row (py 3.5, px 10.5, 14pt text) with the `list_hover` fill under
/// the pointer, `list_active` while selected, an optional check slot and suffix.
pub struct ListItem {
    key: String,
    label: Option<String>,
    children: Vec<El>,
    suffix: Option<El>,
    check_icon: Option<Glyph>,
    selected: bool,
    secondary_selected: bool,
    confirmed: bool,
    disabled: bool,
    separator: bool,
    gap: Option<f32>,
    px: Option<f32>,
    py: Option<f32>,
    radius: Option<f32>,
    text_size: Option<f32>,
    text_color: Option<Color>,
    press_fill: Option<Color>,
    pointer: bool,
    grow: bool,
    acts: Acts,
}

impl ListItem {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: None,
            children: Vec::new(),
            suffix: None,
            check_icon: None,
            selected: false,
            secondary_selected: false,
            confirmed: false,
            disabled: false,
            separator: false,
            gap: None,
            px: None,
            py: None,
            radius: None,
            text_size: None,
            text_color: None,
            press_fill: None,
            pointer: false,
            grow: false,
            acts: Acts::default(),
        }
    }

    /// A text label in the row's type (14pt, `foreground`, or the overrides).
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn child(mut self, e: El) -> Self {
        self.children.push(e);
        self
    }
    pub fn suffix(mut self, e: El) -> Self {
        self.suffix = Some(e);
        self
    }
    /// A trailing check slot (17.5 wide) showing `g` while [`Self::confirmed`].
    pub fn check_icon(mut self, g: Glyph) -> Self {
        self.check_icon = Some(g);
        self
    }
    /// The `list_active` fill.
    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }
    /// A `selection`-coloured outline (the right-clicked row).
    pub fn secondary_selected(mut self, on: bool) -> Self {
        self.secondary_selected = on;
        self
    }
    /// Shows the check icon.
    pub fn confirmed(mut self, on: bool) -> Self {
        self.confirmed = on;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    /// A non-selectable row (`separator()`): muted, no hover or handlers.
    pub fn separator(mut self) -> Self {
        self.separator = true;
        self
    }
    pub fn gap(mut self, g: f32) -> Self {
        self.gap = Some(g);
        self
    }
    pub fn px(mut self, p: f32) -> Self {
        self.px = Some(p);
        self
    }
    pub fn py(mut self, p: f32) -> Self {
        self.py = Some(p);
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = Some(r);
        self
    }
    pub fn text_size(mut self, s: f32) -> Self {
        self.text_size = Some(s);
        self
    }
    pub fn text_color(mut self, c: Color) -> Self {
        self.text_color = Some(c);
        self
    }
    /// The fill while pressed (`.active(|s| s.bg(..))`).
    pub fn press_fill(mut self, c: Color) -> Self {
        self.press_fill = Some(c);
        self
    }
    /// The pointing-hand cursor (`cursor_pointer()`).
    pub fn pointer(mut self) -> Self {
        self.pointer = true;
        self
    }
    /// Takes the room left in its row (`flex_1()`).
    pub fn grow(mut self) -> Self {
        self.grow = true;
        self
    }
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.press(f);
        self
    }
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.press_tx(f);
        self
    }
    /// The pointer came over the row (`on_mouse_enter`).
    pub fn on_hover<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.with(move |e: El| e.on_hover(f));
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let key = self.key;
        let selectable = !(self.disabled || self.separator);
        let active = self.confirmed || self.selected || self.secondary_selected;
        let ink = if selectable {
            self.text_color.unwrap_or(t.kit.foreground)
        } else {
            t.kit.muted_foreground
        };
        let size = self.text_size.unwrap_or(rems(1.0));
        let gap = self.gap.unwrap_or(rems(0.25));
        let mut content = row()
            .key(format!("{key}-content"))
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .center_y()
            .gap(gap);
        if let Some(l) = self.label {
            content = content.child(text(l).font(ty::ui(size)).color(ink).truncate());
        }
        for c in self.children {
            content = content.child(c);
        }
        let mut inner = row()
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .center_y()
            .justify_between()
            .gap(gap)
            .child(content);
        if let Some(g) = self.check_icon {
            let mut slot = el().w(rems(1.25)).center();
            if self.confirmed {
                slot = slot.child(glyph(g, KitSize::Small.icon_px()).color(t.kit.muted_foreground));
            }
            inner = inner.child(slot);
        }
        let mut e = row()
            .key(key)
            .role("listitem")
            .gap(gap)
            .px(self.px.unwrap_or(rems(0.75)))
            .py(self.py.unwrap_or(rems(0.25)))
            .center_y()
            .justify_between()
            .child(inner);
        if let Some(r) = self.radius {
            e = e.radius(r);
        }
        if let Some(s) = self.suffix {
            e = e.child(s);
        }
        if self.grow {
            e = e.grow(1.0).shrink().min_w(0.0);
        }
        if selectable && self.selected {
            e = e.fill(t.kit.list_active);
        }
        if selectable && self.secondary_selected {
            e = e.border(1.0, t.kit.selection);
        }
        if selectable {
            if !active {
                e = e.hover(t.kit.list_hover);
            }
            if let Some(p) = self.press_fill {
                e = e.press(p);
            }
            if self.pointer {
                e = e.cursor(Cursor::Pointer);
            }
            e = crate::motion::pressable_row(e);
        }
        e = e.disabled(self.disabled);
        self.acts.apply(e, selectable)
    }
}

/// A row's place in a [`Command`]: gpui-kit's `IndexPath` (`section` = group, `row` = item).
/// Ungrouped items are section 0, and groups follow them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct IndexPath {
    pub section: usize,
    pub row: usize,
}

impl IndexPath {
    pub fn new(row: usize) -> Self {
        Self { section: 0, row }
    }
    pub fn section(mut self, section: usize) -> Self {
        self.section = section;
        self
    }
}

/// One command: a label (matched by the search), optional icon and keywords, a check, or a
/// custom row body ([`Self::child`]) the row pads and highlights.
#[derive(Default)]
pub struct CommandItem {
    label: Option<String>,
    icon: Option<Glyph>,
    keywords: Vec<String>,
    checked: bool,
    disabled: bool,
    content: Option<El>,
}

impl CommandItem {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn icon(mut self, g: Glyph) -> Self {
        self.icon = Some(g);
        self
    }
    /// More words the search matches.
    pub fn keywords<I: IntoIterator<Item = S>, S: Into<String>>(mut self, k: I) -> Self {
        self.keywords = k.into_iter().map(Into::into).collect();
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    /// The row's body, in place of icon and label (the label still names it and matches).
    pub fn child(mut self, e: El) -> Self {
        self.content = Some(e);
        self
    }

    fn matches(&self, q: &str) -> bool {
        matches(self.label.as_deref(), &self.keywords, q)
    }
}

fn matches(label: Option<&str>, keywords: &[String], q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let q = q.to_lowercase();
    label.is_some_and(|l| l.to_lowercase().contains(&q))
        || keywords.iter().any(|k| k.to_lowercase().contains(&q))
}

/// A titled section of [`CommandItem`]s; its heading hides while none of its items match.
#[derive(Default)]
pub struct CommandGroup {
    label: Option<String>,
    items: Vec<CommandItem>,
}

impl CommandGroup {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn item(mut self, item: CommandItem) -> Self {
        self.items.push(item);
        self
    }
    pub fn items(mut self, items: impl IntoIterator<Item = CommandItem>) -> Self {
        self.items.extend(items);
        self
    }
}

enum CommandEntry {
    Item(CommandItem),
    Group(CommandGroup),
    Separator,
}

/// gpui-kit's `CommandState`: the query and the highlighted item. Keep it in the app state.
#[derive(Clone, Default)]
pub struct CommandState {
    /// The palette's search field.
    pub query: InputState,
    highlighted: Option<IndexPath>,
    /// `set_highlighted(None)` keeps nothing highlighted until a key or the pointer moves it.
    cleared: bool,
}

impl CommandState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn query(&self) -> &str {
        self.query.value()
    }
    /// Sets the query (the highlight goes to the first match on the next build).
    pub fn set_query(&mut self, q: impl Into<String>) {
        self.query.set_value(q);
        self.highlighted = None;
        self.cleared = false;
    }
    /// The highlighted item as last moved by the keyboard, the pointer or the app; the
    /// palette shows the first enabled match when it's gone or unset.
    pub fn highlighted(&self) -> Option<IndexPath> {
        self.highlighted
    }
    pub fn set_highlighted(&mut self, path: Option<IndexPath>) {
        self.highlighted = path;
        self.cleared = path.is_none();
    }
    /// Focus the search field.
    pub fn focus(&mut self) {
        self.query.focus();
    }
}

type Access<S> = Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut CommandState>;
type PathFn<S> = Rc<dyn Fn(&mut S, IndexPath, &mut Tx)>;
type CancelFn<S> = Rc<dyn Fn(&mut S, &mut Tx)>;
type QueryFn<S> = Rc<dyn Fn(&mut S, &str, &mut Tx)>;
type KeyFn<S> = Rc<dyn Fn(&mut S, &KeyEvent, &mut Tx) -> bool>;

/// The handlers a palette was given, held without naming the app state.
#[derive(Clone, Default)]
struct Hooks {
    select: Option<Rc<dyn Any>>,
    confirm: Option<Rc<dyn Any>>,
    cancel: Option<Rc<dyn Any>>,
    query: Option<Rc<dyn Any>>,
    key: Option<Rc<dyn Any>>,
}

fn hook<T: Clone + 'static>(h: &Option<Rc<dyn Any>>) -> Option<T> {
    h.as_ref().map(|h| {
        h.downcast_ref::<T>()
            .expect("one app state type per command")
            .clone()
    })
}

/// What the keys and a new query work from: every item (to match a new query against) and the
/// current matches, in order.
struct Nav {
    key: String,
    items: Vec<(IndexPath, Option<String>, Vec<String>, bool)>,
    matched: Vec<(IndexPath, bool)>,
    current: Option<IndexPath>,
    searchable: bool,
    filterable: bool,
}

impl Nav {
    fn first_enabled(&self, q: &str) -> Option<IndexPath> {
        let q = q.trim();
        self.items
            .iter()
            .find(|(_, l, k, d)| !d && (!self.filterable || matches(l.as_deref(), k, q)))
            .map(|(p, ..)| *p)
    }

    /// The match `step` from the current one, wrapping round and skipping disabled ones.
    fn step(&self, step: isize) -> Option<IndexPath> {
        let len = self.matched.len();
        if len == 0 {
            return None;
        }
        let at = self
            .current
            .and_then(|c| self.matched.iter().position(|(p, _)| *p == c));
        let mut next = at.unwrap_or(if step >= 0 { len - 1 } else { 0 });
        for _ in 0..len {
            next = (next as isize + step).rem_euclid(len as isize) as usize;
            if !self.matched[next].1 {
                return Some(self.matched[next].0);
            }
        }
        None
    }
}

fn item_key(key: &str, p: IndexPath) -> String {
    format!("{key}-item-{}-{}", p.section, p.row)
}

fn highlight<S: 'static>(a: &Access<S>, h: &Hooks, s: &mut S, to: Option<IndexPath>, tx: &mut Tx) {
    let st = a(s);
    let changed = st.highlighted != to;
    st.highlighted = to;
    st.cleared = false;
    if changed && let (Some(p), Some(f)) = (to, hook::<PathFn<S>>(&h.select)) {
        f(s, p, tx);
    }
}

fn command_key<S: 'static>(
    a: &Access<S>,
    h: &Hooks,
    nav: &Nav,
    s: &mut S,
    ev: &KeyEvent,
    tx: &mut Tx,
) -> bool {
    if let Some(f) = hook::<KeyFn<S>>(&h.key)
        && f(s, ev, tx)
    {
        return true;
    }
    match ev.key {
        Key::Up | Key::Down => {
            if let Some(p) = nav.step(if ev.key == Key::Down { 1 } else { -1 })
                && Some(p) != nav.current
            {
                highlight(a, h, s, Some(p), tx);
                tx.scroll_into_view(item_key(&nav.key, p));
            }
            true
        }
        Key::Enter => {
            if let Some(p) = nav.current
                && let Some(f) = hook::<PathFn<S>>(&h.confirm)
            {
                f(s, p, tx);
            }
            true
        }
        Key::Escape => {
            if nav.searchable && !a(s).query.value().is_empty() {
                a(s).query.clear();
                let first = nav.first_enabled("");
                highlight(a, h, s, first, tx);
                if let Some(f) = hook::<QueryFn<S>>(&h.query) {
                    f(s, "", tx);
                }
                return true;
            }
            if let Some(f) = hook::<CancelFn<S>>(&h.cancel) {
                f(s, tx);
            }
            false
        }
        _ => false,
    }
}

enum Act<'a> {
    Hover(IndexPath),
    Confirm(IndexPath),
    Keys(Rc<Nav>),
    Search(Input, &'a Theme, Rc<Nav>),
}

type Wire = Rc<dyn Fn(El, Act<'_>, &Hooks) -> El>;

fn wire<S: 'static>(a: &Access<S>, e: El, act: Act<'_>, h: &Hooks) -> El {
    let (a, h) = (a.clone(), h.clone());
    match act {
        Act::Hover(p) => e.on_hover_change(move |s: &mut S, inside: bool, tx: &mut Tx| {
            if inside {
                highlight(&a, &h, s, Some(p), tx);
            }
        }),
        Act::Confirm(p) => e.on_press_tx(move |s: &mut S, tx: &mut Tx| {
            if let Some(f) = hook::<PathFn<S>>(&h.confirm) {
                f(s, p, tx);
            }
        }),
        Act::Keys(nav) => e.on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
            command_key(&a, &h, &nav, s, ev, tx)
        }),
        Act::Search(input, t, nav) => {
            let (a1, a2, h2, nav2) = (a.clone(), a.clone(), h.clone(), nav.clone());
            input
                .on_edit(lens(move |s: &mut S| &mut a1(s).query))
                .on_change(move |s: &mut S, tx: &mut Tx| {
                    let q = a2(s).query.value().to_owned();
                    let first = nav2.first_enabled(&q);
                    highlight(&a2, &h2, s, first, tx);
                    if let Some(f) = hook::<QueryFn<S>>(&h2.query) {
                        f(s, q.trim(), tx);
                    }
                })
                .on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
                    command_key(&a, &h, &nav, s, ev, tx)
                })
                .build(t)
        }
    }
}

/// gpui-kit's `Command` palette over a [`CommandState`]: a popover-coloured panel (radius 10
/// and a border unless [`Self::bordered`]`(false)`), a search field with a magnifier, and the
/// matching items under their group headings, the highlighted one on `accent`.
pub struct Command {
    key: String,
    state: CommandState,
    entries: Vec<CommandEntry>,
    searchable: bool,
    filterable: bool,
    placeholder: Option<String>,
    empty: Option<El>,
    header: Option<El>,
    footer: Option<El>,
    max_h: f32,
    bordered: bool,
    radius: Option<f32>,
    fill: Option<Color>,
    text_color: Option<Color>,
    wire: Option<Wire>,
    hooks: Hooks,
}

impl Command {
    pub fn new(key: impl Into<String>, state: &CommandState) -> Self {
        Self {
            key: key.into(),
            state: state.clone(),
            entries: Vec::new(),
            searchable: true,
            filterable: true,
            placeholder: None,
            empty: None,
            header: None,
            footer: None,
            max_h: rems(18.75),
            bordered: true,
            radius: None,
            fill: None,
            text_color: None,
            wire: None,
            hooks: Hooks::default(),
        }
    }

    /// An ungrouped item (section 0).
    pub fn item(mut self, item: CommandItem) -> Self {
        self.entries.push(CommandEntry::Item(item));
        self
    }
    pub fn items(mut self, items: impl IntoIterator<Item = CommandItem>) -> Self {
        self.entries
            .extend(items.into_iter().map(CommandEntry::Item));
        self
    }
    pub fn group(mut self, group: CommandGroup) -> Self {
        self.entries.push(CommandEntry::Group(group));
        self
    }
    /// A rule between the entries around it, drawn only when something matches on both sides.
    pub fn separator(mut self) -> Self {
        self.entries.push(CommandEntry::Separator);
        self
    }
    /// Show the search field and filter by it (default on).
    pub fn searchable(mut self, on: bool) -> Self {
        self.searchable = on;
        self
    }
    /// Filter by the query (default on); off keeps every item for an outside search.
    pub fn filterable(mut self, on: bool) -> Self {
        self.filterable = on;
        self
    }
    /// The search field's placeholder (default "Type a command or search...").
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = Some(p.into());
        self
    }
    /// What shows when nothing matches, in place of "No results found.".
    pub fn empty(mut self, e: El) -> Self {
        self.empty = Some(e);
        self
    }
    pub fn header(mut self, e: El) -> Self {
        self.header = Some(e);
        self
    }
    pub fn footer(mut self, e: El) -> Self {
        self.footer = Some(e);
        self
    }
    /// The list's tallest (default 18.75 rem, 262.5).
    pub fn max_h(mut self, h: f32) -> Self {
        self.max_h = h;
        self
    }
    /// The border and large radius (default on).
    pub fn bordered(mut self, on: bool) -> Self {
        self.bordered = on;
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = Some(r);
        self
    }
    /// The panel's fill, in place of `popover`.
    pub fn fill(mut self, c: Color) -> Self {
        self.fill = Some(c);
        self
    }
    /// The rows' text colour, in place of `popover_foreground`.
    pub fn text_color(mut self, c: Color) -> Self {
        self.text_color = Some(c);
        self
    }

    /// Binds the palette to its [`CommandState`] in the app state.
    pub fn on_state<S: 'static>(
        mut self,
        f: impl for<'a> Fn(&'a mut S) -> &'a mut CommandState + 'static,
    ) -> Self {
        let a: Access<S> = Rc::new(f);
        self.wire = Some(Rc::new(move |e, act, h| wire::<S>(&a, e, act, h)));
        self
    }
    /// The highlighted item changed (keys, pointer, new query).
    pub fn on_select<S: 'static>(
        mut self,
        f: impl Fn(&mut S, IndexPath, &mut Tx) + 'static,
    ) -> Self {
        self.hooks.select = Some(Rc::new(Rc::new(f) as PathFn<S>));
        self
    }
    /// An item was clicked or Enter pressed on the highlighted one.
    pub fn on_confirm<S: 'static>(
        mut self,
        f: impl Fn(&mut S, IndexPath, &mut Tx) + 'static,
    ) -> Self {
        self.hooks.confirm = Some(Rc::new(Rc::new(f) as PathFn<S>));
        self
    }
    /// Escape with an empty query (the first Escape clears a query).
    pub fn on_cancel<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.hooks.cancel = Some(Rc::new(Rc::new(f) as CancelFn<S>));
        self
    }
    /// The query changed (trimmed).
    pub fn on_query<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.hooks.query = Some(Rc::new(Rc::new(f) as QueryFn<S>));
        self
    }
    /// Sees every key the palette would (search field or list) before it does; returning
    /// `true` takes the key (e.g. an Escape that dismisses at once instead of clearing the
    /// query first, or an Enter with nothing highlighted).
    pub fn on_key<S: 'static>(
        mut self,
        f: impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool + 'static,
    ) -> Self {
        self.hooks.key = Some(Rc::new(Rc::new(f) as KeyFn<S>));
        self
    }

    fn bind(&self, e: El, act: Act<'_>) -> El {
        match &self.wire {
            Some(w) => w(e, act, &self.hooks),
            None => e,
        }
    }

    pub fn build(mut self, t: &Theme) -> El {
        let key = self.key.clone();
        let q = if self.searchable {
            self.state.query().trim().to_owned()
        } else {
            String::new()
        };
        let filter = |item: &CommandItem| !self.filterable || item.matches(&q);

        // Rows and matches as gpui-kit's `update_matches` lays them out.
        enum Row {
            Separator,
            Heading(String),
            Item(IndexPath, CommandItem),
        }
        let entries = std::mem::take(&mut self.entries);
        let has_ungrouped = entries.iter().any(|e| matches!(e, CommandEntry::Item(_)));
        let mut rows: Vec<Row> = Vec::new();
        let mut items = Vec::new();
        let mut matched: Vec<(IndexPath, bool)> = Vec::new();
        let (mut ungrouped, mut group_ix, mut pending) = (0, 0, false);
        for entry in entries {
            match entry {
                CommandEntry::Separator => pending = !rows.is_empty(),
                CommandEntry::Item(item) => {
                    let p = IndexPath::new(ungrouped);
                    ungrouped += 1;
                    items.push((p, item.label.clone(), item.keywords.clone(), item.disabled));
                    if !filter(&item) {
                        continue;
                    }
                    if std::mem::take(&mut pending) {
                        rows.push(Row::Separator);
                    }
                    matched.push((p, item.disabled));
                    rows.push(Row::Item(p, item));
                }
                CommandEntry::Group(g) => {
                    let section = group_ix + usize::from(has_ungrouped);
                    group_ix += 1;
                    let mut visible = Vec::new();
                    for (r, item) in g.items.into_iter().enumerate() {
                        let p = IndexPath::new(r).section(section);
                        items.push((p, item.label.clone(), item.keywords.clone(), item.disabled));
                        if filter(&item) {
                            visible.push((p, item));
                        }
                    }
                    if visible.is_empty() {
                        continue;
                    }
                    if std::mem::take(&mut pending) {
                        rows.push(Row::Separator);
                    }
                    if let Some(l) = g.label {
                        rows.push(Row::Heading(l));
                    }
                    for (p, item) in visible {
                        matched.push((p, item.disabled));
                        rows.push(Row::Item(p, item));
                    }
                }
            }
        }
        let current = match self
            .state
            .highlighted
            .filter(|h| matched.iter().any(|(p, d)| p == h && !d))
        {
            Some(h) => Some(h),
            None if self.state.cleared => None,
            None => matched.iter().find(|(_, d)| !d).map(|(p, _)| *p),
        };
        let nav = Rc::new(Nav {
            key: key.clone(),
            items,
            matched,
            current,
            searchable: self.searchable,
            filterable: self.filterable,
        });

        let item_radius = match (self.bordered, self.radius) {
            (true, r) => (r.unwrap_or(radius::KIT_LG) - rems(0.25) - 1.0).max(0.0),
            (false, Some(r)) => (r - rems(0.25)).max(0.0),
            (false, None) => radius::KIT,
        };
        let fg = self.text_color.unwrap_or(t.kit.popover_foreground);
        let font = ty::ui(rems(0.875));
        let mut list = col()
            .key(format!("{key}-rows"))
            .padding(rems(0.25))
            .items_stretch();
        let empty = rows.is_empty();
        for r in rows {
            list = list.child(match r {
                Row::Separator => el()
                    .w_full()
                    .py(4.0)
                    .child(el().w_full().h(1.0).fill(t.kit.border)),
                Row::Heading(l) => row().w_full().px(rems(0.5)).py(rems(0.375)).child(
                    text(l)
                        .font(ty::ui(rems(0.75)).medium())
                        .color(t.kit.muted_foreground),
                ),
                Row::Item(p, item) => {
                    let selected = current == Some(p) && !item.disabled;
                    let ink = if item.disabled {
                        t.kit.muted_foreground
                    } else if selected {
                        t.kit.accent_foreground
                    } else {
                        fg
                    };
                    let mut e = row()
                        .key(item_key(&key, p))
                        .role("option")
                        .checked(selected)
                        .w_full()
                        .center_y()
                        .gap(rems(0.5))
                        .px(rems(0.5))
                        .py(rems(0.375))
                        .radius(item_radius);
                    if let Some(l) = &item.label {
                        e = e.label(l.clone());
                    }
                    if selected {
                        e = e.fill(t.kit.accent);
                    }
                    e = match item.content {
                        Some(c) => e.child(c),
                        None => {
                            let mut c = row()
                                .grow(1.0)
                                .shrink()
                                .min_w(0.0)
                                .center_y()
                                .gap(rems(0.5));
                            if let Some(g) = item.icon {
                                let ic = if selected {
                                    t.kit.accent_foreground
                                } else {
                                    t.kit.muted_foreground
                                };
                                c = c.child(glyph(g, rems(1.0)).color(ic));
                            }
                            if let Some(l) = item.label.clone() {
                                c = c.child(text(l).font(font).color(ink).truncate().a11y_hidden());
                            }
                            e.child(c)
                        }
                    };
                    if item.checked {
                        // `ml_auto()`: a spacer pushes the check to the row's end.
                        e = e
                            .child(el().grow(1.0))
                            .child(glyph(Glyph::Check, KitSize::XSmall.icon_px()).color(ink));
                    }
                    if item.disabled {
                        e.disabled(true)
                    } else {
                        let e = self.bind(e, Act::Hover(p));
                        self.bind(e, Act::Confirm(p))
                    }
                }
            });
        }
        let body = if empty {
            let msg = self.empty.take().unwrap_or_else(|| {
                row()
                    .key(format!("{key}-empty"))
                    .w_full()
                    .justify_center()
                    .py(rems(1.5))
                    .child(
                        text("No results found.")
                            .font(font)
                            .color(t.kit.muted_foreground),
                    )
            });
            col().w_full().padding(rems(0.25)).child(msg)
        } else {
            col()
                .key(format!("{key}-list"))
                .role("listbox")
                .w_full()
                .max_h(self.max_h)
                .min_h(0.0)
                .shrink()
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(list)
        };

        let mut root = col()
            .key(key.clone())
            .w_full()
            .fill(self.fill.unwrap_or(t.kit.popover));
        if self.bordered {
            let r = self.radius.unwrap_or(radius::KIT_LG);
            root = root
                .radius(r)
                .clip_rounded(r)
                .border(1.0, t.kit.border)
                .padding(1.0);
        } else if let Some(r) = self.radius {
            root = root.radius(r).clip_rounded(r);
        }
        if let Some(h) = self.header.take() {
            root = root.child(h);
        }
        if self.searchable {
            let input = Input::new(format!("{key}-search"), &self.state.query)
                .appearance(false)
                .px(0.0)
                .placeholder(
                    self.placeholder
                        .clone()
                        .unwrap_or_else(|| "Type a command or search...".into()),
                )
                .prefix(glyph(Glyph::KitSearch, rems(1.0)).color(t.kit.muted_foreground));
            let input = match &self.wire {
                Some(w) => w(el(), Act::Search(input, t, nav.clone()), &self.hooks),
                None => input.build(t),
            };
            root = root.child(
                row()
                    .key(format!("{key}-search-bar"))
                    .px(rems(0.75))
                    .pb(1.0)
                    .border_sides([0.0, 0.0, 1.0, 0.0], t.kit.border)
                    .child(input),
            );
        } else {
            root = self.bind(root.focusable(true).no_focus_ring(), Act::Keys(nav.clone()));
        }
        root = root.child(body);
        if let Some(f) = self.footer.take() {
            root = root.child(f);
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Mods, Runtime};

    #[derive(Default)]
    struct S {
        palette: CommandState,
        picked: Vec<IndexPath>,
        selected: Vec<IndexPath>,
        cancelled: usize,
        row_presses: usize,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let t = Theme::dark();
        let cmd = Command::new("cmd", &s.palette)
            .group(
                CommandGroup::new()
                    .label("Types")
                    .item(CommandItem::new().label("Page").icon(Glyph::FileText))
                    .item(CommandItem::new().label("Person").keywords(["contact"]))
                    .item(CommandItem::new().label("Project").disabled(true)),
            )
            .separator()
            .group(
                CommandGroup::new()
                    .label("Actions")
                    .item(CommandItem::new().label("New page").checked(true)),
            )
            .on_state(|s: &mut S| &mut s.palette)
            .on_select(|s: &mut S, p, _| s.selected.push(p))
            .on_confirm(|s: &mut S, p, _| s.picked.push(p))
            .on_cancel(|s: &mut S, _| s.cancelled += 1)
            .build(t);
        col().w(360.0).padding(20.0).gap(8.0).child(cmd).child(
            ListItem::new("li")
                .label("Row")
                .on_press(|s: &mut S| s.row_presses += 1)
                .build(t),
        )
    }

    fn key(rt: &mut Runtime<S>, key: Key) {
        rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
    }

    fn click(rt: &mut Runtime<S>, k: &str) {
        let (x, y) = rt.find(k).unwrap_or_else(|| panic!("{k}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn command_filters_and_keys_choose() {
        let mut s = S::default();
        s.palette.focus();
        let mut rt = Runtime::new(
            s,
            view,
            (500.0, 600.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.palette.query.is_focused());
        // Down from the first match skips the disabled Project and lands on New page.
        key(&mut rt, Key::Down);
        key(&mut rt, Key::Down);
        assert_eq!(
            rt.state.palette.highlighted(),
            Some(IndexPath::new(0).section(1))
        );
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.picked, [IndexPath::new(0).section(1)]);
        // A keyword query leaves Person alone, highlighted.
        key(&mut rt, Key::Text("cont".into()));
        assert!(rt.find("cmd-item-0-0").is_none());
        assert!(rt.find("cmd-item-0-1").is_some());
        assert!(rt.find("cmd-item-1-0").is_none());
        assert_eq!(rt.state.palette.highlighted(), Some(IndexPath::new(1)));
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.picked.last(), Some(&IndexPath::new(1)));
        // Nothing matches: the empty note.
        key(&mut rt, Key::Text("zzz".into()));
        assert!(rt.find("cmd-empty").is_some());
        // Escape clears the query first, then cancels.
        key(&mut rt, Key::Escape);
        assert_eq!(rt.state.palette.query(), "");
        assert_eq!(rt.state.cancelled, 0);
        key(&mut rt, Key::Escape);
        assert_eq!(rt.state.cancelled, 1);
        // A click confirms that item.
        click(&mut rt, "cmd-item-0-0");
        assert_eq!(rt.state.picked.last(), Some(&IndexPath::new(0)));
        assert!(!rt.state.selected.is_empty());
    }

    #[test]
    fn list_item_presses() {
        let mut rt = Runtime::new(
            S::default(),
            view,
            (500.0, 600.0),
            Env {
                system_dark: false,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        click(&mut rt, "li");
        assert_eq!(rt.state.row_presses, 1);
    }
}

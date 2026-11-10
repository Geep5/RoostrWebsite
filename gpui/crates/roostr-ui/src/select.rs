//! gpui-kit's `Select` + `SelectState` (and its single-select `Combobox`, which is the same
//! trigger and dropdown with a searchable list and a custom trigger body): a bordered trigger
//! with a chevron that opens a popover list under it, with a check on the chosen item, an
//! optional search field, section headers, and Up / Down / Enter / Escape.
//!
//! The items, the choice, the keyboard cursor, the query and whether it's open live in a
//! [`SelectState`] in the app state; the trigger binds to it with [`Select::on_state`]:
//!
//! ```ignore
//! Select::new("agent", &s.agent)
//!     .placeholder("")
//!     .on_state(|s: &mut S| &mut s.agent)
//!     .on_change(|s: &mut S, value: Option<&str>| s.set_agent(value))
//!     .build(t, &mut layer)
//! ```

use std::any::Any;
use std::rc::Rc;

use rune::{Color, Cursor, El, Key, KeyEvent, Transition, Tx, col, el, row, text};

use crate::KitSize;
use crate::icons::{Glyph, glyph};
use crate::input::{
    InputState, fade, input_fill, input_h, input_px, input_text, kit_text_icon_button, lens,
};
use crate::overlay::{Placement, dismissable, split};
use crate::theme::{Theme, radius, rems, shadow, ty};
use crate::{Input, motion};

/// One choice of a select: gpui-kit's `SearchableListItem` (its `value` and `title`), the
/// section it's listed under, and whether it can be chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectItem {
    pub value: String,
    pub title: String,
    pub group: Option<String>,
    pub disabled: bool,
    /// More words the search matches (gpui-kit's `SearchableListItem::matches` overrides).
    pub keywords: Vec<String>,
}

impl SelectItem {
    pub fn new(value: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            title: title.into(),
            group: None,
            disabled: false,
            keywords: Vec::new(),
        }
    }

    /// Listed under the section header `group` (gpui-kit's `SearchableListGroup`); consecutive
    /// items of one group form a section.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }

    /// More words the search matches besides the title.
    pub fn keywords<I: IntoIterator<Item = S>, S: Into<String>>(mut self, k: I) -> Self {
        self.keywords = k.into_iter().map(Into::into).collect();
        self
    }
}

/// gpui-kit's `SelectState`: the items, the committed choice, the keyboard cursor, the search
/// query and whether the dropdown is open. Keep it in the app state.
#[derive(Clone, Default)]
pub struct SelectState {
    items: Vec<SelectItem>,
    selected: Option<usize>,
    cursor: Option<usize>,
    open: bool,
    searchable: bool,
    /// The dropdown's search field.
    pub query: InputState,
    trigger_w: f32,
}

impl SelectState {
    pub fn new(items: Vec<SelectItem>, selected: Option<usize>) -> Self {
        let selected = selected.filter(|&i| i < items.len());
        Self {
            items,
            selected,
            cursor: selected,
            ..Self::default()
        }
    }

    /// A search field over the list (default off).
    pub fn searchable(mut self, on: bool) -> Self {
        self.searchable = on;
        self
    }

    pub fn items(&self) -> &[SelectItem] {
        &self.items
    }

    /// Replaces the items; the choice follows its value, or is dropped when it's gone.
    pub fn set_items(&mut self, items: Vec<SelectItem>) {
        let value = self.selected_value().map(str::to_owned);
        let cursor = self
            .cursor
            .and_then(|i| self.items.get(i))
            .map(|i| i.value.clone());
        self.items = items;
        self.selected = value.and_then(|v| self.position(&v));
        self.cursor = cursor.and_then(|v| self.position(&v)).or(self.selected);
    }

    pub fn position(&self, value: &str) -> Option<usize> {
        self.items.iter().position(|i| i.value == value)
    }

    pub fn set_selected_index(&mut self, index: Option<usize>) {
        self.selected = index.filter(|&i| i < self.items.len());
        self.cursor = self.selected;
    }

    /// Chooses the item with `value` (nothing when none has it), clearing the query.
    pub fn set_selected_value(&mut self, value: &str) {
        self.query.clear();
        let i = self.position(value);
        self.set_selected_index(i);
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_value(&self) -> Option<&str> {
        self.selected_item().map(|i| i.value.as_str())
    }

    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.selected.and_then(|i| self.items.get(i))
    }

    /// The keyboard cursor's item index.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens or closes the dropdown, as a press on the trigger does.
    pub fn toggle(&mut self) {
        if self.open {
            self.close()
        } else {
            self.open = true
        }
    }

    /// Closes the dropdown, dropping the query and putting the cursor back on the choice.
    pub fn close(&mut self) {
        self.open = false;
        if !self.query.value().is_empty() {
            self.query.clear();
        }
        self.cursor = self.selected;
    }

    /// The indices of the items the query leaves, in order. An item shows when its title
    /// contains the query, ignoring case, or its section's title does.
    pub fn visible(&self) -> Vec<usize> {
        let q = self.query.value().trim().to_lowercase();
        (0..self.items.len())
            .filter(|&i| {
                let it = &self.items[i];
                q.is_empty()
                    || it.title.to_lowercase().contains(&q)
                    || it.keywords.iter().any(|k| k.to_lowercase().contains(&q))
                    || it
                        .group
                        .as_ref()
                        .is_some_and(|g| g.to_lowercase().contains(&q))
            })
            .collect()
    }

    /// Moves the cursor to the next (`down`) or previous visible item, wrapping round.
    pub fn move_cursor(&mut self, down: bool) {
        let vis = self.visible();
        if vis.is_empty() {
            return;
        }
        let at = self.cursor.and_then(|c| vis.iter().position(|&i| i == c));
        let next = match (at, down) {
            (None, true) => 0,
            (None, false) => vis.len() - 1,
            (Some(p), true) => (p + 1) % vis.len(),
            (Some(p), false) => (p + vis.len() - 1) % vis.len(),
        };
        self.cursor = Some(vis[next]);
    }

    /// A new query puts the cursor on the first item it leaves.
    fn requery(&mut self) {
        self.cursor = self.visible().first().copied();
    }

    /// Commits the item at `index` (the cursor's on Enter) and closes; `None` when it can't
    /// be chosen. Returns the committed value.
    pub fn confirm(&mut self, index: Option<usize>) -> Option<String> {
        let i = index?;
        let item = self.items.get(i)?;
        if item.disabled {
            return None;
        }
        let value = item.value.clone();
        self.selected = Some(i);
        self.close();
        Some(value)
    }

    /// `clean`: no choice.
    pub fn clear(&mut self) {
        self.selected = None;
        self.cursor = None;
    }
}

type Access<S> = Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut SelectState>;
type ChangeFn<S> = Rc<dyn Fn(&mut S, Option<&str>, &mut Tx)>;
type ItemFn = Box<dyn Fn(&SelectItem, bool) -> El>;

/// What a select's handler wiring attaches.
enum Act<'a> {
    Toggle,
    Layout,
    Pick(usize),
    Clear,
    Keys(&'a str),
    Search(Input, &'a Theme, &'a str),
    Dismiss(&'a str, &'a mut Vec<El>),
}

type Wire = Rc<dyn Fn(El, Act<'_>, Option<&Rc<dyn Any>>) -> El>;

fn change<S: 'static>(f: Option<&Rc<dyn Any>>) -> Option<ChangeFn<S>> {
    f.map(|f| {
        f.downcast_ref::<ChangeFn<S>>()
            .expect("one app state type per select")
            .clone()
    })
}

/// The keys a select's dropdown takes: Up / Down move the cursor, Enter commits it, Escape
/// closes.
fn select_key<S>(
    a: &Access<S>,
    f: &Option<ChangeFn<S>>,
    key: &str,
    s: &mut S,
    ev: &KeyEvent,
    tx: &mut Tx,
) -> bool {
    let st = a(s);
    match ev.key {
        Key::Up | Key::Down => {
            st.move_cursor(ev.key == Key::Down);
            if let Some(c) = st.cursor {
                tx.scroll_into_view(format!("{key}-item-{c}"));
            }
            true
        }
        Key::Enter => {
            let cursor = st.cursor;
            if let Some(v) = st.confirm(cursor)
                && let Some(f) = f
            {
                f(s, Some(&v), tx);
            }
            true
        }
        Key::Escape => {
            st.close();
            true
        }
        _ => false,
    }
}

fn wire<S: 'static>(a: &Access<S>, e: El, act: Act<'_>, on_change: Option<&Rc<dyn Any>>) -> El {
    let f = change::<S>(on_change);
    let a = a.clone();
    match act {
        Act::Toggle => motion::pressable(e).on_press(move |s: &mut S| a(s).toggle()),
        Act::Layout => e.on_layout(move |s: &mut S, r: [f32; 4]| {
            let st = a(s);
            if (st.trigger_w - r[2]).abs() > 0.01 {
                st.trigger_w = r[2];
            }
        }),
        Act::Pick(i) => motion::pressable(e).on_press_tx(move |s: &mut S, tx: &mut Tx| {
            if let Some(v) = a(s).confirm(Some(i))
                && let Some(f) = &f
            {
                f(s, Some(&v), tx);
            }
        }),
        Act::Clear => e.on_press_tx(move |s: &mut S, tx: &mut Tx| {
            a(s).clear();
            if let Some(f) = &f {
                f(s, None, tx);
            }
        }),
        Act::Keys(key) => {
            let key = key.to_owned();
            e.on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
                select_key(&a, &f, &key, s, ev, tx)
            })
        }
        Act::Search(input, t, key) => {
            let (a1, a2, key) = (a.clone(), a.clone(), key.to_owned());
            input
                .on_edit(lens(move |s: &mut S| &mut a1(s).query))
                .on_change(move |s: &mut S, _: &mut Tx| a2(s).requery())
                .on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
                    select_key(&a, &f, &key, s, ev, tx)
                })
                .build(t)
        }
        Act::Dismiss(key, layer) => dismissable(e, key, move |s: &mut S| a(s).close(), layer),
    }
}

/// gpui-kit's `Select` (and single-select `Combobox`) bound to a [`SelectState`].
pub struct Select {
    key: String,
    state: SelectState,
    size: KitSize,
    placeholder: Option<String>,
    search_placeholder: Option<String>,
    title_prefix: Option<String>,
    icon: Option<Glyph>,
    cleanable: bool,
    disabled: bool,
    appearance: bool,
    focus_ring: bool,
    menu_width: Option<f32>,
    menu_max_h: f32,
    trigger: Option<El>,
    empty: Option<El>,
    render_item: Option<ItemFn>,
    fill: Option<Color>,
    border: Option<Color>,
    radius: Option<f32>,
    px: Option<f32>,
    py: Option<f32>,
    h: Option<f32>,
    w: Option<f32>,
    gap: Option<f32>,
    text_size: Option<f32>,
    text_color: Option<Color>,
    wire: Option<Wire>,
    on_change: Option<Rc<dyn Any>>,
}

impl Select {
    pub fn new(key: impl Into<String>, state: &SelectState) -> Self {
        Self {
            key: key.into(),
            state: state.clone(),
            size: KitSize::Medium,
            placeholder: None,
            search_placeholder: None,
            title_prefix: None,
            icon: None,
            cleanable: false,
            disabled: false,
            appearance: true,
            focus_ring: true,
            menu_width: None,
            menu_max_h: rems(20.0),
            trigger: None,
            empty: None,
            render_item: None,
            fill: None,
            border: None,
            radius: None,
            px: None,
            py: None,
            h: None,
            w: None,
            gap: None,
            text_size: None,
            text_color: None,
            wire: None,
            on_change: None,
        }
    }

    /// Binds the select to its [`SelectState`] in the app state.
    pub fn on_state<S: 'static>(
        mut self,
        f: impl for<'a> Fn(&'a mut S) -> &'a mut SelectState + 'static,
    ) -> Self {
        let a: Access<S> = Rc::new(f);
        self.wire = Some(Rc::new(move |e, act, change| wire::<S>(&a, e, act, change)));
        self
    }

    /// A choice was committed (`SelectEvent::Confirm`): its value, `None` when cleared.
    pub fn on_change<S: 'static>(self, f: impl Fn(&mut S, Option<&str>) + 'static) -> Self {
        self.on_change_tx(move |s: &mut S, v: Option<&str>, _: &mut Tx| f(s, v))
    }

    pub fn on_change_tx<S: 'static>(
        mut self,
        f: impl Fn(&mut S, Option<&str>, &mut Tx) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(Rc::new(f) as ChangeFn<S>));
        self
    }

    pub fn size(mut self, size: KitSize) -> Self {
        self.size = size;
        self
    }
    pub fn xsmall(self) -> Self {
        self.size(KitSize::XSmall)
    }
    pub fn small(self) -> Self {
        self.size(KitSize::Small)
    }
    pub fn large(self) -> Self {
        self.size(KitSize::Large)
    }

    /// The trigger's text while nothing is chosen (default "Please select").
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = Some(p.into());
        self
    }
    /// The search field's placeholder (default "Search...").
    pub fn search_placeholder(mut self, p: impl Into<String>) -> Self {
        self.search_placeholder = Some(p.into());
        self
    }
    /// Text before the chosen title on the trigger.
    pub fn title_prefix(mut self, p: impl Into<String>) -> Self {
        self.title_prefix = Some(p.into());
        self
    }
    /// An icon in place of the chevron.
    pub fn icon(mut self, g: Glyph) -> Self {
        self.icon = Some(g);
        self
    }
    /// A clear button in place of the chevron while something is chosen.
    pub fn cleanable(mut self, on: bool) -> Self {
        self.cleanable = on;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    /// `appearance(false)`: no fill, border, radius or ring on the trigger.
    pub fn appearance(mut self, on: bool) -> Self {
        self.appearance = on;
        self
    }
    /// `focus_ring(false)`: no 3pt halo while open or focused (the border still tints).
    pub fn focus_ring(mut self, on: bool) -> Self {
        self.focus_ring = on;
        self
    }
    /// The dropdown's width, in place of the trigger's.
    pub fn menu_width(mut self, w: f32) -> Self {
        self.menu_width = Some(w);
        self
    }
    /// The list's tallest (default 20 rem, 280).
    pub fn menu_max_h(mut self, h: f32) -> Self {
        self.menu_max_h = h;
        self
    }
    /// The trigger's body, in place of the title and chevron (`Combobox::render_trigger`).
    pub fn trigger(mut self, e: El) -> Self {
        self.trigger = Some(e);
        self
    }
    /// What the list shows when nothing matches, in place of the Inbox icon.
    pub fn empty(mut self, e: El) -> Self {
        self.empty = Some(e);
        self
    }
    /// Each row's content, in place of its title (`SearchableListDelegate::render_item`):
    /// `f(item, checked)`. The row keeps its padding, highlight and check.
    pub fn render_item(mut self, f: impl Fn(&SelectItem, bool) -> El + 'static) -> Self {
        self.render_item = Some(Box::new(f));
        self
    }
    /// The trigger's fill (`.bg(..)`).
    pub fn fill(mut self, c: Color) -> Self {
        self.fill = Some(c);
        self
    }
    /// The trigger's border colour (`.border_color(..)`).
    pub fn border_color(mut self, c: Color) -> Self {
        self.border = Some(c);
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = Some(r);
        self
    }
    /// Horizontal padding inside the trigger's border.
    pub fn px(mut self, px: f32) -> Self {
        self.px = Some(px);
        self
    }
    /// Vertical padding inside the trigger's border; with it the trigger is as tall as its
    /// content (`.h_auto().py(..)`).
    pub fn py(mut self, py: f32) -> Self {
        self.py = Some(py);
        self
    }
    pub fn h(mut self, h: f32) -> Self {
        self.h = Some(h);
        self
    }
    /// The trigger's width; as wide as its parent gives otherwise.
    pub fn w(mut self, w: f32) -> Self {
        self.w = Some(w);
        self
    }
    /// The gap between the title and the chevron (default 3.5).
    pub fn gap(mut self, g: f32) -> Self {
        self.gap = Some(g);
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

    fn bind(&self, e: El, act: Act<'_>) -> El {
        match &self.wire {
            Some(w) if !self.disabled => w(e, act, self.on_change.as_ref()),
            _ => e,
        }
    }

    /// The trigger in place, and while open the dropdown (with its scrim) in `layer`.
    pub fn build(mut self, t: &Theme, layer: &mut Vec<El>) -> El {
        let open = self.state.open && !self.disabled;
        let trigger = self.build_trigger(t, open);
        if open {
            let key = self.key.clone();
            let surface = self.build_surface(t);
            let (side, align) = split(Placement::BottomStart);
            let surface = match &self.wire {
                Some(w) => w(surface, Act::Dismiss(&key, layer), self.on_change.as_ref()),
                None => surface,
            };
            layer.push(
                surface
                    .anchored_fit(key, side, align, 6.0)
                    .transition(dropdown_appear())
                    .instant_changes(),
            );
        }
        trigger
    }

    /// The trigger alone: bordered field with the chosen title (or the placeholder in
    /// `muted_foreground`) and a chevron.
    fn build_trigger(&mut self, t: &Theme, open: bool) -> El {
        let key = self.key.clone();
        let disabled = self.disabled;
        let size = self.size;
        let font = ty::ui(self.text_size.unwrap_or(input_text(size)));
        let border_w = 1.0;
        let body = match self.trigger.take() {
            Some(b) => b,
            None => {
                let title = match self.state.selected_item() {
                    Some(it) => {
                        let s = match &self.title_prefix {
                            Some(p) => format!("{p}{}", it.title),
                            None => it.title.clone(),
                        };
                        let c = if disabled {
                            t.kit.muted_foreground
                        } else {
                            self.text_color.unwrap_or(t.kit.foreground)
                        };
                        text(s).font(font).color(c)
                    }
                    None => text(
                        self.placeholder
                            .clone()
                            .unwrap_or_else(|| "Please select".into()),
                    )
                    .font(font)
                    .color(t.kit.muted_foreground),
                };
                let mut inner = row()
                    .key(format!("{key}-inner"))
                    .w_full()
                    .min_w(0.0)
                    .center_y()
                    .justify_between()
                    .gap(self.gap.unwrap_or(rems(0.25)))
                    .child(
                        title
                            .key(format!("{key}-title"))
                            .grow(1.0)
                            .shrink()
                            .min_w(0.0)
                            .truncate(),
                    );
                if self.cleanable && self.state.selected.is_some() {
                    let clear =
                        kit_text_icon_button(t, format!("{key}-clear"), Glyph::Close, "Clear");
                    inner = inner.child(self.bind(clear, Act::Clear));
                } else {
                    let (g, px) = match self.icon {
                        Some(g) => (g, KitSize::XSmall.icon_px()),
                        None => (
                            Glyph::KitChevronDown,
                            match size {
                                KitSize::XSmall => KitSize::XSmall.icon_px(),
                                KitSize::Small => KitSize::Small.icon_px(),
                                _ => KitSize::Medium.icon_px(),
                            },
                        ),
                    };
                    inner = inner.child(
                        glyph(g, px)
                            .key(format!("{key}-caret"))
                            .color(t.kit.muted_foreground)
                            .a11y_hidden(),
                    );
                }
                inner
            }
        };
        let px = self.px.unwrap_or(input_px(size)) + border_w;
        let mut e = row()
            .key(key.clone())
            .role("combobox")
            .expanded(open)
            .center_y()
            .justify_between()
            .px(px)
            .child(body)
            .focusable(!disabled)
            .disabled(disabled);
        if let Some(l) = self.placeholder.clone().filter(|p| !p.is_empty()) {
            e = e.label(l);
        }
        if let Some(v) = self.state.selected_item() {
            e = e.a11y_value(v.title.clone());
        }
        e = match (self.py, self.h) {
            (_, Some(h)) => e.h(h),
            (Some(py), None) => e.py(py + border_w),
            (None, None) => e.h(input_h(size)),
        };
        e = match self.w {
            Some(w) => e.w(w),
            None => e.w_full(),
        };
        if self.appearance {
            let fill = self.fill.unwrap_or_else(|| input_fill(t, disabled));
            let border = if open {
                t.kit.ring
            } else {
                self.border.unwrap_or(t.kit.input)
            };
            e = e
                .fill(fill)
                .radius(self.radius.unwrap_or(radius::KIT))
                .border(1.0, border);
            if open && self.focus_ring {
                e = e.outline(3.0, 0.0, fade(t.kit.ring, 0.5)).no_focus_ring();
            } else if self.focus_ring {
                e = e.focus_within_ring(3.0, 0.0, fade(t.kit.ring, 0.5));
            } else {
                e = e.no_focus_ring();
            }
            if disabled {
                e = e.layer_opacity(0.5);
            }
        } else {
            e = e.no_focus_ring();
        }
        e = self.bind(e, Act::Layout);
        if !open {
            e = self.bind(e, Act::Toggle);
        }
        e
    }

    /// The open dropdown's surface on its own (gpui-kit's popover-styled list), for showing it
    /// in place; [`Self::build`] anchors it under the trigger.
    pub fn build_surface(&mut self, t: &Theme) -> El {
        let key = self.key.clone();
        let size = self.size;
        let font = ty::ui(input_text(size));
        let vis = self.state.visible();
        let pad_x = input_px(size) + 1.0 - 4.0;
        let pad_y = match size {
            KitSize::Large => rems(0.5),
            KitSize::Small => rems(0.125),
            _ => rems(0.25),
        };
        let mut rows = col()
            .key(format!("{key}-rows"))
            .padding(4.0)
            .items_stretch();
        let mut group: Option<&str> = None;
        for &i in &vis {
            let item = &self.state.items[i];
            if item.group.as_deref() != group {
                group = item.group.as_deref();
                if let Some(g) = group {
                    rows = rows.child(
                        row()
                            .key(format!("{key}-group-{i}"))
                            .px(pad_x)
                            .py(pad_y)
                            .child(
                                text(g.to_owned())
                                    .font(ty::ui(rems(0.875)))
                                    .color(t.kit.muted_foreground),
                            ),
                    );
                }
            }
            let checked = self.state.selected == Some(i);
            let cursor = self.state.cursor == Some(i);
            let ink = if item.disabled {
                t.kit.muted_foreground
            } else if cursor {
                t.kit.accent_foreground
            } else {
                t.kit.foreground
            };
            let content = match &self.render_item {
                Some(f) => f(item, checked),
                None => text(item.title.clone())
                    .font(font)
                    .color(ink)
                    .group_color(t.kit.accent_foreground)
                    .truncate(),
            };
            let mut r = row()
                .key(format!("{key}-item-{i}"))
                .role("option")
                .label(item.title.clone())
                .checked(checked)
                .px(pad_x)
                .py(pad_y)
                .gap(rems(0.25))
                .center_y()
                .justify_between()
                .radius(radius::KIT)
                .child(
                    row()
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .center_y()
                        .child(content),
                )
                .child(
                    glyph(Glyph::Check, KitSize::XSmall.icon_px())
                        .color(ink)
                        .group_color(t.kit.accent_foreground)
                        .opacity(if checked { 1.0 } else { 0.0 })
                        .a11y_hidden(),
                );
            if item.disabled {
                r = r.cursor(Cursor::NotAllowed).disabled(true);
            } else {
                r = if cursor {
                    r.fill(t.kit.accent)
                } else {
                    r.hover_group().hover(fade(t.kit.accent, 0.7))
                };
                r = self.bind(r, Act::Pick(i));
            }
            rows = rows.child(r);
        }
        let list = if vis.is_empty() {
            match self.empty.take() {
                Some(e) => e,
                None => row()
                    .key(format!("{key}-empty"))
                    .w_full()
                    .justify_center()
                    .py(rems(1.5))
                    .child(glyph(Glyph::Inbox, 28.0).color(fade(t.kit.muted_foreground, 0.6))),
            }
        } else {
            col()
                .key(format!("{key}-list"))
                .w_full()
                .max_h(self.menu_max_h)
                .min_h(0.0)
                .shrink()
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(rows)
        };
        let mut surface = col()
            .key(format!("{key}-menu"))
            .role("listbox")
            .w(self.menu_width.unwrap_or(self.state.trigger_w.max(1.0)))
            .min_h(0.0)
            .fill(t.kit.popover)
            .radius(radius::KIT)
            .clip_rounded(radius::KIT);
        for s in shadow::kit_popover(t) {
            surface = surface.box_shadow(s);
        }
        if self.state.searchable {
            let search = Input::new(format!("{key}-search"), &self.state.query)
                .appearance(false)
                .px(0.0)
                .size(size)
                .cleanable(true)
                .placeholder(
                    self.search_placeholder
                        .clone()
                        .unwrap_or_else(|| "Search...".into()),
                )
                .prefix(
                    glyph(Glyph::KitSearch, KitSize::Medium.icon_px())
                        .color(t.kit.muted_foreground),
                );
            let search = match &self.wire {
                Some(w) => w(el(), Act::Search(search, t, &key), self.on_change.as_ref()),
                None => search.build(t),
            };
            surface = surface
                .child(
                    row()
                        .key(format!("{key}-search-bar"))
                        .px(if size == KitSize::Small {
                            rems(0.375)
                        } else {
                            rems(0.5)
                        })
                        .border_sides([0.0, 0.0, 1.0, 0.0], t.kit.border)
                        .pb(1.0)
                        .child(search),
                )
                .child(list);
        } else {
            surface = surface.child(list);
            surface = self.bind(
                surface.focusable(true).autofocus().no_focus_ring(),
                Act::Keys(&key),
            );
        }
        surface
    }
}

/// gpui-kit's dropdown entrance (`dropdown_popup`): the surface fades in while it slides the
/// last 8pt down out of the trigger's edge, springing into place on [`motion::POP`] with a
/// slight bounce; no exit motion.
fn dropdown_appear() -> Transition {
    motion::appear(-8.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Mods, Runtime};

    struct S {
        fruit: SelectState,
        changes: Vec<Option<String>>,
    }

    fn items() -> Vec<SelectItem> {
        vec![
            SelectItem::new("apple", "Apple").group("Fruit"),
            SelectItem::new("banana", "Banana").group("Fruit"),
            SelectItem::new("kale", "Kale").group("Leaves"),
            SelectItem::new("leek", "Leek")
                .group("Leaves")
                .disabled(true),
        ]
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let t = Theme::dark();
        let mut layer = Vec::new();
        let body = col().padding(20.0).child(
            Select::new("fruit", &s.fruit)
                .w(200.0)
                .cleanable(true)
                .on_state(|s: &mut S| &mut s.fruit)
                .on_change(|s: &mut S, v: Option<&str>| s.changes.push(v.map(str::to_owned)))
                .build(t, &mut layer),
        );
        crate::root(body, layer)
    }

    fn rt(searchable: bool) -> Runtime<S> {
        let s = S {
            fruit: SelectState::new(items(), None).searchable(searchable),
            changes: vec![],
        };
        let mut rt = Runtime::new(
            s,
            view,
            (500.0, 500.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        rt
    }

    fn click(rt: &mut Runtime<S>, k: &str) {
        let (x, y) = rt.find(k).unwrap_or_else(|| panic!("{k}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
    }

    fn key(rt: &mut Runtime<S>, key: Key) {
        rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn keyboard_chooses_and_escape_closes_without_change() {
        let mut rt = rt(false);
        click(&mut rt, "fruit");
        assert!(rt.state.fruit.is_open());
        assert!(rt.find("fruit-menu").is_some());
        key(&mut rt, Key::Down);
        key(&mut rt, Key::Down);
        key(&mut rt, Key::Enter);
        assert!(!rt.state.fruit.is_open());
        assert_eq!(rt.state.fruit.selected_value(), Some("banana"));
        assert_eq!(rt.state.changes, [Some("banana".to_owned())]);
        // Escape closes and leaves the choice alone.
        click(&mut rt, "fruit");
        key(&mut rt, Key::Down);
        key(&mut rt, Key::Escape);
        assert!(!rt.state.fruit.is_open());
        assert_eq!(rt.state.fruit.selected_value(), Some("banana"));
        assert_eq!(rt.state.fruit.cursor(), Some(1));
        assert_eq!(rt.state.changes.len(), 1);
    }

    #[test]
    fn clicking_an_item_chooses_it_and_disabled_items_refuse() {
        let mut rt = rt(false);
        click(&mut rt, "fruit");
        click(&mut rt, "fruit-item-3");
        assert!(rt.state.fruit.is_open());
        assert_eq!(rt.state.fruit.selected_value(), None);
        click(&mut rt, "fruit-item-2");
        assert!(!rt.state.fruit.is_open());
        assert_eq!(rt.state.fruit.selected_value(), Some("kale"));
        // The clear button drops the choice.
        click(&mut rt, "fruit-clear");
        assert_eq!(rt.state.fruit.selected_value(), None);
        assert!(!rt.state.fruit.is_open());
        assert_eq!(rt.state.changes, [Some("kale".to_owned()), None]);
    }

    #[test]
    fn search_filters_and_enter_takes_the_first_match() {
        let mut rt = rt(true);
        click(&mut rt, "fruit");
        assert!(rt.state.fruit.query.is_focused());
        key(&mut rt, Key::Text("an".into()));
        assert_eq!(rt.state.fruit.visible(), [1]);
        assert!(rt.find("fruit-item-0").is_none());
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.fruit.selected_value(), Some("banana"));
        assert_eq!(rt.state.fruit.query.value(), "");
        // A query nothing matches shows the empty state.
        click(&mut rt, "fruit");
        key(&mut rt, Key::Text("zzz".into()));
        assert!(rt.find("fruit-empty").is_some());
        key(&mut rt, Key::Escape);
        assert!(!rt.state.fruit.is_open());
        assert_eq!(rt.state.fruit.selected_value(), Some("banana"));
    }

    #[test]
    fn set_items_keeps_the_choice_by_value() {
        let mut st = SelectState::new(items(), Some(2));
        st.set_items(items().into_iter().rev().collect());
        assert_eq!(st.selected_value(), Some("kale"));
        assert_eq!(st.selected_index(), Some(1));
        st.set_items(vec![SelectItem::new("x", "X")]);
        assert_eq!(st.selected_value(), None);
    }
}

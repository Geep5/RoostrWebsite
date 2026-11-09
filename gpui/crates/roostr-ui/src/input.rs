//! Text fields: gpui-kit's `Input` + `InputState` (one line) and `Textarea` + `TextareaState`
//! (several lines, optionally growing with the text), drawn on Rune's `text_input`.
//!
//! The state lives in the app's state (`InputState { edit: TextEdit }`); a field binds to it with
//! [`Input::on_edit`], an accessor from the app state to its `InputState`:
//!
//! ```ignore
//! Input::new("name", &s.name)
//!     .placeholder("Property name")
//!     .on_edit(|s: &mut S| &mut s.name)
//!     .on_submit(|s: &mut S, _| s.save())
//!     .on_key(|s: &mut S, ev, _| ev.key == Key::Escape && s.cancel())
//!     .build(t)
//! ```
//!
//! gpui-kit's events map to handlers: `InputEvent::Change` → [`Input::on_change`],
//! `InputEvent::PressEnter` → [`Input::on_submit`], and the `Enter` / `Escape` / `MoveUp` /
//! `MoveDown` actions screens bound on the field → [`Input::on_key`] (which sees each key before
//! the editor and consumes it by returning `true`).

use std::any::Any;
use std::rc::Rc;

use rune::{Color, Cursor, El, Key, KeyEvent, TextEdit, TextStyle, Tx, col, el, row, text_input};

use crate::KitSize;
use crate::icons::{Glyph, glyph};
use crate::theme::{Theme, radius, rems, ty};

/// gpui-kit's input line height (`Rems(1.25)`): 17.5.
pub(crate) const INPUT_LINE: f32 = rems(1.25);
/// Pins a closure's signature to an accessor from the app state into a part of it (closures
/// returning a borrow don't infer the higher-ranked lifetime on their own).
pub(crate) fn lens<S, T: ?Sized, F: for<'a> Fn(&'a mut S) -> &'a mut T>(f: F) -> F {
    f
}

/// GPUI's `Hsla::opacity`: `c` with its alpha multiplied by `f`.
pub(crate) fn fade(c: Color, f: f32) -> Color {
    c.alpha(c.a * f)
}

/// gpui-kit's `Theme::input_background`: `input` at 30% (oklab mix with transparent, which
/// keeps the colour and scales alpha) in a dark theme, `background` in a light one.
pub(crate) fn input_background(t: &Theme) -> Color {
    if t.dark {
        fade(t.kit.input, 0.3)
    } else {
        t.kit.background
    }
}

/// gpui-kit's `input_style` fill: [`input_background`], or `input` at 80% when disabled.
pub(crate) fn input_fill(t: &Theme, disabled: bool) -> Color {
    if disabled {
        fade(t.kit.input, 0.8)
    } else {
        input_background(t)
    }
}

/// `StyleSized::input_h`: h_11 / h_8 / h_6 / h_5 (rems), h_6 for a pixel size.
pub(crate) fn input_h(s: KitSize) -> f32 {
    match s {
        KitSize::Large => rems(2.75),
        KitSize::Medium => rems(2.0),
        KitSize::Small => rems(1.5),
        KitSize::XSmall => rems(1.25),
        _ => rems(1.5),
    }
}

/// `Size::input_px` (points).
pub(crate) fn input_px(s: KitSize) -> f32 {
    match s {
        KitSize::Large => 12.0,
        KitSize::Medium => 10.0,
        KitSize::Small => 8.0,
        KitSize::XSmall => 4.0,
        _ => 8.0,
    }
}

/// `Size::input_py` (points).
pub(crate) fn input_py(s: KitSize) -> f32 {
    match s {
        KitSize::Large => 10.0,
        KitSize::Medium => 8.0,
        KitSize::Small => 2.0,
        KitSize::XSmall => 0.0,
        _ => 2.0,
    }
}

/// `StyleSized::input_text_size`: text_xs / text_sm / text_sm / text_base, 0.875 × a pixel size.
pub(crate) fn input_text(s: KitSize) -> f32 {
    match s {
        KitSize::XSmall => rems(0.75),
        KitSize::Small | KitSize::Medium => rems(0.875),
        KitSize::Large => rems(1.0),
        KitSize::Px(p) => p * 0.875,
    }
}

/// The gap between an input's prefix, text and suffix.
fn input_gap(s: KitSize) -> f32 {
    match s {
        KitSize::Small => 4.0,
        KitSize::Large => 8.0,
        _ => 6.0,
    }
}

/// gpui-kit's xsmall `.text()` icon button (the clear button, the password eye): a bare 10.5pt
/// icon in the foreground at 90%, the full foreground under the pointer.
pub(crate) fn kit_text_icon_button(t: &Theme, key: String, g: Glyph, label: &str) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .center()
        .hover_group()
        .cursor(Cursor::Pointer)
        .child(
            glyph(g, rems(0.75))
                .color(fade(t.kit.foreground, 0.9))
                .group_color(t.kit.foreground)
                .a11y_hidden(),
        )
}

/// A single-line field's model: gpui-kit's `InputState`. Keep it in the app state.
#[derive(Clone, Default)]
pub struct InputState {
    /// The text, selection and focus (Rune's model; read `edit.text` or [`Self::value`]).
    pub edit: TextEdit,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `InputState::default_value`.
    pub fn with_value(value: impl Into<String>) -> Self {
        Self {
            edit: TextEdit::new(value),
        }
    }

    /// `InputState::masked(true)`: a password field (bullets, no copy, no undo).
    pub fn masked(mut self, masked: bool) -> Self {
        self.edit.set_secure(masked);
        self
    }

    pub fn value(&self) -> &str {
        &self.edit.text
    }

    /// `set_value`: replaces the text, keeping focus.
    pub fn set_value(&mut self, value: impl Into<String>) {
        self.edit.replace_text(value);
    }

    /// `clean`: empties the field, keeping focus.
    pub fn clear(&mut self) {
        self.edit.clear();
    }

    /// `focus`: the field takes keyboard focus on the next frame.
    pub fn focus(&mut self) {
        self.edit.focused = true;
    }

    pub fn is_focused(&self) -> bool {
        self.edit.focused
    }

    /// Whether the text shows as bullets (masked and not revealed by the eye toggle).
    pub fn is_masked(&self) -> bool {
        self.edit.is_secure() && !self.edit.is_revealed()
    }

    /// `set_masked`: hides or shows a masked field's text.
    pub fn set_masked(&mut self, masked: bool) {
        self.edit.set_revealed(!masked);
    }

    /// `select_all`.
    pub fn select_all(&mut self) {
        let n = self.edit.text.len();
        self.edit.set_selection(0, n);
    }
}

/// A multi-line field's model: gpui-kit's `TextareaState`, with its row count or auto-grow
/// range and whether Enter submits.
#[derive(Clone)]
pub struct TextareaState {
    pub edit: TextEdit,
    rows: usize,
    grow: Option<(usize, usize)>,
    submit_on_enter: bool,
}

impl Default for TextareaState {
    fn default() -> Self {
        Self {
            edit: TextEdit::new(""),
            rows: 1,
            grow: None,
            submit_on_enter: false,
        }
    }
}

impl TextareaState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_value(value: impl Into<String>) -> Self {
        Self {
            edit: TextEdit::new(value),
            ..Self::default()
        }
    }

    /// At least `rows` lines tall (default 1).
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows.max(1);
        self.grow = None;
        self
    }

    /// Grow with the text from `min_rows` to `max_rows` lines, then scroll.
    pub fn auto_grow(mut self, min_rows: usize, max_rows: usize) -> Self {
        let min = min_rows.max(1);
        self.grow = Some((min, max_rows.max(min)));
        self
    }

    /// Enter submits ([`Textarea::on_submit`]) and Shift+Enter inserts a newline; otherwise
    /// both insert a newline (the default).
    pub fn submit_on_enter(mut self, submit: bool) -> Self {
        self.submit_on_enter = submit;
        self
    }

    pub fn value(&self) -> &str {
        &self.edit.text
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        self.edit.replace_text(value);
    }

    pub fn clear(&mut self) {
        self.edit.clear();
    }

    pub fn focus(&mut self) {
        self.edit.focused = true;
    }

    pub fn is_focused(&self) -> bool {
        self.edit.focused
    }

    /// Puts the caret at byte `at` (`set_selected_range(at..at)`).
    pub fn set_caret(&mut self, at: usize) {
        self.edit.set_caret(at);
    }
}

type EditAccess<S> = Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut TextEdit>;
type Handler<S> = Rc<dyn Fn(&mut S, &mut Tx)>;
type KeyHandler<S> = Rc<dyn Fn(&mut S, &KeyEvent, &mut Tx) -> bool>;

/// The parts of a field its handlers attach to.
struct Parts {
    field: El,
    clear: Option<El>,
    mask: Option<El>,
    readonly: bool,
    /// Plain Enter inserts a newline (a textarea that doesn't submit on Enter).
    newline: bool,
}

/// A field's handlers, held without naming the app state until build, where `attach` (set by
/// the first handler method, monomorphised for its state type) puts them on the parts.
#[derive(Default)]
struct Binds {
    access: Option<Rc<dyn Any>>,
    submit: Option<Rc<dyn Any>>,
    change: Option<Rc<dyn Any>>,
    key: Option<Rc<dyn Any>>,
    attach: Option<fn(&Binds, Parts) -> Parts>,
}

impl Binds {
    fn get<T: Clone + 'static>(slot: &Option<Rc<dyn Any>>) -> Option<T> {
        slot.as_ref().map(|a| {
            a.downcast_ref::<T>()
                .expect("one app state type per field")
                .clone()
        })
    }

    fn access<S: 'static>(&mut self, f: EditAccess<S>) {
        self.access = Some(Rc::new(f));
        self.attach = Some(attach::<S>);
    }
    fn submit<S: 'static>(&mut self, f: Handler<S>) {
        self.submit = Some(Rc::new(f));
        self.attach = Some(attach::<S>);
    }
    fn change<S: 'static>(&mut self, f: Handler<S>) {
        self.change = Some(Rc::new(f));
        self.attach = Some(attach::<S>);
    }
    fn key<S: 'static>(&mut self, f: KeyHandler<S>) {
        self.key = Some(Rc::new(f));
        self.attach = Some(attach::<S>);
    }

    fn apply(&self, parts: Parts, enabled: bool) -> Parts {
        match self.attach {
            Some(attach) if enabled => attach(self, parts),
            _ => parts,
        }
    }
}

/// Whether `ev` would change a field's text (refused while read-only).
fn edits_text(ev: &KeyEvent) -> bool {
    match &ev.key {
        Key::Text(_) | Key::Backspace | Key::Delete | Key::Enter => true,
        Key::Command(c) => matches!(c.to_ascii_lowercase(), 'x' | 'v' | 'z' | 'y'),
        _ => false,
    }
}

fn attach<S: 'static>(b: &Binds, mut p: Parts) -> Parts {
    let access = Binds::get::<EditAccess<S>>(&b.access);
    let submit = Binds::get::<Handler<S>>(&b.submit);
    let change = Binds::get::<Handler<S>>(&b.change);
    let key = Binds::get::<KeyHandler<S>>(&b.key);
    if let Some(a) = access.clone() {
        p.field = p.field.on_edit(rune::edit_access(move |s: &mut S| a(s)));
    }
    if let Some(f) = submit {
        p.field = p.field.on_submit(move |s: &mut S, tx: &mut Tx| f(s, tx));
    }
    if let Some(f) = change.clone() {
        p.field = p
            .field
            .on_input_change(move |s: &mut S, tx: &mut Tx| f(s, tx));
    }
    let (readonly, newline) = (p.readonly, p.newline);
    if key.is_some() || readonly || newline {
        let a = access.clone();
        p.field = p
            .field
            .on_input_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
                if let Some(f) = &key
                    && f(s, ev, tx)
                {
                    return true;
                }
                if readonly && edits_text(ev) {
                    return true;
                }
                if newline && ev.key == Key::Enter && !ev.mods.cmd && !ev.mods.shift {
                    if let Some(a) = &a {
                        a(s).insert("\n");
                    }
                    return true;
                }
                false
            });
    }
    if readonly {
        p.field = p.field.on_input_paste(|_: &mut S, _, _| true);
    }
    if let (Some(clear), Some(a)) = (p.clear.take(), access.clone()) {
        p.clear = Some(clear.on_press_tx(move |s: &mut S, tx: &mut Tx| {
            let e = a(s);
            e.clear();
            e.focused = true;
            if let Some(f) = &change {
                f(s, tx);
            }
        }));
    }
    if let (Some(mask), Some(a)) = (p.mask.take(), access) {
        p.mask = Some(mask.on_press(move |s: &mut S| {
            let e = a(s);
            let revealed = e.is_revealed();
            e.set_revealed(!revealed);
        }));
    }
    p
}

/// Style overrides screens put on a kit field (its `Styled` refinements).
#[derive(Clone, Copy, Default)]
struct Look {
    h: Option<f32>,
    w: Option<f32>,
    px: Option<f32>,
    py: Option<f32>,
    fill: Option<Color>,
    border: Option<Color>,
    radius: Option<f32>,
    text_size: Option<f32>,
    weight: Option<f32>,
    line_height: Option<f32>,
    color: Option<Color>,
}

macro_rules! look_methods {
    () => {
        /// Height in points (`.h(..)`), in place of the size's.
        pub fn h(mut self, h: f32) -> Self {
            self.look.h = Some(h);
            self
        }
        /// Width in points; fills its parent's width otherwise (`size_full`).
        pub fn w(mut self, w: f32) -> Self {
            self.look.w = Some(w);
            self
        }
        /// Horizontal padding inside the border, in place of the size's (`.px(..)`; `.p_0()` is 0).
        pub fn px(mut self, px: f32) -> Self {
            self.look.px = Some(px);
            self
        }
        /// Vertical padding inside the border (`.py(..)`).
        pub fn py(mut self, py: f32) -> Self {
            self.look.py = Some(py);
            self
        }
        /// The fill, in place of the kit's input background (`.bg(..)`).
        pub fn fill(mut self, c: Color) -> Self {
            self.look.fill = Some(c);
            self
        }
        /// The border colour, in place of `input` (`.border_color(..)`).
        pub fn border_color(mut self, c: Color) -> Self {
            self.look.border = Some(c);
            self
        }
        /// Corner radius, in place of the kit's 6 (`.rounded(..)`).
        pub fn radius(mut self, r: f32) -> Self {
            self.look.radius = Some(r);
            self
        }
        /// Text size in points, in place of the size's (`.text_size(..)`); the line stays 17.5.
        pub fn text_size(mut self, size: f32) -> Self {
            self.look.text_size = Some(size);
            self
        }
        /// Font weight (`.font_weight(..)`).
        pub fn weight(mut self, w: f32) -> Self {
            self.look.weight = Some(w);
            self
        }
        /// Line height in points (`.line_height(..)`), in place of 17.5.
        pub fn line_height(mut self, lh: f32) -> Self {
            self.look.line_height = Some(lh);
            self
        }
        /// Text colour, in place of the foreground (`.text_color(..)`).
        pub fn text_color(mut self, c: Color) -> Self {
            self.look.color = Some(c);
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
        pub fn placeholder(mut self, p: impl Into<String>) -> Self {
            self.placeholder = p.into();
            self
        }
        /// `appearance(false)`: no fill, border, radius or focus ring.
        pub fn appearance(mut self, on: bool) -> Self {
            self.appearance = on;
            self
        }
        /// `bordered(false)`: no border (nor its focus tint).
        pub fn bordered(mut self, on: bool) -> Self {
            self.bordered = on;
            self
        }
        /// `focus_bordered(false)` / `focus_ring(false)`: no ring while focused.
        pub fn focus_bordered(mut self, on: bool) -> Self {
            self.focus_bordered = on;
            self
        }
        pub fn disabled(mut self, d: bool) -> Self {
            self.disabled = d;
            self
        }
        /// Focusable and selectable, but the text can't change.
        pub fn readonly(mut self, r: bool) -> Self {
            self.readonly = r;
            self
        }
        /// The text changed (`InputEvent::Change`).
        pub fn on_change<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
            self.binds.change(Rc::new(f) as Handler<S>);
            self
        }
        /// Each key while focused, before the editor; return `true` to consume it (the
        /// `Enter` / `Escape` / `MoveUp` / `MoveDown` actions screens bound on the field).
        pub fn on_key<S: 'static>(
            mut self,
            f: impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool + 'static,
        ) -> Self {
            self.binds.key(Rc::new(f) as KeyHandler<S>);
            self
        }
        /// Enter (`InputEvent::PressEnter`).
        pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
            self.binds.submit(Rc::new(f) as Handler<S>);
            self
        }
    };
}

/// The bordered kit frame both fields share: fill, border, radius, and while focused the ring
/// border and the 3pt `ring` halo at 50% (`focus_ring_style`).
fn frame(
    t: &Theme,
    e: El,
    look: &Look,
    appearance: bool,
    bordered: bool,
    ring: bool,
    disabled: bool,
) -> El {
    if !appearance {
        return e;
    }
    let fill = look.fill.unwrap_or_else(|| input_fill(t, disabled));
    let fill = if disabled { fade(fill, 0.5) } else { fill };
    let mut e = e.fill(fill).radius(look.radius.unwrap_or(radius::KIT));
    if bordered {
        e = e.border(
            1.0,
            if ring {
                t.kit.ring
            } else {
                look.border.unwrap_or(t.kit.input)
            },
        );
    }
    if ring {
        e = e.outline(3.0, 0.0, fade(t.kit.ring, 0.5));
    }
    e
}

fn field_style(size: KitSize, look: &Look) -> TextStyle {
    let style = TextStyle::new(look.text_size.unwrap_or(input_text(size)), ty::NORMAL)
        .lh(look.line_height.unwrap_or(INPUT_LINE));
    match look.weight {
        Some(w) => TextStyle { weight: w, ..style },
        None => style,
    }
}

/// gpui-kit's `Input` bound to an [`InputState`]: a bordered one-line field (h 28, px 10 at
/// Medium, radius 6), optional prefix/suffix, clear button and password eye.
pub struct Input {
    key: String,
    edit: TextEdit,
    placeholder: String,
    size: KitSize,
    appearance: bool,
    bordered: bool,
    focus_bordered: bool,
    cleanable: bool,
    mask_toggle: bool,
    disabled: bool,
    readonly: bool,
    prefix: Option<El>,
    suffix: Option<El>,
    look: Look,
    binds: Binds,
}

impl Input {
    pub fn new(key: impl Into<String>, state: &InputState) -> Self {
        Self {
            key: key.into(),
            edit: state.edit.clone(),
            placeholder: String::new(),
            size: KitSize::Medium,
            appearance: true,
            bordered: true,
            focus_bordered: true,
            cleanable: false,
            mask_toggle: false,
            disabled: false,
            readonly: false,
            prefix: None,
            suffix: None,
            look: Look::default(),
            binds: Binds::default(),
        }
    }

    look_methods!();

    /// Binds the field to its [`InputState`] in the app state.
    pub fn on_edit<S: 'static>(
        mut self,
        f: impl for<'a> Fn(&'a mut S) -> &'a mut InputState + 'static,
    ) -> Self {
        self.binds
            .access(Rc::new(rune::edit_access(move |s: &mut S| &mut f(s).edit)) as EditAccess<S>);
        self
    }

    /// A clear button (×) while the field has text (`cleanable(true)`).
    pub fn cleanable(mut self, on: bool) -> Self {
        self.cleanable = on;
        self
    }

    /// An eye button that shows or hides a masked field's text (`mask_toggle`).
    pub fn mask_toggle(mut self) -> Self {
        self.mask_toggle = true;
        self
    }

    /// An element before the text (an icon, say).
    pub fn prefix(mut self, e: El) -> Self {
        self.prefix = Some(e);
        self
    }

    /// An element after the text.
    pub fn suffix(mut self, e: El) -> Self {
        self.suffix = Some(e);
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let key = self.key;
        let disabled = self.disabled;
        let focused = self.edit.focused && !disabled;
        let style = field_style(self.size, &self.look);
        let ink = if disabled {
            t.kit.muted_foreground
        } else {
            self.look.color.unwrap_or(t.kit.foreground)
        };
        let mut field = text_input(&self.edit, style)
            .single_line()
            .key(format!("{key}-field"))
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .color(ink)
            .placeholder(self.placeholder.clone(), t.kit.muted_foreground)
            .caret(t.kit.caret, t.kit.selection)
            .no_focus_ring()
            .disabled(disabled)
            .read_only(self.readonly);
        if !self.placeholder.is_empty() {
            field = field.label(self.placeholder.clone());
        }
        if self.edit.is_secure() {
            field = field.revealed(self.edit.is_revealed());
        }
        let editable = !disabled && !self.readonly;
        let show_clear = self.cleanable && editable && !self.edit.text.is_empty();
        let clear = show_clear
            .then(|| kit_text_icon_button(t, format!("{key}-clear"), Glyph::Close, "Clear"));
        let mask = self.mask_toggle.then(|| {
            let masked = !self.edit.is_revealed();
            kit_text_icon_button(
                t,
                format!("{key}-mask"),
                if masked { Glyph::Eye } else { Glyph::EyeOff },
                if masked {
                    "Show Password"
                } else {
                    "Hide Password"
                },
            )
        });
        let parts = self.binds.apply(
            Parts {
                field,
                clear,
                mask,
                readonly: self.readonly,
                newline: false,
            },
            !disabled,
        );

        let gap = input_gap(self.size);
        let px = self.look.px.unwrap_or(input_px(self.size));
        let border = if self.appearance && self.bordered {
            1.0
        } else {
            0.0
        };
        let mut e = row()
            .key(key.clone())
            .role("textbox")
            .h(self.look.h.unwrap_or(input_h(self.size)))
            .px(px + border)
            .gap(gap)
            .center_y()
            .cursor(Cursor::Text);
        if let Some(py) = self.look.py {
            e = e.py(py + border);
        }
        e = match self.look.w {
            Some(w) => e.w(w),
            None => e.w_full(),
        };
        if let Some(p) = self.prefix {
            let p = el().key(format!("{key}-prefix")).center().child(p);
            e = e.child(if disabled { p.opacity(0.5) } else { p });
        }
        e = e.child(parts.field);
        let has_suffix = self.suffix.is_some() || parts.mask.is_some() || parts.clear.is_some();
        if has_suffix {
            let mut s = row()
                .key(format!("{key}-suffix"))
                .gap(gap)
                .center_y()
                .cursor(Cursor::Default);
            if let Some(m) = parts.mask {
                s = s.child(m);
            }
            if let Some(c) = parts.clear {
                s = s.child(c);
            }
            if let Some(x) = self.suffix {
                s = s.child(x);
            }
            e = e.child(if disabled { s.opacity(0.5) } else { s });
        }
        let ring = focused && self.focus_bordered && self.bordered;
        frame(
            t,
            e,
            &self.look,
            self.appearance,
            self.bordered,
            ring,
            disabled,
        )
    }
}

/// gpui-kit's `Textarea` bound to a [`TextareaState`]: the kit frame around a multi-line
/// editor padded by the size's input padding (8 × 10 at Medium), `rows` lines tall or growing
/// with the text.
pub struct Textarea {
    key: String,
    edit: TextEdit,
    rows: usize,
    grow: Option<(usize, usize)>,
    submit_on_enter: bool,
    placeholder: String,
    size: KitSize,
    appearance: bool,
    bordered: bool,
    focus_bordered: bool,
    disabled: bool,
    readonly: bool,
    look: Look,
    binds: Binds,
}

impl Textarea {
    pub fn new(key: impl Into<String>, state: &TextareaState) -> Self {
        Self {
            key: key.into(),
            edit: state.edit.clone(),
            rows: state.rows,
            grow: state.grow,
            submit_on_enter: state.submit_on_enter,
            placeholder: String::new(),
            size: KitSize::Medium,
            appearance: true,
            bordered: true,
            focus_bordered: true,
            disabled: false,
            readonly: false,
            look: Look::default(),
            binds: Binds::default(),
        }
    }

    look_methods!();

    /// Binds the field to its [`TextareaState`] in the app state.
    pub fn on_edit<S: 'static>(
        mut self,
        f: impl for<'a> Fn(&'a mut S) -> &'a mut TextareaState + 'static,
    ) -> Self {
        self.binds
            .access(Rc::new(rune::edit_access(move |s: &mut S| &mut f(s).edit)) as EditAccess<S>);
        self
    }

    pub fn build(mut self, t: &Theme) -> El {
        let key = self.key;
        let disabled = self.disabled;
        let focused = self.edit.focused && !disabled;
        let style = field_style(self.size, &self.look);
        let line = style.line_px();
        let ink = if disabled {
            t.kit.muted_foreground
        } else {
            self.look.color.unwrap_or(t.kit.foreground)
        };
        let mut field = text_input(&self.edit, style)
            .key(format!("{key}-field"))
            .w_full()
            .color(ink)
            .placeholder(self.placeholder.clone(), t.kit.muted_foreground)
            .caret(t.kit.caret, t.kit.selection)
            .no_focus_ring()
            .disabled(disabled)
            .read_only(self.readonly);
        if !self.placeholder.is_empty() {
            field = field.label(self.placeholder.clone());
        }
        if !self.submit_on_enter {
            self.binds.submit = None;
        }
        let parts = self.binds.apply(
            Parts {
                field,
                clear: None,
                mask: None,
                readonly: self.readonly,
                newline: !self.submit_on_enter,
            },
            !disabled,
        );
        let border = if self.appearance && self.bordered {
            1.0
        } else {
            0.0
        };
        let px = self.look.px.unwrap_or(input_px(self.size)) + border;
        let py = self.look.py.unwrap_or(input_py(self.size)) + border;
        let body = match self.grow {
            Some((min, max)) => rune::auto_grow(parts.field, min, max),
            None => parts.field,
        };
        let mut e = col()
            .key(key)
            .role("textbox")
            .px(px)
            .py(py)
            .cursor(Cursor::Text)
            .child(body);
        e = match self.look.h {
            Some(h) => e.h(h),
            None if self.grow.is_none() => e.min_h(self.rows as f32 * line + 2.0 * py),
            None => e,
        };
        e = match self.look.w {
            Some(w) => e.w(w),
            None => e.w_full(),
        };
        let ring = focused && self.focus_bordered && self.bordered;
        frame(
            t,
            e,
            &self.look,
            self.appearance,
            self.bordered,
            ring,
            disabled,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Mods, Runtime};

    #[derive(Default)]
    struct S {
        name: InputState,
        note: TextareaState,
        submitted: Vec<String>,
        escaped: usize,
        changes: usize,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let t = Theme::dark();
        col()
            .w(400.0)
            .padding(20.0)
            .gap(20.0)
            .child(
                Input::new("name", &s.name)
                    .placeholder("Name")
                    .cleanable(true)
                    .on_edit(|s: &mut S| &mut s.name)
                    .on_change(|s: &mut S, _| s.changes += 1)
                    .on_submit(|s: &mut S, _| {
                        let v = s.name.value().to_owned();
                        s.submitted.push(v);
                    })
                    .on_key(|s: &mut S, ev: &KeyEvent, _| {
                        if ev.key == Key::Escape {
                            s.escaped += 1;
                            return true;
                        }
                        false
                    })
                    .build(t),
            )
            .child(
                Textarea::new("note", &s.note)
                    .on_edit(|s: &mut S| &mut s.note)
                    .on_submit(|s: &mut S, _| {
                        let v = s.note.value().to_owned();
                        s.submitted.push(v);
                    })
                    .build(t),
            )
    }

    fn key(rt: &mut Runtime<S>, key: Key, mods: Mods) {
        rt.key(KeyEvent { key, mods });
        rt.frame(1.0 / 60.0);
    }

    fn click(rt: &mut Runtime<S>, k: &str) {
        let (x, y) = rt.find(k).unwrap_or_else(|| panic!("{k}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn runtime(s: S) -> Runtime<S> {
        let mut rt = Runtime::new(
            s,
            view,
            (600.0, 400.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        rt
    }

    #[test]
    fn input_edits_submits_and_hands_keys_to_the_screen() {
        let mut rt = runtime(S::default());
        click(&mut rt, "name-field");
        assert!(rt.state.name.is_focused());
        key(&mut rt, Key::Text("Ada".into()), Mods::default());
        assert_eq!(rt.state.name.value(), "Ada");
        assert!(rt.state.changes > 0);
        key(&mut rt, Key::Backspace, Mods::default());
        assert_eq!(rt.state.name.value(), "Ad");
        key(
            &mut rt,
            Key::Command('a'),
            Mods {
                cmd: true,
                ..Mods::default()
            },
        );
        key(&mut rt, Key::Text("Grace".into()), Mods::default());
        assert_eq!(rt.state.name.value(), "Grace");
        key(&mut rt, Key::Enter, Mods::default());
        assert_eq!(rt.state.submitted, ["Grace"]);
        key(&mut rt, Key::Escape, Mods::default());
        assert_eq!(rt.state.escaped, 1);
        assert_eq!(rt.state.name.value(), "Grace");
        // The clear button empties the field and keeps it focused.
        click(&mut rt, "name-clear");
        assert_eq!(rt.state.name.value(), "");
        assert!(rt.state.name.is_focused());
        assert!(rt.find("name-clear").is_none());
    }

    #[test]
    fn textarea_breaks_lines_on_enter_unless_it_submits() {
        let mut rt = runtime(S::default());
        click(&mut rt, "note-field");
        key(&mut rt, Key::Text("one".into()), Mods::default());
        key(
            &mut rt,
            Key::Enter,
            Mods {
                shift: true,
                ..Mods::default()
            },
        );
        key(&mut rt, Key::Text("two".into()), Mods::default());
        assert_eq!(rt.state.note.value(), "one\ntwo");
        // Without submit_on_enter, plain Enter breaks the line too and submits nothing.
        key(&mut rt, Key::Enter, Mods::default());
        assert_eq!(rt.state.note.value(), "one\ntwo\n");
        assert!(rt.state.submitted.is_empty());

        let mut rt = runtime(S {
            note: TextareaState::new().auto_grow(1, 4).submit_on_enter(true),
            ..S::default()
        });
        click(&mut rt, "note-field");
        key(&mut rt, Key::Text("a".into()), Mods::default());
        key(
            &mut rt,
            Key::Enter,
            Mods {
                shift: true,
                ..Mods::default()
            },
        );
        key(&mut rt, Key::Text("b".into()), Mods::default());
        key(&mut rt, Key::Enter, Mods::default());
        assert_eq!(rt.state.note.value(), "a\nb");
        assert_eq!(rt.state.submitted, ["a\nb"]);
    }

    #[test]
    fn readonly_keeps_the_text() {
        fn ro(s: &S, _: &mut Cx<S>) -> El {
            Input::new("ro", &s.name)
                .readonly(true)
                .on_edit(|s: &mut S| &mut s.name)
                .build(Theme::light())
        }
        let mut rt = Runtime::new(
            S {
                name: InputState::with_value("fixed"),
                ..S::default()
            },
            ro,
            (300.0, 100.0),
            Env {
                system_dark: false,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        let (x, y) = rt.find("ro-field").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.key(KeyEvent {
            key: Key::Text("x".into()),
            mods: Mods::default(),
        });
        rt.key(KeyEvent {
            key: Key::Backspace,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.name.value(), "fixed");
    }
}

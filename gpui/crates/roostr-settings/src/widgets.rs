//! Small styled pieces the settings screens share: the browser-default
//! controls of Settings.svelte / SpaceManage.svelte / KeyGate.svelte as
//! their CSS draws them (buttons, switch, text fields, hints, headings).

use std::rc::Rc;

use roostr_ui::color::rgb;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Input, InputState, Theme, motion, ty};
use rune::{
    Align, Color, Cursor, Effect, El, Span as RuneSpan, Spring, SpringPair, TextStyle, Transition,
    Tx, el, row, text,
};

/// `#f55522`: the danger hover / armed colour of these screens.
pub fn danger() -> Color {
    rgb(0xf55522)
}

/// Uppercase section heading (`h3`: 11px, muted, bold).
pub fn h3(t: &Theme, s: impl Into<String>) -> El {
    let s: String = s.into();
    text(s.to_uppercase())
        .font(ty::ui(11.0).lh(14.0).weight(ty::BOLD))
        .color(t.muted)
}

/// Muted 12px helper text (`.hint`, line-height 1.5). It may shrink and wrap
/// inside a row, so a long hint never runs past a narrow sheet's edge.
pub fn hint(t: &Theme, s: impl Into<String>) -> El {
    text(s.into())
        .font(ty::ui(12.0).lh(18.0))
        .color(t.muted)
        .wrap()
        .min_w(0.0)
        .shrink()
}

/// A transient confirmation ("Saved", "Copied ✓"): rises in over 150ms
/// (opacity, y 6 → 0) and fades out over 200ms once `text` is empty (Rune
/// keeps a removed keyed element drawn until its exit settles).
pub fn flash(key: &str, s: &str, build: impl FnOnce(String) -> El) -> Option<El> {
    if s.is_empty() {
        return None;
    }
    Some(
        el().key(key.to_owned())
            .child(build(s.to_owned()))
            .transition(Transition {
                insertion: Effect {
                    offset: rune::Vec2::new(0.0, 6.0),
                    ..Effect::opacity(0.0)
                },
                removal: Effect::opacity(0.0),
                springs: SpringPair::new(
                    motion::CONTROL.spring(),
                    Spring::new(motion::Tween::ease(200).duration, 0.0),
                ),
                ..Transition::identity()
            }),
    )
}

type Press<S> = Rc<dyn Fn(&mut S, &mut Tx)>;

/// A bordered text button as the web CSS draws it.
pub struct Btn<S> {
    key: String,
    label: String,
    leading: Option<El>,
    bg: Option<Color>,
    border: Option<Color>,
    color: Color,
    radius: f32,
    px: f32,
    py: f32,
    size: f32,
    weight: f32,
    hover_border: Option<Color>,
    hover_color: Option<Color>,
    hover_bg: Option<Color>,
    disabled_opacity: f32,
    disabled: bool,
    full_width: bool,
    tooltip: Option<String>,
    on_press: Option<Press<S>>,
}

impl<S: 'static> Btn<S> {
    fn base(t: &Theme, key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            leading: None,
            bg: Some(t.bg),
            border: Some(t.border),
            color: t.fg,
            radius: 8.0,
            px: 14.0,
            py: 7.0,
            size: 13.0,
            weight: ty::NORMAL,
            hover_border: Some(t.accent),
            hover_color: None,
            hover_bg: None,
            disabled_opacity: 0.5,
            disabled: false,
            full_width: false,
            tooltip: None,
            on_press: None,
        }
    }

    /// Settings `.action`: bg, border, radius 8, 7×14, 13px; hover accent border.
    pub fn action(t: &Theme, key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::base(t, key, label)
    }

    /// Settings `.keyrow button` / `.relay button` / `form button`: radius 7, 5×12, 12px.
    pub fn small(t: &Theme, key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            radius: 7.0,
            px: 12.0,
            py: 5.0,
            size: 12.0,
            ..Self::base(t, key, label)
        }
    }

    /// SpaceManage `button`: panel bg, radius 7, 4×10, 12px.
    pub fn manage(t: &Theme, key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            bg: Some(t.panel),
            radius: 7.0,
            px: 10.0,
            py: 4.0,
            size: 12.0,
            ..Self::base(t, key, label)
        }
    }

    /// The `×` remove button (`.x`): borderless, muted 16px, hover `#f55522`.
    pub fn x(t: &Theme, key: impl Into<String>) -> Self {
        Self {
            bg: None,
            border: None,
            color: t.muted,
            px: 4.0,
            py: 0.0,
            size: 16.0,
            hover_border: None,
            hover_color: Some(danger()),
            ..Self::base(t, key, "×")
        }
    }

    /// Solid red confirm (`.del-btn`): white 13px/600, radius 6, disabled at 35%.
    pub fn destructive(t: &Theme, key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            bg: Some(t.red),
            border: None,
            color: t.white,
            radius: 6.0,
            weight: ty::SEMIBOLD,
            hover_border: None,
            disabled_opacity: 0.35,
            ..Self::base(t, key, label)
        }
    }

    /// Muted label (`.subtle`).
    pub fn subtle(mut self, t: &Theme) -> Self {
        self.color = t.muted;
        self
    }

    /// SpaceManage `.danger`: hover turns border and text `#f55522`.
    pub fn danger_hover(mut self) -> Self {
        self.hover_border = Some(danger());
        self.hover_color = Some(danger());
        self
    }

    /// Settings `.action.danger` (armed logout): `#f55522` text, tinted border.
    pub fn armed(mut self) -> Self {
        self.color = danger();
        self.border = Some(danger().alpha(0.4));
        self
    }

    /// Applies `f` when `cond` holds (GPUI's `.when`).
    pub fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }

    pub fn leading(mut self, e: El) -> Self {
        self.leading = Some(e);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Stretch across the parent (a flex-column child in the web).
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn tooltip(mut self, s: impl Into<String>) -> Self {
        self.tooltip = Some(s.into());
        self
    }

    pub fn on_press(self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_press_tx(move |s: &mut S, _: &mut Tx| f(s))
    }

    pub fn on_press_tx(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.on_press = Some(Rc::new(f));
        self
    }

    pub fn colors(mut self, bg: Option<Color>, border: Option<Color>, color: Color) -> Self {
        self.bg = bg;
        self.border = border;
        self.color = color;
        self
    }

    pub fn padding(mut self, x: f32, y: f32) -> Self {
        self.px = x;
        self.py = y;
        self
    }

    pub fn text_style(mut self, size: f32, weight: f32) -> Self {
        self.size = size;
        self.weight = weight;
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    pub fn hover(mut self, border: Option<Color>, color: Option<Color>, bg: Option<Color>) -> Self {
        self.hover_border = border;
        self.hover_color = color;
        self.hover_bg = bg;
        self
    }

    pub fn build(self, _t: &Theme) -> El {
        let disabled = self.disabled;
        let line = (self.size * 1.25).round();
        let border_w = if self.border.is_some() { 1.0 } else { 0.0 };
        let mut label = text(self.label.clone())
            .font(TextStyle::new(self.size, self.weight).lh(line))
            .color(self.color);
        if let Some(c) = self.hover_color.filter(|_| !disabled) {
            label = label.group_color(c);
        }
        let mut b = row()
            .key(self.key)
            .role("button")
            .label(self.label)
            .center()
            .gap(6.0)
            .px(self.px)
            .py(self.py)
            .radius(self.radius)
            .hover_group();
        if let Some(bg) = self.bg {
            b = b.fill(bg);
        }
        if let Some(border) = self.border {
            b = b.border(border_w, border);
        }
        if self.full_width {
            b = b.w_full();
        }
        if let Some(lead) = self.leading {
            b = b.child(lead);
        }
        b = b.child(label);
        if !disabled {
            // Hover border / fill: a ring laid over the edge that shows with the group's
            // hover (Rune's hover is a fill overlay; CSS here changes the border colour).
            if let Some(c) = self.hover_border {
                b = b.child(
                    el().absolute(0.0, 0.0)
                        .w_full()
                        .h_full()
                        .radius(self.radius)
                        .border(1.0, c)
                        .group_opacity(0.0, 1.0)
                        .a11y_hidden(),
                );
            }
            if let Some(c) = self.hover_bg {
                b = b.hover(c);
            }
            b = b.cursor(Cursor::Pointer);
        } else {
            b = b.opacity(self.disabled_opacity).disabled(true);
        }
        if let Some(tip) = self.tooltip {
            b = roostr_ui::tooltip(b, tip);
        }
        match self.on_press.filter(|_| !disabled) {
            // Shrinks while held: a full-width button as a row does.
            Some(f) => {
                let b = if self.full_width {
                    motion::pressable_row(b)
                } else {
                    motion::pressable(b)
                };
                b.focusable(true).on_press_tx(move |s: &mut S, tx| f(s, tx))
            }
            None => b,
        }
    }
}

/// The settings toggle (`.switch`): the kit switch in the accent colour, the
/// knob springing between states. `toggle` gets the requested state.
pub fn switch<S: 'static>(
    t: &Theme,
    key: impl Into<String>,
    on: bool,
    toggle: impl Fn(&mut S, bool) + 'static,
) -> El {
    roostr_ui::Switch::new(key)
        .checked(on)
        .color(t.accent)
        .on_change(toggle)
        .build(t)
}

/// How a text field is drawn.
#[derive(Clone, Copy)]
pub struct FieldStyle {
    pub bg: Color,
    pub radius: f32,
    pub px: f32,
    pub py: f32,
    pub size: f32,
    /// Focus border colour, and its 3px ring.
    pub focus: Color,
    pub ring: Option<Color>,
    pub centered: bool,
}

impl FieldStyle {
    /// `.modal input`: panel bg, radius 6, 6×9, 13px, accent focus + 40% ring.
    pub fn modal(t: &Theme) -> Self {
        Self {
            bg: t.panel,
            radius: 6.0,
            px: 9.0,
            py: 6.0,
            size: 13.0,
            focus: t.accent,
            ring: Some(t.accent.alpha(0.4)),
            centered: false,
        }
    }
}

/// A single-line text field over a roostr-ui [`InputState`] with web chrome.
pub struct Field {
    input: Input,
    focused: bool,
    style: FieldStyle,
    key: String,
}

impl Field {
    pub fn new(key: impl Into<String>, state: &InputState, style: FieldStyle) -> Self {
        let key = key.into();
        let line = (style.size * 1.25).round();
        Self {
            input: Input::new(format!("{key}-input"), state)
                .appearance(false)
                .px(0.0)
                .h(line)
                .text_size(style.size)
                .line_height(line),
            focused: state.is_focused(),
            style,
            key,
        }
    }

    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.input = self.input.placeholder(p);
        self
    }

    /// Binds the field to its [`InputState`] in the app state.
    pub fn on_edit<S: 'static>(
        mut self,
        f: impl for<'a> Fn(&'a mut S) -> &'a mut InputState + 'static,
    ) -> Self {
        self.input = self.input.on_edit(f);
        self
    }

    /// Enter in the field (`InputEvent::PressEnter`).
    pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.input = self.input.on_submit(f);
        self
    }

    /// The text changed (`InputEvent::Change`).
    pub fn on_change<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.input = self.input.on_change(f);
        self
    }

    /// A password field's eye toggle.
    pub fn mask_toggle(mut self) -> Self {
        self.input = self.input.mask_toggle();
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let style = self.style;
        let line = (style.size * 1.25).round();
        let mut input = self.input.text_color(t.fg).build(t);
        if style.centered {
            input = input.align(Align::Center);
        }
        let mut e = row()
            .key(self.key)
            .center_y()
            .min_w(0.0)
            .h(line + style.py * 2.0 + 2.0)
            .px(style.px + 1.0)
            .radius(style.radius)
            .fill(style.bg)
            .border(1.0, if self.focused { style.focus } else { t.border })
            .child(input);
        if self.focused
            && let Some(ring) = style.ring
        {
            e = e.outline(3.0, 0.0, ring);
        }
        e
    }
}

/// The spinning ring of "Checking…" / sign-in waits (`.desk-spin`).
pub fn spinner(t: &Theme, color: Color) -> El {
    el().mr(6.0)
        .child(roostr_ui::Spinner::new().px(10.0).color(color).build(t))
}

/// One run of inline-formatted text.
#[derive(Clone, Copy)]
pub enum Span<'a> {
    Plain(&'a str),
    /// `<b>` / `<strong>`.
    Bold(&'a str),
    /// `<code>`: monospace on a faint highlight, in the foreground colour.
    Code(&'a str),
}

/// Wrapped text mixing plain, bold and code runs (an HTML paragraph), at
/// `size` / `line` in `color`.
pub fn rich(t: &Theme, spans: &[Span], color: Color, size: f32, line: f32) -> El {
    let mut s = String::new();
    let mut runs = Vec::with_capacity(spans.len());
    for span in spans {
        let (part, f): (&str, fn(RuneSpan, &Theme) -> RuneSpan) = match *span {
            Span::Plain(p) => (p, |r, _| r),
            Span::Bold(p) => (p, |r, _| r.weight(ty::BOLD)),
            Span::Code(p) => (p, |r, t| {
                let mut r = r;
                r.mono = true;
                r.color = Some(t.fg);
                r.bg = Some(t.hl_med);
                r
            }),
        };
        let start = s.len();
        s.push_str(part);
        runs.push(f(RuneSpan::new(start, s.len()), t));
    }
    rune::rich(s, runs)
        .font(ty::ui(size).lh(line))
        .color(color)
        .min_w(0.0)
        .shrink()
}

/// One `<option>`: its value and the label shown.
pub type SelectOption = (String, String);

/// A `<select>` over roostr-ui's [`roostr_ui::Select`]: a bordered trigger
/// showing the current option and a menu of the options below it. Its open
/// state lives in the app state behind `lens`; `on_pick` hears picks.
pub fn select<S: 'static>(
    t: &Theme,
    layer: &mut Vec<El>,
    key: &str,
    state: &roostr_ui::SelectState,
    lens: impl for<'a> Fn(&'a mut S) -> &'a mut roostr_ui::SelectState + 'static,
    min_w: f32,
    bg: Color,
    on_pick: impl Fn(&mut S, &str, &mut Tx) + 'static,
) -> El {
    // GPUI's box sized the select to its content: the longest option at
    // 13px (about 0.62 em a character) plus padding, gap and chevron.
    let longest = state
        .items()
        .iter()
        .map(|i| i.title.chars().count())
        .max()
        .unwrap_or(0);
    let width = (longest as f32 * 13.0 * 0.62 + 10.0 * 2.0 + 8.0 + 12.0 + 6.0).max(min_w);
    roostr_ui::Select::new(key.to_owned(), state)
        .on_state(lens)
        .w(width)
        .on_change_tx(move |s: &mut S, v: Option<&str>, tx: &mut Tx| {
            if let Some(v) = v {
                on_pick(s, v, tx)
            }
        })
        .icon(roostr_ui::Glyph::ChevronDown)
        .menu_width(min_w.max(160.0))
        .menu_max_h(320.0)
        .h(30.0)
        .px(10.0)
        .py(6.0)
        .gap(8.0)
        .radius(8.0)
        .fill(bg)
        .border_color(t.border)
        .text_size(13.0)
        .build(t, layer)
}

/// The options of a select, kept in step with `options` and `selected`
/// without echoing a pick back.
pub fn sync_select(state: &mut roostr_ui::SelectState, options: &[SelectOption], selected: &str) {
    let same = state.items().len() == options.len()
        && state
            .items()
            .iter()
            .zip(options)
            .all(|(item, (v, l))| item.value == *v && item.title == *l);
    if !same {
        state.set_items(
            options
                .iter()
                .map(|(v, l)| roostr_ui::SelectItem::new(v.clone(), l.clone()))
                .collect(),
        );
    }
    if state.selected_value() != Some(selected) {
        match state.position(selected) {
            Some(_) => state.set_selected_value(selected),
            None => state.set_selected_index(None),
        }
    }
}

//! Small styled pieces the settings screens share: the browser-default
//! controls of Settings.svelte / SpaceManage.svelte / KeyGate.svelte as
//! their CSS draws them (buttons, switch, text fields, hints, headings).

use std::rc::Rc;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::{motion, theme, ui};

/// A full-window modal layer (`.del-overlay`: `rgb(0 0 0/0.55)`, centered)
/// painted above everything. The caller wires the backdrop's mouse-down
/// (cancel) and key handling; the content should `occlude()`.
pub fn window_overlay(id: impl Into<ElementId>, window: &Window) -> Stateful<Div> {
    div()
        .id(id)
        .occlude()
        .w(window.viewport_size().width)
        .h(window.viewport_size().height)
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(0., 0., 0., 0.55))
}

/// Paint `layer` (from [`window_overlay`]) over the whole window.
pub fn over_window(layer: impl IntoElement) -> Deferred {
    deferred(anchored().position(point(px(0.), px(0.))).child(layer)).with_priority(2)
}

/// `#f55522`: the danger hover / armed colour of these screens.
pub fn danger() -> Hsla {
    rgb(0xf55522).into()
}

/// Uppercase section heading (`h3`: 11px, muted, bold).
pub fn h3(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .text_size(px(11.))
        .line_height(px(14.))
        .font_weight(FontWeight::BOLD)
        .text_color(theme::muted())
        .child(SharedString::from(text.to_uppercase()))
}

/// Muted 12px helper text (`.hint`, line-height 1.5).
pub fn hint(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(theme::muted())
        .child(text.into())
}

/// A transient confirmation ("Saved", "Copied ✓") that rises in over 150ms
/// (opacity, y 6 → 0) and fades out over 200ms after its flag clears. It is
/// never interactive, so the exit may outlive the flag: the owning view keeps
/// one per flash and remembers the last text for the exit to draw.
#[derive(Default)]
pub struct Flash {
    state: motion::MotionState,
    text: SharedString,
}

impl Flash {
    /// The presence declaration: shown or leaving.
    pub fn motion(shown: bool) -> motion::Motion {
        let hidden = motion::Target::default().y(6.).opacity(0.);
        motion::Motion::new(motion::Target::default())
            .exit(hidden)
            .transition(
                if shown {
                    motion::CONTROL
                } else {
                    motion::Tween::ease(200)
                }
                .transition(),
            )
            .present(shown)
    }

    /// The flash for `text` ("" = hidden), each frame; `build` styles the
    /// text. Draws nothing once the exit has settled.
    pub fn render<E: IntoElement>(
        &mut self,
        text: impl Into<SharedString>,
        build: impl FnOnce(SharedString) -> E,
        window: &mut Window,
        cx: &App,
    ) -> AnyElement {
        let text = text.into();
        let shown = !text.is_empty();
        if shown {
            self.text = text;
        }
        let frame = self.state.frame(Self::motion(shown), window, cx);
        let label = self.text.clone();
        frame.render(|frame| frame.apply(div().child(build(label))))
    }
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A bordered text button as the web CSS draws it.
#[derive(IntoElement)]
pub struct Btn {
    id: ElementId,
    label: SharedString,
    leading: Option<AnyElement>,
    bg: Option<Hsla>,
    border: Option<Hsla>,
    color: Hsla,
    radius: Pixels,
    px: Pixels,
    py: Pixels,
    size: Pixels,
    weight: FontWeight,
    hover_border: Option<Hsla>,
    hover_color: Option<Hsla>,
    hover_bg: Option<Hsla>,
    disabled_opacity: f32,
    disabled: bool,
    full_width: bool,
    centered: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl Btn {
    fn base(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            leading: None,
            bg: Some(theme::bg()),
            border: Some(theme::border()),
            color: theme::fg(),
            radius: px(8.),
            px: px(14.),
            py: px(7.),
            size: px(13.),
            weight: FontWeight::NORMAL,
            hover_border: Some(theme::accent()),
            hover_color: None,
            hover_bg: None,
            disabled_opacity: 0.5,
            disabled: false,
            full_width: false,
            centered: true,
            tooltip: None,
            on_click: None,
        }
    }

    /// Settings `.action`: bg, border, radius 8, 7×14, 13px; hover accent border.
    pub fn action(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self::base(id, label)
    }

    /// Settings `.keyrow button` / `.relay button` / `form button`: radius 7, 5×12, 12px.
    pub fn small(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            radius: px(7.),
            px: px(12.),
            py: px(5.),
            size: px(12.),
            ..Self::base(id, label)
        }
    }

    /// SpaceManage `button`: panel bg, radius 7, 4×10, 12px.
    pub fn manage(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            bg: Some(theme::panel()),
            radius: px(7.),
            px: px(10.),
            py: px(4.),
            size: px(12.),
            ..Self::base(id, label)
        }
    }

    /// The `×` remove button (`.x`): borderless, muted 16px, hover `#f55522`.
    pub fn x(id: impl Into<ElementId>) -> Self {
        Self {
            bg: None,
            border: None,
            color: theme::muted(),
            px: px(4.),
            py: px(0.),
            size: px(16.),
            hover_border: None,
            hover_color: Some(danger()),
            ..Self::base(id, "×")
        }
    }

    /// Solid red confirm (`.del-btn`): white 13px/600, radius 6, disabled at 35%.
    pub fn destructive(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            bg: Some(theme::red()),
            border: None,
            color: theme::white(),
            radius: px(6.),
            weight: FontWeight::SEMIBOLD,
            hover_border: None,
            disabled_opacity: 0.35,
            ..Self::base(id, label)
        }
    }

    /// Muted label (`.subtle`), 6px above.
    pub fn subtle(mut self) -> Self {
        self.color = theme::muted();
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
        self.border = Some(rgba(0xf5552266).into());
        self
    }

    pub fn style(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }

    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
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

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    pub fn set_colors(&mut self, bg: Option<Hsla>, border: Option<Hsla>, color: Hsla) {
        self.bg = bg;
        self.border = border;
        self.color = color;
    }

    pub fn set_padding(&mut self, x: f32, y: f32) {
        self.px = px(x);
        self.py = px(y);
    }

    pub fn set_text(&mut self, size: f32, weight: FontWeight) {
        self.size = px(size);
        self.weight = weight;
    }

    pub fn set_radius(&mut self, radius: f32) {
        self.radius = px(radius);
    }

    pub fn set_hover(&mut self, border: Option<Hsla>, color: Option<Hsla>, bg: Option<Hsla>) {
        self.hover_border = border;
        self.hover_color = color;
        self.hover_bg = bg;
    }
}

impl RenderOnce for Btn {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let disabled = self.disabled;
        let line = (self.size * 1.25).round();
        let mut el = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .px(self.px)
            .py(self.py)
            .rounded(self.radius)
            .text_size(self.size)
            .line_height(line)
            .font_weight(self.weight)
            .text_color(self.color)
            .when(self.centered, |d| d.justify_center())
            .when(self.full_width, |d| d.w_full())
            .when_some(self.bg, |d, bg| d.bg(bg))
            .when_some(self.border, |d, border| d.border_1().border_color(border))
            .when(disabled, |d| d.opacity(self.disabled_opacity))
            .when(!disabled, |d| d.cursor_pointer())
            .children(self.leading)
            .child(self.label);
        if !disabled {
            let (hb, hc, hbg) = (self.hover_border, self.hover_color, self.hover_bg);
            el = el.hover(move |s| {
                let s = match hb {
                    Some(c) => s.border_color(c),
                    None => s,
                };
                let s = match hc {
                    Some(c) => s.text_color(c),
                    None => s,
                };
                match hbg {
                    Some(c) => s.bg(c),
                    None => s,
                }
            });
            if let Some(handler) = self.on_click {
                el = el.on_click(move |event, window, cx| handler(event, window, cx));
            }
        }
        if let Some(text) = self.tooltip {
            el = el.tooltip(ui::tooltip(text));
        }
        el
    }
}

/// The settings toggle (`.switch`): 34×20 track, 16px white knob, both
/// easing between states (`transition: left 0.15s ease` / `background 0.15s`).
pub fn switch(
    id: impl Into<ElementId>,
    on: bool,
    toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id = id.into();
    let t = motion::progress(id.clone(), on, motion::CONTROL, window, cx);
    div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(34.))
        .h(px(20.))
        .rounded_full()
        .cursor_pointer()
        .bg(motion::mix(theme::border(), theme::accent(), t))
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(2. + 14. * t))
                .size(px(16.))
                .rounded_full()
                .bg(theme::white()),
        )
        .on_click(move |_, window, cx| toggle(!on, window, cx))
}

/// How a text field is drawn.
#[derive(Clone, Copy)]
pub struct FieldStyle {
    pub bg: Hsla,
    pub radius: f32,
    pub px: f32,
    pub py: f32,
    pub size: f32,
    /// Focus border colour, and its 3px ring.
    pub focus: Hsla,
    pub ring: Option<Hsla>,
    pub centered: bool,
}

impl FieldStyle {
    /// `.modal input`: panel bg, radius 6, 6×9, 13px, accent focus + 40% ring.
    pub fn modal() -> Self {
        let mut ring = theme::accent();
        ring.a = 0.4;
        Self {
            bg: theme::panel(),
            radius: 6.,
            px: 9.,
            py: 6.,
            size: 13.,
            focus: theme::accent(),
            ring: Some(ring),
            centered: false,
        }
    }
}

/// A single-line text field over a gpui-kit [`InputState`] with web chrome.
pub fn field(state: &Entity<InputState>, style: FieldStyle, window: &Window, cx: &App) -> Div {
    let focused = state.focus_handle(cx).is_focused(window);
    let line = (style.size * 1.25).round();
    div()
        .flex()
        .items_center()
        .min_w_0()
        .h(px(line + style.py * 2. + 2.))
        .px(px(style.px))
        .rounded(px(style.radius))
        .bg(style.bg)
        .border_1()
        .border_color(if focused {
            style.focus
        } else {
            theme::border()
        })
        .when(focused, |d| match style.ring {
            Some(ring) => d.shadow(vec![BoxShadow {
                color: ring,
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(3.),
                inset: false,
            }]),
            None => d,
        })
        .text_size(px(style.size))
        .child(
            Input::new(state)
                .appearance(false)
                .px_0()
                .py_0()
                .h(px(line))
                .text_size(px(style.size))
                .when(style.centered, |i| i.text_center()),
        )
}

/// The spinning ring of "Checking…" / sign-in waits (`.desk-spin`).
pub fn spinner(color: Hsla) -> impl IntoElement {
    use gpui_kit::component::Sizable as _;
    div().flex_none().mr(px(6.)).child(
        gpui_kit::component::spinner::Spinner::new()
            .color(color)
            .with_size(px(10.)),
    )
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

/// Wrapped text mixing plain, bold and code runs (an HTML paragraph).
pub fn rich(spans: &[Span], color: Hsla, window: &Window) -> StyledText {
    let base = window.text_style().font();
    let mut text = String::new();
    let mut runs = Vec::with_capacity(spans.len());
    for span in spans {
        let (s, font, run_color, background) = match *span {
            Span::Plain(s) => (s, base.clone(), color, None),
            Span::Bold(s) => (
                s,
                Font {
                    weight: FontWeight::BOLD,
                    ..base.clone()
                },
                color,
                None,
            ),
            Span::Code(s) => (
                s,
                Font {
                    family: theme::fonts::MONO.into(),
                    ..base.clone()
                },
                theme::fg(),
                Some(theme::hl_med()),
            ),
        };
        text.push_str(s);
        runs.push(TextRun {
            len: s.len(),
            font,
            color: run_color,
            background_color: background,
            underline: None,
            strikethrough: None,
        });
    }
    StyledText::new(text).with_runs(runs)
}

type PickHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;
type OpenHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A `<select>`: a bordered trigger showing the current option and a menu
/// of the options below it. The host keeps `open` and flips it in `on_open`.
#[derive(IntoElement)]
pub struct Select {
    id: ElementId,
    options: Vec<(String, SharedString)>,
    selected: String,
    open: bool,
    bg: Hsla,
    min_w: Pixels,
    on_open: Option<OpenHandler>,
    on_pick: Option<PickHandler>,
}

impl Select {
    pub fn new(
        id: impl Into<ElementId>,
        options: Vec<(String, SharedString)>,
        selected: impl Into<String>,
        open: bool,
    ) -> Self {
        Self {
            id: id.into(),
            options,
            selected: selected.into(),
            open,
            bg: theme::bg(),
            min_w: px(0.),
            on_open: None,
            on_pick: None,
        }
    }

    pub fn bg(mut self, bg: Hsla) -> Self {
        self.bg = bg;
        self
    }

    pub fn min_w(mut self, min_w: Pixels) -> Self {
        self.min_w = min_w;
        self
    }

    pub fn on_open(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(f));
        self
    }

    pub fn on_pick(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Select {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let label = self
            .options
            .iter()
            .find(|(v, _)| *v == self.selected)
            .map(|(_, l)| l.clone())
            .unwrap_or_default();
        let on_open = self.on_open.clone();
        let open = self.open;
        let trigger = div()
            .id(self.id.clone())
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .min_w(self.min_w)
            .px(px(10.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(self.bg)
            .border_1()
            .border_color(theme::border())
            .text_size(px(13.))
            .line_height(px(16.))
            .cursor_pointer()
            .child(label)
            .child(
                gpui_kit::component::Icon::new(roostr_core::Glyph::ChevronDown)
                    .size(px(12.))
                    .text_color(theme::muted()),
            )
            .on_click(move |_, window, cx| {
                if let Some(f) = &on_open {
                    f(!open, window, cx);
                }
            });
        let menu = self.open.then(|| {
            let on_open = self.on_open.clone();
            let items = self.options.iter().enumerate().map(|(ix, (value, label))| {
                let value = value.clone();
                let checked = value == self.selected;
                let on_pick = self.on_pick.clone();
                let on_open = self.on_open.clone();
                ui::MenuItem::new(ix, label.clone())
                    .icon(div().w(px(14.)).flex_none().when(checked, |d| {
                        d.child(
                            gpui_kit::component::Icon::new(roostr_core::Glyph::Check).size(px(12.)),
                        )
                    }))
                    .on_click(move |_, window, cx| {
                        if let Some(f) = &on_open {
                            f(false, window, cx);
                        }
                        if let Some(f) = &on_pick {
                            f(&value, window, cx);
                        }
                    })
            });
            deferred(
                anchored().snap_to_window_with_margin(px(8.)).child(
                    ui::menu_panel()
                        .id((self.id.clone(), "menu"))
                        .occlude()
                        .mt(px(4.))
                        .min_w(self.min_w.max(px(160.)))
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .on_mouse_down_out(move |_, window, cx| {
                            if let Some(f) = &on_open {
                                f(false, window, cx);
                            }
                        })
                        .children(items),
                ),
            )
            .with_priority(1)
        });
        // The anchor sits in a zero-height box under the trigger, so the menu opens below it.
        div()
            .flex()
            .flex_col()
            .child(trigger)
            .child(div().h_0().children(menu))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Flash, motion};

    #[test]
    fn flash_hidden_on_mount_then_rises_and_fades() {
        let mut state = motion::MotionState::default();
        let at = Duration::from_millis;
        assert!(
            !state
                .sample(Flash::motion(false), at(0), false)
                .is_mounted()
        );
        let shown = state.sample(Flash::motion(true), at(10), false);
        assert!(shown.is_mounted() && shown.is_animating());
        assert!(shown.target().alpha() < 0.5 && shown.target().y_px() > 3.);
        let settled = state.sample(Flash::motion(true), at(10 + 150), false);
        assert_eq!(
            (settled.target().y_px(), settled.target().alpha()),
            (0., 1.)
        );
        assert!(
            state
                .sample(Flash::motion(false), at(200), false)
                .is_mounted()
        );
        assert!(
            !state
                .sample(Flash::motion(false), at(200 + 200), false)
                .is_mounted()
        );
    }

    #[test]
    fn flash_snaps_under_reduced_motion() {
        let mut state = motion::MotionState::default();
        let at = Duration::from_millis;
        state.sample(Flash::motion(false), at(0), true);
        let shown = state.sample(Flash::motion(true), at(1), true);
        assert_eq!((shown.target().y_px(), shown.target().alpha()), (0., 1.));
        assert!(!shown.is_animating());
        assert!(!state.sample(Flash::motion(false), at(2), true).is_mounted());
    }
}

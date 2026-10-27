//! Small styled pieces the settings screens share: the browser-default
//! controls of Settings.svelte / SpaceManage.svelte / KeyGate.svelte as
//! their CSS draws them (buttons, switch, text fields, hints, headings).

use std::rc::Rc;

use gpui_kit::component::IndexPath;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{self, SelectEvent, SelectState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::{motion, theme};

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

/// Muted 12px helper text (`.hint`, line-height 1.5). It may shrink and wrap
/// inside a row, so a long hint never runs past a narrow sheet's edge.
pub fn hint(text: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
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
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
        use gpui_kit::component::{Disableable as _, Sizable as _, Size};
        let disabled = self.disabled;
        let line = (self.size * 1.25).round();
        let bg = self.bg.unwrap_or(transparent_black());
        // The kit paints the hover state from the variant: border from
        // `color`, text from `foreground`, fill from `hover`. The resting
        // look is the instance style below, which the kit layers on top.
        let hover_fill = self.hover_bg.unwrap_or(bg);
        let hover = ButtonCustomVariant::new(cx)
            .color(
                self.hover_border
                    .or(self.border)
                    .unwrap_or(transparent_black()),
            )
            .foreground(self.hover_color.unwrap_or(self.color))
            .hover(hover_fill)
            .active(hover_fill);
        Button::new(self.id)
            .custom(hover)
            // A free size: no fixed height, so the web padding sets it.
            .with_size(Size::Size(self.size))
            .rounded(self.radius)
            .accessibility_label(self.label.clone())
            .disabled(disabled)
            .flex_none()
            .px(self.px)
            .py(self.py)
            .bg(bg)
            .text_color(self.color)
            .when_some(self.border, |b, border| b.border_1().border_color(border))
            .when(self.full_width, |b| b.w_full())
            .when(disabled, |b| b.opacity(self.disabled_opacity))
            .when(!disabled, |b| b.cursor_pointer())
            // Leading slot first, then the label (the kit puts children after
            // its own label, so both go in one row of ours).
            .child(
                div()
                    .flex()
                    .items_center()
                    .when(self.centered, |d| d.justify_center())
                    .gap(px(6.))
                    .text_size(self.size)
                    .line_height(line)
                    .font_weight(self.weight)
                    .children(self.leading)
                    .child(self.label),
            )
            .when_some(self.tooltip, |b, text| b.tooltip(text))
            .when_some(self.on_click.filter(|_| !disabled), |b, handler| {
                b.on_click(move |event, window, cx| handler(event, window, cx))
            })
    }
}

/// The settings toggle (`.switch`): the kit switch in the accent colour, the
/// knob springing between states. `toggle` gets the requested state.
pub fn switch(
    id: impl Into<ElementId>,
    on: bool,
    toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> gpui_kit::component::switch::Switch {
    gpui_kit::component::switch::Switch::new(id)
        .checked(on)
        .color(theme::accent())
        .on_change(move |next, window, cx| toggle(*next, window, cx))
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

/// One `<option>`: its value and the label shown.
#[derive(Clone)]
struct SelectOption {
    value: String,
    label: SharedString,
}

impl SearchableListItem for SelectOption {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &String {
        &self.value
    }
}

type OptionList = SearchableVec<SelectOption>;

fn option_list(options: &[(String, SharedString)]) -> OptionList {
    SearchableVec::new(
        options
            .iter()
            .map(|(value, label)| SelectOption {
                value: value.clone(),
                label: label.clone(),
            })
            .collect::<Vec<_>>(),
    )
}

/// Per-select element state: the kit state, the options it holds, and the
/// latest pick handler its confirm subscription calls.
struct SelectHost {
    state: Entity<SelectState<OptionList>>,
    options: Vec<(String, SharedString)>,
    on_pick: Option<PickHandler>,
    _confirm: Subscription,
}

/// A `<select>` over the kit [`select::Select`]: a bordered trigger showing
/// the current option and a menu of the options below it. The kit owns the
/// open state (outside click and Escape close it); the host keeps the value
/// and is told of picks through `on_pick`.
#[derive(IntoElement)]
pub struct Select {
    id: ElementId,
    options: Vec<(String, SharedString)>,
    selected: String,
    bg: Hsla,
    min_w: Pixels,
    on_pick: Option<PickHandler>,
}

impl Select {
    pub fn new(
        id: impl Into<ElementId>,
        options: Vec<(String, SharedString)>,
        selected: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            options,
            selected: selected.into(),
            bg: theme::bg(),
            min_w: px(0.),
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

    pub fn on_pick(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Select {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            id,
            options,
            selected,
            bg,
            min_w,
            on_pick,
        } = self;
        let host = window.use_keyed_state((id.clone(), "host"), cx, |window, cx| {
            let ix = options.iter().position(|(v, _)| *v == selected);
            let state = cx.new(|cx| {
                SelectState::new(option_list(&options), ix.map(IndexPath::new), window, cx)
            });
            let confirm = cx.subscribe_in(
                &state,
                window,
                |host: &mut SelectHost, _, event: &SelectEvent<OptionList>, window, cx| {
                    let SelectEvent::Confirm(Some(value)) = event else {
                        return;
                    };
                    if let Some(f) = host.on_pick.clone() {
                        f(value, window, cx);
                    }
                },
            );
            SelectHost {
                state,
                options: options.clone(),
                on_pick: None,
                _confirm: confirm,
            }
        });
        // Follow the host's options and value without echoing a pick back.
        let state = host.update(cx, |host, cx| {
            host.on_pick = on_pick;
            let state = host.state.clone();
            let reselect = host.options != options;
            if reselect {
                host.options = options.clone();
                state.update(cx, |s, cx| s.set_items(option_list(&options), window, cx));
            }
            let want = options
                .iter()
                .any(|(v, _)| *v == selected)
                .then_some(&selected);
            if reselect || state.read(cx).selected_value() != want {
                state.update(cx, |s, cx| match want {
                    Some(value) => s.set_selected_value(value, window, cx),
                    None => s.set_selected_index(None, window, cx),
                });
            }
            state
        });
        // The kit select fills its parent; this box sizes it to its content.
        div().flex_none().min_w(min_w).child(
            select::Select::new(&state)
                .id(id)
                .icon(gpui_kit::component::Icon::new(roostr_core::Glyph::ChevronDown).size(px(12.)))
                .menu_width(min_w.max(px(160.)))
                .menu_max_h(px(320.))
                .min_w(min_w)
                .h(px(30.))
                .px(px(10.))
                .py(px(6.))
                .gap(px(8.))
                .rounded(px(8.))
                .bg(bg)
                .border_color(theme::border())
                .text_size(px(13.))
                .line_height(px(16.))
                .cursor_pointer(),
        )
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

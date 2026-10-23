//! Small modals standing in for the web's `prompt()` / `alert()` (New space,
//! New type, the type-key clash) on gpui-kit's [`Dialog`], skinned like the
//! delete-property modal (`.prop-del-modal`: panel, radius 12, padding 20,
//! width min(420, vw−40), centred), plus the shared outline button.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{
    Dialog, DialogAction, DialogClose, DialogEntrance, DialogFooter,
};
use gpui_kit::component::input::{Enter, Escape, Input, InputState};
use gpui_kit::component::{ElementExt as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::{motion, theme};

actions!(roostr_shell_prompt, [Cancel, Confirm]);

const CONTEXT: &str = "ShellPrompt";

/// The card's height before its first layout (message, field, buttons).
const FIRST_HEIGHT: Pixels = px(150.);

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
    ]);
}

pub(crate) enum PromptEvent {
    /// The entered text (alerts submit "").
    Submit(String),
    Cancel,
}

/// A prompt or alert. It lives in the window's dialog layer (opened on
/// creation, closed by its own Submit / Cancel); the entity is the state
/// that layer renders and the event source the opener subscribes to.
pub(crate) struct Prompt {
    message: SharedString,
    input: Option<Entity<InputState>>,
    focus: FocusHandle,
    /// The card's laid-out height, to seat it in the window's middle.
    height: Rc<Cell<Pixels>>,
}

impl EventEmitter<PromptEvent> for Prompt {}

impl Prompt {
    /// `prompt(message)`: a text field with Cancel / OK.
    pub(crate) fn ask(
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        Self::open(cx.weak_entity(), window, cx);
        input.update(cx, |input, cx| input.focus(window, cx));
        Self {
            message: message.into(),
            input: Some(input),
            focus: cx.focus_handle(),
            height: Rc::new(Cell::new(FIRST_HEIGHT)),
        }
    }

    /// `alert(message)`: the message and OK.
    pub(crate) fn alert(
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        Self::open(cx.weak_entity(), window, cx);
        window.focus(&focus, cx);
        Self {
            message: message.into(),
            input: None,
            focus,
            height: Rc::new(Cell::new(FIRST_HEIGHT)),
        }
    }

    /// Push the dialog onto the window's dialog layer; it renders `this`
    /// (weak: the opener owns the prompt, and the prompt closes the dialog
    /// whenever it answers).
    fn open(this: WeakEntity<Self>, window: &mut Window, cx: &mut App) {
        window.open_dialog(cx, move |dialog, window, cx| match this.upgrade() {
            Some(this) => Self::dialog(&this, dialog, window, cx),
            // Dropped without answering: take the layer down.
            None => {
                window.defer(cx, |window, cx| window.close_dialog(cx));
                dialog.close_button(false)
            }
        });
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self
            .input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        cx.emit(PromptEvent::Submit(text));
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.input.is_some() {
            cx.emit(PromptEvent::Cancel);
        } else {
            cx.emit(PromptEvent::Submit(String::new()));
        }
    }

    /// Answer from a key: emit, then take the dialog down (the dialog's own
    /// OK / Cancel / backdrop paths close it themselves).
    fn answer(this: &Entity<Self>, submit: bool, window: &mut Window, cx: &mut App) {
        this.update(cx, |this, cx| {
            if submit {
                this.submit(cx)
            } else {
                this.cancel(cx)
            }
        });
        window.close_dialog(cx);
    }

    /// The card: the message, the field (prompts), Cancel (prompts) and OK.
    /// It rises 8px into place ([`dialog_appear`]) while the layer, dim
    /// included, fades in.
    fn dialog(this: &Entity<Self>, dialog: Dialog, window: &mut Window, cx: &mut App) -> Dialog {
        let enter = dialog_appear("prompt-appear", window, cx);
        let prompt = this.read(cx);
        let message = prompt.message.clone();
        let input = prompt.input.clone();
        let focus = prompt.focus.clone();
        let height = prompt.height.clone();
        let viewport = window.viewport_size();
        // Centred like the overlay's flex centring: the card's last height
        // (body + 2×20 padding + 2×1 border), then the rise.
        let card = height.get() + px(42.);
        let top = ((viewport.height - card) / 2.).max(px(0.)) + px(enter.target().y_px());
        let (ok, cancel) = (this.clone(), this.clone());
        let (key_ok, key_cancel) = (this.clone(), this.clone());
        let (enter_ok, escape_cancel) = (this.clone(), this.clone());
        dialog
            .entrance(DialogEntrance::Fade)
            .close_button(false)
            .w(card_width(window))
            .margin_top(top)
            .p(px(20.))
            .bg(theme::panel())
            .border_color(theme::border())
            .rounded(theme::radius::pane())
            .line_height(relative(1.2))
            .on_ok(move |_, _, cx| {
                ok.update(cx, |this, cx| this.submit(cx));
                true
            })
            .on_cancel(move |_, _, cx| {
                cancel.update(cx, |this, cx| this.cancel(cx));
                true
            })
            .child(
                div()
                    .id("prompt")
                    .key_context(CONTEXT)
                    .track_focus(&focus)
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .on_action(move |_: &Cancel, window, cx| {
                        Self::answer(&key_cancel, false, window, cx)
                    })
                    .on_action(move |_: &Confirm, window, cx| {
                        Self::answer(&key_ok, true, window, cx)
                    })
                    .capture_action(move |_: &Enter, window, cx| {
                        cx.stop_propagation();
                        Self::answer(&enter_ok, true, window, cx);
                    })
                    .capture_action(move |_: &Escape, window, cx| {
                        cx.stop_propagation();
                        Self::answer(&escape_cancel, false, window, cx);
                    })
                    .child(
                        div()
                            .text_size(px(16.))
                            .text_color(theme::fg())
                            .child(message),
                    )
                    .when_some(input.clone(), |d, input| d.child(Input::new(&input)))
                    .child(
                        DialogFooter::new()
                            .when(input.is_some(), |d| {
                                d.child(div().flex_none().child(
                                    DialogClose::new().child(outline_button(
                                        "prompt-cancel",
                                        "Cancel",
                                        false,
                                        cx,
                                    )),
                                ))
                            })
                            .child(div().flex_none().child(
                                DialogAction::new().child(outline_button(
                                    "prompt-ok",
                                    "OK",
                                    false,
                                    cx,
                                )),
                            )),
                    )
                    .on_prepaint(move |bounds, window, _| {
                        if height.get() != bounds.size.height {
                            height.set(bounds.size.height);
                            window.request_animation_frame();
                        }
                    }),
            )
    }
}

/// `.prop-del-row button`: 1px border, transparent, radius 8, padding 7×14,
/// 13px; hover fills `hover`. Danger: red border and text, hover fills red
/// with white text.
pub(crate) fn outline_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    danger: bool,
    cx: &App,
) -> Button {
    let label = label.into();
    let (color, border, fill, hover_fg) = if danger {
        (theme::red(), theme::red(), theme::red(), theme::white())
    } else {
        (theme::fg(), theme::border(), theme::hover(), theme::fg())
    };
    // The kit paints hover / press from the variant (fill, `color` as the
    // border, `foreground` as the text); the rest state is set below.
    let variant = ButtonCustomVariant::new(cx)
        .color(border)
        .foreground(hover_fg)
        .hover(fill)
        .active(fill)
        .shadow(false);
    Button::new(id)
        .custom(variant)
        .rounded(theme::radius::md())
        .accessibility_label(label.clone())
        .flex_none()
        .h_auto()
        .px(px(14.))
        .py(px(7.))
        .border_1()
        .border_color(border)
        .bg(transparent_black())
        .text_color(color)
        .line_height(relative(1.2))
        .cursor_pointer()
        .child(div().text_size(px(13.)).child(label))
}

/// The full-window dimmed overlay every modal sits on, its dim at `shown`
/// (0..=1) while it fades in.
pub(crate) fn overlay(id: impl Into<ElementId>, shown: f32) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(theme::overlay().opacity(shown))
        .occlude()
        .line_height(relative(1.2))
}

/// A dialog appearing: the card fades in rising 8px over [`motion::FADE`]
/// (the overlay's dim follows its opacity). It takes keys from its first
/// frame and closes at once.
pub(crate) fn dialog_appear(
    id: impl Into<ElementId>,
    window: &mut Window,
    cx: &mut App,
) -> motion::Frame {
    motion::appear_frame(id, 8., window, cx)
}

/// The small modal card's outer width: min(420, vw−40) of content plus
/// 2×20 padding and 2×1 border.
fn card_width(window: &Window) -> Pixels {
    (window.viewport_size().width - px(40.)).min(px(420.)) + px(42.)
}

/// The small modal card.
pub(crate) fn small_card(window: &Window) -> Div {
    div()
        .w(card_width(window))
        .p(px(20.))
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .rounded(theme::radius::pane())
        .flex()
        .flex_col()
        .gap(px(12.))
}

impl Render for Prompt {
    /// The card renders in the window's dialog layer ([`Self::open`]).
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

impl crate::workspace::Workspace {
    /// `alert(message)`.
    pub(crate) fn show_alert(
        &mut self,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let alert = cx.new(|cx| Prompt::alert(message, window, cx));
        cx.subscribe(&alert, |this, _, _: &PromptEvent, cx| {
            this.prompt = None;
            cx.notify();
        })
        .detach();
        self.prompt = Some(alert);
        cx.notify();
    }
}

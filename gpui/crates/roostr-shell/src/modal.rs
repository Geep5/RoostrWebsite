//! Small modals standing in for the web's `prompt()` / `alert()` (New space,
//! New type, the type-key clash) in the delete-property modal's style
//! (`.prop-del-modal`: panel, radius 12, padding 20, width min(420, vw−40)),
//! plus the shared outline button.

use gpui_kit::component::input::{Enter, Escape, Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::theme;

actions!(roostr_shell_prompt, [Cancel, Confirm]);

const CONTEXT: &str = "ShellPrompt";

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

pub(crate) struct Prompt {
    message: SharedString,
    input: Option<Entity<InputState>>,
    focus: FocusHandle,
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
        input.update(cx, |input, cx| input.focus(window, cx));
        Self {
            message: message.into(),
            input: Some(input),
            focus: cx.focus_handle(),
        }
    }

    /// `alert(message)`: the message and OK.
    pub(crate) fn alert(
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            message: message.into(),
            input: None,
            focus,
        }
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
}

/// `.prop-del-row button`: 1px border, transparent, radius 8, padding 7×14, 13px.
pub(crate) fn outline_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    danger: bool,
) -> Stateful<Div> {
    let color = if danger { theme::red() } else { theme::fg() };
    div()
        .id(id)
        .flex_none()
        .px(px(14.))
        .py(px(7.))
        .rounded(theme::radius::md())
        .border_1()
        .border_color(if danger {
            theme::red()
        } else {
            theme::border()
        })
        .text_size(px(13.))
        .text_color(color)
        .cursor_pointer()
        .map(|d| {
            if danger {
                d.hover(|s| s.bg(theme::red()).text_color(theme::white()))
            } else {
                d.hover(|s| s.bg(theme::hover()))
            }
        })
        .child(label.into())
}

/// The full-window dimmed overlay every modal sits on.
pub(crate) fn overlay(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(theme::overlay())
        .occlude()
        .line_height(relative(1.2))
}

/// The small modal card.
pub(crate) fn small_card(window: &Window) -> Div {
    let width = (window.viewport_size().width - px(40.)).min(px(420.)) + px(42.);
    div()
        .w(width)
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.input.clone();
        overlay("prompt-overlay")
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.cancel(cx)),
            )
            .child(
                small_card(window)
                    .id("prompt")
                    .key_context(CONTEXT)
                    .track_focus(&self.focus)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_action(cx.listener(|this, _: &Cancel, _, cx| this.cancel(cx)))
                    .on_action(cx.listener(|this, _: &Confirm, _, cx| this.submit(cx)))
                    .capture_action(cx.listener(|this, _: &Enter, _, cx| {
                        cx.stop_propagation();
                        this.submit(cx);
                    }))
                    .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                        cx.stop_propagation();
                        this.cancel(cx);
                    }))
                    .child(
                        div()
                            .text_size(px(16.))
                            .text_color(theme::fg())
                            .child(self.message.clone()),
                    )
                    .when_some(input, |d, input| d.child(Input::new(&input)))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .when(self.input.is_some(), |d| {
                                d.child(
                                    outline_button("prompt-cancel", "Cancel", false)
                                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                                )
                            })
                            .child(
                                outline_button("prompt-ok", "OK", false)
                                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                            ),
                    ),
            )
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

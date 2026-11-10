//! Small modals standing in for the web's `prompt()` / `alert()` (New space,
//! New type, the type-key clash, a failed create) and the card the
//! delete-property confirmation shares: [`small_dialog`], skinned as
//! `.prop-del-modal` (panel, 1px border, radius 12, padding 20, width
//! min(420, vw−40), centred over the dim, rising 8 pt as it fades in).

use roostr_core::State;
use roostr_ui::overlay;
use roostr_ui::{Input, InputState, Theme, motion, outline_button, radius, shadow, ty};
use rune::{Cx, El, Key, KeyEvent, Mods, Tx, col, el, row, text};

/// The open prompt or alert (one at a time; a new one replaces it).
#[derive(Default)]
struct Prompts {
    open: Option<Prompt>,
    /// The prompt's text field (shown only for a prompt).
    input: InputState,
}

struct Prompt {
    message: String,
    /// `prompt`: the field shows, and OK (or Enter) runs this with its text. `None`: an alert.
    on_submit: Option<fn(&mut State, String)>,
}

/// `prompt(message)`: a field with Cancel / OK; OK (or Enter) calls `on_submit` with the text.
/// Cancel, Escape and a press outside close it without calling anything.
pub(crate) fn ask(s: &mut State, message: impl Into<String>, on_submit: fn(&mut State, String)) {
    let prompts = s.slot::<Prompts>();
    prompts.input = InputState::new();
    prompts.input.focus();
    prompts.open = Some(Prompt {
        message: message.into(),
        on_submit: Some(on_submit),
    });
}

/// `alert(message)`: the message and OK (Escape, Enter and a press outside close it too).
pub(crate) fn alert(s: &mut State, message: impl Into<String>) {
    s.slot::<Prompts>().open = Some(Prompt {
        message: message.into(),
        on_submit: None,
    });
}

/// OK: close, then hand the field's text to the opener (an alert submits "" to nobody).
fn submit(s: &mut State) {
    let prompts = s.slot::<Prompts>();
    let Some(prompt) = prompts.open.take() else {
        return;
    };
    let text = prompts.input.value().to_string();
    if let Some(f) = prompt.on_submit {
        f(s, text);
    }
}

/// Cancel / Escape / outside press: a prompt closes unanswered; an alert answers "" (closes).
fn cancel(s: &mut State) {
    s.slot::<Prompts>().open = None;
}

/// The small modal card's outer width: min(420, vw−40) of content plus 2×20 padding and
/// 2×1 border (narrower windows: the window's width).
const CARD_MAX_W: f32 = 420.0 + 42.0;

/// The small modal card (`.prop-del-modal`) centred over a dim, holding `body`;
/// `dismiss` runs on a click outside or Escape (`None`: it can't be dismissed now).
///
/// The card (keyed `key`) pops open ([`motion::appear`]: it rises 8 pt and scales up into
/// place with a slight bounce as it fades in); the dim (`{key}-dim`) only fades in. It is
/// modal from its first frame: focus stays inside and
/// Return presses the button marked `El::default_button()`.
pub(crate) fn small_dialog(
    t: &Theme,
    key: &str,
    body: El,
    dismiss: Option<fn(&mut State)>,
    layer: &mut Vec<El>,
) {
    layer.push(
        el().key(format!("{key}-dim"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(t.kit.overlay)
            .transition(motion::pop(rune::Effect::opacity(0.0))),
    );
    let mut card = col()
        .key(key.to_string())
        .role("dialog")
        .w_full()
        .max_w(CARD_MAX_W)
        .padding(20.0)
        .items_stretch()
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(radius::PANE);
    for sh in shadow::kit_xl() {
        card = card.box_shadow(sh);
    }
    let card = card
        .child(body)
        .transition(motion::appear(8.0))
        .instant_changes();
    let card = match dismiss {
        Some(close) => overlay::dismissable(card, key, close, layer),
        None => card.modal(false, |_: &mut State| {}),
    };
    layer.push(
        col()
            .key(format!("{key}-host"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .items_center()
            .justify_center()
            .child(card),
    );
}

/// The open prompt or alert, if any: the message (16 pt), the field (prompts), Cancel
/// (prompts) and OK.
pub(crate) fn overlay(s: &State, _cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) {
    let Some(prompts) = s.get::<Prompts>() else {
        return;
    };
    let Some(prompt) = &prompts.open else {
        return;
    };
    let asks = prompt.on_submit.is_some();
    let mut body = col().key("prompt").gap(12.0).items_stretch().child(
        text(prompt.message.clone())
            .font(ty::ui(16.0))
            .color(t.fg)
            .wrap(),
    );
    if asks {
        body = body.child(
            Input::new("prompt-input", &prompts.input)
                .on_edit(prompt_input)
                .on_key(prompt_key)
                .build(t),
        );
    }
    let mut footer = row().key("prompt-footer").gap(8.0).justify_end().center_y();
    if asks {
        footer = footer.child(
            outline_button("prompt-cancel", "Cancel")
                .on_press(cancel)
                .build(t),
        );
    }
    footer = footer.child(
        outline_button("prompt-ok", "OK")
            .on_press(submit)
            .build(t)
            .default_button(),
    );
    small_dialog(t, "prompt-modal", body.child(footer), Some(cancel), layer);
}

fn prompt_input(s: &mut State) -> &mut InputState {
    &mut s.slot::<Prompts>().input
}

/// The field's Enter answers OK and Escape cancels (before the editor sees them).
fn prompt_key(s: &mut State, ev: &KeyEvent, _: &mut Tx) -> bool {
    if ev.mods != Mods::default() {
        return false;
    }
    match ev.key {
        Key::Enter => submit(s),
        Key::Escape => cancel(s),
        _ => return false,
    }
    true
}

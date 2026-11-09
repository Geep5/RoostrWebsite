//! The screen for a vault without an identity (`hasKey == false`): the key
//! IS the account. KeyGate.svelte's sign-in / create-account flow. With a
//! daemon (PairGate.svelte's "Sign in with a Nostr key instead", in an app
//! that already holds the daemon's service token) the key goes to the daemon
//! with `nostr_key_import`; in replica mode the same action keeps it on this
//! device (browser `localStorage` `roostr-key`, the Svelte app's own entry).
//! Either way the vault reopens under it.

use std::sync::LazyLock;

use roostr_core::mode::{self, Mode};
use roostr_core::{State, api, io};
use roostr_ui::color::rgba;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Alert, Button, Checkbox, InputState, Theme, ty};
use rune::{Align, El, Pixels, Tx, col, el, text};

use crate::keys::generate_nsec;
use crate::widgets::{Btn, Field, FieldStyle};

/// The app logo, decoded once.
static LOGO: LazyLock<Pixels> = LazyLock::new(|| {
    let pixels = Pixels::new();
    let image = image::load_from_memory_with_format(
        include_bytes!("../../../../static/logo.png"),
        image::ImageFormat::Png,
    )
    .expect("static/logo.png is a valid PNG")
    .into_rgba8();
    pixels.set(image.width(), image.height(), image.into_raw());
    pixels
});

/// The gate's state (the shell shows the gate instead of the workspace when
/// the daemon has no identity).
pub struct IdentityGate {
    draft: InputState,
    error: String,
    busy: bool,
    /// The new account's key while it is being saved; `None` on the sign-in form.
    created: Option<String>,
    saved: bool,
    copied: bool,
}

impl Default for IdentityGate {
    fn default() -> Self {
        Self {
            draft: InputState::new().masked(true),
            error: String::new(),
            busy: false,
            created: None,
            saved: false,
            copied: false,
        }
    }
}

/// Make `key` this machine's identity, then reopen the vault under it.
fn open(s: &mut State, key: String) {
    let gate = s.slot::<IdentityGate>();
    if gate.busy {
        return;
    }
    gate.error.clear();
    gate.busy = true;
    let task = api::settings::import_key(&key, s);
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            let gate = s.slot::<IdentityGate>();
            gate.busy = false;
            match result {
                Ok(_) => {
                    gate.created = None;
                    // The gate stays in state: a later logout must not find the key still typed in.
                    gate.draft.clear();
                    // The web reloads the page here: restart the store on the new identity.
                    s.store.reconnect();
                }
                Err(error) => gate.error = error.to_string(),
            }
        });
    })
    .detach();
}

/// The typed key, trimmed.
fn typed(gate: &IdentityGate) -> String {
    gate.draft.value().trim().to_string()
}

fn open_typed(s: &mut State) {
    let key = typed(s.slot::<IdentityGate>());
    if !key.is_empty() {
        open(s, key);
    }
}

fn create(s: &mut State) {
    let gate = s.slot::<IdentityGate>();
    gate.error.clear();
    gate.saved = false;
    gate.copied = false;
    match generate_nsec() {
        Ok(key) => gate.created = Some(key),
        Err(error) => gate.error = error.to_string(),
    }
}

fn primary(t: &Theme, key: &'static str, label: &'static str) -> Btn<State> {
    Btn::action(t, key, label)
        .full_width()
        .colors(Some(t.accent), None, t.white)
        .padding(11.0, 11.0)
        .text_style(14.0, ty::SEMIBOLD)
        .radius(10.0)
        .hover(None, None, None)
}

fn ghost(t: &Theme, key: &'static str, label: &'static str) -> Btn<State> {
    Btn::action(t, key, label)
        .full_width()
        .colors(Some(rgba(0xffffff0f)), Some(rgba(0xffffff24)), t.fg)
        .padding(11.0, 11.0)
        .text_style(14.0, ty::SEMIBOLD)
        .radius(10.0)
        .hover(None, None, None)
}

/// A quiet text link under the card (kit text button, muted until hover).
fn link(t: &Theme, key: &'static str, label: &'static str) -> Button {
    Button::new(key)
        .text()
        .h_auto()
        .p(4.0)
        .text_color(t.muted)
        .accessibility_label(label)
        .child(text(label).font(ty::ui(14.0)).color(t.muted))
}

fn sub(t: &Theme, s: &'static str) -> El {
    text(s)
        .font(ty::ui(14.0).lh(22.0))
        .color(t.muted)
        .wrap()
        .align(Align::Center)
        .w_full()
        .mb(8.0)
}

/// The gate, filling its parent.
pub fn identity_gate(s: &State, _cx: &mut rune::Cx<State>, t: &Theme, _layer: &mut Vec<El>) -> El {
    let fallback = IdentityGate::default();
    let gate = s.get::<IdentityGate>().unwrap_or(&fallback);
    let replica = mode::current(s) == Mode::Replica;
    let input_style = FieldStyle {
        bg: rgba(0xffffff0d),
        radius: 10.0,
        px: 14.0,
        py: 11.0,
        size: 14.0,
        focus: t.accent,
        ring: None,
        centered: true,
    };
    let mut card = col()
        .center()
        .gap(10.0)
        .w(420.0)
        .px(32.0)
        .py(36.0)
        .child(
            el().key("gate-logo")
                .size(72.0, 72.0)
                .radius(18.0)
                .pixels(&LOGO),
        )
        .child(
            text("Roostr")
                .font(ty::ui(26.0).lh(32.0).weight(ty::BOLD))
                .color(t.fg)
                .mt(4.0),
        );

    if let Some(created) = &gate.created {
        card = card
            .child(sub(
                t,
                "This key is your new account. Save it somewhere safe, like a password manager - it's the only way back into your vault on another device, and nobody can reset it.",
            ))
            .child(
                el().w_full()
                    .px(14.0)
                    .py(12.0)
                    .radius(10.0)
                    .fill(rgba(0xffffff0d))
                    .border(1.0, rgba(0xffffff1f))
                    .child(
                        text(created.clone())
                            .font(ty::mono(13.0).lh(20.0))
                            .color(t.fg)
                            .wrap()
                            .align(Align::Center)
                            .w_full(),
                    ),
            )
            .child(
                ghost(
                    t,
                    "copy-key",
                    if gate.copied { "Copied" } else { "Copy key" },
                )
                .on_press_tx(|s: &mut State, tx: &mut Tx| {
                    let gate = s.slot::<IdentityGate>();
                    if let Some(key) = &gate.created {
                        tx.copy(key.clone());
                        gate.copied = true;
                    }
                })
                .build(t),
            )
            .child(
                Checkbox::new("saved-key")
                    .checked(gate.saved)
                    .label("I saved my key")
                    .on_change(|s: &mut State, checked| s.slot::<IdentityGate>().saved = checked)
                    .build(t),
            )
            .child(
                primary(t, "open-created", "Open my vault")
                    .disabled(gate.busy || !gate.saved)
                    .on_press(|s: &mut State| {
                        if let Some(key) = s.slot::<IdentityGate>().created.clone() {
                            open(s, key);
                        }
                    })
                    .build(t),
            )
            .child(
                link(t, "back", "Back")
                    .on_press(|s: &mut State| s.slot::<IdentityGate>().created = None)
                    .build(t),
            );
    } else {
        let empty = typed(gate).is_empty();
        card = card
            .child(sub(
                t,
                "Sign in with your Nostr private key. Your notes sync end-to-end encrypted over relays — the key never leaves this device, and only it can decrypt your objects.",
            ))
            .child(
                col()
                    .gap(10.0)
                    .w_full()
                    .child(
                        Field::new("gate-key", &gate.draft, input_style)
                            .placeholder("nsec1… or 64-char hex")
                            .on_edit(|s: &mut State| &mut s.slot::<IdentityGate>().draft)
                            .on_submit(|s: &mut State, _: &mut Tx| open_typed(s))
                            .build(t),
                    )
                    .child(
                        primary(t, "open-key", "Open my vault")
                            .disabled(gate.busy || empty)
                            .on_press(open_typed)
                            .build(t),
                    ),
            )
            .child(text("or").font(ty::ui(12.0)).color(t.muted))
            .child(ghost(t, "create", "Create a new account").on_press(create).build(t))
            .child(
                text(if replica {
                    "Have desktop Roostr? Settings → Reveal private key, and paste it above."
                } else {
                    "This becomes this machine's daemon identity — like signing in on the web app. \
                     Or set it up in the daemon's terminal and retry."
                })
                .font(ty::ui(12.0).lh(18.0))
                .color(t.muted)
                .wrap()
                .align(Align::Center)
                .w_full()
                .mt(6.0),
            );
        if replica {
            // The other way in: the daemon on this computer (pairing, in a
            // browser). The iOS app is replica-only: it has none.
            if !cfg!(target_os = "ios") {
                card = card.child(
                    link(
                        t,
                        "use-daemon",
                        if io::WEB {
                            "Pair with Roostr on this computer instead"
                        } else {
                            "Use the Roostr daemon on this computer instead"
                        },
                    )
                    .on_press_tx(|_: &mut State, tx: &mut Tx| {
                        // True when the relaunched app is running: this one quits.
                        if mode::switch(Mode::Daemon) {
                            tx.quit();
                        }
                    })
                    .build(t),
                );
            }
        } else {
            card = card.child(
                link(t, "retry", "Retry")
                    .on_press(|s: &mut State| s.store.reconnect())
                    .build(t),
            );
        }
    }
    if !gate.error.is_empty() {
        card = card.child(Alert::error("gate-error", gate.error.clone()).build(t));
    }

    el().key("identity-gate")
        .w_full()
        .h_full()
        .center()
        .fill(t.bg)
        .child(card)
}

//! The pairing gate (`PairGate.svelte`, unpaired state): pair this tab with
//! the daemon on this computer by its one-use code. A page served from this
//! machine can read the daemon's current code itself and pair in one click;
//! roostr.space has to paste the code the daemon printed. "Sign in with a
//! Nostr key instead" pairs by the vault owner key (`pairWithOwnerKey`): the
//! harness's challenge signed with it, the key then kept as the hosted app
//! keeps it (`saveKey`).

use std::time::Duration;

use roostr_core::State;
use roostr_core::mode::{self, Mode};
use roostr_core::web as transport;
use roostr_settings::{Btn, Field, FieldStyle};
use roostr_ui::color::rgb;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Glyph, InputState, Theme, glyph, shadow, ty};
use rune::{Color, Cursor, Cx, El, TextStyle, Tx, col, el, row, text};

const CARD_BG: u32 = 0x161617;
const CARD_BORDER: u32 = 0x2c2c2e;
const TEXT: u32 = 0xececec;
const MUTED: u32 = 0xa3a3a8;
const LABEL: u32 = 0x8d8d93;
const HINT: u32 = 0x7d7d83;
const GOLD: u32 = 0xf2c14e;
const ALERT: u32 = 0xff9d94;

/// The gate's state, while it shows.
pub struct PairGate {
    code: InputState,
    /// The daemon's current code, when it serves it to this page.
    daemon_code: Option<String>,
    busy: bool,
    copied: bool,
    error: String,
    key: InputState,
    /// The key section is open.
    key_open: bool,
    /// This origin keeps a key: offered in one click.
    saved_key: bool,
    key_busy: bool,
    key_error: String,
}

fn gate(s: &mut State) -> Option<&mut PairGate> {
    s.slot::<Option<PairGate>>().as_mut()
}

/// Show the gate. `notice`: why it is back (the pairing expired or was removed).
pub fn open(s: &mut State, notice: Option<String>) {
    let saved_key = transport::saved_key().is_some();
    *s.slot::<Option<PairGate>>() = Some(PairGate {
        code: InputState::new().masked(true),
        daemon_code: None,
        busy: false,
        copied: false,
        error: notice.unwrap_or_default(),
        key: InputState::new().masked(true),
        key_open: saved_key,
        saved_key,
        key_busy: false,
        key_error: String::new(),
    });
    s.rt.spawn(async move |cx| {
        let code = transport::daemon_code().await;
        cx.update(|s| {
            if let Some(gate) = gate(s) {
                gate.daemon_code = code;
            }
        });
    })
    .detach();
}

/// The gate is done (this tab holds a session): drop its state.
pub fn close(s: &mut State) {
    *s.slot::<Option<PairGate>>() = None;
}

/// Pair with a pasted key.
fn use_key(s: &mut State) {
    let Some(g) = gate(s) else {
        return;
    };
    let input = g.key.value().to_string();
    if input.trim().is_empty() {
        return;
    }
    match transport::parse_key(&input) {
        Ok(keys) => pair_with_key(s, keys),
        Err(error) => g.key_error = error.to_string(),
    }
}

fn pair_with_key(s: &mut State, keys: roostr_nostr::Keys) {
    let Some(g) = gate(s) else {
        return;
    };
    if g.key_busy {
        return;
    }
    g.key_busy = true;
    g.key_error.clear();
    s.rt.spawn(async move |cx| {
        let result = transport::pair_with_owner_key(&keys).await;
        cx.update(|s| {
            let Some(g) = gate(s) else {
                return;
            };
            g.key_busy = false;
            match result {
                Ok(_) => crate::paired(s),
                Err(error) => g.key_error = error.to_string(),
            }
        });
    })
    .detach();
}

fn pair(s: &mut State, code: String) {
    let Some(g) = gate(s) else {
        return;
    };
    if g.busy {
        return;
    }
    g.busy = true;
    g.error.clear();
    s.rt.spawn(async move |cx| {
        let result = transport::pair(&code).await;
        cx.update(|s| {
            let Some(g) = gate(s) else {
                return;
            };
            g.busy = false;
            match result {
                Ok(_) => crate::paired(s),
                Err(error) => g.error = error.to_string(),
            }
        });
    })
    .detach();
}

fn copy(s: &mut State, tx: &mut Tx) {
    let Some(g) = gate(s) else {
        return;
    };
    let Some(code) = g.daemon_code.clone() else {
        return;
    };
    tx.copy(code);
    g.copied = true;
    s.rt.spawn(async move |cx| {
        cx.timer(Duration::from_millis(1600)).await;
        cx.update(|s| {
            if let Some(g) = gate(s) {
                g.copied = false;
            }
        });
    })
    .detach();
}

fn primary(t: &Theme, key: &str, label: &str) -> Btn<State> {
    Btn::action(t, key, label)
        .colors(Some(rgb(GOLD)), Some(rgb(GOLD)), rgb(0x241a05))
        .padding(16., 10.)
        .text_style(14., ty::SEMIBOLD)
        .radius(9.)
        .hover(Some(rgb(0xf7cf6a)), None, Some(rgb(0xf7cf6a)))
}

fn ghost(t: &Theme, key: &str, label: &str) -> Btn<State> {
    Btn::action(t, key, label)
        .colors(None, Some(rgb(0x3a3a3d)), rgb(0xc9c9cf))
        .padding(16., 10.)
        .text_style(14., ty::SEMIBOLD)
        .radius(9.)
        .hover(Some(rgb(0x4a4a4e)), Some(t.white), Some(rgb(0x232325)))
}

fn paragraph(line: &str, color: Color, size: f32, height: f32) -> El {
    text(line.to_owned())
        .font(TextStyle::new(size, ty::NORMAL).lh(height))
        .color(color)
        .wrap()
}

fn input_style() -> FieldStyle {
    FieldStyle {
        bg: rgb(0x1e1e20),
        radius: 9.,
        px: 12.,
        py: 10.,
        size: 14.,
        focus: rgb(0xa8873a),
        ring: Some(rgb(GOLD).alpha(0.12)),
        centered: false,
    }
}

/// "Sign in with a Nostr key instead" (`<details class="identity">`).
fn key_section(g: &PairGate, t: &Theme) -> El {
    let summary_color = rgb(0xc9c9cf);
    let summary = row()
        .key("key-summary")
        .center_y()
        .gap(6.)
        .cursor(Cursor::Pointer)
        .child(
            glyph(
                if g.key_open {
                    Glyph::ChevronDown
                } else {
                    Glyph::ChevronRight
                },
                12.,
            )
            .color(summary_color),
        )
        .child(
            text("Sign in with a Nostr key instead")
                .font(ty::ui(14.).weight(550.))
                .color(summary_color),
        )
        .on_press(|s: &mut State| {
            if let Some(g) = gate(s) {
                g.key_open = !g.key_open;
                if g.key_open {
                    g.key.focus();
                }
            }
        });
    let mut section = col()
        .mt(18.)
        .pt(14.)
        .border_sides([1., 0., 0., 0.], rgb(0x262628))
        .child(summary);
    if !g.key_open {
        return section;
    }
    let busy = g.key_busy;
    let busy_label = |label: &'static str| if busy { "Pairing…" } else { label };
    let empty = g.key.value().trim().is_empty();
    section = section.child(
        paragraph(
            "Paste the private key of Roostr on this computer (nsec or 64-char hex): this tab signs a one-use challenge with it to pair. The key never leaves this device. Have desktop Roostr? Settings → Reveal private key.",
            rgb(HINT),
            12.,
            19.,
        )
        .mt(10.),
    );
    if g.saved_key {
        section = section.child(
            el().mt(16.).child(
                primary(t, "pair-saved-key", busy_label("Pair with the saved key"))
                    .full_width()
                    .disabled(busy)
                    .on_press(|s: &mut State| {
                        if let Some(keys) = transport::saved_key() {
                            pair_with_key(s, keys);
                        }
                    })
                    .build(t),
            ),
        );
    }
    section = section.child(
        col()
            .mt(16.)
            .gap(10.)
            .child(
                Field::new("pair-key", &g.key, input_style())
                    .placeholder("nsec1… or 64-char hex")
                    .on_edit(|s: &mut State| &mut gate(s).expect("the gate shows").key)
                    .on_submit(|s: &mut State, _: &mut Tx| use_key(s))
                    .build(t),
            )
            .child(
                primary(t, "use-key", busy_label("Use this key"))
                    .full_width()
                    .disabled(busy || empty)
                    .on_press(use_key)
                    .build(t),
            ),
    );
    if !g.key_error.is_empty() {
        section = section.child(paragraph(&g.key_error, rgb(ALERT), 13.5, 22.).mt(14.));
    }
    section
}

/// The gate, filling the window.
pub fn view(s: &State, cx: &mut Cx<State>, t: &'static Theme) -> El {
    let Some(g) = s.get::<Option<PairGate>>().and_then(Option::as_ref) else {
        return el().w_full().h_full().fill(t.gutter);
    };
    let empty = g.code.value().trim().is_empty();
    let mut card = col()
        .key("pair-card")
        .w(480.)
        .px(32.)
        .py(30.)
        .radius(18.)
        .border(1., rgb(CARD_BORDER))
        .fill(rgb(CARD_BG))
        .box_shadow(shadow::gpui(16., 48., 0., 0.45))
        .child(
            el().mb(8.).child(
                text("Pair with your local daemon")
                    .font(ty::ui(19.).semibold())
                    .color(rgb(TEXT)),
            ),
        )
        .child(paragraph(
            "Pairing allows this browser origin to access local data and machine controls for 24 hours. Codes are one-use and expire after five minutes.",
            rgb(MUTED),
            13.5,
            22.,
        ));

    if let Some(code) = g.daemon_code.clone() {
        let shown = code.clone();
        card = card.child(
            col()
                .mt(16.)
                .mb(4.)
                .gap(10.)
                .padding(14.)
                .radius(12.)
                .border(1., rgb(0x2a2a2c))
                .fill(rgb(0x1b1b1d))
                .child(
                    text("This daemon’s current code")
                        .font(ty::ui(12.))
                        .color(rgb(LABEL)),
                )
                .child(
                    col()
                        .px(12.)
                        .py(9.)
                        .radius(8.)
                        .border(1., rgb(0x262628))
                        .fill(rgb(0x121213))
                        .child(
                            text(shown)
                                .font(ty::mono(11.5))
                                .color(rgb(0xe8d9a0))
                                .wrap_anywhere(),
                        ),
                )
                .child(
                    row()
                        .gap(10.)
                        .child(
                            el().grow(1.).child(
                                primary(
                                    t,
                                    "pair-daemon-code",
                                    if g.busy {
                                        "Pairing…"
                                    } else {
                                        "Pair with this code"
                                    },
                                )
                                .full_width()
                                .disabled(g.busy)
                                .on_press(move |s: &mut State| pair(s, code.clone()))
                                .build(t),
                            ),
                        )
                        .child(
                            ghost(
                                t,
                                "copy-daemon-code",
                                if g.copied { "Copied" } else { "Copy" },
                            )
                            .on_press_tx(copy)
                            .build(t),
                        ),
                ),
        );
    }

    card = card.child(
        col()
            .mt(16.)
            .gap(10.)
            .child(
                text(if g.daemon_code.is_some() {
                    "Or paste a terminal code"
                } else {
                    "Terminal pairing code"
                })
                .font(ty::ui(12.5))
                .color(rgb(LABEL)),
            )
            .child(
                Field::new("pair-code", &g.code, input_style())
                    .on_edit(|s: &mut State| &mut gate(s).expect("the gate shows").code)
                    .on_submit(|s: &mut State, _: &mut Tx| {
                        let code = gate(s).map(|g| g.code.value().to_string());
                        if let Some(code) = code {
                            pair(s, code);
                        }
                    })
                    .build(t),
            )
            .child(
                primary(
                    t,
                    "pair-tab",
                    if g.busy {
                        "Pairing…"
                    } else {
                        "Pair this tab"
                    },
                )
                .full_width()
                .disabled(g.busy || empty)
                .on_press(|s: &mut State| {
                    let code = gate(s).map(|g| g.code.value().to_string());
                    if let Some(code) = code {
                        pair(s, code);
                    }
                })
                .build(t),
            ),
    );
    card = card
        .child(
            paragraph(
                "No daemon? Use Roostr in this browser instead: it keeps its own replica and browser-owned identity, synced over your relays. Pairing never uploads or replaces that identity.",
                rgb(HINT),
                12.,
                19.,
            )
            .mt(16.),
        )
        .child(
            el().mt(10.).child(
                ghost(t, "use-replica", "Use Roostr in this browser")
                    .on_press(|_: &mut State| {
                        // The browser reloads into the replica.
                        let _ = mode::switch(Mode::Replica);
                    })
                    .build(t),
            ),
        )
        .child(key_section(g, t));
    if !g.error.is_empty() {
        card = card.child(paragraph(&g.error, rgb(ALERT), 13.5, 22.).mt(14.));
    }

    let height = cx.layout_with("kit-root", |r| r[3]).unwrap_or(0.);
    let page = col()
        .w_full()
        .items_center()
        .pt(height * 0.14)
        .pb(48.)
        .px(16.)
        .child(card.max_w(480.).w_full());
    let scroll = roostr_ui::ScrollArea::vertical("pair-gate", page)
        .w_full()
        .grow()
        .build(t);
    col().w_full().h_full().fill(t.gutter).child(scroll)
}

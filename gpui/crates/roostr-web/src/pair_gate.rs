//! The pairing gate (`PairGate.svelte`, unpaired state): pair this tab with
//! the daemon on this computer by its one-use code. A page served from this
//! machine can read the daemon's current code itself and pair in one click;
//! roostr.space has to paste the code the daemon printed. "Sign in with a
//! Nostr key instead" pairs by the vault owner key (`pairWithOwnerKey`): the
//! harness's challenge signed with it, the key then kept as the hosted app
//! keeps it (`saveKey`).

use std::time::Duration;

use gpui_kit::component::IconNamed as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::mode::{self, Mode};
use roostr_core::web as transport;
use roostr_core::{Glyph, theme};
use roostr_settings::{Btn, FieldStyle, field};

/// Emitted once this tab holds a session.
pub struct Paired;

pub struct PairGate {
    code: Entity<InputState>,
    /// The daemon's current code, when it serves it to this page.
    daemon_code: Option<SharedString>,
    busy: bool,
    copied: bool,
    error: SharedString,
    key: Entity<InputState>,
    /// The key section is open.
    key_open: bool,
    /// This origin keeps a key: offered in one click.
    saved_key: bool,
    key_busy: bool,
    key_error: SharedString,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Paired> for PairGate {}

const CARD_BG: u32 = 0x161617;
const CARD_BORDER: u32 = 0x2c2c2e;
const TEXT: u32 = 0xececec;
const MUTED: u32 = 0xa3a3a8;
const LABEL: u32 = 0x8d8d93;
const HINT: u32 = 0x7d7d83;
const GOLD: u32 = 0xf2c14e;
const ALERT: u32 = 0xff9d94;

impl PairGate {
    /// `notice`: why the gate is back (the pairing expired or was removed).
    pub fn new(notice: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let code = cx.new(|cx| InputState::new(window, cx).masked(true));
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("nsec1… or 64-char hex")
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &code,
                window,
                |this, state, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        let code = state.read(cx).value().to_string();
                        this.pair(code, cx);
                    }
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &key,
                window,
                |this, state, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        let key = state.read(cx).value().to_string();
                        this.use_key(&key, cx);
                    }
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
        ];
        let lookup = cx.spawn(async move |this, cx| {
            let code = transport::daemon_code().await;
            let _ = this.update(cx, |this, cx| {
                this.daemon_code = code.map(Into::into);
                cx.notify();
            });
        });
        let saved_key = transport::saved_key().is_some();
        Self {
            code,
            daemon_code: None,
            busy: false,
            copied: false,
            error: notice.unwrap_or_default(),
            key,
            key_open: saved_key,
            saved_key,
            key_busy: false,
            key_error: SharedString::default(),
            _tasks: vec![lookup],
            _subscriptions: subscriptions,
        }
    }

    /// Pair with a pasted key.
    fn use_key(&mut self, input: &str, cx: &mut Context<Self>) {
        if input.trim().is_empty() {
            return;
        }
        match transport::parse_key(input) {
            Ok(keys) => self.pair_with_key(keys, cx),
            Err(error) => {
                self.key_error = error.to_string().into();
                cx.notify();
            }
        }
    }

    fn pair_with_key(&mut self, keys: roostr_nostr::Keys, cx: &mut Context<Self>) {
        if self.key_busy {
            return;
        }
        self.key_busy = true;
        self.key_error = SharedString::default();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = transport::pair_with_owner_key(&keys).await;
            let _ = this.update(cx, |this, cx| {
                this.key_busy = false;
                match result {
                    Ok(_) => cx.emit(Paired),
                    Err(error) => this.key_error = error.to_string().into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn pair(&mut self, code: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = SharedString::default();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = transport::pair(&code).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(_) => cx.emit(Paired),
                    Err(error) => this.error = error.to_string().into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let Some(code) = self.daemon_code.clone() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(code.to_string()));
        self.copied = true;
        cx.notify();
        let timer = cx.background_executor().timer(Duration::from_millis(1600));
        self._tasks.push(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        }));
    }

    fn primary(id: &'static str, label: &'static str) -> Btn {
        Btn::action(id, label).style(|b| {
            b.set_colors(
                Some(rgb(GOLD).into()),
                Some(rgb(GOLD).into()),
                rgb(0x241a05).into(),
            );
            b.set_padding(16., 10.);
            b.set_text(14., FontWeight::SEMIBOLD);
            b.set_radius(9.);
            b.set_hover(Some(rgb(0xf7cf6a).into()), None, Some(rgb(0xf7cf6a).into()));
        })
    }

    fn ghost(id: &'static str, label: &'static str) -> Btn {
        Btn::action(id, label).style(|b| {
            b.set_colors(None, Some(rgb(0x3a3a3d).into()), rgb(0xc9c9cf).into());
            b.set_padding(16., 10.);
            b.set_text(14., FontWeight::SEMIBOLD);
            b.set_radius(9.);
            b.set_hover(
                Some(rgb(0x4a4a4e).into()),
                Some(theme::white()),
                Some(rgb(0x232325).into()),
            );
        })
    }

    /// "Sign in with a Nostr key instead" (`<details class="identity">`).
    fn key_section(&self, input_style: FieldStyle, window: &Window, cx: &mut Context<Self>) -> Div {
        let summary_color = rgb(0xc9c9cf);
        let summary = div()
            .id("key-summary")
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .text_color(summary_color)
            .font_weight(FontWeight(550.))
            .hover(|s| s.text_color(theme::white()))
            .child(
                svg()
                    .path(if self.key_open {
                        Glyph::ChevronDown.path()
                    } else {
                        Glyph::ChevronRight.path()
                    })
                    .size(px(12.))
                    .text_color(summary_color),
            )
            .child("Sign in with a Nostr key instead")
            .on_click(cx.listener(|this, _, window, cx| {
                this.key_open = !this.key_open;
                if this.key_open {
                    this.key.focus_handle(cx).focus(window, cx);
                }
                cx.notify();
            }));
        let section = div()
            .mt(px(18.))
            .pt(px(14.))
            .border_t_1()
            .border_color(rgb(0x262628))
            .flex()
            .flex_col()
            .child(summary);
        if !self.key_open {
            return section;
        }
        let key_busy = self.key_busy;
        let busy_label = move |label: &'static str| if key_busy { "Pairing…" } else { label };
        let empty = self.key.read(cx).value().trim().is_empty();
        section
            .child(
                div()
                    .mt(px(10.))
                    .text_size(px(12.))
                    .line_height(px(19.))
                    .text_color(rgb(HINT))
                    .child(
                        "Paste the private key of Roostr on this computer (nsec or 64-char hex): this tab signs a one-use challenge with it to pair. The key never leaves this device. Have desktop Roostr? Settings → Reveal private key.",
                    ),
            )
            .when(self.saved_key, |d| {
                d.child(
                    div().mt(px(16.)).child(
                        Self::primary("pair-saved-key", busy_label("Pair with the saved key"))
                            .full_width()
                            .disabled(self.key_busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(keys) = transport::saved_key() {
                                    this.pair_with_key(keys, cx);
                                }
                            })),
                    ),
                )
            })
            .child(
                div()
                    .mt(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(field(&self.key, input_style, window, cx))
                    .child(
                        Self::primary("use-key", busy_label("Use this key"))
                            .full_width()
                            .disabled(self.key_busy || empty)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let key = this.key.read(cx).value().to_string();
                                this.use_key(&key, cx);
                            })),
                    ),
            )
            .when(!self.key_error.is_empty(), |d| {
                d.child(
                    div()
                        .mt(px(14.))
                        .text_size(px(13.5))
                        .line_height(px(22.))
                        .text_color(rgb(ALERT))
                        .child(self.key_error.clone()),
                )
            })
    }
}

fn paragraph(text: &'static str) -> Div {
    div()
        .text_size(px(13.5))
        .line_height(px(22.))
        .text_color(rgb(MUTED))
        .child(text)
}

impl Render for PairGate {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut ring: Hsla = rgb(GOLD).into();
        ring.a = 0.12;
        let input_style = FieldStyle {
            bg: rgb(0x1e1e20).into(),
            radius: 9.,
            px: 12.,
            py: 10.,
            size: 14.,
            focus: rgb(0xa8873a).into(),
            ring: Some(ring),
            centered: false,
        };
        let empty = self.code.read(cx).value().trim().is_empty();
        let mut card = div()
            .flex()
            .flex_col()
            .w(px(480.))
            .max_w_full()
            .px(px(32.))
            .py(px(30.))
            .rounded(px(18.))
            .border_1()
            .border_color(rgb(CARD_BORDER))
            .bg(rgb(CARD_BG))
            .shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., 0.45),
                offset: point(px(0.), px(16.)),
                blur_radius: px(48.),
                spread_radius: px(0.),
                inset: false,
            }])
            .text_color(rgb(TEXT))
            .text_size(px(14.))
            .child(
                div()
                    .mb(px(8.))
                    .text_size(px(19.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Pair with your local daemon"),
            )
            .child(paragraph(
                "Pairing allows this browser origin to access local data and machine controls for 24 hours. Codes are one-use and expire after five minutes.",
            ));

        if let Some(code) = self.daemon_code.clone() {
            card = card.child(
                div()
                    .mt(px(16.))
                    .mb(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .p(px(14.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(0x2a2a2c))
                    .bg(rgb(0x1b1b1d))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(LABEL))
                            .child("This daemon’s current code"),
                    )
                    .child(
                        div()
                            .px(px(12.))
                            .py(px(9.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(rgb(0x262628))
                            .bg(rgb(0x121213))
                            .text_color(rgb(0xe8d9a0))
                            .font_family(theme::fonts::MONO)
                            .text_size(px(11.5))
                            .child(code.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .child(
                                div().flex_1().child(
                                    Self::primary(
                                        "pair-daemon-code",
                                        if self.busy {
                                            "Pairing…"
                                        } else {
                                            "Pair with this code"
                                        },
                                    )
                                    .full_width()
                                    .disabled(self.busy)
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| this.pair(code.to_string(), cx),
                                    )),
                                ),
                            )
                            .child(
                                Self::ghost(
                                    "copy-daemon-code",
                                    if self.copied { "Copied" } else { "Copy" },
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
                            ),
                    ),
            );
        }

        card = card.child(
            div()
                .mt(px(16.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(div().text_size(px(12.5)).text_color(rgb(LABEL)).child(
                    if self.daemon_code.is_some() {
                        "Or paste a terminal code"
                    } else {
                        "Terminal pairing code"
                    },
                ))
                .child(field(&self.code, input_style, window, cx))
                .child(
                    Self::primary(
                        "pair-tab",
                        if self.busy {
                            "Pairing…"
                        } else {
                            "Pair this tab"
                        },
                    )
                    .full_width()
                    .disabled(self.busy || empty)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let code = this.code.read(cx).value().to_string();
                        this.pair(code, cx);
                    })),
                ),
        );
        card = card.child(
            div()
                .mt(px(16.))
                .text_size(px(12.))
                .line_height(px(19.))
                .text_color(rgb(HINT))
                .child(
                    "No daemon? Use Roostr in this browser instead: it keeps its own replica and browser-owned identity, synced over your relays. Pairing never uploads or replaces that identity.",
                ),
        );
        card = card.child(
            div().mt(px(10.)).child(
                Self::ghost("use-replica", "Use Roostr in this browser")
                    .on_click(|_, _, cx| mode::switch(Mode::Replica, cx)),
            ),
        );
        card = card.child(self.key_section(input_style, window, cx));
        if !self.error.is_empty() {
            card = card.child(
                div()
                    .mt(px(14.))
                    .text_size(px(13.5))
                    .line_height(px(22.))
                    .text_color(rgb(ALERT))
                    .child(self.error.clone()),
            );
        }

        div()
            .id("pair-gate")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .pt(window.viewport_size().height * 0.14)
            .pb(px(48.))
            .px(px(16.))
            .bg(theme::gutter())
            .child(card)
    }
}

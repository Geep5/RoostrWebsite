//! The screen for a vault without an identity (`hasKey == false`): the key
//! IS the account. KeyGate.svelte's sign-in / create-account flow. With a
//! daemon (PairGate.svelte's "Sign in with a Nostr key instead", in an app
//! that already holds the daemon's service token) the key goes to the daemon
//! with `nostr_key_import`; in replica mode the same action keeps it on this
//! device (browser `localStorage` `roostr-key`, the Svelte app's own entry).
//! Either way the vault reopens under it.

use std::sync::{Arc, LazyLock};

use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use roostr_core::mode::{self, Mode};
use roostr_core::{Store, api, io, theme};

use crate::keys::generate_nsec;
use crate::widgets::{Btn, FieldStyle, field};

static LOGO: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../../../../static/logo.png").to_vec(),
    ))
});

/// Shown by the shell instead of the workspace when the daemon has no identity.
pub struct IdentityGate {
    draft: Entity<InputState>,
    error: SharedString,
    busy: bool,
    /// The new account's key while it is being saved; `None` on the sign-in form.
    created: Option<SharedString>,
    saved: bool,
    copied: bool,
    _subscriptions: Vec<Subscription>,
}

impl IdentityGate {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("nsec1… or 64-char hex")
                .masked(true)
        });
        let subscriptions = vec![cx.subscribe_in(
            &draft,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let key = state.read(cx).value().trim().to_string();
                    if !key.is_empty() {
                        this.open(key, window, cx);
                    }
                }
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        )];
        Self {
            draft,
            error: SharedString::default(),
            busy: false,
            created: None,
            saved: false,
            copied: false,
            _subscriptions: subscriptions,
        }
    }

    /// Make `key` this machine's identity, then reopen the vault under it.
    fn open(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.error = SharedString::default();
        self.busy = true;
        cx.notify();
        let task = api::settings::import_key(&key, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        this.created = None;
                        // The gate stays mounted: a later logout must not find the key still typed in.
                        this.draft.update(cx, |s, cx| s.set_value("", window, cx));
                        // The web reloads the page here: restart the store on the new identity.
                        Store::global(cx).update(cx, |store, cx| store.reconnect(cx));
                    }
                    Err(error) => this.error = error.to_string().into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        self.error = SharedString::default();
        self.saved = false;
        self.copied = false;
        match generate_nsec() {
            Ok(key) => self.created = Some(key.into()),
            Err(error) => self.error = error.to_string().into(),
        }
        cx.notify();
    }

    fn primary(id: &'static str, label: &'static str) -> Btn {
        Btn::action(id, label).full_width().style(|b| {
            b.set_colors(Some(theme::accent()), None, theme::white());
            b.set_padding(11., 11.);
            b.set_text(14., FontWeight::SEMIBOLD);
            b.set_radius(10.);
            b.set_hover(None, None, None);
        })
    }

    fn ghost(id: &'static str, label: &'static str) -> Btn {
        Btn::action(id, label).full_width().style(|b| {
            b.set_colors(
                Some(rgba(0xffffff0f).into()),
                Some(rgba(0xffffff24).into()),
                theme::fg(),
            );
            b.set_padding(11., 11.);
            b.set_text(14., FontWeight::SEMIBOLD);
            b.set_radius(10.);
            b.set_hover(None, None, None);
        })
    }

    /// A quiet text link under the card (kit text button, muted until hover).
    /// The label carries its own size: the kit's inner box sets one of its own.
    fn link(id: &'static str, label: &'static str) -> Button {
        Button::new(id)
            .text()
            .h_auto()
            .p(px(4.))
            .text_color(theme::muted())
            .accessibility_label(label)
            .child(div().text_size(px(14.)).child(label))
    }
}

fn sub(text: &'static str) -> Div {
    div()
        .mb(px(8.))
        .text_size(px(14.))
        .line_height(px(22.))
        .text_color(theme::muted())
        .text_center()
        .child(text)
}

impl Render for IdentityGate {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let replica = mode::current(cx) == Mode::Replica;
        let input_style = FieldStyle {
            bg: rgba(0xffffff0d).into(),
            radius: 10.,
            px: 14.,
            py: 11.,
            size: 14.,
            focus: theme::accent(),
            ring: None,
            centered: true,
        };
        let mut card = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.))
            .w(px(420.))
            .px(px(32.))
            .py(px(36.))
            .child(img(LOGO.clone()).size(px(72.)).rounded(px(18.)))
            .child(
                div()
                    .mt(px(4.))
                    .text_size(px(26.))
                    .line_height(px(32.))
                    .font_weight(FontWeight::BOLD)
                    .child("Roostr"),
            );

        if let Some(created) = self.created.clone() {
            let key = created.clone();
            card = card
                .child(sub(
                    "This key is your new account. Save it somewhere safe, like a password manager - it's the only way back into your vault on another device, and nobody can reset it.",
                ))
                .child(
                    div()
                        .w_full()
                        .px(px(14.))
                        .py(px(12.))
                        .rounded(px(10.))
                        .bg(rgba(0xffffff0d))
                        .border_1()
                        .border_color(rgba(0xffffff1f))
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .font_family(theme::fonts::MONO)
                        .text_center()
                        .child(created),
                )
                .child(Self::ghost("copy-key", if self.copied { "Copied" } else { "Copy key" }).on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(key.to_string()));
                        this.copied = true;
                        cx.notify();
                    },
                )))
                .child(
                    Checkbox::new("saved-key")
                        .checked(self.saved)
                        .label("I saved my key")
                        .text_size(px(14.))
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.saved = *checked;
                            cx.notify();
                        })),
                )
                .child(Self::primary("open-created", "Open my vault").disabled(self.busy || !self.saved).on_click(cx.listener(
                    |this, _, window, cx| {
                        if let Some(key) = this.created.clone() {
                            this.open(key.to_string(), window, cx);
                        }
                    },
                )))
                .child(Self::link("back", "Back").on_click(cx.listener(|this, _, _, cx| {
                    this.created = None;
                    cx.notify();
                })));
        } else {
            let empty = self.draft.read(cx).value().trim().is_empty();
            card = card
                .child(sub(
                    "Sign in with your Nostr private key. Your notes sync end-to-end encrypted over relays — the key never leaves this device, and only it can decrypt your objects.",
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .w_full()
                        .child(field(&self.draft, input_style, window, cx))
                        .child(Self::primary("open-key", "Open my vault").disabled(self.busy || empty).on_click(cx.listener(
                            |this, _, window, cx| {
                                let key = this.draft.read(cx).value().trim().to_string();
                                if !key.is_empty() {
                                    this.open(key, window, cx);
                                }
                            },
                        ))),
                )
                .child(div().text_size(px(12.)).text_color(theme::muted()).child("or"))
                .child(Self::ghost("create", "Create a new account").on_click(cx.listener(|this, _, _, cx| this.create(cx))))
                .child(
                    div()
                        .mt(px(6.))
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(theme::muted())
                        .text_center()
                        .child(if replica {
                            "Have desktop Roostr? Settings → Reveal private key, and paste it above."
                        } else {
                            "This becomes this machine's daemon identity — like signing in on the web app. \
                             Or set it up in the daemon's terminal and retry."
                        }),
                );
            card = if replica {
                // The other way in: the daemon on this computer (pairing, in a
                // browser). The iOS app is replica-only: it has none.
                if cfg!(target_os = "ios") {
                    card
                } else {
                    card.child(
                        Self::link(
                            "use-daemon",
                            if io::WEB {
                                "Pair with Roostr on this computer instead"
                            } else {
                                "Use the Roostr daemon on this computer instead"
                            },
                        )
                        .on_click(|_, _, cx| mode::switch(Mode::Daemon, cx)),
                    )
                }
            } else {
                card.child(Self::link("retry", "Retry").on_click(|_, _, cx| {
                    Store::global(cx).update(cx, |store, cx| store.reconnect(cx))
                }))
            };
        }
        if !self.error.is_empty() {
            card = card.child(Alert::error("gate-error", self.error.clone()).text_size(px(13.)));
        }

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::bg())
            .text_color(theme::fg())
            .child(card)
    }
}

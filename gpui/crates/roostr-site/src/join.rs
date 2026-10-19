//! `/j#r2.<blob>` (`src/routes/j/+page.svelte`): the public join landing.
//! The link carries NO key - it only names a space and its owner. "Request
//! to join" gift-wraps this computer's npub to the owner (NIP-59) through the
//! local harness (`POST /join-requests/send`); they approve in space
//! settings, and the space key arrives gift-wrapped back.

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::harness::{HarnessError, harness};
use roostr_core::invite::{SpaceJoinLink, decode_join_link};
use roostr_core::io;
use roostr_core::nav::{self, Route};
use roostr_core::theme::fonts;
use roostr_core::ui;

use crate::assets::SiteImage;
use crate::doc::page_fg;
use crate::scroll_keys;
use crate::text::home;

const SEND_PATH: &str = "/join-requests/send";

fn muted() -> Hsla {
    rgb(0x98989d).into()
}

pub(crate) struct JoinPage {
    link: Option<SpaceJoinLink>,
    /// The web page's `hasKey`: this app reaches the daemon's identity
    /// through the harness (the service token stands in for browser pairing).
    has_key: bool,
    busy: bool,
    sent: bool,
    error: String,
    scroll: ScrollHandle,
    focus: FocusHandle,
}

/// In replica mode: this device holds a key.
fn replica_has_key(cx: &App) -> bool {
    roostr_core::account::account(cx).is_some()
        && roostr_core::Store::try_global(cx)
            .and_then(|store| store.read(cx).settings.as_ref().map(|s| s.has_key))
            .unwrap_or(false)
}

impl JoinPage {
    pub(crate) fn new(blob: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            link: decode_join_link(blob),
            has_key: harness(cx).is_some() || replica_has_key(cx),
            busy: false,
            sent: false,
            error: String::new(),
            scroll: ScrollHandle::new(),
            focus,
        }
    }

    /// `request()`: only offered when this app holds an identity (`has_key`):
    /// the harness's, or the replica's own.
    fn request(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(link) = &self.link else {
            return;
        };
        let send: io::BoxFuture<anyhow::Result<()>> =
            if let Some(account) = roostr_core::account::account(cx) {
                account.send_join_request(link)
            } else if let Some(harness) = harness(cx) {
                let body = serde_json::to_value(link).expect("join link serializes");
                Box::pin(async move { harness.post_json(SEND_PATH, &body).await.map(|_| ()) })
            } else {
                return;
            };
        self.busy = true;
        self.error.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = io::spawn_async(cx, send).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.sent = true,
                    Err(error) => this.error = error_message(&error),
                }
                this.busy = false;
                cx.notify();
            })
        })
        .detach();
    }

    fn card(&self, cx: &mut Context<Self>) -> Div {
        let card = div()
            .w(px(420.))
            .max_w_full()
            .border_1()
            .border_color(rgb(0x45454a))
            .rounded(px(12.))
            .p(px(28.))
            .bg(rgb(0x2b2b2e))
            .shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., 0.35),
                offset: point(px(0.), px(18.)),
                blur_radius: px(50.),
                spread_radius: px(0.),
                inset: false,
            }])
            .text_center()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.));
        let Some(link) = &self.link else {
            return card
                .child(heading("Invalid link"))
                .child(muted_text("This join link is invalid or truncated."));
        };
        if self.sent {
            return card
                .child(heading("Request sent"))
                .child(muted_text(format!(
                    "The owner of {} will see your request in their space settings. Once they approve, the space key is delivered to you and the space appears in your app.",
                    link.name.as_deref().filter(|n| !n.is_empty()).unwrap_or("the space")
                )))
                .child(open_app_button());
        }
        let name = link
            .name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or("a shared space");
        let owner: String = link.owner.chars().take(20).collect();
        let full_owner = SharedString::from(link.owner.clone());
        let card = card
            .child(heading(format!("Request to join {name}")))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .items_baseline()
                    .gap_x(px(4.))
                    .text_size(px(14.5))
                    .line_height(relative(1.6))
                    .text_color(muted())
                    .child("A Roostr space owned by")
                    .child(
                        div()
                            .id("owner")
                            .font_family(fonts::MONO)
                            .text_size(px(13.))
                            .text_color(page_fg())
                            .child(format!("{owner}…"))
                            .tooltip(ui::tooltip(full_owner)),
                    ),
            )
            .when(!link.relays.is_empty(), |el| {
                el.child(
                    div()
                        .w_full()
                        .text_size(px(12.5))
                        .text_color(rgb(0x6e6e73))
                        .child(format!("via {}", link.relays.join(", "))),
                )
            })
            .child(muted_text(
                "This link grants nothing by itself — the owner approves each request.",
            ));
        let card = if self.has_key {
            card.child(
                button(
                    "request",
                    if self.busy {
                        "Sending…"
                    } else {
                        "Request to join"
                    },
                )
                .when(self.busy, |el| el.opacity(0.5).cursor_default())
                .when(!self.busy, |el| {
                    el.hover(|s| s.bg(rgb(0x3395ff)).border_color(rgb(0x3395ff)))
                        .on_click(cx.listener(|this, _, _, cx| this.request(cx)))
                }),
            )
        } else {
            card.child(muted_text(
                "Set up your key in the app first, then reopen this link.",
            ))
            .child(open_app_button())
        };
        card.when(!self.error.is_empty(), |el| {
            el.child(
                div()
                    .text_size(px(13.5))
                    .text_color(rgb(0xff6961))
                    .child(self.error.clone()),
            )
        })
    }
}

/// The web shows the harness's answer text, or `Cannot send join request (status)`.
fn error_message(error: &anyhow::Error) -> String {
    match error.downcast_ref::<HarnessError>() {
        Some(e) if e.message == format!("{SEND_PATH}: HTTP {}", e.status) => {
            format!("Cannot send join request ({})", e.status)
        }
        Some(e) => e.message.clone(),
        None => error.to_string(),
    }
}

fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(24.))
        .line_height(relative(1.2))
        .font_weight(FontWeight::BOLD)
        .child(text.into())
}

fn muted_text(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(14.5))
        .line_height(relative(1.6))
        .text_color(muted())
        .child(text.into())
}

/// `.btn.primary`.
fn button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .py(px(10.))
        .px(px(24.))
        .rounded(px(8.))
        .border_1()
        .border_color(rgb(0x0a84ff))
        .bg(rgb(0x0a84ff))
        .text_color(rgb(0xffffff))
        .text_size(px(15.))
        .line_height(relative(1.2))
        .font_weight(FontWeight::MEDIUM)
        .child(label)
}

/// `<a class="btn primary" href="/app">Open Roostr Web</a>`.
fn open_app_button() -> Stateful<Div> {
    button("open-app", "Open Roostr Web")
        .hover(|s| s.bg(rgb(0x3395ff)).border_color(rgb(0x3395ff)))
        .on_click(|_, _, cx| nav::navigate(Route::Home, cx))
}

impl Focusable for JoinPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for JoinPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scroll = self.scroll.clone();
        let page = div().flex().justify_center().min_h_full().w_full().child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .max_w(px(1060.))
                .px(px(24.))
                .child(
                    div().flex().items_center().py(px(22.)).child(
                        div()
                            .id("brand")
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(17.))
                            .child(img(SiteImage::Logo.source()).size(px(22.)))
                            .child("Roostr")
                            .on_click(|_, _, cx| home().open(cx)),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .pt(px(24.))
                        .pb(px(96.))
                        .child(self.card(cx)),
                ),
        );
        scroll_keys(div().id("site-join"), &self.focus, cx, |this: &Self| {
            &this.scroll
        })
        .relative()
        .size_full()
        .bg(roostr_core::theme::bg())
        .text_color(page_fg())
        .font_family(fonts::UI)
        .text_size(px(16.))
        // CSS `line-height: normal` for the system font.
        .line_height(relative(1.2))
        .child(
            div()
                .id("site-join-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .child(page),
        )
        .vertical_scrollbar(&scroll)
    }
}

//! `/j#r2.<blob>` (`src/routes/j/+page.svelte`): the public join landing. The link carries NO
//! key - it only names a space and its owner. "Request to join" gift-wraps this computer's
//! npub to the owner (NIP-59) through the local harness (`POST /join-requests/send`); they
//! approve in space settings, and the space key arrives gift-wrapped back.

use std::collections::HashMap;

use roostr_core::State;
use roostr_core::harness::HarnessError;
use roostr_core::invite::{SpaceJoinLink, decode_join_link};
use roostr_core::nav::{self, Route};
use roostr_rt::BoxFuture;
use roostr_ui::color::rgb;
use roostr_ui::ty::Weights as _;
use roostr_ui::{ScrollArea, Theme, ty};
use rune::{Align, Color, Cursor, Cx, El, col, el, row, text};

use crate::assets::LOGO;
use crate::doc::page_fg;
use crate::text::home;
use crate::{JOIN_SCROLL, scroll_root, track_scroll};

/// The page's root element: its size is the page's viewport.
const ROOT: &str = "site-join";

const SEND_PATH: &str = "/join-requests/send";

fn muted() -> Color {
    rgb(0x98989d)
}

/// One join page's request, by link blob.
#[derive(Default)]
pub(crate) struct JoinPages(HashMap<String, Request>);

/// Where the request of one join page is.
#[derive(Default, Clone, Debug, PartialEq)]
pub(crate) struct Request {
    pub(crate) busy: bool,
    pub(crate) sent: bool,
    pub(crate) error: String,
}

fn request_of<'a>(s: &'a State, blob: &str) -> Option<&'a Request> {
    s.get::<JoinPages>()?.0.get(blob)
}

/// The web page's `hasKey`: this app reaches the daemon's identity through the harness (the
/// service token stands in for browser pairing), or, in replica mode, this device holds a key.
fn has_key(s: &State) -> bool {
    s.harness.is_some()
        || (roostr_core::account::account(s).is_some()
            && s.store.settings.as_ref().is_some_and(|x| x.has_key))
}

/// `request()`: only offered when this app holds an identity ([`has_key`]): the replica's
/// own, or the harness's.
pub(crate) fn request(s: &mut State, blob: &str) {
    let Some(link) = decode_join_link(blob) else {
        return;
    };
    if request_of(s, blob).is_some_and(|r| r.busy) {
        return;
    }
    let send: BoxFuture<anyhow::Result<()>> =
        if let Some(account) = roostr_core::account::account(s) {
            account.send_join_request(&link)
        } else if let Some(harness) = s.harness.clone() {
            let body = serde_json::to_value(&link).expect("join link serializes");
            Box::pin(async move { harness.post_json(SEND_PATH, &body).await.map(|_| ()) })
        } else {
            return;
        };
    let entry = s.slot::<JoinPages>().0.entry(blob.to_string()).or_default();
    entry.busy = true;
    entry.error.clear();
    let sending = s.rt.background(send);
    let blob = blob.to_string();
    s.rt.spawn(async move |cx| {
        let result = sending.await;
        cx.update(|s| {
            let entry = s.slot::<JoinPages>().0.entry(blob).or_default();
            match result {
                Ok(()) => entry.sent = true,
                Err(error) => entry.error = error_message(&error),
            }
            entry.busy = false;
        });
    })
    .detach();
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

fn heading(s: String) -> El {
    text(s)
        .font(ty::ui(24.).bold())
        .color(page_fg())
        .align(Align::Center)
        .wrap()
        .w_full()
}

fn muted_text(s: String) -> El {
    text(s)
        .font(ty::ui(14.5).lh(14.5 * 1.6))
        .color(muted())
        .align(Align::Center)
        .wrap()
        .w_full()
}

/// `.btn.primary`: site blue `#0a84ff`, lightening to `#3395ff` on hover; busy dims to 50%
/// and stops taking presses.
fn button(key: &'static str, label: &'static str, busy: bool) -> El {
    let blue = rgb(0x0a84ff);
    let b = row()
        .key(key)
        .role("button")
        .label(label)
        .items_center()
        .justify_center()
        .py(10.)
        .px(24.)
        .radius(8.)
        .fill(blue)
        .border(1., blue)
        .border_layout()
        .child(text(label).font(ty::ui(15.).medium()).color(Color::WHITE));
    if busy {
        b.opacity(0.5)
    } else {
        b.focusable(true)
            .cursor(Cursor::Pointer)
            .hover(rgb(0x3395ff))
    }
}

/// `<a class="btn primary" href="/app">Open Roostr Web</a>`.
fn open_app_button() -> El {
    button("open-app", "Open Roostr Web", false)
        .on_press(|s: &mut State| nav::navigate(Route::Home, s))
}

fn card(s: &State, blob: &str, max_width: f32) -> El {
    let card = col()
        .key("join-card")
        .w(420f32.min(max_width))
        .border(1., rgb(0x45454a))
        .border_layout()
        .radius(12.)
        .padding(28.)
        .fill(rgb(0x2b2b2e))
        .box_shadow(roostr_ui::shadow::gpui(18., 50., 0., 0.35))
        .items_center()
        .gap(14.);
    let Some(link) = decode_join_link(blob) else {
        return card
            .child(heading("Invalid link".into()))
            .child(muted_text("This join link is invalid or truncated.".into()));
    };
    let state = request_of(s, blob).cloned().unwrap_or_default();
    if state.sent {
        return card
            .child(heading("Request sent".into()))
            .child(muted_text(format!(
                "The owner of {} will see your request in their space settings. Once they approve, the space key is delivered to you and the space appears in your app.",
                space_name(&link, "the space")
            )))
            .child(open_app_button());
    }
    let owner: String = link.owner.chars().take(20).collect();
    let mut card = card
        .child(heading(format!(
            "Request to join {}",
            space_name(&link, "a shared space")
        )))
        .child(
            row()
                .w_full()
                .flex_wrap()
                .justify_center()
                .items_end()
                .gaps(0., 4.)
                .child(
                    text("A Roostr space owned by")
                        .font(ty::ui(14.5).lh(14.5 * 1.6))
                        .color(muted()),
                )
                .child(roostr_ui::tooltip(
                    text(format!("{owner}…"))
                        .key("owner")
                        .font(ty::mono(13.).lh(14.5 * 1.6))
                        .color(page_fg()),
                    link.owner.clone(),
                )),
        );
    if !link.relays.is_empty() {
        card = card.child(
            text(format!("via {}", link.relays.join(", ")))
                .font(ty::ui(12.5))
                .color(rgb(0x6e6e73))
                .align(Align::Center)
                .wrap()
                .w_full(),
        );
    }
    card = card.child(muted_text(
        "This link grants nothing by itself — the owner approves each request.".into(),
    ));
    card = if has_key(s) {
        let blob = blob.to_string();
        let label = if state.busy {
            "Sending…"
        } else {
            "Request to join"
        };
        let b = button("request", label, state.busy);
        card.child(if state.busy {
            b
        } else {
            b.on_press(move |s: &mut State| request(s, &blob))
        })
    } else {
        card.child(muted_text(
            "Set up your key in the app first, then reopen this link.".into(),
        ))
        .child(open_app_button())
    };
    if !state.error.is_empty() {
        card = card.child(
            text(state.error)
                .font(ty::ui(13.5))
                .color(rgb(0xff6961))
                .align(Align::Center)
                .wrap()
                .w_full(),
        );
    }
    card
}

fn space_name<'a>(link: &'a SpaceJoinLink, fallback: &'a str) -> &'a str {
    link.name
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or(fallback)
}

/// The join page for `blob`, filling its parent.
pub(crate) fn view(s: &State, cx: &mut Cx<State>, blob: &str) -> El {
    let [w, h] = cx.layout_size(ROOT).unwrap_or([1280., 800.]);
    let mut brand = row()
        .key("brand")
        .items_center()
        .gap(8.)
        .cursor(Cursor::Pointer)
        .on_press_tx(|s: &mut State, tx: &mut rune::Tx| home().open(s, tx));
    if let Some(images) = s.get::<roostr_ui::Images>() {
        let logo = images.bundled("site:logo", LOGO, 22.);
        brand = brand.child(el().size(22., 22.).pixels(logo.pixels()));
    }
    brand = brand.child(text("Roostr").font(ty::ui(17.).bold()).color(page_fg()));
    let page = row().w_full().min_h(h).justify_center().child(
        col()
            .w_full()
            .max_w(1060.)
            .px(24.)
            .child(row().items_center().py(22.).child(brand))
            .child(
                col()
                    .grow(1.)
                    .items_center()
                    .justify_center()
                    .pt(24.)
                    .pb(96.)
                    .child(card(s, blob, (w.min(1060.) - 48.).max(0.))),
            ),
    );
    let viewport = track_scroll(s, ScrollArea::vertical(JOIN_SCROLL, page).grow().w_full())
        .build(Theme::dark());
    scroll_root(
        col()
            .key(ROOT)
            .w_full()
            .h_full()
            .fill(Theme::dark().bg)
            .child(viewport),
    )
}

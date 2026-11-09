//! Roostr in a browser tab: the desktop app's views compiled for
//! `wasm32-unknown-unknown` on Rune's web target, serving all of
//! roostr.space. The website pages (`/`, `/guide`, `/privacy`, and `/j`
//! without an identity on this tab) show on their own, like the Svelte
//! site's static pages; the workspace (`/app`) starts a vault. By default the
//! tab is its own replica, like the Svelte app in browser mode: the engine
//! (`engine.wasm` beside the page), IndexedDB and the relays, with the
//! identity in `localStorage`. The alternative is the daemon and harness on
//! the visitor's own computer (`http://127.0.0.1:7333`, `:7334`), like the
//! hosted Svelte app in paired mode; the choice is kept per browser
//! ([`roostr_core::mode`]).
//!
//! `www/main.js` loads the module and calls [`run`] with the page's base path
//! (`""` at the site root). Rune draws into one canvas (WebGPU, else WebGL2):
//! a website page, or in paired mode the pairing gate until this tab holds a
//! session, then the workspace (whose own gate asks a replica without a key
//! to sign in). One [`State`] runs the whole tab: unopened while a website
//! page or the gate shows, opened ([`State::open`]) when the vault starts.

#![cfg(target_family = "wasm")]

mod pair_gate;
mod router;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use roostr_core::State;
use roostr_core::harness::Harness;
use roostr_core::mode::{self, Mode};
use roostr_core::nav::{NavEvent, Route, SitePage};
use roostr_core::web::{self as transport, WebDaemonBackend};
use roostr_replica::ReplicaBackend;
use roostr_ui::ty;
use roostr_ui::ty::Weights as _;
use rune::{AppSpec, Cursor, Cx, El, KeyEvent, Tx, col, el, text};
use wasm_bindgen::prelude::*;

/// `sessionStorage` flag: the pairing went away while the app ran; the gate
/// after the reload says so.
const UNPAIRED_FLAG: &str = "roostr-gpui.unpaired";

/// What the tab shows.
#[derive(Default)]
enum Stage {
    /// While the replica opens.
    #[default]
    Starting,
    /// A website page (the route's), before any vault opens.
    Site,
    /// The pairing gate.
    Gate,
    /// The workspace over the open vault.
    App,
    /// The replica could not open (no IndexedDB, no engine).
    Failed(String),
}

/// The page's base path, kept for the replica's engine URL.
struct Base(String);

/// Start Roostr in this page. `base` is the path the page is served under
/// (`""` at the root), so `<base>/app/object/<id>` opens that object.
#[wasm_bindgen]
pub fn run(base: String) -> Result<(), JsValue> {
    log::set_max_level(log::LevelFilter::Warn);
    let tracing = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| {
            s.get_item(roostr_core::sync_trace::STORAGE_KEY)
                .ok()
                .flatten()
        })
        .is_some_and(|v| v == "1");
    roostr_core::sync_trace::set_enabled(tracing);
    roostr_core::sync_trace!(
        "sync tracing on ({} = 1)",
        roostr_core::sync_trace::STORAGE_KEY
    );
    let base = base.trim_end_matches('/').to_string();
    let spec = AppSpec {
        on_key: Some(on_key),
        ..roostr_host::spec("Roostr", (1280., 800.), State::unopened, view)
    };
    let emoji = format!("{base}/{}", roostr_ui::fonts::EMOJI_FONT_PATH);
    rune::App::new(spec)
        .font_url(&emoji)
        .init(move || start(base))
        .try_run()
        .map_err(|error| JsValue::from_str(&error))
}

/// The tab's state: every feature registered on an unopened vault, the
/// router following the address bar, then the first route's stage.
fn start(base: String) -> State {
    let mut s = State::unopened();
    roostr_app::register(&mut s);
    s.on_nav(follow_site);
    router::start(&mut s, &base);
    let initial = router::initial_route(&base);
    s.put(Base(base));
    match initial {
        Route::Site(page) if shows_without_vault(&page) => {
            *s.slot::<Stage>() = Stage::Site;
            roostr_core::nav::navigate(Route::Site(page), &mut s);
        }
        route => {
            router::open_when_ready(&mut s, route);
            boot(&mut s);
        }
    }
    s
}

/// A choice made here wins; otherwise a tab that paired before stays
/// paired; otherwise the page's own default ([`page_mode`]); otherwise a new
/// visitor gets the browser's own replica.
fn chosen_mode() -> Mode {
    mode::saved()
        .or_else(|| transport::paired_session().map(|_| Mode::Daemon))
        .or_else(page_mode)
        .unwrap_or(Mode::Replica)
}

/// The default a server writes into the page, `<meta name="roostr-mode"
/// content="daemon">`: glonOdin's local web app (`bun run service install
/// --web`) serves the pages with it, so a new tab there pairs with this
/// computer's daemon, as the Svelte app's `dev:local` build did.
fn page_mode() -> Option<Mode> {
    let meta = web_sys::window()?
        .document()?
        .query_selector(r#"meta[name="roostr-mode"]"#)
        .ok()??;
    Mode::parse(&meta.get_attribute("content")?)
}

/// Whether `page` shows without a vault, like the Svelte site's static
/// pages. The join page needs one only to send the request: a tab with an
/// identity (a replica key, or a pairing) opens it in the workspace, others
/// show "Set up your key in the app first".
fn shows_without_vault(page: &SitePage) -> bool {
    match page {
        SitePage::Join(_) => match chosen_mode() {
            Mode::Replica => transport::saved_key().is_none(),
            Mode::Daemon => transport::paired_session().is_none(),
        },
        SitePage::Landing | SitePage::Guide | SitePage::Privacy => true,
    }
}

fn stage(s: &State) -> &Stage {
    static STARTING: Stage = Stage::Starting;
    s.get::<Stage>().unwrap_or(&STARTING)
}

/// While a website page shows: another website page replaces it (the view
/// draws the route's), a workspace route starts the vault.
fn follow_site(s: &mut State, event: &NavEvent) {
    if *event != NavEvent::Navigated || !matches!(stage(s), Stage::Site) {
        return;
    }
    let route = s.nav.route().clone();
    if !matches!(route, Route::Site(_)) {
        router::leave_site(s, route);
        boot(s);
    }
}

/// Start the vault in the chosen mode: the browser's replica, or the
/// paired daemon (the pairing gate until this tab holds a session).
fn boot(s: &mut State) {
    *s.slot::<Stage>() = Stage::Starting;
    if chosen_mode() == Mode::Replica {
        open_replica(s);
        return;
    }
    watch_pairing(s);
    if transport::paired_session().is_some() {
        start_app(s);
    } else {
        let notice = take_unpaired_flag().then(|| {
            "Pairing expired or was removed. Obtain a fresh terminal code to reconnect.".to_string()
        });
        pair_gate::open(s, notice);
        *s.slot::<Stage>() = Stage::Gate;
    }
}

/// Open this browser's replica: the engine from beside the page, the
/// IndexedDB store, the `localStorage` identity.
fn open_replica(s: &mut State) {
    let base = s.get::<Base>().map_or_else(String::new, |b| b.0.clone());
    let source = roostr_engine::WebSource::under(&format!("{base}/"));
    s.rt.spawn(async move |cx| {
        let opened = ReplicaBackend::open_browser(&source).await;
        cx.update(|s| match opened {
            Ok(backend) => start_replica(s, backend),
            Err(error) => {
                log::error!("could not open the browser replica: {error:#}");
                *s.slot::<Stage>() = Stage::Failed(format!(
                    "This browser can't keep Roostr's replica: {error:#}"
                ));
            }
        });
    })
    .detach();
}

/// The desktop start-up over the replica; no harness, so computer features
/// show their "Roostr isn't running on this computer" states.
#[allow(clippy::arc_with_non_send_sync)]
fn start_replica(s: &mut State, backend: ReplicaBackend) {
    watch_visibility(backend.clone());
    s.open(Arc::new(backend), None);
    *s.slot::<Stage>() = Stage::App;
}

/// This tab holds a session (the gate paired it).
pub(crate) fn paired(s: &mut State) {
    pair_gate::close(s);
    start_app(s);
}

/// The same start-up as the desktop binary, over the browser transport.
fn start_app(s: &mut State) {
    if matches!(stage(s), Stage::App) {
        return;
    }
    s.open(
        Arc::new(WebDaemonBackend::default()),
        Some(Arc::new(Harness::web())),
    );
    expire_with_session(s);
    *s.slot::<Stage>() = Stage::App;
}

/// Drop the session when it expires (`scheduleExpiry`).
fn expire_with_session(s: &mut State) {
    let Some(session) = transport::paired_session() else {
        return;
    };
    let left = (session.expires_at - js_sys::Date::now()).max(0.);
    s.rt.spawn(async move |cx| {
        cx.timer(Duration::from_millis(left as u64)).await;
        if transport::paired_session().is_none_or(|s| s.token == session.token) {
            transport::unpair();
        }
    })
    .detach();
}

/// React to the pairing changing here or in another tab: a running app
/// that lost it reloads into the gate; a gate whose origin got paired
/// elsewhere opens the app.
fn watch_pairing(s: &mut State) {
    let (tx, mut rx) = mpsc::unbounded::<()>();
    transport::on_pairing_change(move || {
        let _ = tx.unbounded_send(());
    });
    s.rt.spawn(async move |cx| {
        while rx.next().await.is_some() {
            let session = transport::paired_session().is_some();
            cx.update(|s| match (stage(s), session) {
                (Stage::App, false) => {
                    set_unpaired_flag();
                    reload();
                }
                (Stage::Gate, true) => paired(s),
                _ => {}
            });
        }
    })
    .detach();
}

fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match stage(s) {
        Stage::App => roostr_app::on_key(s, ev, tx),
        Stage::Site => roostr_site::on_key(s, ev, tx),
        Stage::Starting | Stage::Gate | Stage::Failed(_) => false,
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_shell::theme(s, cx);
    match stage(s) {
        Stage::App => roostr_shell::view(s, cx),
        Stage::Site => {
            let page = match s.nav.route() {
                Route::Site(page) => roostr_site::view(s, cx, page),
                _ => el(),
            };
            roostr_ui::root(col().w_full().h_full().fill(t.gutter).child(page), [])
        }
        Stage::Gate => roostr_ui::root(pair_gate::view(s, cx, t), []),
        Stage::Starting => roostr_ui::root(el().w_full().h_full().fill(t.gutter), []),
        Stage::Failed(message) => roostr_ui::root(
            col()
                .w_full()
                .h_full()
                .items_center()
                .gap(14.)
                .pt(120.)
                .px(24.)
                .fill(t.bg)
                .child(
                    text("Roostr can't open in this browser")
                        .font(ty::ui(16.).bold())
                        .color(t.fg),
                )
                .child(
                    text(message.clone())
                        .font(ty::ui(13.))
                        .color(t.muted)
                        .wrap(),
                )
                .child(
                    el().key("use-daemon")
                        .px(14.)
                        .py(8.)
                        .radius(8.)
                        .border(1., t.border)
                        .cursor(Cursor::Pointer)
                        .hover(t.hover)
                        .on_press(|_: &mut State| {
                            // The browser reloads, paired mode next.
                            let _ = mode::switch(Mode::Daemon);
                        })
                        .child(
                            text("Pair with Roostr on this computer instead")
                                .font(ty::ui(13.))
                                .color(t.fg),
                        ),
                ),
            [],
        ),
    }
}

fn session_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok()?
}

fn set_unpaired_flag() {
    if let Some(storage) = session_storage() {
        let _ = storage.set_item(UNPAIRED_FLAG, "1");
    }
}

fn take_unpaired_flag() -> bool {
    let Some(storage) = session_storage() else {
        return false;
    };
    let set = storage.get_item(UNPAIRED_FLAG).ok().flatten().is_some();
    let _ = storage.remove_item(UNPAIRED_FLAG);
    set
}

/// Coming back to the tab reconnects the relays and catches up
/// (`visibilitychange` in the Svelte app).
fn watch_visibility(backend: ReplicaBackend) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let target = document.clone();
    let listener = Closure::<dyn FnMut()>::new(move || {
        if target.visibility_state() == web_sys::VisibilityState::Visible {
            backend.resume();
        }
    });
    let _ = document
        .add_event_listener_with_callback("visibilitychange", listener.as_ref().unchecked_ref());
    // Lives as long as the page.
    listener.forget();
}

fn reload() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}

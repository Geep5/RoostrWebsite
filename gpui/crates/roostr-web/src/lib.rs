//! Roostr in a browser tab: the desktop app's views compiled for
//! `wasm32-unknown-unknown`, serving all of roostr.space. The website pages
//! (`/`, `/guide`, `/privacy`, and `/j` without an identity on this tab) show
//! on their own, like the Svelte site's static pages; the workspace (`/app`)
//! starts a vault. By default the tab is its own replica, like the Svelte
//! app in browser mode: the engine (`engine.wasm` beside the page),
//! IndexedDB and the relays, with the identity in `localStorage`. The
//! alternative is the daemon and harness on the visitor's own computer
//! (`http://127.0.0.1:7333`, `:7334`), like the hosted Svelte app in paired
//! mode; the choice is kept per browser ([`roostr_core::mode`]).
//!
//! `www/main.js` loads the module and calls [`run`] with the page's base path
//! (`""` at the site root). GPUI's web platform draws into one canvas with one
//! window: a website page, or in paired mode the pairing gate until this tab
//! holds a session, then the workspace (whose own gate asks a replica without
//! a key to sign in).

#![cfg(target_family = "wasm")]

mod pair_gate;
mod router;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::web::{CanvasFontFallback, WebBackendPreference, WebPlatform};
use gpui_kit::*;
use roostr_core::harness::Harness;
use roostr_core::mode::{self, Mode};
use roostr_core::nav::{Route, SitePage};
use roostr_core::theme;
use roostr_core::web::{self as transport, WebDaemonBackend};
use roostr_core::{Navigator, UiState};
use roostr_replica::ReplicaBackend;
use roostr_shell::{NewObject, OpenSettings, Workspace};
use wasm_bindgen::prelude::*;

use crate::pair_gate::{PairGate, Paired};

thread_local! {
    /// Keeps the application alive for the life of the page.
    static APPLICATION: RefCell<Option<ApplicationHandle>> = const { RefCell::new(None) };
}

/// `sessionStorage` flag: the pairing went away while the app ran; the gate
/// after the reload says so.
const UNPAIRED_FLAG: &str = "roostr-gpui.unpaired";

/// Start Roostr in this page. `base` is the path the page is served under
/// (`""` at the root), so `<base>/app/object/<id>` opens that object.
#[wasm_bindgen]
pub fn run(base: String) -> Result<(), JsValue> {
    gpui_kit::platform::web_init();
    log::set_max_level(log::LevelFilter::Warn);

    // Until the bundled color emoji font arrives (`load_emoji_font`), emoji
    // and any glyph the bundled fonts lack come from the browser's own fonts
    // through Canvas.
    let platform = Rc::new(WebPlatform::new_with_backend_and_font_fallback(
        false,
        WebBackendPreference::Auto,
        CanvasFontFallback::Emoji,
    ));
    let http_client = Arc::new(platform.fetch_http_client());
    let app = Application::with_platform(platform)
        .with_http_client(http_client)
        .with_assets(roostr_core::Assets);

    let handle = app.run_embedded(move |cx| {
        gpui_kit::init(cx);
        follow_reduce_motion(cx);
        // Fonts (Inter as the UI, IBM Plex Sans for `.SystemUIFont`) and the
        // palette, before the first window measures any text.
        theme::init(cx);
        // `secondary`: Command in a Mac browser (vendor/gpui-pre-web), Control
        // elsewhere.
        cx.bind_keys([
            KeyBinding::new("secondary-,", OpenSettings, None),
            KeyBinding::new("secondary-n", NewObject, None),
        ]);
        let base = base.trim_end_matches('/').to_string();
        load_emoji_font(&base, cx);
        if let Err(error) = open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| WebRoot::new(base, window, cx))
        }) {
            log::error!("could not open the Roostr window: {error:#}");
        }
        cx.activate(true);
    });
    APPLICATION.with(|application| *application.borrow_mut() = Some(handle));
    Ok(())
}

/// Fetch the color emoji font from beside the page and add it once it
/// arrives (see [`theme::EMOJI_FONT_PATH`]); the first frames do not wait.
fn load_emoji_font(base: &str, cx: &mut App) {
    let url = format!("{base}/{}", theme::EMOJI_FONT_PATH);
    cx.spawn(async move |cx| {
        let bytes = async {
            let response = transport::fetch("GET", &url, &[], None, None).await?;
            anyhow::ensure!(response.ok(), "HTTP {}", response.status());
            transport::response_bytes(&response).await
        }
        .await;
        match bytes {
            Ok(bytes) => cx.update(|cx| theme::add_emoji_font(bytes, cx)),
            Err(error) => log::warn!("could not load the emoji font {url}: {error:#}"),
        }
    })
    .detach();
}

enum Stage {
    /// While the root is being built, and while the replica opens.
    Starting,
    /// A website page, before any vault opens.
    Site(SitePage, Entity<roostr_site::SiteView>),
    Gate(Entity<PairGate>),
    App(Entity<Workspace>),
    /// The replica could not open (no IndexedDB, no engine).
    Failed(SharedString),
}

/// The window content: a website page, or the pairing gate, then the app.
struct WebRoot {
    base: String,
    stage: Stage,
    router: router::Router,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
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

impl WebRoot {
    fn new(base: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The Navigator outlives the website stage: `roostr_core::init` keeps it.
        UiState::init(cx);
        Navigator::init(cx);
        let initial = router::initial_route(&base);
        let router = router::start(&base, cx);
        let mut this = Self {
            base,
            stage: Stage::Starting,
            router,
            _tasks: Vec::new(),
            _subscriptions: Vec::new(),
        };
        match initial {
            Route::Site(page) if shows_without_vault(&page) => {
                this.show_site(page, window, cx);
            }
            route => {
                this.router.open_when_ready(route);
                this.boot(window, cx);
            }
        }
        this
    }

    /// Show `page` on its own and follow the Navigator: another website
    /// page replaces it, a workspace route starts the vault.
    fn show_site(&mut self, page: SitePage, window: &mut Window, cx: &mut Context<Self>) {
        roostr_site::init(cx);
        let navigator = Navigator::global(cx);
        self._subscriptions.push(cx.observe_in(
            &navigator,
            window,
            |this, navigator, window, cx| {
                if !matches!(this.stage, Stage::Site(..)) {
                    return;
                }
                match navigator.read(cx).route().clone() {
                    Route::Site(page) => this.set_site_page(page, window, cx),
                    route => {
                        this.router.leave_site(route, cx);
                        this.boot(window, cx);
                    }
                }
            },
        ));
        self.set_site_page(page.clone(), window, cx);
        roostr_core::nav::navigate(Route::Site(page), cx);
    }

    fn set_site_page(&mut self, page: SitePage, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(&self.stage, Stage::Site(shown, _) if *shown == page) {
            return;
        }
        let view = cx.new(|cx| roostr_site::SiteView::new(page.clone(), window, cx));
        self.stage = Stage::Site(page, view);
        cx.notify();
    }

    /// Start the vault in the chosen mode: the browser's replica, or the
    /// paired daemon (the pairing gate until this tab holds a session).
    fn boot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stage = Stage::Starting;
        cx.notify();
        if chosen_mode() == Mode::Replica {
            self.open_replica(window, cx);
            return;
        }
        self.watch_pairing(window, cx);
        if transport::paired_session().is_some() {
            self.start_app(window, cx);
        } else {
            let notice = take_unpaired_flag().then(|| {
                SharedString::from(
                    "Pairing expired or was removed. Obtain a fresh terminal code to reconnect.",
                )
            });
            self.show_gate(notice, window, cx);
        }
    }

    fn show_gate(
        &mut self,
        notice: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gate = cx.new(|cx| PairGate::new(notice, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &gate,
            window,
            |this, _, _: &Paired, window, cx| {
                this.start_app(window, cx);
            },
        ));
        self.stage = Stage::Gate(gate);
        cx.notify();
    }

    /// Open this browser's replica: the engine from beside the page, the
    /// IndexedDB store, the `localStorage` identity.
    fn open_replica(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = roostr_engine::WebSource::under(&format!("{}/", self.base));
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let opened = ReplicaBackend::open_browser(&source).await;
            let _ = this.update_in(cx, |this, window, cx| match opened {
                Ok(backend) => this.start_replica(backend, window, cx),
                Err(error) => {
                    log::error!("could not open the browser replica: {error:#}");
                    this.stage = Stage::Failed(
                        format!("This browser can't keep Roostr's replica: {error:#}").into(),
                    );
                    cx.notify();
                }
            });
        }));
    }

    /// The desktop start-up over the replica; no harness, so computer
    /// features show their "Roostr isn't running on this computer" states.
    #[allow(clippy::arc_with_non_send_sync)]
    fn start_replica(
        &mut self,
        backend: ReplicaBackend,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        watch_visibility(backend.clone());
        roostr_core::init(Arc::new(backend), None, cx);
        roostr_app::init(cx);
        let workspace = roostr_app::workspace(window, cx);
        self.stage = Stage::App(workspace);
        cx.notify();
    }

    /// The same start-up as the desktop binary, over the browser transport.
    fn start_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.stage, Stage::App(_)) {
            return;
        }
        roostr_core::init(
            Arc::new(WebDaemonBackend::default()),
            Some(Arc::new(Harness::web())),
            cx,
        );
        roostr_app::init(cx);
        let workspace = roostr_app::workspace(window, cx);
        self.expire_with_session(cx);
        self.stage = Stage::App(workspace);
        cx.notify();
    }

    /// Drop the session when it expires (`scheduleExpiry`).
    fn expire_with_session(&mut self, cx: &mut Context<Self>) {
        let Some(session) = transport::paired_session() else {
            return;
        };
        let left = (session.expires_at - js_sys::Date::now()).max(0.);
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_millis(left as u64));
        self._tasks.push(cx.spawn(async move |_, _| {
            timer.await;
            if transport::paired_session().is_none_or(|s| s.token == session.token) {
                transport::unpair();
            }
        }));
    }

    /// React to the pairing changing here or in another tab: a running app
    /// that lost it reloads into the gate; a gate whose origin got paired
    /// elsewhere opens the app.
    fn watch_pairing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (tx, mut rx) = mpsc::unbounded::<()>();
        transport::on_pairing_change(move || {
            let _ = tx.unbounded_send(());
        });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            while rx.next().await.is_some() {
                let paired = transport::paired_session().is_some();
                let Ok(()) = this.update_in(cx, |this, window, cx| match (&this.stage, paired) {
                    (Stage::App(_), false) => {
                        set_unpaired_flag();
                        reload();
                    }
                    (Stage::Gate(_), true) => this.start_app(window, cx),
                    _ => {}
                }) else {
                    return;
                };
            }
        }));
    }
}

impl Render for WebRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &self.stage {
            Stage::Starting => div().into_any_element(),
            Stage::Site(_, page) => page.clone().into_any_element(),
            Stage::Gate(gate) => gate.clone().into_any_element(),
            Stage::App(workspace) => workspace.clone().into_any_element(),
            Stage::Failed(message) => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(14.))
                .pt(px(120.))
                .px(px(24.))
                .bg(theme::bg())
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::BOLD)
                        .child("Roostr can't open in this browser"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme::muted())
                        .child(message.clone()),
                )
                .child(
                    div()
                        .id("use-daemon")
                        .px(px(14.))
                        .py(px(8.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(theme::border())
                        .cursor_pointer()
                        .text_size(px(13.))
                        .child("Pair with Roostr on this computer instead")
                        .on_click(|_, _, cx| mode::switch(Mode::Daemon, cx)),
                )
                .into_any_element(),
        };
        div()
            .size_full()
            .bg(theme::gutter())
            .font_family(theme::fonts::UI)
            .line_height(relative(theme::LINE_HEIGHT_NORMAL))
            .text_color(theme::fg())
            .child(body)
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

/// `prefers-reduced-motion: reduce`, followed live: with it on, every
/// transition snaps (`roostr_core::motion`) and the looping indicators hold
/// still, as the Svelte app's `@media (prefers-reduced-motion)` stopped them.
fn follow_reduce_motion(cx: &mut App) {
    let Some(query) = web_sys::window()
        .and_then(|window| window.match_media("(prefers-reduced-motion: reduce)").ok())
        .flatten()
    else {
        return;
    };
    let (tx, changes) = mpsc::unbounded();
    let target = query.clone();
    let listener = Closure::<dyn FnMut()>::new(move || {
        let _ = tx.unbounded_send(target.matches());
    });
    let _ = query.add_event_listener_with_callback("change", listener.as_ref().unchecked_ref());
    // Lives as long as the page.
    listener.forget();
    roostr_core::motion::follow_reduce_motion(query.matches(), changes, cx);
}

fn reload() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}

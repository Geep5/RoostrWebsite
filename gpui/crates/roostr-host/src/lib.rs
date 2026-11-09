//! Runs Roostr's [`State`] in a Rune window.
//!
//! Rune drives one state value through three hooks; [`setup`] and [`poll`] are Roostr's:
//! `setup` hands the window's waker to the executor and builds the image cache, `poll` runs
//! the executor and the event listeners each frame, fetches remote images on the desktop, and
//! asks Rune to wake at the next timer. The app binary, the web entry and every feature
//! crate's example use them, so they all run the state the same way.
//!
//! [`run_example`] is the development runner for feature crates' `examples/`: it opens a
//! window on the daemon named by the environment and shows one view in a page-like frame once
//! the store's first load lands.

use std::collections::HashSet;

use roostr_core::State;
use roostr_ui::Images;
use rune::{AppSpec, Cx, El, Tx, Waker};

#[cfg(not(target_family = "wasm"))]
pub use native::*;

/// The device scale SVG icons rasterize at before the window reports its own.
const ICON_SCALE: f32 = 2.0;

/// Remote images being fetched, so a view asking again doesn't fetch twice.
#[derive(Default)]
struct Fetching(HashSet<String>);

/// `AppSpec::setup`: the executor wakes the window through `waker`, and the image cache lives
/// in the state (views read it with `s.get::<Images>()`).
pub fn setup(s: &mut State, waker: Waker) {
    s.rt.set_waker(waker.clone());
    s.put(Images::new(waker, ICON_SCALE));
}

/// `AppSpec::poll`: runs the futures and hands out events, starts fetches of the remote images
/// views asked for, and wakes at the next timer. True when the view must re-run.
pub fn poll(s: &mut State, _clock: f64, tx: &mut Tx) -> bool {
    let ran = s.poll();
    fetch_images(s);
    if let Some(wait) = ran.next_timer {
        tx.wake_after(wait);
    }
    ran.updated
}

/// Desktop: fetch the web images views asked for (Rune has no HTTP client), sending Roostr's
/// User-Agent (Wikimedia refuses requests without one). The browser loads them itself.
fn fetch_images(s: &mut State) {
    let Some(images) = s.get::<Images>() else {
        return;
    };
    let wanted = images.take_wanted();
    if wanted.is_empty() {
        return;
    }
    for uri in wanted {
        if !s.slot::<Fetching>().0.insert(uri.clone()) {
            continue;
        }
        let fetch = s.rt.background(download(uri.clone()));
        s.rt.spawn(async move |cx| {
            let bytes = fetch.await;
            cx.update(|s| {
                s.slot::<Fetching>().0.remove(&uri);
                let Some(images) = s.get::<Images>() else {
                    return;
                };
                match bytes {
                    Ok(bytes) => images.insert(&uri, bytes),
                    Err(error) => {
                        log::debug!("image {uri}: {error}");
                        images.failed(&uri);
                    }
                }
            });
        })
        .detach();
    }
}

#[cfg(not(target_family = "wasm"))]
async fn download(uri: String) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(concat!(
            "Roostr/",
            env!("CARGO_PKG_VERSION"),
            " (+https://roostr.space)"
        ))
        .build()
        .into();
    let mut response = agent.get(&uri).call().map_err(|e| e.to_string())?;
    response.body_mut().read_to_vec().map_err(|e| e.to_string())
}

#[cfg(target_family = "wasm")]
async fn download(uri: String) -> Result<Vec<u8>, String> {
    Err(format!("{uri}: the browser loads web images itself"))
}

/// A Roostr window's spec: Roostr's fonts and the [`setup`]/[`poll`] hooks around `init` and
/// `view`.
pub fn spec(
    title: &'static str,
    size: (f32, f32),
    init: fn() -> State,
    view: fn(&State, &mut Cx<State>) -> El,
) -> AppSpec<State> {
    AppSpec {
        fonts: roostr_ui::fonts::FONTS,
        families: roostr_ui::fonts::FAMILIES,
        setup: Some(setup),
        poll: Some(poll),
        ..AppSpec::new(title, size, init, view)
    }
}

/// The daemon, blocking waits and the example runner: desktop only (a browser tab
/// can't block its one thread, and reaches its daemon through the pairing).
#[cfg(not(target_family = "wasm"))]
mod native {
    use std::sync::Arc;

    use roostr_core::daemon::{DaemonBackend, DaemonConfig};
    use roostr_core::harness::Harness;
    use roostr_core::{Connection, State};
    use roostr_ui::ThemePref;
    use rune::{Cx, El, col, el, text};

    use super::spec;

    /// The state over the daemon named by the environment (`ROOSTR_API`, `ROOSTR_API_TOKEN` or
    /// the token file `GLON_DATA` points at) with this computer's agent harness.
    pub fn daemon_state() -> Result<State, String> {
        let config = DaemonConfig::from_env().map_err(|e| format!("{e:#}"))?;
        let harness = Arc::new(Harness::new(config.clone()));
        Ok(State::new(
            Arc::new(DaemonBackend::new(config)),
            Some(harness),
        ))
    }

    /// Runs the state's futures on this thread until the store's first load lands (or fails, or
    /// `limit` passes): what a headless `--snap` needs, since its virtual clock does not wait for
    /// the network. Waits on the executor's wakes, never sleeps.
    pub fn wait_loaded(s: &mut State, limit: std::time::Duration) {
        let (woke, wakes) = std::sync::mpsc::channel::<()>();
        let woke = parking_lot::Mutex::new(woke);
        s.rt.set_waker(Arc::new(move || {
            let _ = woke.lock().send(());
        }));
        let deadline = std::time::Instant::now() + limit;
        loop {
            s.poll();
            if s.store.loaded || matches!(s.store.connection, Connection::Failed(_)) {
                return;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() || wakes.recv_timeout(left).is_err() {
                return;
            }
        }
    }

    /// How long [`settle`] waits without a wake before it calls the state settled.
    const QUIET: std::time::Duration = std::time::Duration::from_millis(600);

    /// Runs the state's futures on this thread until they go quiet: no wake for [`QUIET`] and no
    /// timer due sooner (a page's loads landed, and whatever those started), or `limit` passes.
    /// What a headless `--snap` of a whole page needs after [`wait_loaded`]: the loads the shown
    /// page started through the poll listeners. Waits on the executor's wakes, never sleeps.
    pub fn settle(s: &mut State, limit: std::time::Duration) {
        let (woke, wakes) = std::sync::mpsc::channel::<()>();
        let woke = parking_lot::Mutex::new(woke);
        s.rt.set_waker(Arc::new(move || {
            let _ = woke.lock().send(());
        }));
        let deadline = std::time::Instant::now() + limit;
        loop {
            let ran = s.poll();
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return;
            }
            let soon = ran.next_timer.filter(|t| *t < QUIET);
            let wait = soon.unwrap_or(QUIET).min(left);
            if wakes.recv_timeout(wait).is_err() && soon.is_none() {
                return;
            }
        }
    }

    /// What an example shows, kept in its state.
    struct Example {
        title: &'static str,
        view: ExampleView,
    }

    #[derive(Clone, Copy)]
    enum ExampleView {
        Plain(fn(&State, &mut Cx<State>) -> El),
        /// A view that floats menus and popovers: they go into the window root's overlay layer.
        Layered(fn(&State, &mut Cx<State>, &mut Vec<El>) -> El),
    }

    /// Opens a window on the daemon named by the environment showing `view` in a page-like frame
    /// (the window's gutter around a rounded page card), after `register` set up the feature's
    /// listeners. It opens once the store's first load lands, so a headless `--snap` shows data.
    /// Command-line arguments Rune doesn't take are in [`rune::extra_args`] (an object id, say).
    pub fn run_example(
        title: &'static str,
        register: fn(&mut State),
        view: fn(&State, &mut Cx<State>) -> El,
    ) {
        run(title, register, ExampleView::Plain(view));
    }

    /// [`run_example`] for a view that pushes overlays (menus, popovers, editors) into `layer`,
    /// which the window's root mounts above everything (`roostr_ui::root`), so window-point
    /// overlays (context menus, a rubber band) land where the pointer is.
    pub fn run_example_layered(
        title: &'static str,
        register: fn(&mut State),
        view: fn(&State, &mut Cx<State>, &mut Vec<El>) -> El,
    ) {
        run(title, register, ExampleView::Layered(view));
    }

    fn run(title: &'static str, register: fn(&mut State), view: ExampleView) {
        #[cfg(not(target_family = "wasm"))]
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
            .try_init();
        let app = rune::App::new(spec(
            title,
            (1280.0, 860.0),
            init_unconfigured,
            example_view,
        ))
        .init(move || {
            let mut s = daemon_state().unwrap_or_else(|error| panic!("{error}"));
            register(&mut s);
            wait_loaded(&mut s, std::time::Duration::from_secs(20));
            s.put(Example { title, view });
            s
        });
        app.run();
    }

    /// `AppSpec::init` must be a function; [`run_example`]'s `App::init` closure replaces it.
    fn init_unconfigured() -> State {
        unreachable!("run_example installs its own init")
    }

    fn example_view(s: &State, cx: &mut Cx<State>) -> El {
        let t = roostr_ui::theme(ThemePref::from_name(s.ui.theme_name()), cx);
        let Some(example) = s.get::<Example>() else {
            return el();
        };
        let mut layer = Vec::new();
        let body = match (&s.store.connection, s.store.loaded) {
            (_, true) => match example.view {
                ExampleView::Plain(view) => view(s, cx),
                ExampleView::Layered(view) => view(s, cx, &mut layer),
            },
            (Connection::Failed(error), false) => message(error, t.red),
            (_, false) => message("Opening your vault…", t.muted),
        };
        let page = col()
            .grow(1.0)
            .min_h(0.0)
            .mx(6.0)
            .my(6.0)
            .radius(roostr_ui::radius::PAGE)
            .fill(t.bg)
            .clip_rounded(roostr_ui::radius::PAGE)
            .child(body);
        let titlebar = el().h(38.0).w_full().window_drag().center().child(
            text(example.title)
                .font(roostr_ui::ty::ui(roostr_ui::ty::SM))
                .color(t.muted),
        );
        roostr_ui::root(
            col()
                .w_full()
                .h_full()
                .fill(t.gutter)
                .child(titlebar)
                .child(page),
            layer,
        )
    }

    fn message(line: &str, color: rune::Color) -> El {
        el().padding(16.0).child(
            text(line.to_owned())
                .font(roostr_ui::ty::ui(roostr_ui::ty::BASE))
                .color(color),
        )
    }
}

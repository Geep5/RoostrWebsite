//! Roostr desktop app: one window holding the shell (`roostr-shell`) with a
//! page per route from every feature crate, against the local glon daemon
//! named by the environment (`GLON_DATA` / `ROOSTR_API`), or - `--replica`,
//! `ROOSTR_BACKEND=replica` or the Settings choice - against this device's
//! own replica (`roostr-replica`), no daemon needed.
//!
//! `roostr [path]` opens a web path once the shell picked the space
//! (`roostr --daemon /app/object/<id>`). Rune's headless flags work too
//! (`--snap out.png --size 1440x900`): the window then opens once the vault
//! loaded and the first page's loads settled, since a snapshot's virtual
//! clock doesn't wait for the network.
//!
//! macOS's Reduce motion is Rune's: it reads the setting when the window
//! opens and whenever it regains focus, and every transition follows it.

use std::sync::Arc;
use std::time::Duration;

use roostr_core::State;
use roostr_core::create::PickFile;
use roostr_core::daemon::{DaemonBackend, DaemonConfig};
use roostr_core::harness::Harness;
use roostr_core::mode::{self, Mode};
use roostr_core::nav::Route;
use roostr_replica::ReplicaBackend;
use roostr_ui::ty::Weights as _;
use roostr_ui::{ThemePref, ty};
use rune::{AppSpec, Cursor, Cx, El, FileDialogOptions, FileDialogs, MenuItem, Tx, col, el, text};

/// The window's size at launch.
const SIZE: (f32, f32) = (1440., 900.);

/// How long a headless snapshot waits for the vault and the first page.
const HEADLESS_LIMIT: Duration = Duration::from_secs(30);

/// Shown instead of the app when its backend can't be opened.
struct ConfigError {
    title: &'static str,
    message: String,
    /// Offer the other mode (a daemon that can't be reached: this device's replica).
    switch_to: Option<Mode>,
}

/// `--replica` / `--daemon`, else `ROOSTR_BACKEND`, else the choice saved in
/// Settings, else the daemon.
fn chosen_mode() -> Mode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == mode::REPLICA_FLAG) {
        return Mode::Replica;
    }
    if args.iter().any(|a| a == mode::DAEMON_FLAG) {
        return Mode::Daemon;
    }
    std::env::var("ROOSTR_BACKEND")
        .ok()
        .and_then(|v| Mode::parse(&v))
        .or_else(mode::saved)
        .unwrap_or(Mode::Daemon)
}

/// `roostr [path]`: the web path to open (`/app/object/<id>`, `/guide`, …),
/// among the arguments Rune doesn't take.
fn route_arg() -> Option<String> {
    rune::extra_args()
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
}

/// The app's state: the vault in the chosen mode with every feature
/// registered, or the reason it can't open.
fn init() -> State {
    let opened = match chosen_mode() {
        Mode::Replica => {
            let dir = ReplicaBackend::default_dir();
            match futures::executor::block_on(ReplicaBackend::open_native(&dir)) {
                // No harness: computer features show "Roostr isn't running on this computer".
                Ok(backend) => Ok(State::new(Arc::new(backend), None)),
                Err(error) => Err(ConfigError {
                    title: "Roostr can't open this device's replica",
                    message: format!("{}: {error:#}", dir.display()),
                    switch_to: Some(Mode::Daemon),
                }),
            }
        }
        Mode::Daemon => match DaemonConfig::from_env() {
            Ok(config) => {
                let harness = Arc::new(Harness::new(config.clone()));
                Ok(State::new(
                    Arc::new(DaemonBackend::new(config)),
                    Some(harness),
                ))
            }
            Err(error) => Err(ConfigError {
                title: "Roostr can't reach its daemon",
                message: format!("{error:#}"),
                switch_to: Some(Mode::Replica),
            }),
        },
    };
    let mut s = match opened {
        Ok(s) => s,
        Err(error) => {
            let mut s = State::unopened();
            s.put(error);
            return s;
        }
    };
    roostr_app::register(&mut s);
    s.pick_file = Some(PickFile(std::rc::Rc::new(pick_file)));
    if let Some(path) = route_arg() {
        roostr_shell::open_when_ready(&mut s, Route::parse(&path));
    }
    s
}

/// `AppSpec::setup`: Roostr's hooks, then for a headless snapshot the vault's
/// first load and the first page's loads (the image cache exists from here,
/// so a cover lands in it).
fn setup(s: &mut State, waker: rune::Waker) {
    roostr_host::setup(s, waker.clone());
    if rune::is_headless() && s.get::<ConfigError>().is_none() {
        roostr_host::wait_loaded(s, HEADLESS_LIMIT);
        roostr_host::settle(s, HEADLESS_LIMIT);
        // Both waited on wakes of their own: the window's again.
        s.rt.set_waker(waker);
    }
}

/// The system's Open dialog (a sheet over the window), for "File" blocks
/// and covers.
fn pick_file(s: &mut State) -> roostr_rt::Task<Option<std::path::PathBuf>> {
    s.rt.spawn(async |_| {
        let options = FileDialogOptions {
            title: "Choose a file".into(),
            ..FileDialogOptions::default()
        };
        match FileDialogs::native().open(options).await {
            Ok(path) => path,
            Err(error) => {
                log::warn!("file dialog: {error}");
                None
            }
        }
    })
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    match s.get::<ConfigError>() {
        Some(error) => config_error(error, cx),
        None => roostr_shell::view(s, cx),
    }
}

/// The backend's failure with the way to the other mode.
fn config_error(error: &ConfigError, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut body = col()
        .w_full()
        .items_center()
        .gap(12.)
        .padding(48.)
        .child(text(error.title).font(ty::ui(16.).bold()).color(t.fg))
        .child(
            text(error.message.clone())
                .font(ty::ui(13.))
                .color(t.muted)
                .wrap(),
        );
    if let Some(mode) = error.switch_to {
        body = body.child(
            el().key("switch-mode")
                .mt(8.)
                .px(14.)
                .py(8.)
                .radius(8.)
                .border(1., t.border)
                .cursor(Cursor::Pointer)
                .hover(t.hover)
                .on_press_tx(move |_: &mut State, tx: &mut Tx| {
                    if mode::switch(mode) {
                        tx.quit();
                    }
                })
                .child(
                    text(match mode {
                        Mode::Replica => {
                            "Use Roostr without the daemon (this device's own replica)"
                        }
                        Mode::Daemon => "Use the daemon on this computer",
                    })
                    .font(ty::ui(13.))
                    .color(t.fg),
                ),
        );
    }
    // The title bar strip: room for the traffic lights, and a handle to drag.
    let titlebar = el().h(38.).w_full().window_drag();
    roostr_ui::root(
        col()
            .w_full()
            .h_full()
            .fill(t.bg)
            .child(titlebar)
            .child(body),
        [],
    )
}

fn settings(s: &mut State, _: &mut Tx) {
    if s.get::<ConfigError>().is_none() {
        roostr_shell::open_settings(s);
    }
}

fn new_object(s: &mut State, _: &mut Tx) {
    if s.get::<ConfigError>().is_none() {
        roostr_shell::new_object(s);
    }
}

fn search(s: &mut State, _: &mut Tx) {
    if s.get::<ConfigError>().is_none() {
        roostr_shell::search(s);
    }
}

fn on_key(s: &mut State, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    s.get::<ConfigError>().is_none() && roostr_app::on_key(s, ev, tx)
}

fn chord(text: &str) -> rune::Shortcut {
    text.parse().expect("a valid accelerator")
}

fn main() {
    // Warnings and errors from every crate go to stderr; `RUST_LOG` overrides.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let spec = AppSpec {
        on_key: Some(on_key),
        setup: Some(setup),
        ..roostr_host::spec("Roostr", SIZE, init, view)
    };
    // The app menu keeps the system's About Roostr, Hide and Quit Roostr (⌘Q);
    // Settings… goes after About, as in every Mac app.
    rune::App::new(spec)
        .chrome(roostr_shell::CHROME)
        .menu_item(MenuItem::separator())
        .menu_item(
            MenuItem::new("Settings…", settings)
                .id("settings")
                .key_equivalent(chord("cmd+,")),
        )
        .menu_item(
            MenuItem::new("New…", new_object)
                .id("new")
                .menu("File")
                .key_equivalent(chord("cmd+n")),
        )
        .menu_item(MenuItem::new("Search", search).id("search").menu("File"))
        .run();
}

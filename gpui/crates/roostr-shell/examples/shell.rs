//! Shell dev harness: the full workspace (rail, sidebar, tabs, header, search,
//! New menu, pinned widgets, drawers, the phone shell at narrow widths)
//! against the daemon named by the environment, with a plain page per route
//! (the object's name and its text blocks) standing in for the real pages.
//!
//! ```sh
//! GLON_DATA=/tmp/roostr-gpui-vault ROOSTR_UI_STATE=/tmp/roostr-ui-shell.json \
//!   cargo run -p roostr-shell --example shell [-- /app/object/<id>] [--snap out.png --size 1440x900]
//! ```
//!
//! Object pages publish a pane, so the Chat and Properties columns, their
//! strips, drawer tab, resizers and the header's pane button can be
//! exercised too; the pane parts show what they stand for.

use roostr_core::State;
use roostr_core::nav::Route;
use roostr_shell::{Pages, PanePart};
use roostr_ui::ty;
use roostr_ui::ty::Weights as _;
use rune::{Cx, El, col, text};

/// A stand-in page: the route's object, its type, and its text blocks.
fn page(s: &State, cx: &mut Cx<State>, route: &Route, _: &mut Vec<El>) -> El {
    let t = roostr_shell::theme(s, cx);
    let (title, detail) = match route {
        Route::Object(id) => match s.store.summary(id) {
            Some(o) => (
                format!("{} {}", o.icon, roostr_name(&o.name)),
                format!("{} · {}", o.type_key, id),
            ),
            None => match s.store.space(id) {
                Some(sp) => (sp.name.clone(), format!("Space · {id}")),
                None => ("…".to_string(), id.clone()),
            },
        },
        Route::Home => (
            "Home".to_string(),
            "Nothing open in this space yet. Pick something in the sidebar, find anything with \
             search, or make something new from the New menu."
                .to_string(),
        ),
        other => (other.path(), String::new()),
    };
    let mut body = col()
        .key("example-page")
        .w_full()
        .gap(10.)
        .child(
            text(title)
                .font(ty::ui(30.).weight(ty::BOLD))
                .color(t.fg)
                .wrap(),
        )
        .child(text(detail).font(ty::ui(13.)).color(t.muted));
    if let Some(object) = route.object_id().and_then(|id| s.store.object(id)) {
        for block in &object.blocks {
            let Some(line) = block.content.text.as_ref().map(|c| c.text.clone()) else {
                continue;
            };
            if !line.is_empty() {
                body = body.child(text(line).font(ty::ui(15.).lh(24.)).color(t.fg).wrap());
            }
        }
    }
    body
}

fn roostr_name(name: &str) -> &str {
    if name.is_empty() { "Untitled" } else { name }
}

/// The pane parts, by name.
fn pane(s: &State, cx: &mut Cx<State>, id: &str, part: PanePart, _: &mut Vec<El>) -> El {
    let t = roostr_shell::theme(s, cx);
    let name = match part {
        PanePart::Column(tab) => tab.name().to_string(),
        PanePart::Sheet => "Pane sheet".to_string(),
    };
    col()
        .key(format!("example-pane-{name}"))
        .w_full()
        .padding(16.)
        .child(
            text(format!("{name} for {id}"))
                .font(ty::ui(13.))
                .color(t.muted)
                .wrap(),
        )
}

/// An object page opens: load the object (its blocks) and publish its pane.
fn shown(s: &mut State, route: &Route) {
    if let Route::Object(id) = route {
        s.store.ensure_object(id);
        if s.store.summary(id).is_some() || s.store.space(id).is_some() {
            s.pane.publish(true, true, 3, 1);
        }
    }
}

fn init() -> State {
    let mut s = roostr_host::daemon_state().unwrap_or_else(|error| panic!("{error}"));
    roostr_shell::register(
        &mut s,
        Pages {
            page,
            pane,
            shown,
            open_settings: |_| log::info!("Settings requested"),
            ..Pages::default()
        },
    );
    if let Some(path) = rune::extra_args().iter().find(|a| a.starts_with('/')) {
        roostr_shell::open_when_ready(&mut s, Route::parse(path));
    }
    roostr_host::wait_loaded(&mut s, std::time::Duration::from_secs(20));
    // The headless snapshot's virtual clock doesn't wait for the network:
    // run the boot (space pick, the opened page's object, the pinned widgets'
    // queries) until the state goes quiet, waiting on the executor's wakes.
    let (woke, wakes) = std::sync::mpsc::channel::<()>();
    let woke = parking_lot::Mutex::new(woke);
    s.rt.set_waker(std::sync::Arc::new(move || {
        let _ = woke.lock().send(());
    }));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        s.poll();
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let quiet = left.min(std::time::Duration::from_millis(400));
        if left.is_zero() || wakes.recv_timeout(quiet).is_err() {
            break;
        }
    }
    s
}

fn main() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
    let spec = rune::AppSpec {
        on_key: Some(roostr_shell::on_key),
        ..roostr_host::spec("Roostr", (1440., 900.), init, roostr_shell::view)
    };
    rune::App::new(spec).chrome(roostr_shell::CHROME).run();
}

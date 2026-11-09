//! `cargo run -p roostr-site --example site -- landing|guide|privacy|join:<blob>`
//! (`--snap out.png --size WxH` for a headless snapshot).
//!
//! Shows one website page filling the window, as the app and the browser do. Links between
//! site pages swap the page in place (the route changes); `/app` links are logged, since this
//! window has no workspace.

use roostr_core::State;
use roostr_core::nav::{self, NavEvent, Route, SitePage};
use roostr_ui::ThemePref;
use rune::{Cx, El, el};

fn page_arg() -> SitePage {
    let arg = rune::extra_args().first().cloned().unwrap_or_default();
    match arg.as_str() {
        "guide" => SitePage::Guide,
        "privacy" => SitePage::Privacy,
        other => match other.strip_prefix("join:") {
            Some(blob) => SitePage::Join(blob.to_string()),
            None => SitePage::Landing,
        },
    }
}

fn init() -> State {
    let mut s = roostr_host::daemon_state().unwrap_or_else(|_| State::unopened());
    roostr_site::register(&mut s);
    s.on_nav(log_app_routes);
    nav::navigate(Route::Site(page_arg()), &mut s);
    s
}

fn log_app_routes(s: &mut State, event: &NavEvent) {
    if *event == NavEvent::Navigated && !matches!(s.nav.route(), Route::Site(_)) {
        println!("navigate: {}", s.nav.route().path());
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    roostr_ui::theme(ThemePref::Dark, cx);
    let page = match s.nav.route() {
        Route::Site(page) => roostr_site::view(s, cx, page),
        _ => el(),
    };
    roostr_ui::root(el().w_full().h_full().child(page), [])
}

fn main() {
    let spec = rune::AppSpec {
        on_key: Some(roostr_site::on_key),
        ..roostr_host::spec("Roostr", (1280.0, 860.0), init, view)
    };
    rune::App::new(spec).run();
}

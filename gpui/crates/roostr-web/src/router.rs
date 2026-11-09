//! The address bar, the tab title and the app's routes. The page's path
//! below the base (`/app/object/<id>`, `/guide`, `/` the landing) names the
//! first route: a website page shows at once, before any vault opens; a
//! workspace route opens once the shell has picked a space. Every navigation
//! then pushes its web path (`Route::path`) onto the browser history, and
//! Back/Forward navigate to the popped path. A URL that already names the
//! route stays as loaded (`/j?utm=x#r2.…`), and `document.title` follows
//! the shown route (`Route::title`), an object page its object's name.

use futures::StreamExt as _;
use futures::channel::mpsc;
use roostr_core::State;
use roostr_core::StoreEvent;
use roostr_core::nav::{self, NavEvent, Route};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

/// The current location as an app path (`/app/object/<id>`, `/guide`,
/// `/j#<blob>`); the base itself is the landing, `/`.
fn current_path(base: &str) -> String {
    let Some(location) = web_sys::window().map(|w| w.location()) else {
        return "/".into();
    };
    let pathname = location.pathname().unwrap_or_default();
    let rest = pathname.strip_prefix(base).unwrap_or(&pathname);
    let rest = rest.trim_end_matches('/');
    let rest = if rest.is_empty() { "/" } else { rest };
    let search = location.search().unwrap_or_default();
    let hash = location.hash().unwrap_or_default();
    format!("{rest}{search}{hash}")
}

/// The route the page's location names.
pub fn initial_route(base: &str) -> Route {
    Route::parse(&current_path(base))
}

fn history() -> Option<web_sys::History> {
    web_sys::window()?.history().ok()
}

/// What the address bar shows, in the state.
#[derive(Default)]
struct Bar {
    base: String,
    /// The URL shown (below the base), so a popped entry is not pushed again.
    shown: String,
    /// The first route the app settles on replaces the loaded URL instead of
    /// adding a history entry.
    first: bool,
    /// A workspace route to open once the shell has picked a space.
    pending: Option<Route>,
    /// The title set, so an unchanged one is not written again.
    title: String,
}

/// Follow the Navigator into the address bar and the browser's Back/Forward
/// into the Navigator. Call once.
pub fn start(s: &mut State, base: &str) {
    s.put(Bar {
        base: base.to_string(),
        shown: current_path(base),
        first: true,
        pending: None,
        title: String::new(),
    });
    s.on_nav(on_nav);
    // An object page's title tracks its name.
    s.on_store(|s, _: &StoreEvent| retitle(s));

    let (tx, mut rx) = mpsc::unbounded::<String>();
    let on_pop = Closure::<dyn Fn()>::new({
        let base = base.to_string();
        move || {
            let _ = tx.unbounded_send(current_path(&base));
        }
    });
    if let Some(window) = web_sys::window() {
        let _ =
            window.add_event_listener_with_callback("popstate", on_pop.as_ref().unchecked_ref());
    }
    // Lives as long as the page.
    on_pop.forget();
    s.rt.spawn(async move |cx| {
        while let Some(path) = rx.next().await {
            cx.update(|s| {
                let route = Route::parse(&path);
                s.slot::<Bar>().shown = path;
                nav::navigate(route, s);
            });
        }
    })
    .detach();
}

/// Open `route` once the shell has picked a space (its saved tab strip
/// would replace a route opened earlier).
pub fn open_when_ready(s: &mut State, route: Route) {
    s.slot::<Bar>().pending = Some(route);
}

/// Leave a website page for `route`, a workspace route whose vault is
/// only now starting: the address bar names it at once.
pub fn leave_site(s: &mut State, route: Route) {
    show(s, &route);
    open_when_ready(s, route);
}

fn on_nav(s: &mut State, _: &NavEvent) {
    if s.nav.space().is_empty() {
        // No space yet: the only routes are the website pages shown before
        // a vault opens. Anything else is the shell starting.
        if !matches!(s.nav.route(), Route::Site(_)) || s.slot::<Bar>().pending.is_some() {
            return;
        }
    } else if let Some(route) = s.slot::<Bar>().pending.take() {
        nav::navigate(route, s);
    }
    let route = s.nav.route().clone();
    show(s, &route);
}

fn show(s: &mut State, route: &Route) {
    let bar = s.slot::<Bar>();
    if !route.shown_by(&bar.shown) {
        let path = route.path();
        let url = format!("{}{path}", bar.base);
        if let Some(history) = history() {
            let result = if bar.first {
                history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url))
            } else {
                history.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url))
            };
            if result.is_err() {
                log::warn!("could not show {url} in the address bar");
            }
        }
        bar.shown = path;
    }
    bar.first = false;
    retitle(s);
}

/// Title the tab for the shown route.
fn retitle(s: &mut State) {
    let route = Route::parse(&s.slot::<Bar>().shown);
    let name = match &route {
        Route::Object(id) => s.store.object(id).map(|object| object.name().to_string()),
        _ => None,
    };
    let title = route.title(name.as_deref());
    let bar = s.slot::<Bar>();
    if bar.title == title {
        return;
    }
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        document.set_title(&title);
    }
    bar.title = title;
}

//! The address bar, the tab title and the app's routes. The page's path
//! below the base (`/app/object/<id>`, `/guide`, `/` the landing) names the
//! first route: a website page shows at once, before any vault opens; a
//! workspace route opens once the shell has picked a space. Every navigation
//! then pushes its web path (`Route::path`) onto the browser history, and
//! Back/Forward navigate to the popped path. A URL that already names the
//! route stays as loaded (`/j?utm=x#r2.…`), and `document.title` follows
//! the shown route (`Route::title`), an object page its object's name.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{App, Entity, EntityId, Subscription};
use roostr_core::nav::{self, Route};
use roostr_core::{Navigator, Store};
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

/// What the address bar shows; shared by the Navigator observer, the
/// Back/Forward listener and [`Router`].
struct Bar {
    base: String,
    /// The URL shown (below the base), so a popped entry is not pushed again.
    shown: RefCell<String>,
    /// The first route the app settles on replaces the loaded URL instead of
    /// adding a history entry.
    first: Cell<bool>,
    /// A workspace route to open once the shell has picked a space.
    pending: RefCell<Option<Route>>,
    /// The title set, so an unchanged one is not written again.
    title: RefCell<String>,
    /// The store whose object names the title follows, once it exists.
    store: RefCell<Option<(EntityId, Subscription)>>,
}

impl Bar {
    fn show(self: &Rc<Self>, route: &Route, cx: &mut App) {
        if !route.shown_by(&self.shown.borrow()) {
            let path = route.path();
            let url = format!("{}{path}", self.base);
            if let Some(history) = history() {
                let result = if self.first.get() {
                    history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url))
                } else {
                    history.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url))
                };
                if result.is_err() {
                    log::warn!("could not show {url} in the address bar");
                }
            }
            *self.shown.borrow_mut() = path;
        }
        self.first.set(false);
        self.retitle(cx);
    }

    /// Title the tab for the shown route, following the store (created
    /// with the vault) so an object page's title tracks its name.
    fn retitle(self: &Rc<Self>, cx: &mut App) {
        if let Some(store) = Store::try_global(cx)
            && self.store.borrow().as_ref().map(|(id, _)| *id) != Some(store.entity_id())
        {
            let subscription = cx.observe(&store, {
                let bar = self.clone();
                move |_: Entity<Store>, cx| bar.retitle(cx)
            });
            *self.store.borrow_mut() = Some((store.entity_id(), subscription));
        }
        let route = Route::parse(&self.shown.borrow());
        let name = match &route {
            Route::Object(id) => Store::try_global(cx)
                .and_then(|store| store.read(cx).object(id))
                .map(|object| object.name().to_string()),
            _ => None,
        };
        let title = route.title(name.as_deref());
        if *self.title.borrow() == title {
            return;
        }
        if let Some(document) = web_sys::window().and_then(|w| w.document()) {
            document.set_title(&title);
        }
        *self.title.borrow_mut() = title;
    }
}

/// The running address-bar follower.
pub struct Router(Rc<Bar>);

impl Router {
    /// Open `route` once the shell has picked a space (its saved tab strip
    /// would replace a route opened earlier).
    pub fn open_when_ready(&self, route: Route) {
        *self.0.pending.borrow_mut() = Some(route);
    }

    /// Leave a website page for `route`, a workspace route whose vault is
    /// only now starting: the address bar names it at once.
    pub fn leave_site(&self, route: Route, cx: &mut App) {
        self.0.show(&route, cx);
        self.open_when_ready(route);
    }
}

/// Follow the Navigator into the address bar and the browser's Back/Forward
/// into the Navigator. Call once, after the Navigator exists.
pub fn start(base: &str, cx: &mut App) -> Router {
    let bar = Rc::new(Bar {
        base: base.to_string(),
        shown: RefCell::new(current_path(base)),
        first: Cell::new(true),
        pending: RefCell::new(None),
        title: RefCell::new(String::new()),
        store: RefCell::new(None),
    });

    let navigator = Navigator::global(cx);
    cx.observe(&navigator, {
        let bar = bar.clone();
        move |navigator, cx| {
            if navigator.read(cx).space().is_empty() {
                // No space yet: the only routes are the website pages shown
                // before a vault opens. Anything else is the shell starting.
                let route = navigator.read(cx).route();
                if !matches!(route, Route::Site(_)) || bar.pending.borrow().is_some() {
                    return;
                }
            } else {
                let pending = bar.pending.borrow_mut().take();
                if let Some(route) = pending {
                    nav::navigate(route, cx);
                }
            }
            let route = navigator.read(cx).route().clone();
            bar.show(&route, cx);
        }
    })
    .detach();

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
    cx.spawn({
        let bar = bar.clone();
        async move |cx| {
            while let Some(path) = rx.next().await {
                let route = Route::parse(&path);
                *bar.shown.borrow_mut() = path;
                cx.update(|cx| nav::navigate(route, cx));
            }
        }
    })
    .detach();
    Router(bar)
}

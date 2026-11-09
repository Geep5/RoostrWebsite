//! The Roostr app as both front ends run it: the desktop binary (`roostr`)
//! and the browser build (`roostr-web`). Every feature crate's listeners,
//! the page for each route, and the object page that dispatches on an
//! object's type, plugged into the shell ([`roostr_shell::Pages`]).

mod object_page;

use roostr_core::State;
use roostr_core::nav::Route;
use roostr_shell::{Pages, PanePart};
use rune::{Cx, El, KeyEvent, Tx};

pub use crate::object_page::Pages as ObjectPages;

/// Install every feature crate's listeners and the shell with every page.
/// Call once on a new [`State`].
pub fn register(s: &mut State) {
    roostr_properties::register(s);
    roostr_editor::register(s);
    roostr_sets::register(s);
    roostr_discussion::register(s);
    roostr_graph::register(s);
    roostr_settings::register(s);
    roostr_site::register(s);
    object_page::register(s);
    roostr_shell::register(s, pages());
}

/// What the app plugs into the shell.
pub fn pages() -> Pages {
    Pages {
        page,
        pane,
        identity_gate,
        open_settings: roostr_settings::open_settings,
        overlays,
        shown,
        closed,
    }
}

/// The main area's view for each route.
fn page(s: &State, cx: &mut Cx<State>, route: &Route, layer: &mut Vec<El>) -> El {
    let t = roostr_shell::theme(s, cx);
    match route {
        Route::Home => object_page::home(s, cx, t, layer),
        Route::Object(id) => object_page::view(s, cx, t, layer, id),
        Route::Graph { .. } => roostr_graph::view(s, cx),
        Route::Chat(id) => roostr_discussion::chat_page(
            s,
            cx,
            t,
            layer,
            id,
            object_page::window_size(s),
        ),
        Route::Site(page) => roostr_site::view(s, cx, page),
    }
}

fn pane(s: &State, cx: &mut Cx<State>, id: &str, part: PanePart, layer: &mut Vec<El>) -> El {
    let t = roostr_shell::theme(s, cx);
    object_page::pane(s, cx, t, layer, id, part)
}

fn identity_gate(s: &State, cx: &mut Cx<State>, layer: &mut Vec<El>) -> El {
    let t = roostr_shell::theme(s, cx);
    roostr_settings::identity_gate(s, cx, t, layer)
}

/// Settings, over everything else.
fn overlays(s: &State, cx: &mut Cx<State>, layer: &mut Vec<El>) {
    let t = roostr_shell::theme(s, cx);
    roostr_settings::settings_modal(s, cx, t, layer);
}

fn shown(s: &mut State, route: &Route) {
    if let Route::Object(id) = route {
        object_page::shown(s, id);
    }
}

fn closed(s: &mut State, route: &Route) {
    if let Route::Object(id) = route {
        object_page::closed(s, id);
    }
}

/// The window's `on_key`: the shell's shortcuts, a website page's keyboard
/// scrolling, a table's sideways arrows, and Escape nobody else took.
pub fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if roostr_shell::on_key(s, ev, tx) {
        return true;
    }
    if matches!(s.nav.route(), Route::Site(_)) && roostr_site::on_key(s, ev, tx) {
        return true;
    }
    if roostr_sets::on_key(s, ev, tx) {
        return true;
    }
    ev.key == rune::Key::Escape && object_page::on_escape(s)
}

//! The Roostr app as both front ends run it: the desktop binary (`roostr`)
//! and the browser build (`roostr-web`). Feature initialization, the page
//! for each route, and the object page that dispatches on an object's type.

mod object_page;

use std::rc::Rc;

use gpui_kit::*;
use roostr_core::nav::Route;
use roostr_shell::{PageFactory, Workspace};

pub use crate::object_page::{HomePage, ObjectPage};

/// Install every feature crate's globals. Call after [`roostr_core::init`].
pub fn init(cx: &mut App) {
    roostr_properties::init(cx);
    roostr_editor::init(cx);
    roostr_sets::init(cx);
    roostr_discussion::init(cx);
    roostr_graph::init(cx);
    roostr_settings::init(cx);
    roostr_site::init(cx);
    roostr_shell::init(cx);
}

/// The main area's view for each route.
pub fn pages() -> PageFactory {
    Rc::new(
        |route: &Route, window: &mut Window, cx: &mut App| -> AnyView {
            match route {
                Route::Home => cx.new(|cx| HomePage::new(window, cx)).into(),
                Route::Object(id) => cx.new(|cx| ObjectPage::new(id.clone(), window, cx)).into(),
                Route::Graph { focus } => cx
                    .new(|cx| roostr_graph::GraphView::new(focus.clone(), window, cx))
                    .into(),
                Route::Chat(id) => cx
                    .new(|cx| roostr_discussion::ChatPage::new(id.clone(), window, cx))
                    .into(),
                Route::Site(page) => cx
                    .new(|cx| roostr_site::SiteView::new(page.clone(), window, cx))
                    .into(),
            }
        },
    )
}

/// The shell with every page, Settings and the identity gate.
pub fn workspace(window: &mut Window, cx: &mut App) -> Entity<Workspace> {
    cx.new(|cx| {
        let mut workspace = Workspace::new(pages(), window, cx);
        workspace.on_open_settings(roostr_settings::open_settings);
        workspace.identity_gate(|window, cx| {
            cx.new(|cx| roostr_settings::IdentityGate::new(window, cx))
                .into()
        });
        workspace
    })
}

/// Open `route` in the active tab once the shell has picked the space (a
/// path given at launch: `/app/object/<id>`, `/guide`, …).
pub fn open_when_ready(route: Route, cx: &mut App) {
    let nav = roostr_core::Navigator::global(cx);
    let mut pending = Some(route);
    cx.observe(&nav, move |nav, cx| {
        if !nav.read(cx).space().is_empty()
            && let Some(route) = pending.take()
        {
            roostr_core::nav::navigate(route, cx);
        }
    })
    .detach();
}

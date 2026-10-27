//! The mobile shell (`routes/app/+layout.svelte` `{#if isMobile}`) in a GPUI
//! test window at iPhone portrait width - Spaces list (pull to sync, hold to
//! reorder) → space home (bottom ＋) → object (back, sync dot), chat objects
//! opening full-screen - and the desktop layout again at landscape width.
#![cfg(not(target_family = "wasm"))]

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    LongPressEvent, Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, TouchPhase,
    VisualTestContext,
};
use gpui_kit::{Pixels, Point, point, px, size};
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::layout::SyncNow;
use roostr_core::nav::{Navigator, Route, navigate};
use roostr_core::store::Store;
use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_shell::Workspace;
use serde_json::Value as Json;

/// (id, type, name) of the objects in the Personal space.
const OBJECTS: [(&str, &str, &str); 2] =
    [("page-1", "page", "Notes"), ("chat-1", "chat", "Team chat")];

/// Two spaces; Personal holds a pinned page and a chat.
#[derive(Default)]
struct Vault {
    mutations: Mutex<Vec<String>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "two spaces".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(OBJECTS
            .iter()
            .enumerate()
            .map(|(i, (id, type_key, name))| ObjectSummary {
                id: (*id).into(),
                type_key: (*type_key).into(),
                name: (*name).into(),
                channel_id: "sp-1".into(),
                updated_at: 1_000 + i as i64,
                ..ObjectSummary::default()
            })
            .collect()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let object = OBJECTS
            .iter()
            .find(|(oid, _, _)| *oid == id)
            .map(|(id, type_key, name)| Object {
                id: (*id).into(),
                type_key: (*type_key).into(),
                fields: [
                    ("name".to_string(), Value::string(*name)),
                    ("channel".to_string(), Value::string("sp-1")),
                ]
                .into(),
                ..Object::default()
            });
        ready(object.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![
            Space {
                id: "sp-1".into(),
                name: "Personal".into(),
                pinned_ids: vec!["page-1".into()],
                created_at: 1,
                ..Space::default()
            },
            Space {
                id: "sp-2".into(),
                name: "Work".into(),
                created_at: 2,
                ..Space::default()
            },
        ]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, _: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        ready(Ok(QueryResult::default()))
    }
    fn mutate(&self, action: &str, _: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        self.mutations.lock().push(action.to_string());
        ready(Ok(JsonMap::new()))
    }
    fn serving(&self, _: &[String]) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<anyhow::Result<BackendEvent>> {
        Box::pin(futures::stream::pending())
    }
}

fn shown(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

/// Let running transitions (a sheet rising) finish.
fn settle_motion(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn center(cx: &mut VisualTestContext, selector: &'static str) -> Point<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
        .center()
}

fn tap(cx: &mut VisualTestContext, selector: &'static str) {
    let at = center(cx, selector);
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
}

fn route(cx: &mut VisualTestContext) -> Route {
    cx.update(|_, cx| Navigator::global(cx).read(cx).route().clone())
}

fn pan(cx: &mut VisualTestContext, at: Point<Pixels>, phase: TouchPhase, dy: f32) {
    cx.simulate_event(ScrollEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Modifiers::none(),
        touch_phase: phase,
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
}

fn hold(cx: &mut VisualTestContext, start: Point<Pixels>, at: Point<Pixels>, phase: TouchPhase) {
    cx.simulate_event(LongPressEvent {
        phase,
        start_position: start,
        position: at,
    });
    cx.run_until_parked();
}

#[gpui::test]
fn phone_width_runs_the_mobile_shell(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    let backend = Arc::new(Vault::default());
    let syncs = Rc::new(Cell::new(0));
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(backend.clone(), None, cx);
        roostr_app::init(cx);
        let syncs = syncs.clone();
        cx.set_global(SyncNow(Rc::new(move |_| syncs.set(syncs.get() + 1))));
    });
    let (_workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(390.), px(844.)));
    cx.run_until_parked();

    // Screen 1: the Spaces list, in creation order.
    assert!(shown(cx, "m-shell") && !shown(cx, "desk"));
    assert!(shown(cx, "m-spaces"));
    assert!(center(cx, "m-card-sp-1").y < center(cx, "m-card-sp-2").y);

    // Pulling the list down past 64px at its top syncs now; a short pull doesn't.
    let list = center(cx, "m-cards");
    pan(cx, list, TouchPhase::Started, 20.);
    pan(cx, list, TouchPhase::Moved, 30.);
    pan(cx, list, TouchPhase::Moved, 30.);
    pan(cx, list, TouchPhase::Ended, 0.);
    assert_eq!(syncs.get(), 1, "a long pull syncs");
    pan(cx, list, TouchPhase::Started, 20.);
    pan(cx, list, TouchPhase::Ended, 0.);
    assert_eq!(syncs.get(), 1, "a short pull does not");

    // Holding Work and dropping it on Personal's top half moves it first.
    let work = center(cx, "m-card-sp-2");
    let personal = cx.debug_bounds("m-card-sp-1").unwrap();
    let above = point(personal.center().x, personal.top() + px(4.));
    hold(cx, work, work, TouchPhase::Started);
    hold(cx, work, above, TouchPhase::Moved);
    hold(cx, work, above, TouchPhase::Ended);
    let order = cx.update(|_, cx| Store::global(cx).read(cx).space_order.clone());
    assert_eq!(order.get("sp-2"), Some(&-999.), "dropped before Personal");
    assert!(center(cx, "m-card-sp-2").y < center(cx, "m-card-sp-1").y);
    assert!(
        shown(cx, "m-spaces"),
        "a lift is a reorder, never navigation"
    );

    // Screen 2: a tap opens the space home; ＋ toggles the New menu.
    tap(cx, "m-card-sp-1");
    assert!(shown(cx, "m-space-home"));
    assert_eq!(
        cx.update(|_, cx| Navigator::global(cx).read(cx).space().to_string()),
        "sp-1"
    );
    tap(cx, "m-compose");
    assert!(shown(cx, "m-create-menu"));
    tap(cx, "m-compose");
    assert!(!shown(cx, "m-create-menu"));

    // Search is a bottom sheet below a 56px strip; a short pull snaps
    // back, a pull past 110px closes it.
    tap(cx, "m-search");
    // It rises into place.
    settle_motion(cx);
    let sheet = cx.debug_bounds("search-modal").expect("the search sheet");
    assert_eq!((sheet.top(), sheet.bottom()), (px(56.), px(844.)));
    let inside = sheet.center();
    pan(cx, inside, TouchPhase::Started, 20.);
    pan(cx, inside, TouchPhase::Moved, 20.);
    assert_eq!(
        cx.debug_bounds("search-modal").unwrap().top(),
        px(96.),
        "it follows the pull"
    );
    pan(cx, inside, TouchPhase::Ended, 0.);
    assert!(shown(cx, "search-modal"), "a short pull snaps back");
    pan(cx, inside, TouchPhase::Started, 20.);
    pan(cx, inside, TouchPhase::Moved, 100.);
    pan(cx, inside, TouchPhase::Ended, 0.);
    assert!(!shown(cx, "search-modal"), "a long pull closes it");

    // An object: back header, sync dot (a tap syncs now), ⋯.
    cx.update(|_, cx| navigate(Route::Object("page-1".into()), cx));
    cx.run_until_parked();
    assert!(shown(cx, "m-back") && shown(cx, "m-sync") && shown(cx, "m-more"));
    // The chat & properties pane opens as a full-screen sheet, and each
    // object opens without it.
    assert!(!shown(cx, "disc-sheet"));
    tap(cx, "m-pane");
    let sheet = cx.debug_bounds("disc-sheet").expect("the pane sheet");
    assert_eq!(sheet.size, size(px(390.), px(844.)));
    cx.update(|_, cx| navigate(Route::Home, cx));
    cx.run_until_parked();
    cx.update(|_, cx| navigate(Route::Object("page-1".into()), cx));
    cx.run_until_parked();
    assert!(!shown(cx, "disc-sheet"), "an object opens on its own page");
    tap(cx, "m-sync");
    assert_eq!(syncs.get(), 2, "the sync dot syncs now");
    // ⋯ leads with Graph; the graph's back returns to the object.
    tap(cx, "m-more");
    tap(cx, "more-graph");
    assert_eq!(
        route(cx),
        Route::Graph {
            focus: Some("page-1".into())
        }
    );
    tap(cx, "m-back");
    assert_eq!(route(cx), Route::Object("page-1".into()));
    // Back goes to the object's space home, not history.
    tap(cx, "m-back");
    assert_eq!(route(cx), Route::Home);
    assert!(shown(cx, "m-space-home"));

    // A chat object opens the full-screen chat page in its place.
    cx.update(|_, cx| navigate(Route::Object("chat-1".into()), cx));
    cx.run_until_parked();
    assert_eq!(route(cx), Route::Chat("chat-1".into()));
    cx.update(|_, cx| Navigator::global(cx).update(cx, |nav, cx| nav.back(cx)));
    cx.run_until_parked();
    assert_eq!(
        route(cx),
        Route::Home,
        "the object page was replaced, not stacked"
    );

    // iPhone landscape is wider than 720pt: the desktop layout.
    cx.simulate_resize(size(px(874.), px(402.)));
    cx.run_until_parked();
    assert!(shown(cx, "desk") && !shown(cx, "m-shell"));
    let mutations = backend.mutations.lock().clone();
    assert!(
        mutations.iter().any(|m| m == "set_field"),
        "the moved space's position was never saved: {mutations:?}"
    );
}

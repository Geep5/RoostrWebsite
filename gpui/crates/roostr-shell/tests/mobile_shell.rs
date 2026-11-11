//! The mobile shell (`routes/app/+layout.svelte` `{#if isMobile}`) headless
//! at iPhone portrait width - Spaces list (pull to sync, hold to reorder) →
//! space home (bottom ＋, search sheet) → object (back, sync dot, ⋯, the pane
//! sheet), the full-screen chat - and the desktop layout again at landscape
//! width.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use parking_lot::Mutex;
use roostr_core::layout::SyncNow;
use roostr_core::nav::{Route, TABS_WRITE_DELAY, navigate, open_tab};
use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream};
use rune::TouchPhase;
use serde_json::Value as Json;

use common::{Harness, pane_pages};

/// (id, type, name) of the objects in the Personal space.
const OBJECTS: [(&str, &str, &str); 2] =
    [("page-1", "page", "Notes"), ("chat-1", "chat", "Team chat")];

/// Two spaces; Personal holds a pinned page and a chat.
#[derive(Default)]
struct Vault {
    mutations: Mutex<Vec<String>>,
}

struct Shared(Arc<Vault>);

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

impl Backend for Shared {
    fn describe(&self) -> String {
        "two spaces".into()
    }
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
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
    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
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
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
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
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, _: &Json) -> BoxFuture<Result<QueryResult>> {
        ready(Ok(QueryResult::default()))
    }
    fn mutate(&self, action: &str, _: JsonMap) -> BoxFuture<Result<JsonMap>> {
        self.0.mutations.lock().push(action.to_string());
        ready(Ok(JsonMap::new()))
    }
    fn serving(&self, _: &[String]) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        Box::pin(futures::stream::pending())
    }
}

fn center(h: &Harness, key: &str) -> [f32; 2] {
    let r = h
        .rect(key)
        .unwrap_or_else(|| panic!("{key} is not on screen"));
    [r[0] + r[2] / 2., r[1] + r[3] / 2.]
}

fn route(h: &Harness) -> Route {
    h.state().nav.route().clone()
}

/// A finger at `at` moving down by each of `steps`, then lifted after a pause.
fn pull(h: &mut Harness, at: [f32; 2], steps: &[f32]) {
    let mut y = at[1];
    let mut time = 0.;
    h.rt.touch(1, TouchPhase::Started, at[0], y, time);
    for dy in steps {
        y += dy;
        time += 0.016;
        h.rt.touch(1, TouchPhase::Moved, at[0], y, time);
        h.frame();
    }
    h.rt.touch(1, TouchPhase::Ended, at[0], y, time + 0.3);
    h.settle();
}

#[test]
fn phone_width_runs_the_mobile_shell() {
    let vault = Arc::new(Vault::default());
    let mut h = Harness::with_pages(Arc::new(Shared(vault.clone())), (390., 844.), pane_pages());
    let syncs = Rc::new(Cell::new(0));
    let counter = syncs.clone();
    h.update(move |s| {
        s.sync_now = Some(SyncNow(Rc::new(move |_| counter.set(counter.get() + 1))));
        navigate(Route::Home, s);
    });

    // Screen 1: the Spaces list, in creation order.
    assert!(h.has("m-shell") && !h.has("desk"));
    assert!(h.has("m-spaces"));
    assert!(center(&h, "m-card-sp-1")[1] < center(&h, "m-card-sp-2")[1]);

    // Pulling the list down past 64px at its top syncs now; a short pull doesn't.
    let list = center(&h, "m-cards");
    pull(&mut h, list, &[20., 30., 30.]);
    assert_eq!(syncs.get(), 1, "a long pull syncs");
    pull(&mut h, list, &[20.]);
    assert_eq!(syncs.get(), 1, "a short pull does not");
    // The same from a card, whose drag scrolls the list by hand.
    let card = center(&h, "m-card-sp-1");
    pull(&mut h, card, &[20., 30., 30.]);
    assert_eq!(syncs.get(), 2, "a long pull from a card syncs");
    assert!(h.has("m-spaces"), "a pull opens nothing");

    // Holding Work and dropping it on Personal's top half moves it first.
    let work = center(&h, "m-card-sp-2");
    let personal = h.rect("m-card-sp-1").unwrap();
    let above = [work[0], personal[1] + 4.];
    h.rt.pointer_move(work[0], work[1]);
    h.rt.pointer_down(work[0], work[1]);
    h.frame();
    h.advance(Duration::from_millis(600));
    h.rt.pointer_move(above[0], above[1]);
    h.frame();
    h.rt.pointer_up(above[0], above[1]);
    h.settle();
    assert_eq!(
        h.state().store.space_order.get("sp-2"),
        Some(&-999.),
        "dropped before Personal"
    );
    assert!(center(&h, "m-card-sp-2")[1] < center(&h, "m-card-sp-1")[1]);
    assert!(h.has("m-spaces"), "a lift is a reorder, never navigation");

    // A tap opens the space home - its pinned cards; nothing opens by
    // itself on a phone (the desktop opens the top pin). ＋ toggles the New menu.
    h.click("m-card-sp-1");
    assert!(h.has("m-space-home"));
    assert_eq!(route(&h), Route::Home);
    assert_eq!(h.state().nav.space(), "sp-1");
    assert!(h.has("m-wcard-page-1"), "Personal's pinned card");
    h.click("m-compose");
    assert!(h.has("m-create-menu"));
    h.click("m-compose");
    assert!(!h.has("m-create-menu"));

    // An object: back header, sync dot (a tap syncs now), ⋯.
    h.update(|s| navigate(Route::Object("page-1".into()), s));
    assert!(h.has("m-back") && h.has("m-sync") && h.has("m-more"));
    // The chat & properties pane opens as a full-screen sheet over the whole
    // phone shell, only on an object with a pane.
    assert!(!h.has("disc-sheet"));
    h.click("m-pane");
    // It rises open on a spring (virtual time).
    h.frames(60);
    assert_eq!(
        h.rect("disc-sheet").expect("the pane sheet"),
        h.rect("m-shell").unwrap()
    );
    assert!(h.has("test-pane-sheet"));
    h.update(|s| navigate(Route::Home, s));
    assert!(!h.has("disc-sheet"), "the sheet belongs to the object");
    // (Each object opening without it is the app's object page: it closes the
    // sheet when shown.)
    h.update(|s| {
        s.pane.set_sheet(false);
        navigate(Route::Object("page-1".into()), s);
    });
    assert!(!h.has("disc-sheet"));
    h.click("m-sync");
    assert_eq!(syncs.get(), 3, "the sync dot syncs now");
    // ⋯ leads with Graph; the graph's back returns to the object.
    h.click("m-more");
    h.click("more-graph");
    assert_eq!(
        route(&h),
        Route::Graph {
            focus: Some("page-1".into())
        }
    );
    h.click("m-back");
    assert_eq!(route(&h), Route::Object("page-1".into()));
    // Back goes to the object's space home, not history.
    h.click("m-back");
    assert_eq!(route(&h), Route::Home);
    assert!(h.has("m-space-home"));
    // And on to the Spaces list.
    h.click("m-spaces");
    assert!(h.has("m-spaces"));

    // A chat opens full screen, with its own header: no shell header over it.
    h.update(|s| navigate(Route::Chat("chat-1".into()), s));
    assert!(h.has("test-page") && !h.has("m-back") && !h.has("m-spaces"));
    h.update(|s| s.nav.back(&mut s.ui));
    assert_eq!(route(&h), Route::Home);

    // iPhone landscape is wider than 720pt: the desktop layout.
    h.resize(874., 402.);
    assert!(h.has("desk") && !h.has("m-shell"));
    let mutations = vault.mutations.lock().clone();
    assert!(
        mutations.iter().any(|m| m == "set_field"),
        "the moved space's position was never saved: {mutations:?}"
    );
}

#[test]
fn entering_a_space_shows_its_home_with_sections_that_fold() {
    let vault = Arc::new(Vault::default());
    let mut h = Harness::with_pages(Arc::new(Shared(vault)), (390., 844.), pane_pages());
    h.update(|s| navigate(Route::Home, s));
    h.click("m-card-sp-1");
    // Recently edited lists the space's objects, newest first.
    assert_eq!(h.keys("m-recent-"), ["m-recent-chat-1", "m-recent-page-1"]);
    h.click("m-section-chev-__recent");
    assert!(h.keys("m-recent-").is_empty(), "folded");
    h.click("m-section-label-__recent");
    assert_eq!(h.keys("m-recent-").len(), 2, "unfolded");
    // A row opens its object.
    h.click("m-recent-page-1");
    assert_eq!(route(&h), Route::Object("page-1".into()));
    // Switching space on a phone lands on the new space's home.
    h.update(|s| s.nav.use_space("sp-2", &mut s.ui));
    assert_eq!(route(&h), Route::Home);
}

#[test]
fn the_search_sheet_pulls_away_by_touch() {
    let vault = Arc::new(Vault::default());
    let mut h = Harness::with_pages(Arc::new(Shared(vault)), (390., 844.), pane_pages());
    h.update(|s| navigate(Route::Home, s));
    h.click("m-card-sp-1");
    // Search is a bottom sheet below a 56px strip; a short pull snaps
    // back, a pull past 110px closes it.
    h.click("m-search");
    let sheet = h.rect("search-modal").expect("the search sheet");
    assert_eq!((sheet[1], sheet[1] + sheet[3]), (56., 844.));
    let inside = [sheet[0] + sheet[2] / 2., sheet[1] + sheet[3] / 2.];
    h.rt.touch(1, TouchPhase::Started, inside[0], inside[1], 0.);
    h.rt.touch(1, TouchPhase::Moved, inside[0], inside[1] + 20., 0.016);
    h.frame();
    h.rt.touch(1, TouchPhase::Moved, inside[0], inside[1] + 40., 0.032);
    h.frames(30);
    assert_eq!(
        h.rect("search-modal").unwrap()[1],
        96.,
        "it follows the pull"
    );
    h.rt.touch(1, TouchPhase::Ended, inside[0], inside[1] + 40., 0.3);
    h.settle();
    assert!(h.has("search-modal"), "a short pull snaps back");
    pull(&mut h, inside, &[20., 100.]);
    assert!(!h.has("search-modal"), "a long pull closes it");
}

#[test]
fn the_open_tabs_list_shows_opens_and_closes_the_spaces_tabs() {
    let vault = Arc::new(Vault::default());
    let mut h = Harness::with_pages(Arc::new(Shared(vault.clone())), (390., 844.), pane_pages());
    h.update(|s| navigate(Route::Home, s));
    h.click("m-card-sp-1");
    // Nothing open yet.
    h.click("m-tabs");
    assert!(h.has("m-open-tabs") && h.has("m-tabs-empty"));
    h.click("m-tabs-back");
    assert!(h.has("m-space-home") && !h.has("m-open-tabs"));

    // Moving around on a phone leaves the tabs alone.
    h.click("m-recent-page-1");
    assert_eq!(route(&h), Route::Object("page-1".into()));
    let paths = |h: &Harness| -> Vec<String> {
        h.state()
            .nav
            .tabs()
            .iter()
            .map(|t| t.route.path())
            .collect()
    };
    assert_eq!(paths(&h), ["/app"]);
    h.click("m-back");
    assert!(h.has("m-space-home"));

    // "Open in new tab" adds tabs; the list shows them in order.
    h.update(|s| {
        open_tab(Route::Object("page-1".into()), true, s);
        open_tab(Route::Object("chat-1".into()), true, s);
    });
    assert_eq!(route(&h), Route::Home, "opened in the background");
    let uids: Vec<u64> = h.state().nav.tabs()[1..].iter().map(|t| t.uid).collect();
    h.click("m-tabs");
    let (page, chat) = (format!("m-tab-{}", uids[0]), format!("m-tab-{}", uids[1]));
    assert!(h.has(&page) && h.has(&chat) && !h.has("m-tabs-empty"));
    assert!(center(&h, &page)[1] < center(&h, &chat)[1]);
    assert!(h.texts().iter().any(|t| t == "Notes") && h.texts().iter().any(|t| t == "Team chat"));

    // × closes a tab; the list stays.
    h.click(&format!("m-tab-close-{}", uids[1]));
    assert!(h.has("m-open-tabs") && h.has(&page) && !h.has(&chat));
    assert_eq!(paths(&h), ["/app", "/app/object/page-1"]);
    assert_eq!(route(&h), Route::Home);

    // A tap opens the tab.
    h.click(&page);
    assert_eq!(route(&h), Route::Object("page-1".into()));
    assert!(h.state().nav.in_tab() && !h.has("m-open-tabs"));
    assert_eq!(h.state().nav.active_index(), 1);

    // The list is the shared one: written after the debounce.
    let writes = |v: &Vault| {
        v.mutations
            .lock()
            .iter()
            .filter(|m| *m == "set_field")
            .count()
    };
    let before = writes(&vault);
    h.advance(TABS_WRITE_DELAY);
    assert_eq!(writes(&vault), before + 1);
    assert_eq!(
        h.state().nav.open_tabs(),
        [("sp-1".to_string(), paths(&h))].into()
    );
}

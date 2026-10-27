//! Settings' section navigation in a GPUI test window: on desktop a side
//! panel lists the shown sections, a click glides the column to its section
//! and the highlighted entry follows the scrolling; at phone width the same
//! entries are a chip row pinned above the sheet's column instead.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, TouchPhase,
    VisualTestContext,
};
use gpui_kit::{Bounds, Pixels, point, px, size};
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space};
use roostr_core::{Backend, BackendEvent, JsonMap};
use serde_json::Value as Json;

/// One section's slug and the test selectors of its card and entries.
struct Section {
    slug: &'static str,
    card: &'static str,
    nav: &'static str,
    chip: &'static str,
}

macro_rules! section {
    ($slug:literal) => {
        Section {
            slug: $slug,
            card: concat!("settings-section-", $slug),
            nav: concat!("settings-nav-", $slug),
            chip: concat!("settings-chip-", $slug),
        }
    };
}

/// The sections Settings shows on a desktop (every one: the vault section
/// is hidden only in the iOS app), in order.
const SECTIONS: [Section; 10] = [
    section!("profile"),
    section!("chat-color"),
    section!("nostr-identity"),
    section!("relays"),
    section!("where-your-vault-lives"),
    section!("desktop-sync-check"),
    section!("dictionary"),
    section!("export"),
    section!("claude-plan"),
    section!("agent"),
];

fn find(slug: &str) -> &'static Section {
    SECTIONS
        .iter()
        .find(|s| s.slug == slug)
        .unwrap_or_else(|| panic!("no section {slug}"))
}
/// How far below the top of the column a picked section comes to rest.
const LANDING: Pixels = px(8.);
/// The scroll-spy line below the top of the column.
const SPY_LINE: Pixels = px(48.);

/// A vault with a key and nothing in it.
struct Vault;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "empty".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(Vec::new()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        ready(Err(anyhow::anyhow!("unknown object {id}")))
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, _: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        ready(Ok(QueryResult::default()))
    }
    fn mutate(&self, _: &str, _: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
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

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
}

fn shown(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

/// Let every running motion finish.
fn settle(cx: &mut VisualTestContext) {
    advance(cx, Duration::from_secs(1));
}

fn advance(cx: &mut VisualTestContext, by: Duration) {
    cx.executor().advance_clock(by);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn tap(cx: &mut VisualTestContext, selector: &'static str) {
    let at = bounds(cx, selector).center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
}

/// A wheel (or touch pan) of `dy` over the settings column; negative
/// scrolls further down.
fn wheel(cx: &mut VisualTestContext, dy: f32) {
    let at = bounds(cx, "settings-scroll").center();
    for (phase, dy) in [
        (TouchPhase::Started, 0.),
        (TouchPhase::Moved, dy),
        (TouchPhase::Ended, 0.),
    ] {
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            modifiers: Modifiers::none(),
            touch_phase: phase,
            source: ScrollSource::Pointer,
        });
        cx.run_until_parked();
    }
}

/// The section whose side-panel entry is highlighted.
fn active_nav(cx: &mut VisualTestContext) -> &'static str {
    let marker = bounds(cx, "settings-nav-active").center();
    SECTIONS
        .iter()
        .find(|s| bounds(cx, s.nav).contains(&marker))
        .expect("no panel entry is highlighted")
        .slug
}

/// The section whose chip is highlighted.
fn active_chip(cx: &mut VisualTestContext) -> &'static str {
    let marker = bounds(cx, "settings-chip-active").center();
    SECTIONS
        .iter()
        .find(|s| bounds(cx, s.chip).contains(&marker))
        .expect("no chip is highlighted")
        .slug
}

/// How far below the top of the column section `slug`'s card starts. (The
/// dialog's own entrance runs on the wall clock, so the column itself may
/// still be moving: positions are measured from it, in one frame.)
fn below_top(cx: &mut VisualTestContext, slug: &str) -> Pixels {
    bounds(cx, find(slug).card).top() - bounds(cx, "settings-scroll").top()
}

/// The section the column shows at its top: the last whose card starts
/// above the spy line.
fn in_view(cx: &mut VisualTestContext) -> &'static str {
    let line = bounds(cx, "settings-scroll").top() + SPY_LINE;
    SECTIONS
        .iter()
        .take_while(|s| bounds(cx, s.card).top() <= line)
        .last()
        .unwrap_or(&SECTIONS[0])
        .slug
}

#[gpui::test]
fn the_settings_sections_have_a_navigation(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(Arc::new(Vault), None, cx);
        roostr_app::init(cx);
    });
    let (_root, cx) = cx.add_window_view(|window, cx| {
        let workspace = roostr_app::workspace(window, cx);
        gpui_kit::base::Root::new(workspace, window, cx)
    });
    cx.simulate_resize(size(px(1280.), px(800.)));
    cx.run_until_parked();
    cx.update(|window, cx| roostr_settings::open_settings(window, cx));
    cx.run_until_parked();
    settle(cx);

    // Desktop: the side panel, not the chips, lists every shown section in
    // order, left of the column.
    assert!(shown(cx, "settings-nav") && !shown(cx, "settings-chips"));
    let panel = bounds(cx, "settings-nav");
    let column = bounds(cx, "settings-scroll");
    assert!(
        panel.right() <= column.left(),
        "the panel sits left of the column"
    );
    let entries: Vec<_> = SECTIONS.iter().map(|s| bounds(cx, s.nav).top()).collect();
    let cards: Vec<_> = SECTIONS.iter().map(|s| bounds(cx, s.card).top()).collect();
    assert!(
        entries.is_sorted() && cards.is_sorted(),
        "entries and sections share one order"
    );
    assert_eq!(active_nav(cx), "profile");

    // A click glides the column until the section rests at its top.
    let from = below_top(cx, "nostr-identity");
    tap(cx, "settings-nav-nostr-identity");
    advance(cx, Duration::from_millis(120));
    let midway = below_top(cx, "nostr-identity");
    assert!(
        LANDING < midway && midway < from,
        "the column glides: {from:?} → {midway:?} → {LANDING:?}"
    );
    settle(cx);
    let landed = below_top(cx, "nostr-identity");
    assert!(
        (landed - LANDING).abs() < px(1.),
        "landed {landed:?} below the top"
    );
    assert_eq!(active_nav(cx), "nostr-identity");

    // Scrolling moves the highlight with it.
    wheel(cx, 20_000.);
    assert_eq!(active_nav(cx), "profile", "back at the top");
    wheel(cx, -900.);
    let shown_section = in_view(cx);
    assert_ne!(shown_section, "profile");
    assert_eq!(active_nav(cx), shown_section);
    wheel(cx, -20_000.);
    assert_eq!(active_nav(cx), "agent", "the end of the column");

    // Phone: the chips pinned above the sheet's column instead.
    cx.simulate_resize(size(px(390.), px(844.)));
    cx.run_until_parked();
    settle(cx);
    assert!(shown(cx, "settings-chips") && !shown(cx, "settings-nav"));
    let row = bounds(cx, "settings-chips");
    let column = bounds(cx, "settings-scroll");
    assert!(
        row.bottom() <= column.top(),
        "the chips sit above the column"
    );
    let chips: Vec<_> = SECTIONS.iter().map(|s| bounds(cx, s.chip).left()).collect();
    assert!(chips.is_sorted(), "the chips keep the sections' order");
    assert_eq!(active_chip(cx), in_view(cx));
    // Scrolled to the end, the last chip scrolls itself into the row.
    wheel(cx, -20_000.);
    assert_eq!(active_chip(cx), "agent");
    let agent = bounds(cx, "settings-chip-agent");
    assert!(
        row.left() <= agent.left() && agent.right() <= row.right(),
        "the active chip is in view: {agent:?} in {row:?}"
    );

    wheel(cx, 20_000.);
    settle(cx);
    assert_eq!(active_chip(cx), "profile");
    let profile = bounds(cx, "settings-chip-profile");
    assert!(
        row.left() <= profile.left(),
        "the row followed back to the start"
    );
    tap(cx, "settings-chip-relays");
    settle(cx);
    let landed = below_top(cx, "relays");
    assert!(
        (landed - LANDING).abs() < px(1.),
        "landed {landed:?} below the top"
    );
    assert_eq!(active_chip(cx), "relays");
}

//! Settings' section navigation in a headless Rune window: on desktop a side
//! panel lists the shown sections, a click glides the column to its section
//! and the highlighted entry follows the scrolling; at phone width the same
//! entries are a chip row pinned above the sheet's column instead.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space};
use roostr_core::{Backend, BackendEvent, JsonMap, State};
use roostr_rt::{BoxFuture, BoxStream};
use rune::{Cx, El, Env, Runtime, Tx, col};
use serde_json::Value as Json;

/// The sections Settings shows on a desktop (every one: the vault section
/// is hidden only in the iOS app), in order.
const SECTIONS: [&str; 11] = [
    "profile",
    "chat-color",
    "appearance",
    "nostr-identity",
    "relays",
    "where-your-vault-lives",
    "desktop-sync-check",
    "dictionary",
    "export",
    "claude-plan",
    "agent",
];

/// How far below the top of the column a picked section comes to rest.
const LANDING: f32 = 8.0;
/// The scroll-spy line below the top of the column.
const SPY_LINE: f32 = 48.0;
const FRAME: f64 = 1.0 / 60.0;

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

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(roostr_settings::theme_pref(s), cx);
    let mut layer = Vec::new();
    roostr_settings::settings_modal(s, cx, t, &mut layer);
    roostr_ui::root(col().w_full().h_full().fill(t.bg), layer)
}

fn poll(s: &mut State, _: f64, _: &mut Tx) -> bool {
    s.poll().updated
}

/// Frames until the state's futures have all landed (background work runs
/// on worker threads; each frame's poll picks up what finished).
fn drain(rt: &mut Runtime<State>) {
    for _ in 0..300 {
        rt.frame(0.0);
        std::thread::yield_now();
    }
    rt.frame(FRAME);
}

fn advance(rt: &mut Runtime<State>, by: Duration) {
    let frames = (by.as_secs_f64() / FRAME).ceil() as usize;
    for _ in 0..frames {
        rt.state.rt.advance(Duration::from_secs_f64(FRAME));
        rt.frame(FRAME);
    }
}

/// Let every running motion finish.
fn settle(rt: &mut Runtime<State>) {
    advance(rt, Duration::from_secs(1));
}

/// Where `key` is drawn now, unless it is gone (or leaving).
fn rect(rt: &Runtime<State>, key: &str) -> Option<[f32; 4]> {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
        .map(|e| e.rect)
}

fn bounds(rt: &Runtime<State>, key: &str) -> [f32; 4] {
    rect(rt, key).unwrap_or_else(|| panic!("{key} is not on screen"))
}

fn shown(rt: &Runtime<State>, key: &str) -> bool {
    rect(rt, key).is_some()
}

fn contains(b: [f32; 4], (x, y): (f32, f32)) -> bool {
    x >= b[0] && x <= b[0] + b[2] && y >= b[1] && y <= b[1] + b[3]
}

fn center(b: [f32; 4]) -> (f32, f32) {
    (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0)
}

fn tap(rt: &mut Runtime<State>, key: &str) {
    let (x, y) = center(bounds(rt, key));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(FRAME);
}

/// A wheel of `dy` over the settings column; negative scrolls further down.
fn wheel(rt: &mut Runtime<State>, dy: f32) {
    let (x, y) = center(bounds(rt, "settings-scroll"));
    rt.wheel(x, y, dy);
    rt.frame(FRAME);
    rt.frame(FRAME);
}

/// The section whose entry (`prefix` "settings-nav" or "settings-chip") is highlighted.
fn active(rt: &Runtime<State>, prefix: &str) -> &'static str {
    let marker = center(bounds(rt, &format!("{prefix}-active")));
    SECTIONS
        .iter()
        .find(|s| contains(bounds(rt, &format!("{prefix}-{s}")), marker))
        .unwrap_or_else(|| panic!("no {prefix} entry is highlighted"))
}

fn card_top(rt: &Runtime<State>, slug: &str) -> f32 {
    bounds(rt, &format!("settings-section-{slug}"))[1]
}

/// How far below the top of the column section `slug`'s card starts.
fn below_top(rt: &Runtime<State>, slug: &str) -> f32 {
    card_top(rt, slug) - bounds(rt, "settings-scroll")[1]
}

/// The section the column shows at its top: the last whose card starts
/// above the spy line.
fn in_view(rt: &Runtime<State>) -> &'static str {
    let line = bounds(rt, "settings-scroll")[1] + SPY_LINE;
    SECTIONS
        .iter()
        .take_while(|s| card_top(rt, s) <= line)
        .last()
        .unwrap_or(&SECTIONS[0])
}

#[test]
fn the_settings_sections_have_a_navigation() {
    let ui_state =
        std::env::temp_dir().join(format!("roostr-settings-nav-{}.json", std::process::id()));
    // SAFETY: this test binary's only test, before the state loads.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", &ui_state) };
    let mut s = State::new(Arc::new(Vault), None);
    roostr_settings::register(&mut s);
    roostr_settings::open_settings(&mut s);
    let env = Env {
        system_dark: true,
        scale: 2.0,
    };
    let mut rt = Runtime::new(s, view, (1280.0, 800.0), env);
    rt.poll = Some(poll);
    drain(&mut rt);
    settle(&mut rt);

    // Desktop: the side panel, not the chips, lists every shown section in
    // order, left of the column.
    assert!(shown(&rt, "settings-nav") && !shown(&rt, "settings-chips"));
    let panel = bounds(&rt, "settings-nav");
    let column = bounds(&rt, "settings-scroll");
    assert!(
        panel[0] + panel[2] <= column[0] + 0.5,
        "the panel sits left of the column"
    );
    let entries: Vec<f32> = SECTIONS
        .iter()
        .map(|s| bounds(&rt, &format!("settings-nav-{s}"))[1])
        .collect();
    let cards: Vec<f32> = SECTIONS.iter().map(|s| card_top(&rt, s)).collect();
    assert!(
        entries.is_sorted() && cards.is_sorted(),
        "entries and sections share one order"
    );
    assert_eq!(active(&rt, "settings-nav"), "profile");

    // A click glides the column until the section rests at its top.
    let from = below_top(&rt, "nostr-identity");
    tap(&mut rt, "settings-nav-nostr-identity");
    advance(&mut rt, Duration::from_millis(120));
    let midway = below_top(&rt, "nostr-identity");
    assert!(
        LANDING < midway && midway < from,
        "the column glides: {from} → {midway} → {LANDING}"
    );
    settle(&mut rt);
    let landed = below_top(&rt, "nostr-identity");
    assert!(
        (landed - LANDING).abs() < 1.0,
        "landed {landed} below the top"
    );
    assert_eq!(active(&rt, "settings-nav"), "nostr-identity");

    // Scrolling moves the highlight with it.
    wheel(&mut rt, 20_000.0);
    settle(&mut rt);
    assert_eq!(active(&rt, "settings-nav"), "profile", "back at the top");
    wheel(&mut rt, -900.0);
    let shown_section = in_view(&rt);
    assert_ne!(shown_section, "profile");
    assert_eq!(active(&rt, "settings-nav"), shown_section);
    wheel(&mut rt, -20_000.0);
    assert_eq!(
        active(&rt, "settings-nav"),
        "agent",
        "the end of the column"
    );

    // Phone: the chips pinned above the sheet's column instead.
    rt.set_viewport((390.0, 844.0), 2.0);
    rt.frame(FRAME);
    settle(&mut rt);
    assert!(shown(&rt, "settings-chips") && !shown(&rt, "settings-nav"));
    let chip_row = bounds(&rt, "settings-chips");
    let column = bounds(&rt, "settings-scroll");
    assert!(
        chip_row[1] + chip_row[3] <= column[1] + 0.5,
        "the chips sit above the column"
    );
    let chips: Vec<f32> = SECTIONS
        .iter()
        .map(|s| bounds(&rt, &format!("settings-chip-{s}"))[0])
        .collect();
    assert!(chips.is_sorted(), "the chips keep the sections' order");
    assert_eq!(active(&rt, "settings-chip"), in_view(&rt));
    // Scrolled to the end, the last chip scrolls itself into the row.
    wheel(&mut rt, -20_000.0);
    settle(&mut rt);
    assert_eq!(active(&rt, "settings-chip"), "agent");
    let agent = bounds(&rt, "settings-chip-agent");
    assert!(
        chip_row[0] <= agent[0] + 0.5 && agent[0] + agent[2] <= chip_row[0] + chip_row[2] + 0.5,
        "the active chip is in view: {agent:?} in {chip_row:?}"
    );

    wheel(&mut rt, 20_000.0);
    settle(&mut rt);
    assert_eq!(active(&rt, "settings-chip"), "profile");
    let profile = bounds(&rt, "settings-chip-profile");
    assert!(
        chip_row[0] <= profile[0] + 0.5,
        "the row followed back to the start"
    );
    // A chip in view (the row is narrower than its chips) glides the column too.
    tap(&mut rt, "settings-chip-appearance");
    settle(&mut rt);
    let landed = below_top(&rt, "appearance");
    assert!(
        (landed - LANDING).abs() < 1.0,
        "landed {landed} below the top"
    );
    assert_eq!(active(&rt, "settings-chip"), "appearance");
    let _ = std::fs::remove_file(ui_state);
}

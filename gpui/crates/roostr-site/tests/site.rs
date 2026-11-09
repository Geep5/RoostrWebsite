//! The site pages headless: links navigate, the guide's table of contents and the keyboard
//! scroll the page, and the join page offers what this app can do.

use std::sync::Once;

use roostr_core::State;
use roostr_core::nav::{self, Route, SitePage};
use rune::{Cx, El, ElementInfo, Env, Key, KeyEvent, Mods, Runtime, el};

fn view(s: &State, cx: &mut Cx<State>) -> El {
    match s.nav.route() {
        Route::Site(page) => roostr_site::view(s, cx, page),
        _ => el(),
    }
}

/// A state with no vault, its UI prefs in a scratch file, showing `page`.
fn runtime(page: SitePage, size: (f32, f32)) -> Runtime<State> {
    static SCRATCH: Once = Once::new();
    SCRATCH.call_once(|| {
        let path =
            std::env::temp_dir().join(format!("roostr-site-test-{}.json", std::process::id()));
        // SAFETY: set once, before any test thread reads the environment through `UiState`.
        unsafe { std::env::set_var("ROOSTR_UI_STATE", path) };
    });
    let mut s = State::unopened();
    roostr_site::register(&mut s);
    nav::navigate(Route::Site(page), &mut s);
    let mut rt = Runtime::new(s, view, size, Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn find(rt: &Runtime<State>, f: impl Fn(&ElementInfo) -> bool) -> ElementInfo {
    rt.elements().into_iter().find(f).expect("element")
}

fn by_text(rt: &Runtime<State>, text: &str) -> ElementInfo {
    find(rt, |e| e.text.as_deref() == Some(text))
}

fn by_key(rt: &Runtime<State>, key: &str) -> ElementInfo {
    find(rt, |e| e.key.as_deref() == Some(key))
}

fn press(rt: &mut Runtime<State>, e: &ElementInfo) {
    let [x, y, w, h] = e.rect;
    let (x, y) = (x + w.min(20.) / 2., y + h / 2.);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn key(rt: &mut Runtime<State>, key: Key) {
    rt.key(KeyEvent {
        key,
        mods: Mods::default(),
    });
    settle(rt);
}

fn settle(rt: &mut Runtime<State>) {
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
}

fn page(rt: &Runtime<State>) -> Route {
    rt.state.nav.route().clone()
}

#[test]
fn landing_links_go_to_the_guide_and_the_app() {
    let mut rt = runtime(SitePage::Landing, (1440., 864.));
    assert_eq!(
        by_text(&rt, "YOUR TEAM,").text.as_deref(),
        Some("YOUR TEAM,")
    );
    let guide = by_text(&rt, "Guide");
    press(&mut rt, &guide);
    assert_eq!(page(&rt), Route::Site(SitePage::Guide));

    let mut rt = runtime(SitePage::Landing, (1440., 864.));
    let open = by_key(&rt, "btn-open");
    press(&mut rt, &open);
    assert_eq!(page(&rt), Route::Home);
}

#[test]
fn a_phone_landing_sets_the_headline_on_two_lines() {
    let rt = runtime(SitePage::Landing, (390., 844.));
    by_text(&rt, "HUMAN AND NOT.");
    assert!(
        rt.elements()
            .iter()
            .all(|e| e.text.as_deref() != Some("HUMAN"))
    );
    // Too narrow for the GitHub label: the mark alone.
    assert!(
        rt.elements()
            .iter()
            .all(|e| e.text.as_deref() != Some("GitHub"))
    );
}

#[test]
fn the_table_of_contents_jumps_to_its_heading() {
    let mut rt = runtime(SitePage::Guide, (1000., 700.));
    let before = by_key(&rt, "site-anchor-agents").rect[1];
    assert!(before > 700., "the heading starts below the fold: {before}");
    let entry = by_text(&rt, "Agents");
    press(&mut rt, &entry);
    let after = by_key(&rt, "site-anchor-agents").rect[1];
    assert!((after - 16.).abs() < 1., "scroll-margin-top 16: {after}");
}

#[test]
fn the_keyboard_scrolls_a_document_page() {
    let mut rt = runtime(SitePage::Privacy, (1000., 700.));
    let start = by_text_top(&rt);
    // A click on the page focuses it, as in a browser.
    rt.pointer_down(950., 400.);
    rt.pointer_up(950., 400.);
    settle(&mut rt);
    key(&mut rt, Key::Down);
    assert!((start - by_text_top(&rt) - 40.).abs() < 0.5);
    key(&mut rt, Key::PageDown);
    assert!((start - by_text_top(&rt) - 40. - 700. * 0.875).abs() < 0.5);
    key(&mut rt, Key::Home);
    assert!((by_text_top(&rt) - start).abs() < 0.5);
    key(&mut rt, Key::End);
    let foot = find(&rt, |e| {
        e.text
            .as_deref()
            .is_some_and(|t| t.starts_with("If this policy changes"))
    });
    assert!(foot.rect[1] < 700., "the end is in view: {:?}", foot.rect);
}

#[test]
fn a_window_routes_the_scroll_keys_with_nothing_focused() {
    let mut rt = runtime(SitePage::Guide, (1000., 700.));
    let start = by_text(&rt, "How Roostr works").rect[1];
    rt.update(|s, tx| {
        let ev = KeyEvent {
            key: Key::Down,
            mods: Mods::default(),
        };
        assert!(roostr_site::on_key(s, &ev, tx));
    });
    settle(&mut rt);
    assert!((start - by_text(&rt, "How Roostr works").rect[1] - 40.).abs() < 0.5);
}

fn by_text_top(rt: &Runtime<State>) -> f32 {
    by_text(rt, "Privacy policy").rect[1]
}

#[test]
fn a_join_page_without_a_key_sends_to_the_app() {
    let link = roostr_core::invite::SpaceJoinLink {
        space: "space-1".into(),
        name: Some("Farwell Castle".into()),
        owner: "npub1owner".into(),
        relays: vec!["wss://relay.example".into()],
    };
    let blob = roostr_core::invite::encode_join_link(&link);
    let mut rt = runtime(SitePage::Join(blob), (1000., 700.));
    by_text(&rt, "Request to join Farwell Castle");
    by_text(&rt, "via wss://relay.example");
    by_text(
        &rt,
        "Set up your key in the app first, then reopen this link.",
    );
    let open = by_key(&rt, "open-app");
    press(&mut rt, &open);
    assert_eq!(page(&rt), Route::Home);
}

#[test]
fn a_broken_join_link_says_so() {
    let rt = runtime(SitePage::Join("r2.nope".into()), (1000., 700.));
    by_text(&rt, "Invalid link");
    by_text(&rt, "This join link is invalid or truncated.");
}

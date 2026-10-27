//! The object's chat pane in a desktop-width window: its messages scroll, and
//! their text selects and copies. Field report (every platform): the chat
//! would neither scroll nor select, and touching it froze the whole window -
//! the message list's scroll handler read the list while the list held its
//! own state borrowed, so the first wheel or pan panicked.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Modifiers, MouseButton, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, TouchPhase,
    VisualTestContext,
};
use gpui_kit::base::TextSelection;
use gpui_kit::{AppContext as _, Pixels, Point, point, px, size};
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value};
use roostr_core::{Backend, BackendEvent, JsonMap, PaneState, PaneTab};
use roostr_shell::Workspace;
use serde_json::{Value as Json, json};

const MESSAGES: usize = 40;

/// One space holding a page whose discussion has `MESSAGES` messages.
struct Vault;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn text(n: usize) -> String {
    format!("Message {n} of the launch thread")
}

fn page() -> Object {
    let messages: Vec<Json> = (1..=MESSAGES)
        .map(|n| {
            json!({
                "id": format!("m{n}"),
                "childrenIds": [],
                "content": { "custom": { "contentType": "chat", "meta": {
                    "author": "peer", "ts": (1_000 + n).to_string(), "text": text(n),
                } } },
            })
        })
        .collect();
    let ids: Vec<&Json> = messages.iter().map(|m| &m["id"]).collect();
    let mut blocks = vec![json!({ "id": "__discussion__", "childrenIds": ids, "content": {} })];
    blocks.extend(messages.iter().cloned());
    let mut page: Object = serde_json::from_value(json!({
        "id": "page-1",
        "typeKey": "page",
        "fields": {},
        "blocks": blocks,
    }))
    .expect("page fixture");
    page.fields = [
        ("name".to_string(), Value::string("Launch plan")),
        ("channel".to_string(), Value::string("sp-1")),
    ]
    .into();
    page
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "a page with a long chat".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(vec![ObjectSummary {
            id: "page-1".into(),
            type_key: "page".into(),
            name: "Launch plan".into(),
            channel_id: "sp-1".into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        }]))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        ready(if id == "page-1" {
            Ok(page())
        } else {
            Err(anyhow::anyhow!("unknown object {id}"))
        })
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

fn wheel(cx: &mut VisualTestContext, at: Point<Pixels>, phase: TouchPhase, dy: f32) {
    cx.simulate_event(ScrollEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Modifiers::none(),
        touch_phase: phase,
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
}

/// Drags from the first message text at or below `at` to the right edge of
/// the line below it, and copies: that message's text. A probe that lands on
/// a wrapped message's last line moves on to the next message.
fn copy_message_at(cx: &mut VisualTestContext, at: Point<Pixels>, right: Pixels) -> String {
    let mut top = at.y;
    for _ in 0..4 {
        let start = (0..40)
            .map(|step| point(at.x, top + px(step as f32 * 4.)))
            .find(|probe| {
                cx.simulate_mouse_move(*probe, None, Modifiers::none());
                cx.update(|window, cx| TextSelection::is_selectable_at(*probe, window, cx))
            })
            .expect("message text below the probe");
        let end = point(right, start.y + px(20.));
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-c");
        cx.run_until_parked();
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        if copied.contains("Message") {
            return copied;
        }
        top = start.y + px(12.);
    }
    String::new()
}

/// The number of the message `copied` came from.
fn message_number(copied: &str) -> usize {
    copied
        .split(|c: char| !c.is_ascii_digit())
        .find(|n| !n.is_empty())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no message number in {copied:?}"))
}

#[gpui::test]
fn the_chat_pane_scrolls_and_its_text_selects(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(Arc::new(Vault), None, cx);
        roostr_app::init(cx);
    });
    // The app's windows are a gpui-kit Root (`open_window`), which carries
    // the window text selection.
    let (_root, cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| Workspace::new(roostr_app::pages(), window, cx));
        gpui_kit::base::Root::new(workspace, window, cx)
    });
    cx.simulate_resize(size(px(1280.), px(800.)));
    cx.update(|_, cx| {
        PaneState::global(cx).update(cx, |pane, cx| pane.set_open(PaneTab::Chat, true, cx));
        navigate(Route::Object("page-1".into()), cx);
    });
    cx.run_until_parked();
    // The opened pane's column slides in (`grid-template-columns 0.16s`).
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();

    let list = cx
        .debug_bounds("chat-messages")
        .expect("the chat pane's messages");
    // Low in the pane, on the left where a peer's bubbles start.
    let probe = point(list.left() + px(51.), list.top() + list.size.height * 0.7);
    let right = list.right() - px(2.);
    let before = copy_message_at(cx, probe, right);
    assert!(
        before.contains("of the launch"),
        "a drag over a message selects its text, which ⌘C copies: {before:?}"
    );
    assert!(
        message_number(&before) > MESSAGES - 8,
        "the chat opens at its newest messages: {before:?}"
    );

    // A wheel (or a touch pan, which arrives as one) scrolls back through
    // the thread.
    wheel(cx, probe, TouchPhase::Started, 0.);
    wheel(cx, probe, TouchPhase::Moved, 600.);
    wheel(cx, probe, TouchPhase::Ended, 0.);
    let after = copy_message_at(cx, probe, right);
    assert!(
        message_number(&after) < message_number(&before),
        "the pane scrolled to older messages: {before:?} then {after:?}"
    );
}

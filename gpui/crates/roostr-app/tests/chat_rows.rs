//! The chat's messages never draw on top of each other: each row sits below
//! the one before it, whatever the row holds (a long wrapped message, a
//! reply, your own message on the right) - when the chat opens,
//! after scrolling back, and after the pane narrows so the text re-wraps.
//! Field report: with the message list on gpui-kit's MessageScroller, some
//! bubbles were drawn over others.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext,
};
use gpui_kit::{AppContext as _, Pixels, Point, point, px, size};
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value};
use roostr_core::{Backend, BackendEvent, JsonMap, PaneState, PaneTab};
use roostr_shell::Workspace;
use serde_json::{Value as Json, json};

const MESSAGES: usize = 30;
const ME: &str = "me";

struct Vault;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

/// Message `n`: every few is long enough to wrap over several lines, every
/// third is yours.
fn message(n: usize) -> Json {
    let long = n % 3 == 1;
    let text = if n % 7 == 2 {
        // An agent-style reply: heading, list, bold, inline code, a code block and a table.
        format!(
            "Message {n}\n\n## Status\n\n- **Done:** the importer\n- **Next:** `check_inbox` retries\n- A third point that is long enough to wrap onto a second line in the pane\n\n```\nlet x = 1;\nlet y = 2;\n```\n\n|Inbox|Checked|\n|---|---|\n|grant@|7:02 AM|\n|support@|7:02 AM|"
        )
    } else if n == MESSAGES - 2 {
        // An agent's report taller than the whole chat, with bold and code spans, then short replies after it.
        format!(
            "Message {n}\n\n{}",
            "**What was wrong:** when a history loads, it first updates any old saved fights to the current `format`. In that account's history, one saved fight was missing its list of players.\n\n".repeat(12)
        )
    } else if long {
        format!(
            "Message {n}: {}",
            "a long update that wraps over several lines in the chat pane, ".repeat(4)
        )
    } else {
        format!("Message {n}")
    };
    let author = if n % 3 == 0 { ME } else { "peer" };
    let meta = json!({ "author": author, "ts": (1_000 + n).to_string(), "text": text });
    json!({
        "id": format!("m{n}"),
        "childrenIds": [],
        "content": { "custom": { "contentType": "chat", "meta": meta } },
    })
}

fn page() -> Object {
    let messages: Vec<Json> = (1..=MESSAGES).map(message).collect();
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
        "a page with a varied chat".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            author_id: ME.into(),
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

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn wheel(cx: &mut VisualTestContext, at: Point<Pixels>, dy: f32) {
    for (phase, dy) in [
        (TouchPhase::Started, 0.),
        (TouchPhase::Moved, dy),
        (TouchPhase::Ended, 0.),
    ] {
        cx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            modifiers: Modifiers::none(),
            touch_phase: phase,
        });
        cx.run_until_parked();
    }
}

/// The rows' and their texts' debug selectors (`msg-m1`, `msg-text-m1`…) - `debug_bounds` takes a `'static` name.
static ROW_NAMES: std::sync::LazyLock<Vec<(String, String)>> = std::sync::LazyLock::new(|| {
    (1..=MESSAGES)
        .map(|n| (format!("msg-m{n}"), format!("msg-text-m{n}")))
        .collect()
});

/// Every message row drawn now sits entirely below the one before it, and
/// its text stays inside it (a row measured shorter than its wrapped text
/// lets the text run over the next message).
fn assert_no_overlap(cx: &mut VisualTestContext, when: &str) {
    let rows: Vec<(usize, gpui::Bounds<Pixels>)> = ROW_NAMES
        .iter()
        .enumerate()
        .filter_map(|(i, (row, text))| {
            let row_bounds = cx.debug_bounds(row.as_str())?;
            if let Some(text_bounds) = cx.debug_bounds(text.as_str()) {
                assert!(
                    text_bounds.bottom() <= row_bounds.bottom() + px(0.5),
                    "{when}: message {}'s text (bottom {:?}) runs out of its row (bottom {:?})",
                    i + 1,
                    text_bounds.bottom(),
                    row_bounds.bottom()
                );
                // Nor out of the sides: a bubble wider than the chat cuts its text off.
                assert!(
                    text_bounds.left() >= row_bounds.left() - px(0.5)
                        && text_bounds.right() <= row_bounds.right() + px(0.5),
                    "{when}: message {}'s text ({:?}..{:?}) runs out of the side of its row ({:?}..{:?})",
                    i + 1,
                    text_bounds.left(),
                    text_bounds.right(),
                    row_bounds.left(),
                    row_bounds.right()
                );
            }
            Some((i + 1, row_bounds))
        })
        .collect();
    // A tall report can fill the whole view on its own.
    assert!(!rows.is_empty(), "{when}: message rows are drawn");
    for pair in rows.windows(2) {
        let ((a, above), (b, below)) = (pair[0], pair[1]);
        assert!(
            above.bottom() <= below.top() + px(0.5),
            "{when}: message {a} (bottom {:?}) runs into message {b} (top {:?})",
            above.bottom(),
            below.top()
        );
    }
}

#[gpui::test]
fn chat_messages_never_overlap(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(Arc::new(Vault), None, cx);
        roostr_app::init(cx);
    });
    let (_root, cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| Workspace::new(roostr_app::pages(), window, cx));
        gpui_kit::base::Root::new(workspace, window, cx)
    });
    cx.simulate_resize(size(px(1280.), px(800.)));
    cx.update(|_, cx| {
        PaneState::global(cx).update(cx, |pane, cx| pane.set_open(PaneTab::Chat, true, cx));
        navigate(Route::Object("page-1".into()), cx);
    });
    settle(cx);
    assert_no_overlap(cx, "when the chat opens");

    let list = cx
        .debug_bounds("chat-messages")
        .expect("the chat pane's messages");
    let probe = point(list.left() + px(60.), list.top() + list.size.height * 0.5);
    wheel(cx, probe, 500.);
    settle(cx);
    assert_no_overlap(cx, "after scrolling back");

    // Dragging the chat column to its narrowest: every message re-wraps taller.
    cx.update(|_, cx| {
        PaneState::global(cx).update(cx, |pane, cx| pane.set_width(PaneTab::Chat, 0., cx));
    });
    settle(cx);
    assert_no_overlap(cx, "after the chat column narrows");

    // A small window squeezes it further still.
    cx.simulate_resize(size(px(760.), px(800.)));
    settle(cx);
    assert_no_overlap(cx, "in a small window");

    // And back wide: the messages re-wrap shorter.
    cx.simulate_resize(size(px(1400.), px(800.)));
    cx.update(|_, cx| {
        PaneState::global(cx).update(cx, |pane, cx| pane.set_width(PaneTab::Chat, 600., cx));
    });
    settle(cx);
    assert_no_overlap(cx, "after the chat column widens again");

    // A phone-width window: the chat is the bottom sheet, its widest messages a few words a line.
    cx.simulate_resize(size(px(400.), px(800.)));
    cx.update(|_, cx| PaneState::global(cx).update(cx, |pane, cx| pane.set_sheet(true, cx)));
    settle(cx);
    assert_no_overlap(cx, "on a phone-width window");
}

//! The right pane on a desktop: two columns beside the page, Chat then
//! Properties, each with its own collapse button and width. A collapsed
//! column is a strip with its name that expands it again; Escape collapses
//! the column expanded last; a window too narrow for both shows one and a
//! strip. The columns' state and widths outlive the app (a new app on the
//! same UI-state file opens them as they were left).
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
use gpui_kit::{point, px, size};
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value};
use roostr_core::{Backend, BackendEvent, JsonMap, PaneState};
use roostr_shell::Workspace;
use serde_json::{Value as Json, json};

/// One space holding a page with a short discussion.
struct Vault;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn page() -> Object {
    let mut page: Object = serde_json::from_value(json!({
        "id": "page-1",
        "typeKey": "page",
        "fields": {},
        "blocks": [
            { "id": "__discussion__", "childrenIds": ["m1"], "content": {} },
            { "id": "m1", "childrenIds": [], "content": { "custom": {
                "contentType": "chat",
                "meta": { "author": "peer", "ts": "1001", "text": "Hello" },
            } } },
        ],
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
        "a page with a chat".into()
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

/// Let column transitions (`grid-template-columns 0.16s`) finish.
fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn shown(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn tap(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
        .center();
    cx.simulate_click(at, Modifiers::none());
    settle(cx);
}

/// (Chat column, Properties column) as expanded columns on screen; panics
/// unless each is exactly one of its column or its strip.
fn columns(cx: &mut VisualTestContext) -> (bool, bool) {
    let chat = (shown(cx, "pane-col-Chat"), shown(cx, "pane-strip-Chat"));
    let props = (
        shown(cx, "pane-col-Properties"),
        shown(cx, "pane-strip-Properties"),
    );
    assert!(
        chat.0 != chat.1,
        "Chat shows as a column or a strip: {chat:?}"
    );
    assert!(
        props.0 != props.1,
        "Properties shows as a column or a strip: {props:?}"
    );
    (chat.0, props.0)
}

/// A desktop app on `page-1` in a `width` x 900 window.
fn open(cx: &mut TestAppContext, width: f32) -> &mut VisualTestContext {
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(Arc::new(Vault), None, cx);
        roostr_app::init(cx);
    });
    let (_workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(width), px(900.)));
    settle(cx);
    cx.update(|_, cx| navigate(Route::Object("page-1".into()), cx));
    settle(cx);
    cx
}

#[gpui::test]
fn chat_and_properties_are_columns_that_collapse_to_strips_and_persist(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before either app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };

    let cx = open(first, 1600.);
    // Both columns by default, Chat then Properties, right of the page.
    assert_eq!(columns(cx), (true, true));
    let chat = cx.debug_bounds("pane-col-Chat").unwrap();
    let props = cx.debug_bounds("pane-col-Properties").unwrap();
    assert!(chat.right() <= props.left(), "Chat sits left of Properties");
    assert!(shown(cx, "chat-messages"), "the chat");
    assert!(!shown(cx, "pane-tab-Chat"), "no tab switch on a desktop");

    // Collapsing Chat leaves Properties and a Chat strip; the strip expands it.
    tap(cx, "pane-collapse-Chat");
    assert_eq!(columns(cx), (false, true));
    assert!(!shown(cx, "chat-messages"));
    tap(cx, "pane-strip-Chat");
    assert_eq!(columns(cx), (true, true));
    assert!(shown(cx, "chat-messages"), "the chat is back");

    // Properties likewise; then both collapsed are two strips.
    tap(cx, "pane-collapse-Properties");
    assert_eq!(columns(cx), (true, false));
    tap(cx, "pane-collapse-Chat");
    assert_eq!(columns(cx), (false, false));
    tap(cx, "pane-strip-Properties");
    assert_eq!(columns(cx), (false, true));
    tap(cx, "pane-strip-Chat");
    assert_eq!(columns(cx), (true, true));

    // Escape that nothing focused handles (the chat's composer has the
    // focus as it opens) collapses the column expanded last (Chat), then the other.
    cx.update(|window, cx| window.blur(cx));
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert_eq!(columns(cx), (false, true));
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert_eq!(columns(cx), (false, false));
    tap(cx, "pane-strip-Chat");

    // The Chat column's resizer: dragged 100px left, the column is 100px wider.
    let before = cx.debug_bounds("pane-col-Chat").unwrap().size.width;
    let grip = cx.debug_bounds("pane-resize-Chat").unwrap().center();
    cx.simulate_mouse_down(grip, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(grip.x - px(50.), grip.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(
        point(grip.x - px(100.), grip.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        point(grip.x - px(100.), grip.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    settle(cx);
    let after = cx.debug_bounds("pane-col-Chat").unwrap().size.width;
    assert_eq!(after - before, px(100.));
    let chat_width = cx.update(|_, cx| PaneState::global(cx).read(cx).chat_width);

    // A window too narrow for both (the page keeps its 420px): Chat, the
    // column expanded last, and a Properties strip; nothing overflows.
    cx.simulate_resize(size(px(1100.), px(900.)));
    settle(cx);
    assert_eq!(columns(cx), (true, false));
    let desk = cx.debug_bounds("desk").unwrap();
    let strip = cx.debug_bounds("pane-strip-Properties").unwrap();
    assert!(
        strip.right() <= desk.right(),
        "the strip is inside the window"
    );
    // The strip brings Properties in, Chat giving way to its strip.
    tap(cx, "pane-strip-Properties");
    assert_eq!(columns(cx), (false, true));
    // With the room back, both show again.
    cx.simulate_resize(size(px(1600.), px(900.)));
    settle(cx);
    assert_eq!(columns(cx), (true, true));
    // Leave Chat expanded and Properties collapsed.
    tap(cx, "pane-collapse-Properties");
    assert_eq!(columns(cx), (true, false));

    // A new app on the same UI-state file opens the columns as they were left.
    let cx = open(second, 1600.);
    assert_eq!(columns(cx), (true, false));
    assert_eq!(
        cx.update(|_, cx| PaneState::global(cx).read(cx).chat_width),
        chat_width,
        "the Chat column's width"
    );
    tap(cx, "pane-strip-Properties");
    assert_eq!(columns(cx), (true, true));
}

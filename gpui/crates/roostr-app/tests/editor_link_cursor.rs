//! A link or mention in a page's text shows a pointing hand under the
//! pointer, as a link does on the web: a click opens it. Past the text, or on
//! plain words, the text cursor stays.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{Modifiers, TestAppContext, VisualTestContext};
use gpui_kit::{px, size};
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{
    Block, BlockContent, Object, ObjectSummary, QueryResult, RelationDef, Settings, Space,
    TextContent, Value, mark,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use serde_json::Value as Json;

const SPACE: &str = "sp-1";
const PAGE: &str = "page-1";
const TEXT: &str = "See example.com today";

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

/// One page with one line, as the store holds it: a block update replaces
/// the line's content (the editor reloads the page after a menu action).
/// Every mutation it receives, kept.
struct Vault {
    page: Mutex<Object>,
    mutations: Mutex<Vec<(String, JsonMap)>>,
}

impl Default for Vault {
    fn default() -> Self {
        Self { page: Mutex::new(page()), mutations: Mutex::default() }
    }
}

fn page() -> Object {
    let mut fields = roostr_core::types::Fields::default();
    fields.insert("name".into(), Value::string("Plans"));
    fields.insert("channel".into(), Value::string(SPACE));
    Object {
        id: PAGE.into(),
        type_key: "page".into(),
        fields,
        blocks: vec![Block {
            id: "p0".into(),
            content: BlockContent {
                text: Some(TextContent {
                    text: TEXT.into(),
                    // "example.com", a web link.
                    marks: Some(vec![roostr_core::types::Mark { from: 4, to: 15, kind: mark::LINK, param: Some("https://example.com".into()) }]),
                    ..TextContent::default()
                }),
                ..BlockContent::default()
            },
            ..Block::default()
        }],
        ..Object::default()
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "one page".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings { has_key: true, ..Settings::default() }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(vec![ObjectSummary {
            id: PAGE.into(),
            type_key: "page".into(),
            name: "Plans".into(),
            channel_id: SPACE.into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        }]))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        ready(if id == PAGE { Ok(self.page.lock().clone()) } else { Err(anyhow::anyhow!("unknown object {id}")) })
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space { id: SPACE.into(), name: "Personal".into(), created_at: 1, ..Space::default() }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, _: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        ready(Ok(QueryResult::default()))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        if action == "block_update"
            && let Some(content) = params.get("content").cloned()
            && let Ok(content) = serde_json::from_value::<BlockContent>(content)
        {
            let block_id = params.get("block_id").and_then(Json::as_str).unwrap_or_default();
            if let Some(b) = self.page.lock().blocks.iter_mut().find(|b| b.id == block_id) {
                b.content = content;
            }
        }
        self.mutations.lock().push((action.to_string(), params));
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
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
}

#[gpui::test]
fn a_link_shows_a_pointing_hand(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    let backend = Arc::new(Vault::default());
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(backend.clone(), None, cx);
        roostr_app::init(cx);
    });
    let (_workspace, cx) =
        cx.add_window_view(|window, cx| roostr_shell::Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(1400.), px(700.)));
    settle(cx);
    cx.update(|_, cx| navigate(Route::Object(PAGE.into()), cx));
    settle(cx);
    settle(cx);

    // Sweep the pointer along the line: the hand shows over "example.com"
    // only - one stretch, after "See ", well before the line's end.
    let line = cx.debug_bounds("block-p0").expect("the line");
    let y = line.center().y;
    let mut hand = Vec::new();
    let mut x = line.left() + px(1.);
    while x < line.right() {
        cx.simulate_mouse_move(gpui::point(x, y), None, Modifiers::none());
        cx.run_until_parked();
        hand.push(cx.debug_bounds("editor-link-hover").is_some());
        x += px(2.);
    }
    let first = hand.iter().position(|h| *h).expect("a hand over the link");
    let last = hand.iter().rposition(|h| *h).unwrap();
    assert!(first > 3, "not over \"See \"");
    assert!(hand[first..=last].iter().all(|h| *h), "one stretch: the link");
    assert!(last + 20 < hand.len(), "and none past the text");
}

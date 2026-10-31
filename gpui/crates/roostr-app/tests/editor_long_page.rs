//! A long page builds only the blocks in and near the window. Field report:
//! scrolling long pages lagged - the page scrolls the editor, and each frame
//! built every block (a text field each). Blocks far below aren't built, yet
//! editing works as before: typing in a block saves it, and arrowing down
//! past the built blocks moves the caret on and types there.
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
    TextContent, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use serde_json::Value as Json;

const SPACE: &str = "sp-1";
const PAGE: &str = "page-1";
const BLOCKS: usize = 300;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

/// One long page; every mutation it receives, kept.
#[derive(Default)]
struct Vault {
    mutations: Mutex<Vec<(String, JsonMap)>>,
}

fn page() -> Object {
    let blocks = (0..BLOCKS)
        .map(|i| Block {
            id: format!("p{i}"),
            content: BlockContent {
                text: Some(TextContent {
                    text: format!("Paragraph {i}"),
                    ..TextContent::default()
                }),
                ..BlockContent::default()
            },
            ..Block::default()
        })
        .collect();
    let mut fields = roostr_core::types::Fields::default();
    fields.insert("name".into(), Value::string("Long page"));
    fields.insert("channel".into(), Value::string(SPACE));
    Object {
        id: PAGE.into(),
        type_key: "page".into(),
        fields,
        blocks,
        ..Object::default()
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "a long page".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings { has_key: true, ..Settings::default() }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(vec![ObjectSummary {
            id: PAGE.into(),
            type_key: "page".into(),
            name: "Long page".into(),
            channel_id: SPACE.into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        }]))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        ready(if id == PAGE { Ok(page()) } else { Err(anyhow::anyhow!("unknown object {id}")) })
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

/// The block whose saved text contains `needle`, if a save carried it.
fn saved_with(vault: &Vault, needle: &str) -> Option<String> {
    vault.mutations.lock().iter().find_map(|(_, p)| {
        let text = serde_json::to_string(p).ok()?;
        text.contains(needle)
            .then(|| p.get("block_id").and_then(Json::as_str).unwrap_or("?").to_string())
    })
}

#[gpui::test]
fn a_long_page_builds_only_the_blocks_near_the_window(cx: &mut TestAppContext) {
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

    assert!(cx.debug_bounds("block-p0").is_some(), "the first block");
    assert!(cx.debug_bounds("block-p299").is_none(), "the last block, far below, isn't built");
    assert!(cx.debug_bounds("block-p50").is_none(), "nor is block 50");

    // Typing in a block saves it.
    let first = cx.debug_bounds("block-p0").unwrap();
    cx.simulate_click(first.center(), Modifiers::none());
    settle(cx);
    cx.simulate_keystrokes("cmd-right");
    cx.simulate_input(" ZZ");
    settle(cx);
    assert_eq!(saved_with(&backend, "ZZ").as_deref(), Some("p0"), "the typing was saved on p0");

    // Down past every block that was built, then type: it lands further on.
    for _ in 0..120 {
        cx.simulate_keystrokes("down");
    }
    cx.run_until_parked();
    cx.simulate_input(" YY");
    settle(cx);
    let landed = saved_with(&backend, "YY").expect("the second typing was saved");
    let n: usize = landed.trim_start_matches('p').parse().expect("a paragraph");
    assert!(n >= 50, "the caret moved past the blocks first built: typed in {landed}");
}

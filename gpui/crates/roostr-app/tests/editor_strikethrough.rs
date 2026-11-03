//! ⌘⇧S strikes the selected text through in a page, as ⌘B bolds it, and
//! takes it off again. Strikethrough was only on the selection toolbar, the
//! one basic mark without a shortcut.
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
const TEXT: &str = "Cancelled plan";

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

/// One page with one line; every mutation it receives, kept.
#[derive(Default)]
struct Vault {
    mutations: Mutex<Vec<(String, JsonMap)>>,
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
                text: Some(TextContent { text: TEXT.into(), ..TextContent::default() }),
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

/// The marks (from, to, type) of the line's newest save; a save without
/// marks leaves them out, so it reads as none.
fn saved_marks(vault: &Vault) -> Option<Vec<(u64, u64, u64)>> {
    fn marks(v: &Json) -> Option<&Vec<Json>> {
        match v {
            Json::Object(m) => m.get("marks").and_then(Json::as_array).or_else(|| m.values().find_map(marks)),
            Json::Array(a) => a.iter().find_map(marks),
            _ => None,
        }
    }
    let mutations = vault.mutations.lock();
    let (_, save) = mutations.iter().rev().find(|(_, p)| {
        serde_json::to_string(p).is_ok_and(|s| s.contains(TEXT))
    })?;
    let save = Json::Object(save.clone());
    Some(
        marks(&save)
            .map(|ms| {
                ms.iter()
                    .map(|m| {
                        let n = |k: &str| m.get(k).and_then(Json::as_u64).unwrap_or(0);
                        (n("from"), n("to"), n("type"))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

#[gpui::test]
fn cmd_shift_s_strikes_the_selection_through_and_back(cx: &mut TestAppContext) {
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

    // The whole line selected.
    let line = cx.debug_bounds("block-p0").expect("the line");
    cx.simulate_click(line.center(), Modifiers::none());
    settle(cx);
    cx.simulate_keystrokes("cmd-right shift-cmd-left");
    cx.simulate_keystrokes("cmd-shift-s");
    settle(cx);
    let end = TEXT.encode_utf16().count() as u64;
    assert_eq!(
        saved_marks(&backend),
        Some(vec![(0, end, u64::from(mark::STRIKETHROUGH))]),
        "⌘⇧S struck the line through"
    );

    // Applying a mark leaves the caret at its end (as ⌘B does); the line
    // selected again, ⌘⇧S takes it off.
    cx.simulate_keystrokes("shift-cmd-left cmd-shift-s");
    settle(cx);
    assert_eq!(saved_marks(&backend), Some(Vec::new()), "a second ⌘⇧S took it off");
}

//! A pinned list in the nav builds only the rows in and near the window, and
//! scrolling the nav builds the rest as they come near. Field report: opening
//! a page's Properties column lagged - every click redraws the whole window,
//! and the nav laid out all of its pinned list's rows (up to 500) each time.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, TouchPhase,
    VisualTestContext,
};
use gpui_kit::{point, px, size};
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_shell::Workspace;
use serde_json::Value as Json;

const ROWS: usize = 300;
const FIRST: &str = "w-row-agent-0";
const MIDDLE: &str = "w-row-agent-150";
const LAST: &str = "w-row-agent-299";

/// One space with one pinned query listing `ROWS` agents.
struct Vault;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn row(n: usize) -> QueryRow {
    QueryRow {
        id: format!("agent-{n}"),
        type_key: "agent".into(),
        name: Some(format!("Agent {n}")),
        ..QueryRow::default()
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "one long pinned query".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(vec![ObjectSummary {
            id: "q-1".into(),
            type_key: "query".into(),
            name: "Agents".into(),
            channel_id: "sp-1".into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        }]))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        ready(if id == "q-1" {
            Ok(Object {
                id: id.into(),
                type_key: "query".into(),
                fields: [
                    ("name".to_string(), Value::string("Agents")),
                    ("channel".to_string(), Value::string("sp-1")),
                ]
                .into(),
                ..Object::default()
            })
        } else {
            Err(anyhow::anyhow!("unknown object {id}"))
        })
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            pinned_ids: vec!["q-1".into()],
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        if body["setId"] != "q-1" {
            return ready(Ok(QueryResult::default()));
        }
        let records: Vec<QueryRow> = (0..ROWS).map(row).collect();
        ready(Ok(QueryResult {
            total: records.len() as u64,
            records,
        }))
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
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn built(cx: &mut VisualTestContext, row: &'static str) -> bool {
    cx.debug_bounds(row).is_some()
}

#[gpui::test]
fn the_navs_pinned_list_builds_only_the_rows_near_the_window(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(Arc::new(Vault), None, cx);
        roostr_app::init(cx);
    });
    let (_workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    settle(cx);
    settle(cx);

    assert!(built(cx, FIRST), "the list's first row");
    assert!(
        !built(cx, LAST),
        "the last of {ROWS} rows, far below the window, isn't built"
    );
    assert!(!built(cx, MIDDLE), "nor is row 150");

    // Scrolling the nav to its end builds the rows that come into view.
    let at = cx.debug_bounds("w-row-agent-0").unwrap().center();
    cx.simulate_event(ScrollEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(-(ROWS as f32) * 30.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    settle(cx);
    assert!(
        built(cx, LAST),
        "scrolled to the end, the last row is built"
    );
    assert!(!built(cx, FIRST), "and the first, far above now, is not");
}

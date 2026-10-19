//! A pinned query's sidebar widget follows the vault: records that sync in
//! (or change) after the widget first loaded show without the pinned object
//! itself changing. Field report: on a phone whose first history walk ran
//! under the open sidebar, a pinned widget kept the half-synced rows.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{px, size};
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_shell::Workspace;
use serde_json::Value as Json;

/// One space with one pinned query; the query's records are `rows`.
struct Vault {
    rows: Mutex<Vec<QueryRow>>,
    /// Queries the widget asked (`setId` of the pinned query).
    widget_queries: Mutex<usize>,
    events: Mutex<Option<mpsc::UnboundedReceiver<anyhow::Result<BackendEvent>>>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn row(id: &str) -> QueryRow {
    QueryRow {
        id: id.into(),
        type_key: "agent".into(),
        name: Some(id.into()),
        ..QueryRow::default()
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "one pinned query".into()
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
        *self.widget_queries.lock() += 1;
        let records = self.rows.lock().clone();
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
        match self.events.lock().take() {
            Some(rx) => Box::pin(rx),
            None => Box::pin(futures::stream::pending()),
        }
    }
}

fn settle(cx: &mut VisualTestContext) {
    // Past the store's refresh throttle.
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
}

#[gpui::test]
fn a_pinned_query_widget_reloads_when_other_objects_sync_in(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    let (tx, rx) = mpsc::unbounded();
    let backend = Arc::new(Vault {
        rows: Mutex::new(vec![row("agent-1")]),
        widget_queries: Mutex::new(0),
        events: Mutex::new(Some(rx)),
    });
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(backend.clone(), None, cx);
        roostr_app::init(cx);
    });
    let (_workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    settle(cx);
    let loaded = *backend.widget_queries.lock();
    assert!(loaded > 0, "the sidebar shows the pinned query's widget");

    // A record the widget lists syncs in; the query object itself never changes.
    backend.rows.lock().push(row("agent-2"));
    tx.unbounded_send(Ok(BackendEvent::Committed("agent-2".into())))
        .unwrap();
    settle(cx);
    assert!(
        *backend.widget_queries.lock() > loaded,
        "the widget asked its query again"
    );
}

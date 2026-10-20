//! A pinned widget's fold is the pinned object's `pin_collapsed` field: a
//! toggle writes it once (showing at once), a change from another device
//! shows when its commit arrives, and the per-device `widget-collapsed` map
//! of earlier builds is written onto the objects once, then dropped.
#![cfg(not(target_family = "wasm"))]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{px, size};
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_shell::Workspace;
use serde_json::{Value as Json, json};

const PINS: [&str; 2] = ["note-1", "query-1"];

/// One space pinning a note and a query; `set_field` applies to `fields`.
struct Vault {
    fields: Mutex<HashMap<String, Fields>>,
    /// `(object_id, key, value)` of every `set_field`.
    writes: Mutex<Vec<(String, String, Json)>>,
    events: Mutex<Option<mpsc::UnboundedReceiver<anyhow::Result<BackendEvent>>>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn type_of(id: &str) -> &'static str {
    if id == "query-1" { "query" } else { "note" }
}

impl Vault {
    fn folds_written(&self, id: &str) -> Vec<Json> {
        self.writes
            .lock()
            .iter()
            .filter(|(o, k, _)| o == id && k == "pin_collapsed")
            .map(|(_, _, v)| v.clone())
            .collect()
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "two pins".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(PINS
            .iter()
            .map(|id| ObjectSummary {
                id: (*id).into(),
                type_key: type_of(id).into(),
                name: (*id).into(),
                channel_id: "sp-1".into(),
                updated_at: 1_000,
                ..ObjectSummary::default()
            })
            .collect()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        ready(match self.fields.lock().get(id) {
            Some(fields) => Ok(Object {
                id: id.into(),
                type_key: type_of(id).into(),
                fields: fields.clone(),
                ..Object::default()
            }),
            None => Err(anyhow::anyhow!("unknown object {id}")),
        })
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            pinned_ids: PINS.iter().map(|id| (*id).into()).collect(),
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
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        if action == "set_field" {
            let id = params["object_id"].as_str().unwrap_or("").to_string();
            let key = params["key"].as_str().unwrap_or("").to_string();
            let value = params["value"].clone();
            if let Ok(v) = serde_json::from_value::<Value>(value.clone())
                && let Some(fields) = self.fields.lock().get_mut(&id)
            {
                fields.insert(key.clone(), v);
            }
            self.writes.lock().push((id, key, value));
        }
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
fn a_pinned_widgets_fold_lives_on_the_object(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    let ui_path = ui_state.path().join("ui-state.json");
    // An earlier build folded the note on this device (and unfolded the query).
    std::fs::write(
        &ui_path,
        json!({ "widget-collapsed": { "note-1": true, "query-1": false } }).to_string(),
    )
    .unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", &ui_path) };
    let (tx, rx) = mpsc::unbounded();
    let backend = Arc::new(Vault {
        fields: Mutex::new(
            PINS.iter()
                .map(|id| ((*id).to_string(), Fields::default()))
                .collect(),
        ),
        writes: Mutex::new(Vec::new()),
        events: Mutex::new(Some(rx)),
    });
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(backend.clone(), None, cx);
        roostr_app::init(cx);
    });
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    settle(cx);
    let folded = |id: &str, cx: &mut VisualTestContext| {
        workspace.update(cx, |w, cx| w.pin_collapsed(id, cx))
    };

    // Migration: the local fold is written onto the note, once; the open
    // default is not written; the local key is gone.
    assert_eq!(
        backend.folds_written("note-1"),
        vec![json!({ "boolValue": true })]
    );
    assert!(backend.folds_written("query-1").is_empty());
    assert!(folded("note-1", cx));
    assert!(!folded("query-1", cx));
    let saved: Json = serde_json::from_str(&std::fs::read_to_string(&ui_path).unwrap()).unwrap();
    assert!(saved.get("widget-collapsed").is_none(), "{saved}");
    tx.unbounded_send(Ok(BackendEvent::Committed("note-1".into())))
        .unwrap();
    settle(cx);
    assert_eq!(backend.folds_written("note-1").len(), 1, "migrated once");

    // A toggle shows at once and writes the field once.
    workspace.update(cx, |w, cx| w.toggle_pin_collapsed("query-1", cx));
    assert!(
        folded("query-1", cx),
        "the fold shows before the write lands"
    );
    settle(cx);
    assert!(folded("query-1", cx));
    assert_eq!(
        backend.folds_written("query-1"),
        vec![json!({ "boolValue": true })]
    );

    // Another device unfolds it: the commit brings it here.
    backend
        .fields
        .lock()
        .get_mut("query-1")
        .unwrap()
        .insert("pin_collapsed".into(), Value::bool(false));
    tx.unbounded_send(Ok(BackendEvent::Committed("query-1".into())))
        .unwrap();
    settle(cx);
    assert!(!folded("query-1", cx));
    assert_eq!(backend.folds_written("query-1").len(), 1);
}

//! A pinned widget's fold is the pinned object's `pin_collapsed` field: a
//! toggle writes it once (showing at once), a change from another device
//! shows when its commit arrives, and the per-device `widget-collapsed` map
//! of earlier builds is written onto the objects once, then dropped.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::channel::mpsc;
use parking_lot::Mutex;
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream};
use roostr_shell::{pin_collapsed, toggle_pin_collapsed};
use serde_json::{Value as Json, json};

use common::Harness;

const PINS: [&str; 2] = ["note-1", "query-1"];

/// One space pinning a note and a query; `set_field` applies to `fields`.
struct Vault {
    fields: Mutex<HashMap<String, Fields>>,
    /// `(object_id, key, value)` of every `set_field`.
    writes: Mutex<Vec<(String, String, Json)>>,
    events: Mutex<Option<mpsc::UnboundedReceiver<Result<BackendEvent>>>>,
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

/// The store's handle on the shared vault.
struct Shared(Arc<Vault>);

impl Backend for Shared {
    fn describe(&self) -> String {
        "two pins".into()
    }
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
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
    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        ready(match self.0.fields.lock().get(id) {
            Some(fields) => Ok(Object {
                id: id.into(),
                type_key: type_of(id).into(),
                fields: fields.clone(),
                ..Object::default()
            }),
            None => Err(anyhow::anyhow!("unknown object {id}")),
        })
    }
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            pinned_ids: PINS.iter().map(|id| (*id).into()).collect(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, _: &Json) -> BoxFuture<Result<QueryResult>> {
        ready(Ok(QueryResult::default()))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<Result<JsonMap>> {
        if action == "set_field" {
            let id = params["object_id"].as_str().unwrap_or("").to_string();
            let key = params["key"].as_str().unwrap_or("").to_string();
            let value = params["value"].clone();
            if let Ok(v) = serde_json::from_value::<Value>(value.clone())
                && let Some(fields) = self.0.fields.lock().get_mut(&id)
            {
                fields.insert(key.clone(), v);
            }
            self.0.writes.lock().push((id, key, value));
        }
        ready(Ok(JsonMap::new()))
    }
    fn serving(&self, _: &[String]) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        match self.0.events.lock().take() {
            Some(rx) => Box::pin(rx),
            None => Box::pin(futures::stream::pending()),
        }
    }
}

/// Deliver what the backend sent, then run past the store's refresh throttle.
fn settle(h: &mut Harness) {
    h.settle();
    h.advance(Duration::from_secs(2));
}

#[test]
fn a_pinned_widgets_fold_lives_on_the_object() {
    let (tx, rx) = mpsc::unbounded();
    let vault = Arc::new(Vault {
        fields: Mutex::new(
            PINS.iter()
                .map(|id| ((*id).to_string(), Fields::default()))
                .collect(),
        ),
        writes: Mutex::new(Vec::new()),
        events: Mutex::new(Some(rx)),
    });
    // An earlier build folded the note on this device (and unfolded the query).
    let mut h = Harness::with_ui(
        Arc::new(Shared(vault.clone())),
        (1280., 800.),
        json!({ "widget-collapsed": { "note-1": true, "query-1": false } }),
    );
    settle(&mut h);

    // Migration: the local fold is written onto the note, once; the open
    // default is not written; the local key is gone.
    assert_eq!(
        vault.folds_written("note-1"),
        vec![json!({ "boolValue": true })]
    );
    assert!(vault.folds_written("query-1").is_empty());
    assert!(pin_collapsed(h.state(), "note-1"));
    assert!(!pin_collapsed(h.state(), "query-1"));
    let saved: Json = serde_json::from_str(&std::fs::read_to_string(&h.ui_path).unwrap()).unwrap();
    assert!(saved.get("widget-collapsed").is_none(), "{saved}");
    tx.unbounded_send(Ok(BackendEvent::Committed("note-1".into())))
        .unwrap();
    settle(&mut h);
    assert_eq!(vault.folds_written("note-1").len(), 1, "migrated once");

    // A toggle shows at once and writes the field once.
    h.rt.update(|s, _| toggle_pin_collapsed(s, "query-1"));
    assert!(
        pin_collapsed(h.state(), "query-1"),
        "the fold shows before the write lands"
    );
    settle(&mut h);
    assert!(pin_collapsed(h.state(), "query-1"));
    assert_eq!(
        vault.folds_written("query-1"),
        vec![json!({ "boolValue": true })]
    );

    // Another device unfolds it: the commit brings it here.
    vault
        .fields
        .lock()
        .get_mut("query-1")
        .unwrap()
        .insert("pin_collapsed".into(), Value::bool(false));
    tx.unbounded_send(Ok(BackendEvent::Committed("query-1".into())))
        .unwrap();
    settle(&mut h);
    assert!(!pin_collapsed(h.state(), "query-1"));
    assert_eq!(vault.folds_written("query-1").len(), 1);
}

//! The shell's motion: held sidebar cards and rows shrink and counts roll their digits.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::channel::mpsc;
use parking_lot::Mutex;
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, SyncPhase, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap, State};
use roostr_rt::{BoxFuture, BoxStream};
use serde_json::Value as Json;

use common::Harness;

/// Space Personal (`sp-1`) with a pinned kanban query (`q-1`, grouped by `done`) and a page
/// (`page-2`, in Recently edited). The query's records are `rows`.
struct Vault {
    rows: Mutex<Vec<QueryRow>>,
    events: Mutex<Option<mpsc::UnboundedReceiver<Result<BackendEvent>>>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn task(id: &str, done: bool) -> QueryRow {
    QueryRow {
        id: id.into(),
        type_key: "task".into(),
        name: Some(id.into()),
        fields: [("done".to_string(), Value::bool(done))].into(),
        ..QueryRow::default()
    }
}

struct Shared(Arc<Vault>);

impl Backend for Shared {
    fn describe(&self) -> String {
        "a pinned kanban".into()
    }
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        ready(Ok(vec![
            ObjectSummary {
                id: "q-1".into(),
                type_key: "query".into(),
                name: "Tasks".into(),
                channel_id: "sp-1".into(),
                updated_at: 1_000,
                ..ObjectSummary::default()
            },
            ObjectSummary {
                id: "page-2".into(),
                type_key: "page".into(),
                name: "Notes".into(),
                icon: "📝".into(),
                channel_id: "sp-1".into(),
                updated_at: 2_000,
                ..ObjectSummary::default()
            },
        ]))
    }
    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        let fields: Vec<(&str, &str)> = match id {
            "q-1" => vec![
                ("name", "Tasks"),
                ("channel", "sp-1"),
                ("viewType", "kanban"),
                ("viewGroupKey", "done"),
            ],
            "page-2" => vec![("name", "Notes"), ("channel", "sp-1")],
            _ => return ready(Err(anyhow::anyhow!("unknown object {id}"))),
        };
        ready(Ok(Object {
            id: id.into(),
            type_key: if id == "q-1" { "query" } else { "page" }.into(),
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), Value::string(v)))
                .collect(),
            ..Object::default()
        }))
    }
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            pinned_ids: vec!["q-1".into()],
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        ready(Ok(vec![RelationDef {
            id: "rel-done".into(),
            key: "done".into(),
            format: "checkbox".into(),
            name: "Done".into(),
            space: Some("sp-1".into()),
            ..RelationDef::default()
        }]))
    }
    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>> {
        if body["setId"] != "q-1" {
            return ready(Ok(QueryResult::default()));
        }
        let records = self.0.rows.lock().clone();
        ready(Ok(QueryResult {
            total: records.len() as u64,
            records,
        }))
    }
    fn mutate(&self, _: &str, _: JsonMap) -> BoxFuture<Result<JsonMap>> {
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

fn vault(rows: Vec<QueryRow>) -> (Arc<Vault>, mpsc::UnboundedSender<Result<BackendEvent>>) {
    let (tx, rx) = mpsc::unbounded();
    let vault = Arc::new(Vault {
        rows: Mutex::new(rows),
        events: Mutex::new(Some(rx)),
    });
    (vault, tx)
}

fn harness(vault: &Arc<Vault>, size: (f32, f32)) -> Harness {
    let mut h = Harness::new(Arc::new(Shared(vault.clone())), size);
    settle(&mut h);
    h
}

/// Deliver what the backend sent, then run past the store's refresh throttle.
fn settle(h: &mut Harness) {
    h.settle();
    h.advance(Duration::from_secs(2));
}

fn scale(h: &Harness, key: &str) -> f32 {
    h.rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
        .unwrap_or_else(|| panic!("no element keyed {key}"))
        .scale
}

/// The shown label of the element keyed `key` (a rolling count reads as its whole text).
fn label(h: &Harness, key: &str) -> String {
    h.rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
        .and_then(|e| e.text)
        .unwrap_or_else(|| panic!("no text keyed {key}"))
}

/// Hold a press at window point `at` for a while: the scale of `key` while held, then
/// after the release (settled).
fn hold(h: &mut Harness, at: [f32; 2], key: &str) -> (f32, f32) {
    h.rt.pointer_move(at[0], at[1]);
    h.rt.pointer_down(at[0], at[1]);
    for _ in 0..30 {
        h.rt.frame(1. / 60.);
    }
    let held = scale(h, key);
    h.rt.pointer_up(at[0], at[1]);
    h.settle();
    // The release springs back (with a little bounce) on the virtual clock.
    h.frames(120);
    (held, scale(h, key))
}

fn center(r: [f32; 4]) -> [f32; 2] {
    [r[0] + r[2] / 2., r[1] + r[3] / 2.]
}

#[test]
fn held_sidebar_cards_and_rows_shrink() {
    let (vault, _tx) = vault(vec![task("t-1", false)]);
    let mut h = harness(&vault, (1280., 800.));
    h.update(|s| navigate(Route::Graph { focus: None }, s));

    // The pinned card's head (a row of the card) dips and opens the pin on release.
    let head = h.rect("widget-head:q-1").expect("the pinned card's head");
    let (held, after) = hold(&mut h, center(head), "widget-head:q-1");
    assert!((held - 0.985).abs() < 1e-3, "the held head dips: {held}");
    assert_eq!(after, 1., "and springs back");
    assert_eq!(*h.state().nav.route(), Route::Object("q-1".into()));

    // The card itself shrinks when held by its edge (where it drags to reorder).
    let card = h.rect("widget-box:q-1").expect("the pinned card");
    let edge = [card[0] + card[2] / 2., card[1] + card[3] - 3.];
    let (held, after) = hold(&mut h, edge, "widget:q-1");
    assert!((held - 0.97).abs() < 1e-3, "the held card shrinks: {held}");
    assert_eq!(after, 1.);

    // A Recently edited row dips like a list row.
    let row = h.rect("recent:page-2").expect("the recent row");
    let (held, after) = hold(&mut h, center(row), "recent:page-2");
    assert!((held - 0.985).abs() < 1e-3, "the held row dips: {held}");
    assert_eq!(after, 1.);
    assert_eq!(*h.state().nav.route(), Route::Object("page-2".into()));
}

#[test]
fn a_held_card_stays_put_under_reduced_motion() {
    let (vault, _tx) = vault(vec![task("t-1", false)]);
    let mut h = harness(&vault, (1280., 800.));
    h.rt.set_system_reduced_motion(true);
    h.update(|s| navigate(Route::Graph { focus: None }, s));
    let head = h.rect("widget-head:q-1").expect("the pinned card's head");
    let (held, _) = hold(&mut h, center(head), "widget-head:q-1");
    assert_eq!(held, 1.);
}

#[test]
fn a_pinned_kanban_count_rolls_to_its_new_value() {
    let (vault, tx) = vault(vec![task("t-1", false), task("t-2", false)]);
    let mut h = harness(&vault, (1280., 800.));
    let key = "w-group-n:q-1:false";
    assert_eq!(label(&h, key), "2");
    let glyph = h.keys(&format!("{key}."));

    vault.rows.lock().push(task("t-3", false));
    tx.unbounded_send(Ok(BackendEvent::Committed("t-3".into())))
        .unwrap();
    settle(&mut h);
    assert_eq!(label(&h, key), "3");
    assert_ne!(
        h.keys(&format!("{key}.")),
        glyph,
        "the new digit is a new glyph (it rolled in)"
    );
}

#[test]
fn the_phone_sync_percentage_rolls() {
    let (vault, _tx) = vault(vec![task("t-1", false)]);
    let mut h = harness(&vault, (390., 844.));
    let syncing = |received: u64| {
        move |s: &mut State| {
            s.store.status.phase = SyncPhase::Live;
            s.store.status.received = Some(received);
            s.store.status.expected = Some(100);
        }
    };
    h.update(|s| navigate(Route::Object("page-2".into()), s));
    h.update(syncing(10));
    assert_eq!(label(&h, "m-sync-label"), "10%");

    h.rt.update(|s, _| syncing(20)(s));
    h.rt.frame(1. / 120.);
    assert_eq!(label(&h, "m-sync-label"), "20%");
    let glyphs = h.rt.elements();
    let rolling = |exiting: bool, digit: &str| {
        glyphs.iter().any(|e| {
            e.exiting == exiting
                && e.text.as_deref() == Some(digit)
                && e.key
                    .as_deref()
                    .is_some_and(|k| k.starts_with("m-sync-label."))
        })
    };
    assert!(rolling(true, "1"), "the old tens digit rolls out");
    assert!(rolling(false, "2"), "the new one rolls in");
    h.settle();
    assert!(!rolling_left(&h), "the roll settles");
}

fn rolling_left(h: &Harness) -> bool {
    h.rt.elements().iter().any(|e| {
        e.exiting
            && e.key
                .as_deref()
                .is_some_and(|k| k.starts_with("m-sync-label."))
    })
}

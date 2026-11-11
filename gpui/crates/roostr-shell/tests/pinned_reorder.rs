//! Items of a pinned collection drag to reorder its members: a gallery card
//! sideways, a list row up or down (on the phone after a long press). The
//! drop writes the collection's `collectionIds`; a drop in place writes
//! nothing, a press still opens the item, a query pin's items never lift,
//! and the item's drag never moves the pin itself.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::channel::mpsc;
use parking_lot::Mutex;
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream};
use serde_json::Value as Json;

use common::Harness;

/// The pins of space `sp-1`: a gallery collection, a list collection, a query.
const PINS: [&str; 3] = ["c-gal", "c-list", "q-1"];

struct Vault {
    /// Each collection's members.
    members: Mutex<HashMap<String, Vec<String>>>,
    /// Every mutation asked: (op, object id, key).
    mutations: Mutex<Vec<(String, String, String)>>,
    events: Mutex<Option<mpsc::UnboundedReceiver<Result<BackendEvent>>>>,
    tx: mpsc::UnboundedSender<Result<BackendEvent>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn row(id: &str) -> QueryRow {
    QueryRow {
        id: id.into(),
        type_key: "page".into(),
        name: Some(id.to_uppercase()),
        fields: [("name".to_string(), Value::string(id.to_uppercase()))].into(),
        ..QueryRow::default()
    }
}

struct Shared(Arc<Vault>);

impl Backend for Shared {
    fn describe(&self) -> String {
        "pinned collections".into()
    }
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        let summary = |id: &str, type_key: &str| ObjectSummary {
            id: id.into(),
            type_key: type_key.into(),
            name: id.to_uppercase(),
            channel_id: "sp-1".into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        };
        let mut all = vec![
            summary("c-gal", "collection"),
            summary("c-list", "collection"),
            summary("q-1", "query"),
        ];
        for id in ["a", "b", "c", "x", "y", "z", "qa", "qb"] {
            all.push(summary(id, "page"));
        }
        ready(Ok(all))
    }
    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        let mut fields: roostr_core::types::Fields = [
            ("name".to_string(), Value::string(id.to_uppercase())),
            ("channel".to_string(), Value::string("sp-1")),
        ]
        .into();
        let type_key = match id {
            "c-gal" | "c-list" => {
                let members = self.0.members.lock()[id].clone();
                fields.insert(
                    "collectionIds".into(),
                    Value::values(members.iter().map(Value::string).collect()),
                );
                if id == "c-gal" {
                    fields.insert("viewType".into(), Value::string("gallery"));
                }
                "collection"
            }
            "q-1" => "query",
            _ => "page",
        };
        ready(Ok(Object {
            id: id.into(),
            type_key: type_key.into(),
            fields,
            ..Object::default()
        }))
    }
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            pinned_ids: PINS.iter().map(|p| p.to_string()).collect(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>> {
        // Newest first, as the engine's `updatedAt desc`: never the member order.
        let records: Vec<QueryRow> = if body["setId"] == "q-1" {
            vec![row("qb"), row("qa")]
        } else {
            let ids: Vec<String> = body["filters"]
                .as_array()
                .and_then(|f| f.iter().find(|f| f["key"] == "id"))
                .and_then(|f| f["value"].as_array())
                .map(|v| {
                    v.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            ids.iter().rev().map(|id| row(id)).collect()
        };
        ready(Ok(QueryResult {
            total: records.len() as u64,
            records,
        }))
    }
    fn mutate(&self, op: &str, params: JsonMap) -> BoxFuture<Result<JsonMap>> {
        let object = params["object_id"].as_str().unwrap_or("").to_string();
        let key = params
            .get("key")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        self.0
            .mutations
            .lock()
            .push((op.to_string(), object.clone(), key.clone()));
        if op == "set_field" && key == "collectionIds" {
            let value: Value = serde_json::from_value(params["value"].clone()).expect("a value");
            let ids = value
                .items()
                .iter()
                .filter_map(|i| i.string_value.clone())
                .collect();
            self.0.members.lock().insert(object.clone(), ids);
            let _ = self
                .0
                .tx
                .unbounded_send(Ok(BackendEvent::Committed(object)));
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

fn vault() -> Arc<Vault> {
    let (tx, rx) = mpsc::unbounded();
    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Arc::new(Vault {
        members: Mutex::new(
            [
                ("c-gal".to_string(), ids(&["a", "b", "c"])),
                ("c-list".to_string(), ids(&["x", "y", "z"])),
            ]
            .into(),
        ),
        mutations: Mutex::new(Vec::new()),
        events: Mutex::new(Some(rx)),
        tx,
    })
}

/// Deliver what the backend sent, then run past the store's refresh throttle.
fn settle(h: &mut Harness) {
    h.settle();
    h.advance(Duration::from_secs(2));
}

fn center(h: &Harness, key: &str) -> [f32; 2] {
    let r = h
        .rect(key)
        .unwrap_or_else(|| panic!("{key} is not on screen"));
    [r[0] + r[2] / 2., r[1] + r[3] / 2.]
}

fn members(vault: &Vault, id: &str) -> Vec<String> {
    vault.members.lock()[id].clone()
}

/// Every mutation asked so far.
fn mutations(vault: &Vault) -> Vec<(String, String, String)> {
    vault.mutations.lock().clone()
}

/// The desktop shell with a sidebar wide enough for the gallery's three columns.
fn desktop(vault: &Arc<Vault>) -> Harness {
    let mut h = Harness::with_ui(
        Arc::new(Shared(vault.clone())),
        (1280., 800.),
        serde_json::json!({ "side-width": "400" }),
    );
    settle(&mut h);
    h
}

/// Press at `from`, move to `to` in `steps` frames, and hold there (no release).
fn press_and_move(h: &mut Harness, from: [f32; 2], to: [f32; 2], steps: usize) {
    h.rt.pointer_move(from[0], from[1]);
    h.rt.pointer_down(from[0], from[1]);
    h.frame();
    for i in 1..=steps {
        let f = i as f32 / steps as f32;
        h.rt.pointer_move(
            from[0] + (to[0] - from[0]) * f,
            from[1] + (to[1] - from[1]) * f,
        );
        h.frame();
    }
}

fn release(h: &mut Harness, at: [f32; 2]) {
    h.rt.pointer_up(at[0], at[1]);
    settle(h);
}

/// The widget's cards or rows, by key, in screen order (left to right, top to bottom).
fn shown(h: &Harness, prefix: &str) -> Vec<String> {
    let mut keys: Vec<(f32, f32, String)> = h
        .keys(prefix)
        .into_iter()
        .map(|k| {
            let r = h.rect(&k).unwrap();
            (r[1].round(), r[0].round(), k[prefix.len()..].to_string())
        })
        .collect();
    keys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    keys.into_iter().map(|(_, _, k)| k).collect()
}

#[test]
fn a_gallery_card_dragged_sideways_reorders_the_collection() {
    let vault = vault();
    let mut h = desktop(&vault);
    // Three columns: the cards share a row.
    let (a, b, c) = (
        h.rect("w-card:a").expect("card a"),
        h.rect("w-card:b").expect("card b"),
        h.rect("w-card:c").expect("card c"),
    );
    assert!(
        a[1] == b[1] && b[1] == c[1],
        "one row of three: {a:?} {b:?} {c:?}"
    );
    assert!(a[0] < b[0] && b[0] < c[0]);
    let pins_before = h.state().store.space("sp-1").unwrap().pinned_ids.clone();

    // A past C's middle: mid-drag the ghost and the drop line show, nothing is written.
    let from = center(&h, "w-card:a");
    let to = [c[0] + c[2] * 0.75, c[1] + c[3] / 2.];
    press_and_move(&mut h, from, to, 6);
    assert!(h.has("w-ghost:c-gal"), "the lifted card's ghost follows");
    assert!(h.has("w-drop:c"), "the drop line marks after C");
    assert!(!h.has("w-ghost:c-list") && !h.has("w-drop:b"));
    assert!(mutations(&vault).is_empty());
    release(&mut h, to);

    assert_eq!(members(&vault, "c-gal"), ["b", "c", "a"]);
    assert_eq!(shown(&h, "w-card:"), ["b", "c", "a"], "the widget shows it");
    // Only the collection's member list was written: the pin stayed put.
    assert_eq!(
        mutations(&vault),
        [(
            "set_field".to_string(),
            "c-gal".to_string(),
            "collectionIds".to_string()
        )]
    );
    assert_eq!(
        h.state().store.space("sp-1").unwrap().pinned_ids,
        pins_before,
        "dragging an item never moves its pin"
    );
    assert!(!h.has("w-ghost:c-gal"));
    assert_ne!(
        *h.state().nav.route(),
        Route::Object("a".into()),
        "a drag opens nothing"
    );
}

#[test]
fn a_list_row_dragged_up_reorders_the_collection() {
    let vault = vault();
    let mut h = desktop(&vault);
    assert_eq!(shown(&h, "w-row-"), ["x", "y", "z", "qb", "qa"]);
    let x = h.rect("w-row-x").unwrap();
    let from = center(&h, "w-row-z");
    let to = [from[0], x[1] + 4.];
    press_and_move(&mut h, from, to, 6);
    assert!(h.has("w-drop:x"), "the drop line marks before X");
    release(&mut h, to);
    assert_eq!(members(&vault, "c-list"), ["z", "x", "y"]);
    let list: Vec<String> = shown(&h, "w-row-")
        .into_iter()
        .filter(|k| ["x", "y", "z"].contains(&k.as_str()))
        .collect();
    assert_eq!(list, ["z", "x", "y"]);
    assert!(
        mutations(&vault)
            .iter()
            .all(|(op, id, _)| op == "set_field" && id == "c-list")
    );
}

#[test]
fn a_query_pins_items_never_lift_and_open_on_a_click() {
    let vault = vault();
    let mut h = desktop(&vault);
    let from = center(&h, "w-row-qa");
    let to = [from[0], h.rect("w-row-qb").unwrap()[1] + 2.];
    press_and_move(&mut h, from, to, 6);
    assert!(!h.has("w-ghost:q-1"), "no ghost");
    assert!(h.keys("w-drop:").is_empty(), "no drop line");
    release(&mut h, to);
    assert!(mutations(&vault).is_empty(), "nothing written");
    assert_eq!(h.state().store.space("sp-1").unwrap().pinned_ids, PINS);
    h.click("w-row-qa");
    assert_eq!(*h.state().nav.route(), Route::Object("qa".into()));
}

#[test]
fn a_collection_item_still_opens_on_a_click() {
    let vault = vault();
    let mut h = desktop(&vault);
    h.click("w-card:b");
    assert_eq!(*h.state().nav.route(), Route::Object("b".into()));
    h.click("w-row-y");
    assert_eq!(*h.state().nav.route(), Route::Object("y".into()));
    assert!(mutations(&vault).is_empty());
}

#[test]
fn dropping_in_place_writes_nothing() {
    let vault = vault();
    let mut h = desktop(&vault);
    h.update(|s| navigate(Route::Object("c".into()), s));
    // Over itself.
    let a = center(&h, "w-card:a");
    press_and_move(&mut h, a, [a[0] + 12., a[1]], 3);
    assert!(h.has("w-ghost:c-gal"), "it lifted");
    release(&mut h, [a[0] + 12., a[1]]);
    // Before its own next card: the same order.
    let b = h.rect("w-card:b").unwrap();
    let to = [b[0] + b[2] * 0.25, b[1] + b[3] / 2.];
    press_and_move(&mut h, a, to, 6);
    assert!(h.has("w-drop:b"));
    release(&mut h, to);
    assert!(mutations(&vault).is_empty(), "nothing written");
    assert_eq!(members(&vault, "c-gal"), ["a", "b", "c"]);
    assert_eq!(shown(&h, "w-card:"), ["a", "b", "c"]);
    assert_eq!(
        *h.state().nav.route(),
        Route::Object("c".into()),
        "and a drag let go on itself opens nothing"
    );
}

#[test]
fn on_a_phone_a_held_item_lifts_and_reorders() {
    let vault = vault();
    let mut h = Harness::new(Arc::new(Shared(vault.clone())), (390., 844.));
    settle(&mut h);
    h.update(|s| navigate(Route::Home, s));
    h.click("m-card-sp-1");
    settle(&mut h);
    assert!(h.has("m-space-home"));
    let x = h.rect("w-row-x").expect("the list collection's rows");
    let from = center(&h, "w-row-z");
    let to = [from[0], x[1] + 4.];

    // Moved before the hold: the space home scrolls, nothing lifts.
    press_and_move(&mut h, from, to, 4);
    assert!(!h.has("w-ghost:c-list"));
    release(&mut h, to);
    assert!(mutations(&vault).is_empty());
    assert!(h.has("m-space-home"), "a scroll opens nothing");

    // Held still, then moved: it lifts and drops before X.
    let from = center(&h, "w-row-z");
    let x = h.rect("w-row-x").unwrap();
    let to = [from[0], x[1] + 4.];
    h.rt.pointer_move(from[0], from[1]);
    h.rt.pointer_down(from[0], from[1]);
    h.frame();
    h.advance(Duration::from_millis(600));
    assert!(h.has("w-ghost:c-list"), "the long press lifts it");
    h.rt.pointer_move(to[0], to[1]);
    h.frame();
    assert!(h.has("w-drop:x"));
    release(&mut h, to);
    assert_eq!(members(&vault, "c-list"), ["z", "x", "y"]);
    assert!(
        h.has("m-space-home"),
        "a lift is a reorder, never navigation"
    );
}

#[test]
fn a_pin_dragged_by_its_name_moves_the_pin_and_opens_nothing() {
    let vault = vault();
    let mut h = desktop(&vault);
    let route = h.state().nav.route().clone();
    let from = center(&h, "widget-head:q-1");
    let target = h.rect("widget:c-gal").expect("the first pin");
    let to = [from[0], target[1] + 4.];
    press_and_move(&mut h, from, to, 8);
    release(&mut h, to);
    // The pin's new place is written as its pin order (the fake vault's space keeps its own).
    assert_eq!(
        mutations(&vault),
        [("set_field".to_string(), "q-1".to_string(), "pin_order".to_string())]
    );
    assert_eq!(*h.state().nav.route(), route, "the drag opened nothing");
}

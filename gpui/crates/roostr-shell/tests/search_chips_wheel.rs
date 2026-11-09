//! The search modal's type chips overflow a desktop window; a plain
//! (vertical) mouse wheel over the chip row scrolls it sideways.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::sync::Arc;

use anyhow::Result;
use parking_lot::Mutex;
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream};
use rune::{GesturePhase, WheelEvent};
use serde_json::Value as Json;

use common::Harness;

const SPACE: &str = "sp-1";
const TYPES: usize = 30;

/// One space with [`TYPES`] long-named types.
struct Vault {
    objects: Mutex<Vec<Object>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn object(id: &str, type_key: &str, pairs: &[(&str, Value)]) -> Object {
    let mut fields: Fields = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    fields.insert("channel".into(), Value::string(SPACE));
    Object {
        id: id.into(),
        type_key: type_key.into(),
        fields,
        ..Object::default()
    }
}

impl Default for Vault {
    fn default() -> Self {
        let objects = (0..TYPES)
            .map(|n| {
                object(
                    &format!("type-{n}"),
                    "type",
                    &[
                        ("name", Value::string(format!("Long type name {n}"))),
                        ("key", Value::string(format!("kind{n}"))),
                    ],
                )
            })
            .collect();
        Self {
            objects: Mutex::new(objects),
        }
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "a space with many types".into()
    }
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        ready(Ok(self
            .objects
            .lock()
            .iter()
            .map(|o| ObjectSummary {
                id: o.id.clone(),
                type_key: o.type_key.clone(),
                name: o.field_str("name").into(),
                channel_id: SPACE.into(),
                updated_at: 1_000,
                ..ObjectSummary::default()
            })
            .collect()))
    }
    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        let object = self.objects.lock().iter().find(|o| o.id == id).cloned();
        ready(object.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
    }
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: SPACE.into(),
            name: "Personal".into(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    /// The type definitions; nothing else is listed here.
    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>> {
        if body["filters"][0]["key"] != "typeKey" {
            return ready(Ok(QueryResult::default()));
        }
        let records: Vec<QueryRow> = self
            .objects
            .lock()
            .iter()
            .filter(|o| o.type_key == "type")
            .map(|o| QueryRow {
                id: o.id.clone(),
                type_key: o.type_key.clone(),
                name: Some(o.field_str("name").into()),
                fields: o.fields.clone(),
                ..QueryRow::default()
            })
            .collect();
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
        Box::pin(futures::stream::pending())
    }
}

/// One wheel gesture of `dy` at (x, y): began, one move, ended.
fn wheel(h: &mut Harness, (x, y): (f32, f32), dy: f32) {
    h.rt.pointer_move(x, y);
    for (phase, dy) in [
        (GesturePhase::Began, 0.),
        (GesturePhase::Moved, dy),
        (GesturePhase::Ended, 0.),
    ] {
        h.rt.wheel_event(WheelEvent::new(0., dy).at(x, y).phase(phase));
        h.frame();
    }
    h.settle();
}

fn center(r: [f32; 4]) -> (f32, f32) {
    (r[0] + r[2] / 2., r[1] + r[3] / 2.)
}

#[test]
fn a_vertical_wheel_scrolls_the_search_chips_sideways() {
    let mut h = Harness::new(Arc::new(Vault::default()), (1280., 900.));
    h.settle();

    h.update(roostr_shell::search);
    h.settle();
    let modal = h.rect("search-modal").expect("the search modal");
    let row = h.rect("search-chips").expect("the chip row");
    let last = h.rect("chip-kind29").expect("the last chip");
    assert!(
        row[0] + row[2] <= modal[0] + modal[2] + 0.5,
        "the chip row stays inside the modal: row {row:?}, modal {modal:?}"
    );
    assert!(
        last[0] > row[0] + row[2],
        "the chips overflow the row: last {last:?}, row {row:?}"
    );
    let first = h.rect("chip-kind0").expect("the first chip")[0];

    // Wheel down (content moves up) over the row: it scrolls sideways.
    wheel(&mut h, center(row), -400.);
    let moved = h.rect("chip-kind0").map(|b| b[0]);
    assert!(
        moved.is_none_or(|x| x < first - 100.),
        "the chips scrolled left: {first:?} -> {moved:?}"
    );
    let last_now = h.rect("chip-kind29").expect("the last chip");
    assert!(
        last_now[0] < last[0] - 100.,
        "the last chip came closer: {last:?} -> {last_now:?}"
    );
}

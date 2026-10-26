//! The search modal's type chips overflow a desktop window; a plain
//! (vertical) mouse wheel over the chip row scrolls it sideways.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Modifiers, Pixels, Point, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase,
    VisualTestContext, point,
};
use gpui_kit::{px, size};
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use serde_json::Value as Json;

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
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
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
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let object = self.objects.lock().iter().find(|o| o.id == id).cloned();
        ready(object.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: SPACE.into(),
            name: "Personal".into(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    /// The type definitions; nothing else is listed here.
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
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
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn wheel(cx: &mut VisualTestContext, at: Point<Pixels>, dy: f32) {
    for (phase, dy) in [
        (TouchPhase::Started, 0.),
        (TouchPhase::Moved, dy),
        (TouchPhase::Ended, 0.),
    ] {
        cx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            modifiers: Modifiers::none(),
            touch_phase: phase,
        });
        cx.run_until_parked();
    }
    settle(cx);
}

#[gpui::test]
fn a_vertical_wheel_scrolls_the_search_chips_sideways(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    let backend = Arc::new(Vault::default());
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(backend.clone(), None, cx);
        roostr_app::init(cx);
    });
    let (_workspace, cx) = cx.add_window_view(|window, cx| {
        roostr_shell::Workspace::new(roostr_app::pages(), window, cx)
    });
    cx.simulate_resize(size(px(1280.), px(900.)));
    settle(cx);

    cx.dispatch_action(roostr_shell::Search);
    settle(cx);
    let modal = cx.debug_bounds("search-modal").expect("the search modal");
    let row = cx.debug_bounds("search-chips").expect("the chip row");
    let last = cx.debug_bounds("chip-kind29").expect("the last chip");
    assert!(
        row.right() <= modal.right() + px(0.5),
        "the chip row stays inside the modal: row {row:?}, modal {modal:?}"
    );
    assert!(
        last.left() > row.right(),
        "the chips overflow the row: last {last:?}, row {row:?}"
    );
    let first = cx
        .debug_bounds("chip-kind0")
        .expect("the first chip")
        .left();

    // Wheel down (content moves up) over the row: it scrolls sideways.
    wheel(cx, row.center(), -400.);
    let moved = cx.debug_bounds("chip-kind0").map(|b| b.left());
    assert!(
        moved.is_none_or(|x| x < first - px(100.)),
        "the chips scrolled left: {first:?} -> {moved:?}"
    );
    let last_now = cx.debug_bounds("chip-kind29").expect("the last chip");
    assert!(
        last_now.left() < last.left() - px(100.),
        "the last chip came closer: {last:?} -> {last_now:?}"
    );
}

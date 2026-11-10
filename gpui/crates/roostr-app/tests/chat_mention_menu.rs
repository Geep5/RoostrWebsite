//! The chat's `@` menu opens just above the composer. Field report: on the
//! Rune app it opened at the window's top-left corner - the overlay named its
//! anchor by a path, and an anchor is found by the element's own key.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::sync::Arc;

use roostr_core::nav::Route;
use roostr_core::types::{Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream, boxed, boxed_stream};
use serde_json::Value as Json;
use support::App;

const SPACE: &str = "sp-1";
const PAGE: &str = "page-1";

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    boxed(std::future::ready(value))
}

fn object(id: &str, type_key: &str, name: &str) -> Object {
    let mut fields = Fields::default();
    fields.insert("name".into(), Value::string(name));
    fields.insert("channel".into(), Value::string(SPACE));
    Object { id: id.into(), type_key: type_key.into(), fields, ..Object::default() }
}

/// A page and two agents in one space.
struct Vault {
    objects: Vec<Object>,
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "a page and two agents".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings { has_key: true, ..Settings::default() }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(self
            .objects
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
        let found = self.objects.iter().find(|o| o.id == id).cloned();
        ready(found.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space { id: SPACE.into(), name: "Personal".into(), created_at: 1, ..Space::default() }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let text = body.to_string();
        let records: Vec<QueryRow> = self
            .objects
            .iter()
            .filter(|o| o.type_key == "agent" && text.contains("\"agent\""))
            .map(|o| QueryRow {
                id: o.id.clone(),
                type_key: o.type_key.clone(),
                name: Some(o.field_str("name").into()),
                fields: o.fields.clone(),
                ..QueryRow::default()
            })
            .collect();
        ready(Ok(QueryResult { total: records.len() as u64, records }))
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
        boxed_stream(futures::stream::pending())
    }
}

#[test]
fn the_mention_menu_opens_above_the_composer() {
    let vault = Vault {
        objects: vec![
            object(PAGE, "page", "Supercell Overview"),
            object("agent-1", "agent", "Brian Agent"),
            object("agent-2", "agent", "Don Agent"),
        ],
    };
    let mut app = App::open(Arc::new(vault), 1568., 934.);
    app.navigate(Route::Object(PAGE.into()));
    app.tap("composer");
    app.type_text("@");
    app.settle();

    let keys: Vec<String> = app.rt.elements().into_iter().filter_map(|e| e.key).collect();
    let box_key = keys.iter().find(|k| k.ends_with("composer-box")).expect("the composer box").clone();
    let row_key = keys.iter().find(|k| k.starts_with("mention-")).unwrap_or_else(|| panic!("no mention menu; keys: {keys:?}")).clone();
    let composer = app.rect(&box_key).expect("the composer box is drawn");
    let row = app.rect(&row_key).expect("a mention row is drawn");

    // Above the composer and lined up with it - not at the window's corner.
    assert!(row[1] + row[3] <= composer[1] + 1., "the menu's row {row:?} is not above the composer {composer:?}");
    assert!(composer[1] - (row[1] + row[3]) < 120., "the menu {row:?} is far from the composer {composer:?}");
    assert!((row[0] - composer[0]).abs() < 40., "the menu {row:?} doesn't start at the composer's left edge {composer:?}");
}

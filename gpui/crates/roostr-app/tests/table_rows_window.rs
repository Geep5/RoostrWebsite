//! A big query's table builds only the rows in and near the window. Field
//! report: scrolling a 187-email query lagged - the page scrolls the table,
//! and each frame built every loaded row (a row of cells each). Rows far
//! below aren't built until they come near; spacers keep the table's height.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{px, size};
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use serde_json::{Value as Json, json};

const SPACE: &str = "sp-1";
const ROWS: usize = 90;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn object(id: &str, type_key: &str, pairs: &[(&str, Value)]) -> Object {
    let mut fields: Fields = pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
    fields.insert("channel".into(), Value::string(SPACE));
    Object { id: id.into(), type_key: type_key.into(), fields, ..Object::default() }
}

/// A space of `ROWS` emails and a query of them showing their sender.
struct Vault {
    objects: Vec<Object>,
}

impl Default for Vault {
    fn default() -> Self {
        let mut objects = vec![
            object("type-email", "type", &[("name", Value::string("Email")), ("key", Value::string("email"))]),
            object("q1", "query", &[("name", Value::string("Support Emails")), ("setOf", Value::values(vec![Value::string("email")])),
                ("viewRelations", serde_json::from_value(json!({"valuesValue":{"items":[{"mapValue":{"entries":{"key":{"stringValue":"email_from"},"width":{"intValue":150}}}}]}})).unwrap())]),
        ];
        objects.extend((0..ROWS).map(|i| {
            object(&format!("e{i}"), "email", &[("name", Value::string(&format!("Email {i}"))), ("email_from", Value::string(&format!("p{i}@example.com")))])
        }));
        Self { objects }
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "a big query".into()
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
                channel_id: o.field_str("channel").into(),
                updated_at: 1_000,
                ..ObjectSummary::default()
            })
            .collect()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let object = self.objects.iter().find(|o| o.id == id).cloned();
        ready(object.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space { id: SPACE.into(), name: "Personal".into(), created_at: 1, ..Space::default() }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(vec![RelationDef {
            id: "rel-from".into(),
            key: "email_from".into(),
            format: "shorttext".into(),
            name: "From".into(),
            space: Some(SPACE.into()),
            ..RelationDef::default()
        }]))
    }
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let text = body.to_string();
        let records: Vec<QueryRow> = self
            .objects
            .iter()
            .filter(|o| {
                (text.contains("\"setId\":\"q1\"") && o.type_key == "email")
                    || (!text.contains("setId") && text.contains(&format!("\"{}\"", o.type_key)) && o.type_key != "query")
            })
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
        Box::pin(futures::stream::pending())
    }
}

fn settle(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
}

const FIRST: &str = "cell-e0-email_from";
const LAST: &str = "cell-e89-email_from"; // ROWS - 1

fn drawn(cx: &mut VisualTestContext, cell: &'static str) -> bool {
    cx.debug_bounds(cell).is_some()
}

#[gpui::test]
fn a_big_querys_table_draws_only_the_rows_near_the_window(cx: &mut TestAppContext) {
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
    cx.simulate_resize(size(px(1400.), px(600.)));
    settle(cx);
    cx.update(|_, cx| navigate(Route::Object("q1".into()), cx));
    settle(cx);
    settle(cx);

    assert!(drawn(cx, FIRST), "the first row");
    assert!(!drawn(cx, LAST), "the last row, ~2900px down, isn't built");
}

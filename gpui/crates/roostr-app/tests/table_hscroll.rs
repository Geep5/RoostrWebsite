//! A table wider than its pane shows a horizontal scrollbar along its bottom;
//! one that fits shows none.
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
const ROWS: usize = 3;
/// Twelve 200px columns: wider than a 1000px window, narrower than a 4000px one.
const COLS: usize = 12;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn object(id: &str, type_key: &str, pairs: &[(&str, Value)]) -> Object {
    let mut fields: Fields = pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
    fields.insert("channel".into(), Value::string(SPACE));
    Object { id: id.into(), type_key: type_key.into(), fields, ..Object::default() }
}

/// A space of `ROWS` emails and a query of them showing `COLS` properties.
struct Vault {
    objects: Vec<Object>,
}

impl Default for Vault {
    fn default() -> Self {
        let mut objects = vec![
            object("type-email", "type", &[("name", Value::string("Email")), ("key", Value::string("email"))]),
            object("q1", "query", &[("name", Value::string("Support Emails")), ("setOf", Value::values(vec![Value::string("email")])),
                ("viewRelations", serde_json::from_value(json!({"valuesValue":{"items":(0..COLS).map(|c| json!({"mapValue":{"entries":{"key":{"stringValue":format!("p{c}")},"width":{"intValue":200}}}})).collect::<Vec<_>>()}})).unwrap())]),
        ];
        objects.extend((0..ROWS).map(|i| {
            {
                let keys: Vec<String> = (0..COLS).map(|c| format!("p{c}")).collect();
                let mut pairs: Vec<(&str, Value)> = vec![("name", Value::string(&format!("Email {i}")))];
                pairs.extend(keys.iter().map(|k| (k.as_str(), Value::string("value"))));
                object(&format!("e{i}"), "email", &pairs)
            }
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
        ready(Ok((0..COLS)
            .map(|c| RelationDef {
                id: format!("rel-p{c}"),
                key: format!("p{c}"),
                format: "shorttext".into(),
                name: format!("Property {c}"),
                space: Some(SPACE.into()),
                ..RelationDef::default()
            })
            .collect()))
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

fn scrollbar(cx: &mut VisualTestContext) -> bool {
    cx.debug_bounds("set-table-scrollbar").is_some()
}

#[gpui::test]
fn a_table_wider_than_its_pane_shows_a_horizontal_scrollbar(cx: &mut TestAppContext) {
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
    cx.simulate_resize(size(px(4000.), px(700.)));
    settle(cx);
    cx.update(|_, cx| navigate(Route::Object("q1".into()), cx));
    settle(cx);
    settle(cx);
    assert!(cx.debug_bounds("cell-e0-p0").is_some(), "the table is drawn");
    assert!(!scrollbar(cx), "a table that fits has no scrollbar");

    cx.simulate_resize(size(px(1000.), px(700.)));
    settle(cx);
    settle(cx);
    assert!(scrollbar(cx), "a table wider than its pane shows one");
    // Along the bottom, under the last row rather than over it.
    let bar = cx.debug_bounds("set-table-scrollbar").unwrap();
    let last = cx.debug_bounds("cell-e2-p0").expect("the last row");
    assert!(bar.bottom() >= last.bottom(), "the scrollbar area ends below the last row");

    // The wheel (a plain vertical turn) over the scrollbar scrolls the table
    // sideways; the same turn over the rows does not.
    let col_x = |cx: &mut VisualTestContext| cx.debug_bounds("cell-e0-p3").expect("a cell").left();
    let before = col_x(cx);
    let wheel = |cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>| {
        cx.simulate_event(gpui::ScrollEvent {
            position: at,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-120.))),
            ..Default::default()
        });
        cx.run_until_parked();
    };
    wheel(cx, last.center());
    assert_eq!(col_x(cx), before, "a wheel over the rows leaves the table where it is");
    wheel(cx, gpui::point(bar.center().x, bar.bottom() - px(4.)));
    assert!(col_x(cx) < before, "a wheel over the scrollbar moves the columns left");

    cx.simulate_resize(size(px(4000.), px(700.)));
    settle(cx);
    settle(cx);
    assert!(!scrollbar(cx), "and it goes when the table fits again");
}

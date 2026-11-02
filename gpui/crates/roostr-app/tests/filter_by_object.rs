//! A query filters on an object property by which objects it holds. Field
//! report: filtering by Agent offered only "is empty" / "is not empty", so
//! there was no way to list what one agent is on. An object property now has
//! "has any of" (and all of / none of) with a picker of the objects it may
//! hold; picking one saves its id in the filter, which the engine matches.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{Modifiers, TestAppContext, VisualTestContext};
use gpui_kit::{px, size};
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use serde_json::{Value as Json, json};

const SPACE: &str = "sp-1";

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn object(id: &str, type_key: &str, pairs: &[(&str, Value)]) -> Object {
    let mut fields: Fields = pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
    fields.insert("channel".into(), Value::string(SPACE));
    Object { id: id.into(), type_key: type_key.into(), fields, ..Object::default() }
}

/// Two agents, a task on the designer, and a query filtering on Agent.
struct Vault {
    objects: Mutex<Vec<Object>>,
}

impl Default for Vault {
    fn default() -> Self {
        let filter: Value = serde_json::from_value(json!({"valuesValue": {"items": [{"mapValue": {"entries": {
            "key": {"stringValue": "agent"}, "condition": {"stringValue": "in"}, "value": {"valuesValue": {"items": []}}
        }}}]}}))
        .unwrap();
        Self {
            objects: Mutex::new(vec![
                object("type-agent", "type", &[("name", Value::string("Agent")), ("key", Value::string("agent"))]),
                object("type-task", "type", &[("name", Value::string("Task")), ("key", Value::string("task"))]),
                object("agent-1", "agent", &[("name", Value::string("Mockup Designer"))]),
                object("agent-2", "agent", &[("name", Value::string("Inbox Triage"))]),
                object("t1", "task", &[("name", Value::string("Landing mockup")), ("agent", Value::values(vec![Value::link("agent-1", "agent")]))]),
                object("q1", "query", &[("name", Value::string("Uses designer")), ("viewFilters", filter)]),
            ]),
        }
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "agents and a query".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings { has_key: true, ..Settings::default() }))
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
                channel_id: o.field_str("channel").into(),
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
        ready(Ok(vec![Space { id: SPACE.into(), name: "Personal".into(), created_at: 1, ..Space::default() }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(vec![
            RelationDef {
                id: "rel-agent".into(),
                key: "agent".into(),
                format: "object".into(),
                name: "Agent".into(),
                object_types: Some(vec!["type-agent".into()]),
                space: Some(SPACE.into()),
                ..RelationDef::default()
            },
            RelationDef {
                id: "rel-repeat-instructions".into(),
                key: "repeat_instructions".into(),
                format: "longtext".into(),
                name: "Repeat instructions".into(),
                space: Some(SPACE.into()),
                ..RelationDef::default()
            },
        ]))
    }
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let text = body.to_string();
        let records: Vec<QueryRow> = self
            .objects
            .lock()
            .iter()
            .filter(|o| text.contains(&format!("\"{}\"", o.type_key)) || text.contains("setId"))
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
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        if action == "set_field" {
            let id = params["object_id"].as_str().unwrap_or_default().to_string();
            let key = params["key"].as_str().unwrap_or_default().to_string();
            let value: Value = serde_json::from_value(params["value"].clone()).expect("value");
            if let Some(o) = self.objects.lock().iter_mut().find(|o| o.id == id) {
                o.fields.insert(key, value);
            }
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
        Box::pin(futures::stream::pending())
    }
}

fn settle(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is not on screen")).center();
    cx.simulate_click(at, Modifiers::none());
    settle(cx);
}

#[gpui::test]
fn a_query_filters_by_the_agent_an_object_has(cx: &mut TestAppContext) {
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
    cx.simulate_resize(size(px(1400.), px(900.)));
    settle(cx);
    cx.update(|_, cx| navigate(Route::Object("q1".into()), cx));
    settle(cx);

    // The first press on the page opens its side columns; the second the panel.
    click(cx, "dv-filter");
    if cx.debug_bounds("fobj-add-agent-1").is_none() {
        click(cx, "dv-filter");
    }
    // "has any of" offers the property's objects: the agents, not the task.
    assert!(cx.debug_bounds("fobj-add-agent-1").is_some(), "Mockup Designer is offered");
    assert!(cx.debug_bounds("fobj-add-agent-2").is_some(), "Inbox Triage is offered");
    assert!(cx.debug_bounds("fobj-add-t1").is_none(), "a task is not an agent");

    click(cx, "fobj-add-agent-1");
    let saved = backend
        .objects
        .lock()
        .iter()
        .find(|o| o.id == "q1")
        .and_then(|o| o.fields.get("viewFilters").cloned())
        .map(|v| serde_json::to_string(&v).unwrap())
        .unwrap_or_default();
    assert!(saved.contains("\"in\"") && saved.contains("agent-1"), "the filter holds the agent's id: {saved}");
    assert!(cx.debug_bounds("fobj-on-agent-1").is_some(), "the picked agent shows under Selected");
    assert!(cx.debug_bounds("fobj-add-agent-1").is_none(), "and is no longer offered");

    // A query repeats like a task: its Properties offer Repeat (its run
    // hands the agent what it holds).
    assert!(cx.debug_bounds("prop-row-__repeat__").is_some(), "Repeat on a query");

    // Repeat instructions - what each run asks of the agent - show once it
    // repeats, right under Repeat.
    assert!(cx.debug_bounds("prop-row-repeat_instructions").is_none(), "not before it repeats");
    // Picking a Repeat (its editor open) shows them already.
    click(cx, "prop-row-__repeat__");
    assert!(cx.debug_bounds("prop-row-repeat_instructions").is_some(), "shown while Repeat is being set");
    let rule: Value = serde_json::from_value(json!({"mapValue": {"entries": {
        "freq": {"stringValue": "day"}, "interval": {"intValue": 1}, "anchor": {"intValue": 0}, "next": {"intValue": 4_102_444_800_000_i64},
        "times": {"valuesValue": {"items": [{"intValue": 540}]}}
    }}}))
    .unwrap();
    if let Some(q) = backend.objects.lock().iter_mut().find(|o| o.id == "q1") {
        q.fields.insert("repeat".into(), rule);
    }
    cx.update(|_, cx| roostr_core::Store::global(cx).update(cx, |s, cx| s.reload_object("q1", cx)));
    settle(cx);
    let repeat = cx.debug_bounds("prop-row-__repeat__").expect("Repeat");
    let instructions = cx.debug_bounds("prop-row-repeat_instructions").expect("Repeat instructions once it repeats");
    assert!(instructions.top() >= repeat.bottom() && instructions.top() - repeat.bottom() < px(40.), "{instructions:?} under {repeat:?}");
}

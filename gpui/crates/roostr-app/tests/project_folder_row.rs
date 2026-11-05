//! An object with an agent shows its Project folder row by default - where
//! its agents work on it - and an object with no agent doesn't.
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
use serde_json::Value as Json;

const SPACE: &str = "sp-1";

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn object(id: &str, type_key: &str, pairs: &[(&str, Value)]) -> Object {
    let mut fields: Fields = pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
    fields.insert("channel".into(), Value::string(SPACE));
    Object { id: id.into(), type_key: type_key.into(), fields, ..Object::default() }
}

/// An agent, a task it is on, and a task with no agent.
struct Vault {
    objects: Mutex<Vec<Object>>,
    /// repeat_set calls' rules.
    rules: Mutex<Vec<Json>>,
}

impl Default for Vault {
    fn default() -> Self {
        Self {
            objects: Mutex::new(vec![
                object("type-agent", "type", &[("name", Value::string("Agent")), ("key", Value::string("agent"))]),
                object("type-task", "type", &[("name", Value::string("Task")), ("key", Value::string("task")), ("layout", Value::string("task"))]),
                object("agent-1", "agent", &[("name", Value::string("Mockup Designer"))]),
                object("t1", "task", &[("name", Value::string("Landing mockup")), ("agent", Value::values(vec![Value::link("agent-1", "agent")]))]),
                object("t2", "task", &[("name", Value::string("Grocery list"))]),
            ]),
            rules: Mutex::new(Vec::new()),
        }
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "an agent and two tasks".into()
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
                id: "rel-repo-path".into(),
                key: "repo_path".into(),
                format: "shorttext".into(),
                name: "Project folder".into(),
                hidden: true,
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
        if action == "repeat_set" {
            self.rules.lock().push(params["rule"].clone());
        }
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
    // The pane columns slide open: their next frame draws them.
    cx.simulate_frame();
}

/// The Properties column, opened from its strip when it starts collapsed.
fn open_properties(cx: &mut VisualTestContext) {
    if cx.debug_bounds("pane-col-Properties").is_none()
        && let Some(strip) = cx.debug_bounds("pane-strip-Properties")
    {
        cx.simulate_click(strip.center(), Modifiers::none());
        settle(cx);
    }
    assert!(cx.debug_bounds("pane-col-Properties").is_some(), "the Properties column is open");
}

#[gpui::test]
fn an_object_with_an_agent_shows_its_project_folder(cx: &mut TestAppContext) {
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

    cx.update(|_, cx| navigate(Route::Object("t1".into()), cx));
    settle(cx);
    settle(cx);
    open_properties(cx);
    assert!(cx.debug_bounds("prop-row-repo_path").is_some(), "a task with an agent shows its Project folder");

    cx.update(|_, cx| navigate(Route::Object("t2".into()), cx));
    settle(cx);
    settle(cx);
    open_properties(cx);
    assert!(cx.debug_bounds("prop-row-repo_path").is_none(), "a task with no agent doesn't");
}

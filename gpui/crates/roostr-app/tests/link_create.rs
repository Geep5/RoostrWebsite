//! A property's link picker makes the object its query names. A Human's
//! `tasks` relation is limited to the Task type; typing a name no task has
//! offers "＋ Create Task “…”", which creates that task in the Human's space
//! and links it. An existing task's exact name offers no create.
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
    field_str,
};
use roostr_core::{Backend, BackendEvent, JsonMap, Store};
use serde_json::{Value as Json, json};

const SPACE: &str = "sp-1";
const HUMAN: &str = "human-1";
const TASK: &str = "task-1";
const TASK_TYPE: &str = "type-task";

/// A `create` the backend was asked for: (name, type key, fields).
type Create = (String, String, Fields);

/// One space: the Task and Human types, a task "Write docs", and a Human
/// whose (empty) `tasks` relation is limited to tasks.
struct Vault {
    objects: Mutex<Vec<Object>>,
    creates: Mutex<Vec<Create>>,
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
        let objects = vec![
            object(
                TASK_TYPE,
                "type",
                &[
                    ("name", Value::string("Task")),
                    ("key", Value::string("task")),
                    ("layout", Value::string("task")),
                ],
            ),
            object(
                "type-human",
                "type",
                &[
                    ("name", Value::string("Human")),
                    ("key", Value::string("human")),
                ],
            ),
            object(TASK, "task", &[("name", Value::string("Write docs"))]),
            object(
                HUMAN,
                "human",
                &[
                    ("name", Value::string("Ada")),
                    ("tasks", Value::values(Vec::new())),
                ],
            ),
        ];
        Self {
            objects: Mutex::new(objects),
            creates: Mutex::default(),
        }
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "a human, a task, and their types".into()
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
        ready(Ok(vec![RelationDef {
            id: "rel-tasks".into(),
            key: "tasks".into(),
            format: "object".into(),
            name: "Tasks".into(),
            object_types: Some(vec![TASK_TYPE.into()]),
            space: Some(SPACE.into()),
            ..RelationDef::default()
        }]))
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
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        let text = |key: &str| params[key].as_str().unwrap_or_default().to_string();
        let mut result = JsonMap::new();
        match action {
            "create" => {
                let fields: Fields = params
                    .get("fields")
                    .map(|f| serde_json::from_value(f.clone()).expect("create fields"))
                    .unwrap_or_default();
                let mut objects = self.objects.lock();
                let id = format!("task-new-{}", objects.len());
                let mut made = Object {
                    id: id.clone(),
                    type_key: text("type_key"),
                    fields: fields.clone(),
                    ..Object::default()
                };
                made.fields
                    .insert("name".into(), Value::string(text("name")));
                objects.push(made);
                self.creates
                    .lock()
                    .push((text("name"), text("type_key"), fields));
                result.insert("id".into(), json!(id));
            }
            "set_field" => {
                let value: Value =
                    serde_json::from_value(params["value"].clone()).expect("set_field value");
                let id = text("object_id");
                if let Some(o) = self.objects.lock().iter_mut().find(|o| o.id == id) {
                    o.fields.insert(text("key"), value);
                }
            }
            _ => {}
        }
        ready(Ok(result))
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

fn shown(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn tap(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
}

fn type_query(cx: &mut VisualTestContext, text: &str) {
    cx.simulate_input(text);
    cx.run_until_parked();
}

fn clear_query(cx: &mut VisualTestContext, len: usize) {
    for _ in 0..len {
        cx.simulate_keystrokes("backspace");
    }
    cx.run_until_parked();
}

/// The ids a link value points at.
fn linked(value: Option<&Value>) -> Vec<String> {
    value
        .map(|v| {
            v.items()
                .iter()
                .filter_map(|i| i.link_value.as_ref().map(|l| l.target_id.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The Human's `tasks` as the backend holds them and as the app shows them.
fn human_tasks(vault: &Vault, cx: &mut VisualTestContext) -> (Vec<String>, Vec<String>) {
    let stored = linked(
        vault
            .objects
            .lock()
            .iter()
            .find(|o| o.id == HUMAN)
            .and_then(|o| o.field("tasks")),
    );
    let shown = cx.update(|_, cx| {
        let object = Store::global(cx).read(cx).object(HUMAN).expect("the human");
        linked(object.field("tasks"))
    });
    (stored, shown)
}

fn creates(vault: &Vault) -> Vec<(String, String, String)> {
    vault
        .creates
        .lock()
        .iter()
        .map(|(name, type_key, fields)| {
            (
                name.clone(),
                type_key.clone(),
                field_str(fields, "channel").to_string(),
            )
        })
        .collect()
}

fn create(name: &str) -> (String, String, String) {
    (name.into(), "task".into(), SPACE.into())
}

#[gpui::test]
fn a_typed_name_in_a_task_link_picker_creates_and_links_the_task(cx: &mut TestAppContext) {
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
    cx.simulate_resize(size(px(1400.), px(900.)));
    settle(cx);

    // The Human's page; its right pane's Properties tab lists `tasks`.
    cx.update(|_, cx| navigate(Route::Object(HUMAN.into()), cx));
    settle(cx);
    tap(cx, "pane-tab-Properties");
    // The empty relation's editor opens straight on its link picker.
    tap(cx, "prop-row-tasks");
    assert!(shown(cx, "obj-menu"), "the link picker");
    assert!(shown(cx, "cand-task-1"), "the space's task is offered");

    // An existing task's exact name (any case): pick it, no create.
    type_query(cx, "write DOCS");
    assert!(shown(cx, "cand-task-1"));
    assert!(
        !shown(cx, "create-from-query"),
        "no create for an existing name"
    );
    clear_query(cx, "write DOCS".len());

    // A name no task has: the create row, and clicking it makes the task in
    // the Human's space and links it.
    type_query(cx, "test make");
    assert!(!shown(cx, "cand-task-1"));
    assert!(shown(cx, "create-from-query"), "the create row");
    tap(cx, "create-from-query");
    assert_eq!(creates(&backend), [create("test make")]);
    let first = "task-new-4".to_string();
    assert_eq!(
        human_tasks(&backend, cx),
        (vec![first.clone()], vec![first.clone()]),
        "the Human links the new task"
    );
    assert!(!shown(cx, "obj-menu"), "the picker closes");

    // Enter on a query with no candidates creates and links too.
    tap(cx, "link-object");
    assert!(shown(cx, "obj-menu"));
    type_query(cx, "another one");
    assert!(shown(cx, "create-from-query"));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        creates(&backend),
        [create("test make"), create("another one")]
    );
    let second = "task-new-5".to_string();
    assert_eq!(
        human_tasks(&backend, cx),
        (vec![first.clone(), second.clone()], vec![first, second]),
    );
}

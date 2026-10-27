//! Making an object of a type starts it from the type's default template, on
//! a Lite device too. Field report: a Meeting made with the nav pane's create
//! button on roostr.space came out without the template's agent - the Lite
//! copy did not hold the template, and its failed read was swallowed.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;

use gpui::TestAppContext;
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::lite::{LiteControl, VaultEstimate};
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use serde_json::Value as Json;

const SPACE: &str = "sp-1";
const TEMPLATE: &str = "tpl-meeting";
const AGENT: &str = "agent-1";

/// A Lite vault: the Meeting type is held, its default template only once fetched.
#[derive(Default)]
struct LiteVault {
    template_held: Mutex<bool>,
    set_fields: Mutex<Vec<(String, String)>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

struct Control(Arc<LiteVault>);

impl LiteControl for Control {
    fn type_counts(&self, _: &str) -> std::collections::HashMap<String, u64> {
        Default::default()
    }
    fn load_space(&self, _: &str) -> BoxFuture<anyhow::Result<()>> {
        ready(Ok(()))
    }
    fn fetch(&self, ids: Vec<String>) -> BoxFuture<anyhow::Result<()>> {
        if ids.iter().any(|id| id == TEMPLATE) {
            *self.0.template_held.lock() = true;
        }
        ready(Ok(()))
    }
    fn estimate(&self) -> BoxFuture<anyhow::Result<VaultEstimate>> {
        ready(Ok(VaultEstimate::default()))
    }
    fn sync_fully(&self) -> BoxFuture<anyhow::Result<()>> {
        ready(Ok(()))
    }
    fn switch_to_lite(&self) -> BoxFuture<anyhow::Result<()>> {
        ready(Ok(()))
    }
}

struct Handle(Arc<LiteVault>);

impl Backend for Handle {
    fn describe(&self) -> String {
        "a lite vault".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(Vec::new()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let field = |k: &str, v: Value| (k.to_string(), v);
        ready(match id {
            TEMPLATE if *self.0.template_held.lock() => Ok(Object {
                id: id.into(),
                type_key: "template".into(),
                fields: [
                    field("name", Value::string("Default Meeting")),
                    field("channel", Value::string(SPACE)),
                    field("agent", Value::values(vec![Value::link(AGENT, "agent")])),
                ]
                .into(),
                ..Object::default()
            }),
            TEMPLATE => Err(anyhow::anyhow!("{id} is not held on this device")),
            _ => Ok(Object {
                id: id.into(),
                type_key: "meeting".into(),
                fields: [field("channel", Value::string(SPACE))].into(),
                ..Object::default()
            }),
        })
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: SPACE.into(),
            name: "Matcherino".into(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let defs = body.to_string().contains("\"relation\"");
        let records = if defs {
            vec![QueryRow {
                id: "type-meeting".into(),
                type_key: "type".into(),
                fields: [
                    ("key".to_string(), Value::string("meeting")),
                    ("name".to_string(), Value::string("Meeting")),
                    ("channel".to_string(), Value::string(SPACE)),
                    ("default_template_id".to_string(), Value::string(TEMPLATE)),
                ]
                .into(),
                ..QueryRow::default()
            }]
        } else {
            Vec::new()
        };
        ready(Ok(QueryResult {
            total: records.len() as u64,
            records,
        }))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        let mut out = JsonMap::new();
        match action {
            "create" => {
                out.insert("id".into(), Json::String("new-meeting".into()));
            }
            "set_field" => self.0.set_fields.lock().push((
                params["object_id"].as_str().unwrap_or_default().into(),
                params["key"].as_str().unwrap_or_default().into(),
            )),
            _ => {}
        }
        ready(Ok(out))
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
    fn lite(&self) -> Option<Arc<dyn LiteControl>> {
        Some(Arc::new(Control(self.0.clone())))
    }
}

#[gpui::test]
fn a_new_object_on_a_lite_device_gets_its_types_default_template(cx: &mut TestAppContext) {
    let vault = Arc::new(LiteVault::default());
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(Arc::new(Handle(vault.clone())), None, cx);
    });
    cx.run_until_parked();
    let made = Arc::new(Mutex::new(None));
    let slot = made.clone();
    cx.update(|cx| {
        let create = roostr_core::create::create_typed("meeting", SPACE, "Meeting with Pedro", cx);
        cx.spawn(async move |_| *slot.lock() = Some(create.await.map_err(|e| e.to_string())))
            .detach();
    });
    cx.run_until_parked();
    let id = made
        .lock()
        .take()
        .expect("create finished")
        .expect("created");
    assert_eq!(id, "new-meeting");
    assert!(
        vault
            .set_fields
            .lock()
            .contains(&("new-meeting".into(), "agent".into())),
        "the template's agent is on the new meeting: {:?}",
        vault.set_fields.lock()
    );
}

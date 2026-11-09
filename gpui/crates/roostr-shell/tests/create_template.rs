//! Making an object of a type from the sidebar's New menu starts it from the
//! type's default template, on a Lite device too. Field report: a Meeting made
//! with the nav pane's create button on roostr.space came out without the
//! template's agent - the Lite copy did not hold the template, and its failed
//! read was swallowed.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::sync::Arc;

use anyhow::Result;
use parking_lot::Mutex;
use roostr_core::lite::{LiteControl, VaultEstimate};
use roostr_core::nav::Route;
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream};
use serde_json::Value as Json;

use common::Harness;

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
    fn load_space(&self, _: &str) -> BoxFuture<Result<()>> {
        ready(Ok(()))
    }
    fn fetch(&self, ids: Vec<String>) -> BoxFuture<Result<()>> {
        if ids.iter().any(|id| id == TEMPLATE) {
            *self.0.template_held.lock() = true;
        }
        ready(Ok(()))
    }
    fn estimate(&self) -> BoxFuture<Result<VaultEstimate>> {
        ready(Ok(VaultEstimate::default()))
    }
    fn sync_fully(&self) -> BoxFuture<Result<()>> {
        ready(Ok(()))
    }
    fn switch_to_lite(&self) -> BoxFuture<Result<()>> {
        ready(Ok(()))
    }
}

struct Handle(Arc<LiteVault>);

impl Backend for Handle {
    fn describe(&self) -> String {
        "a lite vault".into()
    }
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        ready(Ok(Vec::new()))
    }
    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
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
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: SPACE.into(),
            name: "Matcherino".into(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>> {
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
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<Result<JsonMap>> {
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
    fn serving(&self, _: &[String]) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        Box::pin(futures::stream::pending())
    }
    fn lite(&self) -> Option<Arc<dyn LiteControl>> {
        Some(Arc::new(Control(self.0.clone())))
    }
}

#[test]
fn a_meeting_from_the_new_menu_on_a_lite_device_gets_its_types_default_template() {
    let vault = Arc::new(LiteVault::default());
    let mut h = Harness::new(Arc::new(Handle(vault.clone())), (1280., 800.));
    assert!(h.has("create-btn"), "the sidebar's New pill");
    h.click("create-btn");
    assert!(h.has("create-menu"), "the New menu opens under the pill");
    let menu = h.rect("create-menu").unwrap();
    let row = h.rect("create-meeting").expect("the Meeting row");
    assert!(
        row[1] >= menu[1] && row[1] + row[3] <= menu[1] + menu[3] + 0.5,
        "the rows sit inside the menu: {row:?} in {menu:?}"
    );
    h.click("create-meeting");
    assert!(!h.has("create-menu"), "a pick closes the menu");
    assert_eq!(
        h.state().nav.route(),
        &Route::Object("new-meeting".into()),
        "the new meeting opens"
    );
    assert!(
        vault
            .set_fields
            .lock()
            .contains(&("new-meeting".into(), "agent".into())),
        "the template's agent is on the new meeting: {:?}",
        vault.set_fields.lock()
    );
}

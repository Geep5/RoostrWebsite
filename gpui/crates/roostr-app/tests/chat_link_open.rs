//! A web link in a chat message opens in the browser when clicked. Field
//! report: the agent's hyperlink in the chat did nothing on click.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use roostr_core::nav::Route;
use roostr_core::types::{Block, BlockContent, CustomContent, Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream, boxed, boxed_stream};
use serde_json::Value as Json;
use support::App;

const SPACE: &str = "sp-1";
const PAGE: &str = "page-1";
const AGENT: &str = "agent-1";
const URL: &str = "https://example.com/report";

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    boxed(std::future::ready(value))
}

fn object(id: &str, type_key: &str, name: &str) -> Object {
    let mut fields = Fields::default();
    fields.insert("name".into(), Value::string(name));
    fields.insert("channel".into(), Value::string(SPACE));
    Object { id: id.into(), type_key: type_key.into(), fields, ..Object::default() }
}

/// A page whose chat holds one agent message with a markdown link, and the agent.
fn page() -> Object {
    let mut p = object(PAGE, "page", "Report");
    let meta = BTreeMap::from([
        ("author".to_string(), AGENT.to_string()),
        ("ts".to_string(), "1000".to_string()),
        ("text".to_string(), format!("Here it is: [the report]({URL})")),
    ]);
    p.blocks = vec![
        Block {
            id: "__discussion__".into(),
            children_ids: vec!["m1".into()],
            content: BlockContent { custom: Some(CustomContent { content_type: "discussion".into(), meta: Some(BTreeMap::new()), data: None }), ..BlockContent::default() },
            ..Block::default()
        },
        Block {
            id: "m1".into(),
            content: BlockContent { custom: Some(CustomContent { content_type: "chat".into(), meta: Some(meta), data: None }), ..BlockContent::default() },
            ..Block::default()
        },
    ];
    p
}

struct Vault {
    objects: Vec<Object>,
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "a page with a linked chat message".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings { has_key: true, ..Settings::default() }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(self
            .objects
            .iter()
            .map(|o| ObjectSummary { id: o.id.clone(), type_key: o.type_key.clone(), name: o.field_str("name").into(), channel_id: SPACE.into(), updated_at: 1_000, ..ObjectSummary::default() })
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
            .map(|o| QueryRow { id: o.id.clone(), type_key: o.type_key.clone(), name: Some(o.field_str("name").into()), fields: o.fields.clone(), ..QueryRow::default() })
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
fn a_link_in_a_chat_message_opens_in_the_browser() {
    let vault = Vault { objects: vec![page(), object(AGENT, "agent", "Grant Email Bot")] };
    let mut app = App::open(Arc::new(vault), 1568., 934.);
    app.navigate(Route::Object(PAGE.into()));
    app.settle();
    let link = format!("url:{URL}");
    let (x, y) = app.rt.find(&link).unwrap_or_else(|| {
        let keys: Vec<String> = app.rt.elements().into_iter().filter_map(|e| e.key).filter(|k| k.contains("msg") || k.contains("md-")).collect();
        panic!("the link isn't on screen; message keys: {keys:?}")
    });
    app.click_at(x + 4., y + 4.);
    let opened = app.rt.take_url_requests();
    assert_eq!(opened, vec![URL.to_string()], "the click opened nothing");
}

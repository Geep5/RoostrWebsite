//! The Finder bubble: opening it starts a private session (no space) for the space you're in,
//! and when Finder hands the request off, the page it went to opens.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::sync::Arc;

use parking_lot::Mutex;
use roostr_core::nav::Route;
use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream, boxed, boxed_stream};
use serde_json::Value as Json;
use support::App;

const PAGE: &str = "page-1";
const SESSION: &str = "session-1";

/// A space with one page; a Finder session that, once made, carries a hand-off to that page.
#[derive(Default)]
struct Vault {
    created: Mutex<Vec<JsonMap>>,
    posted: Mutex<Vec<String>>,
}

struct Handle(Arc<Vault>);

fn ready<T: Send + 'static>(v: T) -> BoxFuture<T> {
    boxed(std::future::ready(v))
}

impl Backend for Handle {
    fn describe(&self) -> String {
        "finder test".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(vec![ObjectSummary {
            id: PAGE.into(),
            type_key: "page".into(),
            name: "Quotes".into(),
            channel_id: "sp-1".into(),
            updated_at: 1,
            ..ObjectSummary::default()
        }]))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let fields = match id {
            PAGE => [
                ("name".to_string(), Value::string("Quotes")),
                ("channel".to_string(), Value::string("sp-1")),
            ]
            .into(),
            SESSION if !self.0.created.lock().is_empty() => [
                (
                    "finder_handoff".to_string(),
                    Value::string(r#"{"page":"page-1","at":5}"#),
                ),
                (
                    "finder_options".to_string(),
                    Value::string(
                        r#"[{"id":"page-1","name":"Quotes","kind":"page","type":"page","icon":"💬"},{"id":"new","name":"＋ New page","kind":"new"}]"#,
                    ),
                ),
            ]
            .into(),
            _ => return ready(Err(anyhow::anyhow!("unknown object {id}"))),
        };
        let type_key = if id == PAGE { "page" } else { "finder_session" };
        ready(Ok(Object {
            id: id.into(),
            type_key: type_key.into(),
            fields,
            ..Object::default()
        }))
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, _: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        ready(Ok(QueryResult::default()))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        let mut out = JsonMap::new();
        if action == "chat_post" {
            self.0
                .posted
                .lock()
                .push(params["text"].as_str().unwrap_or("").to_string());
        }
        if action == "create" && params["type_key"] == "finder_session" {
            self.0.created.lock().push(params);
            out.insert("id".into(), SESSION.into());
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
        boxed_stream(futures::stream::pending())
    }
}

#[test]
fn the_bubble_starts_a_private_session_and_a_hand_off_opens_its_page() {
    let vault = Arc::new(Vault::default());
    let mut app = App::open(Arc::new(Handle(vault.clone())), 1280., 800.);
    app.navigate(Route::Home);
    assert!(app.shown("finder-bubble") && !app.shown("finder-panel"));
    app.tap("finder-bubble");
    app.settle();
    assert!(app.shown("finder-panel"), "the chat opens");
    let made = vault.created.lock().clone();
    assert_eq!(made.len(), 1, "one new session");
    let fields = &made[0]["fields"];
    assert_eq!(
        fields["finder_space"]["stringValue"], "sp-1",
        "for the space you're in"
    );
    assert!(fields.get("channel").is_none(), "in no space: private");
    assert_eq!(
        app.rt.state.nav.route().clone(),
        Route::Object(PAGE.into()),
        "the hand-off's page opened"
    );

    // Finder's choices are cards to tap; a tap sends the card's name once, and they hide.
    assert!(app.shown("finder-choice-page-1") && app.shown("finder-choice-new"));
    app.tap("finder-choice-page-1");
    app.settle();
    assert_eq!(*vault.posted.lock(), ["Quotes"]);
    assert!(!app.shown("finder-choice-page-1"), "answered: the cards go");
}

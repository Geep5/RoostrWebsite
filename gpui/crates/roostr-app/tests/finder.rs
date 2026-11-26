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
fn the_rail_tile_starts_a_private_session_and_a_hand_off_opens_its_page() {
    let vault = Arc::new(Vault::default());
    let mut app = App::open(Arc::new(Handle(vault.clone())), 1280., 800.);
    app.navigate(Route::Home);
    assert!(app.shown("rail-finder") && !app.shown("finder-panel"));
    app.tap("rail-finder");
    app.settle();
    assert!(app.shown("finder-panel"), "the chat opens");
    // Beside its tile in the rail, bottoms level - not floating over the chat's composer.
    let tile = app.rect("rail-finder").expect("the tile");
    let panel = app.rect("finder-panel").expect("the panel");
    assert!(
        panel[0] >= tile[0] + tile[2],
        "the chat opens right of the rail: {panel:?} vs {tile:?}"
    );
    assert!(
        (panel[1] + panel[3] - (tile[1] + tile[3])).abs() < 1.,
        "bottoms level: {panel:?} vs {tile:?}"
    );
    assert!(
        panel[0] + panel[2] < 1280. / 2.,
        "on the left, away from the chat column"
    );
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

/// A vault whose Finder agent has no Anthropic login: the app answers the session itself,
/// without a model or any computer, with the pages the words name.
#[derive(Default)]
struct Offline {
    /// The session's chat: (author, text).
    chat: Mutex<Vec<(String, String)>>,
    fields: Mutex<Vec<(String, String)>>,
    /// Posted elsewhere (a page's chat): (object, author, text).
    elsewhere: Mutex<Vec<(String, String, String)>>,
}

struct OfflineHandle(Arc<Offline>);

const FINDER: &str = "finder-agent-sp-1";

fn row(id: &str, type_key: &str, name: &str) -> roostr_core::types::QueryRow {
    roostr_core::types::QueryRow {
        id: id.into(),
        type_key: type_key.into(),
        updated_at: 1,
        fields: [
            ("name".to_string(), Value::string(name)),
            ("channel".to_string(), Value::string("sp-1")),
        ]
        .into(),
        ..Default::default()
    }
}

impl Backend for OfflineHandle {
    fn describe(&self) -> String {
        "finder offline test".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            author_id: "me".into(),
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(Vec::new()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let object = match id {
            SESSION => {
                let chat = self.0.chat.lock().clone();
                let mut ids = Vec::new();
                let mut blocks = Vec::new();
                for (n, (author, text)) in chat.into_iter().enumerate() {
                    let id = format!("m{n}");
                    ids.push(id.clone());
                    blocks.push(roostr_core::types::Block {
                        id,
                        content: roostr_core::types::BlockContent {
                            custom: Some(roostr_core::types::CustomContent {
                                content_type: "chat".into(),
                                meta: Some(
                                    [
                                        ("author".to_string(), author),
                                        ("ts".to_string(), (n + 1).to_string()),
                                        ("text".to_string(), text),
                                    ]
                                    .into(),
                                ),
                                data: None,
                            }),
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                }
                Object {
                    id: SESSION.into(),
                    type_key: "finder_session".into(),
                    fields: self
                        .0
                        .fields
                        .lock()
                        .iter()
                        .map(|(k, v)| (k.clone(), Value::string(v)))
                        .collect(),
                    blocks,
                    ..Object::default()
                }
            }
            FINDER => Object {
                id: FINDER.into(),
                type_key: "agent".into(),
                fields: [("name".to_string(), Value::string("Finder"))].into(),
                ..Object::default()
            },
            "sp-1" => Object {
                id: "sp-1".into(),
                type_key: "channel".into(),
                fields: [("name".to_string(), Value::string("Personal"))].into(),
                ..Object::default()
            },
            _ => return ready(Err(anyhow::anyhow!("unknown object {id}"))),
        };
        ready(Ok(object))
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
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let records = if body["filters"].is_array() {
            vec![
                row("hunt", "page", "Hunter Overview"),
                row("groceries", "page", "Groceries"),
                row(FINDER, "agent", "Finder"),
                row("space-agent-sp-1", "agent", "Personal Space"),
            ]
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
                out.insert("id".into(), SESSION.into());
            }
            "chat_post" if params["object_id"] == SESSION => {
                let author = params
                    .get("as_author")
                    .and_then(Json::as_str)
                    .unwrap_or("me")
                    .to_string();
                let text = params["text"].as_str().unwrap_or("").to_string();
                self.0.chat.lock().push((author, text));
            }
            "chat_post" => self.0.elsewhere.lock().push((
                params["object_id"].as_str().unwrap_or("").to_string(),
                params["as_author"].as_str().unwrap_or("").to_string(),
                params["text"].as_str().unwrap_or("").to_string(),
            )),
            "set_field" => {
                let key = params["key"].as_str().unwrap_or("").to_string();
                let value = params["value"]["stringValue"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let mut fields = self.0.fields.lock();
                fields.retain(|(k, _)| *k != key);
                fields.push((key, value));
            }
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
        boxed_stream(futures::stream::pending())
    }
}

/// Finder runs in the app, with no computer: with no model to ask, it still answers with the
/// pages the words name, and tapping one opens it.
#[test]
fn with_no_model_the_app_answers_with_matching_pages_that_open() {
    let vault = Arc::new(Offline::default());
    let mut app = App::open(Arc::new(OfflineHandle(vault.clone())), 1280., 800.);
    app.navigate(Route::Home);
    app.tap("rail-finder");
    app.settle();
    roostr_core::api::chat::post(
        SESSION,
        "open the hunter overview",
        "",
        "",
        &[],
        app.state(),
    )
    .detach();
    app.settle();
    let chat = vault.chat.lock().clone();
    assert_eq!(chat.len(), 2, "Finder answered once: {chat:?}");
    assert_eq!(chat[1].0, FINDER, "as Finder");
    assert!(
        chat[1].1.contains("no Anthropic login"),
        "says why: {}",
        chat[1].1
    );
    assert!(
        app.shown("finder-choice-hunt"),
        "the matching page, as a card"
    );
    assert!(
        !app.shown("finder-choice-groceries"),
        "only pages the words name"
    );
    app.tap("finder-choice-hunt");
    app.settle();
    assert_eq!(
        app.rt.state.nav.route().clone(),
        Route::Object("hunt".into()),
        "the page opened, no model asked"
    );
    assert_eq!(vault.chat.lock().len(), 4, "the tap, then \"Opened …\"");
}

/// [✅ Do it] needs no model: the app posts the request on the page as Finder, tagging the
/// agent (the @ wakes it), says where it went, and opens the page.
#[test]
fn do_it_hands_the_request_to_the_agent_on_its_page() {
    let vault = Arc::new(Offline::default());
    *vault.fields.lock() = vec![
        (
            "finder_options".into(),
            r#"[{"id":"do","name":"✅ Do it","kind":"do"},{"id":"pick","name":"Pick another agent","kind":"pick_agent"}]"#.into(),
        ),
        (
            "finder_pending".into(),
            r#"{"page":"hunt","agent":"space-agent-sp-1","message":"Grant asks: add a pvp note"}"#.into(),
        ),
    ];
    let mut app = App::open(Arc::new(OfflineHandle(vault.clone())), 1280., 800.);
    app.navigate(Route::Home);
    app.tap("rail-finder");
    app.settle();
    app.tap("finder-choice-do");
    app.settle();
    assert_eq!(
        *vault.elsewhere.lock(),
        [(
            "hunt".to_string(),
            FINDER.to_string(),
            "@Personal Space Grant asks: add a pvp note".to_string()
        )]
    );
    let chat = vault.chat.lock().clone();
    assert_eq!(
        chat.last().map(|c| c.1.as_str()),
        Some("➡️ Sent to @Personal Space on [Hunter Overview](/app/object/hunt).")
    );
    assert_eq!(
        app.rt.state.nav.route().clone(),
        Route::Object("hunt".into())
    );
}

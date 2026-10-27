//! An agent added to an object's Agent property is in its open chat at once:
//! the composer addresses it and the @-menu offers it, with no reload. The
//! chat used to keep the agents it loaded when it opened, so a newly added
//! agent was untaggable until the page was refreshed.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{px, size};
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_shell::Workspace;
use serde_json::Value as Json;

/// One space: an agent, and a chat object whose Agent list starts empty.
struct Vault {
    guests: Mutex<Vec<String>>,
    posts: Mutex<Vec<String>>,
    events: Mutex<Option<mpsc::UnboundedReceiver<anyhow::Result<BackendEvent>>>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

impl Vault {
    fn fields(&self, id: &str) -> Fields {
        let name = if id == "agent-1" { "Scout" } else { "Standup" };
        let mut fields: Fields = [
            ("name".to_string(), Value::string(name)),
            ("channel".to_string(), Value::string("sp-1")),
        ]
        .into();
        let guests = self.guests.lock();
        if id == "chat-1" && !guests.is_empty() {
            fields.insert(
                "agent".into(),
                Value::values(guests.iter().map(|g| Value::link(g, "agent")).collect()),
            );
        }
        fields
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "an agent and a chat".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok([
            ("agent-1", "agent", "Scout"),
            ("chat-1", "chat", "Standup"),
        ]
        .iter()
        .map(|(id, type_key, name)| ObjectSummary {
            id: (*id).into(),
            type_key: (*type_key).into(),
            name: (*name).into(),
            channel_id: "sp-1".into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        })
        .collect()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let type_key = match id {
            "agent-1" => "agent",
            "chat-1" => "chat",
            _ => return ready(Err(anyhow::anyhow!("unknown object {id}"))),
        };
        ready(Ok(Object {
            id: id.into(),
            type_key: type_key.into(),
            fields: self.fields(id),
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
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        if body["type"] != "agent" {
            return ready(Ok(QueryResult::default()));
        }
        ready(Ok(QueryResult {
            total: 1,
            records: vec![QueryRow {
                id: "agent-1".into(),
                type_key: "agent".into(),
                fields: self.fields("agent-1"),
                ..QueryRow::default()
            }],
        }))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        if action == "chat_post" {
            self.posts
                .lock()
                .push(params["text"].as_str().unwrap_or_default().to_string());
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
        match self.events.lock().take() {
            Some(rx) => Box::pin(rx),
            None => Box::pin(futures::stream::pending()),
        }
    }
}

fn settle(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
}

#[gpui::test]
fn an_agent_added_to_an_object_is_in_its_open_chat_at_once(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    let (tx, rx) = mpsc::unbounded();
    let backend = Arc::new(Vault {
        guests: Mutex::new(Vec::new()),
        posts: Mutex::new(Vec::new()),
        events: Mutex::new(Some(rx)),
    });
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(backend.clone(), None, cx);
        roostr_app::init(cx);
    });
    let (_workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(390.), px(844.)));
    settle(cx);
    // The chat object opens its full-screen chat; nobody is on it yet.
    cx.update(|_, cx| navigate(Route::Object("chat-1".into()), cx));
    settle(cx);

    // Scout is added to the Agent property while the chat is open.
    backend.guests.lock().push("agent-1".into());
    tx.unbounded_send(Ok(BackendEvent::Committed("chat-1".into())))
        .unwrap();
    settle(cx);

    // The next line goes to Scout: the composer addresses the new agent.
    cx.simulate_input("hello");
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert_eq!(*backend.posts.lock(), ["@Scout hello"]);
}

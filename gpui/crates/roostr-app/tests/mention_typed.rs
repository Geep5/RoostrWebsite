//! `@Agent Name` typed out in full - not picked from the menu - tags the
//! agent like a pick. Field report: "@Grant Email " (a space after the
//! two-word name) showed "No agent matches" and the name stayed plain text.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;

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

/// (id, type, name) of the objects in the space: the agent, and a chat
/// whose one guest is that agent.
const OBJECTS: [(&str, &str, &str); 2] =
    [("agent-1", "agent", "Grant Email"), ("chat-1", "chat", "Standup")];

/// One space holding an agent and a chat it is the guest of.
#[derive(Default)]
struct Vault {
    /// (object id, text) of every chat line posted.
    posts: Mutex<Vec<(String, String)>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn fields(id: &str, name: &str) -> Fields {
    let mut fields: Fields = [
        ("name".to_string(), Value::string(name)),
        ("channel".to_string(), Value::string("sp-1")),
    ]
    .into();
    if id == "chat-1" {
        fields.insert(
            "agent".into(),
            Value::values(vec![Value::link("agent-1", "agent")]),
        );
    }
    fields
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "an agent and its chat".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(OBJECTS
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
        let object = OBJECTS
            .iter()
            .find(|(oid, _, _)| *oid == id)
            .map(|(id, type_key, name)| Object {
                id: (*id).into(),
                type_key: (*type_key).into(),
                fields: fields(id, name),
                ..Object::default()
            });
        ready(object.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
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
    /// The space's agents (`loadObjectAgents`); nothing else is asked for here.
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        if body["type"] != "agent" {
            return ready(Ok(QueryResult::default()));
        }
        ready(Ok(QueryResult {
            total: 1,
            records: vec![QueryRow {
                id: "agent-1".into(),
                type_key: "agent".into(),
                fields: fields("agent-1", "Grant Email"),
                ..QueryRow::default()
            }],
        }))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        if action == "chat_post" {
            let field = |key: &str| params[key].as_str().unwrap_or_default().to_string();
            self.posts.lock().push((field("object_id"), field("text")));
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

/// Types `text` into the focused composer and sends it.
fn send(cx: &mut VisualTestContext, text: &str) {
    cx.simulate_input(text);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
}

fn posts(vault: &Vault) -> Vec<(String, String)> {
    vault.posts.lock().clone()
}

fn post(object_id: &str, text: &str) -> (String, String) {
    (object_id.to_string(), text.to_string())
}

#[gpui::test]
fn a_typed_out_agent_name_tags_the_agent(cx: &mut TestAppContext) {
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
        cx.add_window_view(|window, cx| Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(390.), px(844.)));
    cx.run_until_parked();
    // The chat page, its composer emptied of the seeded `@Grant Email `.
    cx.update(|_, cx| navigate(Route::Object("chat-1".into()), cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-a backspace");
    cx.run_until_parked();
    let shown = |cx: &mut VisualTestContext, selector: &'static str| {
        cx.debug_bounds(selector).is_some()
    };

    cx.simulate_input("@Grant Email");
    cx.run_until_parked();
    assert!(shown(cx, "mention-menu"), "the agent is offered");
    // The space after the full name tags it: no "No agent matches", menu closed.
    cx.simulate_input(" ");
    cx.run_until_parked();
    assert!(!shown(cx, "mention-none"), "no 'No agent matches'");
    assert!(!shown(cx, "mention-menu"), "the menu closes on the tag");
    send(cx, "hi");
    assert_eq!(posts(&backend), [post("chat-1", "@Grant Email hi")]);

    // In other capitals: tagged too, in the agent's own spelling.
    cx.simulate_keystrokes("cmd-a backspace");
    cx.run_until_parked();
    cx.simulate_input("@grant email ");
    cx.run_until_parked();
    assert!(!shown(cx, "mention-none"), "no 'No agent matches'");
    assert!(!shown(cx, "mention-menu"), "the menu closes on the tag");
    send(cx, "again");
    assert_eq!(posts(&backend)[1..], [post("chat-1", "@Grant Email again")]);
}

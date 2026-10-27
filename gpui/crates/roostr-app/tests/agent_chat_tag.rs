//! An agent's chat opens addressed to it on a phone too: the composer starts
//! as `@Agent ` (`seedDefaultAgent`), so a plain message reaches the agent.
//! Field report: the browser and the desktop app did, the iPhone app never
//! did - the composer waited for "the active window" to write the tag, and
//! the iOS platform named none. The composer's own window gets its tag
//! whether or not the platform reports it as the active one.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;

use gpui::{Modifiers, TestAppContext, VisualTestContext};
use gpui_kit::{px, size};
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Navigator, Route, navigate};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_shell::Workspace;
use serde_json::Value as Json;

/// (id, type, name) of the objects in the space: the agent, and a chat
/// whose one guest is that agent.
const OBJECTS: [(&str, &str, &str); 2] =
    [("agent-1", "agent", "Scout"), ("chat-1", "chat", "Standup")];

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
                fields: fields("agent-1", "Scout"),
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

fn tap(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
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
fn an_agents_chat_on_a_phone_opens_addressed_to_it(cx: &mut TestAppContext) {
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
    // iPhone portrait, in a window the platform does not report as active.
    cx.simulate_resize(size(px(390.), px(844.)));
    cx.deactivate_window();
    cx.run_until_parked();
    assert!(cx.update(|_, cx| cx.active_window()).is_none());

    // The agent's page: the header's chat button opens its chat as a
    // full-screen sheet, addressed to the agent.
    cx.update(|_, cx| navigate(Route::Object("agent-1".into()), cx));
    cx.run_until_parked();
    tap(cx, "m-pane");
    assert!(cx.debug_bounds("disc-sheet").is_some(), "the chat sheet");
    send(cx, "hello");
    assert_eq!(posts(&backend), [post("agent-1", "@Scout hello")]);
    // A sent line leaves the composer addressed to the agent again.
    send(cx, "and again");
    assert_eq!(posts(&backend)[1..], [post("agent-1", "@Scout and again")]);

    // A chat object opens the full-screen chat page, addressed to its only
    // guest agent.
    cx.update(|_, cx| navigate(Route::Object("chat-1".into()), cx));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| Navigator::global(cx).read(cx).route().clone()),
        Route::Chat("chat-1".into())
    );
    assert!(
        cx.debug_bounds("agent-typing").is_none(),
        "nobody typing yet"
    );
    send(cx, "standup");
    assert_eq!(posts(&backend)[2..], [post("chat-1", "@Scout standup")]);
    // A line to an agent assumes its reply: it shows typing straight away,
    // from this device alone (no harness here reports anything).
    assert!(
        cx.debug_bounds("agent-typing").is_some(),
        "Scout shows typing"
    );
}

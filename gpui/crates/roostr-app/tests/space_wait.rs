//! A click made while the shell still waits for the remembered space. A
//! replica opens its workspace while it prepares the vault; until the space
//! the app last had open is prepared, the sidebar shows the first space
//! listed. Field report: in that sidebar (Viv) a click on the Agent type
//! landed on another space's "All Agents" query - the remembered space
//! (Matcherino) arrived and its saved tab strip replaced the page clicked.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use gpui_kit::{px, size};
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Navigator, Route};
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, SyncPhase,
    SyncStatus, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_shell::Workspace;
use serde_json::{Value as Json, json};

const VIV: &str = "sp-viv";
const MATCHERINO: &str = "sp-matcherino";
const VIV_AGENT: &str = "bundled-type-agent-viv";
const MATCHERINO_AGENT: &str = "bundled-type-agent-matcherino";
const ALL_AGENTS: &str = "q-all-agents";

/// Viv (shared by another owner) and Matcherino (own, with a pinned "All
/// Agents" query), each with an agent type. Until `prepared`, Matcherino's
/// objects are not prepared yet: reads leave them out.
struct Vault {
    prepared: Mutex<bool>,
    events: Mutex<Option<mpsc::UnboundedReceiver<anyhow::Result<BackendEvent>>>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn fields(pairs: &[(&str, &str)]) -> roostr_core::types::Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::string(*v)))
        .collect()
}

/// (id, typeKey, name, space, key).
type Row = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

const OBJECTS: [Row; 3] = [
    (VIV_AGENT, "type", "Agent", VIV, "agent"),
    (MATCHERINO_AGENT, "type", "Agent", MATCHERINO, "agent"),
    (ALL_AGENTS, "query", "All Agents", MATCHERINO, ""),
];

impl Vault {
    /// What reads see: Matcherino's objects only once it is prepared.
    fn held(&self) -> Vec<Row> {
        let prepared = *self.prepared.lock();
        OBJECTS
            .into_iter()
            .filter(|o| prepared || o.3 != MATCHERINO)
            .collect()
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "two spaces, one still preparing".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(self
            .held()
            .into_iter()
            .map(|(id, type_key, name, space, _)| ObjectSummary {
                id: id.into(),
                type_key: type_key.into(),
                name: name.into(),
                channel_id: space.into(),
                updated_at: 1_000,
                ..ObjectSummary::default()
            })
            .collect()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let object =
            self.held()
                .into_iter()
                .find(|o| o.0 == id)
                .map(|(id, type_key, name, space, key)| {
                    let mut f = fields(&[("name", name), ("channel", space)]);
                    if !key.is_empty() {
                        f.insert("key".into(), Value::string(key));
                    }
                    if type_key == "query" {
                        f.insert("setOf".into(), Value::values(vec![Value::string("agent")]));
                    }
                    Object {
                        id: id.into(),
                        type_key: type_key.into(),
                        fields: f,
                        ..Object::default()
                    }
                });
        ready(object.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        let mut spaces = vec![Space {
            id: VIV.into(),
            name: "Viv".into(),
            owner: "e54e46d8".into(),
            created_at: 1,
            ..Space::default()
        }];
        if *self.prepared.lock() {
            spaces.push(Space {
                id: MATCHERINO.into(),
                name: "Matcherino".into(),
                pinned_ids: vec![ALL_AGENTS.into()],
                created_at: 2,
                ..Space::default()
            });
        }
        ready(Ok(spaces))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let defs = body["filters"][0]["key"] == "typeKey";
        let records: Vec<QueryRow> = if defs {
            self.held()
                .into_iter()
                .filter(|o| o.1 == "type")
                .map(|(id, type_key, name, space, key)| QueryRow {
                    id: id.into(),
                    type_key: type_key.into(),
                    name: Some(name.into()),
                    fields: fields(&[("name", name), ("channel", space), ("key", key)]),
                    ..QueryRow::default()
                })
                .collect()
        } else {
            Vec::new()
        };
        ready(Ok(QueryResult {
            total: records.len() as u64,
            records,
        }))
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
        match self.events.lock().take() {
            Some(rx) => Box::pin(rx),
            None => Box::pin(futures::stream::pending()),
        }
    }
}

fn settle(cx: &mut VisualTestContext) {
    // Past the store's refresh throttle.
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
}

fn status(preparing: Option<(u64, u64)>) -> BackendEvent {
    BackendEvent::Status(SyncStatus {
        phase: SyncPhase::Live,
        imported: 0,
        detail: None,
        pending: None,
        received: None,
        expected: None,
        bootstrapped: preparing.is_none(),
        preparing,
        lite: None,
    })
}

#[gpui::test]
fn a_click_while_the_remembered_space_prepares_stays_where_it_was_made(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    let path = ui_state.path().join("ui-state.json");
    // Last session: Matcherino open on its "All Agents" query.
    let mut tabs = serde_json::Map::new();
    tabs.insert(
        MATCHERINO.into(),
        json!({ "paths": [format!("/app/object/{ALL_AGENTS}")], "active": 0 }),
    );
    let saved = json!({ "glon.channel": MATCHERINO, "roostr.tabs.spaces": tabs });
    std::fs::write(&path, saved.to_string()).unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", &path) };
    let (tx, rx) = mpsc::unbounded();
    tx.unbounded_send(Ok(status(Some((1, 10))))).unwrap();
    let backend = Arc::new(Vault {
        prepared: Mutex::new(false),
        events: Mutex::new(Some(rx)),
    });
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(backend.clone(), None, cx);
        roostr_app::init(cx);
    });
    let (_workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    settle(cx);
    let space = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| Navigator::global(cx).read(cx).space().to_string())
    };
    let route = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| Navigator::global(cx).read(cx).route().clone())
    };
    assert_eq!(space(cx), "", "the shell waits for the remembered space");

    // The sidebar shows Viv, the first space listed: its Agent type opens.
    let row = cx
        .debug_bounds("type:bundled-type-agent-viv")
        .expect("Viv's Agent type in the sidebar");
    assert!(
        cx.debug_bounds("type:bundled-type-agent-matcherino")
            .is_none()
    );
    cx.simulate_click(row.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(route(cx), Route::Object(VIV_AGENT.into()));

    // Matcherino is prepared while the rest still is (reads see it; the
    // replica's commits refresh the store): the page clicked stays open,
    // in the space it was clicked in - not Matcherino's saved "All Agents".
    *backend.prepared.lock() = true;
    tx.unbounded_send(Ok(BackendEvent::Committed(MATCHERINO.into())))
        .unwrap();
    settle(cx);
    assert_eq!(
        route(cx),
        Route::Object(VIV_AGENT.into()),
        "the click's page stays open"
    );
    assert_eq!(space(cx), VIV, "in the space it was made in");
    // And once the whole vault is prepared.
    tx.unbounded_send(Ok(status(None))).unwrap();
    tx.unbounded_send(Ok(BackendEvent::Prepared)).unwrap();
    settle(cx);
    assert_eq!(route(cx), Route::Object(VIV_AGENT.into()));
    assert_eq!(space(cx), VIV);
}

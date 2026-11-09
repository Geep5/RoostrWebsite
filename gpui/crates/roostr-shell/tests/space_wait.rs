//! A click made while the shell still waits for the remembered space. A
//! replica opens its workspace while it prepares the vault; until the space
//! the app last had open is prepared, the sidebar shows the first space
//! listed. Field report: in that sidebar (Viv) a click on the Agent type
//! landed on another space's "All Agents" query - the remembered space
//! (Matcherino) arrived and its saved tab strip replaced the page clicked.
//! Also: a desktop space with nothing open opens its first pin, not its home.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::channel::mpsc;
use parking_lot::Mutex;
use roostr_core::nav::Route;
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, SyncPhase,
    SyncStatus, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream};
use serde_json::{Value as Json, json};

use common::Harness;

const VIV: &str = "sp-viv";
const MATCHERINO: &str = "sp-matcherino";
const VIV_AGENT: &str = "bundled-type-agent-viv";
const MATCHERINO_AGENT: &str = "bundled-type-agent-matcherino";
const ALL_AGENTS: &str = "q-all-agents";
const PLAN: &str = "page-plan";

/// Viv (shared by another owner) and Matcherino (own, with a pinned "All
/// Agents" query), each with an agent type. Until `prepared`, Matcherino's
/// objects are not prepared yet: reads leave them out.
struct Vault {
    prepared: Mutex<bool>,
    events: Mutex<Option<mpsc::UnboundedReceiver<Result<BackendEvent>>>>,
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

const OBJECTS: [Row; 4] = [
    (VIV_AGENT, "type", "Agent", VIV, "agent"),
    (MATCHERINO_AGENT, "type", "Agent", MATCHERINO, "agent"),
    (ALL_AGENTS, "query", "All Agents", MATCHERINO, ""),
    (PLAN, "page", "Plan", MATCHERINO, ""),
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
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
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
    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
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
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
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
                // A pinned section first (no object), then the sidebar's order.
                pinned_ids: vec!["any-favorites".into(), PLAN.into(), ALL_AGENTS.into()],
                created_at: 2,
                ..Space::default()
            });
        }
        ready(Ok(spaces))
    }
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>> {
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
    fn mutate(&self, _: &str, _: JsonMap) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn serving(&self, _: &[String]) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        match self.events.lock().take() {
            Some(rx) => Box::pin(rx),
            None => Box::pin(futures::stream::pending()),
        }
    }
}

/// The store's handle on the shared vault.
struct Shared(Arc<Vault>);

impl Backend for Shared {
    fn describe(&self) -> String {
        self.0.describe()
    }
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        self.0.settings()
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        self.0.objects()
    }
    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        self.0.object(id)
    }
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        self.0.channels()
    }
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        self.0.relations()
    }
    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>> {
        self.0.query(body)
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<Result<JsonMap>> {
        self.0.mutate(action, params)
    }
    fn serving(&self, ids: &[String]) -> BoxFuture<Result<JsonMap>> {
        self.0.serving(ids)
    }
    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        self.0.sync_digest()
    }
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        self.0.events()
    }
}

/// Deliver what the backend sent, then run past the store's refresh throttle.
fn settle(h: &mut Harness) {
    h.settle();
    h.advance(Duration::from_secs(2));
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

#[test]
fn a_click_while_the_remembered_space_prepares_stays_where_it_was_made() {
    // Last session: Matcherino open on its "All Agents" query.
    let mut tabs = serde_json::Map::new();
    tabs.insert(
        MATCHERINO.into(),
        json!({ "paths": [format!("/app/object/{ALL_AGENTS}")], "active": 0 }),
    );
    let saved = json!({ "glon.channel": MATCHERINO, "roostr.tabs.spaces": tabs });
    let (tx, rx) = mpsc::unbounded();
    tx.unbounded_send(Ok(status(Some((1, 10))))).unwrap();
    let vault = Arc::new(Vault {
        prepared: Mutex::new(false),
        events: Mutex::new(Some(rx)),
    });
    let mut h = Harness::with_ui(Arc::new(Shared(vault.clone())), (1280., 800.), saved);
    settle(&mut h);
    let space = |h: &Harness| h.state().nav.space().to_string();
    let route = |h: &Harness| h.state().nav.route().clone();
    assert_eq!(space(&h), "", "the shell waits for the remembered space");

    // The sidebar shows Viv, the first space listed: its Agent type opens.
    assert!(
        h.has(&format!("type:{VIV_AGENT}")),
        "Viv's Agent type in the sidebar"
    );
    assert!(!h.has(&format!("type:{MATCHERINO_AGENT}")));
    h.click(&format!("type:{VIV_AGENT}"));
    assert_eq!(route(&h), Route::Object(VIV_AGENT.into()));

    // Matcherino is prepared while the rest still is (reads see it; the
    // replica's commits refresh the store): the page clicked stays open,
    // in the space it was clicked in - not Matcherino's saved "All Agents".
    *vault.prepared.lock() = true;
    tx.unbounded_send(Ok(BackendEvent::Committed(MATCHERINO.into())))
        .unwrap();
    settle(&mut h);
    assert_eq!(
        route(&h),
        Route::Object(VIV_AGENT.into()),
        "the click's page stays open"
    );
    assert_eq!(space(&h), VIV, "in the space it was made in");
    // And once the whole vault is prepared.
    tx.unbounded_send(Ok(status(None))).unwrap();
    tx.unbounded_send(Ok(BackendEvent::Prepared)).unwrap();
    settle(&mut h);
    assert_eq!(route(&h), Route::Object(VIV_AGENT.into()));
    assert_eq!(space(&h), VIV);

    // The rail: a space with a last-opened page reopens it ("All Agents",
    // though "Plan" is its first pin)...
    h.click(&format!("space:{MATCHERINO}"));
    settle(&mut h);
    assert_eq!(space(&h), MATCHERINO);
    assert_eq!(route(&h), Route::Object(ALL_AGENTS.into()), "last opened");
    // ...and nothing open - the last tab closed, a new tab - opens its first
    // pinned object (a section pin with no object is skipped), not its home.
    h.update(|s| s.nav.close(0, &mut s.ui));
    settle(&mut h);
    assert_eq!(
        route(&h),
        Route::Object(PLAN.into()),
        "the last tab closed: the first pin"
    );
    h.update(|s| roostr_core::nav::open_tab(Route::Home, false, s));
    settle(&mut h);
    assert_eq!(
        route(&h),
        Route::Object(PLAN.into()),
        "a new tab: the first pin"
    );
    let back = h.state().nav.can_go_back();
    h.update(|s| s.nav.back(&mut s.ui));
    settle(&mut h);
    assert!(
        back && route(&h) != Route::Home,
        "Back never lands on the home it replaced"
    );
    // A space with nothing pinned keeps its home.
    h.click(&format!("space:{VIV}"));
    h.update(|s| roostr_core::nav::open_tab(Route::Home, false, s));
    settle(&mut h);
    assert_eq!(route(&h), Route::Home, "no pins: the space home");
}

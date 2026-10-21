//! The global reactive data store (`src/lib/data.svelte.ts`): spaces, object
//! summaries, relation/type definitions, agents and the user's preferences,
//! plus a cache of full object states. Kept live by the backend's commit
//! stream: every commit refetches the cached object it names and schedules a
//! throttled refresh of the collections.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::StreamExt as _;
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, Task};
use serde_json::{Value as Json, json};

use crate::backend::{Backend, BackendEvent, JsonMap};
use crate::color::{DEFAULT_CHAT_COLOR, normalize_hex};
use crate::io;
use crate::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, SyncPhase,
    SyncStatus, field_str,
};

/// Fixed id of the personal preferences object (`USER_PREFS_ID` in api.ts).
pub const USER_PREFS_ID: &str = "__user_prefs__";
/// `connectEvents`: a commit schedules one refresh this far out (throttle, not debounce).
const REFRESH_THROTTLE: Duration = Duration::from_millis(800);
/// Wait between reconnect attempts of the commit stream.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
/// `fetchAllQuery` page size.
const QUERY_PAGE: u64 = 500;

/// A type object (Anytype ObjectType analog).
#[derive(Clone, Debug, PartialEq)]
pub struct TypeDef {
    pub id: String,
    pub key: String,
    pub name: String,
    pub icon: String,
    /// "page" | "task" | ...
    pub layout: String,
    pub default_template_id: String,
    /// Owning space id; "" = bundled/system type, present in every space.
    pub space: String,
    /// Built into Roostr (`bundled`) or a Roostr default (`system`).
    pub builtin: bool,
}

/// Agents never appear in `summaries` (hidden list type), but discussions name them.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRef {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// Owning space; "" = unassigned.
    pub channel: String,
    /// `#RRGGBB`; "" = the default grey bubble.
    pub chat_color: String,
}

#[derive(Clone, Debug)]
pub enum ObjectSlot {
    Loading,
    Ready(Arc<Object>),
    Failed(SharedString),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Connection {
    #[default]
    Connecting,
    Ready,
    /// The backend could not be reached or refused us.
    Failed(SharedString),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreEvent {
    /// One object committed a change (after its cached state, if any, was refetched).
    Committed(String),
    /// The commit stream reconnected: anything may have changed.
    Reconnected,
    /// Collections (spaces, summaries, relations, types, agents, prefs) were refreshed.
    Refreshed,
}

pub struct Store {
    backend: Arc<dyn Backend>,
    pub connection: Connection,
    pub settings: Option<Settings>,
    pub status: SyncStatus,
    pub channels: Vec<Space>,
    pub summaries: Vec<ObjectSummary>,
    pub relations: Vec<RelationDef>,
    pub types: Vec<TypeDef>,
    /// Ids of types and properties that are built in or Roostr defaults.
    pub builtin_ids: HashSet<String>,
    pub agents: Vec<AgentRef>,
    /// The user's own chat bubble color.
    pub chat_color: String,
    /// This person's order of the space list (space id -> position).
    pub space_order: HashMap<String, f64>,
    /// The first full refresh completed.
    pub loaded: bool,
    objects: HashMap<String, ObjectSlot>,
    in_flight: HashSet<String>,
    /// Reloads requested while a fetch of the same object was in flight.
    reload_again: HashSet<String>,
    refresh_running: bool,
    refresh_again: bool,
    /// A `bootstrapped` status held back until a refresh that started after
    /// it lands: the history walk's commits only scheduled a throttled
    /// refresh, and an empty-looking store must not read as an empty vault.
    bootstrap_held: bool,
    refresh_timer: Option<Task<()>>,
    _events: Option<Task<()>>,
}

struct GlobalStore(Entity<Store>);

impl Global for GlobalStore {}

impl EventEmitter<StoreEvent> for Store {}

impl Store {
    /// Create the store as an app global and start loading + listening.
    pub fn init(backend: Arc<dyn Backend>, cx: &mut App) -> Entity<Self> {
        let store = cx.new(|cx| {
            let mut store = Self {
                backend,
                connection: Connection::Connecting,
                settings: None,
                status: SyncStatus::default(),
                channels: Vec::new(),
                summaries: Vec::new(),
                relations: Vec::new(),
                types: Vec::new(),
                builtin_ids: HashSet::new(),
                agents: Vec::new(),
                chat_color: DEFAULT_CHAT_COLOR.to_string(),
                space_order: HashMap::new(),
                loaded: false,
                objects: HashMap::new(),
                in_flight: HashSet::new(),
                reload_again: HashSet::new(),
                refresh_running: false,
                refresh_again: false,
                bootstrap_held: false,
                refresh_timer: None,
                _events: None,
            };
            store.start(cx);
            store
        });
        cx.set_global(GlobalStore(store.clone()));
        store
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalStore>().map(|g| g.0.clone())
    }

    pub fn backend(&self) -> Arc<dyn Backend> {
        self.backend.clone()
    }

    /// Settings first (a daemon without identity cannot serve the app), then
    /// the collections, then the commit stream.
    fn start(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let settings = io::spawn_async(cx, backend.settings()).await;
            let _ = this.update(cx, |this, cx| match settings {
                Ok(settings) if !settings.has_key => {
                    this.connection = Connection::Failed(
                        "Set up the daemon identity in its terminal before opening the app.".into(),
                    );
                    this.settings = Some(settings);
                    cx.notify();
                }
                Ok(settings) => {
                    this.settings = Some(settings);
                    this.refresh_all(cx);
                    this.listen(cx);
                }
                Err(error) => {
                    this.connection = Connection::Failed(error.to_string().into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Retry from scratch after a failed connection.
    pub fn reconnect(&mut self, cx: &mut Context<Self>) {
        self.connection = Connection::Connecting;
        self._events = None;
        cx.notify();
        self.start(cx);
    }

    /// Follow the commit stream for the life of the store, reconnecting
    /// [`RECONNECT_DELAY`] after each failure.
    fn listen(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        self._events = Some(cx.spawn(async move |this, cx| {
            loop {
                let mut events = backend.events();
                while let Some(event) = events.next().await {
                    let failed = event.is_err();
                    let event = event.map_err(|error| error.to_string());
                    let Ok(()) = this.update(cx, |this, cx| this.handle_event(event, cx)) else {
                        return;
                    };
                    if failed {
                        break;
                    }
                }
                drop(events);
                cx.background_executor().timer(RECONNECT_DELAY).await;
            }
        }));
    }

    fn handle_event(&mut self, event: Result<BackendEvent, String>, cx: &mut Context<Self>) {
        match event {
            Ok(BackendEvent::Hello) => {
                // Daemon connectivity does not prove relay history finished backfilling.
                self.status = SyncStatus {
                    phase: SyncPhase::Live,
                    bootstrapped: false,
                    ..self.status.clone()
                };
                let cached: Vec<String> = self.objects.keys().cloned().collect();
                for id in cached {
                    self.reload_object(&id, cx);
                }
                self.schedule_refresh(cx);
                cx.emit(StoreEvent::Reconnected);
                cx.notify();
            }
            Ok(BackendEvent::Committed(id)) => {
                if self.objects.contains_key(&id) {
                    self.reload_object(&id, cx);
                }
                self.schedule_refresh(cx);
                cx.emit(StoreEvent::Committed(id));
            }
            Ok(BackendEvent::Prepared) => {
                // Every view may show a partial vault: reload as on a reconnect.
                let cached: Vec<String> = self.objects.keys().cloned().collect();
                for id in cached {
                    self.reload_object(&id, cx);
                }
                self.refresh_all(cx);
                cx.emit(StoreEvent::Reconnected);
            }
            Ok(BackendEvent::Status(mut status)) => {
                if status.bootstrapped && !self.status.bootstrapped {
                    status.bootstrapped = false;
                    if !self.bootstrap_held {
                        self.bootstrap_held = true;
                        self.refresh_all(cx);
                    }
                } else if !status.bootstrapped {
                    self.bootstrap_held = false;
                }
                if self.status != status {
                    self.status = status;
                    cx.notify();
                }
            }
            Err(detail) => {
                self.status = SyncStatus {
                    phase: SyncPhase::Error,
                    detail: Some(detail),
                    ..self.status.clone()
                };
                cx.notify();
            }
        }
    }

    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        if self.refresh_timer.is_some() {
            return;
        }
        self.refresh_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_THROTTLE).await;
            let _ = this.update(cx, |this, cx| {
                this.refresh_timer = None;
                this.refresh_all(cx);
            });
        }));
    }

    /// `refreshAll`: every collection, in parallel; a commit during a running
    /// refresh queues exactly one more.
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        if self.refresh_running {
            self.refresh_again = true;
            return;
        }
        self.refresh_running = true;
        let b = self.backend.clone();
        let channels = io::spawn(cx, b.channels());
        let summaries = io::spawn(cx, b.objects());
        let relations = io::spawn(cx, b.relations());
        let defs = io::spawn(cx, fetch_defs(b.clone()));
        let agents = io::spawn(cx, fetch_agents(b.clone()));
        let prefs = io::spawn(cx, read_prefs(b));
        cx.spawn(async move |this, cx| {
            let result: Result<_> = async {
                Ok((
                    channels.await?,
                    summaries.await?,
                    relations.await?,
                    defs.await?,
                    agents.await?,
                    prefs.await,
                ))
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.refresh_running = false;
                match result {
                    Ok((
                        channels,
                        summaries,
                        mut relations,
                        (types, builtin_ids),
                        agents,
                        (chat_color, space_order),
                    )) => {
                        for relation in &mut relations {
                            relation.options.sort_by(|a, b| a.order_id.cmp(&b.order_id));
                        }
                        this.channels = channels;
                        this.summaries = summaries;
                        this.relations = relations;
                        this.types = types;
                        this.builtin_ids = builtin_ids;
                        this.agents = agents;
                        this.chat_color = chat_color;
                        this.space_order = space_order;
                        this.loaded = true;
                        if this.bootstrap_held && !this.refresh_again {
                            this.bootstrap_held = false;
                            this.status.bootstrapped = true;
                        }
                        this.connection = Connection::Ready;
                        if this.status.phase == SyncPhase::Idle {
                            this.status.phase = SyncPhase::Live;
                        }
                        cx.emit(StoreEvent::Refreshed);
                    }
                    Err(error) => {
                        // The next commit retries; only a never-loaded store shows the failure.
                        if !this.loaded {
                            this.connection = Connection::Failed(error.to_string().into());
                        }
                        log::warn!("refresh failed: {error:#}");
                    }
                }
                cx.notify();
                if std::mem::take(&mut this.refresh_again) {
                    this.schedule_refresh(cx);
                }
            });
        })
        .detach();
    }

    // ---------------------------------------------------------------- objects

    /// The cached state of `id`, if loaded. Call [`Self::ensure_object`] to load it.
    pub fn object(&self, id: &str) -> Option<Arc<Object>> {
        match self.objects.get(id) {
            Some(ObjectSlot::Ready(object)) => Some(object.clone()),
            _ => None,
        }
    }

    pub fn object_slot(&self, id: &str) -> Option<&ObjectSlot> {
        self.objects.get(id)
    }

    /// Load `id` into the cache unless it is already there or loading.
    pub fn ensure_object(&mut self, id: &str, cx: &mut Context<Self>) {
        if id.is_empty() || self.objects.contains_key(id) {
            return;
        }
        self.objects.insert(id.to_string(), ObjectSlot::Loading);
        self.reload_object(id, cx);
    }

    /// Refetch `id`, keeping the previous state visible until the new one arrives.
    /// A request while a fetch is in flight queues exactly one more fetch after
    /// it, so a commit that lands mid-fetch is never lost.
    pub fn reload_object(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.in_flight.insert(id.to_string()) {
            self.reload_again.insert(id.to_string());
            return;
        }
        let backend = self.backend.clone();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let result = io::spawn_async(cx, backend.object(&id)).await;
            let _ = this.update(cx, |this, cx| {
                this.in_flight.remove(&id);
                let slot = match result {
                    Ok(object) => ObjectSlot::Ready(Arc::new(object)),
                    Err(error) => match this.objects.get(&id) {
                        // Keep a good state over a transient failure.
                        Some(ObjectSlot::Ready(object)) => ObjectSlot::Ready(object.clone()),
                        _ => ObjectSlot::Failed(error.to_string().into()),
                    },
                };
                this.objects.insert(id.clone(), slot);
                cx.notify();
                if this.reload_again.remove(&id) {
                    this.reload_object(&id, cx);
                }
            });
        })
        .detach();
    }

    /// Fetch `id` fresh (not cached), e.g. for a one-off read.
    pub fn fetch_object(&self, id: &str, cx: &App) -> Task<Result<Object>> {
        io::spawn(cx, self.backend.object(id))
    }

    // ---------------------------------------------------------------- queries

    /// `fetchQuery`: one page as the body asks.
    pub fn query(&self, body: Json, cx: &App) -> Task<Result<QueryResult>> {
        io::spawn(cx, self.backend.query(&body))
    }

    /// `fetchAllQuery`: every match, a page at a time.
    pub fn query_all(&self, body: Json, cx: &App) -> Task<Result<Vec<QueryRow>>> {
        io::spawn(cx, query_all(self.backend.clone(), body))
    }

    // -------------------------------------------------------------- mutations

    /// `/api/mutate` with `now_ms` and the wall-clock offset (`tz_offset_min`)
    /// the planner needs. A cached object named by `object_id` is refetched
    /// right away instead of waiting for its commit event.
    pub fn mutate(
        &self,
        action: &str,
        mut params: JsonMap,
        cx: &mut Context<Self>,
    ) -> Task<Result<JsonMap>> {
        params.insert(
            "now_ms".into(),
            json!(chrono::Utc::now().timestamp_millis()),
        );
        params.insert("tz_offset_min".into(), json!(tz_offset_min()));
        self.mutate_platform(action, params, cx)
    }

    /// `/api/mutate` exactly as given: the platform actions (`nostr_key_import`,
    /// `nostr_relays_set`) take no planner clock.
    pub fn mutate_platform(
        &self,
        action: &str,
        params: JsonMap,
        cx: &mut Context<Self>,
    ) -> Task<Result<JsonMap>> {
        let object_id = params
            .get("object_id")
            .and_then(Json::as_str)
            .map(str::to_string);
        let task = io::spawn(cx, self.backend.mutate(action, params));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            if result.is_ok()
                && let Some(id) = object_id
            {
                let _ = this.update(cx, |this, cx| {
                    if this.objects.contains_key(&id) {
                        this.reload_object(&id, cx);
                    }
                });
            }
            result
        })
    }

    // ------------------------------------------------------------- lookups

    pub fn summary(&self, id: &str) -> Option<&ObjectSummary> {
        self.summaries.iter().find(|s| s.id == id)
    }

    pub fn space(&self, id: &str) -> Option<&Space> {
        self.channels.iter().find(|s| s.id == id)
    }

    /// Spaces in this person's order: `space_order[id] ?? createdAt` (`orderOf`).
    pub fn ordered_spaces(&self) -> Vec<&Space> {
        let order = |s: &Space| {
            self.space_order
                .get(&s.id)
                .copied()
                .unwrap_or(s.created_at as f64)
        };
        let mut spaces: Vec<&Space> = self.channels.iter().collect();
        spaces.sort_by(|a, b| {
            order(a)
                .total_cmp(&order(b))
                .then(a.created_at.cmp(&b.created_at))
        });
        spaces
    }

    /// The relation definition for `key`, preferring `space`'s own over a bundled one.
    pub fn relation(&self, key: &str, space: &str) -> Option<&RelationDef> {
        let mut best = None;
        for relation in self.relations.iter().filter(|r| r.key == key) {
            let owner = relation.space.as_deref().unwrap_or("");
            if owner == space {
                return Some(relation);
            }
            if owner.is_empty() || best.is_none() {
                best = Some(relation);
            }
        }
        best
    }

    /// The type definition for `key`, preferring `space`'s own.
    pub fn type_def(&self, key: &str, space: &str) -> Option<&TypeDef> {
        let mut best = None;
        for def in self.types.iter().filter(|t| t.key == key) {
            if def.space == space {
                return Some(def);
            }
            if def.space.is_empty() || best.is_none() {
                best = Some(def);
            }
        }
        best
    }

    /// `layoutOf`: the type object's layout, else the legacy fallback.
    pub fn layout_of(&self, type_key: &str) -> String {
        self.types
            .iter()
            .find(|t| t.key == type_key)
            .map(|t| t.layout.clone())
            .unwrap_or_else(|| {
                if type_key == "task" {
                    "task".into()
                } else {
                    "page".into()
                }
            })
    }

    pub fn agent(&self, id: &str) -> Option<&AgentRef> {
        self.agents.iter().find(|a| a.id == id)
    }

    pub fn author_id(&self) -> &str {
        self.settings.as_ref().map_or("", |s| s.author_id.as_str())
    }
}

/// Local wall-clock offset in minutes east of UTC (`-getTimezoneOffset()`).
pub fn tz_offset_min() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}

/// `fetchAllQuery`: every match, a page at a time. Run it through [`io::spawn`].
pub async fn query_all(backend: Arc<dyn Backend>, body: Json) -> Result<Vec<QueryRow>> {
    let page = |offset: u64| -> io::BoxFuture<Result<QueryResult>> {
        let mut body = body.clone();
        if let Some(map) = body.as_object_mut() {
            map.insert("offset".into(), json!(offset));
            map.insert("limit".into(), json!(QUERY_PAGE));
        }
        backend.query(&body)
    };
    let first = page(0).await?;
    if first.records.len() as u64 >= first.total {
        return Ok(first.records);
    }
    let total = first.total;
    let mut out = first.records;
    while (out.len() as u64) < total {
        let next = page(out.len() as u64).await?;
        if next.records.is_empty() {
            break; // a concurrent delete shrank the set
        }
        out.extend(next.records);
    }
    Ok(out)
}

/// Type and property definition records: the type list and the built-in marks.
async fn fetch_defs(backend: Arc<dyn Backend>) -> Result<(Vec<TypeDef>, HashSet<String>)> {
    let records = query_all(
        backend,
        json!({ "filters": [{ "key": "typeKey", "condition": "in", "value": ["type", "relation"] }] }),
    )
    .await?;
    let builtin_ids: HashSet<String> = records
        .iter()
        .filter(|r| {
            r.fields.get("bundled").and_then(|v| v.bool_value) == Some(true)
                || r.fields.get("system").and_then(|v| v.bool_value) == Some(true)
        })
        .map(|r| r.id.clone())
        .collect();
    let mut types: Vec<TypeDef> = records
        .iter()
        .filter(|r| r.type_key == "type")
        .map(|r| {
            let s = |k: &str| field_str(&r.fields, k).to_string();
            let layout = s("layout");
            TypeDef {
                id: r.id.clone(),
                key: s("key"),
                name: s("name"),
                icon: s("iconEmoji"),
                layout: if layout.is_empty() {
                    "page".into()
                } else {
                    layout
                },
                default_template_id: s("default_template_id"),
                space: s("channel"),
                builtin: builtin_ids.contains(&r.id),
            }
        })
        .filter(|t| !t.key.is_empty())
        .collect();
    types.sort_by_cached_key(|t| t.name.to_lowercase());
    Ok((types, builtin_ids))
}

async fn fetch_agents(backend: Arc<dyn Backend>) -> Result<Vec<AgentRef>> {
    Ok(query_all(backend, json!({ "type": "agent" }))
        .await?
        .into_iter()
        .map(|r| AgentRef {
            name: field_str(&r.fields, "name").to_string(),
            icon: field_str(&r.fields, "iconEmoji").to_string(),
            channel: field_str(&r.fields, "channel").to_string(),
            chat_color: normalize_hex(field_str(&r.fields, "chat_color")).unwrap_or_default(),
            id: r.id,
        })
        .collect())
}

/// `userPrefs.read`: chat color and space order, defaults when absent.
async fn read_prefs(backend: Arc<dyn Backend>) -> (String, HashMap<String, f64>) {
    let Ok(prefs) = backend.object(USER_PREFS_ID).await else {
        return (DEFAULT_CHAT_COLOR.to_string(), HashMap::new());
    };
    let chat_color = normalize_hex(prefs.field_str("chat_color"))
        .unwrap_or_else(|| DEFAULT_CHAT_COLOR.to_string());
    let space_order = prefs
        .field("space_order")
        .and_then(|v| v.entries())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(id, v)| Some((id.clone(), v.as_number()?)))
                .collect()
        })
        .unwrap_or_default();
    (chat_color, space_order)
}

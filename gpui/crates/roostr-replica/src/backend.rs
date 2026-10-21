//! [`ReplicaBackend`]: `src/lib/engine/backend.ts` (`WebBackend`) and iOS
//! `Backend.swift` in Rust. Store + replay + query + mutate + relay sync
//! behind [`roostr_core::Backend`]; all state lives on this device.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use anyhow::{Result, anyhow, bail};
use futures::StreamExt as _;
use futures::channel::mpsc;
use futures::lock::Mutex as AsyncMutex;
use parking_lot::Mutex;
use roostr_core::backend::{Backend, BackendEvent, JsonMap};
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::lite::{LiteControl, VaultEstimate};
use roostr_core::types::{
    LiteStatus, Object, ObjectSummary, QueryResult, RelationDef, RelationOption, Settings, Space,
    SpaceLoad, SpaceMember, SyncPhase, SyncStatus, is_system_type,
};
use roostr_engine::{
    Change, Engine, MutationRequest, MutationResponse, QueryUpdate, ServingInputs, StateInputs,
    VanishCandidate, unpack_core_value_maps,
};
use roostr_nostr::Keys;
use roostr_nostr::spacekeys::{SpaceKeyring, space_owner};
use roostr_replica_store::{
    CachedReplay, ChangeRecord, ChangeStore, PendingPublish, ReplayInput, StoreError,
};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

use crate::device::DeviceInfo;
use crate::identity;
use crate::keyring::{KvSpaceKeyring, SPACE_KEYS};
use crate::local::LocalKv;
use crate::rt;
use crate::sync::{self, LiteScopes, RelaySync, SharedSpaceInfo, SyncHost, VANISH_LOG_ID};

/// Device-local key of the relay list (backend.ts `relays()`).
pub const RELAYS: &str = "roostr-relays";
/// The kind-0 profile cache (profile.ts).
pub const PROFILE: &str = "roostr-profile";
/// Device-local: how much of the vault this replica holds, `"lite"` or
/// `"full"`, decided on its first start ([`Inner::choose_holding`]).
pub const HOLDING: &str = "roostr-holding";
/// Device-local: this install's stable id, the device the directories'
/// per-device counts are written under (`docs/state-sync.md`, "Directories").
pub const DEVICE_ID: &str = "roostr-device-id";
/// Store meta: what a lite replica holds whole ([`LiteScopes`] as JSON).
const LITE_SCOPES: &str = "lite-scopes";
/// The personal directory (`core/directory.odin`), and the prefix of the
/// id of a space's directory.
const DIRECTORY_ID: &str = "__directory__";
const DIRECTORY_SPACE_PREFIX: &str = "__directory__:";
/// [`LiteControl::estimate`], roughly: bytes per relay event (an encrypted
/// change with its envelope), events a sync imports per second, and events
/// per object while this device holds none to measure.
const EVENT_BYTES: u64 = 1_600;
const EVENTS_PER_SECOND: u64 = 120;
const EVENTS_PER_OBJECT: u64 = 3;
/// Persisted replays are valid only for the engine that computed them; the
/// suffix versions this host's row encoding (Backend.swift `replayCacheVersion`).
const REPLAY_ROW_VERSION: &str = "rust/1";
/// How long a full pass replays objects no memo row serves before the
/// first read returns; the rest are prepared in the background.
const PREPARE_INLINE_MS: i64 = 1_000;
/// One background preparation step: the page renders between steps.
const PREPARE_STEP_MS: i64 = 50;
/// A preparation step the store failed is retried after this.
const PREPARE_RETRY: std::time::Duration = std::time::Duration::from_secs(5);
/// Preparation statuses: at most four a second.
const PREPARE_EMIT_MS: i64 = 250;
/// Memo rows the preparation writes at a time: what a reload mid-way loses at most.
const MEMO_ROWS_PER_WRITE: usize = 50;
/// How often a running replica checks its device record
/// ([`Inner::write_device_record`]); the planner writes only what changed,
/// and `last_seen` alone at most every 30 minutes.
const DEVICE_CHECK: std::time::Duration = std::time::Duration::from_secs(5 * 60);
/// A device record write that failed (offline, a relay not answering a
/// lite fetch) is tried again after this.
const DEVICE_RETRY: std::time::Duration = std::time::Duration::from_secs(15);

/// Everything a replica runs on: built by [`ReplicaBackend::open_native`] /
/// [`ReplicaBackend::open_browser`], or by hand in tests.
pub struct ReplicaParts {
    pub engine: Engine,
    pub store: Arc<dyn ChangeStore>,
    pub kv: Arc<dyn LocalKv>,
    /// What [`Backend::describe`] says ("replica in ~/Library/…").
    pub label: String,
}

/// One upcoming occurrence of a recurring object, for device reminders
/// (iOS `Occurrence`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub object_id: String,
    pub name: String,
    pub type_key: String,
    /// Epoch ms of the occurrence (`repeat.next`).
    pub at: i64,
    /// Assigned to an agent: it runs on the machine serving the space, not here.
    pub agent_owned: bool,
}

impl Occurrence {
    /// `"<objectId>:<at>"`: one notification per occurrence.
    pub fn id(&self) -> String {
        format!("{}:{}", self.object_id, self.at)
    }
}

/// The daemon-less backend. Cheap to clone.
#[derive(Clone)]
pub struct ReplicaBackend {
    inner: Arc<Inner>,
}

struct Session {
    keys: Keys,
    author: String,
    sync: RelaySync,
}

#[derive(Default)]
struct State {
    /// Replayed object states (plain maps, as `/api/objects/:id` serves them).
    states: HashMap<String, Arc<Json>>,
    /// What each state was replayed from.
    inputs: HashMap<String, ReplayInput>,
    /// Ids the synced ledger says are gone.
    vanished: HashSet<String>,
    dirty: HashSet<String>,
    all_dirty: bool,
    /// What changed since the last query push.
    query_upserted: HashSet<String>,
    query_removed: HashSet<String>,
    /// Engine generation the query cache was loaded for (`needsColdLoad`).
    query_generation: Option<u64>,
    status: SyncStatus,
    seeded: bool,
    logging_out: bool,
    mutations: usize,
    shared_refreshes: usize,
    /// Objects awaiting their first replay this session, the next last
    /// ([`Inner::ensure`]); those already replayed are skipped.
    backlog: Vec<String>,
    /// The backlog's objects not replayed yet.
    waiting: HashSet<String>,
    /// How many objects the preparation in flight started with.
    preparing_total: u64,
    /// Bumped by every full pass: an older pass's background preparation stops.
    prepare_generation: u64,
    /// The pass a background preparation runs for.
    preparing_driver: Option<u64>,
    last_prepare_emit: i64,
    /// Replay memo rows not written yet.
    memo: Vec<CachedReplay>,
    /// `bootstrapped` as sync last reported it; shown once prepared.
    sync_bootstrapped: bool,
    /// `Some` on a lite replica ([`roostr_core::lite`]).
    lite: Option<LiteHold>,
    /// The spaces sync follows as shared (their own `#h` stream); every
    /// other space syncs in the personal stream.
    shared_spaces: HashSet<String>,
    /// This session's device record writer runs ([`Inner::start_device_records`]).
    device_writer: bool,
}

/// What a lite replica holds and is doing.
#[derive(Default)]
struct LiteHold {
    scopes: LiteScopes,
    /// The first directory fetch finished with every relay answering.
    checked: bool,
    /// Space downloads in flight.
    loading: Vec<SpaceLoad>,
    /// Ids fetched from the relays this session: each is asked once (sync
    /// follows it from then on, or no relay had it).
    asked: HashSet<String>,
    /// Bumped whenever the shared spaces (the keys a fetch asks under)
    /// change: a fetch that overlapped a change records nothing as asked.
    keys_generation: u64,
}

struct Inner {
    engine: Engine,
    store: Arc<dyn ChangeStore>,
    kv: Arc<dyn LocalKv>,
    keyring: Arc<KvSpaceKeyring>,
    label: String,
    session: Mutex<Option<Arc<Session>>>,
    state: Mutex<State>,
    listeners: Mutex<Vec<mpsc::UnboundedSender<BackendEvent>>>,
    start_lock: AsyncMutex<()>,
    ensure_lock: AsyncMutex<()>,
    /// A claim is a read/plan/commit transaction, not one engine call.
    mutation_lock: AsyncMutex<()>,
    /// The engine's query cache: pushes, queries and plans never interleave.
    cache_lock: AsyncMutex<()>,
    refresh_lock: AsyncMutex<()>,
    /// Owner deletion signals, applied one at a time.
    owner_signals: AsyncMutex<()>,
    /// A fresh replica starts lite ([`ReplicaBackend::lite_by_default`]).
    lite_default: AtomicBool,
    /// One lite directory fetch at a time ([`Inner::lite_frame`]).
    lite_frame_lock: AsyncMutex<()>,
    /// What this replica's device record calls it.
    device: DeviceInfo,
    /// One device record write at a time.
    device_lock: AsyncMutex<()>,
    /// The browser store again, concretely, for the one-time import of the
    /// Svelte site's vault (set by [`ReplicaBackend::open_browser`]).
    #[cfg(target_family = "wasm")]
    svelte_import: std::cell::OnceCell<roostr_replica_store::IdbChangeStore>,
}

fn now_ms() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn field<'a>(o: &'a Json, key: &str) -> &'a Json {
    &o["fields"][key]
}

fn fstr<'a>(o: &'a Json, key: &str) -> &'a str {
    field(o, key)["stringValue"].as_str().unwrap_or("")
}

fn fbool(o: &Json, key: &str) -> bool {
    field(o, key)["boolValue"].as_bool() == Some(true)
}

/// A Map value's member, whichever form the state carries: an object, or
/// `[key, value]` pairs (Backend+Repeat.swift `repeatEntries`).
fn map_member<'a>(value: &'a Json, key: &str) -> Option<&'a Json> {
    let entries = &value["mapValue"]["entries"];
    if let Some(map) = entries.as_object() {
        return map.get(key);
    }
    entries.as_array()?.iter().find_map(|pair| {
        let pair = pair.as_array().filter(|pair| pair.len() == 2)?;
        (pair[0].as_str() == Some(key)).then(|| &pair[1])
    })
}

/// `repeat`'s map entries.
fn repeat_entry<'a>(o: &'a Json, key: &str) -> Option<&'a Json> {
    map_member(field(o, "repeat"), key)
}

/// A space's directory id (`core/directory.odin` `directory_space_id`).
fn directory_id(space_id: &str) -> String {
    format!("{DIRECTORY_SPACE_PREFIX}{space_id}")
}

fn is_directory_id(id: &str) -> bool {
    id == DIRECTORY_ID || id.starts_with(DIRECTORY_SPACE_PREFIX)
}

/// A directory's entries under `prefix` (`space:`, `pin:`, `type:`,
/// `rel:`, `count:`): (the rest of the field key, its value).
fn directory_entries<'a>(
    dir: &'a Json,
    prefix: &'a str,
) -> impl Iterator<Item = (&'a str, &'a Json)> {
    dir["fields"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(move |(key, value)| Some((key.strip_prefix(prefix)?, value)))
}

/// A directory Map entry's string member ("" when unset).
fn member_str<'a>(entry: &'a Json, key: &str) -> &'a str {
    map_member(entry, key)
        .and_then(|v| v["stringValue"].as_str())
        .unwrap_or("")
}

/// A builder completed this directory (`version` 1).
fn directory_ready(dir: Option<&Arc<Json>>) -> bool {
    dir.is_some_and(|d| !deleted(d) && fint(d, "version") == Some(1))
}

/// Objects per type key a directory counts: every device's
/// `count:<type>:<device>` summed (the type key is all before the last `:`).
fn directory_counts(dir: &Json) -> HashMap<String, u64> {
    let mut sums: HashMap<String, i64> = HashMap::new();
    for (rest, value) in directory_entries(dir, "count:") {
        let Some((type_key, _device)) = rest.rsplit_once(':') else {
            continue;
        };
        *sums.entry(type_key.to_string()).or_default() += int_of(&value["intValue"]).unwrap_or(0);
    }
    sums.into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(key, n)| (key, n as u64))
        .collect()
}

/// Counted as an object of the vault, as the directories count (spaces and
/// bookkeeping are not).
fn counted(o: &Json) -> bool {
    !deleted(o) && type_key(o) != "channel" && !is_system_type(type_key(o))
}

/// protobuf int64 as JSON: a number, or a string.
fn int_of(v: &Json) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str()?.parse().ok())
}

fn fint(o: &Json, key: &str) -> Option<i64> {
    int_of(&field(o, key)["intValue"])
}

fn type_key(o: &Json) -> &str {
    o["typeKey"].as_str().unwrap_or("")
}

fn deleted(o: &Json) -> bool {
    o["deleted"].as_bool() == Some(true)
}

fn created_at(o: &Json) -> i64 {
    int_of(&o["createdAt"]).unwrap_or(0)
}

/// `valuesValue.items[].mapValue.entries` of a list field.
fn entries<'a>(o: &'a Json, key: &str) -> impl Iterator<Item = &'a Json> {
    field(o, key)["valuesValue"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| &item["mapValue"]["entries"])
        .filter(|e| e.is_object())
}

fn entry_str<'a>(entry: &'a Json, key: &str) -> &'a str {
    entry[key]["stringValue"].as_str().unwrap_or("")
}

/// objectId → owning space id ("" = personal). Channels own themselves.
fn space_of_state(o: &Json) -> String {
    if type_key(o) == "channel" {
        o["id"].as_str().unwrap_or("").to_string()
    } else {
        fstr(o, "channel").to_string()
    }
}

fn heads(o: &Json) -> Vec<&str> {
    o["heads"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect()
}

fn unavailable(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<StoreError>(),
        Some(StoreError::Unavailable(_) | StoreError::Closed)
    )
}

impl ReplicaBackend {
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new(parts: ReplicaParts) -> Self {
        let keyring = Arc::new(KvSpaceKeyring::new(parts.kv.clone()));
        Self {
            inner: Arc::new(Inner {
                engine: parts.engine,
                store: parts.store,
                kv: parts.kv,
                keyring,
                label: parts.label,
                session: Mutex::new(None),
                state: Mutex::new(State {
                    all_dirty: true,
                    ..State::default()
                }),
                listeners: Mutex::new(Vec::new()),
                start_lock: AsyncMutex::new(()),
                ensure_lock: AsyncMutex::new(()),
                mutation_lock: AsyncMutex::new(()),
                cache_lock: AsyncMutex::new(()),
                refresh_lock: AsyncMutex::new(()),
                owner_signals: AsyncMutex::new(()),
                lite_default: AtomicBool::new(false),
                lite_frame_lock: AsyncMutex::new(()),
                device: DeviceInfo::this_platform(),
                device_lock: AsyncMutex::new(()),
                #[cfg(target_family = "wasm")]
                svelte_import: std::cell::OnceCell::new(),
            }),
        }
    }

    /// A replica that holds nothing yet starts lite ([`roostr_core::lite`]):
    /// the browser and the iPhone. One that already holds a vault, or that
    /// chose before, keeps its choice ([`HOLDING`]).
    pub fn lite_by_default(self) -> Self {
        self.inner.lite_default.store(true, Ordering::Relaxed);
        self
    }

    /// The desktop replica in `dir` (created): `replica.sqlite` and the
    /// private `local.json` (key, space keys, relays; mode 0600).
    #[cfg(not(target_family = "wasm"))]
    pub async fn open_native(dir: &std::path::Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let store = roostr_replica_store::SqliteChangeStore::open(dir.join("replica.sqlite"))
            .await
            .map_err(|e| anyhow!("{e}"))?;
        let kv = crate::local::FileLocalKv::open(dir.join("local.json"))?;
        Ok(Self::new(ReplicaParts {
            engine: Engine::native().map_err(|e| anyhow!("{e}"))?,
            store: Arc::new(store),
            kv: Arc::new(kv),
            label: format!("this device's replica in {}", dir.display()),
        }))
    }

    /// The default desktop replica directory: `$ROOSTR_REPLICA_DIR`, else
    /// `<data dir>/Roostr/replica`.
    #[cfg(not(target_family = "wasm"))]
    pub fn default_dir() -> std::path::PathBuf {
        if let Some(dir) = std::env::var_os("ROOSTR_REPLICA_DIR") {
            return dir.into();
        }
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("Roostr")
            .join("replica")
    }

    /// The browser replica: IndexedDB `roostr-replica`, the page's
    /// `localStorage`, the site's engine.wasm from `source`. Its first start
    /// imports the Svelte site's IndexedDB vault (`roostr`) once; a browser
    /// with nothing to import starts lite.
    #[cfg(target_family = "wasm")]
    #[allow(clippy::arc_with_non_send_sync)]
    pub async fn open_browser(source: &roostr_engine::WebSource) -> Result<Self> {
        let engine = Engine::load(source).await.map_err(|e| anyhow!("{e}"))?;
        let store = roostr_replica_store::IdbChangeStore::open(
            roostr_replica_store::IdbChangeStore::DEFAULT_NAME,
        )
        .await
        .map_err(|e| anyhow!("{e}"))?;
        let backend = Self::new(ReplicaParts {
            engine,
            store: Arc::new(store.clone()),
            kv: Arc::new(crate::local::BrowserLocalKv),
            label: "this browser's replica".into(),
        })
        .lite_by_default();
        let _ = backend.inner.svelte_import.set(store);
        Ok(backend)
    }

    pub fn kv(&self) -> Arc<dyn LocalKv> {
        self.inner.kv.clone()
    }

    pub fn has_key(&self) -> bool {
        identity::load(&*self.inner.kv).is_some()
    }

    /// The current sync status (`backend.status`).
    pub fn status(&self) -> SyncStatus {
        self.inner.state.lock().status.clone()
    }

    /// Start the replica now (lazily done by every read).
    pub async fn start(&self) -> Result<()> {
        self.inner.ready().await.map(|_| ())
    }

    pub fn stop(&self) {
        self.inner.stop();
    }

    /// Reconnect every relay and reconcile (foregrounding, network changes).
    pub fn resume(&self) {
        if let Some(session) = self.inner.session() {
            session.sync.resume();
        }
    }

    /// Wait until sync has nothing in flight (tests, shutdown).
    pub async fn await_idle(&self) {
        if let Some(session) = self.inner.session() {
            session.sync.await_idle().await;
        }
    }

    /// Settings → "Reveal private key": `(nsec, hex)`.
    pub fn export_key(&self) -> Result<(String, String)> {
        let keys = identity::load(&*self.inner.kv).ok_or_else(|| anyhow!("no key"))?;
        Ok((keys.nsec(), keys.secret_hex()))
    }

    /// Log out on this device: refuses while changes are unpublished, then
    /// stops sync, erases the local replica and forgets the identity. The
    /// relays keep the encrypted history.
    pub async fn logout(&self) -> Result<()> {
        self.inner.logout().await
    }

    /// This identity's kind-0 profile from the relays (the cache when none answers).
    pub async fn profile(&self) -> Result<roostr_nostr::profile::Profile> {
        self.inner.profile().await
    }

    /// Merge-publish a kind-0 patch; resolves to the published profile.
    pub async fn save_profile(
        &self,
        patch: roostr_nostr::profile::Profile,
    ) -> Result<roostr_nostr::profile::Profile> {
        self.inner.save_profile(patch).await
    }

    pub fn cached_profile(&self) -> roostr_nostr::profile::Profile {
        self.inner.cached_profile()
    }

    pub fn relays(&self) -> Vec<String> {
        self.inner.relays()
    }

    pub fn keyring(&self) -> Arc<KvSpaceKeyring> {
        self.inner.keyring.clone()
    }

    /// This identity's keys, when it has one.
    pub fn keys(&self) -> Option<Keys> {
        identity::load(&*self.inner.kv)
    }

    /// Live recurring objects whose current occurrence is still ahead of
    /// `after` (epoch ms), soonest first, at most `limit`
    /// (Backend+Repeat.swift `upcomingOccurrences`). The engine keeps
    /// `repeat.next`; this only reads it.
    pub async fn upcoming_occurrences(&self, after: i64, limit: usize) -> Result<Vec<Occurrence>> {
        self.inner.upcoming_occurrences(after, limit).await
    }

    /// Write this device's record now (glonOdin `core/device.odin`; it is
    /// also written once the first sync settles, when what this device
    /// holds changes, and on a timer). `true` when something was due: the
    /// planner writes only what changed and `last_seen` alone at most
    /// every 30 minutes.
    pub async fn write_device_record(&self) -> Result<bool> {
        self.inner.ready().await?;
        self.inner.write_device_record().await
    }
}

impl Inner {
    fn session(&self) -> Option<Arc<Session>> {
        self.session.lock().clone()
    }

    fn emit(&self, event: BackendEvent) {
        self.listeners
            .lock()
            .retain(|tx| tx.unbounded_send(event.clone()).is_ok());
    }

    fn notify(&self, ids: &[String]) {
        for id in ids {
            self.emit(BackendEvent::Committed(id.clone()));
        }
    }

    fn relays(&self) -> Vec<String> {
        match self.kv.get(RELAYS) {
            // Never-configured devices get the default; a stored list (even
            // empty) is the user's choice.
            None => roostr_nostr::DEFAULT_RELAYS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            Some(text) => serde_json::from_str::<Option<Vec<String>>>(&text)
                .ok()
                .flatten()
                .unwrap_or_else(|| {
                    roostr_nostr::DEFAULT_RELAYS
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                }),
        }
    }

    // ------------------------------------------------------------ lifecycle

    /// backend.ts `start()`, once per identity; every read goes through it.
    #[allow(clippy::arc_with_non_send_sync)]
    async fn ready(self: &Arc<Self>) -> Result<Arc<Session>> {
        if let Some(session) = self.session() {
            return Ok(session);
        }
        let _start = self.start_lock.lock().await;
        if let Some(session) = self.session() {
            return Ok(session);
        }
        if self.state.lock().logging_out {
            bail!("Logout in progress");
        }
        let keys = identity::load(&*self.kv).ok_or_else(|| anyhow!("no key"))?;
        // Before the first sync: imported obligations are offered when it starts.
        #[cfg(target_family = "wasm")]
        self.import_svelte_vault(&keys).await;
        {
            let mut st = self.state.lock();
            st.all_dirty = true;
            st.seeded = false;
        }
        let session = Arc::new(self.new_session(keys));
        self.choose_holding(&session).await?;
        *self.session.lock() = Some(session.clone());
        self.refresh_shared().await;
        // Converge the bundled catalog: a replica that already walked the
        // vault seeds now; a fresh one when its first history pass ends.
        if self.store.bootstrapped().await.unwrap_or(false) {
            self.seed_defaults();
        }
        self.start_sync(&session).await;
        self.spawn_refresh();
        Ok(session)
    }

    /// The Svelte site's IndexedDB vault, imported into this store once
    /// (`roostr_replica_store::svelte_import`). Never fatal: a failed import
    /// leaves its flag unset and is retried on the next start.
    #[cfg(target_family = "wasm")]
    async fn import_svelte_vault(&self, keys: &Keys) {
        use roostr_replica_store::svelte_import::{self, SvelteImport};
        let Some(store) = self.svelte_import.get() else {
            return;
        };
        // The boot screen's line while it runs (`boot_progress` shows a
        // backfill's detail): reading, then the changes as they are written.
        let progress = |written: u64, total: u64| {
            let detail = if total == 0 {
                "Importing this browser's earlier vault".to_string()
            } else {
                format!(
                    "Importing · {} of {} changes",
                    roostr_core::sync_status::count(written),
                    roostr_core::sync_status::count(total)
                )
            };
            self.emit_detail(Some(detail));
        };
        let outcome = svelte_import::import_svelte_vault(
            store,
            svelte_import::SVELTE_DB,
            &self.engine,
            keys.pubkey(),
            &keys.author_id(),
            &progress,
        )
        .await;
        if self.state.lock().status.detail.is_some() {
            self.emit_detail(None);
        }
        match outcome {
            Ok(SvelteImport::Imported(stats)) => {
                log::info!("[replica] imported the Svelte site's vault: {stats:?}");
            }
            Ok(SvelteImport::Foreign) => {
                log::warn!(
                    "[replica] IndexedDB `roostr` holds another identity's vault; left alone"
                );
            }
            Ok(_) => {}
            Err(error) => {
                log::warn!(
                    "[replica] Svelte vault import failed, retried on the next start: {error}"
                );
            }
        }
    }

    #[allow(clippy::arc_with_non_send_sync)]
    fn new_session(self: &Arc<Self>, keys: Keys) -> Session {
        let host: Arc<dyn SyncHost> = Arc::new(Host(Arc::downgrade(self)));
        let sync = RelaySync::new(
            self.engine.clone(),
            self.store.clone(),
            keys.clone(),
            self.relays(),
            self.keyring.clone(),
            self.kv.clone(),
            host,
        );
        Session {
            author: keys.author_id(),
            keys,
            sync,
        }
    }

    fn stop(&self) {
        if let Some(session) = self.session.lock().take() {
            session.sync.stop();
        }
        // The next start prepares again from what the memo then holds.
        let mut st = self.state.lock();
        st.seeded = false;
        st.device_writer = false;
        st.prepare_generation += 1;
    }

    /// Relays changed: the running sync moves to the new list.
    #[allow(clippy::arc_with_non_send_sync)]
    async fn restart_sync(self: &Arc<Self>) {
        let _start = self.start_lock.lock().await;
        let Some(old) = self.session.lock().take() else {
            return;
        };
        old.sync.stop();
        let session = Arc::new(self.new_session(old.keys.clone()));
        self.state.lock().device_writer = false;
        if let Err(error) = self.choose_holding(&session).await {
            log::warn!("[replica] holding after a relay change: {error:#}");
        }
        *self.session.lock() = Some(session.clone());
        self.refresh_shared().await;
        self.start_sync(&session).await;
    }

    /// Lite or full, decided on a replica's first start ([`HOLDING`]): lite
    /// when this platform starts lite ([`ReplicaBackend::lite_by_default`])
    /// and the store holds nothing yet; a replica already holding a vault
    /// stays full. A lite session starts with what it holds whole.
    async fn choose_holding(&self, session: &Session) -> Result<()> {
        let lite = match self.kv.get(HOLDING).as_deref() {
            Some(holding) => holding == "lite",
            None => {
                let fresh = self.lite_default.load(Ordering::Relaxed)
                    && !self.store.bootstrapped().await?
                    && self.store.object_ids().await?.is_empty();
                self.kv
                    .set(HOLDING, Some(if fresh { "lite" } else { "full" }));
                fresh
            }
        };
        if !lite {
            self.state.lock().lite = None;
            return Ok(());
        }
        let scopes: LiteScopes = self
            .store
            .meta(LITE_SCOPES.to_string())
            .await?
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        {
            let mut st = self.state.lock();
            match st.lite.as_mut() {
                Some(lite) => lite.scopes = scopes.clone(),
                None => {
                    st.lite = Some(LiteHold {
                        scopes: scopes.clone(),
                        ..LiteHold::default()
                    });
                }
            }
        }
        session.sync.set_lite(Some(scopes)).await?;
        Ok(())
    }

    /// Starts `session`'s sync - once every object is prepared: sync asks
    /// which space an object is in (where its writes go, whose orphans this
    /// device may rebase), which only its state answers. A lite replica
    /// then fetches its directories.
    async fn start_sync(self: &Arc<Self>, session: &Arc<Session>) {
        if self.state.lock().waiting.is_empty() {
            match session.sync.start().await {
                Ok(()) => self.spawn_lite_frame(),
                Err(error) => log::warn!("[replica] sync start failed: {error:#}"),
            }
            return;
        }
        let (this, session) = (self.clone(), session.clone());
        let start: BoxFuture<()> = Box::pin(async move {
            let started = match this.ensure_complete().await {
                Ok(()) => session.sync.start().await,
                Err(error) => Err(error),
            };
            match started {
                Ok(()) => this.spawn_lite_frame(),
                Err(error) => log::warn!("[replica] sync start failed: {error:#}"),
            }
        });
        rt::spawn(start);
    }

    /// Idempotent by fixture: a converged space commits nothing; once per
    /// session. Never on a lite replica: converging reads every space.
    fn seed_defaults(self: &Arc<Self>) {
        {
            let mut st = self.state.lock();
            if st.seeded
                || st.logging_out
                || st.lite.is_some()
                || self.session.lock().is_none()
            {
                return;
            }
            st.seeded = true;
        }
        let this = self.clone();
        rt::spawn(async move {
            // Converging reads every space: none may be still unprepared.
            let result = match this.ensure_complete().await {
                Ok(()) => {
                    this.mutate_inner("bootstrap_space_defaults", JsonMap::new(), false)
                        .await
                }
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                log::warn!("[replica] bootstrap_space_defaults failed: {error:#}");
            }
        });
    }

    async fn logout(self: &Arc<Self>) -> Result<()> {
        {
            let mut st = self.state.lock();
            if st.logging_out || st.mutations > 0 || st.shared_refreshes > 0 {
                bail!("Wait for the current save before logging out");
            }
            st.logging_out = true;
        }
        let result = async {
            let pending = self.store.pending_publishes().await?;
            if !pending.is_empty() {
                bail!(
                    "Unpublished changes remain on this device. Connect to a relay or explicitly export them before logging out."
                );
            }
            self.stop();
            self.store.clear().await?;
            // The next identity decides lite or full afresh.
            for key in [SPACE_KEYS, PROFILE, HOLDING, "roostr-shared-queued"] {
                self.kv.set(key, None);
            }
            identity::clear(&*self.kv);
            self.engine.reset_all().await;
            let mut st = self.state.lock();
            let status = SyncStatus::default();
            *st = State {
                all_dirty: true,
                logging_out: true,
                status,
                // A preparation still running for the old vault stops.
                prepare_generation: st.prepare_generation + 1,
                ..State::default()
            };
            Ok(())
        }
        .await;
        self.state.lock().logging_out = false;
        result
    }

    // ---------------------------------------------------------------- states

    /// Recompute dirty states: a read waits for the pass already running and
    /// runs at most one more, covering everything dirtied before it was made.
    ///
    /// Objects with no state this session that no memo row serves (the
    /// first open after an import or an engine update: the whole vault) are
    /// the backlog. A full pass replays it for [`PREPARE_INLINE_MS`] at most
    /// and leaves the rest to [`Inner::start_preparing`], so the workspace
    /// opens while the vault is still being prepared: reads see the objects
    /// prepared so far, what is asked for by id goes first
    /// ([`Inner::ensure_ids`]), and the status shows the count and withholds
    /// `bootstrapped` until [`BackendEvent::Prepared`].
    async fn ensure(self: &Arc<Self>) -> Result<()> {
        let _pass = self.ensure_lock.lock().await;
        let needed = {
            let st = self.state.lock();
            st.all_dirty || !st.dirty.is_empty()
        };
        if needed {
            self.rebuild_states().await?;
        }
        Ok(())
    }

    /// [`Inner::ensure`] and the whole backlog: for answers about every
    /// object (the sync digest, serving, reminders, the catalog seeding).
    async fn ensure_complete(self: &Arc<Self>) -> Result<()> {
        loop {
            if self.session().is_none() {
                // Stopped (a logout, the tab going away): keep what is prepared.
                self.save_memo(true).await;
                bail!("the replica stopped");
            }
            self.ensure().await?;
            let more = {
                let _pass = self.ensure_lock.lock().await;
                self.prepare(PREPARE_STEP_MS, None).await?
            };
            self.save_memo(!more).await;
            if !more {
                return Ok(());
            }
            // The page renders between steps.
            roostr_nostr::sleep(std::time::Duration::ZERO).await;
        }
    }

    /// [`Inner::ensure`], then those of `ids` still in the backlog, now.
    async fn ensure_ids(self: &Arc<Self>, ids: &[String]) -> Result<()> {
        self.ensure().await?;
        let wanted: Vec<String> = {
            let st = self.state.lock();
            ids.iter()
                .filter(|id| st.waiting.contains(*id))
                .cloned()
                .collect()
        };
        if wanted.is_empty() {
            return Ok(());
        }
        {
            let _pass = self.ensure_lock.lock().await;
            self.prepare(i64::MAX, Some(&wanted)).await?;
        }
        self.save_memo(true).await;
        Ok(())
    }

    async fn rebuild_states(self: &Arc<Self>) -> Result<()> {
        let rebuilt = std::mem::take(&mut self.state.lock().all_dirty);
        if rebuilt {
            self.state.lock().dirty.clear();
            let misses = match self.restore_cached_states().await {
                Ok(ids) => ids,
                Err(error) => {
                    self.state.lock().all_dirty = true;
                    return Err(error);
                }
            };
            let mut st = self.state.lock();
            st.prepare_generation += 1;
            st.waiting = misses.iter().cloned().collect();
            st.preparing_total = misses.len() as u64;
            st.backlog = misses;
            st.backlog.reverse();
            st.vanished.clear();
        }
        let ids: Vec<String> = self.state.lock().dirty.drain().collect();
        if !ids.is_empty() {
            let pending = match self.store.pending_change_ids().await {
                Ok(pending) => pending,
                Err(error) => {
                    self.requeue(rebuilt, &ids);
                    return Err(error.into());
                }
            };
            for (index, id) in ids.iter().enumerate() {
                match self.replay_one(id, &pending).await {
                    Ok(Some(row)) => self.state.lock().memo.push(row),
                    Ok(None) => {}
                    Err(error) if unavailable(&error) => {
                        // An interrupted pass must run again, or objects it
                        // never reached stay missing for the session.
                        self.requeue(rebuilt, &ids[index..]);
                        return Err(error);
                    }
                    // One malformed legacy object must never brick the vault.
                    Err(error) => log::warn!("[replica] replay failed for {id}: {error:#}"),
                }
                self.state.lock().waiting.remove(id);
            }
        }
        if rebuilt {
            // The vanish ledger and the spaces come first (see
            // `restore_cached_states`): the sweep below reads the ledger.
            if let Err(error) = self.prepare(PREPARE_INLINE_MS, None).await {
                self.state.lock().all_dirty = true;
                return Err(error);
            }
        }
        self.save_memo(true).await;
        self.enforce_vanished(rebuilt, &ids).await?;
        // A directory replayed: what a lite replica draws may have changed.
        if self.state.lock().lite.is_some() && ids.iter().any(|id| is_directory_id(id)) {
            self.emit_status(true);
        }
        if rebuilt && !self.state.lock().waiting.is_empty() {
            self.emit_status(true);
            self.start_preparing();
        }
        Ok(())
    }

    fn requeue(&self, rebuilt: bool, ids: &[String]) {
        let mut st = self.state.lock();
        if rebuilt {
            st.all_dirty = true;
        }
        st.dirty.extend(ids.iter().cloned());
    }

    /// Replays backlog objects - `only` those, else the next ones - until
    /// `budget_ms` passed (one at least); whether any are left. Each lands
    /// in the view as it is replayed. Caller holds `ensure_lock`.
    async fn prepare(self: &Arc<Self>, budget_ms: i64, only: Option<&[String]>) -> Result<bool> {
        let started = now_ms();
        let mut only = only.map(|ids| ids.iter());
        let mut pending: Option<HashSet<String>> = None;
        loop {
            let next = {
                let mut st = self.state.lock();
                loop {
                    let id = match only.as_mut() {
                        Some(ids) => ids.next().cloned(),
                        None => st.backlog.pop(),
                    };
                    match id {
                        Some(id) if !st.waiting.contains(&id) => continue,
                        id => break id,
                    }
                }
            };
            let Some(id) = next else {
                break;
            };
            if pending.is_none() {
                pending = Some(self.store.pending_change_ids().await?);
            }
            match self
                .replay_one(&id, pending.as_ref().expect("read above"))
                .await
            {
                Ok(Some(row)) => self.state.lock().memo.push(row),
                Ok(None) => {}
                Err(error) if unavailable(&error) => {
                    // Still waiting: the next step starts with it.
                    self.state.lock().backlog.push(id);
                    return Err(error);
                }
                Err(error) => log::warn!("[replica] replay failed for {id}: {error:#}"),
            }
            self.state.lock().waiting.remove(&id);
            // The ledger sweeps every state prepared so far; anything else
            // is checked against it (no refresh: the pass's own follows).
            if id == VANISH_LOG_ID {
                self.enforce_vanished(true, &[]).await?;
            } else {
                self.enforce_vanished(false, std::slice::from_ref(&id))
                    .await?;
            }
            let touched = [id];
            self.notify(&touched);
            if now_ms() - started >= budget_ms {
                break;
            }
        }
        Ok(!self.state.lock().waiting.is_empty())
    }

    /// The rest of the backlog in the background, a step at a time with the
    /// page rendering in between and the memo written as it goes (a reload
    /// resumes where this left off); then reads see the whole vault again.
    fn start_preparing(self: &Arc<Self>) {
        let generation = {
            let mut st = self.state.lock();
            if st.preparing_driver == Some(st.prepare_generation) {
                return;
            }
            st.preparing_driver = Some(st.prepare_generation);
            st.prepare_generation
        };
        let this = self.clone();
        let driver: BoxFuture<()> = Box::pin(async move {
            let current = || {
                let st = this.state.lock();
                st.prepare_generation == generation && !st.logging_out
            };
            let mut wait = std::time::Duration::ZERO;
            while current() {
                roostr_nostr::sleep(wait).await;
                let step = {
                    let _pass = this.ensure_lock.lock().await;
                    if !current() {
                        break;
                    }
                    this.prepare(PREPARE_STEP_MS, None).await
                };
                match step {
                    Ok(true) => {
                        wait = std::time::Duration::ZERO;
                        this.save_memo(false).await;
                        this.emit_status(false);
                    }
                    Ok(false) => break,
                    Err(error) => {
                        log::warn!("[replica] preparing paused: {error:#}");
                        wait = PREPARE_RETRY;
                    }
                }
            }
            this.save_memo(true).await;
            let finished = {
                let mut st = this.state.lock();
                if st.preparing_driver == Some(generation) {
                    st.preparing_driver = None;
                }
                let finished = st.prepare_generation == generation && st.waiting.is_empty();
                if finished {
                    st.preparing_total = 0;
                }
                finished
            };
            if finished {
                this.emit_status(true);
                this.emit(BackendEvent::Prepared);
                // Shared spaces read every object of theirs.
                this.spawn_refresh();
            }
        });
        rt::spawn(driver);
    }

    /// Writes the memo rows replayed since the last write: once enough
    /// piled up, or all of them when `all`.
    async fn save_memo(&self, all: bool) {
        let rows = {
            let mut st = self.state.lock();
            if st.memo.is_empty() || (!all && st.memo.len() < MEMO_ROWS_PER_WRITE) {
                return;
            }
            std::mem::take(&mut st.memo)
        };
        if let Err(error) = self
            .store
            .put_cached_replays(rows, self.replay_version())
            .await
        {
            log::debug!("[replica] replay memo not written: {error}");
        }
    }

    /// Shows `detail` as what the replica is doing before its sync runs
    /// (a backfill's detail, which the boot screen shows), or clears it.
    #[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
    fn emit_detail(&self, detail: Option<String>) {
        let next = {
            let mut st = self.state.lock();
            st.status.phase = if detail.is_some() {
                SyncPhase::Backfill
            } else {
                SyncPhase::Idle
            };
            st.status.detail = detail;
            st.status.lite = Self::lite_status(&st);
            st.status.clone()
        };
        self.emit(BackendEvent::Status(next));
    }

    /// `(prepared, total)` of the preparation in flight.
    fn preparing(st: &State) -> Option<(u64, u64)> {
        (st.preparing_total > 0 && !st.waiting.is_empty()).then(|| {
            let left = st.waiting.len() as u64;
            (st.preparing_total.saturating_sub(left), st.preparing_total)
        })
    }

    /// Re-sends the status with the preparation folded in: at most every
    /// [`PREPARE_EMIT_MS`] unless `now`.
    fn emit_status(&self, now: bool) {
        let next = {
            let mut st = self.state.lock();
            let at = now_ms();
            if !now && at - st.last_prepare_emit < PREPARE_EMIT_MS {
                return;
            }
            st.last_prepare_emit = at;
            let preparing = Self::preparing(&st);
            st.status.preparing = preparing;
            st.status.bootstrapped = st.sync_bootstrapped && preparing.is_none();
            st.status.lite = Self::lite_status(&st);
            st.status.clone()
        };
        self.emit(BackendEvent::Status(next));
    }

    fn replay_version(&self) -> String {
        format!("{}/{REPLAY_ROW_VERSION}", self.engine.source_fingerprint())
    }

    /// Every persisted replay whose input is exactly what the store holds now
    /// under this engine; returns the ids that still need a replay, in the
    /// order they are prepared: the vanish ledger and the keyring's spaces
    /// (the sweep and the shared refresh read them), then the objects with
    /// the fewest changes - spaces, types and properties before long pages,
    /// and the most objects in view soonest.
    async fn restore_cached_states(&self) -> Result<Vec<String>> {
        let inputs = self.store.replay_inputs().await?;
        let wanted: Vec<String> = {
            let mut st = self.state.lock();
            // States of objects the store no longer holds go.
            let gone: Vec<String> = st
                .states
                .keys()
                .filter(|id| !inputs.contains_key(*id))
                .cloned()
                .collect();
            for id in gone {
                st.states.remove(&id);
                st.inputs.remove(&id);
                st.query_removed.insert(id);
            }
            inputs
                .iter()
                .filter(|(id, input)| st.inputs.get(*id) != Some(input))
                .map(|(id, _)| id.clone())
                .collect()
        };
        let rows = self
            .store
            .cached_replays(wanted.clone(), self.replay_version())
            .await?;
        let mut misses = Vec::new();
        let mut st = self.state.lock();
        for id in wanted {
            let input = &inputs[&id];
            let state = rows
                .get(&id)
                .filter(|row| row.serves(input))
                .and_then(|row| serde_json::from_slice::<Json>(&row.state).ok());
            match state {
                Some(state) => {
                    st.states.insert(id.clone(), Arc::new(state));
                    st.inputs.insert(id, input.clone());
                }
                None => misses.push(id),
            }
        }
        drop(st);
        let first: HashSet<String> = self
            .keyring
            .all()
            .map(|keys| keys.into_keys().collect())
            .unwrap_or_default();
        misses.sort_by_cached_key(|id| {
            let rank = if id == VANISH_LOG_ID {
                0
            } else if first.contains(id) {
                1
            } else {
                2
            };
            (rank, inputs[id].change_count, id.clone())
        });
        Ok(misses)
    }

    /// Replays one object (current base + live deltas); the memo row to write.
    async fn replay_one(
        &self,
        id: &str,
        pending: &HashSet<String>,
    ) -> Result<Option<CachedReplay>> {
        let held = self.store.object_inputs(id.to_string()).await?;
        let input = ReplayInput::new(
            held.changes.iter().map(|c| c.id.as_str()),
            held.checkpoint.as_ref().map_or("", |c| c.hash.as_str()),
            held.bases.iter().map(|b| b.hash.as_str()),
        );
        if held.changes.is_empty() && held.bases.is_empty() && held.checkpoint.is_none() {
            let mut st = self.state.lock();
            if st.states.remove(id).is_some() {
                st.query_removed.insert(id.to_string());
            }
            st.inputs.remove(id);
            return Ok(None);
        }
        if self.state.lock().inputs.get(id) == Some(&input) {
            return Ok(None);
        }
        let inputs = state_inputs(held.changes, held.bases, held.checkpoint, pending)?;
        let Some(state) = self.engine.replay_from_base(&inputs).await? else {
            return Ok(None);
        };
        let bytes = serde_json::to_vec(&state)?;
        let mut st = self.state.lock();
        let changed = st
            .states
            .get(id)
            .is_none_or(|previous| heads(previous) != heads(&state));
        // Same heads, same state: a relay echoing this device's own change
        // leaves the engine's cache (and any upsert owed for it) as it is.
        if changed {
            st.query_upserted.insert(id.to_string());
        }
        st.states.insert(id.to_string(), Arc::new(state));
        st.inputs.insert(id.to_string(), input.clone());
        Ok(Some(CachedReplay {
            object_id: id.to_string(),
            input,
            state: bytes,
        }))
    }

    /// The ledger wins over whatever replayed (backend.ts `enforceVanished`):
    /// every state when `all` (or the ledger is among `touched`), else
    /// `touched`. A full pass starts its vanished set afresh
    /// (`rebuild_states`); objects it prepares later add to it.
    async fn enforce_vanished(self: &Arc<Self>, all: bool, touched: &[String]) -> Result<()> {
        let ledger_changed = touched.iter().any(|id| id == VANISH_LOG_ID);
        let sweep = all || ledger_changed;
        let (ledger, candidates) = {
            let st = self.state.lock();
            if !sweep && st.vanished.is_empty() {
                return Ok(());
            }
            let pick = |o: &Arc<Json>| VanishCandidate {
                id: o["id"].as_str().unwrap_or("").to_string(),
                channel: fstr(o, "channel").to_string(),
            };
            let candidates: Vec<VanishCandidate> = if sweep {
                st.states.values().map(pick).collect()
            } else {
                touched
                    .iter()
                    .filter_map(|id| st.states.get(id))
                    .map(pick)
                    .collect()
            };
            (st.states.get(VANISH_LOG_ID).cloned(), candidates)
        };
        let entries = self.engine.vanished(ledger.as_deref(), &candidates).await?;
        {
            let mut st = self.state.lock();
            for entry in entries {
                if st.states.remove(&entry.object_id).is_some() {
                    st.query_removed.insert(entry.object_id.clone());
                }
                st.inputs.remove(&entry.object_id);
                st.vanished.insert(entry.object_id);
            }
        }
        if ledger_changed && !all {
            self.spawn_refresh();
        }
        Ok(())
    }

    fn states_snapshot(&self) -> Vec<Arc<Json>> {
        self.state.lock().states.values().cloned().collect()
    }

    // --------------------------------------------------------------- shared

    /// [`Inner::refresh_shared`] in the background. Boxed with its bounds
    /// spelled out: the refresh replays, and a replay can schedule a refresh.
    fn spawn_refresh(self: &Arc<Self>) {
        let this = self.clone();
        let refresh: BoxFuture<()> = Box::pin(async move { this.refresh_shared().await });
        rt::spawn(refresh);
    }

    /// backend.ts `refreshShared`: hand the sync layer the shared view built
    /// from the keyring and member fields, publish allowlists, owe the
    /// h-deletions of vanished spaces, send owner key wraps, and queue each
    /// shared space's history under its key.
    async fn refresh_shared(self: &Arc<Self>) {
        let Some(session) = self.session() else {
            return;
        };
        if self.state.lock().logging_out {
            return;
        }
        let _one = self.refresh_lock.lock().await;
        self.state.lock().shared_refreshes += 1;
        if let Err(error) = self.refresh_shared_inner(&session).await {
            log::warn!("[replica] shared-space refresh failed: {error:#}");
        }
        self.state.lock().shared_refreshes -= 1;
    }

    async fn refresh_shared_inner(self: &Arc<Self>, session: &Session) -> Result<()> {
        let my_pk = session.keys.pubkey().to_string();
        let keys = self.keyring.all().map_err(|e| anyhow!("{e}"))?;
        // The spaces' states and the ledger, prepared first if still waiting;
        // a preparation in flight refreshes again once it has every object.
        let mut ids: Vec<String> = keys.keys().cloned().collect();
        ids.push(VANISH_LOG_ID.to_string());
        self.ensure_ids(&ids).await?;
        let mut infos = Vec::new();
        let mut members_by_space: HashMap<String, Vec<String>> = HashMap::new();
        let mut name_by_space: HashMap<String, String> = HashMap::new();
        let (vanished, ledger) = {
            let st = self.state.lock();
            for (space_id, entry) in &keys {
                if entry.key.len() != 64 || st.vanished.contains(space_id) {
                    continue;
                }
                let state = st.states.get(space_id);
                let mut writers = vec![entry.owner.clone().unwrap_or_else(|| my_pk.clone())];
                let mut member_hexes = Vec::new();
                for e in state.into_iter().flat_map(|s| entries(s, "members")) {
                    if let Some(hex) = roostr_nostr::keys::npub_to_hex(entry_str(e, "npub")) {
                        if entry_str(e, "role") == "writer" && !writers.contains(&hex) {
                            writers.push(hex.clone());
                        }
                        member_hexes.push(hex);
                    }
                }
                if member_hexes.is_empty() && entry.owner.is_none() {
                    continue;
                }
                members_by_space.insert(space_id.clone(), member_hexes);
                name_by_space.insert(
                    space_id.clone(),
                    state.map_or("", |s| fstr(s, "name")).to_string(),
                );
                infos.push(SharedSpaceInfo {
                    space_id: space_id.clone(),
                    key_hex: entry.key.clone(),
                    key_id: entry.key_id as i64,
                    writers,
                    owner: entry.owner.clone(),
                });
            }
            (
                !st.vanished.is_empty(),
                st.states.get(VANISH_LOG_ID).cloned(),
            )
        };
        let shared_changed = {
            let mut guard = self.state.lock();
            let st = &mut *guard;
            let shared: HashSet<String> = infos.iter().map(|i| i.space_id.clone()).collect();
            let changed = shared != st.shared_spaces;
            // Lite: an id asked before a space's key was installed may be
            // that space's: ask again under the new key.
            if changed && let Some(lite) = st.lite.as_mut() {
                lite.asked.clear();
                lite.keys_generation += 1;
            }
            st.shared_spaces = shared;
            changed
        };
        session.sync.set_shared_spaces(infos.clone()).await?;
        if shared_changed && let Some(lite) = self.state.lock().lite.as_mut() {
            lite.asked.clear();
            lite.keys_generation += 1;
        }

        // A vanished space whose key is still here owes the relays one h-deletion.
        if vanished {
            let ledger_entries = self.engine.vanished(ledger.as_deref(), &[]).await?;
            for entry in ledger_entries {
                let Some(key) = keys.get(&entry.object_id) else {
                    continue;
                };
                if entry.left || key.key.len() != 64 {
                    continue;
                }
                session
                    .sync
                    .publish_space_deletion(entry.object_id.clone(), key.key.clone())
                    .await;
            }
        }

        // Owner duty: every member holds the current key.
        for info in &infos {
            if info.owner.as_deref().is_some_and(|o| o != my_pk) {
                continue;
            }
            let members = members_by_space
                .get(&info.space_id)
                .cloned()
                .unwrap_or_default();
            if members.is_empty() {
                continue;
            }
            session
                .sync
                .send_invite_wraps(
                    info.space_id.clone(),
                    info.key_hex.clone(),
                    info.key_id,
                    name_by_space
                        .get(&info.space_id)
                        .cloned()
                        .unwrap_or_default(),
                    members,
                )
                .await;
        }

        let mut owned_writers: Vec<String> = Vec::new();
        let mut any_owned = false;
        // A lite replica that does not hold an owned space's object yet does
        // not know its members: publishing would shut them out.
        let mut members_unknown = false;
        for info in infos
            .iter()
            .filter(|i| i.owner.as_deref().is_none_or(|o| o == my_pk))
        {
            any_owned = true;
            {
                let st = self.state.lock();
                members_unknown |= st.lite.is_some() && !st.states.contains_key(&info.space_id);
            }
            for w in &info.writers {
                if !owned_writers.contains(w) {
                    owned_writers.push(w.clone());
                }
            }
        }
        if any_owned && !members_unknown {
            session.sync.publish_allowlist(owned_writers).await;
        }

        // Publication acknowledgements, not enqueue markers, decide durability.
        for info in &infos {
            let objects: Vec<String> = {
                let st = self.state.lock();
                st.states
                    .iter()
                    .filter(|(_, s)| space_of_state(s) == info.space_id)
                    .map(|(id, _)| id.clone())
                    .collect()
            };
            let owed = self
                .store
                .unpublished_changes(objects, format!("{}/{}/", info.space_id, info.key_id))
                .await?;
            let queued = owed.len();
            for change in owed {
                session
                    .sync
                    .publish(change.bytes, change.id, change.object_id, change.b)
                    .await?;
            }
            if queued > 0 {
                log::info!(
                    "[replica] space {} shared (key #{}): queued {queued} change(s)",
                    &info.space_id[..info.space_id.len().min(8)],
                    info.key_id
                );
            }
        }
        // Lite: spaces whose key just arrived bring their directories.
        self.spawn_lite_frame();
        Ok(())
    }

    // ----------------------------------------------------------------- lite

    fn is_lite(&self) -> bool {
        self.state.lock().lite.is_some()
    }

    /// This install's stable id ([`DEVICE_ID`]), made on first use.
    fn device_id(&self) -> String {
        if let Some(id) = self.kv.get(DEVICE_ID).filter(|id| !id.is_empty()) {
            return id;
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.kv.set(DEVICE_ID, Some(&id));
        id
    }

    /// The spaces a lite replica knows of: those whose object it holds and
    /// those its personal directory lists (vanished ones left out), sorted.
    fn known_spaces(st: &State) -> Vec<String> {
        let mut spaces: BTreeSet<String> = st
            .states
            .values()
            .filter(|o| !deleted(o) && type_key(o) == "channel")
            .filter_map(|o| o["id"].as_str().map(str::to_string))
            .collect();
        if let Some(dir) = st.states.get(DIRECTORY_ID) {
            spaces.extend(directory_entries(dir, "space:").map(|(id, _)| id.to_string()));
        }
        spaces.retain(|space| !st.vanished.contains(space));
        spaces.into_iter().collect()
    }

    /// A lite replica holds `space_id` whole: its own stream when it is
    /// shared here, else the personal stream it syncs in - and no download
    /// of it is still running.
    fn space_whole(st: &State, lite: &LiteHold, space_id: &str) -> bool {
        if lite.loading.iter().any(|l| l.space_id == space_id) {
            return false;
        }
        if st.shared_spaces.contains(space_id) {
            lite.scopes.spaces.contains(space_id)
        } else {
            lite.scopes.personal
        }
    }

    /// [`SyncStatus::lite`]: `None` on a full replica.
    fn lite_status(st: &State) -> Option<LiteStatus> {
        let lite = st.lite.as_ref()?;
        let spaces = Self::known_spaces(st);
        let unready_spaces = if lite.checked {
            spaces
                .iter()
                .filter(|space| !directory_ready(st.states.get(&directory_id(space))))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        Some(LiteStatus {
            ready: directory_ready(st.states.get(DIRECTORY_ID)),
            checked: lite.checked,
            unready_spaces,
            full_spaces: spaces
                .iter()
                .filter(|space| Self::space_whole(st, lite, space))
                .cloned()
                .collect(),
            loading: lite.loading.clone(),
        })
    }

    /// A lite replica's pins of `space_id`: the held objects' own (`held`,
    /// as their states order them) and, for every object not held, its
    /// directory's `pin:` entry; by position, then id (pins.ts).
    fn lite_pins(st: &State, space_id: &str, held: &[String]) -> Vec<String> {
        let mut pins: Vec<(String, String)> = held
            .iter()
            .map(|id| {
                let order = st
                    .states
                    .get(id)
                    .map_or("", |s| fstr(s, roostr_core::pins::PIN_KEY));
                (order.to_string(), id.clone())
            })
            .collect();
        if let Some(dir) = st.states.get(&directory_id(space_id)) {
            for (id, entry) in directory_entries(dir, "pin:") {
                if st.states.contains_key(id) || st.vanished.contains(id) {
                    continue;
                }
                pins.push((member_str(entry, "order").to_string(), id.to_string()));
            }
        }
        pins.sort();
        pins.dedup_by(|a, b| a.1 == b.1);
        pins.into_iter().map(|(_, id)| id).collect()
    }

    /// The lite directory fetch in the background ([`Inner::lite_frame`]).
    fn spawn_lite_frame(self: &Arc<Self>) {
        if !self.is_lite() {
            return;
        }
        let this = self.clone();
        let frame: BoxFuture<()> = Box::pin(async move {
            if let Err(error) = this.lite_frame().await {
                log::warn!("[replica] lite directories: {error:#}");
            }
        });
        rt::spawn(frame);
    }

    /// What a lite replica draws from: the personal directory and the
    /// vanish ledger, every installed or listed space with its directory,
    /// then the types and properties those directories list (object pages
    /// and the type list read them). Each id is asked of the relays once a
    /// session. A vault the relays have never seen (a new key) has nothing
    /// to leave out: this device then holds it whole.
    async fn lite_frame(self: &Arc<Self>) -> Result<()> {
        let _one = self.lite_frame_lock.lock().await;
        let Some(session) = self.session() else {
            return Ok(());
        };
        if !self.is_lite() || !session.sync.running() {
            return Ok(());
        }
        let keyed: Vec<String> = self
            .keyring
            .all()
            .map_err(|e| anyhow!("{e}"))?
            .into_keys()
            .collect();
        let mut ids = vec![DIRECTORY_ID.to_string(), VANISH_LOG_ID.to_string()];
        for space in &keyed {
            ids.push(space.clone());
            ids.push(directory_id(space));
        }
        let mut complete = self.lite_fetch(&session, ids).await?;
        let listed: Vec<String> = {
            let st = self.state.lock();
            st.states
                .get(DIRECTORY_ID)
                .map(|dir| {
                    directory_entries(dir, "space:")
                        .map(|(id, _)| id.to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        // The device records the personal directory lists come with it.
        let ids = listed
            .iter()
            .flat_map(|space| [space.clone(), directory_id(space)])
            .chain(self.listed_devices())
            .collect();
        complete &= self.lite_fetch(&session, ids).await?;
        let nothing = {
            let st = self.state.lock();
            !st.states.contains_key(DIRECTORY_ID)
                && st.states.values().all(|o| type_key(o) != "channel")
        } && keyed.is_empty()
            && listed.is_empty();
        if nothing && complete && session.sync.personal_stream_empty().await.unwrap_or(false) {
            log::info!("[replica] the relays hold no vault for this key yet: holding it whole");
            drop(_one);
            return self.sync_fully().await;
        }
        if complete && let Some(lite) = self.state.lock().lite.as_mut() {
            lite.checked = true;
        }
        self.emit_status(true);
        let defs: Vec<String> = {
            let st = self.state.lock();
            Self::known_spaces(&st)
                .iter()
                .filter_map(|space| st.states.get(&directory_id(space)))
                .flat_map(|dir| {
                    directory_entries(dir, "type:")
                        .chain(directory_entries(dir, "rel:"))
                        .map(|(id, _)| id.to_string())
                })
                .collect()
        };
        self.lite_fetch(&session, defs).await?;
        // With the personal directory held, this device's record is listed
        // in it; with the definitions in, its share is what it will hold.
        // A relay that did not answer leaves the fetch incomplete, not the
        // directory missing.
        if complete || self.state.lock().states.contains_key(DIRECTORY_ID) {
            self.start_device_records();
        }
        Ok(())
    }

    /// Lite: fetch those of `ids` this replica neither holds nor asked for
    /// this session, then replay them. `false` when some relay did not
    /// answer (those ids are asked again next time).
    async fn lite_fetch(self: &Arc<Self>, session: &Session, ids: Vec<String>) -> Result<bool> {
        self.ensure().await?;
        let (wanted, generation): (Vec<String>, u64) = {
            let st = self.state.lock();
            let Some(lite) = st.lite.as_ref() else {
                return Ok(true);
            };
            let mut seen = HashSet::new();
            let wanted = ids
                .into_iter()
                .filter(|id| {
                    !id.is_empty()
                        && !st.states.contains_key(id)
                        && !st.waiting.contains(id)
                        && !st.vanished.contains(id)
                        && !lite.asked.contains(id)
                        && seen.insert(id.clone())
                })
                .collect();
            (wanted, lite.keys_generation)
        };
        if wanted.is_empty() {
            return Ok(true);
        }
        let complete = session.sync.fetch_objects(wanted.clone()).await?;
        if complete
            && let Some(lite) = self.state.lock().lite.as_mut()
            && lite.keys_generation == generation
        {
            lite.asked.extend(wanted.iter().cloned());
        }
        self.ensure_ids(&wanted).await?;
        Ok(complete)
    }

    /// [`LiteControl::load_space`]: hold `space_id` whole from now on - its
    /// stream when shared here, else the personal stream every unshared
    /// space syncs in - and download it now.
    async fn load_space(self: &Arc<Self>, space_id: &str) -> Result<()> {
        let session = self.ready().await?;
        let scopes = {
            let mut guard = self.state.lock();
            let st = &mut *guard;
            let shared = st.shared_spaces.contains(space_id);
            let Some(lite) = st.lite.as_mut() else {
                return Ok(());
            };
            if lite.loading.iter().any(|l| l.space_id == space_id) {
                return Ok(());
            }
            let mut next = lite.scopes.clone();
            if shared {
                next.spaces.insert(space_id.to_string());
            } else {
                next.personal = true;
            }
            if next == lite.scopes {
                return Ok(());
            }
            lite.scopes = next.clone();
            lite.loading.push(SpaceLoad {
                space_id: space_id.to_string(),
                received: 0,
                expected: 0,
            });
            next
        };
        self.emit_status(true);
        let result = async {
            self.store
                .set_meta(
                    LITE_SCOPES.to_string(),
                    Some(serde_json::to_string(&scopes)?),
                )
                .await?;
            session.sync.set_lite(Some(scopes)).await
        }
        .await;
        if let Some(lite) = self.state.lock().lite.as_mut() {
            lite.loading.retain(|l| l.space_id != space_id);
        }
        let replayed = self.ensure().await;
        self.emit_status(true);
        // More of the vault is here now.
        self.spawn_device_record();
        match result {
            Ok(true) => {}
            Ok(false) => log::info!(
                "[replica] space {space_id}: not every relay answered; sync completes it later"
            ),
            Err(error) => return Err(error),
        }
        replayed
    }

    /// [`LiteControl::sync_fully`]: from now on this replica holds the whole
    /// vault. Resolves once the download started.
    async fn sync_fully(self: &Arc<Self>) -> Result<()> {
        let session = self.ready().await?;
        if self.state.lock().lite.take().is_none() {
            return Ok(());
        }
        self.kv.set(HOLDING, Some("full"));
        self.store.set_meta(LITE_SCOPES.to_string(), None).await?;
        self.emit_status(true);
        self.spawn_device_record();
        let sync = session.sync.clone();
        let download: BoxFuture<()> = Box::pin(async move {
            match sync.set_lite(None).await {
                Ok(complete) => log::info!("[replica] full sync pass ended (complete: {complete})"),
                Err(error) => log::warn!("[replica] full sync: {error:#}"),
            }
        });
        rt::spawn(download);
        Ok(())
    }

    /// [`LiteControl::estimate`], from the store and the directories' counts.
    async fn estimate(self: &Arc<Self>) -> Result<VaultEstimate> {
        self.ready().await?;
        self.ensure().await?;
        let (full, held_objects, counted_total) = {
            let st = self.state.lock();
            let held = st
                .states
                .iter()
                .filter(|(id, o)| counted(o) && !st.vanished.contains(*id))
                .count() as u64;
            let total: u64 = st
                .states
                .iter()
                .filter(|(id, o)| id.starts_with(DIRECTORY_SPACE_PREFIX) && !deleted(o))
                .map(|(_, dir)| directory_counts(dir).values().sum::<u64>())
                .sum();
            (st.lite.is_none(), held, total)
        };
        let changes: u64 = self.store.change_counts().await?.values().sum();
        let bases = self.store.current_bases().await?.len() as u64;
        let held_events = changes + bases;
        let total_objects = if full {
            held_objects
        } else {
            counted_total.max(held_objects)
        };
        let per_object = held_events
            .checked_div(held_objects)
            .map_or(EVENTS_PER_OBJECT, |n| n.max(1));
        let missing_events = if full {
            0
        } else {
            (total_objects - held_objects) * per_object
        };
        Ok(VaultEstimate {
            full,
            held_objects,
            total_objects,
            held_events,
            missing_events,
            held_bytes: held_events * EVENT_BYTES,
            missing_bytes: missing_events * EVENT_BYTES,
            seconds: missing_events / EVENTS_PER_SECOND,
        })
    }

    // -------------------------------------------------------------- devices

    /// Device record ids the personal directory lists (`device:<recordId>`).
    fn listed_devices(&self) -> Vec<String> {
        let st = self.state.lock();
        st.states
            .get(DIRECTORY_ID)
            .map(|dir| {
                directory_entries(dir, "device:")
                    .map(|(id, _)| id.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Lite: fetch the device records the personal directory lists that
    /// this replica neither holds nor asked for (Settings lists them all).
    fn spawn_lite_devices(self: &Arc<Self>) {
        let this = self.clone();
        let fetch: BoxFuture<()> = Box::pin(async move {
            let result = async {
                let Some(session) = this.session() else {
                    return Ok(());
                };
                this.ensure().await?;
                let ids = this.listed_devices();
                this.lite_fetch(&session, ids).await.map(|_| ())
            }
            .await;
            if let Err(error) = result {
                log::warn!("[replica] lite device records: {error:#}");
            }
        });
        rt::spawn(fetch);
    }

    /// Start this session's device record writer, once: the record now,
    /// then a check every [`DEVICE_CHECK`] while the session runs (a failed
    /// write is tried again after [`DEVICE_RETRY`]).
    fn start_device_records(self: &Arc<Self>) {
        let Some(session) = self.session() else {
            return;
        };
        {
            let mut st = self.state.lock();
            if st.device_writer || st.logging_out {
                return;
            }
            st.device_writer = true;
        }
        let (inner, session) = (Arc::downgrade(self), Arc::downgrade(&session));
        let writer: BoxFuture<()> = Box::pin(async move {
            loop {
                let wait = {
                    let Some(this) = inner.upgrade() else {
                        return;
                    };
                    let current = session
                        .upgrade()
                        .zip(this.session())
                        .is_some_and(|(ours, running)| Arc::ptr_eq(&ours, &running));
                    if !current {
                        return;
                    }
                    match this.write_device_record().await {
                        Ok(_) => DEVICE_CHECK,
                        Err(error) => {
                            log::warn!("[replica] device record: {error:#}");
                            DEVICE_RETRY
                        }
                    }
                };
                roostr_nostr::sleep(wait).await;
            }
        });
        rt::spawn(writer);
    }

    /// What this device holds changed: its record now, once the writer runs.
    fn spawn_device_record(self: &Arc<Self>) {
        if !self.state.lock().device_writer {
            return;
        }
        let this = self.clone();
        let write: BoxFuture<()> = Box::pin(async move {
            if let Err(error) = this.write_device_record().await {
                log::warn!("[replica] device record: {error:#}");
            }
        });
        rt::spawn(write);
    }

    /// This device's record (`device_seen`, glonOdin `core/device.odin`):
    /// its auto name and kind, full or lite and about how much it holds.
    /// The planner writes only what changed (a renamed record keeps its
    /// name), and `last_seen` alone at most every 30 minutes. `true` when
    /// something was written.
    async fn write_device_record(self: &Arc<Self>) -> Result<bool> {
        let _one = self.device_lock.lock().await;
        if self.session().is_none() || self.state.lock().logging_out {
            return Ok(false);
        }
        let lite = self.is_lite();
        let percent = if lite {
            self.estimate().await?.percent()
        } else {
            100
        };
        let mut params = JsonMap::new();
        params.insert(
            "object_id".into(),
            json!(roostr_core::devices::record_id(&self.device_id())),
        );
        params.insert("name".into(), json!(self.device.name));
        params.insert("kind".into(), json!(self.device.kind));
        params.insert("holding".into(), json!(if lite { "lite" } else { "full" }));
        params.insert("percent".into(), json!(percent));
        let out = self.mutate_inner("device_seen", params, false).await?;
        Ok(out.get("changed").and_then(Json::as_bool) == Some(true))
    }

    // ---------------------------------------------------------------- query

    /// Make the engine's query cache this replica's vault: the cold corpus
    /// load once per engine generation, then only the deltas. Returns the
    /// result of the push (`body` queried) - the cold path's full JSON
    /// fallback answers in the same call. Caller holds `cache_lock`.
    async fn push_query(&self, body: Option<&Json>) -> Result<roostr_engine::QueryResult> {
        let generation = self.engine.generation();
        let cold = self.state.lock().query_generation != Some(generation);
        if cold && let Err(error) = self.load_corpus().await {
            // A refused corpus must not wedge querying: push every state as
            // JSON with a reset, slower but independent.
            log::warn!("[replica] corpus load fell back to JSON: {error:#}");
            let states = self.states_snapshot();
            let upserts: Vec<Json> = states.iter().map(|s| (**s).clone()).collect();
            let result = self
                .engine
                .query(QueryUpdate {
                    upserts: &upserts,
                    removed: &[],
                    reset: true,
                    body,
                    now_ms: now_ms(),
                })
                .await;
            if result.is_ok() {
                let mut st = self.state.lock();
                st.query_generation = Some(self.engine.generation());
                st.query_upserted.clear();
                st.query_removed.clear();
            }
            return Ok(result?);
        }
        let (upserts, removed) = {
            let st = self.state.lock();
            let upserts: Vec<Json> = st
                .query_upserted
                .iter()
                .filter_map(|id| st.states.get(id))
                .map(|s| (**s).clone())
                .collect();
            let removed: Vec<String> = st
                .query_removed
                .iter()
                .filter(|id| !st.states.contains_key(*id))
                .cloned()
                .collect();
            (upserts, removed)
        };
        let result = self
            .engine
            .query(QueryUpdate {
                upserts: &upserts,
                removed: &removed,
                reset: false,
                body,
                now_ms: now_ms(),
            })
            .await;
        match result {
            // Cleared only after the push succeeds: a failed call leaves the
            // delta owing, or the engine keeps stale rows for good.
            Ok(result) => {
                let mut st = self.state.lock();
                st.query_upserted.clear();
                st.query_removed.clear();
                Ok(result)
            }
            Err(error) => {
                if self.engine.generation() != generation {
                    self.state.lock().query_generation = None;
                }
                Err(error.into())
            }
        }
    }

    /// Cold start: hand the engine the protobuf this replica stores.
    async fn load_corpus(&self) -> Result<()> {
        let based: HashSet<String> = self.store.current_bases().await?.into_keys().collect();
        let histories = self.store.all_change_histories().await?;
        let (histories, covered) = {
            let st = self.state.lock();
            let histories: Vec<roostr_engine::ObjectHistory> = histories
                .into_iter()
                .filter(|h| !st.vanished.contains(&h.object_id) && !based.contains(&h.object_id))
                .map(|h| roostr_engine::ObjectHistory {
                    checkpoint: h.checkpoint,
                    changes: h.changes,
                })
                .collect();
            let covered: Vec<String> = st
                .query_upserted
                .iter()
                .filter(|id| !based.contains(*id))
                .cloned()
                .collect();
            (histories, covered)
        };
        self.engine
            .load_corpus(histories.iter(), true, now_ms())
            .await?;
        let mut st = self.state.lock();
        // Every pending upsert of a corpus object is a replay of the bytes
        // just loaded; objects held from a base go in as their JSON state.
        for id in covered {
            st.query_upserted.remove(&id);
        }
        for id in based {
            if st.states.contains_key(&id) {
                st.query_upserted.insert(id);
            }
        }
        let vanished: Vec<String> = st.vanished.iter().cloned().collect();
        st.query_removed.extend(vanished);
        st.query_generation = Some(self.engine.generation());
        Ok(())
    }

    async fn query(self: &Arc<Self>, body: &Json) -> Result<QueryResult> {
        self.ready().await?;
        self.ensure().await?;
        let result = {
            let _cache = self.cache_lock.lock().await;
            self.push_query(Some(body)).await?
        };
        let records = result
            .records
            .into_iter()
            .map(|row| serde_json::from_value(unpack_core_value_maps(row)))
            .collect::<Result<_, _>>()?;
        Ok(QueryResult {
            total: result.total.max(0) as u64,
            records,
        })
    }

    // ------------------------------------------------------------- mutation

    async fn mutate_inner(
        self: &Arc<Self>,
        action: &str,
        params: JsonMap,
        owner_signal: bool,
    ) -> Result<JsonMap> {
        let session = self.ready().await?;
        {
            let mut st = self.state.lock();
            if st.logging_out {
                bail!("Logout in progress");
            }
            st.mutations += 1;
        }
        let result = async {
            let _claim = self.mutation_lock.lock().await;
            self.apply_mutation(&session, action, params, owner_signal)
                .await
        }
        .await;
        self.state.lock().mutations -= 1;
        result
    }

    async fn apply_mutation(
        self: &Arc<Self>,
        session: &Session,
        action: &str,
        params: JsonMap,
        owner_signal: bool,
    ) -> Result<JsonMap> {
        let pk = session.keys.pubkey();
        let channel_id = params
            .get("channel_id")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        // An owner deletes a space for everyone, a member only leaves it.
        if action == "space_leave" && !channel_id.is_empty() {
            let entry = self.keyring.get(&channel_id).map_err(|e| anyhow!("{e}"))?;
            if space_owner(entry.as_ref(), pk).is_empty() {
                bail!("You own this space: delete it for everyone instead of leaving it.");
            }
        }
        if action == "vanish" && !owner_signal {
            let keys = self.keyring.all().map_err(|e| anyhow!("{e}"))?;
            let ids = params.get("object_id").into_iter().chain(
                params
                    .get("object_ids")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten(),
            );
            for id in ids.filter_map(Json::as_str) {
                if !space_owner(keys.get(id), pk).is_empty() {
                    bail!("Only the space's owner can delete it for everyone: leave it instead.");
                }
            }
        }
        let rotating = action == "channel_member_remove" || action == "channel_key_rotate";
        let key_id = if rotating {
            self.keyring
                .get(&channel_id)
                .map_err(|e| anyhow!("{e}"))?
                .map_or(0, |k| k.key_id as i64)
                + 1
        } else {
            0
        };
        let params = Json::Object(params);
        let plan = self.plan(action, &params, &session.author, key_id).await?;
        let mut shared_changed = false;
        // Key material stays in the host; invalid requests never rotate keys.
        if rotating {
            self.keyring
                .rotate(&channel_id)
                .map_err(|e| anyhow!("{e}"))?;
            shared_changed = true;
        }
        for (index, change) in plan.changes.into_iter().enumerate() {
            let object_id = change.object_id.clone();
            self.commit(session, change).await?;
            if action == "channel_create" && index == 0 {
                self.keyring
                    .ensure(&object_id)
                    .map_err(|e| anyhow!("{e}"))?;
                shared_changed = true;
            }
            if object_id == VANISH_LOG_ID {
                shared_changed = true;
            }
        }
        // Leaving keeps nothing to sync the space with.
        if action == "space_leave" {
            self.keyring
                .remove(&channel_id)
                .map_err(|e| anyhow!("{e}"))?;
            shared_changed = true;
        }
        if !plan.vanish_changes.is_empty() {
            shared_changed = true;
        }
        for change in plan.vanish_changes {
            self.commit(session, change).await?;
        }
        if shared_changed {
            self.spawn_refresh();
        }
        let mut out = match unpack_core_value_maps(plan.result) {
            Json::Object(map) => map,
            Json::Null => JsonMap::new(),
            other => {
                let mut map = JsonMap::new();
                map.insert("result".into(), other);
                map
            }
        };
        out.insert("ok".into(), Json::Bool(true));
        Ok(out)
    }

    /// mutate.ts `planMutation`: plan over the engine's query cache; ids the
    /// cache lacks come back missing, and the one retry carries this
    /// replica's states of them as overlay and names the rest absent.
    async fn plan(
        self: &Arc<Self>,
        action: &str,
        params: &Json,
        author: &str,
        key_id: i64,
    ) -> Result<roostr_engine::MutationPlan> {
        self.ensure().await?;
        let id_seed = uuid::Uuid::new_v4().to_string();
        let device = self.device_id();
        let request = MutationRequest {
            action,
            params,
            timestamp: now_ms(),
            author,
            id_seed: &id_seed,
            key_id,
            device: &device,
            objects: None,
            overlay: &[],
            absent: &[],
        };
        let first = {
            let _cache = self.cache_lock.lock().await;
            self.push_query(None).await?;
            self.engine.mutation(&request).await?
        };
        let missing = match first {
            MutationResponse::Plan(plan) => return Ok(plan),
            MutationResponse::Missing(ids) => ids,
        };
        // A lite replica fetches what it does not hold before calling it absent.
        if let Some(session) = self.session().filter(|_| self.is_lite()) {
            self.lite_fetch(&session, missing.clone()).await?;
        }
        self.ensure_ids(&missing).await?;
        let overlay: Vec<Json> = {
            let st = self.state.lock();
            missing
                .iter()
                .filter_map(|id| st.states.get(id))
                .map(|s| (**s).clone())
                .collect()
        };
        let held: HashSet<&str> = overlay.iter().filter_map(|s| s["id"].as_str()).collect();
        let absent: Vec<String> = missing
            .iter()
            .filter(|id| !held.contains(id.as_str()))
            .cloned()
            .collect();
        let retried = {
            let _cache = self.cache_lock.lock().await;
            self.push_query(None).await?;
            self.engine
                .mutation(&MutationRequest {
                    overlay: &overlay,
                    absent: &absent,
                    ..request
                })
                .await?
        };
        match retried {
            MutationResponse::Plan(plan) => Ok(plan),
            MutationResponse::Missing(ids) => bail!(
                "Mutation needs objects this replica cannot supply: {}",
                ids.join(", ")
            ),
        }
    }

    /// Address, durably persist (with its publication obligation) and
    /// publish one planned change. On an object with a base it is a delta of
    /// that base: its events carry `["b", base]`.
    async fn commit(self: &Arc<Self>, session: &Session, mut change: Change) -> Result<()> {
        let object_id = change.object_id.clone();
        let held = self.store.object_inputs(object_id.clone()).await?;
        let pending = self.store.pending_change_ids().await?;
        let base = held.current_base.as_ref().map(|b| b.hash.clone());
        let inputs = state_inputs(held.changes, held.bases, held.checkpoint, &pending)?;
        change.parent_ids = self.engine.object_heads(&inputs).await?;
        let bytes = self.engine.encode_change(&change).await?;
        // Round-trip through decode so the stored JSON matches relay-imported
        // changes byte for byte.
        let mut decoded = self.engine.decode_change(&bytes).await?;
        decoded.b = base.clone();
        let id = decoded.id.clone();
        let record = ChangeRecord {
            id: id.clone(),
            object_id: object_id.clone(),
            bytes: bytes.clone(),
            json: serde_json::to_string(&decoded)?,
            b: base.clone(),
            rebased: false,
        };
        let obligation = PendingPublish {
            key: id.clone(),
            object_id: object_id.clone(),
            change_id: id.clone(),
            bytes: bytes.clone(),
            space_id: None,
            key_id: None,
            base: base.clone(),
            events: None,
        };
        self.store.add_local_change(record, obligation).await?;
        // The engine applies its own planned change to the cache it plans over.
        let applied = {
            let _cache = self.cache_lock.lock().await;
            let warm = self.state.lock().query_generation == Some(self.engine.generation());
            warm && match self
                .engine
                .apply_committed(std::slice::from_ref(&decoded))
                .await
            {
                Ok(out) => out.applied.len() == 1,
                Err(error) => {
                    log::debug!("[replica] cache apply failed, upserting instead: {error}");
                    false
                }
            }
        };
        self.state.lock().dirty.insert(object_id.clone());
        self.ensure().await?;
        if applied {
            let mut st = self.state.lock();
            let exact = st
                .states
                .get(&object_id)
                .is_some_and(|s| heads(s) == [id.as_str()]);
            if exact {
                st.query_upserted.remove(&object_id);
            }
        }
        // Publishing is an obligation, not part of the write: an unreachable
        // relay must never turn a committed change into a failed one.
        if let Err(error) = session
            .sync
            .publish(bytes, id, object_id.clone(), base)
            .await
        {
            log::error!("[replica] publish deferred: {error:#}");
        }
        self.notify(&[object_id]);
        Ok(())
    }

    // -------------------------------------------------------------- reads

    async fn settings(&self) -> Result<Settings> {
        let keys = identity::load(&*self.kv);
        Ok(Settings {
            has_key: keys.is_some(),
            relays: self.relays(),
            author_id: keys.as_ref().map(Keys::author_id).unwrap_or_default(),
            npub: keys.as_ref().map(Keys::npub),
            identity_pending: false,
            device_id: self.device_id(),
        })
    }

    async fn objects(self: &Arc<Self>) -> Result<Vec<ObjectSummary>> {
        self.ready().await?;
        self.ensure().await?;
        let st = self.state.lock();
        let mut out: Vec<ObjectSummary> = st
            .states
            .values()
            .filter(|o| !deleted(o) && !is_system_type(type_key(o)))
            .map(|o| ObjectSummary {
                id: o["id"].as_str().unwrap_or("").to_string(),
                type_key: type_key(o).to_string(),
                name: fstr(o, "name").to_string(),
                updated_at: int_of(&o["updatedAt"]).unwrap_or(0),
                channel_id: fstr(o, "channel").to_string(),
                icon: fstr(o, "iconEmoji").to_string(),
                done: Some(fbool(o, "done")),
            })
            .collect();
        // Lite: the pinned objects not held yet, as their directory lists them.
        if st.lite.is_some() {
            for (dir_id, dir) in st
                .states
                .iter()
                .filter(|(id, o)| id.starts_with(DIRECTORY_SPACE_PREFIX) && !deleted(o))
            {
                let space = &dir_id[DIRECTORY_SPACE_PREFIX.len()..];
                for (id, entry) in directory_entries(dir, "pin:") {
                    if st.states.contains_key(id) || st.vanished.contains(id) {
                        continue;
                    }
                    out.push(ObjectSummary {
                        id: id.to_string(),
                        type_key: member_str(entry, "type").to_string(),
                        name: member_str(entry, "name").to_string(),
                        updated_at: 0,
                        channel_id: space.to_string(),
                        icon: member_str(entry, "icon").to_string(),
                        done: None,
                    });
                }
            }
        }
        // Newest first, id ascending on a tie - the daemon's order.
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    async fn upcoming_occurrences(
        self: &Arc<Self>,
        after: i64,
        limit: usize,
    ) -> Result<Vec<Occurrence>> {
        self.ready().await?;
        self.ensure_complete().await?;
        let st = self.state.lock();
        let mut out: Vec<Occurrence> = st
            .states
            .iter()
            .filter(|(id, o)| {
                id.as_str() != VANISH_LOG_ID && !st.vanished.contains(*id) && !deleted(o)
            })
            .filter_map(|(id, o)| {
                let at = repeat_entry(o, "next").and_then(|next| int_of(&next["intValue"]))?;
                (at > after).then(|| Occurrence {
                    object_id: id.clone(),
                    name: fstr(o, "name").to_string(),
                    type_key: type_key(o).to_string(),
                    at,
                    agent_owned: !field(o, "assignee").is_null() || !field(o, "agent").is_null(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.object_id.cmp(&b.object_id)));
        out.truncate(limit);
        Ok(out)
    }

    /// One object; a lite replica fetches it from the relays first when it
    /// does not hold it.
    async fn object(self: &Arc<Self>, id: &str) -> Result<Object> {
        let session = self.ready().await?;
        self.ensure_ids(&[id.to_string()]).await?;
        if self.is_lite() && !self.state.lock().states.contains_key(id) {
            self.lite_fetch(&session, vec![id.to_string()]).await?;
        }
        let state = self
            .state
            .lock()
            .states
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown object {id}"))?;
        Ok(serde_json::from_value((*state).clone())?)
    }

    async fn channels(self: &Arc<Self>) -> Result<Vec<Space>> {
        let session = self.ready().await?;
        self.ensure().await?;
        let keys = self.keyring.all().map_err(|e| anyhow!("{e}"))?;
        let st = self.state.lock();
        let pins = pins_by_space(st.states.values());
        let mut out: Vec<Space> = st
            .states
            .values()
            .filter(|o| !deleted(o) && type_key(o) == "channel")
            .map(|o| {
                let id = o["id"].as_str().unwrap_or("").to_string();
                let image = fstr(o, "iconImage");
                Space {
                    name: fstr(o, "name").to_string(),
                    // An image icon (link or upload) wins over an emoji.
                    icon: if image.is_empty() {
                        fstr(o, "iconEmoji")
                    } else {
                        image
                    }
                    .to_string(),
                    pinned_ids: pins.get(&id).cloned().unwrap_or_default(),
                    members: entries(o, "members")
                        .map(|e| SpaceMember {
                            npub: entry_str(e, "npub").to_string(),
                            role: entry_str(e, "role").to_string(),
                        })
                        .collect(),
                    key_id: fint(o, "keyId").unwrap_or(1),
                    created_at: created_at(o),
                    owner: space_owner(keys.get(&id), session.keys.pubkey()),
                    id,
                }
            })
            .collect();
        // Lite: the spaces the personal directory lists that are not held,
        // and each space's pins of objects not held.
        if st.lite.is_some() {
            let held: HashSet<String> = out.iter().map(|s| s.id.clone()).collect();
            if let Some(dir) = st.states.get(DIRECTORY_ID) {
                for (id, entry) in directory_entries(dir, "space:") {
                    if held.contains(id) || st.vanished.contains(id) {
                        continue;
                    }
                    out.push(Space {
                        name: member_str(entry, "name").to_string(),
                        icon: member_str(entry, "icon").to_string(),
                        pinned_ids: Vec::new(),
                        members: Vec::new(),
                        key_id: keys.get(id).map_or(1, |k| k.key_id as i64),
                        created_at: map_member(entry, "created")
                            .and_then(|v| int_of(&v["intValue"]))
                            .unwrap_or(0),
                        owner: space_owner(keys.get(id), session.keys.pubkey()),
                        id: id.to_string(),
                    });
                }
            }
            for space in &mut out {
                space.pinned_ids = Self::lite_pins(&st, &space.id, &space.pinned_ids);
            }
        }
        // Creation order, oldest first: the first channel is the stable default space.
        out.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    async fn relations(self: &Arc<Self>) -> Result<Vec<RelationDef>> {
        self.ready().await?;
        self.ensure().await?;
        let st = self.state.lock();
        Ok(st
            .states
            .values()
            .filter(|o| !deleted(o) && type_key(o) == "relation" && !fstr(o, "key").is_empty())
            .map(|o| {
                let format = fstr(o, "format");
                RelationDef {
                    id: o["id"].as_str().unwrap_or("").to_string(),
                    key: fstr(o, "key").to_string(),
                    format: if format.is_empty() {
                        "shorttext"
                    } else {
                        format
                    }
                    .to_string(),
                    name: fstr(o, "name").to_string(),
                    icon_emoji: Some(fstr(o, "iconEmoji").to_string()),
                    space: Some(fstr(o, "channel").to_string()),
                    hidden: fbool(o, "hidden"),
                    read_only: fbool(o, "readOnly"),
                    max_count: fint(o, "maxCount").unwrap_or(0),
                    object_types: Some(
                        field(o, "object_types")["valuesValue"]["items"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|i| i["stringValue"].as_str())
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect(),
                    ),
                    options: entries(o, "options")
                        .map(|e| RelationOption {
                            id: entry_str(e, "id").to_string(),
                            text: entry_str(e, "text").to_string(),
                            color: entry_str(e, "color").to_string(),
                            order_id: entry_str(e, "orderId").to_string(),
                        })
                        .collect(),
                }
            })
            .collect())
    }

    /// The daemon's `POST /api/serving`: the engine's serving rule over this
    /// replica's agents, machines, skills and capabilities.
    async fn serving(self: &Arc<Self>, object_ids: &[String]) -> Result<JsonMap> {
        self.ready().await?;
        self.ensure_complete().await?;
        let (objects, agents, machines, skills, capabilities) = {
            let st = self.state.lock();
            let of = |kind: &str| -> Vec<Json> {
                st.states
                    .values()
                    .filter(|o| type_key(o) == kind)
                    .map(|o| (**o).clone())
                    .collect()
            };
            let objects: Vec<(String, Option<Json>)> = object_ids
                .iter()
                .filter(|id| !id.is_empty())
                .map(|id| (id.clone(), st.states.get(id).map(|o| (**o).clone())))
                .collect();
            (
                objects,
                of("agent"),
                of("machine"),
                of("skill"),
                of("capability"),
            )
        };
        let mut out = JsonMap::new();
        for (id, object) in objects {
            let serving = self
                .engine
                .resolve_serving(&ServingInputs {
                    object: object.as_ref(),
                    agents: &agents,
                    machines: &machines,
                    skills: &skills,
                    capabilities: &capabilities,
                })
                .await?;
            out.insert(
                id,
                json!({
                    "machineId": serving.machine_id,
                    "reason": serving.reason,
                    "skills": serving.skills,
                    "candidates": serving.candidates,
                }),
            );
        }
        Ok(out)
    }

    /// The daemon's `GET /api/sync/digest`: sha256 over
    /// `"<objectId>:<sorted change ids>\n"` lines, objects sorted, vanished
    /// objects excluded.
    async fn sync_digest(self: &Arc<Self>) -> Result<JsonMap> {
        self.ready().await?;
        self.ensure_complete().await?;
        let vanished = self.state.lock().vanished.clone();
        let mut ids: Vec<String> = self
            .store
            .object_ids()
            .await?
            .into_iter()
            .filter(|id| !vanished.contains(id))
            .collect();
        ids.sort();
        let (mut text, mut changes, mut objects) = (String::new(), 0usize, 0usize);
        for oid in ids {
            let mut cids: Vec<String> = self
                .store
                .changes_for(oid.clone())
                .await?
                .into_iter()
                .map(|c| c.id)
                .collect();
            if cids.is_empty() {
                continue;
            }
            cids.sort();
            text.push_str(&format!("{oid}:{}\n", cids.join(",")));
            changes += cids.len();
            objects += 1;
        }
        let digest = hex::encode(Sha256::digest(text.as_bytes()));
        let mut out = JsonMap::new();
        out.insert("digest".into(), json!(digest));
        out.insert("objects".into(), json!(objects));
        out.insert("changes".into(), json!(changes));
        out.insert("computedAt".into(), json!(now_ms()));
        Ok(out)
    }

    // ----------------------------------------------------- platform actions

    /// `nostr_key_import`: make `key` this device's identity. A different
    /// identity logs the old one out first: its replica must not leak.
    async fn import_key(self: &Arc<Self>, key: &str) -> Result<JsonMap> {
        let next = Keys::parse(key).map_err(|_| anyhow!(identity::INVALID_KEY))?;
        if let Some(current) = identity::load(&*self.kv) {
            if current.pubkey() == next.pubkey() {
                return Ok(ok_map());
            }
            self.logout().await?;
        }
        identity::save(&*self.kv, key)?;
        Ok(ok_map())
    }

    async fn set_relays(self: &Arc<Self>, relays: Vec<String>) -> Result<JsonMap> {
        self.kv.set(RELAYS, Some(&serde_json::to_string(&relays)?));
        self.restart_sync().await;
        Ok(ok_map())
    }

    fn cached_profile(&self) -> roostr_nostr::profile::Profile {
        self.kv
            .get(PROFILE)
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn profile_relays(&self) -> Vec<roostr_nostr::Relay> {
        let mut urls = self.relays();
        if urls.is_empty() {
            urls = roostr_nostr::DEFAULT_RELAYS
                .iter()
                .map(|s| s.to_string())
                .collect();
        }
        urls.into_iter().map(roostr_nostr::Relay::new).collect()
    }

    async fn profile(&self) -> Result<roostr_nostr::profile::Profile> {
        let Some(keys) = identity::load(&*self.kv) else {
            return Ok(Default::default());
        };
        let relays = self.profile_relays();
        match roostr_nostr::profile::fetch(&relays, keys.pubkey(), PROFILE_TIMEOUT).await {
            Ok(found) => {
                let profile = found.unwrap_or_default();
                self.kv
                    .set(PROFILE, Some(&Json::Object(profile.clone()).to_string()));
                Ok(profile)
            }
            Err(_) => Ok(self.cached_profile()),
        }
    }

    async fn save_profile(
        &self,
        patch: roostr_nostr::profile::Profile,
    ) -> Result<roostr_nostr::profile::Profile> {
        let keys = identity::load(&*self.kv).ok_or_else(|| anyhow!("no key"))?;
        let relays = self.profile_relays();
        let next = roostr_nostr::profile::save(
            &relays,
            &keys,
            patch,
            self.cached_profile(),
            PROFILE_TIMEOUT,
        )
        .await
        .map_err(|e| anyhow!("{e}"))?;
        self.kv
            .set(PROFILE, Some(&Json::Object(next.clone()).to_string()));
        Ok(next)
    }
}

const PROFILE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

fn ok_map() -> JsonMap {
    let mut map = JsonMap::new();
    map.insert("ok".into(), Json::Bool(true));
    map
}

/// The engine's state-sync request for one object's holdings.
fn state_inputs(
    changes: Vec<ChangeRecord>,
    bases: Vec<roostr_replica_store::BaseRecord>,
    checkpoint: Option<roostr_replica_store::CheckpointRecord>,
    pending: &HashSet<String>,
) -> Result<StateInputs> {
    let pending = changes
        .iter()
        .filter(|c| pending.contains(&c.id))
        .map(|c| c.id.clone())
        .collect();
    let changes = changes
        .iter()
        .map(|c| serde_json::from_str::<Change>(&c.json))
        .collect::<Result<_, _>>()?;
    Ok(StateInputs {
        bases: bases.into_iter().map(|b| b.bytes).collect(),
        changes,
        checkpoint: checkpoint.map(|c| c.bytes),
        pending,
    })
}

/// pins.ts `pinsBySpace` over replayed states.
fn pins_by_space<'a>(states: impl Iterator<Item = &'a Arc<Json>>) -> HashMap<String, Vec<String>> {
    let all: Vec<&Json> = states.map(|s| &**s).filter(|s| !deleted(s)).collect();
    let fallback = all
        .iter()
        .filter(|s| type_key(s) == "channel")
        .min_by(|a, b| {
            created_at(a)
                .cmp(&created_at(b))
                .then(a["id"].as_str().cmp(&b["id"].as_str()))
        })
        .and_then(|s| s["id"].as_str())
        .unwrap_or("");
    let mut by_space: HashMap<String, Vec<(&str, &str)>> = HashMap::new();
    for s in &all {
        let key = fstr(s, roostr_core::pins::PIN_KEY);
        if key.is_empty() {
            continue;
        }
        let space = match fstr(s, "channel") {
            "" => fallback,
            space => space,
        };
        by_space
            .entry(space.to_string())
            .or_default()
            .push((s["id"].as_str().unwrap_or(""), key));
    }
    by_space
        .into_iter()
        .map(|(space, mut list)| {
            list.sort_by(|x, y| x.1.cmp(y.1).then(x.0.cmp(y.0)));
            (
                space,
                list.into_iter().map(|(id, _)| id.to_string()).collect(),
            )
        })
        .collect()
}

/// The sync layer's view of the backend; weak, so a dropped backend stops it.
struct Host(Weak<Inner>);

impl SyncHost for Host {
    fn on_objects(&self, ids: Vec<String>) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        let (shared, directory) = {
            let mut st = inner.state.lock();
            st.dirty.extend(ids.iter().cloned());
            let shared = ids.iter().any(|id| {
                id == VANISH_LOG_ID || st.states.get(id).is_some_and(|s| type_key(s) == "channel")
            });
            (
                shared,
                st.lite.is_some() && ids.iter().any(|id| id == DIRECTORY_ID),
            )
        };
        inner.notify(&ids);
        // A synced commit on a space object can change members or keys.
        if shared {
            inner.spawn_refresh();
        }
        // Lite: a device the personal directory now lists is fetched.
        if directory {
            inner.spawn_lite_devices();
        }
    }

    fn on_status(&self, status: sync::SyncStatus) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        let next = {
            let mut guard = inner.state.lock();
            let st = &mut *guard;
            // A space download in flight shows the pass's progress.
            if let Some(lite) = st.lite.as_mut() {
                for load in &mut lite.loading {
                    load.received = status.received.unwrap_or(load.received);
                    load.expected = status.expected.unwrap_or(load.expected);
                }
            }
            let previous = &st.status;
            let preparing = Inner::preparing(st);
            let lite = Inner::lite_status(st);
            st.status = SyncStatus {
                phase: match status.phase {
                    sync::Phase::Idle => SyncPhase::Idle,
                    sync::Phase::Backfill => SyncPhase::Backfill,
                    sync::Phase::Live => SyncPhase::Live,
                    sync::Phase::Error => SyncPhase::Error,
                },
                imported: status.imported.unwrap_or(previous.imported),
                detail: status.detail,
                pending: status.pending.or(previous.pending),
                received: status.received,
                expected: status.expected,
                // A vault still being prepared reads as one still loading.
                bootstrapped: status.bootstrapped && preparing.is_none(),
                preparing,
                lite,
            };
            st.sync_bootstrapped = status.bootstrapped;
            st.status.clone()
        };
        inner.emit(BackendEvent::Status(next));
    }

    fn on_space_key(&self) {
        if let Some(inner) = self.0.upgrade() {
            inner.spawn_refresh();
        }
    }

    /// Settings reads [`roostr_core::account::Account::join_requests`] when
    /// it opens the space page; nothing is pushed.
    fn on_join_request(&self) {}

    fn on_space_vanished(&self, space_id: String) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        // Serialized: live and history both deliver the owner's signal, and
        // the second must see the ledger entry the first one wrote.
        rt::spawn(async move {
            let _one = inner.owner_signals.lock().await;
            let result = async {
                inner.ensure().await?;
                if !inner.state.lock().vanished.contains(&space_id) {
                    let mut params = JsonMap::new();
                    params.insert("object_ids".into(), json!([space_id]));
                    inner.mutate_inner("vanish", params, true).await?;
                }
                anyhow::Ok(())
            }
            .await;
            if let Err(error) = result {
                log::warn!("[replica] owner deletion of space {space_id} failed: {error:#}");
            }
        });
    }

    fn space_gone(&self, space_id: &str) -> bool {
        self.0
            .upgrade()
            .is_some_and(|inner| inner.state.lock().vanished.contains(space_id))
    }

    fn space_of(&self, object_id: &str) -> String {
        self.0
            .upgrade()
            .and_then(|inner| {
                inner
                    .state
                    .lock()
                    .states
                    .get(object_id)
                    .map(|s| space_of_state(s))
            })
            .unwrap_or_default()
    }

    fn on_history_pass(&self, complete: bool, reachable: bool) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        // Seed only once this replica holds what the relays hold; offline
        // (no relay reachable) it seeds anyway, so a first run works.
        if complete || !reachable {
            inner.seed_defaults();
        }
        // Lite: directories the relays did not answer for are asked again.
        let unchecked = inner.state.lock().lite.as_ref().is_some_and(|l| !l.checked);
        if unchecked && reachable {
            inner.spawn_lite_frame();
        }
        // This device's record, once it holds its earlier one: a full
        // replica after a complete pass (a lite one starts at the end of its
        // directory fetch, [`Inner::lite_frame`]), or offline.
        if (complete && !inner.is_lite()) || !reachable {
            inner.start_device_records();
        }
    }
}

impl Backend for ReplicaBackend {
    fn describe(&self) -> String {
        self.inner.label.clone()
    }

    fn settings(&self) -> BoxFuture<Result<Settings>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.settings().await })
    }

    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.objects().await })
    }

    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        let (inner, id) = (self.inner.clone(), id.to_string());
        Box::pin(async move { inner.object(&id).await })
    }

    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.channels().await })
    }

    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.relations().await })
    }

    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>> {
        let (inner, body) = (self.inner.clone(), body.clone());
        Box::pin(async move { inner.query(&body).await })
    }

    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<Result<JsonMap>> {
        let (inner, action) = (self.inner.clone(), action.to_string());
        Box::pin(async move {
            match action.as_str() {
                "nostr_key_import" => {
                    let key = params.get("key").and_then(Json::as_str).unwrap_or("");
                    inner.import_key(key.trim()).await
                }
                "nostr_relays_set" => {
                    let relays = params
                        .get("relays")
                        .and_then(Json::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Json::as_str)
                        .map(str::to_string)
                        .collect();
                    inner.set_relays(relays).await
                }
                _ => inner.mutate_inner(&action, params, false).await,
            }
        })
    }

    fn serving(&self, object_ids: &[String]) -> BoxFuture<Result<JsonMap>> {
        let (inner, ids) = (self.inner.clone(), object_ids.to_vec());
        Box::pin(async move { inner.serving(&ids).await })
    }

    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.sync_digest().await })
    }

    /// One subscription to commits and status: `Hello` and the current
    /// status first, then every change as it lands. It never fails.
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        let (tx, rx) = mpsc::unbounded();
        let _ = tx.unbounded_send(BackendEvent::Hello);
        let _ = tx.unbounded_send(BackendEvent::Status(self.status()));
        self.inner.listeners.lock().push(tx);
        let inner = self.inner.clone();
        // Starting here too: a store that only listens still syncs.
        rt::spawn(async move {
            if inner.session().is_none()
                && identity::load(&*inner.kv).is_some()
                && let Err(error) = inner.ready().await
            {
                log::warn!("[replica] start failed: {error:#}");
            }
        });
        Box::pin(rx.map(Ok))
    }

    #[allow(clippy::arc_with_non_send_sync)]
    fn account(&self) -> Option<Arc<dyn roostr_core::account::Account>> {
        Some(Arc::new(self.clone()))
    }

    #[allow(clippy::arc_with_non_send_sync)]
    fn lite(&self) -> Option<Arc<dyn LiteControl>> {
        Some(Arc::new(self.clone()))
    }
}

impl LiteControl for ReplicaBackend {
    fn type_counts(&self, space_id: &str) -> HashMap<String, u64> {
        self.inner
            .state
            .lock()
            .states
            .get(&directory_id(space_id))
            .map(|dir| directory_counts(dir))
            .unwrap_or_default()
    }

    fn load_space(&self, space_id: &str) -> BoxFuture<Result<()>> {
        let (inner, space_id) = (self.inner.clone(), space_id.to_string());
        Box::pin(async move { inner.load_space(&space_id).await })
    }

    fn fetch(&self, ids: Vec<String>) -> BoxFuture<Result<()>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let session = inner.ready().await?;
            if inner.is_lite() {
                inner.lite_fetch(&session, ids).await?;
            }
            Ok(())
        })
    }

    fn estimate(&self) -> BoxFuture<Result<VaultEstimate>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.estimate().await })
    }

    fn sync_fully(&self) -> BoxFuture<Result<()>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.sync_fully().await })
    }
}

impl roostr_core::account::Account for ReplicaBackend {
    fn export_key(&self) -> Result<(String, String)> {
        ReplicaBackend::export_key(self)
    }

    fn logout(&self) -> BoxFuture<Result<()>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.logout().await })
    }

    fn profile(&self) -> BoxFuture<Result<Json>> {
        let inner = self.inner.clone();
        Box::pin(async move { Ok(Json::Object(inner.profile().await?)) })
    }

    fn save_profile(&self, patch: Json) -> BoxFuture<Result<Json>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let Json::Object(patch) = patch else {
                bail!("a profile patch is an object");
            };
            Ok(Json::Object(inner.save_profile(patch).await?))
        })
    }

    fn join_requests(&self) -> BoxFuture<Result<Vec<Json>>> {
        let rows = sync::invite::list_join_requests(&*self.inner.kv);
        Box::pin(async move {
            Ok(rows
                .into_iter()
                .map(serde_json::to_value)
                .collect::<Result<_, _>>()?)
        })
    }

    fn clear_join_request(&self, key: &str) -> BoxFuture<Result<()>> {
        sync::invite::clear_join_request(&*self.inner.kv, key);
        Box::pin(async { Ok(()) })
    }

    fn send_join_request(
        &self,
        link: &roostr_core::invite::SpaceJoinLink,
    ) -> BoxFuture<Result<()>> {
        let keys = identity::load(&*self.inner.kv);
        let link = sync::invite::JoinLink {
            space: link.space.clone(),
            owner: link.owner.clone(),
            relays: link.relays.clone(),
        };
        Box::pin(async move {
            let keys =
                keys.ok_or_else(|| anyhow!("no local identity - open the app once first"))?;
            sync::invite::send_join_request(&keys, &link).await
        })
    }

    fn blossom_upload_authorization(&self, sha256: &str, name: &str) -> Result<String> {
        let keys = identity::load(&*self.inner.kv)
            .ok_or_else(|| anyhow!("Sign in with your key to add files."))?;
        let now = (now_ms() / 1000) as u64;
        let event = keys.sign(roostr_nostr::blossom::upload_auth_template(
            sha256, name, now,
        ));
        Ok(roostr_nostr::blossom::authorization_header(&event))
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests;

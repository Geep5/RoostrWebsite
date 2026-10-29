//! Relay sync: history walk, live subscriptions, durable publishing and
//! shared spaces (sync.ts `RelaySync`, SyncEngine.swift), wire-compatible
//! with the Svelte replica, the iOS app and the glon-odin daemon.
//!
//! Wire schema (`glonOdin/core/wire.odin`, the engine's `wire` method):
//! kind 1078 changes self-encrypted with NIP-44, `h` = blinded object id,
//! big changes split into `["c", group, index, total]` parts; 1079
//! checkpoints; 31078 bases with their 1080 parts by `#c`. A shared space
//! syncs under its 32-byte key as the conversation key, each event also
//! tagged with the blinded space stream `blind(key, "space:"+id)` so one
//! `#h` filter pulls the whole space.
//!
//! The host keeps secp256k1 (signing, verification - the relay client
//! checks every received event - and ECDH). Receiving (decryption, chunk
//! reassembly, cursor, replay obligations) is the engine's `sync` session;
//! publishing is the same session's outbox (order, dedupe, the rotated-key
//! rule, backoff, sealing on first attempt): this layer signs the sealed
//! parts, persists the exact signed events before sending, and reports.
//!
//! History is NIP-77 per relay and stream (self `authors:[pk]`, each space
//! `#h`): held events (store `record_held`, per scope) are reconciled and
//! every needed id fetched with `REQ {ids}`. A relay that refuses NEG-OPEN
//! (NEG-ERR, NOTICE, silence) is walked page by page via `until` instead.
//! Bases first, then checkpoints, then deltas under the per-stream floor
//! rules of `docs/state-sync.md` (see [`history`]).
//!
//! A lite session ([`RelaySync::set_lite`], see [`lite`]) walks only the
//! streams it holds whole and follows the objects it holds by their `#h`
//! tags; everything else is fetched by id when it is opened.

mod history;
mod ingest;
pub mod invite;
mod lite;
mod live;
mod outbox;
mod spaces;

pub use lite::LiteScopes;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use roostr_engine::{Engine, SessionOpen, SessionSpace};
use roostr_nostr::spacekeys::SpaceKeyring;
use roostr_nostr::{Keys, Relay};
use roostr_replica_store::{ChangeStore, HeldEvent};

use crate::local::LocalKv;
use crate::{MaybeSend, MaybeSync};

pub use roostr_nostr::DEFAULT_RELAYS;

/// The synced vanish ledger object (`src/vanish.odin`). It never gets a
/// base, so its stream is always read whole.
pub const VANISH_LOG_ID: &str = "__vanished__";
/// Base hashes per `#b` delta filter.
pub const BASE_HASH_BATCH: usize = 200;
/// Keeps full-sized 40k-character encrypted chunks inside a relay's 8 MiB
/// response budget.
pub const PAGE_LIMIT: u64 = 128;
/// Ids per `REQ {ids}` after a reconcile.
pub const FETCH_BATCH: usize = 100;
/// Ids per fetch for a checkpoint or base stream: each event can be ~40k chars.
pub const CHECKPOINT_FETCH_BATCH: usize = 8;

/// One event per this interval leaves for the relays.
const PUBLISH_SPACING: Duration = Duration::from_millis(120);
/// Ceiling of the adaptive delay between history pages.
const PAGE_SPACING_MS: u64 = 400;
/// Floor of the adaptive page delay.
const MIN_PAGE_SPACING_MS: u64 = 40;
/// Progress statuses during a history pass: at most two a second.
const PROGRESS_EMIT_MS: i64 = 500;
/// Object notifications are batched this long outside history pages.
const NOTIFY_DEBOUNCE: Duration = Duration::from_millis(100);
/// Deadline of one history page or id fetch (EOSE).
const PAGE_TIMEOUT: Duration = Duration::from_secs(15);
/// Deadline of one publish (OK) per relay.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);
/// A stream holding bases reads 1078 history back to its newest base's
/// created_at minus this.
const BASE_LOOKBACK_S: i64 = 86_400;
/// Store meta: per stream scope, the unix second up to which its history
/// below the window was swept for objects no base covers (JSON map).
const UNBASED_SWEPT: &str = "unbased-swept";
/// A device with no cursor yet subscribes live from this long before now.
const LIVE_COLD_START_LOOKBACK_S: i64 = 300;
/// Deafness watchdog period.
const WATCHDOG_PERIOD: Duration = Duration::from_secs(60);
/// A change group a whole-stream reconcile could not complete is retired
/// once its newest part is this old: the rest was deleted (compaction), not
/// still being published. The engine's group expiry.
const CHANGE_GROUP_GRACE_S: i64 = 300;
/// Objects whose orphans a pass resolves per hold of the import lock.
const ORPHAN_BATCH: usize = 16;

/// A keyring the sync layer can share across tasks.
pub trait Keyring: SpaceKeyring + MaybeSend + MaybeSync {}
impl<T: SpaceKeyring + MaybeSend + MaybeSync + ?Sized> Keyring for T {}

/// What the status dot shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Backfill,
    Live,
    Error,
}

/// One sync status report (backend.ts `SyncStatus`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncStatus {
    pub phase: Phase,
    /// Changes imported this session.
    pub imported: Option<u64>,
    pub detail: Option<String>,
    /// Changes written locally but not yet published.
    pub pending: Option<u64>,
    /// Items imported by the history pass in flight.
    pub received: Option<u64>,
    /// Ids the pass's reconciles said are missing so far (approximate).
    pub expected: Option<u64>,
    /// One full history walk has completed on this device.
    pub bootstrapped: bool,
}

/// A shared space as the backend derives it from the keyring and the
/// space object's members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSpaceInfo {
    pub space_id: String,
    pub key_hex: String,
    pub key_id: i64,
    /// Hex pubkeys allowed to author events.
    pub writers: Vec<String>,
    /// The administrator; `None` = this identity's own space.
    pub owner: Option<String>,
}

#[derive(Debug, Clone)]
struct SharedSpace {
    info: SharedSpaceInfo,
    /// `blind(key, "space:"+id)`: the space stream's `#h` value.
    tag: String,
    writers: HashSet<String>,
}

/// Counters for diagnostics (sync.ts `SyncStats`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub events: u64,
    pub decrypt_failures: u64,
    pub decode_failures: u64,
    pub imported: u64,
    pub checkpoints: u64,
    pub bases: u64,
    pub rebased: u64,
    /// Shared-space payloads the space's rules refused (they stay unheld).
    pub refused: u64,
    pub blinded_tags: HashSet<String>,
}

/// What the backend hears from sync. Calls are synchronous and must not
/// block: queue async work (`crate::rt::spawn`).
pub trait SyncHost: MaybeSend + MaybeSync + 'static {
    /// Objects whose stored inputs changed.
    fn on_objects(&self, ids: Vec<String>);
    fn on_status(&self, status: SyncStatus);
    /// A gift-wrapped space key arrived and was imported.
    fn on_space_key(&self);
    /// The owner of an installed shared space deleted it for everyone.
    fn on_space_vanished(&self, space_id: String);
    /// The space is vanished or left here: its invite keys are ignored.
    fn space_gone(&self, space_id: &str) -> bool;
    /// The space owning `object_id` ("" = personal); channels own themselves.
    fn space_of(&self, object_id: &str) -> String;
    /// A history pass ended: `complete` when every relay covered every
    /// stream, `reachable` once any relay answered this session.
    fn on_history_pass(&self, complete: bool, reachable: bool);
    /// A join request for a space this identity administers was recorded
    /// ([`invite::list_join_requests`]).
    fn on_join_request(&self);
}

/// A relay event row in the held-events table (sync.ts `RelayEventRow`).
#[derive(Debug, Clone)]
struct HeldRow {
    scope: String,
    event: HeldEvent,
}

/// Mutable sync state, never locked across an await.
struct State {
    stats: SyncStats,
    shared: BTreeMap<String, SharedSpace>,
    cursor: i64,
    stopped: bool,
    started: bool,
    /// Some relay accepted a connection this session.
    relay_reached: bool,
    live_up: bool,
    bootstrapped: bool,
    bootstrapping: bool,
    resuming: bool,
    catching_up: u32,
    watchdog_busy: bool,
    /// Host mirror of the session's replay obligations: chunk key → earliest created_at.
    replay_groups: HashMap<String, i64>,
    history_complete: bool,
    checkpoints_loaded: bool,
    /// `i64::MAX` = none (sync.ts `Infinity`).
    discarded_chunk_floor: i64,
    replay_fault_generation: u64,
    active_live_events: u32,
    active_imports: u32,
    pending_count: i64,
    queue_running: bool,
    negentropy_unsupported: HashSet<String>,
    /// The history pass in flight: (received, expected).
    pass: Option<(u64, u64)>,
    last_progress_emit: i64,
    pending_parts: HashMap<String, Vec<HeldRow>>,
    imported_parts: HashSet<String>,
    open_base_groups: HashSet<String>,
    based_objects: HashSet<String>,
    pending_objects: HashSet<String>,
    notify_scheduled: bool,
    first_walk_done: bool,
    /// A history walk completed ([`RelaySync::history_walked`]).
    history_walked: bool,
    /// Covered change ids per base hash (`Inner::base_covers`).
    base_covered: HashMap<String, Arc<HashSet<String>>>,
    live_tasks: Vec<futures::future::AbortHandle>,
    live_seen: HashSet<String>,
    /// `Some` in a lite session: what it holds whole and what it follows.
    lite: Option<lite::Follow>,
    /// A live resubscribe is scheduled (lite: new objects to follow).
    resubscribe_scheduled: bool,
}

pub(crate) struct Inner {
    id: u64,
    engine: Engine,
    store: Arc<dyn ChangeStore>,
    keys: Keys,
    pk: String,
    /// NIP-44 self conversation key, hex, as the engine's `wire` takes it.
    conversation_key: String,
    secret_hex: String,
    relays: Vec<Relay>,
    keyring: Arc<dyn Keyring>,
    local: Arc<dyn LocalKv>,
    host: Arc<dyn SyncHost>,
    state: Mutex<State>,
    /// Serializes imports (sync.ts `importChain`).
    import_lock: futures::lock::Mutex<()>,
    /// Serializes cursor persistence (`cursorChain`).
    cursor_lock: futures::lock::Mutex<()>,
    /// One history pass at a time (`backfillChain`).
    backfill_lock: futures::lock::Mutex<()>,
    session_lock: futures::lock::Mutex<()>,
    /// One NIP-77 negotiation at a time per relay, by relay index.
    negotiations: Vec<futures::lock::Mutex<()>>,
    /// One history `REQ` page at a time per relay, by relay index.
    fetches: Vec<futures::lock::Mutex<()>>,
    live_tx: futures::channel::mpsc::UnboundedSender<live::LiveMsg>,
    live_rx: Mutex<Option<futures::channel::mpsc::UnboundedReceiver<live::LiveMsg>>>,
}

/// The engine's receive session is process-global (one per engine
/// natively) and single-flight: only its owner may feed or close it, so a
/// newer instance retires an older one (sync.ts `sessionOwner`).
static SESSION_OWNER: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn now_ms() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn now_s() -> i64 {
    now_ms() / 1000
}

fn short(text: impl std::fmt::Display, max: usize) -> String {
    text.to_string().chars().take(max).collect()
}

/// An event id or pubkey shortened for trace lines.
fn short_id(id: &str) -> &str {
    roostr_core::sync_trace::head(id, 8)
}

/// One relay event for a trace line: id, kind, signer (`me` for this
/// identity), its `h`/`c` tags and when it was signed (UTC).
fn describe_event(event: &roostr_nostr::Event, me: &str) -> String {
    let by = if event.pubkey == me {
        "me"
    } else {
        short_id(&event.pubkey)
    };
    let hs: Vec<&str> = event.tag_values("h").collect();
    let chunk = event
        .tag("c")
        .map(|c| format!(" c={}", short_id(c)))
        .unwrap_or_default();
    let s = event.created_at % 86_400;
    format!(
        "{} kind {} by {by} h=[{}]{chunk} signed {:02}:{:02}:{:02}Z",
        short_id(&event.id),
        event.kind,
        hs.join(","),
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Relay sync for one identity; cheap to clone.
#[derive(Clone)]
pub struct RelaySync {
    inner: Arc<Inner>,
}

impl RelaySync {
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new(
        engine: Engine,
        store: Arc<dyn ChangeStore>,
        keys: Keys,
        relays: Vec<String>,
        keyring: Arc<dyn Keyring>,
        local: Arc<dyn LocalKv>,
        host: Arc<dyn SyncHost>,
    ) -> Self {
        let pk = keys.pubkey().to_string();
        let conversation_key = hex::encode(
            roostr_nostr::nip44::conversation_key(&keys, &pk).expect("own pubkey is on the curve"),
        );
        let secret_hex = keys.secret_hex();
        let relays: Vec<Relay> = relays.into_iter().map(Relay::new).collect();
        let negotiations = relays
            .iter()
            .map(|_| futures::lock::Mutex::new(()))
            .collect();
        let fetches = relays
            .iter()
            .map(|_| futures::lock::Mutex::new(()))
            .collect();
        let (live_tx, live_rx) = futures::channel::mpsc::unbounded();
        Self {
            inner: Arc::new(Inner {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                engine,
                store,
                keys,
                pk,
                conversation_key,
                secret_hex,
                relays,
                keyring,
                local,
                host,
                state: Mutex::new(State {
                    stats: SyncStats::default(),
                    shared: BTreeMap::new(),
                    cursor: 0,
                    stopped: false,
                    started: false,
                    relay_reached: false,
                    live_up: false,
                    bootstrapped: false,
                    bootstrapping: false,
                    resuming: false,
                    catching_up: 0,
                    watchdog_busy: false,
                    replay_groups: HashMap::new(),
                    history_complete: false,
                    checkpoints_loaded: false,
                    discarded_chunk_floor: i64::MAX,
                    replay_fault_generation: 0,
                    active_live_events: 0,
                    active_imports: 0,
                    pending_count: 0,
                    queue_running: false,
                    negentropy_unsupported: HashSet::new(),
                    pass: None,
                    last_progress_emit: 0,
                    pending_parts: HashMap::new(),
                    imported_parts: HashSet::new(),
                    open_base_groups: HashSet::new(),
                    based_objects: HashSet::new(),
                    pending_objects: HashSet::new(),
                    notify_scheduled: false,
                    first_walk_done: false,
                    history_walked: false,
                    base_covered: HashMap::new(),
                    live_tasks: Vec::new(),
                    live_seen: HashSet::new(),
                    lite: None,
                    resubscribe_scheduled: false,
                }),
                import_lock: futures::lock::Mutex::new(()),
                cursor_lock: futures::lock::Mutex::new(()),
                backfill_lock: futures::lock::Mutex::new(()),
                session_lock: futures::lock::Mutex::new(()),
                negotiations,
                fetches,
                live_tx,
                live_rx: Mutex::new(Some(live_rx)),
            }),
        }
    }

    /// This identity's hex pubkey.
    pub fn pubkey(&self) -> &str {
        &self.inner.pk
    }

    pub fn relays(&self) -> Vec<String> {
        self.inner
            .relays
            .iter()
            .map(|r| r.url().to_string())
            .collect()
    }

    pub fn stats(&self) -> SyncStats {
        self.inner.state.lock().stats.clone()
    }

    /// Unix seconds of the newest imported event.
    pub fn cursor(&self) -> i64 {
        self.inner.state.lock().cursor
    }

    /// Changes queued for publishing and not yet acknowledged.
    pub fn pending(&self) -> i64 {
        self.inner.state.lock().pending_count
    }

    /// Opens the session, re-offers every stored publish obligation, starts
    /// the history walk in the background, fetches gift wraps and brings the
    /// live subscriptions up. The walk's end is reported through
    /// [`SyncHost::on_history_pass`] (and [`RelaySync::await_first_walk`]).
    pub async fn start(&self) -> anyhow::Result<()> {
        self.inner.clone().start().await
    }

    pub fn stop(&self) {
        self.inner.stop();
    }

    /// Sockets may be dead (sleep, network change): drop them, resubscribe
    /// and run a history pass. Concurrent calls share one pass.
    pub fn resume(&self) {
        self.inner.clone().resume();
    }

    /// Replace the shared-space view. New spaces get a full backfill of
    /// their stream plus live subscriptions.
    pub async fn set_shared_spaces(&self, infos: Vec<SharedSpaceInfo>) -> anyhow::Result<()> {
        self.inner.clone().set_shared_spaces(infos).await
    }

    /// Record the durable obligation(s) for a committed change, then hand
    /// them to the outbox. `base`: the base the change was written on.
    pub async fn publish(
        &self,
        bytes: Vec<u8>,
        change_id: String,
        object_id: String,
        base: Option<String>,
    ) -> anyhow::Result<()> {
        self.inner
            .clone()
            .publish(bytes, change_id, object_id, base)
            .await
    }

    /// Owner duty: publish the relay write-allowlist.
    pub async fn publish_allowlist(&self, writers: Vec<String>) {
        self.inner.publish_allowlist(writers).await;
    }

    /// The h-deletion of a vanished space, sent until a relay accepts it.
    pub async fn publish_space_deletion(&self, space_id: String, key_hex: String) {
        self.inner.publish_space_deletion(space_id, key_hex).await;
    }

    /// Owner duty: gift-wrap the current key to every member that has not
    /// received this keyId yet.
    pub async fn send_invite_wraps(
        &self,
        space_id: String,
        key_hex: String,
        key_id: i64,
        name: String,
        member_hexes: Vec<String>,
    ) {
        self.inner
            .send_invite_wraps(space_id, key_hex, key_id, name, member_hexes)
            .await;
    }

    /// Resolves once the first history walk of [`RelaySync::start`] ended.
    pub async fn await_first_walk(&self) {
        while !self.inner.state.lock().first_walk_done {
            roostr_nostr::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A history walk completed here: what the relays hold is known, so a
    /// shared space owes them only the changes they lack. Until then a host
    /// queues no space publishes (they would re-seal what another device
    /// already published); the walk that completes it calls
    /// [`SyncHost::on_space_key`] so the host owes them then.
    pub fn history_walked(&self) -> bool {
        self.inner.state.lock().history_walked
    }

    /// Resolves once no history pass, import, live event or publish is in
    /// flight and the outbox is empty (or sync stopped).
    pub async fn await_idle(&self) {
        loop {
            let idle = {
                let st = self.inner.state.lock();
                st.stopped
                    || (!st.bootstrapping
                        && !st.resuming
                        && st.pass.is_none()
                        && st.active_imports == 0
                        && st.active_live_events == 0
                        && st.pending_count == 0
                        && !st.queue_running)
            };
            if idle && self.inner.import_lock.try_lock().is_some() {
                return;
            }
            roostr_nostr::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Inner {
    fn stopped(&self) -> bool {
        self.state.lock().stopped
    }

    fn session_open(&self) -> bool {
        *SESSION_OWNER.lock().unwrap_or_else(|e| e.into_inner()) == self.id
    }

    fn session_spaces(&self) -> Vec<SessionSpace> {
        self.state
            .lock()
            .shared
            .values()
            .map(|sp| SessionSpace {
                space_id: sp.info.space_id.clone(),
                key_hex: sp.info.key_hex.clone(),
                key_id: sp.info.key_id,
                // A keyring entry without an imported owner is this identity's own space.
                owner: Some(sp.info.owner.clone().unwrap_or_else(|| self.pk.clone())),
            })
            .collect()
    }

    /// (Re)open the engine session from this instance's cursor and the
    /// persisted obligations.
    async fn open_session(&self) -> anyhow::Result<()> {
        let replay_groups = self.store.replay_groups().await?;
        let bases = self.store.current_bases().await?;
        let cursor = {
            let mut st = self.state.lock();
            st.based_objects = bases.into_keys().collect();
            st.cursor
        };
        let state = self
            .engine
            .session_open(&SessionOpen {
                pk: self.pk.clone(),
                conversation_key: self.conversation_key.clone(),
                secret: Some(self.secret_hex.clone()),
                spaces: self.session_spaces(),
                cursor,
                replay_groups,
                imported_groups: Vec::new(),
            })
            .await?;
        *SESSION_OWNER.lock().unwrap_or_else(|e| e.into_inner()) = self.id;
        self.state.lock().replay_groups = state.replay_groups.into_iter().collect();
        Ok(())
    }

    /// Paths that run before `start` open the session on first need.
    async fn ensure_session(&self) -> anyhow::Result<()> {
        if self.session_open() || self.stopped() {
            return Ok(());
        }
        let _guard = self.session_lock.lock().await;
        if self.session_open() || self.stopped() {
            return Ok(());
        }
        self.open_session().await
    }

    /// Give up session ownership; true when this instance held it.
    fn release_session(id: u64) -> bool {
        let mut owner = SESSION_OWNER.lock().unwrap_or_else(|e| e.into_inner());
        let held = *owner == id;
        if held {
            *owner = 0;
        }
        held
    }

    // ── Status ─────────────────────────────────────────────────────

    fn emit(&self, status: SyncStatus) {
        self.host.on_status(status);
    }

    /// A sync error: on the status line and in the log (the console), whole.
    fn emit_error(&self, detail: String) {
        log::warn!("[sync] error: {detail}");
        let bootstrapped = self.state.lock().bootstrapped;
        self.emit(SyncStatus {
            phase: Phase::Error,
            imported: None,
            detail: Some(detail),
            pending: None,
            received: None,
            expected: None,
            bootstrapped,
        });
    }

    fn emit_backfill(&self, detail: Option<String>) {
        let (imported, bootstrapped) = {
            let st = self.state.lock();
            (st.stats.imported, st.bootstrapped)
        };
        self.emit(SyncStatus {
            phase: Phase::Backfill,
            imported: Some(imported),
            detail,
            pending: None,
            received: None,
            expected: None,
            bootstrapped,
        });
    }

    /// The dot answers "am I connected and is my work published"; history
    /// verification in the background belongs in the detail text.
    fn emit_live_status(&self) {
        let status = {
            let mut st = self.state.lock();
            let live = st.live_up && st.catching_up == 0;
            st.last_progress_emit = now_ms();
            let detail = if !st.live_up {
                None
            } else if live {
                (!st.history_complete).then(|| {
                    format!(
                        "{} · {} changes so far",
                        if st.checkpoints_loaded {
                            "verifying full history in background"
                        } else {
                            "loading spaces"
                        },
                        st.stats.imported
                    )
                })
            } else {
                Some("catching up".to_string())
            };
            SyncStatus {
                phase: if live { Phase::Live } else { Phase::Backfill },
                imported: Some(st.stats.imported),
                detail,
                pending: Some(st.pending_count.max(0) as u64),
                received: st.pass.map(|(received, _)| received),
                expected: st.pass.and_then(|(_, e)| (e > 0).then_some(e)),
                bootstrapped: st.bootstrapped,
            }
        };
        self.emit(status);
    }

    /// Progress during a pass: at most one status per [`PROGRESS_EMIT_MS`].
    fn emit_progress(&self) {
        let due = now_ms() - self.state.lock().last_progress_emit >= PROGRESS_EMIT_MS;
        if due {
            self.emit_live_status();
        }
    }

    // ── Object notifications ───────────────────────────────────────

    fn flush_object_notify(&self) {
        let ids: Vec<String> = {
            let mut st = self.state.lock();
            st.pending_objects.drain().collect()
        };
        if !ids.is_empty() {
            roostr_core::sync_trace!("objects changed, re-reading: {}", ids.join(", "));
            self.host.on_objects(ids);
        }
    }

    fn schedule_object_notify(self: &Arc<Self>) {
        {
            let mut st = self.state.lock();
            if st.notify_scheduled || st.pending_objects.is_empty() {
                return;
            }
            st.notify_scheduled = true;
        }
        let this = self.clone();
        crate::rt::spawn(async move {
            roostr_nostr::sleep(NOTIFY_DEBOUNCE).await;
            this.state.lock().notify_scheduled = false;
            this.flush_object_notify();
        });
    }

    // ── Lifecycle ──────────────────────────────────────────────────

    async fn start(self: Arc<Self>) -> anyhow::Result<()> {
        {
            let mut st = self.state.lock();
            anyhow::ensure!(!st.started, "relay sync already started");
            st.started = true;
        }
        // A device that walked from a manifest floor never held the older
        // history: this also clears its bootstrapped flag, so it walks from zero.
        self.store.forget_checkpoint_floors().await?;
        let cursor = self.store.cursor().await?;
        let bootstrapped = self.store.bootstrapped().await?;
        {
            let mut st = self.state.lock();
            st.cursor = cursor;
            st.bootstrapped = bootstrapped;
        }
        self.open_session().await?;
        // Lite: follow what the store holds (the backend prepared every state).
        let lite = self.state.lock().lite.is_some();
        if lite {
            self.refresh_followed().await?;
        }
        // Space obligations wait for a completed walk (`release_space_pending`).
        for pending in self.store.pending_publishes().await? {
            if pending.space_id.is_none() {
                self.clone().offer_to_outbox(&pending).await;
            }
        }
        self.emit(SyncStatus {
            phase: Phase::Backfill,
            imported: Some(0),
            detail: None,
            pending: None,
            received: None,
            expected: None,
            bootstrapped,
        });

        // `since = cursor+1` is only sound once ONE full walk completed here:
        // the cursor tracks the NEWEST import, so an interrupted first walk
        // would otherwise skip everything older, forever. The walk runs in
        // the background; its progress persists page by page via the floor.
        // A lite session never walks the vault: it has no floor to resume.
        let floor = if bootstrapped || lite {
            None
        } else {
            self.store.bootstrap_floor().await?
        };
        self.state.lock().bootstrapping = true;
        let this = self.clone();
        crate::rt::spawn(async move {
            let since = if bootstrapped { cursor + 1 } else { 1 };
            let mut walked = this.backfill(since, floor).await;
            // Space obligations restored at start may be history another
            // device of the account already published under the space key,
            // older than the cursor: walk every stream from the start (only
            // what is not held is fetched), which marks what the relays
            // hold, before they are released and the rest sealed.
            if bootstrapped
                && walked.is_ok()
                && !this.stopped()
                && this.space_pending_restored().await
            {
                walked = this.backfill(1, None).await;
            }
            match walked {
                Ok(complete) if !this.stopped() => {
                    if !bootstrapped && complete {
                        this.mark_bootstrapped().await;
                    }
                    if complete {
                        this.note_history_walked();
                    }
                    this.emit_live_status();
                }
                Ok(_) => {}
                Err(error) => {
                    if !this.stopped() {
                        this.emit_error(format!("{error:#}"));
                    }
                }
            }
            {
                let mut st = this.state.lock();
                st.bootstrapping = false;
                st.first_walk_done = true;
            }
            // Obligations restored at start waited for this walk, which
            // marked what the relays hold; even an incomplete one must not
            // strand them (a held base still settles a compacted history).
            if !this.stopped()
                && let Err(error) = this.release_space_pending().await
            {
                log::warn!("[sync] releasing space obligations: {error}");
            }
        });

        if self.stopped() {
            return Ok(());
        }
        // Gift wraps addressed to us: created_at is randomized, so no
        // cursor - the seen-set dedupes.
        self.fetch_wraps().await;
        self.fetch_keyring_backup().await;
        self.clone().spawn_live_worker();
        self.subscribe_live();
        self.clone().spawn_watchdog();
        self.emit_live_status();
        Ok(())
    }

    /// Never in a lite session: it holds only what it opened.
    async fn mark_bootstrapped(&self) {
        if self.state.lock().lite.is_some() {
            return;
        }
        match self.store.set_bootstrapped().await {
            Ok(()) => self.state.lock().bootstrapped = true,
            Err(error) => log::warn!("[sync] bootstrapped flag: {error}"),
        }
    }

    fn stop(&self) {
        let tasks = {
            let mut st = self.state.lock();
            st.stopped = true;
            st.live_up = false;
            std::mem::take(&mut st.live_tasks)
        };
        for task in tasks {
            task.abort();
        }
        if self.session_open() {
            // Closing is an engine call: fire it without blocking the caller.
            let engine = self.engine.clone();
            let id = self.id;
            crate::rt::spawn(async move {
                if Self::release_session(id) {
                    let _ = engine.session_close().await;
                }
            });
        }
        for relay in &self.relays {
            relay.disconnect();
        }
        self.live_tx.close_channel();
    }

    fn resume(self: Arc<Self>) {
        {
            let mut st = self.state.lock();
            if st.stopped || !st.live_up || st.resuming {
                return;
            }
            st.resuming = true;
            st.catching_up += 1;
        }
        crate::rt::spawn(async move {
            self.emit_live_status();
            for relay in &self.relays {
                relay.disconnect();
            }
            self.subscribe_live();
            let result = self.catchup_from_store().await;
            {
                let mut st = self.state.lock();
                st.catching_up -= 1;
                st.resuming = false;
            }
            match result {
                Err(error) if !self.stopped() => self.emit_error(format!("{error:#}")),
                _ if !self.stopped() => {
                    self.wake_outbox().await;
                    self.emit_live_status();
                }
                _ => {}
            }
        });
    }

    /// A history pass under start's rules (incremental once bootstrapped),
    /// then every gift wrap again.
    async fn catchup_from_store(self: &Arc<Self>) -> anyhow::Result<bool> {
        let bootstrapped = self.store.bootstrapped().await?;
        let (since, floor) = if bootstrapped {
            (self.state.lock().cursor + 1, None)
        } else {
            (1, self.store.bootstrap_floor().await?)
        };
        let complete = self.catchup(since, floor).await?;
        if complete && !bootstrapped && !self.stopped() {
            self.mark_bootstrapped().await;
        }
        if complete && !self.stopped() {
            self.note_history_walked();
        }
        Ok(complete)
    }

    /// A completed walk: the relays' holdings are marked, so space
    /// obligations they already hold are dropped and the rest offered
    /// ([`RelaySync::history_walked`]); the host then owes its shared spaces
    /// what is still missing.
    fn note_history_walked(self: &Arc<Self>) {
        let this = self.clone();
        crate::rt::spawn(async move {
            let first = !std::mem::replace(&mut this.state.lock().history_walked, true);
            if let Err(error) = this.release_space_pending().await {
                log::warn!("[sync] releasing space obligations: {error}");
            }
            if first {
                this.host.on_space_key();
            }
        });
    }

    /// One history pass, then every gift wrap again (randomized created_at:
    /// re-queried on every catch-up; the seen-set dedupes).
    async fn catchup(
        self: &Arc<Self>,
        since: i64,
        resume_until: Option<i64>,
    ) -> anyhow::Result<bool> {
        let complete = self.backfill(since, resume_until).await?;
        self.fetch_wraps().await;
        self.fetch_keyring_backup().await;
        Ok(complete)
    }

    /// Live subscriptions start after the cursor; a device that has
    /// imported nothing starts shortly before now (history covers the rest).
    fn live_since(&self) -> i64 {
        let cursor = self.state.lock().cursor;
        if cursor > 0 {
            cursor + 1
        } else {
            now_s() - LIVE_COLD_START_LOOKBACK_S
        }
    }

    // ── Shared spaces ──────────────────────────────────────────────

    async fn set_shared_spaces(self: Arc<Self>, infos: Vec<SharedSpaceInfo>) -> anyhow::Result<()> {
        let mut next = BTreeMap::new();
        for info in infos {
            if !is_hex64(&info.key_hex) {
                continue;
            }
            let tag = self
                .engine
                .blind_space(&info.key_hex, &format!("space:{}", info.space_id))
                .await?;
            let writers = info.writers.iter().cloned().collect();
            next.insert(info.space_id.clone(), SharedSpace { info, tag, writers });
        }
        let (fresh, live_up, stopped) = {
            let mut st = self.state.lock();
            let previous: HashSet<&str> = st.shared.values().map(|sp| sp.tag.as_str()).collect();
            let fresh = next.values().any(|sp| !previous.contains(sp.tag.as_str()));
            st.shared = next;
            (fresh, st.live_up, st.stopped)
        };
        if self.session_open() {
            self.engine.session_spaces(&self.session_spaces()).await?;
        }
        // Lite: a space's key decides the tag its objects are followed by.
        self.refresh_followed().await?;
        if stopped || !live_up {
            return Ok(()); // start() wires the subscriptions itself
        }
        self.subscribe_live();
        // A newly joined space's whole stream is unseen. Walk it before
        // returning: the caller then owes the relays only the changes that
        // stream lacks - a space another device of the account shared
        // already holds its history there, and re-sealing all of it would
        // flood the relay. Until a walk completes, nothing is owed
        // ([`RelaySync::history_walked`]).
        if fresh {
            let walked = self.backfill(1, None).await;
            let vault_walked = self.state.lock().history_walked;
            if !matches!(walked, Ok(true)) {
                self.state.lock().history_walked = false;
            } else if vault_walked && let Err(error) = self.release_space_pending().await {
                log::warn!("[sync] releasing space obligations: {error}");
            }
            walked?;
        }
        Ok(())
    }

    /// The space a write to `object_id` is shared under, if installed.
    fn space_for(&self, object_id: &str) -> Option<SharedSpaceInfo> {
        let space_id = self.host.space_of(object_id);
        self.state
            .lock()
            .shared
            .get(&space_id)
            .map(|sp| sp.info.clone())
    }

    /// This device's own orphans of a shared object are rebased only while
    /// its identity may write the space; anything else is this vault's own.
    fn may_rebase(&self, object_id: &str) -> bool {
        let space_id = self.host.space_of(object_id);
        let st = self.state.lock();
        match st.shared.get(&space_id) {
            None => true,
            Some(sp) => {
                sp.writers.contains(&self.pk)
                    || sp.info.owner.as_deref().unwrap_or(&self.pk) == self.pk
            }
        }
    }

    /// This event's held rows: the self stream when we signed it, and every
    /// installed space stream it is tagged into.
    fn held_rows(&self, event: &roostr_nostr::Event) -> Vec<HeldRow> {
        let b = (event.kind == roostr_nostr::kinds::CHANGE)
            .then(|| event.tag("b").map(str::to_string))
            .flatten();
        let row = |scope: String| HeldRow {
            scope,
            event: HeldEvent {
                id: event.id.clone(),
                created_at: event.created_at as i64,
                kind: event.kind,
                b: b.clone(),
            },
        };
        let mut rows = Vec::new();
        if event.pubkey == self.pk {
            rows.push(row(String::new()));
        }
        let st = self.state.lock();
        for sp in st.shared.values() {
            if event.tag_values("h").any(|h| h == sp.tag) {
                rows.push(row(sp.tag.clone()));
            }
        }
        rows
    }

    async fn record_held(&self, rows: Vec<HeldRow>) -> anyhow::Result<()> {
        let mut by_scope: BTreeMap<String, Vec<HeldEvent>> = BTreeMap::new();
        for row in rows {
            by_scope.entry(row.scope).or_default().push(row.event);
        }
        for (scope, events) in by_scope {
            self.store.record_held(scope, events).await?;
        }
        Ok(())
    }
}

fn is_hex64(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

//! The replica's on-device change store.
//!
//! Everything a daemon-less Roostr host persists between launches: the
//! content-addressed changes of every object (exact wire bytes plus the
//! engine's decoded JSON), publication obligations, relay cursors and
//! bootstrap floors, kind-1079 checkpoints, kind-31078 bases and the current
//! base per object, the relay events held per stream (the local item set of
//! NIP-77 reconciliation) and the replay memo.
//!
//! [`ChangeStore`] is the union of the iOS `ChangeStore` + `MetaStore` +
//! `ReplayCacheStore` (RoostrIOS `SQLiteChangeStore.swift`) and the browser
//! `ChangeStoreApi` (RoostrWebsite `engine/store.ts`). Two backends:
//! [`SqliteChangeStore`] natively and [`IdbChangeStore`] on wasm32.
//!
//! Every composite operation that must be atomic (a local change with its
//! publication obligation, a rebase with its orphan mark, the cursor with its
//! replay groups, publication with its pending removal, a base with its
//! stream floors and current pointer, a page of relay imports with its
//! publication marks and held events) is one method running in one storage
//! transaction; callers never stitch writes together.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(target_family = "wasm")]
mod idb;
#[cfg(not(target_family = "wasm"))]
mod sqlite;

#[cfg(target_family = "wasm")]
pub use idb::{IDB_TIMEOUT_MS, IdbChangeStore, svelte_import};
#[cfg(not(target_family = "wasm"))]
pub use sqlite::SqliteChangeStore;

#[cfg(not(target_family = "wasm"))]
mod bounds {
    use super::*;
    /// `Send` natively, nothing on wasm (same convention as `roostr_core::io`).
    pub trait MaybeSend: Send {}
    impl<T: Send + ?Sized> MaybeSend for T {}
    /// `Sync` natively, nothing on wasm.
    pub trait MaybeSync: Sync {}
    impl<T: Sync + ?Sized> MaybeSync for T {}
    /// A boxed future: `Send` natively, local on wasm.
    pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
}

#[cfg(target_family = "wasm")]
mod bounds {
    use super::*;
    /// `Send` natively, nothing on wasm (same convention as `roostr_core::io`).
    pub trait MaybeSend {}
    impl<T: ?Sized> MaybeSend for T {}
    /// `Sync` natively, nothing on wasm.
    pub trait MaybeSync {}
    impl<T: ?Sized> MaybeSync for T {}
    /// A boxed future: `Send` natively, local on wasm.
    pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;
}

pub use bounds::{BoxFuture, MaybeSend, MaybeSync};

/// What every [`ChangeStore`] method returns.
pub type StoreFuture<'a, T> = BoxFuture<'a, Result<T, StoreError>>;

/// Superseded bases kept per object beside the current one (iOS
/// `baseChainKept`): bases at least this many epochs below the newest are
/// pruned when a new base is stored.
pub const BASE_CHAIN_KEPT: i64 = 8;

/// Store failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The database itself is unreachable or stopped responding (IndexedDB
    /// timeout, blocked upgrade, private mode), as opposed to bad data.
    #[error("local storage unavailable: {0}")]
    Unavailable(String),
    /// The store was closed; every later call fails with this.
    #[error("store is closed")]
    Closed,
    /// A stored row does not decode (or a value does not encode).
    #[error("corrupt store row: {0}")]
    Corrupt(String),
    /// The backend rejected an operation.
    #[error("storage: {0}")]
    Backend(String),
}

/// One stored change: exact wire bytes plus the engine's decoded JSON text
/// (`codec decode` shape, kept verbatim). `b` is the base hash its event
/// named (`["b", hash]`; `None` = pre-migration or written with no base);
/// `rebased` marks a change folded away here - an orphan this device
/// rebased, another device's leftover, an own change proven folded: kept
/// as history, never replayed again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRecord {
    pub id: String,
    pub object_id: String,
    pub bytes: Vec<u8>,
    pub json: String,
    pub b: Option<String>,
    pub rebased: bool,
}

/// Durable publication obligation, keyed `changeId` (personal) or
/// `spaceId/keyId/changeId` (shared). Serialized exactly like the iOS
/// `PendingPublish` Codable (camelCase, `bytes` base64).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPublish {
    pub key: String,
    pub object_id: String,
    pub change_id: String,
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<i64>,
    /// Hash of the base the change was written on: its events carry `["b", base]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Exact signed Nostr events retained across retries and relaunches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<serde_json::Value>>,
}

/// One object's kind-1079 checkpoint. Supersession is the core's rule
/// ([`CheckpointRecord::supersedes`]); `covered` is informational.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRecord {
    pub object_id: String,
    pub bytes: Vec<u8>,
    pub hash: String,
    /// Sorted head change ids at checkpoint time.
    pub heads: Vec<String>,
    pub covered: u64,
}

impl CheckpointRecord {
    /// Whether `self` replaces `held`: the engine's one store rule
    /// (`core.checkpoint_supersedes` over both payloads) - `self` covers a
    /// strict superset of `held`'s change ids, or the same set with the
    /// larger sha256 of its bytes. An incomparable fork (another device's
    /// branch) never replaces the held one, however many changes it covers;
    /// never a timestamp.
    pub async fn supersedes(
        &self,
        held: &CheckpointRecord,
        engine: &roostr_engine::Engine,
    ) -> Result<bool, StoreError> {
        engine
            .checkpoint_supersedes(&self.bytes, &held.bytes)
            .await
            .map_err(|e| StoreError::Corrupt(format!("checkpoint {}: {e}", self.object_id)))
    }
}

/// One object's base (kind 31078): a Checkpoint protobuf of epoch >= 1 as
/// received. `hash` is its identity, `created_at` unix ms, `prev_base` the
/// previous base's hash ("" at epoch 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseRecord {
    pub object_id: String,
    pub hash: String,
    pub epoch: i64,
    pub created_at: i64,
    pub prev_base: String,
    pub bytes: Vec<u8>,
}

impl BaseRecord {
    /// The state-sync current-base order: epoch, then `created_at`, then hash.
    pub fn outranks(&self, other: &BaseRecord) -> bool {
        (self.epoch, self.created_at, &self.hash) > (other.epoch, other.created_at, &other.hash)
    }

    /// The current base among `bases` (the highest by [`BaseRecord::outranks`]).
    pub fn current(bases: &[BaseRecord]) -> Option<&BaseRecord> {
        bases
            .iter()
            .max_by(|a, b| (a.epoch, a.created_at, &a.hash).cmp(&(b.epoch, b.created_at, &b.hash)))
    }
}

/// An object's current base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentBase {
    pub hash: String,
    pub created_at: i64,
}

/// A relay event this device holds within one stream scope ("" = the self
/// stream, else the space stream's `#h` value). `b` is the delta's
/// `["b", base]` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldEvent {
    pub id: String,
    pub created_at: i64,
    pub kind: u32,
    pub b: Option<String>,
}

/// Which held events of a scope make a reconciliation's local item set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeldQuery {
    /// Kinds to include (empty = none).
    pub kinds: Vec<u32>,
    /// Inclusive lower `created_at` bound.
    pub since: Option<i64>,
    /// Inclusive upper `created_at` bound.
    pub until: Option<i64>,
    /// Keep only deltas on these bases (`#b` filter).
    pub b: Option<Vec<String>>,
}

/// One NIP-77 item: results are sorted by `(created_at, id)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HeldItem {
    pub created_at: i64,
    pub id: String,
}

/// What one page of relay imports writes, as one transaction
/// ([`ChangeStore::import`]): the page's changes, the publication keys they
/// settle and the relay events now held. A page of a first sync is a few
/// hundred events; writing it as one transaction instead of one per change
/// is what keeps a large vault's first sync fast on a slow disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportBatch {
    /// Added unless held (by id, also within the batch), as [`ChangeStore::add_changes`].
    pub changes: Vec<ChangeRecord>,
    /// Marked published, any pending obligation removed, as [`ChangeStore::mark_published`].
    pub published: Vec<String>,
    /// `(scope, event)`: remembered as [`ChangeStore::record_held`] does.
    pub held: Vec<(String, HeldEvent)>,
}

impl ImportBatch {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.published.is_empty() && self.held.is_empty()
    }
}

/// Exactly what a replay consumes for one object: its stored change ids
/// (rebased orphans excluded) in insertion order, the checkpoint it seeds
/// from and the bases it holds. Equal inputs replay to equal states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayInput {
    pub change_count: u64,
    /// [`ReplayInput::digest`] of the stored change ids.
    pub change_digest: String,
    /// Checkpoint hash; "" for none.
    pub checkpoint_hash: String,
    /// Held base hashes, sorted and comma-joined; "" for none.
    pub base_hashes: String,
}

impl ReplayInput {
    /// The input of `change_ids` (replay order) over a checkpoint and bases.
    pub fn new<'a>(
        change_ids: impl IntoIterator<Item = &'a str>,
        checkpoint_hash: &str,
        base_hashes: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let mut hasher = Sha256::new();
        let mut count = 0u64;
        for id in change_ids {
            hasher.update(id.as_bytes());
            hasher.update(b"\n");
            count += 1;
        }
        let mut bases: Vec<&str> = base_hashes.into_iter().collect();
        bases.sort_unstable();
        Self {
            change_count: count,
            change_digest: hex(&hasher.finalize()),
            checkpoint_hash: checkpoint_hash.to_owned(),
            base_hashes: bases.join(","),
        }
    }

    /// sha256 hex over each id followed by a newline (iOS `ReplayInput.digest`).
    pub fn digest<'a>(change_ids: impl IntoIterator<Item = &'a str>) -> String {
        Self::new(change_ids, "", []).change_digest
    }
}

/// One persisted replay: the state the engine produced from `input`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedReplay {
    pub object_id: String,
    pub input: ReplayInput,
    pub state: Vec<u8>,
}

impl CachedReplay {
    /// A memo is served only for exactly the input it was written from
    /// (the engine version is matched by [`ChangeStore::cached_replays`]).
    pub fn serves(&self, current: &ReplayInput) -> bool {
        &self.input == current
    }
}

/// Everything held for one object that its state replays from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectInputs {
    /// Stored changes in insertion order, rebased orphans excluded.
    pub changes: Vec<ChangeRecord>,
    /// Every held base (current and superseded), in current-base order.
    pub bases: Vec<BaseRecord>,
    pub checkpoint: Option<CheckpointRecord>,
    /// Stored change rows, rebased ones included.
    pub count: u64,
    pub current_base: Option<CurrentBase>,
}

/// One object's raw history for a corpus load.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectHistory {
    pub object_id: String,
    pub checkpoint: Option<Vec<u8>>,
    /// Change bytes in insertion order.
    pub changes: Vec<Vec<u8>>,
}

/// Meta keys shared by both backends.
pub(crate) mod keys {
    pub const CURSOR: &str = "cursor";
    pub const REPLAY_GROUPS: &str = "replay-groups";
    pub const BOOTSTRAPPED: &str = "bootstrapped";
    pub const BOOTSTRAP_FLOOR: &str = "bootstrap-floor";
    pub const CHECKPOINT_FLOORS: &str = "checkpoint-floors";
    pub const BASE_STREAMS: &str = "base-streams";
    pub const PENDING: &str = "pending:";
    pub const PUBLISHED: &str = "published:";
    /// `own:<changeId>` → object id: written on this device, kept until a
    /// held base proves it folded (docs/state-sync.md "Orphans and rebase").
    pub const OWN: &str = "own:";
}

/// Persistence of a replica host. All methods are idempotent where the
/// operation allows; writes of one method commit atomically.
pub trait ChangeStore: MaybeSend + MaybeSync {
    // ---- changes ----

    /// Adds records not already held (by id, also within the batch), each
    /// with the next insertion sequence. Returns how many were new.
    fn add_changes(&self, records: Vec<ChangeRecord>) -> StoreFuture<'_, usize>;
    /// Atomically writes one page of relay imports ([`ImportBatch`]).
    /// Returns how many of its changes were new.
    fn import(&self, batch: ImportBatch) -> StoreFuture<'_, usize>;
    /// Atomically stores a change this device wrote, its publication
    /// obligation and its own mark (kept after publication, until a held
    /// base proves it folded).
    fn add_local_change(
        &self,
        record: ChangeRecord,
        pending: PendingPublish,
    ) -> StoreFuture<'_, ()>;
    /// Atomically stores a rebased change, marks `orphan_id` rebased and
    /// moves the own mark from the orphan to the rebased change (only own
    /// orphans are rebased).
    fn add_rebased(&self, orphan_id: String, record: ChangeRecord) -> StoreFuture<'_, ()>;
    /// Folds changes away: marked rebased (kept as history, never replayed
    /// again) and their own marks dropped - another device's leftovers and
    /// own changes proven folded.
    fn mark_rebased(&self, ids: Vec<String>) -> StoreFuture<'_, ()>;
    /// Every stored change of the object, rebased included, in insertion order.
    fn changes_for(&self, object_id: String) -> StoreFuture<'_, Vec<ChangeRecord>>;
    /// The stored changes of `object_ids` (rebased included, each object's
    /// in insertion order) whose publication key - `key_prefix` followed by
    /// the change id - is not marked published. One read, however many
    /// changes: what a shared space's publication sweep owes.
    fn unpublished_changes(
        &self,
        object_ids: Vec<String>,
        key_prefix: String,
    ) -> StoreFuture<'_, Vec<ChangeRecord>>;
    /// Every object held as changes, a checkpoint or a base: objects with
    /// changes first, by their first change's insertion, then the rest.
    fn object_ids(&self) -> StoreFuture<'_, Vec<String>>;
    /// Stored change rows per object (rebased included).
    fn change_counts(&self) -> StoreFuture<'_, HashMap<String, u64>>;
    /// What one object's state replays from.
    fn object_inputs(&self, object_id: String) -> StoreFuture<'_, ObjectInputs>;
    /// Every object's change bytes and checkpoint, for a corpus load.
    fn all_change_histories(&self) -> StoreFuture<'_, Vec<ObjectHistory>>;

    // ---- bases ----

    /// Holds a base unless already held (by object and hash), re-points the
    /// object's current base and prunes bases [`BASE_CHAIN_KEPT`] epochs
    /// below the newest. Each of `streams` ("" = personal, else a space tag)
    /// moves its newest-base floor forward to the base's `created_at`, also
    /// for a base already held. Returns whether the base was new.
    fn put_base(&self, record: BaseRecord, streams: Vec<String>) -> StoreFuture<'_, bool>;
    /// Every base held for the object, in current-base order (current last).
    fn bases(&self, object_id: String) -> StoreFuture<'_, Vec<BaseRecord>>;
    fn current_base(&self, object_id: String) -> StoreFuture<'_, Option<CurrentBase>>;
    /// Object id -> current base.
    fn current_bases(&self) -> StoreFuture<'_, HashMap<String, CurrentBase>>;
    /// Hash of each object's current base, sorted.
    fn current_base_hashes(&self) -> StoreFuture<'_, Vec<String>>;
    /// Objects held as changes with no base, sorted.
    fn objects_without_base(&self) -> StoreFuture<'_, Vec<String>>;
    /// Stream scope -> `created_at` (unix ms) of the newest base held from it.
    fn newest_base_by_stream(&self) -> StoreFuture<'_, HashMap<String, i64>>;

    // ---- checkpoints ----

    fn checkpoint(&self, object_id: String) -> StoreFuture<'_, Option<CheckpointRecord>>;
    /// Stores `record` when it [`supersedes`](CheckpointRecord::supersedes)
    /// the held one (or none is held), asking `engine` for the rule inside
    /// the write transaction. Returns whether it was stored.
    fn put_checkpoint(
        &self,
        engine: &roostr_engine::Engine,
        record: CheckpointRecord,
    ) -> StoreFuture<'_, bool>;
    fn all_checkpoints(&self) -> StoreFuture<'_, HashMap<String, CheckpointRecord>>;
    /// Drops the obsolete `checkpoint-floors` meta row and, when any floor
    /// was positive, the bootstrapped flag with it.
    fn forget_checkpoint_floors(&self) -> StoreFuture<'_, ()>;

    // ---- meta ----

    /// Host meta row (relay lists, wraps seen, join requests, ...).
    fn meta(&self, key: String) -> StoreFuture<'_, Option<String>>;
    /// Upserts the row; `None` deletes it.
    fn set_meta(&self, key: String, value: Option<String>) -> StoreFuture<'_, ()>;
    /// Relay cursor (unix seconds of the newest imported event); 0 when unset.
    fn cursor(&self) -> StoreFuture<'_, i64>;
    /// Persists the cursor and, when given, the replay groups, atomically.
    fn set_cursor(
        &self,
        cursor: i64,
        replay_groups: Option<Vec<(String, i64)>>,
    ) -> StoreFuture<'_, ()>;
    /// Unresolved replay group keys with their earliest event timestamps.
    fn replay_groups(&self) -> StoreFuture<'_, Vec<(String, i64)>>;
    /// True once one complete history walk finished on this device.
    fn bootstrapped(&self) -> StoreFuture<'_, bool>;
    fn set_bootstrapped(&self) -> StoreFuture<'_, ()>;
    /// Resume point of an interrupted first walk.
    fn bootstrap_floor(&self) -> StoreFuture<'_, Option<i64>>;
    fn set_bootstrap_floor(&self, floor: Option<i64>) -> StoreFuture<'_, ()>;

    // ---- publication ----

    /// Every pending obligation, in key order.
    fn pending_publishes(&self) -> StoreFuture<'_, Vec<PendingPublish>>;
    fn pending(&self, key: String) -> StoreFuture<'_, Option<PendingPublish>>;
    /// Upserts the obligation under its key.
    fn save_pending(&self, item: PendingPublish) -> StoreFuture<'_, ()>;
    /// Change ids with any pending obligation.
    fn pending_change_ids(&self) -> StoreFuture<'_, HashSet<String>>;
    /// Changes written on this device that no held base has proven folded
    /// yet (published or not): change id -> object id.
    fn own_changes(&self) -> StoreFuture<'_, HashMap<String, String>>;
    fn is_published(&self, key: String) -> StoreFuture<'_, bool>;
    /// Marks published and removes any pending obligation of the key.
    fn mark_published(&self, key: String) -> StoreFuture<'_, ()>;

    // ---- held relay events (NIP-77) ----

    /// Remembers relay events held in `scope`; the first record per
    /// `(scope, id)` wins.
    fn record_held(&self, scope: String, events: Vec<HeldEvent>) -> StoreFuture<'_, ()>;
    /// The held events of `scope` matching `query`, sorted by `(created_at, id)`.
    fn held_events(&self, scope: String, query: HeldQuery) -> StoreFuture<'_, Vec<HeldItem>>;

    // ---- replay memo ----

    /// Every stored object's current replay input.
    fn replay_inputs(&self) -> StoreFuture<'_, HashMap<String, ReplayInput>>;
    /// The memo rows of `object_ids` written under `engine_version`.
    fn cached_replays(
        &self,
        object_ids: Vec<String>,
        engine_version: String,
    ) -> StoreFuture<'_, HashMap<String, CachedReplay>>;
    /// Upserts `rows` in one transaction.
    fn put_cached_replays(
        &self,
        rows: Vec<CachedReplay>,
        engine_version: String,
    ) -> StoreFuture<'_, ()>;
    fn clear_cached_replays(&self) -> StoreFuture<'_, ()>;

    // ---- lifecycle ----

    /// Empties every table (logout keeping the database file).
    fn clear(&self) -> StoreFuture<'_, ()>;
    /// Closes the connection; every later call fails with [`StoreError::Closed`].
    fn close(&self) -> StoreFuture<'_, ()>;
}

/// Replay inputs from per-object id lists, checkpoint hashes and base hashes.
pub(crate) fn replay_inputs_from(
    ids: BTreeMap<String, Vec<String>>,
    checkpoints: HashMap<String, String>,
    mut bases: HashMap<String, Vec<String>>,
) -> HashMap<String, ReplayInput> {
    let mut inputs = HashMap::new();
    let mut objects: HashSet<String> = ids.keys().cloned().collect();
    objects.extend(checkpoints.keys().cloned());
    objects.extend(bases.keys().cloned());
    for object in objects {
        let changes = ids.get(&object).map(Vec::as_slice).unwrap_or_default();
        let checkpoint = checkpoints.get(&object).map(String::as_str).unwrap_or("");
        let held = bases.remove(&object).unwrap_or_default();
        let input = ReplayInput::new(
            changes.iter().map(String::as_str),
            checkpoint,
            held.iter().map(String::as_str),
        );
        inputs.insert(object, input);
    }
    inputs
}

pub(crate) fn encode_groups(groups: &[(String, i64)]) -> String {
    serde_json::to_string(groups).expect("string/int pairs serialize")
}

pub(crate) fn decode_groups(raw: &str) -> Result<Vec<(String, i64)>, StoreError> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| StoreError::Corrupt(format!("replay-groups: {e}")))?;
    let pairs = value
        .as_array()
        .ok_or_else(|| StoreError::Corrupt("replay-groups is not an array".into()))?;
    pairs
        .iter()
        .map(|pair| match pair.as_array().map(Vec::as_slice) {
            Some([serde_json::Value::String(key), at]) => at
                .as_i64()
                .or_else(|| at.as_f64().map(|f| f as i64))
                .map(|at| (key.clone(), at))
                .ok_or_else(|| {
                    StoreError::Corrupt("replay-groups entry has non-numeric at".into())
                }),
            _ => Err(StoreError::Corrupt(
                "replay-groups entry is not [key, at]".into(),
            )),
        })
        .collect()
}

pub(crate) fn decode_pending(raw: &str) -> Result<PendingPublish, StoreError> {
    serde_json::from_str(raw).map_err(|e| StoreError::Corrupt(format!("pending: {e}")))
}

pub(crate) fn encode_pending(item: &PendingPublish) -> Result<String, StoreError> {
    serde_json::to_string(item).map_err(|e| StoreError::Corrupt(format!("pending: {e}")))
}

/// The change id of a pending key (`changeId` or `spaceId/keyId/changeId`).
pub(crate) fn pending_change_id(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

/// Whether a `checkpoint-floors` row shortened a walk (any floor > 0).
pub(crate) fn floors_shortened_walk(raw: &str) -> bool {
    serde_json::from_str::<HashMap<String, f64>>(raw)
        .map(|floors| floors.values().any(|v| *v > 0.0))
        .unwrap_or(false)
}

pub(crate) fn decode_streams(raw: Option<&str>) -> Result<HashMap<String, i64>, StoreError> {
    match raw {
        None => Ok(HashMap::new()),
        Some(raw) => {
            serde_json::from_str(raw).map_err(|e| StoreError::Corrupt(format!("base-streams: {e}")))
        }
    }
}

/// Moves each stream's newest-base floor to `created_at`; returns whether any moved.
pub(crate) fn advance_streams(
    by_stream: &mut HashMap<String, i64>,
    streams: &[String],
    created_at: i64,
) -> bool {
    let mut moved = false;
    for scope in streams {
        let at = by_stream.entry(scope.clone()).or_insert(i64::MIN);
        if *at < created_at {
            *at = created_at;
            moved = true;
        }
    }
    by_stream.retain(|_, at| *at != i64::MIN);
    moved
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    out
}

mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_matches_ios_definition() {
        // sha256 over each id + "\n" (iOS ReplayInput.digest), checked with `shasum -a 256`.
        assert_eq!(
            ReplayInput::digest(["a", "b"]),
            "911169ddaaf146aff539f58c26c489af3b892dff0fe283c1c264c65ae5aa59a2"
        );
        assert_eq!(
            ReplayInput::digest([]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let input = ReplayInput::new(["a", "b"], "cp", ["z", "y"]);
        assert_eq!((input.change_count, input.base_hashes.as_str()), (2, "y,z"));
    }

    #[test]
    fn pending_json_matches_ios_codable() {
        let item = PendingPublish {
            key: "s1/3/c2".into(),
            object_id: "o2".into(),
            change_id: "c2".into(),
            bytes: vec![9, 8, 7],
            space_id: Some("s1".into()),
            key_id: Some(3),
            base: None,
            events: None,
        };
        let json = encode_pending(&item).unwrap();
        assert_eq!(
            json,
            r#"{"key":"s1/3/c2","objectId":"o2","changeId":"c2","bytes":"CQgH","spaceId":"s1","keyId":3}"#
        );
        assert_eq!(decode_pending(&json).unwrap(), item);
        assert_eq!(pending_change_id("s1/3/c2"), "c2");
        assert_eq!(pending_change_id("c1"), "c1");
    }

    #[test]
    fn replay_groups_accept_float_timestamps() {
        assert_eq!(
            decode_groups(r#"[["g",1.0],["h",2]]"#).unwrap(),
            vec![("g".into(), 1), ("h".into(), 2)]
        );
        assert!(decode_groups(r#"[["g"]]"#).is_err());
        assert!(decode_groups(r#"{}"#).is_err());
    }
}

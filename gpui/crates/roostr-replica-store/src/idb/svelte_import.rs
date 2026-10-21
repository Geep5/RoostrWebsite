//! One-time import of the Svelte site's browser vault.
//!
//! Before this app, roostr.space's `/app` kept its replica in IndexedDB
//! `roostr` (`engine/store.ts`, `DB_VERSION` 5) on the same origin and under
//! the same `localStorage` `roostr-key`. [`import_svelte_vault`] copies it
//! into this store once, before the replica's first sync, so a returning
//! visitor keeps every change it held and - above all - publishes what it
//! never got to publish: the `pending:<key>` obligations, which the outbox
//! offers as soon as sync starts.
//!
//! Carried, row for row:
//! - `changes` (key: change id → `{objectId, bytes, json, rebased?}`): the
//!   bytes verbatim, `json` (the codec's ChangeJSON object) as JSON text, `b`
//!   from it, `rebased` when set. A `json` that does not parse as the
//!   engine's [`Change`] is re-decoded from the bytes instead. Inserted in
//!   `(timestamp, id)` order (store.ts kept no insertion order).
//! - `checkpoints` (`{objectId, bytes, hash, heads}`): `covered`, which
//!   store.ts did not keep, is counted from the protobuf's `covered_ids`; a
//!   held checkpoint is replaced only by the core supersede rule.
//! - `bases`, each object's chain pruned and its current base re-pointed as
//!   `put_base` does. `base-current` itself is not read: it held the
//!   state-sync current base (epoch, then createdAt, then hash -
//!   docs/state-sync.md), which is exactly what re-pointing derives.
//! - `relay-events` → held events (the NIP-77 item sets), per scope.
//! - `meta`: `published:*`; `pending:*` (never over an obligation or a
//!   publication this store already has); `base-streams` (merged, newest
//!   wins); and, only into a store that has not begun its own history,
//!   `cursor`, `replay-groups`, `bootstrap-floor`, `checkpoint-floors` and
//!   last `bootstrapped` - the same meaning in both stores (`sync/` ports
//!   sync.ts), so an imported vault resumes its relay sync instead of
//!   walking all history again.
//! - Not carried: `states`, the replay memo. Its rows are keyed by another
//!   input (change count + checkpoint/base hash → ObjectJSON) than this
//!   store's (`ReplayInput` → engine-versioned state bytes), and the replica
//!   replays every object on its first start anyway.
//!
//! Ownership: store.ts kept no owner. The Svelte app scoped the database to
//! `roostr-key` by destroying it on every identity change (logout and key
//! import both ran `destroyDatabase` before touching the key), so it belongs
//! to the stored key until this app changes that key. To keep that true
//! afterwards, the import first claims the database (Svelte meta
//! `gpui-import` = `{owner}`, the pubkey) and marks it imported when done
//! (`{owner, importedAt}`): a database claimed by another key, or already
//! imported (by anyone - also by this key before a logout erased this
//! store), is never imported again. Before claiming, the vault must look
//! like this key's: at least one change authored by its author id (every
//! vault holds the owner's own writes - its Personal space at least), and
//! every signed personal event kept in an obligation signed by its pubkey.
//! A vault failing either is someone else's and is left alone.
//!
//! The Svelte database is left in place, apart from that stamp: it is the
//! user's data, and the stamp plus this store's [`IMPORT_FLAG`] already
//! make the import happen once. Checking for it never creates it:
//! `indexedDB.databases()` where available, else an open that aborts its
//! upgrade (which discards a database that did not exist).
//!
//! Failure: any error returns before [`IMPORT_FLAG`] is set, so the next
//! start retries rather than losing unpublished changes to a transient
//! error. Every write is idempotent (changes by id, held events by scope and
//! id, obligations only where none is held), so a retry after a partial
//! import is safe. The flag is set on every settled outcome (no database,
//! empty, someone else's, imported), after which a start costs one meta read.

use std::cell::Cell;

use roostr_engine::{Change, Engine};
use wasm_bindgen_futures::JsFuture;

use super::*;

/// The Svelte site's database (store.ts `DB_NAME`).
pub const SVELTE_DB: &str = "roostr";
/// Meta row of this store: set once the Svelte vault is settled; its value
/// names the outcome (`absent`, `empty`, `stamped`, `foreign`, `imported`).
pub const IMPORT_FLAG: &str = "svelte-import";
/// Meta row written into the Svelte database: `{owner, importedAt?}`.
pub const STAMP: &str = "gpui-import";
/// Rows per write transaction, well inside the [`IDB_TIMEOUT_MS`] guard.
const BATCH: usize = 1000;

/// What [`import_svelte_vault`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SvelteImport {
    /// An earlier start settled it: the [`IMPORT_FLAG`] value.
    Settled(String),
    /// No Svelte database on this origin.
    Absent,
    /// The database holds no change and no obligation.
    Empty,
    /// The database was already imported (by any key, possibly this one
    /// before a logout).
    Stamped,
    /// The database is another key's vault.
    Foreign,
    Imported(ImportStats),
}

impl SvelteImport {
    fn flag(&self) -> Option<&'static str> {
        match self {
            SvelteImport::Settled(_) => None,
            SvelteImport::Absent => Some("absent"),
            SvelteImport::Empty => Some("empty"),
            SvelteImport::Stamped => Some("stamped"),
            SvelteImport::Foreign => Some("foreign"),
            SvelteImport::Imported(_) => Some("imported"),
        }
    }
}

/// Rows an import wrote (new to this store) or skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportStats {
    pub changes: usize,
    pub checkpoints: usize,
    pub bases: usize,
    pub held: usize,
    pub published: usize,
    pub pending: usize,
    /// Rows that did not decode (left behind).
    pub skipped: usize,
    /// Whether the relay cursor and bootstrap state were carried.
    pub resumed: bool,
}

/// Imports the Svelte vault in database `source` into `target` once for
/// the identity `pubkey` (author id `author_id`). See the module docs.
pub async fn import_svelte_vault(
    target: &IdbChangeStore,
    source: &str,
    engine: &Engine,
    pubkey: &str,
    author_id: &str,
) -> Result<SvelteImport, StoreError> {
    if let Some(settled) = target.meta(IMPORT_FLAG.into()).await? {
        return Ok(SvelteImport::Settled(settled));
    }
    let outcome = match Svelte::open(source).await? {
        None => SvelteImport::Absent,
        Some(svelte) => {
            let outcome = import_from(&svelte, target, engine, pubkey, author_id).await;
            svelte.close();
            outcome?
        }
    };
    if let Some(flag) = outcome.flag() {
        target
            .set_meta(IMPORT_FLAG.into(), Some(flag.into()))
            .await?;
    }
    Ok(outcome)
}

async fn import_from(
    svelte: &Svelte,
    target: &IdbChangeStore,
    engine: &Engine,
    pubkey: &str,
    author_id: &str,
) -> Result<SvelteImport, StoreError> {
    let mut stats = ImportStats::default();
    let meta = svelte.meta(&mut stats).await?;
    match &meta.stamp {
        Some(stamp) if stamp.imported => return Ok(SvelteImport::Stamped),
        Some(stamp) if stamp.owner != pubkey => return Ok(SvelteImport::Foreign),
        _ => {}
    }
    let mut changes = svelte.changes(engine, &mut stats).await?;
    if changes.is_empty() && meta.pending.is_empty() {
        return Ok(SvelteImport::Empty);
    }
    let signed_here = meta
        .pending
        .iter()
        .filter(|item| item.space_id.is_none())
        .flat_map(|item| item.events.iter().flatten())
        .all(|event| event["pubkey"].as_str() == Some(pubkey));
    if !signed_here || !changes.iter().any(|change| change.author == author_id) {
        return Ok(SvelteImport::Foreign);
    }
    if meta.stamp.is_none() {
        svelte.stamp(pubkey, false).await?;
    }

    changes.sort_by(|a, b| (a.timestamp, &a.record.id).cmp(&(b.timestamp, &b.record.id)));
    let records: Vec<ChangeRecord> = changes.into_iter().map(|c| c.record).collect();
    for chunk in records.chunks(BATCH) {
        stats.changes += write_changes(target, chunk.to_vec()).await?;
    }
    let checkpoints = svelte.checkpoints(&mut stats).await?;
    for chunk in checkpoints.chunks(BATCH) {
        stats.checkpoints += write_checkpoints(target, engine, chunk.to_vec()).await?;
    }
    let mut by_object: BTreeMap<String, Vec<BaseRecord>> = BTreeMap::new();
    for base in svelte.bases(&mut stats).await? {
        by_object
            .entry(base.object_id.clone())
            .or_default()
            .push(base);
    }
    let by_object: Vec<(String, Vec<BaseRecord>)> = by_object.into_iter().collect();
    for chunk in by_object.chunks(BATCH) {
        stats.bases += write_bases(target, chunk.to_vec()).await?;
    }
    for (scope, events) in svelte.held(&mut stats).await? {
        for chunk in events.chunks(BATCH) {
            stats.held += chunk.len();
            target.record_held(scope.clone(), chunk.to_vec()).await?;
        }
    }
    write_meta(target, meta, &mut stats).await?;
    svelte.stamp(pubkey, true).await?;
    Ok(SvelteImport::Imported(stats))
}

// ------------------------------------------------------------- source

/// One Svelte change row, decoded.
struct SvelteChange {
    record: ChangeRecord,
    author: String,
    timestamp: i64,
}

/// `{owner, importedAt?}` in the Svelte meta store.
struct Stamp {
    owner: String,
    imported: bool,
}

#[derive(Default)]
struct SvelteMeta {
    published: Vec<String>,
    pending: Vec<PendingPublish>,
    cursor: Option<i64>,
    replay_groups: Option<Vec<(String, i64)>>,
    bootstrapped: bool,
    bootstrap_floor: Option<i64>,
    checkpoint_floors: Option<String>,
    base_streams: HashMap<String, i64>,
    stamp: Option<Stamp>,
}

/// An open connection to the Svelte database.
struct Svelte {
    db: IdbDatabase,
    _on_version_change: Handler,
}

impl Svelte {
    /// Opens database `name` when it exists; never creates it.
    async fn open(name: &str) -> Result<Option<Self>, StoreError> {
        let name = name.to_owned();
        guarded("open", async move {
            if !listed(&name).await {
                return Ok(None);
            }
            let request = factory()?.open(&name).map_err(unavailable)?;
            let created = Rc::new(Cell::new(false));
            let upgrade = {
                let (request, created) = (request.clone(), created.clone());
                Handler::new(move |_: Event| {
                    // Opened without a version, an upgrade means the database
                    // did not exist: aborting it discards the empty one.
                    created.set(true);
                    if let Some(tx) = request.transaction() {
                        let _ = tx.abort();
                    }
                })
            };
            request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
            let opened = Request::new(request.clone().unchecked_into()).next().await;
            request.set_onupgradeneeded(None);
            if created.get() {
                return Ok(None);
            }
            let db: IdbDatabase = opened
                .map_err(|e| StoreError::Unavailable(e.to_string()))?
                .unchecked_into();
            // Never hold up another connection's upgrade or delete.
            let on_version_change = {
                let db = db.clone();
                Handler::new(move |_: Event| db.close())
            };
            db.set_onversionchange(Some(on_version_change.as_ref().unchecked_ref()));
            Ok(Some(Self {
                db,
                _on_version_change: on_version_change,
            }))
        })
        .await
    }

    fn close(&self) {
        self.db.set_onversionchange(None);
        self.db.close();
    }

    /// Every key and value of object store `name` (none when it is missing:
    /// a database from before store.ts version 5), read in chunks.
    async fn rows(&self, name: &'static str) -> Result<(Vec<JsValue>, Vec<JsValue>), StoreError> {
        let db = self.db.clone();
        guarded("read", async move {
            let (mut keys, mut values) = (Vec::new(), Vec::new());
            if !db.object_store_names().contains(name) {
                return Ok((keys, values));
            }
            let tx = db.transaction_with_str(name).map_err(js_error)?;
            each_row(Rows::Store(&os(&tx, name)?), |key, value| {
                keys.push(key);
                values.push(value);
                Ok(())
            })
            .await?;
            Ok((keys, values))
        })
        .await
    }

    async fn meta(&self, stats: &mut ImportStats) -> Result<SvelteMeta, StoreError> {
        let (names, values) = self.rows(META).await?;
        let mut meta = SvelteMeta::default();
        for (key, value) in names.iter().zip(&values) {
            let Some(key) = key.as_string() else {
                continue;
            };
            match key.as_str() {
                keys::CURSOR => meta.cursor = value.as_f64().map(|v| v as i64),
                keys::REPLAY_GROUPS => {
                    meta.replay_groups = json_text(value).and_then(|t| decode_groups(&t).ok())
                }
                keys::BOOTSTRAPPED => meta.bootstrapped = value.as_bool() == Some(true),
                keys::BOOTSTRAP_FLOOR => meta.bootstrap_floor = value.as_f64().map(|v| v as i64),
                keys::CHECKPOINT_FLOORS => meta.checkpoint_floors = json_text(value),
                keys::BASE_STREAMS => {
                    meta.base_streams = json_text(value)
                        .and_then(|t| serde_json::from_str::<HashMap<String, f64>>(&t).ok())
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(scope, at)| (scope, at as i64))
                        .collect()
                }
                STAMP => {
                    meta.stamp = Some(Stamp {
                        owner: opt_text(value, "owner").unwrap_or_default(),
                        imported: !field(value, "importedAt").is_undefined(),
                    })
                }
                _ => {
                    if let Some(published) = key.strip_prefix(keys::PUBLISHED) {
                        meta.published.push(published.to_owned());
                    } else if key.starts_with(keys::PENDING) {
                        match pending_from(value) {
                            Ok(item) => meta.pending.push(item),
                            Err(_) => stats.skipped += 1,
                        }
                    }
                }
            }
        }
        Ok(meta)
    }

    async fn changes(
        &self,
        engine: &Engine,
        stats: &mut ImportStats,
    ) -> Result<Vec<SvelteChange>, StoreError> {
        let (ids, values) = self.rows(CHANGES).await?;
        let mut out = Vec::with_capacity(values.len());
        for (key, row) in ids.iter().zip(&values) {
            let (Some(id), Ok(object_id), Ok(bytes)) =
                (key.as_string(), text(row, "objectId"), bytes(row, "bytes"))
            else {
                stats.skipped += 1;
                continue;
            };
            let json = field(row, "json");
            let mut record = ChangeRecord {
                id,
                object_id,
                bytes,
                json: json_text(&json).unwrap_or_default(),
                b: opt_text(&json, "b"),
                rebased: field(row, "rebased").is_truthy(),
            };
            let change = match serde_json::from_str::<Change>(&record.json) {
                Ok(change) => change,
                Err(_) => match engine.decode_change(&record.bytes).await {
                    Ok(mut change) => {
                        change.b = record.b.clone();
                        record.json = serde_json::to_string(&change)
                            .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                        change
                    }
                    Err(_) => {
                        stats.skipped += 1;
                        continue;
                    }
                },
            };
            out.push(SvelteChange {
                record,
                author: change.author,
                timestamp: change.timestamp,
            });
        }
        Ok(out)
    }

    async fn checkpoints(
        &self,
        stats: &mut ImportStats,
    ) -> Result<Vec<CheckpointRecord>, StoreError> {
        let (_, values) = self.rows(CHECKPOINTS).await?;
        let mut out = Vec::with_capacity(values.len());
        for row in &values {
            let record = (|| -> Result<CheckpointRecord, StoreError> {
                let bytes = bytes(row, "bytes")?;
                Ok(CheckpointRecord {
                    object_id: text(row, "objectId")?,
                    hash: text(row, "hash")?,
                    heads: array(field(row, "heads"))
                        .iter()
                        .filter_map(JsValue::as_string)
                        .collect(),
                    covered: covered_ids(&bytes),
                    bytes,
                })
            })();
            match record {
                Ok(record) => out.push(record),
                Err(_) => stats.skipped += 1,
            }
        }
        Ok(out)
    }

    async fn bases(&self, stats: &mut ImportStats) -> Result<Vec<BaseRecord>, StoreError> {
        let (_, values) = self.rows(BASES).await?;
        let mut out = Vec::with_capacity(values.len());
        for row in &values {
            match base_from(row) {
                Ok(base) => out.push(base),
                Err(_) => stats.skipped += 1,
            }
        }
        Ok(out)
    }

    async fn held(
        &self,
        stats: &mut ImportStats,
    ) -> Result<BTreeMap<String, Vec<HeldEvent>>, StoreError> {
        let (_, values) = self.rows(RELAY_EVENTS).await?;
        let mut out: BTreeMap<String, Vec<HeldEvent>> = BTreeMap::new();
        for row in &values {
            let event = (|| -> Result<(String, HeldEvent), StoreError> {
                Ok((
                    text(row, "scope")?,
                    HeldEvent {
                        id: text(row, "id")?,
                        created_at: number(row, "createdAt")?,
                        kind: number(row, "kind")? as u32,
                        b: opt_text(row, "b"),
                    },
                ))
            })();
            match event {
                Ok((scope, event)) => out.entry(scope).or_default().push(event),
                Err(_) => stats.skipped += 1,
            }
        }
        Ok(out)
    }

    /// Claims the database for `owner`; `imported` marks the import done.
    async fn stamp(&self, owner: &str, imported: bool) -> Result<(), StoreError> {
        let db = self.db.clone();
        let value = object(&[
            ("owner", owner.into()),
            (
                "importedAt",
                if imported {
                    js_sys::Date::now().into()
                } else {
                    JsValue::UNDEFINED
                },
            ),
        ]);
        guarded("write", async move {
            let tx = db
                .transaction_with_str_and_mode(META, IdbTransactionMode::Readwrite)
                .map_err(js_error)?;
            os(&tx, META)?
                .put_with_key(&value, &STAMP.into())
                .map_err(js_error)?;
            done(&tx).await
        })
        .await
    }
}

/// Whether `indexedDB.databases()` lists `name`; true when it cannot tell
/// (no such method, or it failed): the open then aborts any creation.
async fn listed(name: &str) -> bool {
    let Ok(factory) = factory() else {
        return true;
    };
    let Some(databases) = Reflect::get(&factory, &"databases".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
    else {
        return true;
    };
    let Some(promise) = databases
        .call0(&factory)
        .ok()
        .and_then(|p| p.dyn_into::<js_sys::Promise>().ok())
    else {
        return true;
    };
    match JsFuture::from(promise).await {
        Ok(list) => array(list)
            .iter()
            .any(|db| opt_text(db, "name").as_deref() == Some(name)),
        Err(_) => true,
    }
}

fn json_text(value: &JsValue) -> Option<String> {
    if value.is_undefined() {
        return None;
    }
    js_sys::JSON::stringify(value).ok().map(String::from)
}

/// A store.ts `PendingPublish` (`bytes` a Uint8Array, `events` signed
/// nostr-tools events).
fn pending_from(value: &JsValue) -> Result<PendingPublish, StoreError> {
    let events = field(value, "events");
    let events = if events.is_undefined() || events.is_null() {
        None
    } else {
        let text = json_text(&events).ok_or_else(|| StoreError::Corrupt("events".into()))?;
        Some(serde_json::from_str(&text).map_err(|e| StoreError::Corrupt(format!("events: {e}")))?)
    };
    Ok(PendingPublish {
        key: text(value, "key")?,
        object_id: text(value, "objectId")?,
        change_id: text(value, "changeId")?,
        bytes: bytes(value, "bytes")?,
        space_id: opt_text(value, "spaceId"),
        key_id: field(value, "keyId").as_f64().map(|v| v as i64),
        base: opt_text(value, "base"),
        events,
    })
}

/// Occurrences of `covered_ids` (field 3, length-delimited) in a Checkpoint
/// protobuf (docs/checkpoint-sync.md); stops at the first malformed field.
fn covered_ids(bytes: &[u8]) -> u64 {
    fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *bytes.get(*at)?;
            *at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }
    let (mut at, mut covered) = (0usize, 0u64);
    while at < bytes.len() {
        let Some(tag) = varint(bytes, &mut at) else {
            break;
        };
        let skip = match tag & 7 {
            0 => varint(bytes, &mut at).map(|_| 0),
            1 => Some(8),
            2 => varint(bytes, &mut at).inspect(|_| covered += u64::from(tag >> 3 == 3)),
            5 => Some(4),
            _ => None,
        };
        let Some(skip) = skip.and_then(|n| usize::try_from(n).ok()) else {
            break;
        };
        at = at.saturating_add(skip);
    }
    covered
}

// ------------------------------------------------------------- target

async fn write_changes(
    target: &IdbChangeStore,
    records: Vec<ChangeRecord>,
) -> Result<usize, StoreError> {
    let target = target.clone();
    guarded("write", async move {
        let tx = target.write(&[CHANGES])?;
        let store = os(&tx, CHANGES)?;
        let inserted = IdbChangeStore::insert_changes(&store, &records).await?;
        // Also for an orphan this store already held unmarked.
        for record in records.iter().filter(|record| record.rebased) {
            mark(&store, &record.id).await?;
        }
        done(&tx).await?;
        Ok(inserted)
    })
    .await
}

async fn write_checkpoints(
    target: &IdbChangeStore,
    engine: &Engine,
    records: Vec<CheckpointRecord>,
) -> Result<usize, StoreError> {
    let (target, engine) = (target.clone(), engine.clone());
    guarded("write", async move {
        let tx = target.write(&[CHECKPOINTS])?;
        let store = os(&tx, CHECKPOINTS)?;
        let mut stored = 0;
        for record in records {
            let key: JsValue = record.object_id.as_str().into();
            let held = req(store.get(&key)).await?;
            // As in put_checkpoint: the engine call does not yield.
            if !held.is_undefined() && !record.supersedes(&checkpoint_from(&held)?, &engine).await?
            {
                continue;
            }
            store
                .put_with_key(&checkpoint_value(&record), &key)
                .map_err(js_error)?;
            stored += 1;
        }
        done(&tx).await?;
        Ok(stored)
    })
    .await
}

/// `put_base` per object, without stream floors (carried from meta).
async fn write_bases(
    target: &IdbChangeStore,
    objects: Vec<(String, Vec<BaseRecord>)>,
) -> Result<usize, StoreError> {
    let target = target.clone();
    guarded("write", async move {
        let tx = target.write(&[BASES, BASE_CURRENT])?;
        let (bases, current) = (os(&tx, BASES)?, os(&tx, BASE_CURRENT)?);
        let mut stored = 0;
        for (object_id, records) in objects {
            for record in &records {
                let key = key2(record.object_id.as_str(), record.hash.as_str());
                if req(bases.get_key(&key)).await?.is_undefined() {
                    bases.put(&base_value(record)).map_err(js_error)?;
                    stored += 1;
                }
            }
            let mut held = IdbChangeStore::bases_in(&bases, &object_id).await?;
            let Some(newest) = held.iter().map(|base| base.epoch).max() else {
                continue;
            };
            for base in held
                .iter()
                .filter(|base| base.epoch <= newest - BASE_CHAIN_KEPT)
            {
                bases
                    .delete(&key2(base.object_id.as_str(), base.hash.as_str()))
                    .map_err(js_error)?;
            }
            held.retain(|base| base.epoch > newest - BASE_CHAIN_KEPT);
            if let Some(best) = BaseRecord::current(&held) {
                let value = object(&[
                    ("hash", best.hash.as_str().into()),
                    ("createdAt", (best.created_at as f64).into()),
                ]);
                current
                    .put_with_key(&value, &object_id.as_str().into())
                    .map_err(js_error)?;
            }
        }
        done(&tx).await?;
        Ok(stored)
    })
    .await
}

async fn write_meta(
    target: &IdbChangeStore,
    meta: SvelteMeta,
    stats: &mut ImportStats,
) -> Result<(), StoreError> {
    let target = target.clone();
    let (published, pending, resumed) = guarded("write", async move {
        let tx = target.write(&[META])?;
        let store = os(&tx, META)?;
        let get = |key: String| req(store.get(&key.into()));
        let put = |key: &str, value: &str| {
            store
                .put_with_key(&value.into(), &key.into())
                .map_err(js_error)
        };
        let mut published = 0;
        for key in &meta.published {
            let at = format!("{}{key}", keys::PUBLISHED);
            if get(at.clone()).await?.is_undefined() {
                put(&at, "1")?;
                store
                    .delete(&format!("{}{key}", keys::PENDING).into())
                    .map_err(js_error)?;
                published += 1;
            }
        }
        let mut pending = 0;
        for item in &meta.pending {
            let at = format!("{}{}", keys::PENDING, item.key);
            if get(format!("{}{}", keys::PUBLISHED, item.key))
                .await?
                .is_undefined()
                && get(at.clone()).await?.is_undefined()
            {
                put(&at, &encode_pending(item)?)?;
                pending += 1;
            }
        }
        if !meta.base_streams.is_empty() {
            let raw = get(keys::BASE_STREAMS.into()).await?.as_string();
            let mut streams = decode_streams(raw.as_deref())?;
            for (scope, at) in &meta.base_streams {
                let held = streams.entry(scope.clone()).or_insert(*at);
                *held = (*held).max(*at);
            }
            let encoded =
                serde_json::to_string(&streams).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            put(keys::BASE_STREAMS, &encoded)?;
        }
        // Only a store with no history of its own takes over the Svelte
        // walk's progress; `bootstrapped` goes last.
        let fresh = get(keys::CURSOR.into()).await?.is_undefined()
            && get(keys::BOOTSTRAPPED.into()).await?.is_undefined()
            && get(keys::BOOTSTRAP_FLOOR.into()).await?.is_undefined();
        if fresh {
            if let Some(cursor) = meta.cursor {
                put(keys::CURSOR, &cursor.to_string())?;
            }
            if let Some(groups) = &meta.replay_groups {
                put(keys::REPLAY_GROUPS, &encode_groups(groups))?;
            }
            if let Some(floor) = meta.bootstrap_floor {
                put(keys::BOOTSTRAP_FLOOR, &floor.to_string())?;
            }
            if let Some(floors) = &meta.checkpoint_floors {
                put(keys::CHECKPOINT_FLOORS, floors)?;
            }
            if meta.bootstrapped {
                put(keys::BOOTSTRAPPED, "1")?;
            }
        }
        done(&tx).await?;
        Ok((published, pending, fresh))
    })
    .await?;
    stats.published = published;
    stats.pending = pending;
    stats.resumed = resumed;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn counts_covered_ids_and_skips_other_fields() {
        // object_id (1, len 2), epoch (2, varint 300), two covered_ids (3), a fixed64 (9).
        let bytes = [
            0x0a, 2, b'o', b'1', 0x10, 0xac, 0x02, 0x1a, 1, 0xaa, 0x1a, 2, 0xbb, 0xcc, 0x49, 0, 0,
            0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(covered_ids(&bytes), 2);
        assert_eq!(covered_ids(&[]), 0);
        // Truncated length: counted, then stops.
        assert_eq!(covered_ids(&[0x1a, 5, 1]), 1);
    }
}

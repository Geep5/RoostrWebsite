//! Record mode (glonOdin `docs/state-objects.md`): one stamped record per
//! object instead of a change DAG, synced as patches over RoostrRelay's
//! stream cursor and loaded from packs on Blossom. Off unless the replica
//! is opened with it ([`RECORDS_FLAG`], [`crate::ReplicaBackend::with_records`]).
//!
//! Wire (shared with the record-mode daemon, `glonOdin/src/records_sync.odin`):
//! - **Stream**: a shared space is the relay stream of its `s` tag
//!   `blind(key hex, "space:"+id)` under the space key; the personal stream
//!   is the author's own events (no `s`) under the self conversation key,
//!   blinded with the secret key's bytes.
//! - **Patch** (kind 1081): NIP-44 of base64(canonical Patch), split into
//!   [`CHUNK_CHARS`] parts tagged `["c", gid, i, n]`; tags `h` =
//!   blind(object id), `s` (spaces), `["v", "1"]`.
//! - **Vanish** (kind 1082): NIP-44 of `{"object_id"}`, same tags: drop the
//!   record; the id stays in a local ledger for [`TOMBSTONE_DAYS`].
//! - **Pack** (kind 31081, `d` = the stream's `s`, or
//!   `blind(secret, "stream:personal")`): NIP-44 JSON [`PackAnnouncement`];
//!   the blob is `pack.rs`'s framed encryption under
//!   `pack_key(stream key, cursor)`.
//!
//! Loading a stream: its newest pack when this device has no cursor, its
//! cursor is below the pack's `deleted_through` or behind the relay's
//! window, then `REQ after` = cursor until the head,
//! then live with seq. Cursors are of the first relay (seqs are per relay);
//! patches go to every relay. Local edits become patches through the
//! engine's `record` `patch` action (the daemon's commit), kept in the
//! journal until a relay accepts them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt as _;
use futures::lock::Mutex as AsyncMutex;
use parking_lot::Mutex;
use roostr_core::{blossom, http};
use roostr_engine::Change;
use roostr_engine::Engine;
use roostr_engine::pack::{
    FRAME, PACK_BUCKETS, PackHeader, TAG, frame_range, frames_decrypt, pack_bucket_bytes, pack_decrypt,
    pack_key, pack_read_bucket, pack_read_header, pack_root,
};
use roostr_engine::record::{
    Hlc, Patch, Record, Stamp, bucket_hashes, hlc_now, hlc_observe, patch_canonical, patch_decode,
    record_apply_patch, record_canonical, record_decode, record_hash, record_merge, record_view,
    root_hash,
};
use roostr_nostr::relay::SubscriptionItem;
use roostr_nostr::{Event, EventTemplate, Filter, Keys, Relay, nip44};
use roostr_replica_store::{JournalItem, RecordBatch, RecordStore};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use crate::{MaybeSend, MaybeSync, rt};

/// Kind of a patch event.
pub const PATCH_KIND: u32 = 1081;
/// Kind of a vanish event.
pub const VANISH_KIND: u32 = 1082;
/// Kind of a pack announcement (addressable).
pub const PACK_KIND: u32 = 31081;
/// Base64 characters per patch part (`core/wire.odin` `WIRE_CHUNK_CHARS`).
pub const CHUNK_CHARS: usize = 40_000;
/// Parts per patch at most (`WIRE_MAX_CHUNKS`).
pub const MAX_CHUNKS: usize = 64;
/// How long a vanished id refuses patches.
pub const TOMBSTONE_DAYS: u64 = 30;
/// Local key turning record mode on (`"1"`): the browser's `localStorage`,
/// the desktop's `local.json`; `ROOSTR_RECORDS=1` natively too.
pub const RECORDS_FLAG: &str = "roostr-records";
/// Events per `REQ after` page (RoostrRelay's default).
const PAGE: u64 = 500;
const QUERY_TIMEOUT: Duration = Duration::from_secs(20);
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(15);
const BACKOFF_FLOOR: Duration = Duration::from_secs(1);
const BACKOFF_CEILING: Duration = Duration::from_secs(60);
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Largest pack blob a device downloads whole.
const MAX_PACK_BYTES: u64 = 1 << 30;

const META_HLC: &str = "hlc";
const META_CURSOR: &str = "cursor:";
const META_VANISHED: &str = "vanished:";

fn now_ms() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `core/wire.odin` `wire_blind`: the first 16 hex of sha256(prefix ‖ id).
pub fn blind(prefix: &[u8], id: &str) -> String {
    let digest = Sha256::new()
        .chain_update(prefix)
        .chain_update(id.as_bytes())
        .finalize();
    hex::encode(&digest[..8])
}

/// This signer's stamp device index: the first four bytes (big-endian) of
/// sha256(pubkey hex), never 0 (0 is a conversion seed).
pub fn device_index(pubkey_hex: &str) -> u32 {
    let digest = Sha256::digest(pubkey_hex.as_bytes());
    u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) | 1
}

// ── Streams ─────────────────────────────────────────────────────────

/// One relay stream and its keys.
#[derive(Clone)]
pub struct StreamKey {
    /// The shared space; `None` = the personal stream.
    pub space_id: Option<String>,
    /// NIP-44 conversation key and pack stream key.
    conversation: [u8; 32],
    /// Blinding prefix of `h` tags.
    prefix: Vec<u8>,
    /// The space stream tag; `None` for the personal stream.
    pub s: Option<String>,
    /// The space's owner (`["owner", pk]`: consent to the owner's
    /// deletion of the space's events).
    owner: Option<String>,
    /// The pack announcement's `d`.
    pub d: String,
    /// The pack header's `stream` and the relay's stream name: the `s` tag,
    /// or the author's pubkey.
    pub stream: String,
}

impl std::fmt::Debug for StreamKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamKey")
            .field("space_id", &self.space_id)
            .field("stream", &self.stream)
            .finish_non_exhaustive()
    }
}

impl StreamKey {
    pub fn personal(keys: &Keys) -> Result<Self> {
        let conversation = nip44::conversation_key(keys, keys.pubkey()).map_err(|e| anyhow!("{e}"))?;
        let prefix = keys.secret_bytes().to_vec();
        Ok(Self {
            space_id: None,
            conversation,
            d: blind(&prefix, "stream:personal"),
            prefix,
            s: None,
            owner: None,
            stream: keys.pubkey().to_string(),
        })
    }

    pub fn space(space_id: &str, key_hex: &str, owner: &str) -> Result<Self> {
        let key: [u8; 32] = hex::decode(key_hex)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| anyhow!("space {space_id}: the key is not 32 bytes of hex"))?;
        let prefix = key_hex.as_bytes().to_vec();
        let s = blind(&prefix, &format!("space:{space_id}"));
        Ok(Self {
            space_id: Some(space_id.to_string()),
            conversation: key,
            prefix,
            d: s.clone(),
            stream: s.clone(),
            s: Some(s),
            owner: Some(owner.to_ascii_lowercase()).filter(|o| !o.is_empty()),
        })
    }

    /// A pack's plaintext encrypted for this stream at `cursor`: the blob
    /// a pack builder uploads.
    pub fn pack_blob(&self, plaintext: &[u8], cursor: u64) -> Vec<u8> {
        roostr_engine::pack::pack_encrypt(plaintext, &self.conversation, cursor)
    }

    /// The object's blinded `h` tag in this stream.
    pub fn tag_of(&self, object_id: &str) -> String {
        blind(&self.prefix, object_id)
    }

    fn tags(&self, object_id: &str) -> Vec<Vec<String>> {
        // Space events also carry the space-wide `h` and the owner, so the
        // owner's deletion of the space by `#h` covers them.
        let mut tags = vec![vec!["h".to_string(), self.tag_of(object_id)]];
        if let Some(s) = &self.s {
            tags.push(vec!["s".into(), s.clone()]);
            tags.push(vec!["h".into(), s.clone()]);
        }
        if let Some(owner) = &self.owner {
            tags.push(vec!["owner".into(), owner.clone()]);
        }
        tags.push(vec!["v".into(), "1".into()]);
        tags
    }

    /// The stream's patches and vanishes with seq above `after`.
    fn after(&self, author: &str, after: u64) -> Filter {
        let filter = Filter::new().kinds([PATCH_KIND, VANISH_KIND]).after(after);
        match &self.s {
            Some(s) => filter.tag('s', [s.clone()]),
            None => filter.authors([author]),
        }
    }

    /// Which stream an event of this relay belongs to (RoostrRelay: the
    /// first `s` tag, else the author).
    fn holds(&self, event: &Event) -> bool {
        match (&self.s, event.tag("s")) {
            (Some(s), Some(tag)) => s == tag,
            (None, None) => event.pubkey == self.stream,
            _ => false,
        }
    }

    fn cursor_key(&self) -> String {
        format!("{META_CURSOR}{}", self.stream)
    }

    fn seal(&self, text: &str) -> Result<String> {
        nip44::encrypt(text, &self.conversation).map_err(|e| anyhow!("nip44: {e}"))
    }

    fn open(&self, content: &str) -> Result<String> {
        nip44::decrypt(content, &self.conversation).map_err(|e| anyhow!("nip44: {e}"))
    }
}

// ── Events ──────────────────────────────────────────────────────────

/// The kind-31081 content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackAnnouncement {
    #[serde(default)]
    pub v: u32,
    #[serde(default)]
    pub stream: String,
    pub cursor: u64,
    /// Hex sha256 of the blob.
    pub sha256: String,
    /// Blob bytes.
    pub size: u64,
    #[serde(default)]
    pub url: String,
    /// Hex fingerprint root.
    pub root: String,
    /// The 256 bucket hashes, 16 hex each.
    pub buckets: Vec<String>,
    pub records: u64,
    #[serde(default)]
    pub built_at: u64,
    #[serde(default)]
    pub builder: String,
    /// Patches with seq at or below this were deleted from the relay.
    #[serde(default)]
    pub deleted_through: u64,
}

impl PackAnnouncement {
    /// The header matches what was announced: root and bucket prefixes.
    fn check(&self, header: &PackHeader) -> Result<()> {
        if header.buckets.len() != PACK_BUCKETS {
            bail!("pack header without {PACK_BUCKETS} buckets");
        }
        if hex::encode(pack_root(header)) != self.root {
            bail!("pack root differs from the announcement");
        }
        if self.buckets.len() == PACK_BUCKETS
            && header
                .buckets
                .iter()
                .zip(&self.buckets)
                .any(|(bucket, announced)| !hex::encode(bucket.hash).starts_with(announced.as_str()))
        {
            bail!("pack bucket hashes differ from the announcement");
        }
        if header.cursor != self.cursor {
            bail!("pack cursor {} is not the announced {}", header.cursor, self.cursor);
        }
        Ok(())
    }
}

/// The signed parts of one patch.
pub fn seal_patch(keys: &Keys, stream: &StreamKey, patch: &Patch, created_at: u64) -> Result<Vec<Event>> {
    let text = BASE64.encode(patch_canonical(patch));
    let count = text.len().div_ceil(CHUNK_CHARS).max(1);
    if count > MAX_CHUNKS {
        bail!("patch of {} exceeds the chunk limit", patch.object_id);
    }
    let gid = (count > 1).then(|| hex::encode(&Sha256::digest(text.as_bytes())[..8]));
    let mut events = Vec::with_capacity(count);
    for index in 0..count {
        let part = &text[(index * CHUNK_CHARS).min(text.len())..((index + 1) * CHUNK_CHARS).min(text.len())];
        let mut tags = stream.tags(&patch.object_id);
        if let Some(gid) = &gid {
            tags.push(vec!["c".into(), gid.clone(), index.to_string(), count.to_string()]);
        }
        events.push(keys.sign(EventTemplate {
            kind: PATCH_KIND,
            created_at,
            tags,
            content: stream.seal(part)?,
        }));
    }
    Ok(events)
}

pub fn seal_vanish(keys: &Keys, stream: &StreamKey, object_id: &str, created_at: u64) -> Result<Event> {
    let content = serde_json::json!({ "object_id": object_id }).to_string();
    Ok(keys.sign(EventTemplate {
        kind: VANISH_KIND,
        created_at,
        tags: stream.tags(object_id),
        content: stream.seal(&content)?,
    }))
}

pub fn seal_pack(keys: &Keys, stream: &StreamKey, announcement: &PackAnnouncement, created_at: u64) -> Result<Event> {
    Ok(keys.sign(EventTemplate {
        kind: PACK_KIND,
        created_at,
        tags: vec![vec!["d".into(), stream.d.clone()], vec!["v".into(), "1".into()]],
        content: stream.seal(&serde_json::to_string(announcement)?)?,
    }))
}

pub fn open_pack(stream: &StreamKey, event: &Event) -> Result<PackAnnouncement> {
    Ok(serde_json::from_str(&stream.open(&event.content)?)?)
}

/// What a stream event says.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Patch(Patch),
    Vanish(String),
}

/// Reassembles chunked patches across events.
#[derive(Default)]
struct Parts {
    groups: HashMap<String, BTreeMap<usize, String>>,
}

impl Parts {
    /// The event's message once whole; `Ok(None)` while parts are missing.
    fn open(&mut self, stream: &StreamKey, event: &Event) -> Result<Option<Incoming>> {
        let check = |object_id: &str| -> Result<()> {
            let tag = stream.tag_of(object_id);
            if !event.tag_values("h").any(|h| h == tag) {
                bail!("h tag does not name {object_id}");
            }
            Ok(())
        };
        let text = stream.open(&event.content)?;
        match event.kind {
            VANISH_KIND => {
                let body: Json = serde_json::from_str(&text)?;
                let id = body["object_id"].as_str().ok_or_else(|| anyhow!("vanish without object_id"))?;
                check(id)?;
                Ok(Some(Incoming::Vanish(id.to_string())))
            }
            PATCH_KIND => {
                let chunk = event.tags.iter().find(|t| t.first().map(String::as_str) == Some("c"));
                let whole = match chunk {
                    None => text,
                    Some(tag) => {
                        let (gid, index, total) = match tag.as_slice() {
                            [_, gid, index, total, ..] => (gid.clone(), index.parse::<usize>()?, total.parse::<usize>()?),
                            _ => bail!("malformed c tag"),
                        };
                        if total == 0 || total > MAX_CHUNKS || index >= total {
                            bail!("c tag out of range");
                        }
                        let group = self.groups.entry(gid.clone()).or_default();
                        group.insert(index, text);
                        if group.len() < total {
                            return Ok(None);
                        }
                        let whole: String = self.groups.remove(&gid).unwrap_or_default().into_values().collect();
                        if hex::encode(&Sha256::digest(whole.as_bytes())[..8]) != gid {
                            bail!("patch parts do not hash to their group");
                        }
                        whole
                    }
                };
                let bytes = BASE64.decode(whole.as_bytes())?;
                let patch = patch_decode(&bytes).ok_or_else(|| anyhow!("malformed patch"))?;
                check(&patch.object_id)?;
                Ok(Some(Incoming::Patch(patch)))
            }
            kind => bail!("kind {kind} is not a record event"),
        }
    }
}

// ── Local records ───────────────────────────────────────────────────

/// The record store with its clock: every read-modify-write of records
/// goes through here, one at a time.
pub struct Records {
    store: Arc<dyn RecordStore>,
    lock: AsyncMutex<()>,
    hlc: Mutex<Hlc>,
    device: AtomicU64,
}

impl Records {
    pub async fn open(store: Arc<dyn RecordStore>) -> Result<Self> {
        let last = store
            .record_meta(META_HLC.into())
            .await?
            .and_then(|text| text.parse::<u64>().ok())
            .unwrap_or(0);
        Ok(Self {
            store,
            lock: AsyncMutex::new(()),
            hlc: Mutex::new(Hlc { last }),
            device: AtomicU64::new(0),
        })
    }

    /// The stamp device of this replica's writes (its signer's index).
    pub fn set_device(&self, device: u32) {
        self.device.store(u64::from(device), Ordering::Relaxed);
    }

    pub fn store(&self) -> &Arc<dyn RecordStore> {
        &self.store
    }

    pub async fn ids(&self) -> Result<Vec<String>> {
        Ok(self.store.records().await?.into_iter().map(|(id, _)| id).collect())
    }

    pub async fn all(&self) -> Result<Vec<Record>> {
        self.store
            .records()
            .await?
            .into_iter()
            .map(|(id, bytes)| record_decode(&bytes).ok_or_else(|| anyhow!("record {id} does not decode")))
            .collect()
    }

    pub async fn get(&self, object_id: &str) -> Result<Option<Record>> {
        match self.store.record(object_id.to_string()).await? {
            None => Ok(None),
            Some(bytes) => Ok(Some(
                record_decode(&bytes).ok_or_else(|| anyhow!("record {object_id} does not decode"))?,
            )),
        }
    }

    /// The object's state as the replica serves it (`record_view`), with
    /// `heads` = the record hash so an unchanged record is recognized.
    pub async fn view(&self, object_id: &str) -> Result<Option<Json>> {
        Ok(self.get(object_id).await?.map(|record| view_of(&record)))
    }

    /// The fingerprint root of the records `keep` selects.
    pub async fn root(&self, keep: impl Fn(&str) -> bool) -> Result<[u8; 32]> {
        let records: Vec<Record> = self.all().await?.into_iter().filter(|r| keep(&r.object_id)).collect();
        Ok(root_hash(&bucket_hashes(&records)))
    }

    pub async fn cursor(&self, stream: &StreamKey) -> Result<Option<u64>> {
        Ok(self
            .store
            .record_meta(stream.cursor_key())
            .await?
            .and_then(|text| text.parse().ok()))
    }

    fn clock_meta(&self) -> (String, Option<String>) {
        (META_HLC.into(), Some(self.hlc.lock().last.to_string()))
    }

    /// Commits a planned local change: its patch (stamped now on this
    /// device) merged into the record and journaled, in one transaction.
    /// `false` when the change wrote nothing a record keeps.
    pub async fn commit_local(&self, engine: &Engine, change: &Change) -> Result<bool> {
        let _one = self.lock.lock().await;
        let current = self.get(&change.object_id).await?;
        let wall = if change.timestamp > 0 { change.timestamp as u64 } else { now_ms() };
        let device = self.device.load(Ordering::Relaxed) as u32;
        let stamp = {
            let mut clock = self.hlc.lock();
            Stamp {
                time: hlc_now(&mut clock, wall.max(now_ms())),
                device,
            }
        };
        let (patch, merged) = engine
            .record_patch(current.as_ref(), change, stamp)
            .await
            .map_err(|e| anyhow!("{e}"))?;
        if patch.entries.is_empty() {
            return Ok(false);
        }
        let batch = RecordBatch {
            put: vec![(merged.object_id.clone(), record_canonical(&merged))],
            journal: vec![JournalItem {
                key: format!("{:016x}-{}", stamp.time, patch.object_id),
                object_id: patch.object_id.clone(),
                patch: patch_canonical(&patch),
            }],
            meta: vec![self.clock_meta()],
            ..RecordBatch::default()
        };
        self.store.write_records(batch).await?;
        Ok(true)
    }

    /// Drops `object_id` locally (a vanish): the record goes, its id stays
    /// in the ledger for [`TOMBSTONE_DAYS`].
    pub async fn vanish_local(&self, object_id: &str) -> Result<()> {
        let _one = self.lock.lock().await;
        self.store
            .write_records(RecordBatch {
                remove: vec![object_id.to_string()],
                meta: vec![(format!("{META_VANISHED}{object_id}"), Some(now_ms().to_string()))],
                ..RecordBatch::default()
            })
            .await?;
        Ok(())
    }

    async fn vanished(&self, object_id: &str) -> Result<bool> {
        let at = self
            .store
            .record_meta(format!("{META_VANISHED}{object_id}"))
            .await?
            .and_then(|text| text.parse::<u64>().ok());
        Ok(at.is_some_and(|at| now_ms().saturating_sub(at) < TOMBSTONE_DAYS * DAY_MS))
    }

    /// Merges received messages and moves the stream's cursor, in one
    /// transaction; the ids that changed.
    async fn apply(&self, stream: &StreamKey, items: Vec<Incoming>, cursor: Option<u64>) -> Result<Vec<String>> {
        let _one = self.lock.lock().await;
        let mut touched: BTreeMap<String, Option<Record>> = BTreeMap::new();
        let mut vanished: HashSet<String> = HashSet::new();
        for item in items {
            match item {
                Incoming::Vanish(id) => {
                    vanished.insert(id.clone());
                    touched.insert(id, None);
                }
                Incoming::Patch(patch) => {
                    if vanished.contains(&patch.object_id) || self.vanished(&patch.object_id).await? {
                        continue;
                    }
                    let wall = now_ms();
                    let refused = {
                        let mut clock = self.hlc.lock();
                        patch.entries.iter().any(|entry| hlc_observe(&mut clock, entry.stamp, wall).is_err())
                    };
                    if refused {
                        log::warn!("[records] refused a patch of {} stamped over a day ahead", patch.object_id);
                        continue;
                    }
                    let current = match touched.remove(&patch.object_id) {
                        Some(record) => record,
                        None => self.get(&patch.object_id).await?,
                    };
                    let start = current.unwrap_or_else(|| Record {
                        object_id: patch.object_id.clone(),
                        ..Record::default()
                    });
                    match record_apply_patch(&start, &patch) {
                        Ok(merged) => {
                            touched.insert(patch.object_id.clone(), Some(merged));
                        }
                        Err(error) => {
                            log::warn!("[records] patch of {} does not apply: {error}", patch.object_id);
                            touched.insert(patch.object_id.clone(), Some(start));
                        }
                    }
                }
            }
        }
        let mut batch = RecordBatch {
            meta: vec![self.clock_meta()],
            ..RecordBatch::default()
        };
        for (id, record) in &touched {
            match record {
                Some(record) => batch.put.push((id.clone(), record_canonical(record))),
                None => {
                    batch.remove.push(id.clone());
                    batch.meta.push((format!("{META_VANISHED}{id}"), Some(now_ms().to_string())));
                }
            }
        }
        if let Some(cursor) = cursor {
            batch.meta.push((stream.cursor_key(), Some(cursor.to_string())));
        }
        self.store.write_records(batch).await?;
        Ok(touched.into_keys().collect())
    }

    /// Merges a pack's records into what is held (local entries with newer
    /// stamps survive) and sets the stream's cursor to the pack's unless
    /// this device is already past it, in one transaction.
    async fn merge_pack(&self, stream: &StreamKey, records: Vec<Record>, cursor: u64) -> Result<Vec<String>> {
        let _one = self.lock.lock().await;
        let held: HashMap<String, Vec<u8>> = self.store.records().await?.into_iter().collect();
        let mut batch = RecordBatch::default();
        let mut touched = Vec::new();
        {
            let mut clock = self.hlc.lock();
            for record in records {
                for entry in record.kind.iter().chain(&record.deleted).chain(&record.fields).chain(&record.blocks).chain(&record.messages) {
                    clock.last = clock.last.max(entry.stamp.time);
                }
                let merged = match held.get(&record.object_id).and_then(|bytes| record_decode(bytes)) {
                    Some(local) => record_merge(&local, &record).map_err(|e| anyhow!("{e}"))?,
                    None => record,
                };
                let bytes = record_canonical(&merged);
                if held.get(&merged.object_id) != Some(&bytes) {
                    touched.push(merged.object_id.clone());
                    batch.put.push((merged.object_id.clone(), bytes));
                }
            }
        }
        let mine = self.cursor(stream).await?;
        if mine.is_none_or(|mine| mine < cursor) {
            batch.meta.push((stream.cursor_key(), Some(cursor.to_string())));
        }
        batch.meta.push(self.clock_meta());
        self.store.write_records(batch).await?;
        Ok(touched)
    }
}

/// `record_view` plus `heads` = [record hash].
pub fn view_of(record: &Record) -> Json {
    let mut view = record_view(record);
    if let Json::Object(map) = &mut view {
        map.insert("heads".into(), Json::from(vec![hex::encode(record_hash(record))]));
    }
    view
}

// ── Packs ───────────────────────────────────────────────────────────

async fn get_range(url: &str, start: u64, end: u64) -> Result<Vec<u8>> {
    let range = format!("bytes={start}-{}", end - 1);
    let res = http::get(url, &[("Range", range.as_str())], None, end - start + 1)
        .await
        .map_err(|e| anyhow!("Blossom range {range}: {e}"))?;
    if !res.is_success() {
        bail!("Blossom range {range}: {} {}", res.status, res.reason);
    }
    if res.body.len() as u64 != end - start {
        bail!("Blossom range {range}: {} bytes", res.body.len());
    }
    Ok(res.body)
}

/// Where a pack blob is: its announced URL, else the Blossom server's.
fn pack_url(server: &str, announcement: &PackAnnouncement) -> String {
    if announcement.url.is_empty() {
        format!("{server}/{}", announcement.sha256)
    } else {
        announcement.url.clone()
    }
}

/// Downloads, verifies and opens a whole pack.
pub async fn fetch_pack(server: &str, stream: &StreamKey, announcement: &PackAnnouncement) -> Result<Vec<Record>> {
    if announcement.size > MAX_PACK_BYTES {
        bail!("pack of {} bytes is too large to load", announcement.size);
    }
    let cipher = blossom::download_blob(&pack_url(server, announcement), &announcement.sha256).await?;
    if cipher.len() as u64 != announcement.size {
        bail!("pack blob is {} bytes, announced {}", cipher.len(), announcement.size);
    }
    let plaintext = pack_decrypt(&cipher, &stream.conversation, announcement.cursor)?;
    let (header, start) = pack_read_header(&plaintext)?;
    announcement.check(&header)?;
    let mut records = Vec::with_capacity(header.records as usize);
    for bucket in 0..PACK_BUCKETS {
        if header.buckets[bucket].count == 0 {
            continue;
        }
        let bytes = pack_bucket_bytes(&plaintext, &header, start, bucket)?;
        records.extend(pack_read_bucket(bytes, &header, bucket)?);
    }
    Ok(records)
}

/// Lite: one bucket's records by HTTP Range (frame 0 for the header, then
/// the frames holding the bucket), checked against the announced hashes.
pub async fn fetch_pack_bucket(
    server: &str,
    stream: &StreamKey,
    announcement: &PackAnnouncement,
    bucket: usize,
) -> Result<Vec<Record>> {
    let url = pack_url(server, announcement);
    let key = pack_key(&stream.conversation, announcement.cursor);
    let whole = FRAME + TAG;
    let size = announcement.size;
    let frames = size.div_ceil(whole).max(1);
    let total = size.checked_sub(frames * TAG).ok_or_else(|| anyhow!("pack blob too small"))?;
    let first = frames_decrypt(&get_range(&url, 0, whole.min(size)).await?, &key, 0)?;
    let (header, start) = pack_read_header(&first)?;
    announcement.check(&header)?;
    let entry = header.buckets[bucket];
    if entry.count == 0 {
        return Ok(Vec::new());
    }
    let offset = start as u64 + entry.offset;
    let bytes = if offset + entry.length <= first.len() as u64 {
        first[offset as usize..(offset + entry.length) as usize].to_vec()
    } else {
        let range = frame_range(offset, entry.length, total);
        let plain = frames_decrypt(&get_range(&url, range.cipher_start, range.cipher_end).await?, &key, range.first)?;
        plain
            .get(range.skip as usize..(range.skip + entry.length) as usize)
            .ok_or_else(|| anyhow!("bucket {bucket} runs past its frames"))?
            .to_vec()
    };
    Ok(pack_read_bucket(&bytes, &header, bucket)?)
}

// ── Sync ────────────────────────────────────────────────────────────

/// What record sync tells its host.
pub trait RecordHost: MaybeSend + MaybeSync {
    /// Records changed (received patches, a pack).
    fn on_records(&self, ids: Vec<String>);
    /// The shared space a write to `object_id` goes to (`None`: personal).
    fn space_of(&self, object_id: &str) -> Option<String>;
}

/// Record-mode relay sync of one identity. Cheap to clone.
#[derive(Clone)]
pub struct RecordSync {
    inner: Arc<SyncInner>,
}

struct SyncInner {
    keys: Keys,
    relays: Vec<Relay>,
    records: Arc<Records>,
    host: Arc<dyn RecordHost>,
    personal: StreamKey,
    spaces: Mutex<Vec<StreamKey>>,
    stopped: AtomicBool,
    started: AtomicBool,
    /// Bumped by every (re)subscription: an older live loop ends.
    generation: AtomicU64,
    /// One catch-up or pack load per stream at a time.
    stream_lock: AsyncMutex<()>,
    publish_lock: AsyncMutex<()>,
}

impl RecordSync {
    pub fn new(keys: Keys, relays: Vec<String>, records: Arc<Records>, host: Arc<dyn RecordHost>) -> Result<Self> {
        records.set_device(device_index(keys.pubkey()));
        Ok(Self {
            inner: Arc::new(SyncInner {
                personal: StreamKey::personal(&keys)?,
                keys,
                relays: relays.into_iter().map(Relay::new).collect(),
                records,
                host,
                spaces: Mutex::new(Vec::new()),
                stopped: AtomicBool::new(false),
                started: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                stream_lock: AsyncMutex::new(()),
                publish_lock: AsyncMutex::new(()),
            }),
        })
    }

    pub fn records(&self) -> &Arc<Records> {
        &self.inner.records
    }

    /// Every stream synced: personal first, then the spaces.
    pub fn streams(&self) -> Vec<StreamKey> {
        self.inner.streams()
    }

    pub fn stream_of(&self, object_id: &str) -> StreamKey {
        self.inner.stream_of(object_id)
    }

    /// The shared spaces (id, key hex, owner pubkey) synced as their own
    /// streams. New ones are loaded and the live subscription follows them.
    pub async fn set_spaces(&self, spaces: Vec<(String, String, String)>) -> Result<()> {
        let next: Vec<StreamKey> = spaces
            .iter()
            .filter_map(|(id, key, owner)| StreamKey::space(id, key, owner).map_err(|e| log::warn!("[records] {e:#}")).ok())
            .collect();
        let fresh: Vec<StreamKey> = {
            let mut held = self.inner.spaces.lock();
            let known: HashSet<String> = held.iter().map(|s| s.stream.clone()).collect();
            let fresh = next.iter().filter(|s| !known.contains(&s.stream)).cloned().collect();
            *held = next;
            fresh
        };
        if self.inner.started.load(Ordering::Relaxed) && !fresh.is_empty() {
            for stream in &fresh {
                self.inner.sync_stream(stream).await?;
            }
            self.inner.clone().subscribe();
        }
        Ok(())
    }

    /// Loads every stream (pack, then patches after the cursor), sends the
    /// journal and goes live.
    pub async fn start(&self) -> Result<()> {
        if self.inner.relays.is_empty() {
            return Ok(()); // offline: the journal waits
        }
        self.inner.stopped.store(false, Ordering::Relaxed);
        self.inner.started.store(true, Ordering::Relaxed);
        for stream in self.streams() {
            self.inner.sync_stream(&stream).await?;
        }
        self.inner.publish_journal().await?;
        self.inner.clone().subscribe();
        Ok(())
    }

    pub fn stop(&self) {
        self.inner.stopped.store(true, Ordering::Relaxed);
        self.inner.started.store(false, Ordering::Relaxed);
        self.inner.generation.fetch_add(1, Ordering::Relaxed);
        for relay in &self.inner.relays {
            relay.disconnect();
        }
    }

    /// Sends the journal in the background (after a local commit).
    pub fn spawn_publish(&self) {
        let inner = self.inner.clone();
        rt::spawn(async move {
            if let Err(error) = inner.publish_journal().await {
                log::info!("[records] journal not sent yet: {error:#}");
            }
        });
    }

    /// Sends every journaled patch; each leaves the journal once a relay
    /// accepted it.
    pub async fn publish_journal(&self) -> Result<usize> {
        self.inner.publish_journal().await
    }

    /// Publishes a vanish of `object_id` and drops it locally.
    pub async fn vanish(&self, object_id: &str) -> Result<()> {
        self.inner.records.vanish_local(object_id).await?;
        let stream = self.inner.stream_of(object_id);
        let event = seal_vanish(&self.inner.keys, &stream, object_id, roostr_nostr::now_s())?;
        roostr_nostr::relay::publish_any(&self.inner.relays, &event, PUBLISH_TIMEOUT).await?;
        Ok(())
    }

    /// One stream's newest pack announcement on the relays.
    pub async fn announcement(&self, stream: &StreamKey) -> Result<Option<PackAnnouncement>> {
        self.inner.announcement(stream).await
    }

    /// Lite: the record of one object from its stream's pack bucket (Range
    /// requests), merged with the stream's patches for it after the pack.
    /// Not stored.
    pub async fn fetch_object(&self, object_id: &str) -> Result<Option<Record>> {
        let stream = self.inner.stream_of(object_id);
        let mut record = None;
        let mut after = 0;
        if let Some(announcement) = self.inner.announcement(&stream).await? {
            let bucket = usize::from(roostr_engine::record::record_bucket(object_id));
            let records = fetch_pack_bucket(&self.inner.server().await, &stream, &announcement, bucket).await?;
            record = records.into_iter().find(|r| r.object_id == object_id);
            after = announcement.cursor;
        }
        let filter = stream.after(self.inner.keys.pubkey(), after).tag('h', [stream.tag_of(object_id)]);
        let (events, _) = self.inner.page(filter).await?;
        let mut parts = Parts::default();
        for (event, _) in events {
            match parts.open(&stream, &event) {
                Ok(Some(Incoming::Patch(patch))) if patch.object_id == object_id => {
                    let start = record.take().unwrap_or_else(|| Record {
                        object_id: object_id.to_string(),
                        ..Record::default()
                    });
                    record = Some(record_apply_patch(&start, &patch).map_err(|e| anyhow!("{e}"))?);
                }
                Ok(Some(Incoming::Vanish(id))) if id == object_id => record = None,
                Ok(_) => {}
                Err(error) => log::debug!("[records] skipped event {}: {error:#}", event.id),
            }
        }
        Ok(record)
    }
}

/// A stream's heads from a `Cursor` EOSE.
#[derive(Debug, Clone, Copy, Default)]
struct Heads {
    head: u64,
    oldest: u64,
}

impl Heads {
    fn of(value: &Json) -> Option<Self> {
        Some(Self {
            head: value.get("head")?.as_u64()?,
            oldest: value.get("oldest")?.as_u64()?,
        })
    }
}

impl SyncInner {
    fn streams(&self) -> Vec<StreamKey> {
        std::iter::once(self.personal.clone())
            .chain(self.spaces.lock().iter().cloned())
            .collect()
    }

    fn stream_of(&self, object_id: &str) -> StreamKey {
        self.host
            .space_of(object_id)
            .and_then(|space| {
                self.spaces
                    .lock()
                    .iter()
                    .find(|s| s.space_id.as_deref() == Some(space.as_str()))
                    .cloned()
            })
            .unwrap_or_else(|| self.personal.clone())
    }

    fn cursor_relay(&self) -> Result<&Relay> {
        self.relays.first().ok_or_else(|| anyhow!("no relays"))
    }

    async fn server(&self) -> String {
        let urls: Vec<String> = self.relays.iter().map(|r| r.url().to_string()).collect();
        blossom::blossom_server(&urls).await
    }

    async fn announcement(&self, stream: &StreamKey) -> Result<Option<PackAnnouncement>> {
        let filter = Filter::new().kinds([PACK_KIND]).tag('d', [stream.d.clone()]);
        let events = roostr_nostr::relay::query_many(&self.relays, &[filter], QUERY_TIMEOUT).await?;
        let mut newest: Option<(u64, PackAnnouncement)> = None;
        for event in events {
            match open_pack(stream, &event) {
                Ok(announcement) if newest.as_ref().is_none_or(|(cursor, _)| announcement.cursor > *cursor) => {
                    newest = Some((announcement.cursor, announcement));
                }
                Ok(_) => {}
                Err(error) => log::debug!("[records] unreadable pack announcement {}: {error:#}", event.id),
            }
        }
        Ok(newest.map(|(_, a)| a))
    }

    /// One `REQ after` page from the cursor relay: sequenced events and the
    /// heads (when the relay sent them).
    async fn page(&self, filter: Filter) -> Result<(Vec<(Event, u64)>, Option<Heads>)> {
        let relay = self.cursor_relay()?;
        let mut sub = relay.subscribe(vec![filter]);
        let collect = async {
            let mut events = Vec::new();
            loop {
                match sub.next().await {
                    Some(Ok(SubscriptionItem::Sequenced(event, seq))) => events.push((event, seq)),
                    Some(Ok(SubscriptionItem::Event(event))) => {
                        log::debug!("[records] event {} came without a seq", event.id);
                    }
                    Some(Ok(SubscriptionItem::Cursor(value))) => return Ok((events, Heads::of(&value))),
                    Some(Ok(SubscriptionItem::Eose)) => return Ok((events, None)),
                    Some(Err(error)) => return Err(anyhow!("{}: {error}", relay.url())),
                    None => return Err(anyhow!("{}: disconnected", relay.url())),
                }
            }
        };
        roostr_nostr::timeout(QUERY_TIMEOUT, collect)
            .await
            .unwrap_or_else(|| Err(anyhow!("{}: no EOSE", relay.url())))
    }

    /// Brings one stream up to the relay's head: the newest pack when there
    /// is no cursor, the builder deleted patches after it (the pack's
    /// `deleted_through`), or the relay no longer holds what follows it;
    /// then pages. The relay's window alone does not show a retention cut:
    /// any older event the stream keeps (a profile, another member's patch)
    /// holds `oldest` down.
    async fn sync_stream(&self, stream: &StreamKey) -> Result<()> {
        let _one = self.stream_lock.lock().await;
        let announcement = self.announcement(stream).await?;
        let mut cursor = self.records.cursor(stream).await?;
        // Without a cursor the newest pack is the start, if there is one.
        let mut loaded_pack = cursor.is_none();
        if let Some(pack) = &announcement
            && cursor.is_none_or(|c| c < pack.cursor && c < pack.deleted_through)
        {
            if let Some(c) = cursor {
                log::info!("[records] {stream:?}: cursor {c} is behind the pack's retention ({})", pack.deleted_through);
            }
            self.load_pack(stream, pack).await?;
            cursor = Some(cursor.map_or(pack.cursor, |c| c.max(pack.cursor)));
            loaded_pack = true;
        }
        let mut after = cursor.unwrap_or(0);
        loop {
            let (events, heads) = self
                .page(stream.after(self.keys.pubkey(), after).limit(PAGE))
                .await?;
            if let Some(heads) = heads
                && !loaded_pack
                && after + 1 < heads.oldest
            {
                // Away past the window: the newest pack, merged, then on.
                log::info!("[records] {stream:?}: cursor {after} is behind the relay's window ({})", heads.oldest);
                loaded_pack = true;
                if let Some(pack) = &announcement {
                    self.load_pack(stream, pack).await?;
                    after = after.max(pack.cursor);
                }
                continue;
            }
            let count = events.len() as u64;
            let last = events.iter().map(|(_, seq)| *seq).max().unwrap_or(after);
            self.apply(stream, events, last).await?;
            after = last;
            let done = heads.is_some_and(|h| after >= h.head) || count < PAGE;
            if done {
                if cursor.is_none() || cursor != Some(after) {
                    self.records.apply(stream, Vec::new(), Some(after)).await?;
                }
                return Ok(());
            }
        }
    }

    /// Downloads, checks and merges the announced pack.
    async fn load_pack(&self, stream: &StreamKey, announcement: &PackAnnouncement) -> Result<()> {
        let records = fetch_pack(&self.server().await, stream, announcement)
            .await
            .with_context(|| format!("pack of {stream:?} at cursor {}", announcement.cursor))?;
        let touched = self.records.merge_pack(stream, records, announcement.cursor).await?;
        if !touched.is_empty() {
            self.host.on_records(touched);
        }
        Ok(())
    }

    async fn apply(&self, stream: &StreamKey, mut events: Vec<(Event, u64)>, cursor: u64) -> Result<()> {
        events.sort_by_key(|(_, seq)| *seq);
        let mut parts = Parts::default();
        let mut items = Vec::new();
        for (event, _) in &events {
            if !stream.holds(event) {
                continue;
            }
            match parts.open(stream, event) {
                Ok(Some(item)) => items.push(item),
                Ok(None) => {}
                Err(error) => log::debug!("[records] skipped event {}: {error:#}", event.id),
            }
        }
        let touched = self.records.apply(stream, items, Some(cursor)).await?;
        if !touched.is_empty() {
            self.host.on_records(touched);
        }
        Ok(())
    }

    async fn publish_journal(&self) -> Result<usize> {
        let _one = self.publish_lock.lock().await;
        if self.relays.is_empty() {
            return Ok(0);
        }
        let journal = self.records.store().journal().await?;
        let mut sent = 0;
        for item in journal {
            let Some(patch) = patch_decode(&item.patch) else {
                log::warn!("[records] journal item {} does not decode; dropped", item.key);
                self.records.store().remove_journal(vec![item.key]).await?;
                continue;
            };
            let stream = self.stream_of(&item.object_id);
            for event in seal_patch(&self.keys, &stream, &patch, roostr_nostr::now_s())? {
                roostr_nostr::relay::publish_any(&self.relays, &event, PUBLISH_TIMEOUT).await?;
            }
            self.records.store().remove_journal(vec![item.key]).await?;
            sent += 1;
        }
        Ok(sent)
    }

    /// The live subscription on the cursor relay, every stream after its
    /// cursor; a gap or a reconnect catches up through [`Self::sync_stream`].
    fn subscribe(self: Arc<Self>) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        rt::spawn(async move {
            let mut backoff = BACKOFF_FLOOR;
            let mut reconnecting = false;
            while !self.stopped.load(Ordering::Relaxed) && self.generation.load(Ordering::Relaxed) == generation {
                if reconnecting {
                    roostr_nostr::sleep(backoff).await;
                    backoff = (backoff * 2).min(BACKOFF_CEILING);
                    let mut caught_up = true;
                    for stream in self.streams() {
                        if let Err(error) = self.sync_stream(&stream).await {
                            log::info!("[records] catch-up after a reconnect failed: {error:#}");
                            caught_up = false;
                            break;
                        }
                    }
                    if !caught_up {
                        continue;
                    }
                    if let Err(error) = self.publish_journal().await {
                        log::info!("[records] journal not sent yet: {error:#}");
                    }
                }
                if let Err(error) = self.live(generation).await {
                    log::info!("[records] live subscription ended: {error:#}");
                }
                reconnecting = true;
            }
        });
    }

    async fn live(&self, generation: u64) -> Result<()> {
        let streams = self.streams();
        let mut cursors = HashMap::new();
        let mut filters = Vec::new();
        for stream in &streams {
            let cursor = self.records.cursor(stream).await?.unwrap_or(0);
            cursors.insert(stream.stream.clone(), cursor);
            filters.push(stream.after(self.keys.pubkey(), cursor).limit(1));
        }
        let relay = self.cursor_relay()?.clone();
        let mut sub = relay.subscribe(filters);
        loop {
            let item = sub.next().await;
            if self.stopped.load(Ordering::Relaxed) || self.generation.load(Ordering::Relaxed) != generation {
                return Ok(());
            }
            match item {
                Some(Ok(SubscriptionItem::Sequenced(event, seq))) => {
                    let Some(stream) = streams.iter().find(|s| s.holds(&event)) else {
                        continue;
                    };
                    let cursor = cursors.get(&stream.stream).copied().unwrap_or(0);
                    if seq <= cursor {
                        continue;
                    }
                    if seq == cursor + 1 {
                        self.apply(stream, vec![(event, seq)], seq).await?;
                    } else {
                        // A gap: everything after the cursor, in order.
                        self.sync_stream(stream).await?;
                    }
                    let now = self.records.cursor(stream).await?.unwrap_or(seq);
                    cursors.insert(stream.stream.clone(), now);
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => bail!("{}: {error}", relay.url()),
                None => bail!("{}: disconnected", relay.url()),
            }
        }
    }
}

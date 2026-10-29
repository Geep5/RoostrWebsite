//! `replay`, DAG heads, and state sync (docs/state-sync.md "Core API"):
//! `replay.ts` and Engine.swift's state-sync calls.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::codec::{base64_bytes, base64_option, to_base64};
use crate::record::{Patch, Record, ReplayWriters, Stamp, from_object_state, patch_decode, record_canonical, record_decode};
use crate::{Change, Engine, EngineError};

/// Everything held for one object that its state is built from
/// (`ObjectInputs` / iOS `ObjectHoldings`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StateInputs {
    /// Every held base of the object (raw Checkpoint protobuf, epoch >= 1), current and superseded.
    pub bases: Vec<Vec<u8>>,
    /// Held changes, rebased orphans excluded; `b` set when their event carried one.
    pub changes: Vec<Change>,
    /// Legacy kind-1079 checkpoint; only sent when there is no base.
    pub checkpoint: Option<Vec<u8>>,
    /// Ids of changes this device wrote and never published (any object;
    /// narrowed to this object's changes on the way in).
    pub pending: Vec<String>,
    /// Ids of changes this device wrote that no held base has proven folded
    /// yet, published or not (pending ones need not repeat; narrowed like
    /// `pending`). Only these are ever rebased here.
    pub own: Vec<String>,
}

impl StateInputs {
    pub fn is_empty(&self) -> bool {
        self.bases.is_empty() && self.changes.is_empty() && self.checkpoint.is_none()
    }
}

/// The shared state-sync request (`replay.ts stateRequest`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateRequest<'a> {
    action: &'a str,
    bases: Vec<String>,
    changes: &'a [Change],
    pending: Vec<&'a str>,
    own: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    orphans: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    now_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    force: Option<bool>,
}

impl<'a> StateRequest<'a> {
    fn new(action: &'a str, input: &'a StateInputs) -> Self {
        let held: HashSet<&str> = input
            .changes
            .iter()
            .map(|change| change.id.as_str())
            .collect();
        let held_only = |ids: &'a [String]| -> Vec<&'a str> {
            ids.iter()
                .map(String::as_str)
                .filter(|id| held.contains(id))
                .collect()
        };
        StateRequest {
            action,
            bases: input.bases.iter().map(|base| to_base64(base)).collect(),
            changes: &input.changes,
            pending: held_only(&input.pending),
            own: held_only(&input.own),
            checkpoint: input
                .checkpoint
                .as_deref()
                .filter(|_| input.bases.is_empty())
                .map(to_base64),
            orphans: None,
            now_ms: None,
            force: None,
        }
    }
}

/// `classify`: how each held change relates to the current base (hex id lists).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    /// `None`: no base, every change is live.
    pub base: Option<ClassificationBase>,
    pub live: Vec<String>,
    pub covered: Vec<String>,
    pub premigration_covered: Vec<String>,
    pub orphan: Vec<String>,
    pub stale: Vec<String>,
    pub live_heads: Vec<String>,
    /// Own orphans: what [`Engine::rebase`] rebases.
    pub rebase: Vec<String>,
    /// Another device's not-live leftovers: fold them away, never rebase.
    pub discard: Vec<String>,
    /// Own changes a held strict ancestor of the current base folded (by id
    /// or a copy of the edit): drop the own mark, fold them away.
    pub proven: Vec<String>,
}

/// The current base a [`Classification`] is relative to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ClassificationBase {
    pub hash: String,
    pub epoch: i64,
}

/// `rebase`: orphans re-applied on the current base, in publish order.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Rebased {
    /// Hash of the base every change was rebased onto: published as `["b", base]`.
    pub base: String,
    pub changes: Vec<RebasedChange>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RebasedChange {
    /// The orphan this change replaces (stored as rebased, never replayed again).
    pub orphan: String,
    pub id: String,
    pub change: Change,
    /// Protobuf of the new change.
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
}

/// `base_current`: the current base (highest epoch, then created_at, then hash).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BaseCurrent {
    pub index: usize,
    pub hash: String,
    pub epoch: i64,
}

/// A base as the engine summarises it (bytes stay opaque).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseSummary {
    pub object_id: String,
    /// sha256 hex of the base bytes (field 8 absent): its identity.
    pub hash: String,
    pub epoch: i64,
    /// Hex hash of the previous base; "" at epoch 1.
    pub prev_base: String,
    /// Unix ms; part of the current-base tie-break.
    pub created_at: i64,
    pub head_ids: Vec<String>,
    /// Number of covered change ids.
    pub covered: i64,
}

/// `base_decode`: a base's summary, covered ids and state.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseDecoded {
    #[serde(flatten)]
    pub summary: BaseSummary,
    pub covered_ids: Vec<String>,
    /// The base's ObjectJSON, `heads` = its head ids.
    pub state: Value,
}

/// `base_open_blob`: a pointer base's decrypted blob as an ingested base.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BaseBlobOpened {
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
    pub base: BaseSummary,
}

/// `compact_plan`: whether to fold the object's live deltas into a new base.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactPlan {
    pub compact: bool,
    pub reason: String,
    pub live: i64,
    pub live_bytes: i64,
    pub oldest_ms: i64,
    pub orphans: i64,
    pub stale: i64,
    /// The new base's protobuf, when `compact`.
    #[serde(default, deserialize_with = "base64_option")]
    pub base: Option<Vec<u8>>,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub epoch: Option<i64>,
    #[serde(default)]
    pub prev_base: Option<String>,
    /// Change ids the new base covers.
    #[serde(default)]
    pub folded: Option<Vec<String>>,
}

#[derive(Serialize)]
struct ReplayRequest<'a> {
    changes: &'a [Change],
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint: Option<String>,
}

#[derive(Serialize)]
struct HeadsRequest<'a, C> {
    action: &'a str,
    changes: &'a [C],
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint: Option<String>,
}

impl Engine {
    /// Replays an object's full change set (any order) into the ObjectJSON
    /// `/api/objects/:id` serves; a legacy checkpoint seeds the state and the
    /// changes it covers are skipped (`core.checkpoint_for_replay`). `None`
    /// when there is nothing to replay.
    pub async fn replay(
        &self,
        changes: &[Change],
        checkpoint: Option<&[u8]>,
    ) -> Result<Option<Value>, EngineError> {
        if changes.is_empty() && checkpoint.is_none() {
            return Ok(None);
        }
        let payload = ReplayRequest {
            changes,
            checkpoint: checkpoint.map(to_base64),
        };
        self.call_as("replay", &payload, None).await
    }

    /// DAG heads: changes no other change names as a parent, plus the
    /// checkpoint's heads nothing built on (`mutation` `heads`).
    pub async fn heads(
        &self,
        changes: &[Change],
        checkpoint: Option<&[u8]>,
    ) -> Result<Vec<String>, EngineError> {
        self.heads_of(changes, checkpoint).await
    }

    async fn heads_of<C: Serialize>(
        &self,
        changes: &[C],
        checkpoint: Option<&[u8]>,
    ) -> Result<Vec<String>, EngineError> {
        let payload = HeadsRequest {
            action: "heads",
            changes,
            checkpoint: checkpoint.map(to_base64),
        };
        self.call_as("mutation", &payload, None).await
    }

    /// Current base state + live deltas (`replay_from_base`); with no base,
    /// the plain replay seeded by the legacy checkpoint. The state's `heads`
    /// are the sorted heads it was replayed to.
    pub async fn replay_from_base(
        &self,
        input: &StateInputs,
    ) -> Result<Option<Value>, EngineError> {
        if input.is_empty() {
            return Ok(None);
        }
        self.call_as("sync", &StateRequest::new("replay_from_base", input), None)
            .await
    }

    pub async fn classify(&self, input: &StateInputs) -> Result<Classification, EngineError> {
        self.call_as("sync", &StateRequest::new("classify", input), None)
            .await
    }

    /// Rebases this device's own `orphans` (hex ids; every own orphan when
    /// `None`) onto the current base. Needs a base.
    pub async fn rebase(
        &self,
        input: &StateInputs,
        orphans: Option<&[String]>,
    ) -> Result<Rebased, EngineError> {
        let mut request = StateRequest::new("rebase", input);
        request.orphans = orphans;
        self.call_as("sync", &request, None).await
    }

    /// Index of the current base among `bases`; `None` for none.
    pub async fn base_current(
        &self,
        bases: &[Vec<u8>],
    ) -> Result<Option<BaseCurrent>, EngineError> {
        if bases.is_empty() {
            return Ok(None);
        }
        let payload = serde_json::json!({
            "action": "base_current",
            "bases": bases.iter().map(|base| to_base64(base)).collect::<Vec<_>>(),
        });
        self.call_as("sync", &payload, None).await
    }

    pub async fn base_decode(&self, base: &[u8]) -> Result<BaseDecoded, EngineError> {
        let payload = serde_json::json!({"action": "base_decode", "base": to_base64(base)});
        self.call_as("sync", &payload, None).await
    }

    /// A pointer base's blob, verified (sha256) and decrypted by the host:
    /// it must be a base whose hash is the pointer's `hash`.
    pub async fn base_open_blob(
        &self,
        hash: &str,
        bytes: &[u8],
    ) -> Result<BaseBlobOpened, EngineError> {
        let payload = serde_json::json!({"action": "base_open_blob", "hash": hash, "bytes": to_base64(bytes)});
        self.call_as("sync", &payload, None).await
    }

    /// Whether (and how) to compact the object into a new base at `now_ms`.
    pub async fn compact_plan(
        &self,
        input: &StateInputs,
        now_ms: i64,
        force: bool,
    ) -> Result<CompactPlan, EngineError> {
        let mut request = StateRequest::new("compact_plan", input);
        request.now_ms = Some(now_ms);
        request.force = Some(force);
        if let Some(checkpoint) = input.checkpoint.as_deref() {
            // compact_plan reads the legacy checkpoint even beside bases.
            request.checkpoint = Some(to_base64(checkpoint));
        }
        self.call_as("sync", &request, None).await
    }

    /// DAG heads a new local change builds on (`replay.ts objectHeads`).
    /// With a base: the live heads plus the base heads nothing live built on
    /// (heads of the live deltas over the current base as a checkpoint);
    /// without one, the plain heads of every held change and the legacy checkpoint.
    pub async fn object_heads(&self, input: &StateInputs) -> Result<Vec<String>, EngineError> {
        if input.bases.is_empty() {
            return self
                .heads(&input.changes, input.checkpoint.as_deref())
                .await;
        }
        let classification = self.classify(input).await?;
        let Some(current) = self.base_current(&input.bases).await? else {
            return self.heads(&input.changes, None).await;
        };
        let live: HashSet<&str> = classification.live.iter().map(String::as_str).collect();
        let changes: Vec<&Change> = input
            .changes
            .iter()
            .filter(|change| live.contains(change.id.as_str()))
            .collect();
        self.heads_of(&changes, Some(&input.bases[current.index]))
            .await
    }

    /// One replay of the object's DAG that also reports, per entry key, the
    /// change that last wrote it (`record` `writers`): the input of
    /// [`crate::record::from_object_state`]. The state is lossless (wire
    /// blocks, `__content__` kept).
    pub async fn record_writers(
        &self,
        changes: &[Change],
        checkpoint: Option<&[u8]>,
    ) -> Result<ReplayWriters, EngineError> {
        let payload = RecordRequest {
            action: "writers",
            changes,
            checkpoint: checkpoint.map(to_base64),
            devices: None,
        };
        self.call_as("record", &payload, None).await
    }

    /// Converts an object's DAG into a stamped [`Record`]: one replay through
    /// [`Engine::record_writers`], stamped here with `device_of(author)`.
    pub async fn record_from_changes(
        &self,
        changes: &[Change],
        checkpoint: Option<&[u8]>,
        device_of: impl Fn(&str) -> Option<u32>,
    ) -> Result<Record, EngineError> {
        let replayed = self.record_writers(changes, checkpoint).await?;
        let mut writers = replayed.writers;
        writers.seed = replayed.seed;
        from_object_state(&replayed.state, &writers, device_of)
            .map_err(|error| EngineError::Domain(error.to_string()))
    }

    /// The engine's own conversion (`record` `from_changes`), for parity
    /// checks against [`Engine::record_from_changes`].
    pub async fn engine_record_from_changes(
        &self,
        changes: &[Change],
        checkpoint: Option<&[u8]>,
        devices: &BTreeMap<String, u32>,
    ) -> Result<EngineRecord, EngineError> {
        let payload = RecordRequest {
            action: "from_changes",
            changes,
            checkpoint: checkpoint.map(to_base64),
            devices: Some(devices),
        };
        self.call_as("record", &payload, None).await
    }

    /// The patch a local change writes on `record` (`None`: the object has
    /// no record yet), every entry at `stamp`, and the record with it merged
    /// (`record` `patch`, the daemon's record-mode commit). An empty patch
    /// means the change wrote nothing a record keeps.
    pub async fn record_patch(
        &self,
        record: Option<&Record>,
        change: &Change,
        stamp: Stamp,
    ) -> Result<(Patch, Record), EngineError> {
        #[derive(Deserialize)]
        struct Out {
            patch: String,
            record: String,
        }
        let payload = serde_json::json!({
            "action": "patch",
            "record": record.map(|r| hex_encode(&record_canonical(r))).unwrap_or_default(),
            "change": change,
            "stamp": {"time": stamp.time, "device": stamp.device},
        });
        let out: Out = self.call_as("record", &payload, None).await?;
        let bad = |what: &str| EngineError::Domain(format!("record patch: malformed {what}"));
        let patch = hex_decode(&out.patch)
            .and_then(|bytes| patch_decode(&bytes))
            .ok_or_else(|| bad("patch"))?;
        let merged = hex_decode(&out.record)
            .and_then(|bytes| record_decode(&bytes))
            .ok_or_else(|| bad("record"))?;
        Ok((patch, merged))
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[derive(Serialize)]
struct RecordRequest<'a> {
    action: &'a str,
    changes: &'a [Change],
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    devices: Option<&'a BTreeMap<String, u32>>,
}

/// `record` `from_changes`: the engine's converted record.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EngineRecord {
    /// Record JSON ([`Record::from_json`]).
    pub record: Value,
    /// Hex canonical protobuf.
    pub canonical: String,
    /// Hex sha256 of `canonical`.
    pub hash: String,
}

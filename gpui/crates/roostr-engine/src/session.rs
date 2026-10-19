//! `sync` receive session and outbox (`sync_session.odin`, `sync_outbox.odin`;
//! `sync.ts RelaySync`, Engine.swift `SessionState`/`IngestResult`/…).
//!
//! The session is engine-global: one per engine, opened by the host that
//! owns relay sync. It reassembles chunk groups, tracks the cursor high-water
//! mark and replay obligations, decrypts and verifies events the host has
//! already signature-checked, and queues publish obligations with ordering,
//! dedupe, the rotated-key rule, backoff and sealing on first attempt.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::codec::base64_bytes;
use crate::{BaseSummary, Change, Engine, EngineError, SharedProvenance};

/// A space the session decrypts and blinds for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSpace {
    pub space_id: String,
    pub key_hex: String,
    pub key_id: i64,
    /// Owner pubkey (hex); a keyring entry without one is this identity's own space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// `session`: opens (or reopens) the receive session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOpen {
    /// This identity's pubkey, hex.
    pub pk: String,
    /// Personal NIP-44 conversation key (self ECDH), hex.
    pub conversation_key: String,
    /// Secret key hex: blinds personal `h` tags (outbox seals).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    pub spaces: Vec<SessionSpace>,
    /// Unix seconds of the newest imported event.
    pub cursor: i64,
    /// Persisted replay obligations: chunk key → earliest created_at.
    pub replay_groups: Vec<(String, i64)>,
    /// Chunk groups already imported.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub imported_groups: Vec<String>,
}

/// The session's cursor, open groups, outbox size and replay obligations.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub cursor: i64,
    /// Partially assembled chunk groups.
    pub groups: i64,
    #[serde(default)]
    pub pending: i64,
    pub replay_groups: Vec<(String, i64)>,
    /// Earliest replay obligation; the history walk must reach back to it.
    #[serde(default)]
    pub replay_floor: Option<i64>,
}

/// A relay event as the host received (and signature-checked) it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayEvent {
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u16,
    pub tags: Vec<Vec<String>>,
    pub content: String,
}

/// `ingest`: what one event did to the session.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestResult {
    pub cursor: i64,
    /// A complete payload, once its last part arrived.
    #[serde(default)]
    pub item: Option<IngestItem>,
    /// A replay fault: the host must re-walk history from here.
    #[serde(default)]
    pub fault_at: Option<i64>,
    /// The replay obligations, when they changed.
    #[serde(default)]
    pub replay_groups: Option<Vec<(String, i64)>>,
    #[serde(default)]
    pub decrypt_failure: bool,
    #[serde(default)]
    pub decode_failure: bool,
    /// The event's blinded `h` tag.
    #[serde(default)]
    pub h_tag: Option<String>,
    /// A verified kind-5 from the administrator of an installed space: that
    /// space is gone for every member.
    #[serde(default)]
    pub space_vanished: Option<String>,
}

/// One decrypted relay payload: a change (1078), a checkpoint (1079) or a base (31078 + 1080 parts).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestItem {
    /// Wire bytes of the full change, checkpoint or base.
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
    #[serde(default)]
    pub change: Option<Change>,
    /// The base hash a change's event named (`["b", hash]`); `None` = pre-migration.
    #[serde(default)]
    pub b: Option<String>,
    #[serde(default)]
    pub checkpoint: Option<CheckpointSummary>,
    #[serde(default)]
    pub base: Option<BaseSummary>,
    /// Chunk group to [`Engine::settle`] once imported.
    #[serde(default)]
    pub chunk_key: Option<String>,
    /// Set for shared-space events: authorize before import.
    #[serde(default)]
    pub provenance: Option<SharedProvenance>,
}

/// A reassembled kind-1079 checkpoint as the session summarises it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointSummary {
    pub object_id: String,
    pub head_ids: Vec<String>,
    pub covered: i64,
    pub hash: String,
}

/// `settle` / `retire`: the replay obligations, when they changed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleResult {
    #[serde(default)]
    pub replay_groups: Option<Vec<(String, i64)>>,
}

/// A publish obligation offered to the outbox (`PendingPublish`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxPending {
    pub key: String,
    pub object_id: String,
    pub change_id: String,
    /// Change protobuf.
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<i64>,
    /// Hash of the base the change was written on: sealed as `["b", base]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// The host already holds signed events for it (retries reuse them).
    pub has_events: bool,
}

/// `outbox_enqueue`: whether the item was queued, and the queue size.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EnqueueResult {
    pub queued: bool,
    /// "already queued" / "rotated space key".
    #[serde(default)]
    pub reason: Option<String>,
    pub pending: i64,
}

/// `outbox_next`: the next obligation due, or how long to wait.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxNext {
    #[serde(default)]
    pub item: Option<OutboxItem>,
    pub wait_ms: i64,
    pub pending: i64,
}

/// One obligation handed out; `sealed` until the host reports `sealed: true`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxItem {
    pub key: String,
    pub object_id: String,
    pub change_id: String,
    pub attempts: i64,
    #[serde(default)]
    pub space_id: Option<String>,
    #[serde(default)]
    pub key_id: Option<i64>,
    #[serde(default)]
    pub sealed: Option<crate::Sealed>,
}

#[derive(Deserialize)]
struct Pending {
    pending: i64,
}

#[derive(Serialize)]
struct WithAction<'a, T> {
    action: &'a str,
    #[serde(flatten)]
    fields: &'a T,
}

#[derive(Serialize)]
struct Spaces<'a> {
    action: &'a str,
    spaces: &'a [SessionSpace],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Ingest<'a> {
    action: &'a str,
    event: &'a RelayEvent,
    now_ms: i64,
}

#[derive(Serialize)]
struct Enqueue<'a> {
    action: &'a str,
    pending: &'a OutboxPending,
}

impl Engine {
    /// Opens the receive session (closing any previous one).
    pub async fn session_open(&self, open: &SessionOpen) -> Result<SessionState, EngineError> {
        self.call_as(
            "sync",
            &WithAction {
                action: "session",
                fields: open,
            },
            None,
        )
        .await
    }

    /// Replaces the session's spaces (keyring changed).
    pub async fn session_spaces(
        &self,
        spaces: &[SessionSpace],
    ) -> Result<SessionState, EngineError> {
        self.call_as(
            "sync",
            &Spaces {
                action: "spaces",
                spaces,
            },
            None,
        )
        .await
    }

    /// Feeds one signature-checked event.
    pub async fn ingest(
        &self,
        event: &RelayEvent,
        now_ms: i64,
    ) -> Result<IngestResult, EngineError> {
        self.call_as(
            "sync",
            &Ingest {
                action: "ingest",
                event,
                now_ms,
            },
            None,
        )
        .await
    }

    /// Reports whether a reassembled chunk group's payload was imported.
    pub async fn settle(
        &self,
        chunk_key: &str,
        imported: bool,
    ) -> Result<SettleResult, EngineError> {
        let payload = json!({"action": "settle", "chunkKey": chunk_key, "imported": imported});
        self.call_as("sync", &payload, None).await
    }

    /// A complete scan from `since` (unix s) could not resolve the remaining
    /// groups: drop those obligations instead of re-scanning forever.
    pub async fn retire(&self, since: i64) -> Result<SettleResult, EngineError> {
        self.call_as("sync", &json!({"action": "retire", "since": since}), None)
            .await
    }

    pub async fn session_state(&self) -> Result<SessionState, EngineError> {
        self.call_as("sync", &json!({"action": "state"}), None)
            .await
    }

    pub async fn session_close(&self) -> Result<bool, EngineError> {
        self.call_as("sync", &json!({"action": "close"}), None)
            .await
    }

    pub async fn outbox_enqueue(
        &self,
        pending: &OutboxPending,
    ) -> Result<EnqueueResult, EngineError> {
        self.call_as(
            "sync",
            &Enqueue {
                action: "outbox_enqueue",
                pending,
            },
            None,
        )
        .await
    }

    pub async fn outbox_next(&self, now_ms: i64) -> Result<OutboxNext, EngineError> {
        self.call_as(
            "sync",
            &json!({"action": "outbox_next", "nowMs": now_ms}),
            None,
        )
        .await
    }

    /// Reports a publish attempt; `sealed` once the host persisted the signed
    /// events of a sealed item. Returns the queue size.
    pub async fn outbox_result(
        &self,
        key: &str,
        ok: bool,
        sealed: bool,
        now_ms: i64,
    ) -> Result<i64, EngineError> {
        let payload = json!({"action": "outbox_result", "key": key, "ok": ok, "sealed": sealed, "nowMs": now_ms});
        let out: Pending = self.call_as("sync", &payload, None).await?;
        Ok(out.pending)
    }

    /// The transport is back: queued items skip their backoff. Returns the queue size.
    pub async fn outbox_wake(&self) -> Result<i64, EngineError> {
        let out: Pending = self
            .call_as("sync", &json!({"action": "outbox_wake"}), None)
            .await?;
        Ok(out.pending)
    }
}

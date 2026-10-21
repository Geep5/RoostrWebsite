//! `mutation`: the planner (`mutate.ts planMutation`, Engine.swift `mutation`).
//!
//! The planner reads states from the engine's query cache, which the host
//! brings up to date ([`Engine::query`] without a body) right before each
//! call. An object the params name that the cache lacks comes back in
//! [`MutationResponse::Missing`]; the host retries with its states of those
//! ids as `overlay` and names the rest `absent`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::values::Packed;
use crate::{Change, Engine, EngineError};

pub use crate::query::Applied;

/// One planner request. Entropy is explicit: the planner reads no clock and
/// draws no randomness.
#[derive(Debug, Clone, Copy)]
pub struct MutationRequest<'a> {
    /// `create`, `set_field`, `block_add`, `vanish`, `channel_create`, … (mutate.odin).
    pub action: &'a str,
    pub params: &'a Value,
    /// Unix ms stamped on every planned change.
    pub timestamp: i64,
    /// This device's key-derived author id.
    pub author: &'a str,
    /// Seed new object/block ids derive from (a fresh UUID per request).
    pub id_seed: &'a str,
    /// Space key id the plan writes under (rotations: the next one).
    pub key_id: i64,
    /// This install's stable device id: the directories' per-device count
    /// fields (`count:<type>:<device>`) are written under it ("" = the author).
    pub device: &'a str,
    /// Plan over exactly these states instead of the query cache.
    pub objects: Option<&'a [Value]>,
    /// States that win over the cache (the retry after `Missing`).
    pub overlay: &'a [Value],
    /// Ids the host looked up and does not hold.
    pub absent: &'a [String],
}

#[derive(Serialize)]
struct Payload<'a> {
    action: &'a str,
    params: Packed<'a>,
    timestamp: i64,
    author: &'a str,
    id_seed: &'a str,
    key_id: i64,
    #[serde(skip_serializing_if = "str::is_empty")]
    device: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    objects: Option<Vec<Packed<'a>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    overlay: Vec<Packed<'a>>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    absent: &'a [String],
}

/// A plan: the changes to address, commit and publish in order, the
/// action's result, and the vanish ledger work.
#[derive(Debug, Clone, PartialEq)]
pub struct MutationPlan {
    /// Unaddressed (`id` empty); the host sets `parentIds` to the object's
    /// heads ([`crate::Engine::object_heads`]) and encodes each.
    pub changes: Vec<Change>,
    pub result: Value,
    pub vanish_ids: Vec<String>,
    pub vanish_changes: Vec<Change>,
}

/// The planner's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum MutationResponse {
    Plan(MutationPlan),
    /// Ids the params name that the cache lacks: nothing was planned.
    Missing(Vec<String>),
}

#[derive(Deserialize)]
struct RawResponse {
    missing: Option<Vec<String>>,
    #[serde(default)]
    changes: Vec<Change>,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    vanish_ids: Vec<String>,
    #[serde(default)]
    vanish_changes: Vec<Change>,
}

impl Engine {
    pub async fn mutation(
        &self,
        request: &MutationRequest<'_>,
    ) -> Result<MutationResponse, EngineError> {
        let payload = Payload {
            action: request.action,
            params: Packed::new(request.params),
            timestamp: request.timestamp,
            author: request.author,
            id_seed: request.id_seed,
            key_id: request.key_id,
            device: request.device,
            objects: request
                .objects
                .map(|objects| objects.iter().map(Packed::new).collect()),
            overlay: request.overlay.iter().map(Packed::new).collect(),
            absent: request.absent,
        };
        let raw: RawResponse = self.call_as("mutation", &payload, None).await?;
        Ok(match raw.missing {
            Some(missing) => MutationResponse::Missing(missing),
            None => MutationResponse::Plan(MutationPlan {
                changes: raw.changes,
                result: raw.result,
                vanish_ids: raw.vanish_ids,
                vanish_changes: raw.vanish_changes,
            }),
        })
    }
}

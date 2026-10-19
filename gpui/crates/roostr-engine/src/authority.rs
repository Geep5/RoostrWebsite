//! Shared-space authority (`authority.odin`): the gate every received shared
//! change, checkpoint and base passes, and the vanish ledger.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::codec::to_base64;
use crate::{Change, Engine, EngineError, SharedProvenance};

/// What is being authorised.
#[derive(Debug, Clone, Copy)]
pub enum AuthorizeSubject<'a> {
    Change(&'a Change),
    /// A legacy checkpoint's protobuf: owner only.
    Checkpoint(&'a [u8]),
    /// A base's protobuf (epoch >= 1): owner only.
    Base(&'a [u8]),
}

/// The space as this device knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceAuthority {
    pub space_id: String,
    pub key_id: i64,
    /// Owner pubkey (hex); this identity's own pk for its own spaces.
    pub owner: String,
}

/// The gate's verdict.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Authorization {
    pub ok: bool,
    pub reason: String,
}

/// An object the vanish rule judges by its space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VanishCandidate {
    pub id: String,
    /// Its `channel` field; "" outside a space.
    pub channel: String,
}

/// An object to drop: in the ledger, or in a vanished or left space.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VanishedEntry {
    pub object_id: String,
    pub at: i64,
    /// The identity left the space (relay copies stay for everyone else).
    #[serde(default)]
    pub left: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthorizePayload<'a> {
    action: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    change: Option<&'a Change>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<String>,
    provenance: &'a SharedProvenance,
    space: &'a SpaceAuthority,
    trusted_space: Option<&'a Value>,
    existing: Option<&'a Value>,
}

#[derive(Serialize)]
struct VanishedPayload<'a> {
    action: &'a str,
    ledger: Option<&'a Value>,
    objects: &'a [VanishCandidate],
}

impl Engine {
    /// Authorises a shared-space subject received under `provenance`.
    /// `trusted_space` is the space object as held before this event,
    /// `existing` the subject object's current state.
    pub async fn authorize(
        &self,
        subject: AuthorizeSubject<'_>,
        provenance: &SharedProvenance,
        space: &SpaceAuthority,
        trusted_space: Option<&Value>,
        existing: Option<&Value>,
    ) -> Result<Authorization, EngineError> {
        let (action, change, checkpoint, base) = match subject {
            AuthorizeSubject::Change(change) => ("authorize", Some(change), None, None),
            AuthorizeSubject::Checkpoint(bytes) => {
                ("authorizeCheckpoint", None, Some(to_base64(bytes)), None)
            }
            AuthorizeSubject::Base(bytes) => ("authorizeBase", None, None, Some(to_base64(bytes))),
        };
        let payload = AuthorizePayload {
            action,
            change,
            checkpoint,
            base,
            provenance,
            space,
            trusted_space,
            existing,
        };
        self.call_as("sync", &payload, None).await
    }

    /// Every ledger entry (the `__vanished__` object's state) plus each
    /// candidate whose space vanished or was left: the complete set to drop.
    pub async fn vanished(
        &self,
        ledger: Option<&Value>,
        objects: &[VanishCandidate],
    ) -> Result<Vec<VanishedEntry>, EngineError> {
        let payload = VanishedPayload {
            action: "vanished",
            ledger,
            objects,
        };
        self.call_as("sync", &payload, None).await
    }
}

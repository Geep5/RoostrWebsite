//! Join links and requests (sync.ts join requests, `importSpaceInvite`,
//! `sendJoinRequest`; `src/lib/invite.ts`).
//!
//! Requests this identity received as a space administrator are kept in
//! [`LocalKv`] under `roostr-join-requests`, the Svelte app's key and JSON.

use roostr_nostr::nip59::JoinRequest as JoinRumor;
use roostr_nostr::relay::publish_any;
use roostr_nostr::spacekeys::SpaceKeyring;
use roostr_nostr::{Keys, Relay, bech32};
use serde::{Deserialize, Serialize};

use super::{DEFAULT_RELAYS, PUBLISH_TIMEOUT};
use crate::local::LocalKv;

const JOIN_REQUESTS: &str = "roostr-join-requests";

/// A request to join a space this device administers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    /// "spaceId/requesterHex": the stable dedupe key.
    pub key: String,
    pub space: String,
    pub space_name: String,
    pub requester: String,
    pub requester_npub: String,
    /// The requester's kind-0 name at request time, when the relays had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    /// Unix ms.
    pub at: i64,
}

/// A join link's payload (`/j` page).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinLink {
    pub space: String,
    /// The owner's npub (or hex).
    pub owner: String,
    /// Relays to knock on; empty = the defaults.
    pub relays: Vec<String>,
}

/// A space key delivered out of band (an invite link).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceInviteLink {
    pub space: String,
    /// The owner's npub (or hex).
    pub owner: String,
    pub key: String,
    pub key_id: i64,
}

/// `npub1…` or 64 hex → lowercase hex.
pub fn npub_to_hex(npub: &str) -> Option<String> {
    let text = npub.trim();
    if let Some(hex) = bech32::decode_hex32(text, "npub") {
        return Some(hex);
    }
    let lower = text.to_ascii_lowercase();
    (lower.len() == 64 && lower.bytes().all(|c| c.is_ascii_hexdigit())).then_some(lower)
}

pub fn list_join_requests(local: &dyn LocalKv) -> Vec<JoinRequest> {
    local
        .get(JOIN_REQUESTS)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_join_requests(local: &dyn LocalKv, requests: &[JoinRequest]) {
    match serde_json::to_string(requests) {
        Ok(raw) => local.set(JOIN_REQUESTS, Some(&raw)),
        Err(error) => log::warn!("[sync] join requests: {error}"),
    }
}

pub fn clear_join_request(local: &dyn LocalKv, key: &str) {
    let rest: Vec<JoinRequest> = list_join_requests(local)
        .into_iter()
        .filter(|r| r.key != key)
        .collect();
    write_join_requests(local, &rest);
}

/// Record (or refresh) a request; its key is derived from space and requester.
pub(super) fn record_join_request(local: &dyn LocalKv, mut request: JoinRequest) {
    request.key = format!("{}/{}", request.space, request.requester);
    let mut rest: Vec<JoinRequest> = list_join_requests(local)
        .into_iter()
        .filter(|r| r.key != request.key)
        .collect();
    rest.push(request);
    write_join_requests(local, &rest);
}

/// Send a join request to a space owner (the public join page).
pub async fn send_join_request(keys: &Keys, link: &JoinLink) -> anyhow::Result<()> {
    let owner =
        npub_to_hex(&link.owner).ok_or_else(|| anyhow::anyhow!("bad owner key in the link"))?;
    let urls: Vec<String> = if link.relays.is_empty() {
        DEFAULT_RELAYS.iter().map(|r| r.to_string()).collect()
    } else {
        link.relays.clone()
    };
    let relays: Vec<Relay> = urls.into_iter().map(Relay::new).collect();
    let wrap = JoinRumor {
        space: link.space.clone(),
    }
    .wrap(keys, &owner)?;
    let result = publish_any(&relays, &wrap, PUBLISH_TIMEOUT).await;
    for relay in &relays {
        relay.disconnect();
    }
    Ok(result?)
}

/// Accept an invite link: store the space key; the next shared-space
/// refresh backfills the space. Only the space's administrator may replace
/// a key already held.
pub fn import_space_invite(
    keyring: &dyn SpaceKeyring,
    local_pk: &str,
    invite: &SpaceInviteLink,
) -> anyhow::Result<bool> {
    let Some(owner) = npub_to_hex(&invite.owner) else {
        return Ok(false);
    };
    if invite.key_id < 1 {
        return Ok(false);
    }
    Ok(keyring.accept_invite(
        &invite.space,
        &owner,
        &invite.key,
        invite.key_id as u64,
        local_pk,
    )?)
}

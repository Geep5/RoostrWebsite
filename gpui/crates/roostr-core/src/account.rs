//! The identity a daemon-less backend keeps on this device (browser mode's
//! `keys.ts`, `engine/profile.ts`, sync.ts join requests). A daemon's
//! harness serves the same features over HTTP; the UI asks
//! [`crate::Backend::account`] first and falls back to the harness.

use anyhow::Result;
use serde_json::Value as Json;

use crate::invite::SpaceJoinLink;
use crate::io::{BoxFuture, MaybeSend, MaybeSync};

pub trait Account: MaybeSend + MaybeSync {
    /// Settings → "Reveal private key": `(nsec, hex)`.
    fn export_key(&self) -> Result<(String, String)>;
    /// "Log out on this device": refuses while changes are unpublished, then
    /// erases the local replica and forgets the key.
    fn logout(&self) -> BoxFuture<Result<()>>;
    /// The kind-0 profile (`{name, picture, …}`) from the relays.
    fn profile(&self) -> BoxFuture<Result<Json>>;
    /// Merge-publish a `{key: value}` patch ("" clears); the published profile.
    fn save_profile(&self, patch: Json) -> BoxFuture<Result<Json>>;
    /// Pending requests to join spaces this identity administers, in the
    /// harness's `/join-requests` row shape.
    fn join_requests(&self) -> BoxFuture<Result<Vec<Json>>>;
    fn clear_join_request(&self, key: &str) -> BoxFuture<Result<()>>;
    /// Gift-wrap this identity's join request to the link's owner.
    fn send_join_request(&self, link: &SpaceJoinLink) -> BoxFuture<Result<()>>;
    /// The `Authorization` header of a Blossom upload of the blob `sha256`
    /// (a BUD-11 kind-24242 event signed with this identity, files.ts
    /// `blossomSigner`).
    fn blossom_upload_authorization(&self, sha256: &str, name: &str) -> Result<String>;
}

/// The running backend's on-device account; `None` with a daemon.
pub fn account(cx: &gpui_kit::App) -> Option<std::sync::Arc<dyn Account>> {
    crate::Store::try_global(cx)?.read(cx).backend().account()
}

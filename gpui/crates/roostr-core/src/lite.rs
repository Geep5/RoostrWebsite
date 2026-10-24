//! How much of the vault a replica holds, and the lite replica's controls.
//!
//! A lite replica (the default on roostr.space and the iPhone) starts from
//! the directory objects the engine keeps (`__directory__`, and
//! `__directory__:<space>` per space): the sidebar renders from them, an
//! object is fetched from the relays when it is opened, and a space is
//! downloaded whole only when something needs all of it (a pinned query,
//! search, backlinks...). A full replica holds and follows everything. Any
//! replica answers [`crate::Backend::lite`]; a daemon does not.

use std::collections::HashMap;

use anyhow::Result;

use crate::io::{BoxFuture, MaybeSend, MaybeSync};

pub trait LiteControl: MaybeSend + MaybeSync {
    /// Objects per type key in `space_id`, as its directory counts them
    /// (every object, held or not). Empty when the directory is unknown.
    fn type_counts(&self, space_id: &str) -> HashMap<String, u64>;
    /// Download `space_id` whole and follow it from then on; progress shows
    /// in [`crate::LiteStatus::loading`]. Resolves once it is held.
    fn load_space(&self, space_id: &str) -> BoxFuture<Result<()>>;
    /// Make sure `ids` are held, fetching from the relays what is not. Ids
    /// no relay has are skipped (not an error).
    fn fetch(&self, ids: Vec<String>) -> BoxFuture<Result<()>>;
    /// How much of the vault this device holds, and what a full sync would
    /// download. Approximate; may ask the relays.
    fn estimate(&self) -> BoxFuture<Result<VaultEstimate>>;
    /// Settings → "Fully sync this device": from now on this device holds
    /// and follows the whole vault ([`LiteControl::switch_to_lite`] undoes it).
    fn sync_fully(&self) -> BoxFuture<Result<()>>;
    /// Settings → "Switch to Lite": a full replica drops its local copy and
    /// starts again lite - signed in, its space keys kept - holding only what
    /// is opened. Refused while changes are not sent yet. The caller reloads
    /// (the browser) once it resolves.
    fn switch_to_lite(&self) -> BoxFuture<Result<()>>;
}

/// [`LiteControl::estimate`]: approximate counts and sizes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VaultEstimate {
    /// The device holds the whole vault (a full replica).
    pub full: bool,
    pub held_objects: u64,
    /// Objects in the vault, from the directories' counts (at least `held_objects`).
    pub total_objects: u64,
    /// Relay events this device holds.
    pub held_events: u64,
    /// Relay events a full sync would still download (about).
    pub missing_events: u64,
    /// Bytes this device stores for the vault (about).
    pub held_bytes: u64,
    /// Bytes a full sync would download (about).
    pub missing_bytes: u64,
    /// Rough seconds a full sync takes.
    pub seconds: u64,
}

impl VaultEstimate {
    /// About how much of the vault is here, 0-100.
    pub fn percent(&self) -> u8 {
        if self.full {
            return 100;
        }
        let total = self.held_events + self.missing_events;
        let (part, whole) = if total > 0 {
            (self.held_events, total)
        } else {
            (self.held_objects, self.total_objects.max(self.held_objects))
        };
        if whole == 0 {
            return 100;
        }
        ((part as f64 / whole as f64 * 100.0).round() as u8).min(100)
    }
}

/// The running backend's vault controls; `None` with a daemon.
pub fn lite(cx: &gpui_kit::App) -> Option<std::sync::Arc<dyn LiteControl>> {
    crate::Store::try_global(cx)?.read(cx).backend().lite()
}

/// The running replica is lite (`None`: a daemon or a full replica).
pub fn status(cx: &gpui_kit::App) -> Option<crate::LiteStatus> {
    crate::Store::try_global(cx)?.read(cx).status.lite.clone()
}

//! Several relays at once (the `SimplePool` calls sync.ts makes) and the
//! publish pacing both hosts apply.

use std::collections::HashSet;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use web_time::Instant;

use super::{Relay, RelayError};
use crate::event::Event;
use crate::filter::Filter;
use crate::shared::Locked;
use crate::time;

/// Sends are paced one event per this interval (sync.ts
/// `PUBLISH_SPACING_MS`, SyncEngine.swift `publishSpacing`) so public
/// relays don't rate-limit the device.
pub const PUBLISH_SPACING: Duration = Duration::from_millis(120);

/// Spaces calls out: each [`Pacer::ready`] resolves at least `spacing`
/// after the previous one, however many tasks share the pacer.
pub struct Pacer {
    spacing: Duration,
    next: Locked<Option<Instant>>,
}

impl Pacer {
    pub fn new(spacing: Duration) -> Self {
        Self {
            spacing,
            next: Locked::new(None),
        }
    }

    /// Wait for this caller's slot.
    pub async fn ready(&self) {
        let now = Instant::now();
        let at = self.next.with(|next| {
            let at = next.filter(|t| *t > now).unwrap_or(now);
            *next = Some(at + self.spacing);
            at
        });
        if at > now {
            time::sleep(at - now).await;
        }
    }
}

impl Default for Pacer {
    /// Paced at [`PUBLISH_SPACING`].
    fn default() -> Self {
        Self::new(PUBLISH_SPACING)
    }
}

/// `Promise.any(pool.publish(relays, event))`: done as soon as one relay
/// accepts; the error of the last relay to fail when none does.
pub async fn publish_any(
    relays: &[Relay],
    event: &Event,
    timeout: Duration,
) -> Result<(), RelayError> {
    let mut pending: FuturesUnordered<_> = relays
        .iter()
        .map(|relay| relay.publish(event, timeout))
        .collect();
    let mut last = RelayError::Transport("no relays".into());
    while let Some(result) = pending.next().await {
        match result {
            Ok(()) => return Ok(()),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// `pool.querySync(relays, filter)`: every relay queried at once, events
/// deduplicated by id in arrival order. Fails only when no relay answered.
pub async fn query_many(
    relays: &[Relay],
    filters: &[Filter],
    timeout: Duration,
) -> Result<Vec<Event>, RelayError> {
    let mut pending: FuturesUnordered<_> = relays
        .iter()
        .map(|relay| relay.query(filters.to_vec(), timeout))
        .collect();
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    let mut answered = false;
    let mut last = RelayError::Transport("no relays".into());
    while let Some(result) = pending.next().await {
        match result {
            Ok(batch) => {
                answered = true;
                events.extend(batch.into_iter().filter(|e| seen.insert(e.id.clone())));
            }
            Err(error) => last = error,
        }
    }
    if answered { Ok(events) } else { Err(last) }
}

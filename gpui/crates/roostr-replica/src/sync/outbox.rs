//! Durable publishing (sync.ts `publish`/`runPublishQueue`/`publishOnce`,
//! SyncEngine.swift's outbox).
//!
//! A local write never depends on the network: the obligation is stored
//! first, then offered to the engine outbox, which owns order, dedupe, the
//! rotated-key rule and backoff, and seals a change on its first attempt.
//! This side signs the sealed parts, persists the exact signed events
//! BEFORE sending (retries and restarts resend them byte for byte), sends
//! one event per 120 ms to the first relay that accepts, records the
//! events as held and marks the obligation published.

use std::sync::Arc;
use std::time::Duration;

use roostr_engine::{OutboxItem, OutboxPending};
use roostr_nostr::relay::publish_any;
use roostr_nostr::{Event, EventTemplate, kinds};
use roostr_replica_store::PendingPublish;

use super::{Inner, PUBLISH_SPACING, PUBLISH_TIMEOUT, now_ms, now_s, short};

impl Inner {
    pub(super) async fn publish(
        self: Arc<Self>,
        bytes: Vec<u8>,
        change_id: String,
        object_id: String,
        base: Option<String>,
    ) -> anyhow::Result<()> {
        let mut candidates = vec![PendingPublish {
            key: change_id.clone(),
            object_id: object_id.clone(),
            change_id: change_id.clone(),
            bytes: bytes.clone(),
            space_id: None,
            key_id: None,
            base: base.clone(),
            events: None,
        }];
        if let Some(space) = self.space_for(&object_id) {
            candidates.push(PendingPublish {
                key: format!("{}/{}/{change_id}", space.space_id, space.key_id),
                object_id,
                change_id,
                bytes,
                space_id: Some(space.space_id),
                key_id: Some(space.key_id),
                base,
                events: None,
            });
        }
        let mut owed = Vec::new();
        for item in candidates {
            if self.store.is_published(item.key.clone()).await? {
                continue;
            }
            // An entry already saved (by the backend's atomic commit, or
            // sealed by an earlier attempt) is never clobbered.
            match self.store.pending(item.key.clone()).await? {
                Some(saved) => owed.push(saved),
                None => {
                    self.store.save_pending(item.clone()).await?;
                    owed.push(item);
                }
            }
        }
        if owed.is_empty() {
            return Ok(());
        }
        // Stored, unsent on failure: the next start() re-offers it.
        if let Err(error) = self.ensure_session().await {
            self.emit_error(format!("publish deferred: {}", short(error, 120)));
            return Ok(());
        }
        for item in &owed {
            self.clone().offer_to_outbox(item).await;
        }
        Ok(())
    }

    /// Hand a durable obligation to the engine outbox; the store record
    /// stays either way.
    pub(super) async fn offer_to_outbox(self: Arc<Self>, pending: &PendingPublish) {
        if !self.session_open() || self.stopped() {
            return; // the next start() re-offers from the store
        }
        let result = self
            .engine
            .outbox_enqueue(&OutboxPending {
                key: pending.key.clone(),
                object_id: pending.object_id.clone(),
                change_id: pending.change_id.clone(),
                bytes: pending.bytes.clone(),
                space_id: pending.space_id.clone(),
                key_id: pending.key_id,
                base: pending.base.clone(),
                has_events: pending.events.is_some(),
            })
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.emit_error(format!("publish deferred: {}", short(error, 120)));
                return;
            }
        };
        self.state.lock().pending_count = result.pending;
        if !result.queued {
            return;
        }
        self.emit_live_status();
        self.ensure_publish_queue();
    }

    fn ensure_publish_queue(self: &Arc<Self>) {
        {
            let mut st = self.state.lock();
            if st.queue_running || st.stopped {
                return;
            }
            st.queue_running = true;
        }
        let this = self.clone();
        crate::rt::spawn(async move { this.run_publish_queue().await });
    }

    /// The transport is back: queued items skip their backoff.
    pub(super) async fn wake_outbox(self: &Arc<Self>) {
        if !self.session_open() {
            return;
        }
        if let Ok(pending) = self.engine.outbox_wake().await {
            self.state.lock().pending_count = pending;
            if pending > 0 {
                self.ensure_publish_queue();
            }
        }
    }

    /// Drains the engine outbox: one event per [`PUBLISH_SPACING`],
    /// outcomes reported back so the engine applies backoff.
    async fn run_publish_queue(self: Arc<Self>) {
        loop {
            while !self.stopped() && self.session_open() {
                let next = match self.engine.outbox_next(now_ms()).await {
                    Ok(next) => next,
                    Err(error) => {
                        // An unsealable change: the engine backed it off; keep draining.
                        self.emit_error(format!("publish rejected: {}", short(error, 120)));
                        roostr_nostr::sleep(PUBLISH_SPACING).await;
                        continue;
                    }
                };
                self.state.lock().pending_count = next.pending;
                let Some(item) = next.item else {
                    if next.pending == 0 {
                        break;
                    }
                    let wait = next.wait_ms.clamp(50, 500) as u64;
                    roostr_nostr::sleep(Duration::from_millis(wait)).await;
                    continue;
                };
                let (ok, sealed) = self.publish_once(&item).await;
                if self.session_open() {
                    match self
                        .engine
                        .outbox_result(&item.key, ok, sealed, now_ms())
                        .await
                    {
                        Ok(pending) => self.state.lock().pending_count = pending,
                        Err(error) => log::warn!("[sync] outbox result: {error}"),
                    }
                }
                roostr_nostr::sleep(PUBLISH_SPACING).await;
            }
            self.state.lock().queue_running = false;
            // An offer that raced the drain's last look must not be stranded.
            let owed = !self.stopped()
                && self.session_open()
                && self
                    .engine
                    .session_state()
                    .await
                    .is_ok_and(|state| state.pending > 0);
            if !owed {
                break;
            }
            let mut st = self.state.lock();
            if st.queue_running {
                break;
            }
            st.queue_running = true;
        }
        if !self.stopped() {
            self.emit_live_status();
        }
    }

    /// Sign the engine's sealed parts on the first attempt, persist the
    /// exact signed events before sending, then send. Returns (ok, sealed).
    async fn publish_once(self: &Arc<Self>, item: &OutboxItem) -> (bool, bool) {
        let mut sealed = false;
        let result: anyhow::Result<()> = async {
            if self.store.is_published(item.key.clone()).await? {
                return Ok(());
            }
            let Some(mut pending) = self.store.pending(item.key.clone()).await? else {
                self.emit_error(format!(
                    "publish skipped: no stored obligation for {}",
                    item.key
                ));
                return Ok(());
            };
            if pending.events.is_none() {
                let parts = item
                    .sealed
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("no signed events to resend"))?;
                let created_at = now_s() as u64;
                let events = parts
                    .parts
                    .iter()
                    .map(|part| {
                        let event = self.keys.sign(EventTemplate {
                            kind: kinds::CHANGE,
                            created_at,
                            tags: part.tags.clone(),
                            content: part.content.clone(),
                        });
                        serde_json::to_value(event)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                pending.events = Some(events);
            }
            // Persist BEFORE sending.
            self.store.save_pending(pending.clone()).await?;
            sealed = true;
            let events: Vec<Event> = pending
                .events
                .iter()
                .flatten()
                .map(|value| serde_json::from_value(value.clone()))
                .collect::<Result<_, _>>()?;
            for event in &events {
                publish_any(&self.relays, event, PUBLISH_TIMEOUT).await?;
                if events.len() > 1 {
                    roostr_nostr::sleep(PUBLISH_SPACING).await;
                }
            }
            // Their ids are known: the next reconcile need not fetch our own writes back.
            let rows = events.iter().flat_map(|e| self.held_rows(e)).collect();
            self.record_held(rows).await?;
            self.store.mark_published(item.key.clone()).await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => (true, sealed),
            Err(error) => {
                self.emit_error(format!(
                    "publish rejected (attempt {}): {}",
                    item.attempts + 1,
                    short(error, 120)
                ));
                (false, sealed)
            }
        }
    }
}

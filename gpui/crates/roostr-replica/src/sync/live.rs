//! Live subscriptions since the cursor (sync.ts `subscribeLive`,
//! `handleLiveEvent`, `watchdog`; SyncEngine.swift `liveLoop`).
//!
//! One subscription per relay carries this identity's DAG events, every
//! installed space's `#h` stream (DAG + h-deletions) - in a lite session
//! only the streams it holds whole, plus each followed object by its `h`
//! tag - and gift wraps addressed to us. [`roostr_nostr::relay::live`]
//! resubscribes after a failure with the backoff (2 s doubling to 60 s),
//! rebuilding the filters so `since` follows the cursor; a reconnect runs a
//! catch-up pass (what
//! the dead socket missed) and wakes the outbox. Every event funnels into
//! one worker, so live handling is serialized like sync.ts' `liveChain`.

use std::sync::{Arc, Weak};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::future::{AbortHandle, Abortable};
use roostr_nostr::nip59::WRAP_LOOKBACK_S;
use roostr_nostr::relay::{LiveItem, live, query_many};
use roostr_nostr::{Event, Filter, kinds};

use super::{
    BASE_HASH_BATCH, Inner, PAGE_TIMEOUT, WATCHDOG_PERIOD, describe_event, now_s, short, short_id,
};

/// Remembered live event ids (relays deliver the same event each).
const LIVE_SEEN_CAP: usize = 20_000;

pub(crate) enum LiveMsg {
    Event(Event),
    /// A relay's subscription came back after a failure.
    Reconnected,
}

impl Inner {
    /// The DAG streams this session follows, unbounded: this identity's own
    /// events and every installed space's `#h` stream (`space_kinds`), or in
    /// a lite session the streams it holds whole plus each followed object
    /// by its `h` tag.
    fn scope_filters(&self, space_kinds: &[u32]) -> Vec<Filter> {
        let (personal, tags) = self.whole_streams();
        let mut filters = Vec::new();
        if personal {
            filters.push(Filter::new().kinds(kinds::DAG).authors([self.pk.clone()]));
        }
        if !tags.is_empty() {
            filters.push(
                Filter::new()
                    .kinds(space_kinds.iter().copied())
                    .tag('h', tags),
            );
        }
        for chunk in self.followed_tags().chunks(BASE_HASH_BATCH) {
            filters.push(
                Filter::new()
                    .kinds(kinds::DAG)
                    .tag('h', chunk.iter().cloned()),
            );
        }
        filters
    }

    fn live_filters(&self) -> Vec<Filter> {
        let since = self.live_since().max(0) as u64;
        let mut filters: Vec<Filter> = self
            .scope_filters(&kinds::SPACE)
            .into_iter()
            .map(|filter| filter.since(since))
            .collect();
        filters.push(
            Filter::new()
                .kinds([kinds::GIFT_WRAP])
                .tag('p', [self.pk.clone()])
                .since((now_s() as u64).saturating_sub(WRAP_LOOKBACK_S)),
        );
        filters
    }

    /// (Re)subscribe every relay with filters from the current cursor and
    /// space set.
    pub(super) fn subscribe_live(self: &Arc<Self>) {
        let mut handles = Vec::new();
        for relay in &self.relays {
            let (handle, registration) = AbortHandle::new_pair();
            handles.push(handle);
            let weak: Weak<Inner> = Arc::downgrade(self);
            let tx = self.live_tx.clone();
            let trace_url = relay.url().to_string();
            let stream = live(relay.clone(), move || {
                let filters = weak
                    .upgrade()
                    .map(|this| this.live_filters())
                    .unwrap_or_default();
                roostr_core::sync_trace!(
                    "live subscribe {trace_url}: {}",
                    serde_json::to_string(&filters).unwrap_or_default()
                );
                filters
            });
            let url = relay.url().to_string();
            let task = async move {
                let mut stream = std::pin::pin!(stream);
                while let Some(item) = stream.next().await {
                    let msg = match item {
                        LiveItem::Event(event) => LiveMsg::Event(event),
                        LiveItem::Up { reconnected } => {
                            roostr_core::sync_trace!("live {url}: up (reconnected: {reconnected})");
                            if !reconnected {
                                continue;
                            }
                            LiveMsg::Reconnected
                        }
                        LiveItem::Down(error) => {
                            roostr_core::sync_trace!("live {url}: down: {error}");
                            log::debug!("[sync] live {url}: {error}");
                            continue;
                        }
                    };
                    if tx.unbounded_send(msg).is_err() {
                        return;
                    }
                }
            };
            crate::rt::spawn(async move {
                let _ = Abortable::new(task, registration).await;
            });
        }
        let previous = {
            let mut st = self.state.lock();
            st.live_up = !st.stopped;
            if st.stopped {
                Vec::new()
            } else {
                std::mem::replace(&mut st.live_tasks, handles.clone())
            }
        };
        for handle in previous {
            handle.abort();
        }
        if self.stopped() {
            for handle in handles {
                handle.abort();
            }
        }
    }

    /// The single consumer of every live subscription.
    pub(super) fn spawn_live_worker(self: Arc<Self>) {
        let Some(rx) = self.live_rx.lock().take() else {
            return;
        };
        crate::rt::spawn(self.live_worker(rx));
    }

    async fn live_worker(self: Arc<Self>, mut rx: UnboundedReceiver<LiveMsg>) {
        while let Some(msg) = rx.next().await {
            if self.stopped() {
                return;
            }
            match msg {
                LiveMsg::Event(event) => {
                    let fresh = {
                        let mut st = self.state.lock();
                        if st.live_seen.len() >= LIVE_SEEN_CAP {
                            st.live_seen.clear();
                        }
                        st.live_seen.insert(event.id.clone())
                    };
                    if !fresh {
                        roostr_core::sync_trace!(
                            "live event {} again (already seen)",
                            short_id(&event.id)
                        );
                        continue;
                    }
                    roostr_core::sync_trace!("live event {}", describe_event(&event, &self.pk));
                    if event.kind == kinds::GIFT_WRAP {
                        self.handle_wrap(&event).await;
                    } else if let Err(error) = self.handle_live_event(event).await {
                        log::warn!("[sync] live event: {error}");
                    }
                }
                LiveMsg::Reconnected => self.clone().after_reconnect(),
            }
        }
    }

    /// A subscription outlived a dead socket: catch up on what it missed
    /// (cursor-bounded pass + gift wraps) and wake the outbox.
    fn after_reconnect(self: Arc<Self>) {
        {
            let mut st = self.state.lock();
            if st.stopped || st.resuming || st.bootstrapping {
                return;
            }
            st.resuming = true;
            st.catching_up += 1;
        }
        crate::rt::spawn(async move {
            self.emit_live_status();
            let result = self.catchup_from_store().await;
            {
                let mut st = self.state.lock();
                st.catching_up -= 1;
                st.resuming = false;
            }
            if let Err(error) = result
                && !self.stopped()
            {
                self.emit_error(error.to_string());
            }
            self.wake_outbox().await;
            if !self.stopped() {
                self.emit_live_status();
            }
        });
    }

    pub(super) async fn handle_live_event(self: &Arc<Self>, event: Event) -> anyhow::Result<()> {
        self.state.lock().active_live_events += 1;
        let result = async {
            match self.ingest_event(&event).await? {
                Some(item) => self.import_batch(vec![item], false).await?,
                None if event.kind == kinds::BASE => {
                    // A live split base: its parts went out first, possibly before the cursor.
                    if let Some(gid) = event.tag("c")
                        && self.state.lock().open_base_groups.remove(gid)
                    {
                        let this = self.clone();
                        let gid = gid.to_string();
                        crate::rt::spawn(async move { this.fetch_live_base_parts(gid).await });
                    }
                }
                None => {}
            }
            anyhow::Ok(())
        }
        .await;
        if result.is_err() {
            self.record_replay_fault(1).await;
        }
        self.state.lock().active_live_events -= 1;
        let persisted = self.persist_cursor().await;
        result.and(persisted)
    }

    async fn fetch_live_base_parts(self: Arc<Self>, gid: String) {
        let filter = Filter::new().kinds([kinds::BASE_PART]).tag('c', [gid]);
        if let Ok(parts) = query_many(&self.relays, &[filter], PAGE_TIMEOUT).await {
            for part in parts {
                // The next history pass fetches them otherwise.
                let _ = self.live_tx.unbounded_send(LiveMsg::Event(part));
            }
        }
    }

    pub(super) fn spawn_watchdog(self: Arc<Self>) {
        let weak = Arc::downgrade(&self);
        drop(self);
        crate::rt::spawn(async move {
            loop {
                roostr_nostr::sleep(WATCHDOG_PERIOD).await;
                let Some(this) = weak.upgrade() else { return };
                if this.stopped() {
                    return;
                }
                this.watchdog().await;
            }
        });
    }

    /// Deafness watchdog: compare the relays' head against the cursor; when
    /// behind (or unanswered), say "catching up", recover, go live again.
    async fn watchdog(self: &Arc<Self>) {
        {
            let mut st = self.state.lock();
            if st.stopped || st.watchdog_busy || st.bootstrapping || st.resuming {
                return;
            }
            st.watchdog_busy = true;
        }
        let result = self.watchdog_pass().await;
        let mut st = self.state.lock();
        st.watchdog_busy = false;
        if let Err(error) = result {
            st.history_complete = false;
            drop(st);
            self.emit_error(error.to_string());
        }
    }

    async fn watchdog_pass(self: &Arc<Self>) -> anyhow::Result<()> {
        let groups = if self.session_open() {
            self.engine.session_state().await?.groups
        } else {
            0
        };
        let incomplete = {
            let st = self.state.lock();
            !st.history_complete || groups > 0 || st.discarded_chunk_floor != i64::MAX
        };
        if incomplete {
            let bootstrapped = self.store.bootstrapped().await?;
            let (since, floor) = if bootstrapped {
                (self.store.cursor().await?, None)
            } else {
                (1, self.store.bootstrap_floor().await?)
            };
            if self.backfill(since, floor).await? {
                self.mark_bootstrapped().await;
            }
            self.subscribe_live();
            self.emit_live_status();
            return Ok(());
        }
        let filters: Vec<Filter> = self
            .scope_filters(&kinds::DAG)
            .into_iter()
            .map(|filter| filter.limit(1))
            .collect();
        if filters.is_empty() {
            return Ok(());
        }
        let head = match query_many(&self.relays, &filters, PAGE_TIMEOUT).await {
            Ok(events) => events
                .iter()
                .map(|e| e.created_at as i64)
                .max()
                .unwrap_or(0),
            Err(error) => {
                if self.stopped() {
                    return Ok(());
                }
                self.emit_backfill(Some(format!(
                    "relay unresponsive - reconnecting ({})",
                    short(error, 80)
                )));
                for relay in &self.relays {
                    relay.disconnect();
                }
                let cursor = self.state.lock().cursor;
                self.catchup(cursor + 1, None).await?;
                self.subscribe_live();
                self.emit_live_status();
                return Ok(());
            }
        };
        let cursor = self.state.lock().cursor;
        if head > cursor {
            self.emit_backfill(Some("catching up".into()));
            self.catchup(cursor + 1, None).await?;
            self.subscribe_live();
            self.emit_live_status();
        }
        Ok(())
    }
}

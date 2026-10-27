//! The history pass (sync.ts `backfill`/`walkHistory`, SyncEngine.swift
//! `walkHistory`).
//!
//! Per relay and stream: NIP-77 against the held set, every needed id
//! fetched with `REQ {ids}`; a relay that refuses NEG-OPEN is walked
//! backwards page by page via `until` instead (paced against its own cost).
//! Order (`docs/state-sync.md`): bases (31078) and their 1080 parts by
//! `#c`; legacy 1079 checkpoints; then deltas. A stream holding no base
//! reads its whole 1078 history (dual read); one holding bases reads the
//! live deltas of the held current bases (`#b`), the vanish ledger, and a
//! 1078 window bounded by ITS newest base's created_at minus one day -
//! never another stream's floor. Older history the window leaves out is
//! swept once per stream: every event the relay holds there minus those of
//! objects holding a base (ids only, by NIP-77), so an object no computer
//! has based yet is never cut off.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use roostr_nostr::negentropy::NegentropyStorage;
use roostr_nostr::relay::NEG_TIMEOUT;
use roostr_nostr::{Event, Filter, RelayError, kinds};
use roostr_replica_store::{HeldItem, HeldQuery};

use super::{
    BASE_HASH_BATCH, BASE_LOOKBACK_S, CHANGE_GROUP_GRACE_S, CHECKPOINT_FETCH_BATCH, FETCH_BATCH,
    Inner, MIN_PAGE_SPACING_MS, ORPHAN_BATCH, PAGE_LIMIT, PAGE_SPACING_MS, PAGE_TIMEOUT,
    UNBASED_SWEPT, VANISH_LOG_ID, now_ms, now_s, short,
};

/// One relay stream filter, with the store scope of its held events.
#[derive(Clone)]
struct StreamFilter {
    scope: String,
    filter: Filter,
}

/// How one stream on one relay was covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Complete,
    Incomplete,
    /// The relay does not reconcile this filter: walk it instead.
    Unsupported,
}

/// The ids one relay holds of objects holding a base, below the window.
enum Covered {
    Ids(HashSet<String>),
    /// The relay does not reconcile: walk the range instead.
    Unsupported,
    Failed,
}

/// A blinded `h` tag (`core/wire.odin` blind): the first 16 hex of
/// sha256(prefix ‖ utf8(id)), the prefix being the personal secret's bytes
/// or a space key's hex text.
pub(crate) fn blind(prefix: &[u8], id: &str) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::new()
        .chain_update(prefix)
        .chain_update(id.as_bytes())
        .finalize();
    hex::encode(&digest[..8])
}

/// What one history pass (or one lite fetch) shares across its concurrent covers.
pub(super) struct Pass {
    seen: Mutex<HashSet<String>>,
    /// Ids claimed by some stream's fetch: fetched once per pass.
    claimed: Mutex<HashSet<String>>,
    pub(super) complete: AtomicBool,
    /// Some relay/filter fell back to the paged walk.
    walked: AtomicBool,
    covered_until: Mutex<Option<i64>>,
    since: i64,
    /// The `h` tags of every event this pass saw, when asked
    /// ([`Pass::watching_tags`]): which blinded tag an object fetched by id
    /// answered to.
    h_tags: Option<Mutex<HashSet<String>>>,
}

impl Pass {
    pub(super) fn new(relays: usize, since: i64, resume_until: Option<i64>) -> Self {
        Self {
            seen: Mutex::new(HashSet::new()),
            claimed: Mutex::new(HashSet::new()),
            complete: AtomicBool::new(relays > 0),
            walked: AtomicBool::new(false),
            covered_until: Mutex::new(resume_until),
            since,
            h_tags: None,
        }
    }

    /// Also collect the `h` tags of the events this pass imports.
    pub(super) fn watching_tags(mut self) -> Self {
        self.h_tags = Some(Mutex::new(HashSet::new()));
        self
    }

    /// The `h` tags seen so far (empty unless [`Pass::watching_tags`]).
    pub(super) fn tags_seen(&self) -> HashSet<String> {
        self.h_tags
            .as_ref()
            .map(|tags| tags.lock().clone())
            .unwrap_or_default()
    }
}

fn pace(started: i64) -> Duration {
    let took = (now_ms() - started).max(0) as u64;
    Duration::from_millis(PAGE_SPACING_MS.min(MIN_PAGE_SPACING_MS.max(took)))
}

impl Inner {
    /// One history pass at a time; true only when EVERY relay covered
    /// every stream. A reconcile always covers the whole stream;
    /// `since`/`resume_until` only shape the fallback walk.
    pub(super) async fn backfill(
        self: &Arc<Self>,
        since: i64,
        resume_until: Option<i64>,
    ) -> anyhow::Result<bool> {
        let _guard = self.backfill_lock.lock().await;
        self.state.lock().pass = Some((0, 0));
        let result = self.walk_history(since, resume_until).await;
        if result.is_ok() && !self.stopped() {
            self.emit_live_status(); // the pass's final count, unthrottled
        }
        // Ended: the status line stops counting, whoever called.
        self.state.lock().pass = None;
        match result {
            Ok(complete) => {
                if !self.stopped() {
                    self.emit_live_status();
                }
                Ok(complete)
            }
            Err(error) => {
                self.record_replay_fault(1).await;
                let _ = self.persist_cursor().await;
                Err(error)
            }
        }
    }

    async fn walk_history(
        self: &Arc<Self>,
        since: i64,
        resume_until: Option<i64>,
    ) -> anyhow::Result<bool> {
        self.state.lock().history_complete = false;
        self.ensure_session().await?;
        // A full-history walk persists its progress page by page; the floor
        // only means something while bootstrapping (never in a lite session).
        let lite = self.state.lock().lite.is_some();
        let track_floor = since <= 1 && !lite;
        // An unresolved group may be missing fragments older than any
        // observed part: repair scans need full history.
        let (since, checkpoint) = {
            let mut st = self.state.lock();
            let base = if st.replay_groups.is_empty() {
                since
            } else {
                0
            };
            let since = base.min(st.discarded_chunk_floor);
            st.discarded_chunk_floor = st.discarded_chunk_floor.min(since);
            (since, st.replay_fault_generation)
        };
        self.persist_cursor().await?;
        let pass = Pass::new(self.relays.len(), since, resume_until);

        // Every stream, or in a lite session the ones it holds whole; the
        // objects it follows elsewhere are caught up below.
        let (personal, tags) = self.whole_streams();
        let pk = self.pk.clone();
        let mut base_filters = Vec::new();
        let mut checkpoint_filters = Vec::new();
        let mut change_filters = Vec::new();
        if personal {
            base_filters.push(StreamFilter {
                scope: String::new(),
                filter: Filter::new().kinds([kinds::BASE]).authors([pk.clone()]),
            });
            checkpoint_filters.push(StreamFilter {
                scope: String::new(),
                filter: Filter::new()
                    .kinds([kinds::CHECKPOINT])
                    .authors([pk.clone()]),
            });
            change_filters.push(StreamFilter {
                scope: String::new(),
                filter: Filter::new().kinds([kinds::CHANGE]).authors([pk.clone()]),
            });
        }
        for tag in tags {
            base_filters.push(StreamFilter {
                scope: tag.clone(),
                filter: Filter::new().kinds([kinds::BASE]).tag('h', [tag.clone()]),
            });
            checkpoint_filters.push(StreamFilter {
                scope: tag.clone(),
                filter: Filter::new()
                    .kinds([kinds::CHECKPOINT, kinds::DELETION])
                    .tag('h', [tag.clone()]),
            });
            change_filters.push(StreamFilter {
                scope: tag.clone(),
                filter: Filter::new().kinds([kinds::CHANGE]).tag('h', [tag]),
            });
        }

        // Bases first: every object renders from its base as soon as it lands.
        self.cover(&pass, &base_filters, None, false).await?;
        // A split base's 31078 names its parts' group: fetch what is missing.
        self.fetch_base_parts(&pass).await?;
        self.cover(&pass, &checkpoint_filters, None, false).await?;
        self.state.lock().checkpoints_loaded = true;
        self.emit_live_status();

        // Per stream, never global: one stream's newest base says nothing
        // about another's unbased objects.
        let newest = self.store.newest_base_by_stream().await?;
        let floor_of = |scope: &str| {
            newest
                .get(scope)
                .map(|at| (at / 1000 - BASE_LOOKBACK_S).max(1))
        };
        let bounded: Vec<&StreamFilter> = change_filters
            .iter()
            .filter(|sf| floor_of(&sf.scope).is_some())
            .collect();
        if !bounded.is_empty() {
            // A bounded stream's live deltas of every held current base, and
            // the vanish ledger's whole stream when the personal one is bounded.
            let mut hashes: Vec<String> = self
                .store
                .current_bases()
                .await?
                .into_values()
                .map(|base| base.hash)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            hashes.sort();
            let mut delta_filters = Vec::new();
            for chunk in hashes.chunks(BASE_HASH_BATCH) {
                for sf in &bounded {
                    delta_filters.push(StreamFilter {
                        scope: sf.scope.clone(),
                        filter: sf.filter.clone().tag('b', chunk.iter().cloned()),
                    });
                }
            }
            if personal && floor_of("").is_some() {
                let ledger = self
                    .engine
                    .blind_personal(&self.secret_hex, VANISH_LOG_ID)
                    .await?;
                delta_filters.push(StreamFilter {
                    scope: String::new(),
                    filter: Filter::new()
                        .kinds([kinds::CHANGE])
                        .authors([pk.clone()])
                        .tag('h', [ledger]),
                });
            }
            self.cover(&pass, &delta_filters, None, false).await?;
        }
        // Then each stream's 1078s: since its floor when it holds bases,
        // else whole - a pre-migration vault syncs exactly as before.
        let changes: Vec<StreamFilter> = change_filters
            .iter()
            .map(|sf| match floor_of(&sf.scope) {
                None => sf.clone(),
                Some(floor) => StreamFilter {
                    scope: sf.scope.clone(),
                    filter: sf.filter.clone().since(floor as u64),
                },
            })
            .collect();
        self.cover(&pass, &changes, resume_until, track_floor)
            .await?;
        // Then what each bounded stream holds below its window.
        let floors: HashMap<String, i64> = change_filters
            .iter()
            .filter_map(|sf| Some((sf.scope.clone(), floor_of(&sf.scope)?)))
            .collect();
        self.sweep_unbased(&pass, &change_filters, &floors).await?;
        // A lite session: the objects it follows outside those streams,
        // everything new since its cursor.
        let followed = if lite {
            Some(self.walk_followed(&pass).await?)
        } else {
            None
        };

        // This device's writes - unpublished, or published and not yet
        // proven folded - whose object moved to a newer base meanwhile
        // rebase (or turn out folded) now.
        let mut owed: Vec<String> = self
            .store
            .pending_publishes()
            .await?
            .into_iter()
            .map(|p| p.object_id)
            .collect();
        owed.extend(self.store.own_changes().await?.into_values());
        owed.sort();
        owed.dedup();
        // A few objects per hold of the import lock: live events (a change
        // to the open object) land between them, not after all of them.
        for objects in owed.chunks(ORPHAN_BATCH) {
            let _import = self.import_lock.lock().await;
            self.resolve_orphans(objects.iter().cloned()).await?;
        }
        let covered = match followed {
            // What the walk covered for sure: the followed objects since then.
            Some(since) => since,
            None if pass.walked.load(Ordering::Relaxed) => since,
            None => 0,
        };
        // Events mid-flight are not a fault: let them land, then judge.
        self.drain_inflight().await;
        let complete = pass.complete.load(Ordering::Relaxed);
        // Every relay answered for every filter and nothing faulted: a
        // checkpoint group still open is missing parts no relay holds, so the
        // engine retires it (a change group stays until a covering repair).
        let clean = {
            let st = self.state.lock();
            complete && !st.stopped && st.replay_fault_generation == checkpoint
        };
        if clean && self.session_open() {
            // Every stream reconciled whole (no paged walk, no lite
            // follow-up): a change group still open has no other part on
            // any relay - compaction folded it - unless it is being
            // published right now.
            let reconciled = followed.is_none() && !pass.walked.load(Ordering::Relaxed);
            let result = self
                .engine
                .retire(
                    if covered <= 1 { 0 } else { covered },
                    reconciled.then(|| now_s() - CHANGE_GROUP_GRACE_S),
                )
                .await?;
            if let Some(groups) = result.replay_groups {
                self.state.lock().replay_groups = groups.into_iter().collect();
            }
        }
        let history_complete = {
            let mut st = self.state.lock();
            st.history_complete = complete
                && !st.stopped
                && st.replay_groups.is_empty()
                && st.replay_fault_generation == checkpoint
                && covered <= st.discarded_chunk_floor;
            if st.history_complete {
                st.discarded_chunk_floor = i64::MAX;
            }
            st.history_complete
        };
        // A lite pass that fell short redoes its followed objects from where
        // this one started, not the vault from the beginning.
        let fault_at = followed.unwrap_or(since);
        if history_complete {
            if track_floor {
                self.store.set_bootstrap_floor(None).await?;
            }
        } else {
            self.record_replay_fault(fault_at).await;
        }
        self.persist_cursor().await?;
        // Restore the obligation if a new group or fault appeared while the
        // repaired cursor was committing.
        self.drain_inflight().await;
        let regressed = {
            let st = self.state.lock();
            !st.history_complete
                || st.stopped
                || !st.replay_groups.is_empty()
                || st.replay_fault_generation != checkpoint
        };
        if history_complete && regressed {
            self.record_replay_fault(fault_at).await;
            self.persist_cursor().await?;
        }
        let (stopped, complete, reachable) = {
            let st = self.state.lock();
            (st.stopped, st.history_complete, st.relay_reached)
        };
        if !stopped {
            roostr_core::sync_trace!(
                "history pass done (complete: {complete}, relay reachable: {reachable})"
            );
            self.host.on_history_pass(complete, reachable);
        }
        Ok(complete)
    }

    /// Every relay × filter at once: reconcile, or walk where the relay
    /// does not reconcile. Store failures propagate.
    async fn cover(
        self: &Arc<Self>,
        pass: &Pass,
        filters: &[StreamFilter],
        resume: Option<i64>,
        track: bool,
    ) -> anyhow::Result<()> {
        let jobs = self.relays.iter().enumerate().flat_map(|(index, relay)| {
            filters.iter().map(move |sf| async move {
                let unsupported = self
                    .state
                    .lock()
                    .negentropy_unsupported
                    .contains(relay.url());
                if !unsupported {
                    match self.reconcile_stream(index, pass, sf, None).await? {
                        Outcome::Complete => return Ok(()),
                        Outcome::Incomplete => {
                            pass.complete.store(false, Ordering::Relaxed);
                            return Ok(());
                        }
                        Outcome::Unsupported => {}
                    }
                }
                pass.walked.store(true, Ordering::Relaxed);
                self.walk(index, pass, &sf.filter, pass.since, resume, track)
                    .await
                    .map(|_| ())
            })
        });
        for result in futures::future::join_all(jobs).await {
            result?;
        }
        Ok(())
    }

    /// The parts (1080 by `#c`) of every split base whose 31078 arrived
    /// without them, from every relay.
    pub(super) async fn fetch_base_parts(self: &Arc<Self>, pass: &Pass) -> anyhow::Result<()> {
        let gids: Vec<String> = self.state.lock().open_base_groups.drain().collect();
        if gids.is_empty() {
            return Ok(());
        }
        let fetches = self.relays.iter().enumerate().map(|(index, relay)| {
            let gids = &gids;
            async move {
                for chunk in gids.chunks(CHECKPOINT_FETCH_BATCH) {
                    if self.stopped() {
                        pass.complete.store(false, Ordering::Relaxed);
                        return Ok(());
                    }
                    let filter = Filter::new()
                        .kinds([kinds::BASE_PART])
                        .tag('c', chunk.iter().cloned());
                    match self.query_page(index, filter).await {
                        Ok(page) => self.import_page(pass, page).await?,
                        Err(error) => {
                            pass.complete.store(false, Ordering::Relaxed);
                            self.emit_backfill(Some(format!(
                                "{}: {}",
                                relay.url(),
                                short(error, 100)
                            )));
                            return Ok(());
                        }
                    }
                }
                anyhow::Ok(())
            }
        });
        for result in futures::future::join_all(fetches).await {
            result?;
        }
        Ok(())
    }

    /// The paged walk of one filter on one relay, newest first via `until`,
    /// from `since` at the oldest; true when it reached the end.
    pub(super) async fn walk(
        self: &Arc<Self>,
        index: usize,
        pass: &Pass,
        filter: &Filter,
        since: i64,
        resume: Option<i64>,
        track: bool,
    ) -> anyhow::Result<bool> {
        let relay = &self.relays[index];
        let mut until = resume;
        loop {
            if self.stopped() {
                pass.complete.store(false, Ordering::Relaxed);
                return Ok(false);
            }
            // A relay that fails or stalls is this walk's problem; a STORE
            // failure (below, outside this guard) propagates.
            let started = now_ms();
            let mut page_filter = filter.clone().limit(PAGE_LIMIT);
            page_filter.since = Some(since.max(filter.since.unwrap_or(0) as i64).max(0) as u64);
            page_filter.until = until.map(|u| u.max(0) as u64);
            let page = match self.query_page(index, page_filter).await {
                Ok(page) => page,
                Err(error) => {
                    pass.complete.store(false, Ordering::Relaxed);
                    self.emit_backfill(Some(format!("{}: {}", relay.url(), short(error, 100))));
                    return Ok(false);
                }
            };
            let len = page.len() as u64;
            let oldest = page.iter().map(|e| e.created_at as i64).min();
            self.note_received(page.len());
            self.import_page(pass, page).await?;
            self.emit_progress();
            if len < PAGE_LIMIT {
                return Ok(true);
            }
            let oldest = oldest.unwrap_or(0);
            if until.is_some_and(|u| oldest >= u) {
                pass.complete.store(false, Ordering::Relaxed);
                self.emit_backfill(Some(format!(
                    "{}: saturated same-timestamp history page",
                    relay.url()
                )));
                return Ok(false);
            }
            until = Some(oldest);
            // Coverage is only as deep as the slowest concurrent walker.
            if track {
                let floor = {
                    let mut covered = pass.covered_until.lock();
                    let next = covered.map_or(oldest, |c| c.min(oldest));
                    *covered = Some(next);
                    next
                };
                self.store.set_bootstrap_floor(Some(floor)).await?;
            }
            roostr_nostr::sleep(super::history::pace(started)).await;
        }
    }

    /// `REQ` until a real EOSE: a timeout or CLOSED is an error, never a
    /// short page. One at a time per relay: concurrent pages of large events
    /// outgrow a relay's outbound budget for one connection, and a reader
    /// that falls behind it is disconnected (every query in flight fails).
    pub(super) async fn query_page(
        &self,
        index: usize,
        filter: Filter,
    ) -> Result<Vec<Event>, RelayError> {
        let _fetching = self.fetches[index].lock().await;
        let result = self.relays[index].query(vec![filter], PAGE_TIMEOUT).await;
        if result.is_ok() {
            self.state.lock().relay_reached = true;
        }
        result
    }

    /// Ingest a page (oldest first, checkpoints and bases ahead of changes
    /// within one second) and import what completed.
    pub(super) async fn import_page(
        self: &Arc<Self>,
        pass: &Pass,
        mut page: Vec<Event>,
    ) -> anyhow::Result<()> {
        page.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then(b.kind.cmp(&a.kind))
                .then(a.id.cmp(&b.id))
        });
        let mut batch = Vec::new();
        for event in page {
            if !pass.seen.lock().insert(event.id.clone()) {
                continue;
            }
            if let Some(tags) = &pass.h_tags {
                tags.lock()
                    .extend(event.tag_values("h").map(str::to_string));
            }
            if let Some(item) = self.ingest_event(&event).await? {
                batch.push(item);
            }
        }
        if !batch.is_empty() {
            self.import_batch(batch, true).await?;
        }
        Ok(())
    }

    /// NIP-77 cover of one stream filter on one relay: reconcile the held
    /// set, then fetch every needed id not in `skip`. `claimed` is shared by
    /// every stream of one pass: an event in two streams is fetched once.
    async fn reconcile_stream(
        self: &Arc<Self>,
        index: usize,
        pass: &Pass,
        sf: &StreamFilter,
        skip: Option<&HashSet<String>>,
    ) -> anyhow::Result<Outcome> {
        let relay = &self.relays[index];
        let held = self
            .store
            .held_events(
                sf.scope.clone(),
                HeldQuery {
                    kinds: sf.filter.kinds.clone().unwrap_or_default(),
                    since: sf.filter.since.map(|s| s as i64),
                    until: sf.filter.until.map(|u| u as i64),
                    b: sf.filter.tags.get(&'b').cloned(),
                },
            )
            .await?;
        let need = match self.negotiate(index, sf, &held).await {
            Ok(need) => need,
            Err(outcome) => return Ok(outcome),
        };
        let need: Vec<String> = {
            let mut claimed = pass.claimed.lock();
            need.into_iter()
                .filter(|id| !skip.is_some_and(|skip| skip.contains(id)))
                .filter(|id| claimed.insert(id.clone()))
                .collect()
        };
        if let Some((_, expected)) = self.state.lock().pass.as_mut() {
            *expected += need.len() as u64;
        }
        let kinds = sf.filter.kinds.as_deref().unwrap_or_default();
        let batch = if kinds.contains(&kinds::CHECKPOINT) || kinds.contains(&kinds::BASE) {
            CHECKPOINT_FETCH_BATCH
        } else {
            FETCH_BATCH
        };
        let mut missing: HashSet<String> = HashSet::new();
        let chunks: Vec<&[String]> = need.chunks(batch).collect();
        for (i, ids) in chunks.iter().enumerate() {
            if self.stopped() {
                return Ok(Outcome::Incomplete);
            }
            let started = now_ms();
            let filter = Filter::new()
                .ids(ids.iter().cloned())
                .limit(ids.len() as u64);
            let page = match self.query_page(index, filter).await {
                Ok(page) => page,
                Err(error) => {
                    self.emit_backfill(Some(format!("{}: {}", relay.url(), short(error, 100))));
                    return Ok(Outcome::Incomplete);
                }
            };
            let mut wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
            let page: Vec<Event> = page
                .into_iter()
                .filter(|e| wanted.remove(e.id.as_str()))
                .collect();
            missing.extend(wanted.into_iter().map(str::to_string));
            self.note_received(page.len());
            self.import_page(pass, page).await?;
            self.emit_progress();
            if i + 1 < chunks.len() {
                roostr_nostr::sleep(pace(started)).await;
            }
        }
        if missing.is_empty() {
            return Ok(Outcome::Complete);
        }
        // A needed id the relay no longer returns was deleted meanwhile -
        // another device compacted it into a base (NIP-09) - and is settled
        // when the relay no longer lists it either. Only one it still lists
        // but did not return is asked for again next run.
        let listed: HashSet<String> = match self.negotiate(index, sf, &held).await {
            Ok(need) => need.into_iter().collect(),
            Err(outcome) => return Ok(outcome),
        };
        let held_back = missing.iter().filter(|id| listed.contains(*id)).count();
        self.note_received(missing.len() - held_back);
        roostr_core::sync_trace!(
            "{}: {} reconciled event(s) deleted before the fetch, {held_back} withheld",
            relay.url(),
            missing.len() - held_back
        );
        if held_back > 0 {
            self.emit_backfill(Some(format!(
                "{}: {held_back} reconciled events not returned",
                relay.url()
            )));
            return Ok(Outcome::Incomplete);
        }
        Ok(Outcome::Complete)
    }

    /// NIP-77 against `held`: the ids `sf`'s stream on relay `index` holds
    /// and `held` lacks, or how the cover ends without them.
    async fn negotiate(
        &self,
        index: usize,
        sf: &StreamFilter,
        held: &[HeldItem],
    ) -> Result<Vec<String>, Outcome> {
        let relay = &self.relays[index];
        // One negotiation at a time per relay (relays cap NEG sessions).
        let _negotiating = self.negotiations[index].lock().await;
        if self
            .state
            .lock()
            .negentropy_unsupported
            .contains(relay.url())
        {
            return Err(Outcome::Unsupported);
        }
        let storage = match NegentropyStorage::from_hex(
            held.iter()
                .map(|h| (h.created_at.max(0) as u64, h.id.as_str())),
        ) {
            Ok(storage) => storage,
            Err(error) => {
                self.emit_backfill(Some(format!("held set: {error}")));
                return Err(Outcome::Incomplete);
            }
        };
        match relay.reconcile(&sf.filter, storage, NEG_TIMEOUT).await {
            Ok(reconciled) => {
                self.state.lock().relay_reached = true;
                Ok(reconciled.need)
            }
            Err(error) => {
                if !matches!(error.error, RelayError::Transport(_)) {
                    self.state.lock().relay_reached = true;
                }
                if error.relay_refused() {
                    // Don't ask this relay again this session.
                    self.state
                        .lock()
                        .negentropy_unsupported
                        .insert(relay.url().to_string());
                    return Err(Outcome::Unsupported);
                }
                if !error.answered {
                    // No socket, or failed before any answer: walk this filter.
                    return Err(Outcome::Unsupported);
                }
                self.emit_backfill(Some(format!("{}: {}", relay.url(), short(&error, 100))));
                Err(Outcome::Incomplete)
            }
        }
    }

    /// `n` events of the pass in flight settled: fetched, or gone for good.
    fn note_received(&self, n: usize) {
        if let Some((received, _)) = self.state.lock().pass.as_mut() {
            *received += n as u64;
        }
    }

    /// Each bounded stream's history below its window that no held base
    /// covers. An object nobody has based (its computer offline, a harness
    /// without the unbased fallback) can start before the window; without
    /// this a fresh device never holds it. Per relay, the ids of every
    /// event of an object holding a base are listed by NIP-77 (nothing
    /// fetched), and the stream's reconcile fetches the rest. A stream is
    /// swept up to its floor once every relay covered it; later passes
    /// sweep only what the floor moved past.
    async fn sweep_unbased(
        self: &Arc<Self>,
        pass: &Pass,
        change_filters: &[StreamFilter],
        floors: &HashMap<String, i64>,
    ) -> anyhow::Result<()> {
        if self.relays.is_empty() {
            return Ok(());
        }
        let mut swept: BTreeMap<String, i64> = self
            .store
            .meta(UNBASED_SWEPT.to_string())
            .await?
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        // (the stream below its floor, swept since, floor)
        let due: Vec<(StreamFilter, i64, i64)> = change_filters
            .iter()
            .filter_map(|sf| {
                let floor = *floors.get(&sf.scope)?;
                let from = swept.get(&sf.scope).copied().unwrap_or(0);
                (from < floor).then(|| {
                    let mut filter = sf.filter.clone().until(floor as u64);
                    filter.since = (from > 0).then_some(from as u64);
                    let scope = sf.scope.clone();
                    (StreamFilter { scope, filter }, from, floor)
                })
            })
            .collect();
        if due.is_empty() {
            return Ok(());
        }
        let tags = self.based_object_tags().await?;
        let since = due.iter().map(|(_, from, _)| *from).min().unwrap_or(0);
        let until = due.iter().map(|(_, _, floor)| *floor).max().unwrap_or(0);
        let jobs = (0..self.relays.len()).map(|index| {
            let (due, tags) = (&due, &tags);
            async move {
                let covered = if tags.is_empty() {
                    Covered::Ids(HashSet::new())
                } else {
                    self.covered_ids(index, tags, since, until).await
                };
                let mut done = vec![false; due.len()];
                for (i, (sf, from, floor)) in due.iter().enumerate() {
                    if self.stopped() {
                        break;
                    }
                    let walk = match &covered {
                        Covered::Failed => continue,
                        Covered::Unsupported => true,
                        Covered::Ids(covered) => {
                            match self
                                .reconcile_stream(index, pass, sf, Some(covered))
                                .await?
                            {
                                Outcome::Complete => {
                                    done[i] = true;
                                    false
                                }
                                Outcome::Incomplete => false,
                                Outcome::Unsupported => true,
                            }
                        }
                    };
                    if walk {
                        // No ids to subtract: read the whole range.
                        done[i] = self
                            .walk(index, pass, &sf.filter, *from, Some(*floor), false)
                            .await?;
                    }
                }
                anyhow::Ok(done)
            }
        });
        let mut complete = vec![true; due.len()];
        for result in futures::future::join_all(jobs).await {
            for (all, done) in complete.iter_mut().zip(result?) {
                *all &= done;
            }
        }
        let mut advanced = false;
        for ((sf, _, floor), complete) in due.iter().zip(complete) {
            if complete {
                swept.insert(sf.scope.clone(), *floor);
                advanced = true;
            } else {
                pass.complete.store(false, Ordering::Relaxed);
            }
        }
        if advanced {
            self.store
                .set_meta(
                    UNBASED_SWEPT.to_string(),
                    Some(serde_json::to_string(&swept)?),
                )
                .await?;
        }
        Ok(())
    }

    /// The blinded object tags (`h`) every held base's object may carry:
    /// the personal one and one per installed shared space.
    async fn based_object_tags(&self) -> anyhow::Result<Vec<String>> {
        let objects = self.store.current_bases().await?;
        let mut prefixes = vec![hex::decode(&self.secret_hex)?];
        prefixes.extend(
            self.state
                .lock()
                .shared
                .values()
                .map(|sp| sp.info.key_hex.as_bytes().to_vec()),
        );
        let mut tags: Vec<String> = objects
            .keys()
            .flat_map(|id| prefixes.iter().map(move |prefix| blind(prefix, id)))
            .collect();
        tags.sort();
        tags.dedup();
        Ok(tags)
    }

    /// The ids of every 1078 `relay` holds tagged with one of `tags`
    /// (`created_at` in `since..=until`), listed by NIP-77 against an empty
    /// set: nothing is fetched.
    async fn covered_ids(&self, index: usize, tags: &[String], since: i64, until: i64) -> Covered {
        let relay = &self.relays[index];
        let mut ids = HashSet::new();
        for chunk in tags.chunks(BASE_HASH_BATCH) {
            if self.stopped() {
                return Covered::Failed;
            }
            let mut filter = Filter::new()
                .kinds([kinds::CHANGE])
                .tag('h', chunk.iter().cloned())
                .until(until.max(0) as u64);
            if since > 0 {
                filter = filter.since(since as u64);
            }
            let _negotiating = self.negotiations[index].lock().await;
            if self
                .state
                .lock()
                .negentropy_unsupported
                .contains(relay.url())
            {
                return Covered::Unsupported;
            }
            let Ok(empty) = NegentropyStorage::from_hex(std::iter::empty()) else {
                return Covered::Failed;
            };
            match relay.reconcile(&filter, empty, NEG_TIMEOUT).await {
                Ok(reconciled) => {
                    self.state.lock().relay_reached = true;
                    ids.extend(reconciled.need);
                }
                Err(error) => {
                    if error.relay_refused() {
                        self.state
                            .lock()
                            .negentropy_unsupported
                            .insert(relay.url().to_string());
                        return Covered::Unsupported;
                    }
                    self.emit_backfill(Some(format!("{}: {}", relay.url(), short(&error, 100))));
                    return Covered::Failed;
                }
            }
        }
        Covered::Ids(ids)
    }

    /// Settle every live event and import in flight; new arrivals extend
    /// the wait, they never fault.
    pub(super) async fn drain_inflight(&self) {
        loop {
            {
                let st = self.state.lock();
                if st.stopped || (st.active_live_events == 0 && st.active_imports == 0) {
                    return;
                }
            }
            roostr_nostr::sleep(Duration::from_millis(20)).await;
        }
    }
}

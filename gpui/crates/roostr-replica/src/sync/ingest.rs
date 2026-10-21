//! Event → change / checkpoint / base (sync.ts `ingestEvent`,
//! `importBatch`, `rebaseOrphans`; SyncEngine.swift `ingest`/`importOne`).
//!
//! The engine session owns reassembly, the cursor and replay bookkeeping;
//! this side records what it reports, gates shared payloads through the
//! engine's authority, stores imports, and keeps the held-events table (the
//! NIP-77 item set) in step: an event is held only once its payload
//! imported, so a faulted or unauthorized one is asked for again.
//!
//! A batch (one relay page) writes its changes, their publication marks
//! and its held events as one store transaction
//! ([`ChangeStore::import`](roostr_replica_store::ChangeStore::import)),
//! not one per change: a large vault's first sync is thousands of changes.
//! A shared payload's authorization reads what the store holds for its
//! space and object; when the batch has buffered changes of either, the
//! buffer is written first, so every verdict sees exactly what it did when
//! each change was written on its own.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Context;
use roostr_engine::{
    AuthorizeSubject, BaseSummary, Change, CheckpointSummary, EngineError, RelayEvent,
    SharedProvenance, SpaceAuthority, StateInputs,
};
use roostr_nostr::{Event, kinds};
use roostr_replica_store::{BaseRecord, ChangeRecord, CheckpointRecord, ImportBatch, ObjectInputs};
use serde_json::Value;

use super::{HeldRow, Inner, describe_event, now_ms, short, short_id};

/// One decrypted relay payload.
pub(super) struct ImportItem {
    pub(super) object_id: String,
    bytes: Vec<u8>,
    change: Option<Change>,
    checkpoint: Option<CheckpointSummary>,
    base: Option<BaseSummary>,
    provenance: Option<SharedProvenance>,
    chunk_key: Option<String>,
    /// Relay events this payload came from (every part of its group).
    held: Vec<HeldRow>,
    part_key: Option<String>,
}

/// One batch's writes not yet in the store ([`Inner::flush`]).
#[derive(Default)]
struct Pending {
    batch: ImportBatch,
    /// Objects whose changes `batch` holds.
    objects: HashSet<String>,
    /// Chunk keys of the items written with `batch`, settled once it commits.
    settle: Vec<String>,
}

/// What the store holds for one object as a batch's authorization reads it:
/// its inputs as read, plus the changes the batch buffered for it since -
/// exactly what a read after writing them would return - and their
/// replay, made when first asked.
struct Held {
    inputs: ObjectInputs,
    state: Option<Option<Value>>,
}

/// One batch's [`Held`] per object id. An entry goes when the batch writes
/// the object otherwise (a base, a checkpoint), or buffers a change whose
/// newness the inputs cannot tell (the object holds rebased rows).
type States = HashMap<String, Held>;

/// `record`, buffered for its object, in `states`.
fn hold_buffered(states: &mut States, record: &ChangeRecord) {
    let Some(held) = states.get_mut(&record.object_id) else {
        return;
    };
    // A rebased row is held but not among `changes`: the inputs alone cannot
    // tell a re-sent orphan from a new change, the store must.
    if held.inputs.count != held.inputs.changes.len() as u64 {
        states.remove(&record.object_id);
        return;
    }
    if !held.inputs.changes.iter().any(|c| c.id == record.id) {
        held.inputs.changes.push(record.clone());
        held.inputs.count += 1;
        held.state = None;
    }
}

/// Stored changes (engine JSON) → replay input.
pub(super) fn state_inputs(
    input: ObjectInputs,
    pending: Vec<String>,
) -> anyhow::Result<StateInputs> {
    let changes = input
        .changes
        .iter()
        .map(|record| {
            let mut change: Change = serde_json::from_str(&record.json)?;
            if change.b.is_none() {
                change.b.clone_from(&record.b);
            }
            Ok(change)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(StateInputs {
        bases: input.bases.into_iter().map(|b| b.bytes).collect(),
        changes,
        checkpoint: input.checkpoint.map(|c| c.bytes),
        pending,
    })
}

impl Inner {
    /// Feed one relay event (signature-checked by the relay client) to the
    /// engine session; the decoded payload once complete.
    pub(super) async fn ingest_event(&self, event: &Event) -> anyhow::Result<Option<ImportItem>> {
        self.state.lock().stats.events += 1;
        if !kinds::SPACE.contains(&event.kind) {
            roostr_core::sync_trace!("ingest {}: not a sync kind, skipped", short_id(&event.id));
            return Ok(None);
        }
        self.ensure_session().await?;
        if !self.session_open() {
            return Ok(None); // stopped
        }
        let r = self
            .engine
            .ingest(
                &RelayEvent {
                    pubkey: event.pubkey.clone(),
                    created_at: event.created_at as i64,
                    kind: event.kind as u16,
                    tags: event.tags.clone(),
                    content: event.content.clone(),
                },
                now_ms(),
            )
            .await?;
        {
            let mut st = self.state.lock();
            st.cursor = r.cursor;
            if let Some(tag) = &r.h_tag {
                st.stats.blinded_tags.insert(tag.clone());
            }
            if r.decrypt_failure {
                st.stats.decrypt_failures += 1;
            }
            if r.decode_failure {
                st.stats.decode_failures += 1;
            }
            if let Some(groups) = r.replay_groups.clone() {
                st.replay_groups = groups.into_iter().collect();
            }
        }
        if let Some(at) = r.fault_at {
            self.record_replay_fault(at).await;
        }
        // The engine only reports an h-deletion signed by the space's owner.
        if let Some(space) = r.space_vanished.clone() {
            self.host.on_space_vanished(space);
        }
        let rows = self.held_rows(event);
        let chunk = event.tag("c").map(str::to_string);
        // A base's 31078 and its 1080 parts are one group (keyed as the base kind).
        let group_kind = if event.kind == kinds::BASE_PART {
            kinds::BASE
        } else {
            event.kind
        };
        let part_key = chunk.as_ref().map(|gid| {
            let hs: Vec<&str> = event.tag_values("h").collect();
            format!("{}|{}|{gid}|{group_kind}", event.pubkey, hs.join(","))
        });
        if roostr_core::sync_trace::enabled() {
            let what = match &r.item {
                None => "no payload yet (a part of a split base, or nothing to import)".to_string(),
                Some(item) => match (&item.base, &item.checkpoint, &item.change) {
                    (Some(base), _, _) => format!("base of {}", base.object_id),
                    (_, Some(cp), _) => format!("checkpoint of {}", cp.object_id),
                    (_, _, Some(change)) => {
                        format!("change {} of {}", short_id(&change.id), change.object_id)
                    }
                    _ => "empty payload".to_string(),
                },
            };
            roostr_core::sync_trace!(
                "ingest {}: {what}{}{}{}{}",
                describe_event(event, &self.pk),
                if r.decrypt_failure {
                    " | DECRYPT FAILED (no key for this stream?)"
                } else {
                    ""
                },
                if r.decode_failure {
                    " | DECODE FAILED"
                } else {
                    ""
                },
                if r.fault_at.is_some() {
                    " | REPLAY FAULT"
                } else {
                    ""
                },
                r.item
                    .as_ref()
                    .and_then(|i| i.provenance.as_ref())
                    .map(|p| format!(" | shared space {} key {}", short_id(&p.space_id), p.key_id))
                    .unwrap_or_default(),
            );
        }
        let Some(item) = r.item else {
            let record = {
                let mut st = self.state.lock();
                let imported = part_key
                    .as_ref()
                    .is_some_and(|k| st.imported_parts.contains(k));
                if event.kind == kinds::BASE
                    && r.fault_at.is_none()
                    && !r.decrypt_failure
                    && !imported
                    && let Some(gid) = &chunk
                {
                    st.open_base_groups.insert(gid.clone());
                }
                if r.fault_at.is_some() {
                    if let Some(key) = &part_key {
                        st.pending_parts.remove(key);
                    }
                    None
                } else if let Some(key) = part_key
                    .as_ref()
                    .filter(|_| !r.decrypt_failure && !imported)
                {
                    st.pending_parts
                        .entry(key.clone())
                        .or_default()
                        .extend(rows);
                    None
                } else {
                    Some(rows)
                }
            };
            if let Some(rows) = record {
                self.record_held(rows).await?;
            }
            return Ok(None);
        };
        let held = {
            let mut st = self.state.lock();
            match &part_key {
                Some(key) => {
                    let mut held = st.pending_parts.remove(key).unwrap_or_default();
                    held.extend(rows);
                    held
                }
                None => rows,
            }
        };
        let (object_id, change) = if let Some(base) = &item.base {
            (base.object_id.clone(), None)
        } else if let Some(checkpoint) = &item.checkpoint {
            (checkpoint.object_id.clone(), None)
        } else if let Some(mut change) = item.change {
            change.b = item.b;
            (change.object_id.clone(), Some(change))
        } else {
            return Ok(None);
        };
        Ok(Some(ImportItem {
            object_id,
            bytes: item.bytes,
            change,
            checkpoint: item.checkpoint,
            base: item.base,
            provenance: item.provenance,
            chunk_key: item.chunk_key,
            held,
            part_key,
        }))
    }

    /// Shared authority: existing scope and privileges come from what this
    /// replica already holds, never from the candidate - replayed as the
    /// backend does, from the inputs as stored plus what this batch
    /// buffered for them ([`Held`]). An object read with buffered changes
    /// the inputs cannot account for writes the buffer first.
    async fn authorized(
        &self,
        item: &ImportItem,
        p: &SharedProvenance,
        pending: &mut Pending,
        states: &mut States,
        imported: &mut HashSet<String>,
    ) -> anyhow::Result<bool> {
        let Some(space) = self
            .state
            .lock()
            .shared
            .get(&p.space_id)
            .map(|sp| sp.info.clone())
        else {
            return Ok(false);
        };
        for id in [&p.space_id, &item.object_id] {
            if !states.contains_key(id) {
                if pending.objects.contains(id) {
                    self.flush(pending, imported).await?;
                }
                let inputs = self.store.object_inputs(id.clone()).await?;
                states.insert(
                    id.clone(),
                    Held {
                        inputs,
                        state: None,
                    },
                );
            }
            let held = states.get_mut(id).expect("inserted above");
            if held.state.is_none() {
                let input = state_inputs(held.inputs.clone(), Vec::new())?;
                held.state = Some(self.engine.replay_from_base(&input).await?);
            }
        }
        let state = |id: &String| {
            states
                .get(id)
                .and_then(|held| held.state.as_ref()?.as_ref())
        };
        let trusted = state(&p.space_id);
        let existing = state(&item.object_id);
        let subject = if item.base.is_some() {
            AuthorizeSubject::Base(&item.bytes)
        } else if item.checkpoint.is_some() {
            AuthorizeSubject::Checkpoint(&item.bytes)
        } else if let Some(change) = &item.change {
            AuthorizeSubject::Change(change)
        } else {
            return Ok(false);
        };
        let authority = SpaceAuthority {
            space_id: space.space_id.clone(),
            key_id: space.key_id,
            owner: space.owner.clone().unwrap_or_else(|| self.pk.clone()),
        };
        match self
            .engine
            .authorize(subject, p, &authority, trusted, existing)
            .await
        {
            Ok(verdict) => Ok(verdict.ok),
            // A malformed change or state is a rejection, not a fault.
            Err(EngineError::Domain(_)) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Keep a checkpoint when it supersedes the one held (the core's rule:
    /// covers a superset, then hash; an incomparable fork never replaces it).
    async fn import_checkpoint(
        &self,
        item: &ImportItem,
        summary: &CheckpointSummary,
    ) -> anyhow::Result<()> {
        let mut heads = summary.head_ids.clone();
        heads.sort();
        let fresh = self
            .store
            .put_checkpoint(
                &self.engine,
                CheckpointRecord {
                    object_id: summary.object_id.clone(),
                    bytes: item.bytes.clone(),
                    hash: summary.hash.clone(),
                    heads,
                    covered: summary.covered.max(0) as u64,
                },
            )
            .await?;
        if fresh {
            let mut st = self.state.lock();
            st.stats.checkpoints += 1;
            st.pending_objects.insert(summary.object_id.clone());
        }
        Ok(())
    }

    /// Hold a base and re-point the object's current one. Bases are never
    /// deleted by sync: a superseded one keeps more orphans decidable.
    ///
    /// A base from a space stream holds every change it covers there - the
    /// compaction that sealed it deleted their deltas - so those changes are
    /// marked published under the space key: no device re-seals a space's
    /// compacted history.
    async fn import_base(&self, item: &ImportItem, summary: &BaseSummary) -> anyhow::Result<()> {
        if let Some(p) = &item.provenance {
            let covered = self.engine.base_decode(&item.bytes).await?.covered_ids;
            let published: Vec<String> = covered
                .iter()
                .map(|id| format!("{}/{}/{id}", p.space_id, p.key_id))
                .collect();
            roostr_core::sync_trace!(
                "base of {} covers {} change(s): published under space {}",
                summary.object_id,
                published.len(),
                short_id(&p.space_id)
            );
            self.store
                .import(ImportBatch {
                    published,
                    ..ImportBatch::default()
                })
                .await?;
        }
        let streams: Vec<String> = item
            .held
            .iter()
            .map(|row| row.scope.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let fresh = self
            .store
            .put_base(
                BaseRecord {
                    object_id: summary.object_id.clone(),
                    hash: summary.hash.clone(),
                    epoch: summary.epoch,
                    created_at: summary.created_at,
                    prev_base: summary.prev_base.clone(),
                    bytes: item.bytes.clone(),
                },
                streams,
            )
            .await?;
        let mut st = self.state.lock();
        st.based_objects.insert(summary.object_id.clone());
        if fresh {
            st.stats.bases += 1;
            st.pending_objects.insert(summary.object_id.clone());
        }
        Ok(())
    }

    /// Rebase every orphan of these objects onto its current base, store
    /// each with its orphan marked, and publish it carrying `b`. Callers
    /// hold the import lock. An engine refusal is reported, never a fault.
    pub(super) async fn rebase_orphans(
        self: &Arc<Self>,
        object_ids: impl IntoIterator<Item = String>,
    ) -> anyhow::Result<()> {
        let mut pending: Option<HashSet<String>> = None;
        let mut done = HashSet::new();
        for object_id in object_ids {
            if !done.insert(object_id.clone()) {
                continue;
            }
            let skip = {
                let st = self.state.lock();
                st.stopped || !st.based_objects.contains(&object_id)
            };
            if skip || !self.may_rebase(&object_id) {
                continue;
            }
            if pending.is_none() {
                pending = Some(self.store.pending_change_ids().await?);
            }
            let input = self.store.object_inputs(object_id.clone()).await?;
            if input.bases.is_empty() {
                continue;
            }
            let ids: Vec<String> = pending.iter().flatten().cloned().collect();
            let inputs = state_inputs(input, ids)?;
            let rebased = match self.engine.rebase(&inputs, None).await {
                Ok(rebased) => rebased,
                Err(error @ EngineError::Domain(_)) => {
                    self.emit_error(format!(
                        "rebase of {object_id} refused: {}",
                        short(error, 120)
                    ));
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            for rc in rebased.changes {
                // Stored as relay imports are: the decoded protobuf plus its base.
                let mut stored = match self.engine.decode_change(&rc.bytes).await {
                    Ok(change) => change,
                    Err(_) => rc.change.clone(),
                };
                stored.b = Some(rebased.base.clone());
                let id = if stored.id.is_empty() {
                    rc.id.clone()
                } else {
                    stored.id.clone()
                };
                self.store
                    .add_rebased(
                        rc.orphan.clone(),
                        ChangeRecord {
                            id: id.clone(),
                            object_id: object_id.clone(),
                            bytes: rc.bytes.clone(),
                            json: serde_json::to_string(&stored)?,
                            b: Some(rebased.base.clone()),
                            rebased: false,
                        },
                    )
                    .await?;
                if let Some(pending) = pending.as_mut() {
                    pending.insert(id.clone());
                }
                {
                    let mut st = self.state.lock();
                    st.stats.rebased += 1;
                    st.pending_objects.insert(object_id.clone());
                }
                self.clone()
                    .publish(rc.bytes, id, object_id.clone(), Some(rebased.base.clone()))
                    .await?;
            }
        }
        Ok(())
    }

    /// Report a reassembled group's import outcome to the engine session.
    async fn settle(&self, chunk_key: &str, imported: bool) {
        if !self.session_open() {
            return;
        }
        match self.engine.settle(chunk_key, imported).await {
            Ok(result) => {
                if let Some(groups) = result.replay_groups {
                    self.state.lock().replay_groups = groups.into_iter().collect();
                }
            }
            Err(error) => log::warn!("[sync] settle {chunk_key}: {error}"),
        }
    }

    pub(super) async fn import_batch(
        self: &Arc<Self>,
        batch: Vec<ImportItem>,
        immediate_notify: bool,
    ) -> anyhow::Result<()> {
        self.state.lock().active_imports += 1;
        let mut imported = HashSet::new();
        let result = {
            let _import = self.import_lock.lock().await;
            self.import_items(&batch, &mut imported, immediate_notify)
                .await
        };
        if result.is_err() {
            self.record_replay_fault(1).await;
            let _ = self.persist_cursor().await;
        }
        for item in &batch {
            if let Some(key) = &item.chunk_key
                && !imported.contains(key)
            {
                self.settle(key, false).await;
            }
        }
        self.state.lock().active_imports -= 1;
        result
    }

    async fn import_items(
        self: &Arc<Self>,
        batch: &[ImportItem],
        imported: &mut HashSet<String>,
        immediate_notify: bool,
    ) -> anyhow::Result<()> {
        let mut pending = Pending::default();
        let mut states = States::new();
        let mut parts = Vec::new();
        let mut touched = Vec::new();
        for item in batch {
            // Unauthorized payloads stay unheld: a later reconcile (a lite
            // fetch: its next round) re-judges them.
            if let Some(p) = &item.provenance
                && !self
                    .authorized(item, p, &mut pending, &mut states, imported)
                    .await
                    .context("authorize")?
            {
                roostr_core::sync_trace!(
                    "import REFUSED for {} (signer {}, space {}): the authority gate said no",
                    item.object_id,
                    short_id(&p.signer),
                    short_id(&p.space_id)
                );
                self.state.lock().stats.refused += 1;
                continue;
            }
            roostr_core::sync_trace!(
                "import {} for {}",
                if item.base.is_some() {
                    "base".to_string()
                } else if item.checkpoint.is_some() {
                    "checkpoint".to_string()
                } else {
                    format!(
                        "change {}",
                        item.change.as_ref().map_or("?", |c| short_id(&c.id))
                    )
                },
                item.object_id
            );
            if let Some(base) = &item.base {
                states.remove(&item.object_id);
                self.import_base(item, base).await.context("base")?;
            } else if let Some(checkpoint) = &item.checkpoint {
                states.remove(&item.object_id);
                self.import_checkpoint(item, checkpoint).await?;
            } else if let Some(change) = &item.change {
                let record = ChangeRecord {
                    id: change.id.clone(),
                    object_id: change.object_id.clone(),
                    bytes: item.bytes.clone(),
                    json: serde_json::to_string(change)?,
                    b: change.b.clone(),
                    rebased: false,
                };
                hold_buffered(&mut states, &record);
                pending.batch.changes.push(record);
                pending.batch.published.push(match &item.provenance {
                    Some(p) => format!("{}/{}/{}", p.space_id, p.key_id, change.id),
                    None => change.id.clone(),
                });
                pending.objects.insert(change.object_id.clone());
            }
            if let Some(key) = &item.chunk_key {
                pending.settle.push(key.clone());
            }
            pending.batch.held.extend(
                item.held
                    .iter()
                    .map(|row| (row.scope.clone(), row.event.clone())),
            );
            if let Some(key) = &item.part_key {
                parts.push(key.clone());
            }
            touched.push(item.object_id.clone());
            if let Some((received, _)) = self.state.lock().pass.as_mut() {
                *received += 1;
            }
        }
        self.flush(&mut pending, imported).await?;
        self.state.lock().imported_parts.extend(parts);
        // A new base can orphan held deltas, and a delta can arrive orphaned.
        self.rebase_orphans(touched)
            .await
            .context("rebase orphans")?;
        if immediate_notify {
            self.flush_object_notify();
        } else {
            self.schedule_object_notify();
        }
        Ok(())
    }

    /// Writes the batch's buffered changes, publication marks and held
    /// events in one transaction, then settles their groups and owes the
    /// host their objects.
    async fn flush(
        &self,
        pending: &mut Pending,
        imported: &mut HashSet<String>,
    ) -> anyhow::Result<()> {
        let batch = std::mem::take(&mut pending.batch);
        if !batch.is_empty() {
            let added = self.store.import(batch).await?;
            let mut st = self.state.lock();
            st.stats.imported += added as u64;
            st.pending_objects.extend(pending.objects.drain());
        }
        for key in std::mem::take(&mut pending.settle) {
            self.settle(&key, true).await;
            imported.insert(key);
        }
        Ok(())
    }

    pub(super) async fn record_replay_fault(&self, at: i64) {
        {
            let mut st = self.state.lock();
            st.replay_fault_generation += 1;
            st.discarded_chunk_floor = st.discarded_chunk_floor.min(at);
            st.history_complete = false;
        }
        // A closed store (stop() under a running walk) is not an error here.
        let _ = self.store.set_bootstrap_floor(None).await;
    }

    /// The cursor only moves back on a fault, and forward once history is
    /// complete and nothing is in flight; replay obligations ride along.
    pub(super) async fn persist_cursor(&self) -> anyhow::Result<()> {
        let _guard = self.cursor_lock.lock().await;
        // The mirror must reflect persisted obligations before overwriting them.
        self.ensure_session().await?;
        let saved = self.store.cursor().await?;
        let (value, groups) = {
            let st = self.state.lock();
            let floor = st
                .replay_groups
                .values()
                .copied()
                .fold(st.discarded_chunk_floor, i64::min);
            let next = if floor == i64::MAX {
                st.cursor
            } else {
                st.cursor.min((floor - 1).max(0))
            };
            let advance = next < saved
                || (st.history_complete
                    && st.active_live_events == 0
                    && st.active_imports == 0
                    && next > saved);
            let mut groups: Vec<(String, i64)> = st
                .replay_groups
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            groups.sort();
            (if advance { next } else { saved }, groups)
        };
        self.store.set_cursor(value, Some(groups)).await?;
        Ok(())
    }
}

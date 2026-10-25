//! A [`ChangeStore`] over SQLite whose writes a test can delay or fail:
//! the seam the sync recovery tests inject disk faults and races through.
//! Include with `#[path = "support/hooked_store.rs"] mod hooked_store;`.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;
use roostr_replica_store::{
    BaseRecord, BoxFuture, CachedReplay, ChangeRecord, ChangeStore, CheckpointRecord, CurrentBase,
    HeldEvent, HeldItem, HeldQuery, ImportBatch, ObjectHistory, ObjectInputs, PendingPublish,
    ReplayInput, SqliteChangeStore, StoreError, StoreFuture,
};

type Gate = Box<dyn FnMut(&[ChangeRecord]) -> BoxFuture<'static, Result<(), StoreError>> + Send>;
type CursorGate = Box<dyn FnMut(i64) -> BoxFuture<'static, ()> + Send>;
type HeldHook = Box<dyn FnMut(&str) + Send>;

#[derive(Default)]
struct Hooks {
    import: Option<Gate>,
    set_cursor: Option<CursorGate>,
    held_events: Option<HeldHook>,
}

pub struct HookedStore {
    pub inner: Arc<SqliteChangeStore>,
    hooks: Mutex<Hooks>,
}

impl HookedStore {
    pub fn new(inner: SqliteChangeStore) -> Self {
        Self {
            inner: Arc::new(inner),
            hooks: Mutex::new(Hooks::default()),
        }
    }

    /// Runs before every `import` carrying changes (a page of relay imports)
    /// with those changes; an `Err` fails the write unapplied.
    pub fn on_import(
        &self,
        hook: impl FnMut(&[ChangeRecord]) -> BoxFuture<'static, Result<(), StoreError>> + Send + 'static,
    ) {
        self.hooks.lock().import = Some(Box::new(hook));
    }

    /// Runs before every `set_cursor` with the value about to be written.
    pub fn on_set_cursor(&self, hook: impl FnMut(i64) -> BoxFuture<'static, ()> + Send + 'static) {
        self.hooks.lock().set_cursor = Some(Box::new(hook));
    }

    /// Called with the scope of every `held_events` read (a reconcile starting).
    pub fn on_held_events(&self, hook: impl FnMut(&str) + Send + 'static) {
        self.hooks.lock().held_events = Some(Box::new(hook));
    }

    pub fn clear_hooks(&self) {
        *self.hooks.lock() = Hooks::default();
    }
}

impl ChangeStore for HookedStore {
    fn add_changes(&self, records: Vec<ChangeRecord>) -> StoreFuture<'_, usize> {
        self.inner.add_changes(records)
    }
    fn import(&self, batch: ImportBatch) -> StoreFuture<'_, usize> {
        let gate = if batch.changes.is_empty() {
            None
        } else {
            self.hooks
                .lock()
                .import
                .as_mut()
                .map(|hook| hook(&batch.changes))
        };
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.await?;
            }
            self.inner.import(batch).await
        })
    }
    fn add_local_change(
        &self,
        record: ChangeRecord,
        pending: PendingPublish,
    ) -> StoreFuture<'_, ()> {
        self.inner.add_local_change(record, pending)
    }
    fn add_rebased(&self, orphan_id: String, record: ChangeRecord) -> StoreFuture<'_, ()> {
        self.inner.add_rebased(orphan_id, record)
    }
    fn mark_rebased(&self, ids: Vec<String>) -> StoreFuture<'_, ()> {
        self.inner.mark_rebased(ids)
    }
    fn changes_for(&self, object_id: String) -> StoreFuture<'_, Vec<ChangeRecord>> {
        self.inner.changes_for(object_id)
    }
    fn unpublished_changes(
        &self,
        object_ids: Vec<String>,
        key_prefix: String,
    ) -> StoreFuture<'_, Vec<ChangeRecord>> {
        self.inner.unpublished_changes(object_ids, key_prefix)
    }
    fn object_ids(&self) -> StoreFuture<'_, Vec<String>> {
        self.inner.object_ids()
    }
    fn change_counts(&self) -> StoreFuture<'_, HashMap<String, u64>> {
        self.inner.change_counts()
    }
    fn object_inputs(&self, object_id: String) -> StoreFuture<'_, ObjectInputs> {
        self.inner.object_inputs(object_id)
    }
    fn all_change_histories(&self) -> StoreFuture<'_, Vec<ObjectHistory>> {
        self.inner.all_change_histories()
    }
    fn put_base(&self, record: BaseRecord, streams: Vec<String>) -> StoreFuture<'_, bool> {
        self.inner.put_base(record, streams)
    }
    fn bases(&self, object_id: String) -> StoreFuture<'_, Vec<BaseRecord>> {
        self.inner.bases(object_id)
    }
    fn current_base(&self, object_id: String) -> StoreFuture<'_, Option<CurrentBase>> {
        self.inner.current_base(object_id)
    }
    fn current_bases(&self) -> StoreFuture<'_, HashMap<String, CurrentBase>> {
        self.inner.current_bases()
    }
    fn current_base_hashes(&self) -> StoreFuture<'_, Vec<String>> {
        self.inner.current_base_hashes()
    }
    fn objects_without_base(&self) -> StoreFuture<'_, Vec<String>> {
        self.inner.objects_without_base()
    }
    fn newest_base_by_stream(&self) -> StoreFuture<'_, HashMap<String, i64>> {
        self.inner.newest_base_by_stream()
    }
    fn checkpoint(&self, object_id: String) -> StoreFuture<'_, Option<CheckpointRecord>> {
        self.inner.checkpoint(object_id)
    }
    fn put_checkpoint(
        &self,
        engine: &roostr_engine::Engine,
        record: CheckpointRecord,
    ) -> StoreFuture<'_, bool> {
        self.inner.put_checkpoint(engine, record)
    }
    fn all_checkpoints(&self) -> StoreFuture<'_, HashMap<String, CheckpointRecord>> {
        self.inner.all_checkpoints()
    }
    fn forget_checkpoint_floors(&self) -> StoreFuture<'_, ()> {
        self.inner.forget_checkpoint_floors()
    }
    fn meta(&self, key: String) -> StoreFuture<'_, Option<String>> {
        self.inner.meta(key)
    }
    fn set_meta(&self, key: String, value: Option<String>) -> StoreFuture<'_, ()> {
        self.inner.set_meta(key, value)
    }
    fn cursor(&self) -> StoreFuture<'_, i64> {
        self.inner.cursor()
    }
    fn set_cursor(
        &self,
        cursor: i64,
        replay_groups: Option<Vec<(String, i64)>>,
    ) -> StoreFuture<'_, ()> {
        let gate = self
            .hooks
            .lock()
            .set_cursor
            .as_mut()
            .map(|hook| hook(cursor));
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.await;
            }
            self.inner.set_cursor(cursor, replay_groups).await
        })
    }
    fn replay_groups(&self) -> StoreFuture<'_, Vec<(String, i64)>> {
        self.inner.replay_groups()
    }
    fn bootstrapped(&self) -> StoreFuture<'_, bool> {
        self.inner.bootstrapped()
    }
    fn set_bootstrapped(&self) -> StoreFuture<'_, ()> {
        self.inner.set_bootstrapped()
    }
    fn bootstrap_floor(&self) -> StoreFuture<'_, Option<i64>> {
        self.inner.bootstrap_floor()
    }
    fn set_bootstrap_floor(&self, floor: Option<i64>) -> StoreFuture<'_, ()> {
        self.inner.set_bootstrap_floor(floor)
    }
    fn pending_publishes(&self) -> StoreFuture<'_, Vec<PendingPublish>> {
        self.inner.pending_publishes()
    }
    fn pending(&self, key: String) -> StoreFuture<'_, Option<PendingPublish>> {
        self.inner.pending(key)
    }
    fn save_pending(&self, item: PendingPublish) -> StoreFuture<'_, ()> {
        self.inner.save_pending(item)
    }
    fn pending_change_ids(&self) -> StoreFuture<'_, HashSet<String>> {
        self.inner.pending_change_ids()
    }
    fn own_changes(&self) -> StoreFuture<'_, HashMap<String, String>> {
        self.inner.own_changes()
    }
    fn is_published(&self, key: String) -> StoreFuture<'_, bool> {
        self.inner.is_published(key)
    }
    fn mark_published(&self, key: String) -> StoreFuture<'_, ()> {
        self.inner.mark_published(key)
    }
    fn record_held(&self, scope: String, events: Vec<HeldEvent>) -> StoreFuture<'_, ()> {
        self.inner.record_held(scope, events)
    }
    fn held_events(&self, scope: String, query: HeldQuery) -> StoreFuture<'_, Vec<HeldItem>> {
        if let Some(hook) = self.hooks.lock().held_events.as_mut() {
            hook(&scope);
        }
        self.inner.held_events(scope, query)
    }
    fn replay_inputs(&self) -> StoreFuture<'_, HashMap<String, ReplayInput>> {
        self.inner.replay_inputs()
    }
    fn cached_replays(
        &self,
        object_ids: Vec<String>,
        engine_version: String,
    ) -> StoreFuture<'_, HashMap<String, CachedReplay>> {
        self.inner.cached_replays(object_ids, engine_version)
    }
    fn put_cached_replays(
        &self,
        rows: Vec<CachedReplay>,
        engine_version: String,
    ) -> StoreFuture<'_, ()> {
        self.inner.put_cached_replays(rows, engine_version)
    }
    fn clear_cached_replays(&self) -> StoreFuture<'_, ()> {
        self.inner.clear_cached_replays()
    }
    fn clear(&self) -> StoreFuture<'_, ()> {
        self.inner.clear()
    }
    fn close(&self) -> StoreFuture<'_, ()> {
        self.inner.close()
    }
}

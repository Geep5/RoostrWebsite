//! A store that stops answering must not silently delete objects from the
//! view: the read fails loudly, and the next read replays what the failure
//! left owing (live on iOS Safari: a space list missing rows for the rest of
//! the session after a caught read error dropped the object from the retry
//! set).

#![cfg(not(target_family = "wasm"))]

mod backend_support;
mod support;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use backend_support::{signed_in, sqlite};
use roostr_core::Backend;
use roostr_replica_store::*;
use support::{ENGINE_TURN, block_on, create, encode};

/// The replica's SQLite store, except that while `wedged` is set reading
/// `flaky`'s inputs fails as a wedged IndexedDB does.
struct FlakyStore {
    inner: Arc<SqliteChangeStore>,
    wedged: AtomicBool,
}

impl ChangeStore for FlakyStore {
    fn object_inputs(&self, object_id: String) -> StoreFuture<'_, ObjectInputs> {
        if object_id == "flaky" && self.wedged.load(Ordering::SeqCst) {
            return Box::pin(async {
                Err(StoreError::Unavailable(
                    "local storage stopped responding (read); reload the page".into(),
                ))
            });
        }
        self.inner.object_inputs(object_id)
    }

    fn add_changes(&self, records: Vec<ChangeRecord>) -> StoreFuture<'_, usize> {
        self.inner.add_changes(records)
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
    fn object_ids(&self) -> StoreFuture<'_, Vec<String>> {
        self.inner.object_ids()
    }
    fn change_counts(&self) -> StoreFuture<'_, HashMap<String, u64>> {
        self.inner.change_counts()
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
        self.inner.set_cursor(cursor, replay_groups)
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

#[test]
fn a_store_that_stops_answering_is_retried_not_silently_dropped() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let keys = roostr_nostr::Keys::generate();
        let engine = roostr_engine::Engine::native().unwrap();
        let inner = sqlite(dir.path()).await;
        let mut records = Vec::new();
        for (id, name) in [("good", "loads"), ("flaky", "the other space")] {
            records.push(
                encode(&engine, create(&keys, id, "task", name, &[], 100))
                    .await
                    .0,
            );
        }
        inner.add_changes(records).await.unwrap();
        let store = Arc::new(FlakyStore {
            inner,
            wedged: AtomicBool::new(true),
        });
        let backend = signed_in(dir.path(), store.clone(), &[], &keys).await;

        // Loud, not partial: the caller learns the view is incomplete.
        let error = backend.objects().await.unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<StoreError>(),
                Some(StoreError::Unavailable(_))
            ),
            "{error:#}"
        );

        // The next attempt replays what the failure left owing.
        store.wedged.store(false, Ordering::SeqCst);
        let mut names: Vec<String> = backend
            .objects()
            .await
            .unwrap()
            .into_iter()
            .filter(|o| o.id == "good" || o.id == "flaky")
            .map(|o| o.name)
            .collect();
        names.sort();
        assert_eq!(names, ["loads", "the other space"]);
        backend.stop();
    });
}

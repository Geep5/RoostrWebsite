//! A store that stops answering must not silently delete objects from the
//! view: the read fails loudly, and the next read replays what the failure
//! left owing (live on iOS Safari: a space list missing rows for the rest of
//! the session after a caught read error dropped the object from the retry
//! set). A store that answers slowly must not hold the first read for the
//! whole vault's replay: what is not prepared in time is prepared in the
//! background, and a reload resumes from the memo written so far.

#![cfg(not(target_family = "wasm"))]

mod backend_support;
mod support;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use backend_support::{signed_in, sqlite};
use futures::StreamExt;
use roostr_core::Backend;
use roostr_core::backend::BackendEvent;
use roostr_replica_store::*;
use support::{ENGINE_TURN, block_on, create, encode, eventually};

/// The replica's SQLite store, except that while `wedged` is set reading
/// `flaky`'s inputs fails as a wedged IndexedDB does, and every read of an
/// object's inputs takes `delay_ms` (a slow disk) and is counted.
struct FlakyStore {
    inner: Arc<SqliteChangeStore>,
    wedged: AtomicBool,
    delay_ms: AtomicU64,
    /// Every object whose inputs were read, in order.
    input_reads: parking_lot::Mutex<Vec<String>>,
}

impl FlakyStore {
    fn new(inner: Arc<SqliteChangeStore>, wedged: bool, delay_ms: u64) -> Self {
        Self {
            inner,
            wedged: AtomicBool::new(wedged),
            delay_ms: AtomicU64::new(delay_ms),
            input_reads: parking_lot::Mutex::new(Vec::new()),
        }
    }
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
        self.input_reads.lock().push(object_id.clone());
        let delay = Duration::from_millis(self.delay_ms.load(Ordering::SeqCst));
        Box::pin(async move {
            roostr_nostr::sleep(delay).await;
            self.inner.object_inputs(object_id).await
        })
    }

    fn add_changes(&self, records: Vec<ChangeRecord>) -> StoreFuture<'_, usize> {
        self.inner.add_changes(records)
    }
    fn import(&self, batch: ImportBatch) -> StoreFuture<'_, usize> {
        self.inner.import(batch)
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
        let store = Arc::new(FlakyStore::new(inner, true, 0));
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

/// `n` tasks `t000`.. (`"task 0"`..) in a fresh store in `dir`, with no
/// replay memo: what the first open after an import holds.
async fn tasks_vault(
    dir: &std::path::Path,
    keys: &roostr_nostr::Keys,
    n: usize,
) -> Arc<SqliteChangeStore> {
    let engine = roostr_engine::Engine::native().unwrap();
    let inner = sqlite(dir).await;
    let mut records = Vec::new();
    for i in 0..n {
        let task = create(
            keys,
            &format!("t{i:03}"),
            "task",
            &format!("task {i}"),
            &[],
            100,
        );
        records.push(encode(&engine, task).await.0);
    }
    inner.add_changes(records).await.unwrap();
    inner
}

/// The vault's tasks the backend serves now.
async fn tasks(backend: &roostr_replica::ReplicaBackend) -> HashSet<String> {
    backend
        .objects()
        .await
        .unwrap()
        .into_iter()
        .filter(|o| o.type_key == "task" && o.name.starts_with("task "))
        .map(|o| o.id)
        .collect()
}

const WAIT: Duration = Duration::from_secs(60);

#[test]
fn a_slow_vault_opens_at_once_and_prepares_the_rest_in_the_background() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let keys = roostr_nostr::Keys::generate();
        const N: usize = 150;
        let inner = tasks_vault(dir.path(), &keys, N).await;
        // 20 ms a read: the whole vault takes three seconds to replay.
        let store = Arc::new(FlakyStore::new(inner.clone(), false, 20));
        let backend = signed_in(dir.path(), store.clone(), &[], &keys).await;
        let mut events = backend.events();

        let started = Instant::now();
        let first = tasks(&backend).await;
        assert!(
            started.elapsed() < Duration::from_millis(2_500),
            "the first read waited for the whole vault: {:?}",
            started.elapsed()
        );
        assert!(
            !first.is_empty() && first.len() < N,
            "{} of {N}",
            first.len()
        );
        let status = backend.status();
        let (prepared, total) = status.preparing.expect("still preparing");
        assert_eq!(total, N as u64);
        assert!(prepared < total);
        assert!(!status.bootstrapped, "a vault being prepared is not loaded");

        // The rest lands in the background; the stream says when it is whole.
        let prepared = async {
            while let Some(event) = events.next().await {
                if matches!(event, Ok(BackendEvent::Prepared)) {
                    return true;
                }
            }
            false
        };
        let timeout = async {
            roostr_nostr::sleep(WAIT).await;
            false
        };
        futures::pin_mut!(prepared, timeout);
        let prepared = futures::future::select(prepared, timeout)
            .await
            .factor_first()
            .0;
        assert!(prepared, "no Prepared event");
        assert_eq!(tasks(&backend).await.len(), N);
        assert_eq!(backend.status().preparing, None);
        // Each task was replayed once.
        let reads = store.input_reads.lock().clone();
        let mut seen = HashSet::new();
        for id in reads.iter().filter(|id| id.starts_with('t')) {
            assert!(seen.insert(id.clone()), "{id} replayed twice");
        }
        backend.stop();

        // Reopened, the memo serves every task: nothing to prepare.
        let again = Arc::new(FlakyStore::new(inner, false, 20));
        let reopened = signed_in(dir.path(), again.clone(), &[], &keys).await;
        assert_eq!(tasks(&reopened).await.len(), N);
        assert_eq!(reopened.status().preparing, None);
        assert!(
            !again
                .input_reads
                .lock()
                .iter()
                .any(|id| id.starts_with('t')),
            "a task was replayed again"
        );
        reopened.stop();
    });
}

#[test]
fn a_reload_mid_preparation_resumes_from_the_memo_it_wrote() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let keys = roostr_nostr::Keys::generate();
        const N: usize = 200;
        let inner = tasks_vault(dir.path(), &keys, N).await;
        let store = Arc::new(FlakyStore::new(inner.clone(), false, 20));
        let backend = signed_in(dir.path(), store.clone(), &[], &keys).await;
        tasks(&backend).await;
        let midway = eventually(WAIT, async || {
            backend
                .status()
                .preparing
                .is_none_or(|(prepared, _)| prepared >= 120)
        })
        .await;
        assert!(midway);
        // A reload: the preparation stops where it is.
        backend.stop();
        roostr_nostr::sleep(Duration::from_millis(300)).await;
        let before: HashSet<String> = store
            .input_reads
            .lock()
            .iter()
            .filter(|id| id.starts_with('t'))
            .cloned()
            .collect();
        assert!(
            before.len() >= 120 && before.len() < N,
            "{} prepared before the reload",
            before.len()
        );

        let again = Arc::new(FlakyStore::new(inner, false, 20));
        let reopened = signed_in(dir.path(), again.clone(), &[], &keys).await;
        let whole = eventually(WAIT, async || tasks(&reopened).await.len() == N).await;
        assert!(whole, "the reopened vault never had every task");
        let after: Vec<String> = again
            .input_reads
            .lock()
            .iter()
            .filter(|id| id.starts_with('t'))
            .cloned()
            .collect();
        let resumed: HashSet<&String> = after.iter().collect();
        assert_eq!(resumed.len(), after.len(), "a task was replayed twice");
        // Only what the first session had not written to the memo yet
        // (written every 50 objects, and as it stopped) is replayed again.
        let replayed_again = resumed.iter().filter(|id| before.contains(**id)).count();
        assert!(
            replayed_again == 0,
            "{replayed_again} task(s) the first session prepared were replayed again"
        );
        assert_eq!(before.len() + resumed.len(), N);
        reopened.stop();
    });
}

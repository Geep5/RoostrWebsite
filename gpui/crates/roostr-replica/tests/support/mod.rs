//! Shared fixtures for the sync integration tests: a host that records what
//! sync tells it, a replica (native engine + SQLite store + relay sync),
//! changes committed the way the backend commits them, and a child process
//! for a second replica (the native engine - and its receive session - is
//! one per process).
#![allow(dead_code)]

pub mod relay;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use roostr_engine::{Change, Engine, Op};
use roostr_nostr::Keys;
use roostr_nostr::spacekeys::{InMemorySpaceKeyring, KeyFile};
use roostr_replica::local::{InMemoryLocalKv, LocalKv};
use roostr_replica::sync::{Keyring, RelaySync, SyncHost, SyncStatus};
use roostr_replica_store::{ChangeRecord, ChangeStore, PendingPublish, SqliteChangeStore};
use serde_json::json;

/// The native engine's receive session is process-global: tests that run
/// a replica take turns.
pub static ENGINE_TURN: Mutex<()> = Mutex::new(());

#[derive(Default)]
pub struct TestHost {
    pub objects: Mutex<HashSet<String>>,
    pub statuses: Mutex<Vec<SyncStatus>>,
    pub space_keys: Mutex<usize>,
    pub join_requests: Mutex<usize>,
    pub vanished: Mutex<Vec<String>>,
    pub passes: Mutex<Vec<(bool, bool)>>,
    /// object id → space id.
    pub spaces: Mutex<HashMap<String, String>>,
}

impl SyncHost for TestHost {
    fn on_objects(&self, ids: Vec<String>) {
        self.objects.lock().extend(ids);
    }
    fn on_status(&self, status: SyncStatus) {
        self.statuses.lock().push(status);
    }
    fn on_space_key(&self) {
        *self.space_keys.lock() += 1;
    }
    fn on_space_vanished(&self, space_id: String) {
        self.vanished.lock().push(space_id);
    }
    fn space_gone(&self, _space_id: &str) -> bool {
        false
    }
    fn space_of(&self, object_id: &str) -> String {
        self.spaces
            .lock()
            .get(object_id)
            .cloned()
            .unwrap_or_default()
    }
    fn on_history_pass(&self, complete: bool, reachable: bool) {
        self.passes.lock().push((complete, reachable));
    }
    fn on_join_request(&self) {
        *self.join_requests.lock() += 1;
    }
}

pub struct Replica {
    pub engine: Engine,
    pub keys: Keys,
    pub store: Arc<SqliteChangeStore>,
    pub host: Arc<TestHost>,
    pub keyring: Arc<InMemorySpaceKeyring>,
    pub local: Arc<InMemoryLocalKv>,
    pub sync: RelaySync,
}

impl Replica {
    pub fn new(keys: Keys, relays: Vec<String>, store: SqliteChangeStore) -> Replica {
        Self::with_keyring(
            keys,
            relays,
            store,
            InMemorySpaceKeyring::new(KeyFile::default()),
        )
    }

    pub fn with_keyring(
        keys: Keys,
        relays: Vec<String>,
        store: SqliteChangeStore,
        keyring: InMemorySpaceKeyring,
    ) -> Replica {
        let engine = Engine::native().expect("native engine");
        let store = Arc::new(store);
        let host = Arc::new(TestHost::default());
        let keyring = Arc::new(keyring);
        let local = Arc::new(InMemoryLocalKv::new());
        let sync = RelaySync::new(
            engine.clone(),
            store.clone() as Arc<dyn ChangeStore>,
            keys.clone(),
            relays,
            keyring.clone() as Arc<dyn Keyring>,
            local.clone() as Arc<dyn LocalKv>,
            host.clone() as Arc<dyn SyncHost>,
        );
        Replica {
            engine,
            keys,
            store,
            host,
            keyring,
            local,
            sync,
        }
    }

    pub async fn memory(keys: Keys, relays: Vec<String>) -> Replica {
        Self::new(keys, relays, SqliteChangeStore::in_memory().await.unwrap())
    }

    /// Commit a change the way the backend does (atomic local change +
    /// personal obligation), then hand it to sync.
    pub async fn commit(&self, change: Change) -> String {
        let (record, bytes) = encode(&self.engine, change).await;
        let id = record.id.clone();
        let object_id = record.object_id.clone();
        self.store
            .add_local_change(
                record,
                PendingPublish {
                    key: id.clone(),
                    object_id: object_id.clone(),
                    change_id: id.clone(),
                    bytes: bytes.clone(),
                    space_id: None,
                    key_id: None,
                    base: None,
                    events: None,
                },
            )
            .await
            .unwrap();
        self.sync
            .publish(bytes, id.clone(), object_id, None)
            .await
            .unwrap();
        id
    }

    /// Every stored change id.
    pub async fn change_ids(&self) -> HashSet<String> {
        let mut ids = HashSet::new();
        for object in self.store.object_ids().await.unwrap() {
            for change in self.store.changes_for(object).await.unwrap() {
                ids.insert(change.id);
            }
        }
        ids
    }

    /// The replayed state of one object.
    pub async fn state(&self, object_id: &str) -> Option<serde_json::Value> {
        let input = self
            .store
            .object_inputs(object_id.to_string())
            .await
            .unwrap();
        let changes = input
            .changes
            .iter()
            .map(|c| serde_json::from_str(&c.json).unwrap())
            .collect();
        self.engine
            .replay_from_base(&roostr_engine::StateInputs {
                bases: input.bases.into_iter().map(|b| b.bytes).collect(),
                changes,
                checkpoint: input.checkpoint.map(|c| c.bytes),
                pending: Vec::new(),
                own: Vec::new(),
            })
            .await
            .unwrap()
    }
}

/// A change creating `object_id` of `type_key` named `name`, plus extra fields.
pub fn create(
    keys: &Keys,
    object_id: &str,
    type_key: &str,
    name: &str,
    extra: &[(&str, serde_json::Value)],
    timestamp: i64,
) -> Change {
    let mut ops = vec![
        Op::ObjectCreate {
            type_key: type_key.into(),
        },
        Op::FieldSet {
            key: "name".into(),
            value: Some(json!({"stringValue": name})),
        },
    ];
    for (key, value) in extra {
        ops.push(Op::FieldSet {
            key: (*key).into(),
            value: Some(value.clone()),
        });
    }
    Change {
        id: String::new(),
        object_id: object_id.into(),
        parent_ids: Vec::new(),
        ops,
        timestamp,
        author: keys.author_id(),
        snapshot: None,
        b: None,
    }
}

/// Address, encode and decode a change as the backend's commit does.
pub async fn encode(engine: &Engine, mut change: Change) -> (ChangeRecord, Vec<u8>) {
    change.id = engine.change_id(&change).await.unwrap();
    let bytes = engine.encode_change(&change).await.unwrap();
    let decoded = engine.decode_change(&bytes).await.unwrap();
    assert_eq!(decoded.id, change.id);
    (
        ChangeRecord {
            id: change.id.clone(),
            object_id: change.object_id.clone(),
            bytes: bytes.clone(),
            json: serde_json::to_string(&decoded).unwrap(),
            b: None,
            rebased: false,
        },
        bytes,
    )
}

pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    async_io::block_on(future)
}

/// Poll `check` until it holds or `limit` passes.
pub async fn eventually(limit: Duration, mut check: impl AsyncFnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if check().await {
            return true;
        }
        if start.elapsed() > limit {
            return false;
        }
        roostr_nostr::sleep(Duration::from_millis(50)).await;
    }
}

/// Run `test` (an `#[ignore]`d test of this binary) in a child process
/// with `env`; its stdout lines.
pub fn spawn_child(test: &str, env: &[(&str, String)]) -> std::process::Child {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            test,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    for (key, value) in env {
        command.env(key, value);
    }
    command.spawn().unwrap()
}

/// The child's `RESULT <json>` line.
pub fn child_result(child: std::process::Child) -> serde_json::Value {
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "child failed:\n{stdout}");
    stdout
        .lines()
        .find_map(|line| line.split_once("RESULT ").map(|(_, json)| json))
        .map(|json| serde_json::from_str(json).unwrap())
        .unwrap_or_else(|| panic!("child printed no result:\n{stdout}"))
}

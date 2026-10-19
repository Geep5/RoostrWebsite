//! A [`ReplicaBackend`] on a SQLite store and a device-local file in a test
//! directory, with an explicit relay list (empty = offline).
#![allow(dead_code)]

use std::sync::Arc;

use roostr_core::Backend;
use roostr_core::backend::JsonMap;
use roostr_replica::local::{FileLocalKv, LocalKv};
use roostr_replica::{RELAYS, ReplicaBackend, ReplicaParts};
use roostr_replica_store::{ChangeStore, SqliteChangeStore};
use serde_json::{Value, json};

pub fn params(value: Value) -> JsonMap {
    value.as_object().cloned().unwrap()
}

pub async fn sqlite(dir: &std::path::Path) -> Arc<SqliteChangeStore> {
    Arc::new(
        SqliteChangeStore::open(dir.join("replica.sqlite"))
            .await
            .unwrap(),
    )
}

/// A backend over `store`, signed out, with `relays` configured.
pub fn backend_on(
    dir: &std::path::Path,
    store: Arc<dyn ChangeStore>,
    relays: &[&str],
) -> ReplicaBackend {
    let kv = Arc::new(FileLocalKv::open(dir.join("local.json")).unwrap());
    // A stored empty relay list is the user's choice of none: offline.
    kv.set(RELAYS, Some(&serde_json::to_string(relays).unwrap()));
    ReplicaBackend::new(ReplicaParts {
        engine: roostr_engine::Engine::native().unwrap(),
        store,
        kv,
        label: "test replica".into(),
    })
}

/// `backend_on` signed in as `keys`.
pub async fn signed_in(
    dir: &std::path::Path,
    store: Arc<dyn ChangeStore>,
    relays: &[&str],
    keys: &roostr_nostr::Keys,
) -> ReplicaBackend {
    let backend = backend_on(dir, store, relays);
    backend
        .mutate(
            "nostr_key_import",
            params(json!({ "key": keys.secret_hex() })),
        )
        .await
        .unwrap();
    backend
}

//! The bundled catalog (`bootstrap_space_defaults`) converges only once
//! this replica holds what the relays hold. Rehearsal finding: a fresh or
//! half-synced web replica seeded at startup, before its first history
//! pass, and re-wrote and re-published the vault's whole bundled catalog on
//! every load (453 relay events over 3 loads). Offline it still seeds at once.

#![cfg(not(target_family = "wasm"))]

mod backend_support;
mod support;

use std::sync::Arc;
use std::time::Duration;

use backend_support::{params, signed_in, sqlite};
use roostr_core::Backend;
use roostr_nostr::{Keys, kinds};
use roostr_replica_store::{ChangeRecord, ChangeStore, SqliteChangeStore};
use serde_json::json;
use support::relay::TestRelay;
use support::{ENGINE_TURN, block_on, create, encode, eventually};

const WAIT: Duration = Duration::from_secs(30);

/// The space this vault holds; its bundled catalog is what seeding writes.
async fn personal_space(keys: &Keys) -> ChangeRecord {
    let engine = roostr_engine::Engine::native().unwrap();
    encode(
        &engine,
        create(keys, "personal", "channel", "Personal", &[], 100),
    )
    .await
    .0
}

/// A half-synced replica: the space arrived, its bundled catalog not yet
/// (an interrupted first load).
async fn half_synced(dir: &std::path::Path, keys: &Keys) -> Arc<SqliteChangeStore> {
    let store = sqlite(dir).await;
    store
        .add_changes(vec![personal_space(keys).await])
        .await
        .unwrap();
    store
}

fn changes_on(relay: &TestRelay) -> usize {
    relay
        .world()
        .events
        .iter()
        .filter(|e| e.kind == kinds::CHANGE)
        .count()
}

#[test]
fn a_fresh_replica_that_syncs_existing_defaults_publishes_zero_seeding_changes() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    block_on(async {
        // The vault on the relay: a space another device seeded.
        let seeder_dir = tempfile::tempdir().unwrap();
        let seeder_store = half_synced(seeder_dir.path(), &keys).await;
        let seeder = signed_in(
            seeder_dir.path(),
            seeder_store.clone(),
            &[relay.url.as_str()],
            &keys,
        )
        .await;
        seeder
            .mutate("bootstrap_space_defaults", params(json!({})))
            .await
            .unwrap();
        assert!(
            eventually(WAIT, async || seeder_store
                .pending_publishes()
                .await
                .unwrap()
                .is_empty())
            .await,
            "the seeder never published its catalog"
        );
        seeder.stop();
        drop(seeder);
        let vault = changes_on(&relay);
        assert!(vault > 20, "{vault} catalog events");

        let dir = tempfile::tempdir().unwrap();
        let store = half_synced(dir.path(), &keys).await;
        let backend = signed_in(dir.path(), store.clone(), &[relay.url.as_str()], &keys).await;
        backend.start().await.unwrap();
        assert!(
            eventually(WAIT, async || backend.status().bootstrapped).await,
            "the first history walk never completed"
        );
        // Seeding runs once the walk ends; give its plan time to land.
        roostr_nostr::sleep(Duration::from_secs(2)).await;
        backend.await_idle().await;
        assert_eq!(store.pending_publishes().await.unwrap(), []);
        assert_eq!(
            changes_on(&relay),
            vault,
            "seeding re-published the catalog"
        );
        // The catalog is there - synced, not re-written.
        assert!(!backend.relations().await.unwrap().is_empty());
        backend.stop();
    });
}

/// Seeds with no relay answering: the obligations stay pending.
async fn seeds_offline(relays: &[&str]) {
    let keys = Keys::generate();
    let dir = tempfile::tempdir().unwrap();
    let store = half_synced(dir.path(), &keys).await;
    let backend = signed_in(dir.path(), store.clone(), relays, &keys).await;
    backend.start().await.unwrap();
    assert!(
        eventually(WAIT, async || store
            .pending_publishes()
            .await
            .unwrap()
            .len()
            > 20)
        .await,
        "the first run never seeded"
    );
    assert!(!backend.relations().await.unwrap().is_empty());
    backend.stop();
}

#[test]
fn offline_the_first_run_still_seeds() {
    let _turn = ENGINE_TURN.lock();
    // Nothing listens on port 1: no relay is reachable.
    block_on(seeds_offline(&["ws://127.0.0.1:1"]));
}

#[test]
fn with_no_relay_configured_the_first_run_still_seeds() {
    let _turn = ENGINE_TURN.lock();
    block_on(seeds_offline(&[]));
}

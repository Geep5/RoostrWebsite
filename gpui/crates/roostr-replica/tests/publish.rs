//! Publishing is an obligation, never part of the write (the Svelte host's
//! `scripts/sync-publish.test.ts`): `publish` stores the obligation BEFORE
//! the network is touched, so an unreachable relay defers the change
//! instead of failing the write; a change already published is not queued.
#![cfg(not(target_family = "wasm"))]

mod support;

use roostr_nostr::Keys;
use roostr_replica_store::ChangeStore;
use support::{ENGINE_TURN, Replica, block_on, create, encode};

/// A relay address nothing listens on.
fn dead_relay() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("ws://{}", listener.local_addr().unwrap())
}

#[test]
fn an_unreachable_relay_defers_the_publish_instead_of_failing_the_write() {
    let _turn = ENGINE_TURN.lock();
    let keys = Keys::generate();
    block_on(async {
        let r = Replica::memory(keys.clone(), vec![dead_relay()]).await;
        let change = create(&keys, "task", "page", "hello from mobile", &[], 100);
        let (record, bytes) = encode(&r.engine, change).await;
        r.sync
            .publish(bytes, record.id.clone(), record.object_id, None)
            .await
            .unwrap();
        // Stored, unsent: start() re-offers every stored obligation.
        let owed: Vec<String> = r
            .store
            .pending_publishes()
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.change_id)
            .collect();
        assert_eq!(owed, std::slice::from_ref(&record.id));
        assert!(!r.store.is_published(record.id).await.unwrap());
        r.sync.stop();
    });
}

#[test]
fn an_already_published_change_is_not_queued_again() {
    let _turn = ENGINE_TURN.lock();
    let keys = Keys::generate();
    block_on(async {
        let r = Replica::memory(keys.clone(), vec![dead_relay()]).await;
        let change = create(&keys, "task", "page", "hello from mobile", &[], 100);
        let (record, bytes) = encode(&r.engine, change).await;
        r.store.mark_published(record.id.clone()).await.unwrap();
        r.sync
            .publish(bytes, record.id, record.object_id, None)
            .await
            .unwrap();
        assert!(r.store.pending_publishes().await.unwrap().is_empty());
        r.sync.stop();
    });
}

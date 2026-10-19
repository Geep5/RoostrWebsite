//! The owner's kind-5 h-deletion of a shared space is the signal members'
//! devices vanish it on: sync reports it only when the space's owner signed
//! it. (What the backend does with the signal - and with vanished spaces -
//! is covered in `backend::tests`.)

#![cfg(not(target_family = "wasm"))]

mod support;

use std::time::Duration;

use roostr_nostr::{EventTemplate, Keys, kinds};
use roostr_replica::sync::SharedSpaceInfo;
use support::relay::TestRelay;
use support::{ENGINE_TURN, Replica, block_on, eventually};

const WAIT: Duration = Duration::from_secs(30);

/// The spaces a device signed in as `local` reports vanished after its
/// relay served one h-deletion of space "space" signed by `signer`.
/// `owner`: the space's imported owner (`None` = this identity's own).
fn signalled(local: &Keys, owner: Option<&Keys>, signer: &Keys, writers: &[&Keys]) -> Vec<String> {
    let relay = TestRelay::start();
    block_on(async {
        let replica = Replica::memory(local.clone(), vec![relay.url.clone()]).await;
        let key_hex = hex::encode([4u8; 32]);
        let tag = replica
            .engine
            .blind_space(&key_hex, "space:space")
            .await
            .unwrap();
        relay.insert(signer.sign(EventTemplate {
            kind: kinds::DELETION,
            created_at: roostr_nostr::now_s(),
            tags: vec![
                vec!["h".into(), tag],
                vec!["k".into(), "1078".into()],
                vec!["k".into(), "1079".into()],
            ],
            content: "space deleted".into(),
        }));
        replica.sync.start().await.unwrap();
        replica.sync.await_first_walk().await;
        replica
            .sync
            .set_shared_spaces(vec![SharedSpaceInfo {
                space_id: "space".into(),
                key_hex,
                key_id: 1,
                writers: writers.iter().map(|k| k.pubkey().to_string()).collect(),
                owner: owner.map(|k| k.pubkey().to_string()),
            }])
            .await
            .unwrap();
        assert!(
            eventually(WAIT, async || replica.sync.stats().events > 0).await,
            "the space stream never delivered the deletion"
        );
        replica.sync.await_idle().await;
        let mut vanished = replica.host.vanished.lock().clone();
        replica.sync.stop();
        // Live and history may both deliver it; the backend applies it once.
        vanished.dedup();
        vanished
    })
}

#[test]
fn only_the_space_owners_h_deletion_vanishes_a_shared_space() {
    let _turn = ENGINE_TURN.lock();
    let owner = Keys::from_secret_hex(&hex::encode([1u8; 32])).unwrap();
    let member = Keys::from_secret_hex(&hex::encode([2u8; 32])).unwrap();
    let writers = [&owner, &member];
    assert_eq!(
        signalled(&member, Some(&owner), &member, &writers),
        Vec::<String>::new()
    );
    assert_eq!(
        signalled(&member, Some(&owner), &owner, &writers),
        ["space"]
    );
    // No imported owner: the space is this identity's own.
    assert_eq!(
        signalled(&owner, None, &member, &writers),
        Vec::<String>::new()
    );
    assert_eq!(signalled(&owner, None, &owner, &writers), ["space"]);
}

//! Direct-line signals (kind 25079) on the relay path: the live subscription
//! hands the host this account's and its spaces' writers' signals with their
//! scope and drops strangers'; a signal goes out sealed for its scope, a
//! hello in every scope.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::time::Duration;

use roostr_nostr::{Event, EventTemplate, Keys, kinds, nip44};
use roostr_replica::sync::SharedSpaceInfo;
use serde_json::{Value, json};
use support::relay::TestRelay;
use support::{ENGINE_TURN, Replica, block_on, eventually};

const WAIT: Duration = Duration::from_secs(30);

fn space(owner: &Keys) -> SharedSpaceInfo {
    SharedSpaceInfo {
        space_id: "sp".into(),
        key_hex: hex::encode([5u8; 32]),
        key_id: 1,
        writers: vec![owner.pubkey().to_string()],
        owner: Some(owner.pubkey().to_string()),
    }
}

fn payload(from: &str) -> Value {
    json!({"v":1,"type":"offer","from":from,"to":"phone","session":"s1","role":"host",
        "sdp":"v=0","candidate":"","mid":"","ts":1})
}

fn self_key(keys: &Keys) -> [u8; 32] {
    nip44::conversation_key(keys, keys.pubkey()).unwrap()
}

fn space_key() -> [u8; 32] {
    [5u8; 32]
}

fn signal(signer: &Keys, key: &[u8; 32], tags: Vec<Vec<String>>, body: &Value) -> Event {
    signer.sign(EventTemplate {
        kind: kinds::SIGNAL,
        created_at: roostr_nostr::now_s() + 1,
        tags,
        content: nip44::encrypt(&body.to_string(), key).unwrap(),
    })
}

#[test]
fn signals_reach_the_host_with_their_scope_and_strangers_are_dropped() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let me = Keys::generate();
    let host = Keys::generate();
    let stranger = Keys::generate();
    block_on(async {
        let r = Replica::memory(me.clone(), vec![relay.url.clone()]).await;
        let sp = space(&host);
        let tag = r.engine.blind_space(&sp.key_hex, "space:sp").await.unwrap();
        r.sync.set_shared_spaces(vec![sp]).await.unwrap();
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        let h = vec![vec!["h".to_string(), tag]];

        // Own, personal: its own scope.
        relay.insert(signal(&me, &self_key(&me), vec![], &payload("own")));
        // A stranger cannot speak in this account's own scope.
        relay.insert(signal(
            &stranger,
            &self_key(&stranger),
            vec![],
            &payload("x1"),
        ));
        // The space's writer: the space's scope.
        relay.insert(signal(&host, &space_key(), h.clone(), &payload("host")));
        // Someone holding the key but not a writer: dropped.
        relay.insert(signal(&stranger, &space_key(), h.clone(), &payload("x2")));
        // This account in the space: still its own scope.
        relay.insert(signal(&me, &space_key(), h, &payload("own-in-space")));

        let all = eventually(WAIT, async || r.host.signals.lock().len() >= 3).await;
        assert!(all, "three signals arrived: {:?}", r.host.signals.lock());
        roostr_nostr::sleep(Duration::from_millis(300)).await;
        let heard: Vec<(String, Option<String>)> = r
            .host
            .signals
            .lock()
            .iter()
            .map(|(p, space)| (p["from"].as_str().unwrap().to_string(), space.clone()))
            .collect();
        assert_eq!(
            heard,
            [
                ("own".to_string(), None),
                ("host".to_string(), Some("sp".to_string())),
                ("own-in-space".to_string(), None),
            ]
        );
        r.sync.stop();
    });
}

#[test]
fn a_signal_goes_out_sealed_for_its_scope_and_a_hello_in_every_scope() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let me = Keys::generate();
    let owner = Keys::generate();
    block_on(async {
        let r = Replica::memory(me.clone(), vec![relay.url.clone()]).await;
        let sp = space(&owner);
        let tag = r.engine.blind_space(&sp.key_hex, "space:sp").await.unwrap();
        r.sync.set_shared_spaces(vec![sp]).await.unwrap();
        let signals = || -> Vec<Event> {
            let w = relay.world();
            w.events
                .iter()
                .filter(|e| e.kind == kinds::SIGNAL)
                .cloned()
                .collect()
        };
        let open = |e: &Event, key: &[u8; 32]| -> Value {
            serde_json::from_str(&nip44::decrypt(&e.content, key).unwrap()).unwrap()
        };

        let answer = json!({"v":1,"type":"answer","from":"phone","to":"host-1","session":"s1",
            "role":"viewer","sdp":"v=0","candidate":"","mid":"","ts":1});
        r.sync
            .publish_signal(answer.clone(), Some("sp".into()))
            .await
            .unwrap();
        let sent = signals();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].pubkey, me.pubkey());
        assert_eq!(sent[0].tags, vec![vec!["h".to_string(), tag.clone()]]);
        assert_eq!(open(&sent[0], &space_key()), answer);

        r.sync.publish_signal(answer.clone(), None).await.unwrap();
        let sent = signals();
        assert_eq!(sent.len(), 2);
        assert!(
            sent[1].tags.is_empty(),
            "the account's own scope has no tags"
        );
        assert_eq!(open(&sent[1], &self_key(&me)), answer);

        assert!(
            r.sync
                .publish_signal(answer.clone(), Some("unknown".into()))
                .await
                .is_err(),
            "no key for that space"
        );

        let hello = json!({"v":1,"type":"hello","from":"phone","to":"","session":"",
            "role":"viewer","sdp":"","candidate":"","mid":"","ts":2});
        r.sync.publish_signal(hello.clone(), None).await.unwrap();
        let sent = signals();
        assert_eq!(sent.len(), 4, "personal + the space");
        let scopes: Vec<bool> = sent[2..].iter().map(|e| e.tags.is_empty()).collect();
        assert_eq!(scopes, [true, false]);
        assert_eq!(open(&sent[2], &self_key(&me)), hello);
        assert_eq!(open(&sent[3], &space_key()), hello);
    });
}

//! Relay sync end to end: native engine + SQLite store + an in-process
//! relay. A second replica runs in a child process (`child_*` tests, run
//! only when their environment is set): the native engine's receive session
//! is one per process, as it is one per app.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::time::Duration;

use roostr_engine::{Op, SealRequest};
use roostr_nostr::spacekeys::SpaceKeyring;
use roostr_nostr::{Event, EventTemplate, Keys, kinds};
use roostr_replica::sync::invite::{JoinLink, list_join_requests, send_join_request};
use roostr_replica::sync::{SharedSpaceInfo, VANISH_LOG_ID};
use roostr_replica_store::{ChangeStore, HeldQuery, SqliteChangeStore};
use serde_json::{Value, json};
use support::relay::TestRelay;
use support::{
    ENGINE_TURN, Replica, block_on, child_result, create, encode, eventually, spawn_child,
};

const WAIT: Duration = Duration::from_secs(30);

/// Seal and sign `change` as this identity's outbox would, at `created_at`.
async fn sealed_events(
    replica: &Replica,
    change: roostr_engine::Change,
    created_at: u64,
) -> (String, Vec<Event>) {
    let (record, bytes) = encode(&replica.engine, change).await;
    let keys = &replica.keys;
    let conversation =
        hex::encode(roostr_nostr::nip44::conversation_key(keys, keys.pubkey()).unwrap());
    let sealed = replica
        .engine
        .seal(&SealRequest {
            change: &bytes,
            conversation_key: &conversation,
            secret: Some(&keys.secret_hex()),
            space: None,
            object_id: &record.object_id,
            base: None,
            nonce: None,
        })
        .await
        .unwrap();
    let events = sealed
        .parts
        .into_iter()
        .map(|part| {
            keys.sign(EventTemplate {
                kind: kinds::CHANGE,
                created_at,
                tags: part.tags,
                content: part.content,
            })
        })
        .collect();
    (record.id, events)
}

/// Relay history: 300 small changes over 300 seconds, a chunked one and the
/// vanish ledger; returns every change id.
fn seed_history(relay: &TestRelay, keys: &Keys) -> Vec<String> {
    block_on(async {
        let seeder = Replica::memory(keys.clone(), Vec::new()).await;
        let base = roostr_nostr::now_s() - 5_000;
        let mut ids = Vec::new();
        for i in 0..300u64 {
            let change = create(
                keys,
                &format!("note-{i:03}"),
                "page",
                &format!("Note {i}"),
                &[],
                1_000 + i as i64,
            );
            let (id, events) = sealed_events(&seeder, change, base + i).await;
            ids.push(id);
            events.into_iter().for_each(|e| relay.insert(e));
        }
        // ~120k characters of body: sealed into several `c` parts.
        let body = "lorem ipsum dolor sit amet ".repeat(4_500);
        let big = create(
            keys,
            "big-note",
            "page",
            "Big",
            &[("content", json!({"stringValue": body}))],
            2_000,
        );
        let (id, events) = sealed_events(&seeder, big, base + 400).await;
        assert!(events.len() > 1, "the big change is chunked");
        ids.push(id);
        events.into_iter().for_each(|e| relay.insert(e));
        let ledger = create(keys, VANISH_LOG_ID, "vanish_log", "", &[], 3_000);
        let (id, events) = sealed_events(&seeder, ledger, base + 500).await;
        ids.push(id);
        events.into_iter().for_each(|e| relay.insert(e));
        ids
    })
}

async fn fresh_walk(keys: &Keys, relay: &TestRelay) -> Replica {
    let replica = Replica::memory(keys.clone(), vec![relay.url.clone()]).await;
    replica.sync.start().await.unwrap();
    replica.sync.await_first_walk().await;
    replica
}

#[test]
fn history_completes_by_negentropy_and_by_paged_walk() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    let ids = seed_history(&relay, &keys);
    let relay_events = relay.world().events.len();

    block_on(async {
        // NIP-77.
        let a = fresh_walk(&keys, &relay).await;
        let held = a.change_ids().await;
        for id in &ids {
            assert!(held.contains(id), "negentropy walk missed {id}");
        }
        assert_eq!(a.host.passes.lock().last(), Some(&(true, true)));
        assert!(a.store.bootstrapped().await.unwrap());
        let stats = a.sync.stats();
        assert_eq!((stats.decrypt_failures, stats.decode_failures), (0, 0));
        assert_eq!(stats.imported, ids.len() as u64);
        // The store keeps each change's received protobuf wire bytes.
        for object in a.store.object_ids().await.unwrap() {
            for record in a.store.changes_for(object).await.unwrap() {
                assert_eq!(
                    record.bytes.first(),
                    Some(&0x0a),
                    "{} is protobuf",
                    record.id
                );
            }
        }
        {
            let world = relay.world();
            assert!(world.neg_opens > 0, "reconciled");
            assert_eq!(world.paged_reqs, 0, "no paged fallback while NIP-77 works");
        }
        // Every relay event is held under the self scope: the next reconcile needs nothing.
        let held_events = a
            .store
            .held_events(
                String::new(),
                HeldQuery {
                    kinds: kinds::DAG.to_vec(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(held_events.len(), relay_events);
        let big = a
            .state("big-note")
            .await
            .expect("chunked change reassembled");
        assert_eq!(big["typeKey"], "page");
        assert!(a.sync.cursor() > 0);
        a.sync.stop();

        // Paged fallback: the relay answers NEG-OPEN with a NOTICE.
        relay.world().no_nip77 = true;
        let b = fresh_walk(&keys, &relay).await;
        let held = b.change_ids().await;
        for id in &ids {
            assert!(held.contains(id), "paged walk missed {id}");
        }
        assert_eq!(b.host.passes.lock().last(), Some(&(true, true)));
        assert!(b.store.bootstrapped().await.unwrap());
        // 300+ change events: the first page, then two more by `until`.
        assert!(relay.world().paged_reqs >= 2, "paged by until");
        assert!(b.state("big-note").await.is_some());
        // The floor is cleared once the bootstrap walk completed.
        assert_eq!(b.store.bootstrap_floor().await.unwrap(), None);
        b.sync.stop();
    });
}

#[test]
fn outbox_survives_restart() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("ws://{}", listener.local_addr().unwrap())
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replica.sqlite");
    let keys = Keys::generate();

    let (change_id, signed) = block_on(async {
        // Offline: the write commits, the outbox seals and persists the
        // signed events, every send fails.
        let a = Replica::new(
            keys.clone(),
            vec![dead.clone()],
            SqliteChangeStore::open(&path).await.unwrap(),
        );
        a.sync.start().await.unwrap();
        let id = a
            .commit(create(
                &keys,
                "offline-note",
                "page",
                "Written offline",
                &[],
                10,
            ))
            .await;
        let sealed = eventually(WAIT, async || {
            a.store
                .pending(id.clone())
                .await
                .unwrap()
                .is_some_and(|p| p.events.is_some())
        })
        .await;
        assert!(sealed, "the sealed events are persisted before any send");
        assert!(!a.store.is_published(id.clone()).await.unwrap());
        let pending = a.store.pending(id.clone()).await.unwrap().unwrap();
        a.sync.stop();
        a.store.close().await.unwrap();
        let events: Vec<Event> = pending
            .events
            .unwrap()
            .into_iter()
            .map(|v| serde_json::from_value(v).unwrap())
            .collect();
        (id, events)
    });
    assert!(relay.world().events.is_empty());
    assert!(
        signed.iter().all(|e| e.tag("expiration").is_none()),
        "personal events never expire"
    );

    block_on(async {
        // Restart online: start() re-offers the stored obligation and the
        // exact signed events go out.
        let b = Replica::new(
            keys.clone(),
            vec![relay.url.clone()],
            SqliteChangeStore::open(&path).await.unwrap(),
        );
        b.sync.start().await.unwrap();
        let published = eventually(WAIT, async || {
            b.store.is_published(change_id.clone()).await.unwrap()
        })
        .await;
        assert!(published, "the obligation was published after the restart");
        let on_relay: Vec<String> = relay.world().events.iter().map(|e| e.id.clone()).collect();
        for event in &signed {
            assert!(
                on_relay.contains(&event.id),
                "the persisted event {} was sent byte for byte",
                event.id
            );
        }
        assert!(b.store.pending(change_id.clone()).await.unwrap().is_none());
        assert!(b.store.pending_publishes().await.unwrap().is_empty());
        b.sync.await_idle().await;
        assert_eq!(b.sync.pending(), 0);
        b.sync.stop();
    });
}

#[test]
fn same_key_replicas_converge() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    block_on(async {
        let a = Replica::memory(keys.clone(), vec![relay.url.clone()]).await;
        a.sync.start().await.unwrap();
        a.sync.await_first_walk().await;
        let x = a
            .commit(create(&keys, "from-a", "page", "Written on A", &[], 100))
            .await;
        a.sync.await_idle().await;
        assert!(a.store.is_published(x.clone()).await.unwrap());

        // B: same key, own process, empty store.
        let child = spawn_child(
            "child_same_key",
            &[
                ("ROOSTR_CHILD_RELAY", relay.url.clone()),
                ("ROOSTR_CHILD_SECRET", keys.secret_hex()),
                ("ROOSTR_CHILD_WANT", x.clone()),
            ],
        );
        let result = child_result(child);
        assert_eq!(result["saw"], true, "B imported A's change: {result}");
        assert_eq!(result["name"], "Written on A");
        let y = result["wrote"].as_str().unwrap().to_string();

        // A hears B's write live.
        let heard = eventually(WAIT, async || a.change_ids().await.contains(&y)).await;
        assert!(heard, "A received B's change live");
        assert!(a.host.objects.lock().contains("from-b"));
        let state = a.state("from-b").await.unwrap();
        assert_eq!(state["fields"]["name"]["stringValue"], "Written on B");
        a.sync.stop();
    });
}

#[test]
#[ignore = "child process of same_key_replicas_converge"]
fn child_same_key() {
    let Ok(url) = std::env::var("ROOSTR_CHILD_RELAY") else {
        return;
    };
    let keys = Keys::from_secret_hex(&std::env::var("ROOSTR_CHILD_SECRET").unwrap()).unwrap();
    let want = std::env::var("ROOSTR_CHILD_WANT").unwrap();
    block_on(async {
        let b = Replica::memory(keys.clone(), vec![url]).await;
        b.sync.start().await.unwrap();
        b.sync.await_first_walk().await;
        let saw = b.change_ids().await.contains(&want);
        let name = b
            .state("from-a")
            .await
            .map(|s| s["fields"]["name"]["stringValue"].clone())
            .unwrap_or(Value::Null);
        let wrote = b
            .commit(create(&keys, "from-b", "page", "Written on B", &[], 200))
            .await;
        b.sync.await_idle().await;
        assert!(b.store.is_published(wrote.clone()).await.unwrap());
        println!(
            "RESULT {}",
            json!({"saw": saw, "name": name, "wrote": wrote})
        );
        b.sync.stop();
    });
}

/// The account's keyring backup as the daemon publishes it (nostrsync.ts
/// `publishKeyring`): its key file, NIP-44 encrypted to itself.
fn keyring_backup(keys: &Keys, channels: Value, created_at: u64) -> Event {
    let ck = roostr_nostr::nip44::conversation_key(keys, keys.pubkey()).unwrap();
    let file = json!({"version": 1, "channels": channels, "localPubkey": keys.pubkey()});
    keys.sign(EventTemplate {
        kind: kinds::KEYRING,
        created_at,
        tags: vec![vec!["d".into(), kinds::KEYRING_D.into()]],
        content: roostr_nostr::nip44::encrypt(&file.to_string(), &ck).unwrap(),
    })
}

/// A space made on another device of the account (the daemon) reaches this
/// replica through the keyring backup, at start and live; a key this replica
/// holds at the same or a newer keyId is kept.
#[test]
fn keyring_backup_installs_the_accounts_space_keys() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    let viv = "a".repeat(64);
    let ours = "b".repeat(64);
    let theirs = "c".repeat(64);
    let later = "d".repeat(64);
    relay.insert(keyring_backup(
        &keys,
        json!({
            "viv": {"key": viv, "keyId": 1, "createdAt": 1},
            "kept": {"key": theirs, "keyId": 1, "createdAt": 1},
        }),
        1_000,
    ));
    block_on(async {
        let a = Replica::memory(keys.clone(), vec![relay.url.clone()]).await;
        a.keyring
            .set(
                "kept",
                roostr_nostr::spacekeys::SpaceKey {
                    key: ours.clone(),
                    key_id: 2,
                    created_at: 1,
                    owner: None,
                },
            )
            .unwrap();
        a.sync.start().await.unwrap();
        a.sync.await_first_walk().await;
        let installed = eventually(WAIT, async || a.keyring.get("viv").unwrap().is_some()).await;
        assert!(installed, "the backup's space key was installed at start");
        assert_eq!(a.keyring.get("viv").unwrap().unwrap().key, viv);
        assert_eq!(
            a.keyring.get("kept").unwrap().unwrap().key,
            ours,
            "a newer local keyId wins"
        );
        assert!(
            *a.host.space_keys.lock() > 0,
            "the host was told to refresh its spaces"
        );

        // A rotation on the other device arrives live.
        relay.insert(keyring_backup(
            &keys,
            json!({"viv": {"key": later, "keyId": 2, "createdAt": 2}}),
            roostr_nostr::now_s() + 1,
        ));
        let rotated = eventually(WAIT, async || {
            a.keyring
                .get("viv")
                .unwrap()
                .is_some_and(|k| k.key == later)
        })
        .await;
        assert!(rotated, "a newer keyId in a live backup replaced the key");
        a.sync.stop();
    });
}

/// `members` as a channel stores it: `[{npub, role}]`.
fn members(entries: &[(&str, &str)]) -> Value {
    let items: Vec<Value> = entries
        .iter()
        .map(|(npub, role)| json!({"mapValue": {"entries": {"npub": {"stringValue": npub}, "role": {"stringValue": role}}}}))
        .collect();
    roostr_engine::pack_core_value_maps(&json!({"valuesValue": {"items": items}}))
}

#[test]
fn shared_space_invite_between_identities() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let owner = Keys::generate();
    let member = Keys::generate();
    block_on(async {
        let a = Replica::memory(owner.clone(), vec![relay.url.clone()]).await;
        let key = a.keyring.ensure("garden").unwrap();
        {
            let mut spaces = a.host.spaces.lock();
            spaces.insert("garden".into(), "garden".into());
            spaces.insert("tomatoes".into(), "garden".into());
        }
        a.sync.start().await.unwrap();
        a.sync.await_first_walk().await;
        a.sync
            .set_shared_spaces(vec![SharedSpaceInfo {
                space_id: "garden".into(),
                key_hex: key.key.clone(),
                key_id: 1,
                writers: vec![owner.pubkey().into(), member.pubkey().into()],
                owner: None,
            }])
            .await
            .unwrap();
        let channel = create(
            &owner,
            "garden",
            "channel",
            "Garden",
            &[("members", members(&[(&member.npub(), "writer")]))],
            100,
        );
        a.commit(channel).await;
        let page = create(
            &owner,
            "tomatoes",
            "page",
            "Tomatoes",
            &[("channel", json!({"stringValue": "garden"}))],
            101,
        );
        let page_id = a.commit(page).await;
        // Shared objects owe a publish under the space key too.
        let shared_key = format!("garden/1/{page_id}");
        let published = eventually(WAIT, async || {
            a.store.is_published(shared_key.clone()).await.unwrap()
        })
        .await;
        assert!(published, "published under the space key");
        a.sync
            .send_invite_wraps(
                "garden".into(),
                key.key.clone(),
                1,
                "Garden".into(),
                vec![member.pubkey().into()],
            )
            .await;

        // The member, in its own process: key by gift wrap, space by #h,
        // a write back, and a join request.
        let child = spawn_child(
            "child_space_member",
            &[
                ("ROOSTR_CHILD_RELAY", relay.url.clone()),
                ("ROOSTR_CHILD_SECRET", member.secret_hex()),
                ("ROOSTR_CHILD_OWNER", owner.pubkey().to_string()),
                ("ROOSTR_CHILD_PARENT", page_id.clone()),
            ],
        );
        let result = child_result(child);
        assert_eq!(
            result["key"], key.key,
            "the gift-wrapped key arrived: {result}"
        );
        assert_eq!(
            result["page"], "Tomatoes",
            "the space's history synced: {result}"
        );
        let wrote = result["wrote"].as_str().unwrap().to_string();

        // The owner authorizes and imports the member's write from the space stream.
        let heard = eventually(WAIT, async || a.change_ids().await.contains(&wrote)).await;
        assert!(heard, "the owner imported the member's write");
        let state = a.state("tomatoes").await.unwrap();
        assert_eq!(state["fields"]["name"]["stringValue"], "Tomatoes (ripe)");
        // And collected the join request.
        let knocked = eventually(WAIT, async || *a.host.join_requests.lock() > 0).await;
        assert!(knocked, "join request recorded");
        let requests = list_join_requests(a.local.as_ref());
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].requester, member.pubkey());
        assert_eq!(requests[0].key, format!("garden/{}", member.pubkey()));
        a.sync.stop();
    });
}

#[test]
#[ignore = "child process of shared_space_invite_between_identities"]
fn child_space_member() {
    let Ok(url) = std::env::var("ROOSTR_CHILD_RELAY") else {
        return;
    };
    let keys = Keys::from_secret_hex(&std::env::var("ROOSTR_CHILD_SECRET").unwrap()).unwrap();
    let owner = std::env::var("ROOSTR_CHILD_OWNER").unwrap();
    let parent = std::env::var("ROOSTR_CHILD_PARENT").unwrap();
    block_on(async {
        let b = Replica::memory(keys.clone(), vec![url.clone()]).await;
        {
            let mut spaces = b.host.spaces.lock();
            spaces.insert("garden".into(), "garden".into());
            spaces.insert("tomatoes".into(), "garden".into());
        }
        b.sync.start().await.unwrap();
        // start() fetched the gift wraps.
        let got = eventually(WAIT, async || b.keyring.get("garden").unwrap().is_some()).await;
        assert!(got, "space key imported from the gift wrap");
        assert!(*b.host.space_keys.lock() > 0);
        let entry = b.keyring.get("garden").unwrap().unwrap();
        assert_eq!(entry.owner.as_deref(), Some(owner.as_str()));
        b.sync.await_first_walk().await;
        // What the backend's refreshShared does on `on_space_key`.
        b.sync
            .set_shared_spaces(vec![SharedSpaceInfo {
                space_id: "garden".into(),
                key_hex: entry.key.clone(),
                key_id: entry.key_id as i64,
                writers: vec![owner.clone(), keys.pubkey().into()],
                owner: Some(owner.clone()),
            }])
            .await
            .unwrap();
        let synced = eventually(WAIT, async || b.state("tomatoes").await.is_some()).await;
        assert!(synced, "the space backfilled");
        let page = b.state("tomatoes").await.unwrap()["fields"]["name"]["stringValue"].clone();
        // A member's write to the shared page.
        let edit = roostr_engine::Change {
            id: String::new(),
            object_id: "tomatoes".into(),
            parent_ids: vec![parent.clone()],
            ops: vec![Op::FieldSet {
                key: "name".into(),
                value: Some(json!({"stringValue": "Tomatoes (ripe)"})),
            }],
            timestamp: 300,
            author: keys.author_id(),
            snapshot: None,
            b: None,
        };
        let wrote = b.commit(edit).await;
        let shared_key = format!("garden/{}/{wrote}", entry.key_id);
        let published = eventually(WAIT, async || {
            b.store.is_published(shared_key.clone()).await.unwrap()
        })
        .await;
        assert!(published, "the member published under the space key");
        send_join_request(
            &keys,
            &JoinLink {
                space: "garden".into(),
                owner: roostr_nostr::bech32::encode("npub", &hex::decode(&owner).unwrap()),
                relays: vec![url],
            },
        )
        .await
        .unwrap();
        println!(
            "RESULT {}",
            json!({"key": entry.key, "page": page, "wrote": wrote})
        );
        b.sync.stop();
    });
}

//! The replica backend: offline (identity, spaces, pages, blocks, queries,
//! the replay memo across a restart, logout), and two devices of one key
//! meeting over a relay. Native only: the engine is one per process, so
//! replicas take turns (`ENGINE_TURN`) and never run side by side.

#![cfg(not(target_family = "wasm"))]

mod support;

use std::sync::Arc;

use futures::StreamExt as _;
use roostr_core::Backend;
use roostr_core::backend::{BackendEvent, JsonMap};
use roostr_replica::local::{FileLocalKv, LocalKv};
use roostr_replica::{RELAYS, ReplicaBackend, ReplicaParts};
use roostr_replica_store::{ChangeStore, SqliteChangeStore};
use serde_json::{Value, json};

fn params(value: Value) -> JsonMap {
    value.as_object().cloned().unwrap()
}

async fn open(dir: &std::path::Path) -> (ReplicaBackend, Arc<SqliteChangeStore>) {
    open_with(dir, &[]).await
}

async fn open_with(
    dir: &std::path::Path,
    relays: &[&str],
) -> (ReplicaBackend, Arc<SqliteChangeStore>) {
    let store = Arc::new(
        SqliteChangeStore::open(dir.join("replica.sqlite"))
            .await
            .unwrap(),
    );
    let kv = Arc::new(FileLocalKv::open(dir.join("local.json")).unwrap());
    // A stored empty relay list is the user's choice of none: offline.
    kv.set(RELAYS, Some(&serde_json::to_string(relays).unwrap()));
    let backend = ReplicaBackend::new(ReplicaParts {
        engine: roostr_engine::Engine::native().unwrap(),
        store: store.clone(),
        kv,
        label: "test replica".into(),
    });
    (backend, store)
}

fn text_of(object: &roostr_core::Object) -> String {
    serde_json::to_string(&object.blocks).unwrap()
}

#[test]
fn offline_replica_round_trip() {
    let _turn = support::ENGINE_TURN.lock();
    async_io::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (backend, store) = open(dir.path()).await;

        // No identity: the gate shows, reads refuse.
        let settings = backend.settings().await.unwrap();
        assert!(!settings.has_key);
        assert!(backend.objects().await.is_err());

        // "Create account": the gate imports a fresh nsec.
        let keys = roostr_nostr::Keys::generate();
        backend
            .mutate("nostr_key_import", params(json!({ "key": keys.nsec() })))
            .await
            .unwrap();
        let settings = backend.settings().await.unwrap();
        assert!(settings.has_key);
        assert_eq!(settings.npub.as_deref(), Some(keys.npub().as_str()));
        assert_eq!(settings.author_id, keys.author_id());
        assert!(settings.relays.is_empty());

        let mut events = backend.events();
        assert_eq!(events.next().await.unwrap().unwrap(), BackendEvent::Hello);

        let space = backend
            .mutate("channel_create", params(json!({ "name": "Personal" })))
            .await
            .unwrap();
        let space_id = space["id"].as_str().unwrap().to_string();
        let spaces = backend.channels().await.unwrap();
        let personal = spaces.iter().find(|s| s.id == space_id).unwrap();
        assert_eq!(personal.name, "Personal");
        assert_eq!(personal.owner, "", "a space created here is owned here");
        assert!(
            roostr_nostr::spacekeys::SpaceKeyring::get(&*backend.keyring(), &space_id)
                .unwrap()
                .is_some()
        );

        let page = backend
            .mutate(
                "create",
                params(json!({ "name": "Replica page", "fields": { "channel": { "stringValue": space_id } } })),
            )
            .await
            .unwrap();
        let page_id = page["id"].as_str().unwrap().to_string();
        backend
            .mutate(
                "block_add",
                params(json!({
                    "object_id": page_id,
                    "block": { "content": { "text": { "text": "typed offline" } } },
                    "target_id": "",
                    "position": 0,
                })),
            )
            .await
            .unwrap();

        let object = backend.object(&page_id).await.unwrap();
        assert_eq!(
            roostr_core::types::field_str(&object.fields, "name"),
            "Replica page"
        );
        assert!(text_of(&object).contains("typed offline"));

        let summaries = backend.objects().await.unwrap();
        assert!(
            summaries
                .iter()
                .any(|s| s.id == page_id && s.channel_id == space_id)
        );

        let found = backend
            .query(&json!({ "textQuery": "Replica page", "limit": 10 }))
            .await
            .unwrap();
        assert!(found.records.iter().any(|r| r.id == page_id), "{found:?}");

        // A second edit lands in the warm query cache too.
        backend
            .mutate(
                "set_field",
                params(json!({ "object_id": page_id, "key": "name", "value": { "stringValue": "Renamed page" } })),
            )
            .await
            .unwrap();
        let found = backend
            .query(&json!({ "textQuery": "Renamed", "limit": 10 }))
            .await
            .unwrap();
        assert!(found.records.iter().any(|r| r.id == page_id), "{found:?}");

        let digest = backend.sync_digest().await.unwrap();
        assert!(digest["changes"].as_u64().unwrap() >= 3);
        let serving = backend
            .serving(std::slice::from_ref(&page_id))
            .await
            .unwrap();
        assert_eq!(serving[&page_id]["reason"], "unserved");

        // Every local change owes a publication (no relay took it).
        let pending = store.pending_publishes().await.unwrap();
        assert!(!pending.is_empty());

        // Restart on the same files: the vault comes back as it was.
        backend.stop();
        drop(events);
        drop(backend);
        let (backend, _store) = open(dir.path()).await;
        let object = backend.object(&page_id).await.unwrap();
        assert!(text_of(&object).contains("typed offline"));
        let again = backend.sync_digest().await.unwrap();
        assert_eq!(again["digest"], digest["digest"]);

        // Logout refuses while changes are unpublished - once the reopened
        // replica's background seeding has finished saving ("Wait for the
        // current save" until then).
        let account = backend.account().unwrap();
        let mut refused = None;
        let settled = support::eventually(std::time::Duration::from_secs(10), async || {
            let error = account.logout().await.unwrap_err().to_string();
            let saving = error.contains("Wait for the current save");
            refused = Some(error);
            !saving
        })
        .await;
        let refused = refused.unwrap();
        assert!(
            settled && refused.contains("Unpublished changes"),
            "{refused}"
        );
        let (nsec, hex) = account.export_key().unwrap();
        assert_eq!((nsec, hex), (keys.nsec(), keys.secret_hex()));
        backend.stop();
    });
}

/// Settings → "Switch to Lite": refused while this device holds changes not
/// sent yet (they would be lost); otherwise the local copy goes and the next
/// start is lite, still signed in.
#[test]
fn switch_to_lite_keeps_unsent_changes_and_otherwise_starts_lite() {
    use roostr_core::lite::LiteControl;
    let _turn = support::ENGINE_TURN.lock();
    async_io::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (backend, store) = open(dir.path()).await;
        let keys = roostr_nostr::Keys::generate();
        backend
            .mutate("nostr_key_import", params(json!({ "key": keys.nsec() })))
            .await
            .unwrap();
        let kv = backend.kv();
        // A browser that holds the whole vault.
        kv.set(roostr_replica::HOLDING, Some("full"));
        // Offline: the page is written here and not sent.
        let page = backend
            .mutate("create", params(json!({ "name": "Unsent" })))
            .await
            .unwrap();
        let page_id = page["id"].as_str().unwrap().to_string();
        // A save still settling (its shared-space refresh) answers "wait".
        let switch = async || loop {
            match backend.switch_to_lite().await {
                Err(e) if e.to_string().starts_with("Wait for the current save") => {
                    async_io::Timer::after(std::time::Duration::from_millis(20)).await;
                }
                other => return other,
            }
        };
        let refused = switch().await.unwrap_err().to_string();
        assert!(refused.contains("not sent yet"), "{refused}");
        assert!(
            backend.object(&page_id).await.is_ok(),
            "the unsent page stays"
        );
        assert_eq!(kv.get(roostr_replica::HOLDING).as_deref(), Some("full"));

        // Nothing owed: the copy goes, the next start is lite, the key stays.
        for pending in store.pending_publishes().await.unwrap() {
            store.mark_published(pending.key).await.unwrap();
        }
        switch().await.unwrap();
        assert!(store.object_ids().await.unwrap().is_empty());
        assert_eq!(kv.get(roostr_replica::HOLDING).as_deref(), Some("lite"));
        assert!(
            roostr_replica::identity::load(&*kv).is_some(),
            "still signed in"
        );
        // The same running app (the iPhone, no page reload) opens again lite
        // and empty: nothing of the dropped copy lingers in memory.
        let listed = backend.objects().await.unwrap();
        assert!(
            listed.iter().all(|o| o.id != page_id),
            "the dropped copy is gone in place"
        );
        assert!(backend.status().lite.is_some(), "reopened lite");
    });
}

/// Device reminders read each recurring object's `repeat.next`.
#[test]
fn upcoming_occurrences_follow_repeat_next() {
    let _turn = support::ENGINE_TURN.lock();
    async_io::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (backend, _store) = open(dir.path()).await;
        let keys = roostr_nostr::Keys::generate();
        backend
            .mutate("nostr_key_import", params(json!({ "key": keys.nsec() })))
            .await
            .unwrap();
        let space = backend
            .mutate("channel_create", params(json!({ "name": "Personal" })))
            .await
            .unwrap();
        let channel = space["id"].as_str().unwrap().to_string();
        let mut ids = Vec::new();
        for name in ["Water plants", "Plain page"] {
            let page = backend
                .mutate(
                    "create",
                    params(json!({ "name": name, "fields": { "channel": { "stringValue": channel } } })),
                )
                .await
                .unwrap();
            ids.push(page["id"].as_str().unwrap().to_string());
        }
        let rule = roostr_core::types::RepeatRule {
            freq: roostr_core::types::RepeatFreq::Hour,
            interval: 1,
            weekdays: Vec::new(),
            monthly: "date".into(),
            times: Vec::new(),
            window: None,
            tz: "UTC".into(),
            anchor_ms: None,
        };
        let set = backend
            .mutate(
                "repeat_set",
                params(
                    json!({ "object_id": ids[0], "rule": serde_json::to_value(&rule).unwrap() }),
                ),
            )
            .await
            .unwrap();
        let next = set["next"].as_i64().unwrap();

        let upcoming = backend.upcoming_occurrences(next - 1, 64).await.unwrap();
        assert_eq!(upcoming.len(), 1, "{upcoming:?}");
        let occurrence = &upcoming[0];
        assert_eq!(
            (
                occurrence.object_id.as_str(),
                occurrence.name.as_str(),
                occurrence.at
            ),
            (ids[0].as_str(), "Water plants", next)
        );
        assert!(!occurrence.agent_owned);
        assert_eq!(occurrence.id(), format!("{}:{next}", ids[0]));
        // Past occurrences are not reminders.
        assert!(
            backend
                .upcoming_occurrences(next, 64)
                .await
                .unwrap()
                .is_empty()
        );
        backend.stop();
    });
}

async fn block_text(backend: &ReplicaBackend, id: &str) -> Option<String> {
    let object = backend.object(id).await.ok()?;
    Some(text_of(&object))
}

#[test]
fn two_devices_of_one_key_meet_over_a_relay() {
    let _turn = support::ENGINE_TURN.lock();
    let relay = support::relay::TestRelay::start();
    let keys = roostr_nostr::Keys::generate();
    let limit = std::time::Duration::from_secs(30);
    async_io::block_on(async {
        // Device A: a fresh account writes a space, a page and its text.
        let a_dir = tempfile::tempdir().unwrap();
        let (a, a_store) = open_with(a_dir.path(), &[relay.url.as_str()]).await;
        a.mutate("nostr_key_import", params(json!({ "key": keys.nsec() })))
            .await
            .unwrap();
        let space = a
            .mutate("channel_create", params(json!({ "name": "Personal" })))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let page = a
            .mutate(
                "create",
                params(json!({ "name": "Shared page", "fields": { "channel": { "stringValue": space } } })),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        a.mutate(
            "block_add",
            params(json!({
                "object_id": page,
                "block": { "content": { "text": { "text": "written on A" } } },
                "target_id": "",
                "position": 0,
            })),
        )
        .await
        .unwrap();
        // Every obligation reaches the relay.
        assert!(
            support::eventually(limit, async || a_store
                .pending_publishes()
                .await
                .unwrap()
                .is_empty())
            .await,
            "A never published"
        );
        let a_digest = a.sync_digest().await.unwrap();
        a.stop();
        drop(a);

        // Device B: same key, empty store - the history walk brings it all.
        let b_dir = tempfile::tempdir().unwrap();
        let (b, _b_store) = open_with(b_dir.path(), &[relay.url.as_str()]).await;
        b.mutate(
            "nostr_key_import",
            params(json!({ "key": keys.secret_hex() })),
        )
        .await
        .unwrap();
        b.start().await.unwrap();
        assert!(
            support::eventually(limit, async || {
                block_text(&b, &page)
                    .await
                    .is_some_and(|t| t.contains("written on A"))
            })
            .await,
            "B never saw A's page"
        );
        let spaces = b.channels().await.unwrap();
        assert_eq!(spaces.iter().filter(|s| s.name == "Personal").count(), 1);
        assert!(
            support::eventually(limit, async || b.status().bootstrapped).await,
            "B's first history walk never completed"
        );
        // A's whole change set arrived (B may add its own catalog changes on top).
        let b_digest = b.sync_digest().await.unwrap();
        assert!(b_digest["changes"].as_u64() >= a_digest["changes"].as_u64());

        // B edits; A, back, gets it.
        b.mutate(
            "set_field",
            params(json!({ "object_id": page, "key": "name", "value": { "stringValue": "Edited on B" } })),
        )
        .await
        .unwrap();
        b.await_idle().await;
        b.stop();
        drop(b);

        let (a, _a_store) = open_with(a_dir.path(), &[relay.url.as_str()]).await;
        a.start().await.unwrap();
        assert!(
            support::eventually(limit, async || {
                a.object(&page).await.is_ok_and(|o| {
                    roostr_core::types::field_str(&o.fields, "name") == "Edited on B"
                })
            })
            .await,
            "A never saw B's edit"
        );
        a.stop();
    });
}

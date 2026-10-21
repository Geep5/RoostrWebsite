//! Device records end to end (glonOdin `core/device.odin`, Settings →
//! "Your devices"): two replicas of one throwaway key each write theirs and
//! each sees both.
//!
//! ```sh
//! # the in-process test relay
//! cargo test -p roostr-replica --test devices -- --nocapture
//! # a local RoostrRelay (never the production one)
//! DEVICES_RELAY=ws://127.0.0.1:7790 cargo test -p roostr-replica --test devices -- --nocapture
//! ```
//!
//! Each replica runs in its own process (the native engine is one per
//! process) on its own throwaway directory:
//! 1. A full replica A starts on the empty vault: its record (full, 100%)
//!    appears and the personal directory lists it; it writes a few pages,
//!    and a second write within 30 minutes plans nothing.
//! 2. A lite replica B lists both records - A's fetched through the
//!    personal directory - with B lite at the share it holds; B renames A.
//! 3. A again: it lists B (lite, B's share) and keeps the name B gave it -
//!    its own write plans nothing: the auto name never replaces a person's,
//!    and last_seen is still the first run's.
#![cfg(not(target_family = "wasm"))]

mod backend_support;
mod support;

use std::path::PathBuf;
use std::time::Duration;

use backend_support::{params, signed_in, sqlite};
use roostr_core::Backend;
use roostr_core::devices::{DeviceRecord, record_id};
use roostr_nostr::Keys;
use roostr_replica::{HOLDING, ReplicaBackend};
use serde_json::{Value, json};
use support::relay::TestRelay;
use support::{block_on, child_result, eventually, spawn_child};

const WAIT: Duration = Duration::from_secs(180);
const STUDIO: &str = "Studio Mac";

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

/// Every device record the replica holds.
async fn devices(backend: &ReplicaBackend) -> Vec<DeviceRecord> {
    backend
        .query(&json!({ "type": "device" }))
        .await
        .map(|result| result.records.iter().map(DeviceRecord::from_row).collect())
        .unwrap_or_default()
}

async fn device(backend: &ReplicaBackend, id: &str) -> Option<DeviceRecord> {
    devices(backend).await.into_iter().find(|d| d.id == id)
}

async fn own_record(backend: &ReplicaBackend) -> String {
    record_id(&backend.settings().await.unwrap().device_id)
}

#[test]
fn devices_end_to_end() {
    let local = std::env::var("DEVICES_RELAY").ok();
    let test_relay = local.is_none().then(TestRelay::start);
    let url = local.unwrap_or_else(|| test_relay.as_ref().unwrap().url.clone());
    let keys = Keys::generate();
    let root = tempfile::tempdir().unwrap();
    let dir = |name: &str| root.path().join(name).display().to_string();
    let env = |extra: &[(&'static str, String)]| {
        let mut all = vec![
            ("DEVICES_RELAY_URL", url.clone()),
            ("DEVICES_KEY", keys.secret_hex()),
        ];
        all.extend_from_slice(extra);
        all
    };

    let first = child_result(spawn_child(
        "devices_full",
        &env(&[("DEVICES_DIR", dir("a")), ("DEVICES_PHASE", "first".into())]),
    ));
    println!("A, first run: {first}");
    let lite = child_result(spawn_child(
        "devices_lite",
        &env(&[
            ("DEVICES_DIR", dir("b")),
            ("DEVICES_PEER", first.to_string()),
        ]),
    ));
    println!("B (lite): {lite}");
    let again = child_result(spawn_child(
        "devices_full",
        &env(&[
            ("DEVICES_DIR", dir("a")),
            ("DEVICES_PHASE", "again".into()),
            ("DEVICES_FIRST", first.to_string()),
            ("DEVICES_PEER", lite.to_string()),
        ]),
    ));
    println!("A, again: {again}");
    assert_ne!(first["record"], lite["record"]);
    assert_eq!(first["record"], again["record"]);
}

/// Replica A, full (child of [`devices_end_to_end`]).
#[test]
#[ignore]
fn devices_full() {
    if var("DEVICES_DIR").is_empty() {
        return;
    }
    let dir = PathBuf::from(var("DEVICES_DIR"));
    let relay = var("DEVICES_RELAY_URL");
    let keys = Keys::parse(&var("DEVICES_KEY")).unwrap();
    let _ = env_logger::builder().try_init();
    block_on(async {
        std::fs::create_dir_all(&dir).unwrap();
        let store = sqlite(&dir).await;
        let backend = signed_in(&dir, store, &[&relay], &keys).await;
        backend.start().await.unwrap();
        let me = own_record(&backend).await;
        if var("DEVICES_PHASE") == "first" {
            // Written once the first pass settles; listed for lite replicas.
            let written = eventually(WAIT, async || {
                device(&backend, &me)
                    .await
                    .is_some_and(|d| !d.lite && d.percent == 100)
            })
            .await;
            assert!(written, "no device record: {:?}", devices(&backend).await);
            let listed = eventually(WAIT, async || {
                backend
                    .object("__directory__")
                    .await
                    .is_ok_and(|dir| dir.fields.contains_key(&format!("device:{me}")))
            })
            .await;
            assert!(listed, "the personal directory does not list the record");
            let record = device(&backend, &me).await.unwrap();
            assert_eq!(record.kind, "desktop");
            assert!(record.name.starts_with("Desktop app"), "{}", record.name);
            assert!(!record.name_custom);
            // A vault a lite replica holds only part of: a space of pages.
            let space = backend
                .mutate("channel_create", params(json!({ "name": "Home" })))
                .await
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string();
            for i in 0..5 {
                let fields = json!({ "channel": { "stringValue": space } });
                backend
                    .mutate(
                        "create",
                        params(json!({ "name": format!("Page {i}"), "type_key": "page", "fields": fields })),
                    )
                    .await
                    .unwrap();
            }
            // Nothing changed within 30 minutes: no write.
            assert!(!backend.write_device_record().await.unwrap());
            assert_eq!(
                device(&backend, &me).await.unwrap().last_seen,
                record.last_seen
            );
            backend.await_idle().await;
            println!(
                "RESULT {}",
                json!({ "record": me, "name": record.name, "last_seen": record.last_seen })
            );
        } else {
            let first: Value = serde_json::from_str(&var("DEVICES_FIRST")).unwrap();
            let peer: Value = serde_json::from_str(&var("DEVICES_PEER")).unwrap();
            let peer_id = peer["record"].as_str().unwrap().to_string();
            let percent = peer["percent"].as_u64().unwrap() as u8;
            let both = eventually(WAIT, async || {
                let b = device(&backend, &peer_id).await;
                let a = device(&backend, &me).await;
                b.is_some_and(|b| b.lite && b.percent == percent)
                    && a.is_some_and(|a| a.name == STUDIO && a.name_custom)
            })
            .await;
            assert!(both, "devices: {:?}", devices(&backend).await);
            // Its own write plans nothing: the auto name differs, but a
            // person named it; and 30 minutes have not passed.
            assert!(!backend.write_device_record().await.unwrap());
            let record = device(&backend, &me).await.unwrap();
            assert_eq!(record.name, STUDIO);
            assert_eq!(record.last_seen, first["last_seen"].as_i64().unwrap());
            assert_eq!(devices(&backend).await.len(), 2);
            println!(
                "RESULT {}",
                json!({ "record": me, "name": record.name, "last_seen": record.last_seen, "peer_percent": percent })
            );
        }
        backend.stop();
    });
}

/// Replica B, lite (child of [`devices_end_to_end`]).
#[test]
#[ignore]
fn devices_lite() {
    if var("DEVICES_DIR").is_empty() {
        return;
    }
    let dir = PathBuf::from(var("DEVICES_DIR"));
    let relay = var("DEVICES_RELAY_URL");
    let keys = Keys::parse(&var("DEVICES_KEY")).unwrap();
    let peer: Value = serde_json::from_str(&var("DEVICES_PEER")).unwrap();
    let peer_id = peer["record"].as_str().unwrap().to_string();
    let _ = env_logger::builder().try_init();
    block_on(async {
        std::fs::create_dir_all(&dir).unwrap();
        let store = sqlite(&dir).await;
        let backend = signed_in(&dir, store, &[&relay], &keys).await;
        backend.kv().set(HOLDING, Some("lite"));
        backend.start().await.unwrap();
        let lite = backend.lite().unwrap();
        let me = own_record(&backend).await;
        // Both: A's fetched through the personal directory, its own written
        // once the directories are in.
        let both = eventually(WAIT, async || {
            let a = device(&backend, &peer_id).await;
            let b = device(&backend, &me).await;
            a.is_some_and(|a| !a.lite && a.percent == 100 && a.name == peer["name"])
                && b.is_some_and(|b| b.lite)
        })
        .await;
        assert!(both, "devices: {:?}", devices(&backend).await);
        assert!(backend.status().lite.is_some(), "still lite");
        // The record says the share this device holds.
        let mut percent = 0;
        let settled = eventually(WAIT, async || {
            backend.write_device_record().await.unwrap();
            let held = lite.estimate().await.unwrap().percent();
            percent = device(&backend, &me).await.unwrap().percent;
            percent == held
        })
        .await;
        assert!(
            settled,
            "record {percent}% vs estimate {:?}",
            lite.estimate().await
        );
        assert!(
            percent < 100,
            "a lite replica of a vault with unopened pages holds {percent}%: {:?} {:?}",
            lite.estimate().await,
            backend.status().lite
        );
        let record = device(&backend, &me).await.unwrap();
        assert_eq!(record.kind, "desktop");

        // A person names A.
        backend
            .mutate(
                "device_rename",
                params(json!({ "object_id": peer_id, "name": STUDIO })),
            )
            .await
            .unwrap();
        let renamed = device(&backend, &peer_id).await.unwrap();
        assert!(renamed.name == STUDIO && renamed.name_custom);
        backend.await_idle().await;
        println!("RESULT {}", json!({ "record": me, "percent": percent }));
        backend.stop();
    });
}

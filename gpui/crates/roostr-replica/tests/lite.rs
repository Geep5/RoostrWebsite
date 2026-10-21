//! Lite replicas end to end (manual; `--ignored`): a full replica writes a
//! vault, then fresh lite replicas draw it from its directories and fetch
//! only what they open or load.
//!
//! ```sh
//! # the in-process test relay
//! cargo test -p roostr-replica --test lite -- --ignored --nocapture lite_end_to_end
//! # a local RoostrRelay (never the production one)
//! LITE_RELAY=ws://127.0.0.1:7790 \
//!   cargo test -p roostr-replica --test lite -- --ignored --nocapture lite_end_to_end
//! ```
//!
//! One throwaway identity A writes, through a full replica, two spaces of
//! its own (they sync in its personal stream) and a space shared with a
//! second throwaway identity B, each with three pinned and four unpinned
//! pages. Then, each in its own process (the native engine is one per
//! process) and its own throwaway directory:
//! - A lite replica of A lists both its spaces and their pins from the
//!   directories while holding none of the pages; opening a pinned page
//!   fetches that page alone; loading a space downloads it (the personal
//!   stream, so both own spaces); "sync fully" makes it a full replica.
//! - A lite replica of B (the key arrives by invite) lists the shared space
//!   and its pins, opens one page alone, and loading the space downloads
//!   exactly that space.
#![cfg(not(target_family = "wasm"))]

mod backend_support;
mod support;

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use backend_support::{params, signed_in, sqlite};
use roostr_core::Backend;
use roostr_nostr::relay::{Relay, query_many};
use roostr_nostr::spacekeys::SpaceKeyring as _;
use roostr_nostr::{Filter, Keys, kinds};
use roostr_replica::{HOLDING, ReplicaBackend};
use roostr_replica_store::ChangeStore;
use serde_json::{Value, json};
use support::relay::TestRelay;
use support::{block_on, child_result, eventually, spawn_child};

const WAIT: Duration = Duration::from_secs(180);

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

/// The replica's logs on stderr, as `RUST_LOG` asks (`info` shows sync's).
fn logs() {
    let _ = env_logger::builder().try_init();
}

async fn mutate(backend: &ReplicaBackend, action: &str, value: Value) -> String {
    let out = backend.mutate(action, params(value)).await.unwrap();
    out.get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The directory `id` is built and lists `pins` pins (`None`: any).
async fn directory_built(backend: &ReplicaBackend, id: &str, pins: Option<usize>) -> bool {
    let Ok(dir) = backend.object(id).await else {
        return false;
    };
    let version = dir.fields.get("version").and_then(|v| v.int_value);
    let listed = dir.fields.keys().filter(|k| k.starts_with("pin:")).count();
    version == Some(1) && pins.is_none_or(|n| n == listed)
}

async fn held(store: &dyn ChangeStore) -> BTreeSet<String> {
    store.object_ids().await.unwrap().into_iter().collect()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn space<'a>(expect: &'a Value, name: &str) -> &'a Value {
    expect["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap()
}

fn pages(space: &Value) -> BTreeSet<String> {
    strings(&space["pinned"])
        .into_iter()
        .chain(strings(&space["other"]))
        .collect()
}

#[test]
#[ignore]
fn lite_end_to_end() {
    let local = std::env::var("LITE_RELAY").ok();
    let test_relay = local.is_none().then(TestRelay::start);
    let url = local.unwrap_or_else(|| test_relay.as_ref().unwrap().url.clone());
    let owner = Keys::generate();
    let member = Keys::generate();
    let root = tempfile::tempdir().unwrap();
    let dir = |name: &str| root.path().join(name).display().to_string();
    println!("relay {url}, replicas under {}", root.path().display());
    // Throwaway identities, printed so one phase can be rerun by hand.
    println!(
        "owner key {}, member key {}",
        owner.secret_hex(),
        member.secret_hex()
    );

    let writer = child_result(spawn_child(
        "lite_writer",
        &[
            ("LITE_RELAY_URL", url.clone()),
            ("LITE_KEY", owner.secret_hex()),
            ("LITE_MEMBER", member.npub()),
            ("LITE_DIR", dir("writer")),
        ],
    ));
    println!("writer (full): {writer}");
    let expect = writer.to_string();
    let lite_owner = child_result(spawn_child(
        "lite_owner",
        &[
            ("LITE_RELAY_URL", url.clone()),
            ("LITE_KEY", owner.secret_hex()),
            ("LITE_DIR", dir("lite-owner")),
            ("LITE_EXPECT", expect.clone()),
        ],
    ));
    println!("lite replica of the owner: {lite_owner}");
    let lite_member = child_result(spawn_child(
        "lite_member",
        &[
            ("LITE_RELAY_URL", url),
            ("LITE_KEY", member.secret_hex()),
            ("LITE_DIR", dir("lite-member")),
            ("LITE_EXPECT", expect),
        ],
    ));
    println!("lite replica of the member: {lite_member}");
}

/// A's full replica writes the vault (child of [`lite_end_to_end`]).
#[test]
#[ignore]
fn lite_writer() {
    if var("LITE_DIR").is_empty() {
        return;
    }
    let dir = PathBuf::from(var("LITE_DIR"));
    let relay = var("LITE_RELAY_URL");
    let keys = Keys::parse(&var("LITE_KEY")).unwrap();
    let member = var("LITE_MEMBER");
    logs();
    block_on(async {
        std::fs::create_dir_all(&dir).unwrap();
        let store = sqlite(&dir).await;
        let backend = signed_in(&dir, store.clone(), &[&relay], &keys).await;
        backend.start().await.unwrap();
        // An empty vault's first pass seeds the defaults; their repair
        // builds the personal directory.
        assert!(eventually(WAIT, async || directory_built(&backend, "__directory__", None).await).await);
        let mut spaces = Vec::new();
        for (name, shared) in [("Garden", false), ("Work", false), ("Team", true)] {
            let id = mutate(&backend, "channel_create", json!({ "name": name })).await;
            if shared {
                mutate(
                    &backend,
                    "channel_member_add",
                    json!({ "channel_id": id, "npub": member, "role": "writer" }),
                )
                .await;
            }
            let mut pinned = Vec::new();
            let mut other = Vec::new();
            for i in 0..7 {
                let mut fields = json!({ "channel": { "stringValue": id } });
                if i < 3 {
                    fields["pin_order"] = json!({ "stringValue": format!("a{i}") });
                }
                let page = mutate(
                    &backend,
                    "create",
                    json!({ "name": format!("{name} page {i}"), "type_key": "page", "fields": fields }),
                )
                .await;
                if i < 3 { &mut pinned } else { &mut other }.push(page);
            }
            spaces.push(json!({ "id": id, "name": name, "shared": shared, "pinned": pinned, "other": other }));
        }
        for space in &spaces {
            let dir_id = format!("__directory__:{}", space["id"].as_str().unwrap());
            assert!(eventually(WAIT, async || directory_built(&backend, &dir_id, Some(3)).await).await);
        }
        backend.await_idle().await;
        // The member's invite (the shared space's key) is on the relay.
        let invited = eventually(WAIT, async || {
            let member_hex = roostr_nostr::keys::npub_to_hex(&member).unwrap();
            let filter = Filter::new()
                .kinds([kinds::GIFT_WRAP])
                .tag('p', [member_hex]);
            query_many(&[Relay::new(relay.clone())], &[filter], Duration::from_secs(10))
                .await
                .is_ok_and(|wraps| !wraps.is_empty())
        })
        .await;
        assert!(invited, "no invite for the member");
        // Sharing republishes the space's earlier changes under its key, in
        // the background after the invite: wait until its stream holds still.
        let team = spaces.iter().find(|s| s["shared"] == true).unwrap();
        let team_id = team["id"].as_str().unwrap();
        let key = backend.keyring().get(team_id).unwrap().unwrap().key;
        let stream = {
            use sha2::Digest as _;
            let digest = sha2::Sha256::new()
                .chain_update(key.as_bytes())
                .chain_update(format!("space:{team_id}").as_bytes())
                .finalize();
            hex::encode(&digest[..8])
        };
        let mut last = usize::MAX;
        let settled = eventually(WAIT, async || {
            backend.await_idle().await;
            roostr_nostr::sleep(Duration::from_secs(2)).await;
            let filter = Filter::new().kinds([kinds::CHANGE]).tag('h', [stream.clone()]);
            let count = query_many(&[Relay::new(relay.clone())], &[filter], Duration::from_secs(10))
                .await
                .map_or(0, |events| events.len());
            std::mem::replace(&mut last, count) == count && count > 0
        })
        .await;
        assert!(settled, "the shared space's stream never settled");
        let objects = held(&*store).await.len();
        println!("RESULT {}", json!({ "spaces": spaces, "objects_held": objects }));
        backend.stop();
    });
}

/// A fresh lite replica on `dir`, signed in as `keys`, started.
async fn lite_replica(
    dir: &std::path::Path,
    relay: &str,
    keys: &Keys,
) -> (ReplicaBackend, std::sync::Arc<roostr_replica_store::SqliteChangeStore>) {
    std::fs::create_dir_all(dir).unwrap();
    let store = sqlite(dir).await;
    let backend = signed_in(dir, store.clone(), &[relay], keys).await;
    backend.kv().set(HOLDING, Some("lite"));
    backend.start().await.unwrap();
    (backend, store)
}

/// A lite replica of the owner (child of [`lite_end_to_end`]).
#[test]
#[ignore]
fn lite_owner() {
    if var("LITE_DIR").is_empty() {
        return;
    }
    let dir = PathBuf::from(var("LITE_DIR"));
    let relay = var("LITE_RELAY_URL");
    let keys = Keys::parse(&var("LITE_KEY")).unwrap();
    let expect: Value = serde_json::from_str(&var("LITE_EXPECT")).unwrap();
    logs();
    block_on(async {
        let (backend, store) = lite_replica(&dir, &relay, &keys).await;
        let lite = backend.lite().unwrap();
        let checked = eventually(WAIT, async || {
            backend.status().lite.is_some_and(|l| l.checked)
        })
        .await;
        assert!(checked, "the directories never arrived");
        let garden = space(&expect, "Garden");
        let work = space(&expect, "Work");
        let team = space(&expect, "Team");
        let all_pages: BTreeSet<String> = [garden, work, team].into_iter().flat_map(pages).collect();

        // Both own spaces and their pins, from the directories alone.
        let listed = eventually(WAIT, async || {
            let channels = backend.channels().await.unwrap();
            [garden, work].into_iter().all(|s| {
                channels
                    .iter()
                    .any(|c| c.id == s["id"] && c.name == s["name"] && c.pinned_ids == strings(&s["pinned"]))
            })
        })
        .await;
        assert!(listed, "spaces or pins missing: {:?}", backend.channels().await);
        let summaries = backend.objects().await.unwrap();
        for id in strings(&garden["pinned"]).iter().chain(&strings(&work["pinned"])) {
            let summary = summaries.iter().find(|s| &s.id == id).expect("pin summary");
            assert_eq!(summary.type_key, "page");
            assert!(summary.name.contains(" page "), "{}", summary.name);
        }
        let before = held(&*store).await;
        let held_pages = |set: &BTreeSet<String>| set.intersection(&all_pages).cloned().collect::<BTreeSet<_>>();
        assert!(held_pages(&before).is_empty(), "pages held before opening: {:?}", held_pages(&before));
        let status = backend.status().lite.unwrap();
        assert!(status.ready);
        let estimate_start = lite.estimate().await.unwrap();
        assert!(!estimate_start.full && estimate_start.total_objects > estimate_start.held_objects);

        // Opening a pinned page fetches that page alone.
        let target = strings(&garden["pinned"])[0].clone();
        let opened = backend.object(&target).await.unwrap();
        assert_eq!(opened.fields.get("name").and_then(|v| v.string_value.clone()).as_deref(), Some("Garden page 0"));
        let after_open = held(&*store).await;
        assert_eq!(held_pages(&after_open), BTreeSet::from([target.clone()]));
        // The spaces' properties (and types) arrive from their directories'
        // listings, still without any page.
        let defs = eventually(WAIT, async || !backend.relations().await.unwrap().is_empty()).await;
        assert!(defs, "no properties fetched from the directories");
        let relations_held = backend.relations().await.unwrap().len();
        assert_eq!(held_pages(&held(&*store).await), BTreeSet::from([target.clone()]));

        // Loading a space downloads it: these spaces sync in the personal
        // stream, so both own spaces arrive whole - with whatever else this
        // identity published there (a page written in the shared space
        // before the writer's sync treated it as shared went out personal
        // first).
        let garden_id = garden["id"].as_str().unwrap().to_string();
        lite.load_space(&garden_id).await.unwrap();
        let after_load = held(&*store).await;
        let own: BTreeSet<String> = pages(garden).union(&pages(work)).cloned().collect();
        let loaded = held_pages(&after_load);
        assert!(own.is_subset(&loaded), "missing {:?}", own.difference(&loaded));
        assert!(loaded.difference(&own).all(|id| pages(team).contains(id)));
        let full_spaces = backend.status().lite.unwrap().full_spaces;
        assert!(full_spaces.contains(&garden_id), "{full_spaces:?}");
        let counts = lite.type_counts(&garden_id);
        assert_eq!(counts.get("page"), Some(&7), "{counts:?}");
        let estimate_lite = lite.estimate().await.unwrap();

        // "Fully sync this device": a full replica from then on.
        lite.sync_fully().await.unwrap();
        let full = eventually(WAIT, async || {
            let status = backend.status();
            status.lite.is_none() && status.bootstrapped
        })
        .await;
        assert!(full, "never became full: {:?}", backend.status());
        backend.await_idle().await;
        let after_full = held(&*store).await;
        let estimate_full = lite.estimate().await.unwrap();
        assert!(estimate_full.full);
        println!(
            "RESULT {}",
            json!({
                "checked_ready": status.ready,
                "objects_held_after_directories": before.len(),
                "pages_held_after_directories": held_pages(&before).len(),
                "objects_held_after_open": after_open.len(),
                "pages_held_after_open": held_pages(&after_open).len(),
                "properties_held_before_any_load": relations_held,
                "estimate_at_start": format!(
                    "{}% held, {} of {} objects, ~{} events to download",
                    estimate_start.percent(),
                    estimate_start.held_objects,
                    estimate_start.total_objects,
                    estimate_start.missing_events
                ),
                "objects_held_after_load_space": after_load.len(),
                "pages_held_after_load_space": held_pages(&after_load).len(),
                "garden_page_count_from_directory": counts.get("page"),
                "estimate_while_lite_percent": estimate_lite.percent(),
                "objects_held_after_sync_fully": after_full.len(),
                "pages_held_after_sync_fully": held_pages(&after_full).len(),
                "full_after_sync_fully": estimate_full.full,
            })
        );
        backend.stop();
    });
}

/// A lite replica of the member of the shared space (child of
/// [`lite_end_to_end`]).
#[test]
#[ignore]
fn lite_member() {
    if var("LITE_DIR").is_empty() {
        return;
    }
    let dir = PathBuf::from(var("LITE_DIR"));
    let relay = var("LITE_RELAY_URL");
    let keys = Keys::parse(&var("LITE_KEY")).unwrap();
    let expect: Value = serde_json::from_str(&var("LITE_EXPECT")).unwrap();
    logs();
    block_on(async {
        let (backend, store) = lite_replica(&dir, &relay, &keys).await;
        let lite = backend.lite().unwrap();
        let team = space(&expect, "Team");
        let team_id = team["id"].as_str().unwrap().to_string();
        let team_pages = pages(team);
        // The invite brings the key; the space and its pins come from its directory.
        let listed = eventually(WAIT, async || {
            let channels = backend.channels().await.unwrap();
            channels
                .iter()
                .any(|c| c.id == team_id && c.pinned_ids == strings(&team["pinned"]))
        })
        .await;
        assert!(listed, "shared space or pins missing: {:?}", backend.channels().await);
        let before = held(&*store).await;
        let held_pages = |set: &BTreeSet<String>| set.intersection(&team_pages).cloned().collect::<BTreeSet<_>>();
        assert!(held_pages(&before).is_empty());

        let target = strings(&team["pinned"])[1].clone();
        backend.object(&target).await.unwrap();
        let after_open = held(&*store).await;
        assert_eq!(held_pages(&after_open), BTreeSet::from([target]));

        lite.load_space(&team_id).await.unwrap();
        let after_load = held(&*store).await;
        assert_eq!(held_pages(&after_load), team_pages);
        let status = backend.status().lite.unwrap();
        assert!(status.full_spaces.contains(&team_id), "{status:?}");
        // Only the shared space's stream: nothing of the owner's own spaces.
        let others: HashSet<String> = [space(&expect, "Garden"), space(&expect, "Work")]
            .into_iter()
            .flat_map(pages)
            .collect();
        assert!(after_load.iter().all(|id| !others.contains(id)));
        println!(
            "RESULT {}",
            json!({
                "objects_held_after_directories": before.len(),
                "pages_held_after_directories": held_pages(&before).len(),
                "objects_held_after_open": after_open.len(),
                "pages_held_after_open": held_pages(&after_open).len(),
                "objects_held_after_load_space": after_load.len(),
                "pages_held_after_load_space": held_pages(&after_load).len(),
                "team_full": status.full_spaces.contains(&team_id),
            })
        );
        backend.stop();
    });
}

/// A type's page on a lite replica: each space's own type object, listing
/// that space's objects of the type (`--ignored`, like [`lite_end_to_end`]).
/// Two spaces each hold agents; Matcherino (the reader's own) also holds a
/// pinned "All Agents" query over the agent type. Viv is first the reader's
/// own space, then a space another identity owns and shared with the
/// reader. The sidebar of Viv opens Viv's agent type, and the type page
/// lists Viv's agents once the space is downloaded - on the reader's full
/// replica and on a fresh lite one.
#[test]
#[ignore]
fn lite_type_pages() {
    let local = std::env::var("LITE_RELAY").ok();
    let test_relay = local.is_none().then(TestRelay::start);
    let url = local.unwrap_or_else(|| test_relay.as_ref().unwrap().url.clone());
    let root = tempfile::tempdir().unwrap();
    let dir = |name: &str| root.path().join(name).display().to_string();
    for shared in [false, true] {
        let reader = Keys::generate();
        let tag = if shared { "shared" } else { "own" };
        let mut expect = Vec::new();
        let mut viv = "Viv";
        if shared {
            let owner = Keys::generate();
            let written = child_result(spawn_child(
                "lite_types_writer",
                &[
                    ("LITE_RELAY_URL", url.clone()),
                    ("LITE_KEY", owner.secret_hex()),
                    ("LITE_DIR", dir(&format!("{tag}-owner"))),
                    ("LITE_SPACES", "Viv".into()),
                    ("LITE_MEMBER", reader.npub()),
                ],
            ));
            println!("owner of Viv (full): {written}");
            expect.extend(written["spaces"].as_array().unwrap().iter().cloned());
            viv = "";
        }
        let written = child_result(spawn_child(
            "lite_types_writer",
            &[
                ("LITE_RELAY_URL", url.clone()),
                ("LITE_KEY", reader.secret_hex()),
                ("LITE_DIR", dir(&format!("{tag}-writer"))),
                ("LITE_SPACES", [viv, "Matcherino"].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(",")),
                ("LITE_QUERY", "Matcherino".into()),
                ("LITE_JOINED", json!({ "spaces": expect }).to_string()),
            ],
        ));
        println!("{tag}: reader (full): {written}");
        expect.extend(written["spaces"].as_array().unwrap().iter().cloned());
        let read = child_result(spawn_child(
            "lite_types_reader",
            &[
                ("LITE_RELAY_URL", url.clone()),
                ("LITE_KEY", reader.secret_hex()),
                ("LITE_DIR", dir(&format!("{tag}-reader"))),
                ("LITE_EXPECT", json!({ "spaces": expect }).to_string()),
            ],
        ));
        println!("{tag}: reader (lite): {read}");
    }
}

/// The type definitions the store's type list reads (`fetch_defs`).
async fn type_defs(backend: &ReplicaBackend) -> Vec<roostr_core::types::QueryRow> {
    let body = json!({
        "filters": [{ "key": "typeKey", "condition": "in", "value": ["type", "relation"] }],
        "limit": 10_000,
    });
    backend
        .query(&body)
        .await
        .unwrap()
        .records
        .into_iter()
        .filter(|r| r.type_key == "type")
        .collect()
}

fn row_str<'a>(row: &'a roostr_core::types::QueryRow, key: &str) -> &'a str {
    row.fields
        .get(key)
        .and_then(|v| v.string_value.as_deref())
        .unwrap_or("")
}

/// The sidebar's agent type row of `space`: what its click opens.
async fn agent_type_of(backend: &ReplicaBackend, space: &str) -> Option<String> {
    type_defs(backend)
        .await
        .into_iter()
        .find(|r| row_str(r, "key") == "agent" && row_str(r, "channel") == space)
        .map(|r| r.id)
}

/// The agents the type page of `type_id` lists (`table_body`,
/// `DataViewKind::Type`); none while the type is not there.
async fn type_page_rows(backend: &ReplicaBackend, type_id: &str, space: &str) -> BTreeSet<String> {
    let Ok(object) = backend.object(type_id).await else {
        return BTreeSet::new();
    };
    let key = object.fields.get("key").and_then(|v| v.string_value.clone()).unwrap_or_default();
    let filter = roostr_core::filters::space_filter_of(&object.fields, space);
    let body = json!({ "type": key, "filters": [filter], "limit": 10_000 });
    backend
        .query(&body)
        .await
        .unwrap()
        .records
        .into_iter()
        .map(|r| r.id)
        .collect()
}

/// Check, on `backend`, that `space`'s agent type is the one its sidebar
/// opens and that its page lists the space's own agents.
async fn check_type_page(backend: &ReplicaBackend, space: &Value) -> Vec<Value> {
    let id = space["id"].as_str().unwrap();
    let agent_type = space["agent_type"].as_str().unwrap();
    let defs: Vec<Value> = type_defs(backend)
        .await
        .iter()
        .filter(|r| row_str(r, "key") == "agent")
        .map(|r| json!({ "id": r.id, "channel": row_str(r, "channel"), "name": row_str(r, "name") }))
        .collect();
    assert_eq!(agent_type_of(backend, id).await.as_deref(), Some(agent_type), "{}: {defs:?}", space["name"]);
    let opened = backend.object(agent_type).await.unwrap();
    assert_eq!((opened.id.as_str(), opened.type_key.as_str()), (agent_type, "type"));
    assert_eq!(opened.field_str("channel"), id);
    let rows = type_page_rows(backend, agent_type, id).await;
    assert_eq!(rows, strings(&space["agents"]).into_iter().collect(), "{}'s type page", space["name"]);
    defs
}

/// A full replica writes `LITE_SPACES` (comma-separated names), each with
/// two agents; `LITE_QUERY` also gets a pinned "All Agents" query and
/// `LITE_MEMBER` is made a writer of every space. The spaces of
/// `LITE_JOINED` (shared with this identity) must arrive and show their own
/// agent type pages (child of [`lite_type_pages`]).
#[test]
#[ignore]
fn lite_types_writer() {
    if var("LITE_DIR").is_empty() {
        return;
    }
    let dir = PathBuf::from(var("LITE_DIR"));
    let relay = var("LITE_RELAY_URL");
    let keys = Keys::parse(&var("LITE_KEY")).unwrap();
    let member = var("LITE_MEMBER");
    let query_in = var("LITE_QUERY");
    let joined: Vec<Value> = serde_json::from_str::<Value>(&var("LITE_JOINED"))
        .ok()
        .and_then(|j| j["spaces"].as_array().cloned())
        .unwrap_or_default();
    logs();
    block_on(async {
        std::fs::create_dir_all(&dir).unwrap();
        let store = sqlite(&dir).await;
        let backend = signed_in(&dir, store.clone(), &[&relay], &keys).await;
        backend.start().await.unwrap();
        assert!(eventually(WAIT, async || directory_built(&backend, "__directory__", None).await).await);
        let mut spaces = Vec::new();
        for name in var("LITE_SPACES").split(',') {
            let id = mutate(&backend, "channel_create", json!({ "name": name })).await;
            if !member.is_empty() {
                mutate(
                    &backend,
                    "channel_member_add",
                    json!({ "channel_id": id, "npub": member, "role": "writer" }),
                )
                .await;
            }
            let channel = json!({ "channel": { "stringValue": id } });
            let mut agents = Vec::new();
            for i in 0..2 {
                agents.push(
                    mutate(
                        &backend,
                        "create",
                        json!({ "name": format!("{name} agent {i}"), "type_key": "agent", "fields": channel }),
                    )
                    .await,
                );
            }
            let mut all_agents = String::new();
            if name == query_in {
                let mut fields = channel.clone();
                fields["setOf"] = json!({ "valuesValue": { "items": [{ "stringValue": "agent" }] } });
                fields["pin_order"] = json!({ "stringValue": "a0" });
                all_agents = mutate(
                    &backend,
                    "create",
                    json!({ "name": "All Agents", "type_key": "query", "fields": fields }),
                )
                .await;
            }
            let agent_type = agent_type_of(&backend, &id).await.expect("the space's agent type");
            spaces.push(json!({
                "id": id,
                "name": name,
                "agents": agents,
                "agent_type": agent_type,
                "all_agents": all_agents,
            }));
        }
        for space in &spaces {
            let dir_id = format!("__directory__:{}", space["id"].as_str().unwrap());
            assert!(eventually(WAIT, async || directory_built(&backend, &dir_id, None).await).await);
            check_type_page(&backend, space).await;
        }
        // Spaces shared with this identity arrive by invite.
        let mut joined_defs = Vec::new();
        for space in &joined {
            let id = space["id"].as_str().unwrap();
            let agents: BTreeSet<String> = strings(&space["agents"]).into_iter().collect();
            let arrived = eventually(WAIT, async || {
                backend.channels().await.unwrap().iter().any(|c| c.id == id)
                    && agent_type_of(&backend, id).await.is_some()
                    && type_page_rows(&backend, space["agent_type"].as_str().unwrap(), id).await == agents
            })
            .await;
            assert!(arrived, "{} never arrived whole", space["name"]);
            joined_defs.push(check_type_page(&backend, space).await);
        }
        backend.await_idle().await;
        if !member.is_empty() {
            // The member's invite is on the relay, the spaces' streams settled.
            let member_hex = roostr_nostr::keys::npub_to_hex(&member).unwrap();
            let invited = eventually(WAIT, async || {
                let filter = Filter::new().kinds([kinds::GIFT_WRAP]).tag('p', [member_hex.clone()]);
                query_many(&[Relay::new(relay.clone())], &[filter], Duration::from_secs(10))
                    .await
                    .is_ok_and(|wraps| !wraps.is_empty())
            })
            .await;
            assert!(invited, "no invite for the member");
            roostr_nostr::sleep(Duration::from_secs(3)).await;
            backend.await_idle().await;
        }
        println!("RESULT {}", json!({ "spaces": spaces, "joined_agent_types": joined_defs }));
        backend.stop();
    });
}

/// A fresh lite replica of A (child of [`lite_type_pages`]).
#[test]
#[ignore]
fn lite_types_reader() {
    if var("LITE_DIR").is_empty() {
        return;
    }
    let dir = PathBuf::from(var("LITE_DIR"));
    let relay = var("LITE_RELAY_URL");
    let keys = Keys::parse(&var("LITE_KEY")).unwrap();
    let expect: Value = serde_json::from_str(&var("LITE_EXPECT")).unwrap();
    logs();
    block_on(async {
        let (backend, _store) = lite_replica(&dir, &relay, &keys).await;
        let lite = backend.lite().unwrap();
        let checked = eventually(WAIT, async || backend.status().lite.is_some_and(|l| l.checked)).await;
        assert!(checked, "the directories never arrived");
        let mut out = Vec::new();
        for name in ["Viv", "Matcherino"] {
            let s = space(&expect, name);
            let id = s["id"].as_str().unwrap().to_string();
            // The sidebar lists the space's agent type once its directory is in.
            let listed = eventually(WAIT, async || agent_type_of(&backend, &id).await.is_some()).await;
            assert!(listed, "no agent type listed for {name}");
            // Its page downloads the space (`ensure_space`), then lists its agents.
            lite.load_space(&id).await.unwrap();
            out.push(json!({ "space": name, "agent_types": check_type_page(&backend, s).await }));
        }
        println!("RESULT {}", json!({ "checked": out }));
        backend.stop();
    });
}

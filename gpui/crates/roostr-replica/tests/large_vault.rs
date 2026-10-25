//! Seeds a large throwaway vault onto a relay, for loading the browser
//! replica against a realistic volume (manual; `--ignored`):
//!
//! ```sh
//! SEED_RELAY=ws://127.0.0.1:7791 SEED_OUT=/tmp/large-vault \
//!   cargo test --release -p roostr-replica --test large_vault -- --ignored --nocapture
//! ```
//!
//! Two fresh identities: the user, and a partner who owns a space the user
//! writes in. Each writes through its own offline replica (real mutations:
//! spaces, pages, text blocks, renames); every stored obligation is then
//! sealed and signed as the outbox does, and a share of the objects get a
//! base sealed as a computer makes one (engine `compact_plan` + `seal_base`).
//! The events go to `SEED_RELAY` over parallel connections (the relay
//! paces each connection). `SEED_OUT/browser.json` holds the user's
//! `localStorage` for the tab: `roostr-key`, `roostr-relays`,
//! `roostr-space-keys` (the partner's space as an invite would install it).
//!
//! `SEED_OBJECTS` (default 1300) user pages, `SEED_PARTNER_OBJECTS` (300)
//! partner pages, ~11 changes each; `SEED_BASES` (230) bases.
//!
//! `first_sync` (also `--ignored`, same variables) then times this machine's
//! first sync of that vault into a fresh native (SQLite) replica, signed in
//! from `SEED_OUT/browser.json`: until the first history walk completes.
#![cfg(not(target_family = "wasm"))]

mod backend_support;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use backend_support::{params, signed_in, sqlite};
use roostr_core::Backend;
use roostr_engine::{Change, Engine, SealRequest, SealSpace, StateInputs};
use roostr_nostr::relay::Relay;
use roostr_nostr::spacekeys::{KeyFile, SpaceKey, SpaceKeyring};
use roostr_nostr::{Event, EventTemplate, Keys, kinds};
use roostr_replica::ReplicaBackend;
use roostr_replica::keyring::SPACE_KEYS;
use roostr_replica_store::{ChangeStore, SqliteChangeStore};
use serde_json::json;

const CONNECTIONS: usize = 24;
const DAY_S: u64 = 86_400;

fn env_or(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Deterministic prose: `len` characters of words.
fn prose(seed: u64, len: usize) -> String {
    const WORDS: [&str; 24] = [
        "garden",
        "meeting",
        "recipe",
        "invoice",
        "tomorrow",
        "plan",
        "review",
        "notes",
        "travel",
        "budget",
        "school",
        "doctor",
        "project",
        "draft",
        "ideas",
        "weekend",
        "groceries",
        "call",
        "follow",
        "up",
        "with",
        "the",
        "and",
        "for",
    ];
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut out = String::with_capacity(len + 16);
    while out.len() < len {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(WORDS[(state >> 33) as usize % WORDS.len()]);
    }
    out
}

async fn mutate(backend: &ReplicaBackend, action: &str, value: serde_json::Value) -> String {
    let out = backend.mutate(action, params(value)).await.unwrap();
    out.get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// `count` pages across `channels`: a create, eight text blocks, two
/// renames. Records each page's channel in `spaces` when it is shared.
async fn write_pages(
    backend: &ReplicaBackend,
    channels: &[String],
    shared: &[String],
    spaces: &mut Spaces,
    count: usize,
    seed: u64,
) {
    for i in 0..count {
        let channel = &channels[i % channels.len()];
        let id = mutate(
            backend,
            "create",
            json!({ "name": format!("Page {seed}-{i}"), "fields": { "channel": { "stringValue": channel } } }),
        )
        .await;
        if shared.contains(channel) {
            spaces.insert(id.clone(), channel.clone());
        }
        for block in 0..8 {
            let text = prose(
                seed * 1_000_003 + (i * 16 + block) as u64,
                120 + (i * 37 + block * 91) % 700,
            );
            backend
                .mutate(
                    "block_add",
                    params(json!({
                        "object_id": id,
                        "block": { "content": { "text": { "text": text } } },
                        "target_id": "",
                        "position": block,
                    })),
                )
                .await
                .unwrap();
        }
        for rename in 0..2 {
            backend
                .mutate(
                    "set_field",
                    params(json!({ "object_id": id, "key": "name", "value": { "stringValue": format!("Page {seed}-{i} v{rename}") } })),
                )
                .await
                .unwrap();
        }
        if i % 100 == 0 {
            eprintln!("[seed] {seed}: {i}/{count} pages");
        }
    }
}

/// An offline replica of `keys` in `dir`.
async fn replica(dir: &std::path::Path, keys: &Keys) -> (ReplicaBackend, Arc<SqliteChangeStore>) {
    std::fs::create_dir_all(dir).unwrap();
    let store = sqlite(dir).await;
    let backend = signed_in(dir, store.clone(), &[], keys).await;
    (backend, store)
}

fn sign(keys: &Keys, kind: u32, created_at: u64, tags: Vec<Vec<String>>, content: String) -> Event {
    keys.sign(EventTemplate {
        kind,
        created_at,
        tags,
        content,
    })
}

/// Object id → the shared space it lives in (the space object is its own).
type Spaces = HashMap<String, String>;

/// Seals a change (or base, `base`) into `space`'s stream, else the personal one.
fn stream<'a>(
    keyring: &dyn SpaceKeyring,
    personal: &'a str,
    space: Option<&'a String>,
) -> (String, Option<(&'a String, SpaceKey)>) {
    match space {
        Some(id) => {
            let key = keyring.get(id).unwrap().expect("space key");
            (key.key.clone(), Some((id, key)))
        }
        None => (personal.to_string(), None),
    }
}

/// Every stored obligation of `store`, sealed as the outbox seals it (the
/// personal copy, and the space copy of a shared object) and signed by
/// `keys`, spread over the last 120 days by object.
async fn delta_events(
    engine: &Engine,
    store: &SqliteChangeStore,
    keys: &Keys,
    keyring: &dyn SpaceKeyring,
    spaces: &Spaces,
) -> Vec<Event> {
    let personal = hex::encode(roostr_nostr::nip44::conversation_key(keys, keys.pubkey()).unwrap());
    let secret = keys.secret_hex();
    let now = now_s();
    let mut order: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for item in store.pending_publishes().await.unwrap() {
        let next = order.len();
        let rank = *order.entry(item.object_id.clone()).or_insert(next);
        let created_at = now - 120 * DAY_S + (rank as u64 * 7919) % (119 * DAY_S);
        let copies = [None, spaces.get(&item.object_id)];
        for space in copies
            .iter()
            .enumerate()
            .filter(|(i, s)| *i == 0 || s.is_some())
            .map(|(_, s)| *s)
        {
            let (conversation, space) = stream(keyring, &personal, space);
            let sealed = engine
                .seal(&SealRequest {
                    change: &item.bytes,
                    conversation_key: &conversation,
                    secret: space.is_none().then_some(secret.as_str()),
                    space: space.as_ref().map(|(id, key)| SealSpace {
                        key_hex: &key.key,
                        space_id: id,
                        owner: key.owner.as_deref(),
                    }),
                    object_id: &item.object_id,
                    base: item.base.as_deref(),
                    nonce: None,
                })
                .await
                .unwrap();
            for part in sealed.parts {
                out.push(sign(
                    keys,
                    kinds::CHANGE,
                    created_at,
                    part.tags,
                    part.content,
                ));
            }
        }
    }
    out
}

/// Bases for `count` of `store`'s objects (each folding every change held),
/// sealed into the object's stream (its space's when shared).
async fn base_events(
    engine: &Engine,
    store: &SqliteChangeStore,
    keys: &Keys,
    keyring: &dyn SpaceKeyring,
    spaces: &Spaces,
    count: usize,
) -> Vec<Vec<Event>> {
    let personal = hex::encode(roostr_nostr::nip44::conversation_key(keys, keys.pubkey()).unwrap());
    let secret = keys.secret_hex();
    let created_at = now_s() - 3600;
    let mut out = Vec::new();
    let ids = store.object_ids().await.unwrap();
    // Every third object, so personal and shared ones both get bases.
    for id in ids.iter().step_by(3).take(count) {
        let changes: Vec<Change> = store
            .changes_for(id.clone())
            .await
            .unwrap()
            .iter()
            .map(|record| serde_json::from_str(&record.json).unwrap())
            .collect();
        let plan = engine
            .compact_plan(
                &StateInputs {
                    bases: Vec::new(),
                    changes,
                    checkpoint: None,
                    pending: Vec::new(),
                    own: Vec::new(),
                },
                created_at as i64 * 1000,
                true,
            )
            .await
            .unwrap();
        let Some(base) = plan.base.filter(|_| plan.compact) else {
            continue;
        };
        let (conversation, space) = stream(keyring, &personal, spaces.get(id));
        let sealed = engine
            .seal_base(
                &base,
                &conversation,
                space.is_none().then_some(secret.as_str()),
                space.as_ref().map(|(s, k)| SealSpace {
                    key_hex: &k.key,
                    space_id: s,
                    owner: k.owner.as_deref(),
                }),
                None,
                None,
            )
            .await
            .unwrap();
        out.push(
            sealed
                .parts
                .into_iter()
                .chain([sealed.event.expect("an inline base")])
                .map(|p| sign(keys, p.kind as u32, created_at, p.tags, p.content))
                .collect(),
        );
    }
    out
}

/// Publishes every group (in order within a group) over parallel connections.
fn publish(relay: &str, groups: Vec<Vec<Event>>) {
    let mut lanes: Vec<Vec<Event>> = vec![Vec::new(); CONNECTIONS];
    for (i, group) in groups.into_iter().enumerate() {
        lanes[i % CONNECTIONS].extend(group);
    }
    let total: usize = lanes.iter().map(Vec::len).sum();
    eprintln!("[seed] publishing {total} events over {CONNECTIONS} connections");
    let threads: Vec<_> = lanes
        .into_iter()
        .map(|lane| {
            let relay = relay.to_string();
            std::thread::spawn(move || {
                async_io::block_on(async move {
                    let connection = Relay::new(relay);
                    for event in &lane {
                        let mut tries = 0;
                        while let Err(error) =
                            connection.publish(event, Duration::from_secs(20)).await
                        {
                            tries += 1;
                            assert!(tries < 20, "publish failed: {error}");
                            async_io::Timer::after(Duration::from_millis(1100)).await;
                        }
                        async_io::Timer::after(Duration::from_millis(36)).await;
                    }
                })
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
#[ignore = "seeds a live relay; run by hand"]
fn seed_large_vault() {
    let relay = std::env::var("SEED_RELAY").expect("SEED_RELAY");
    let out = std::path::PathBuf::from(std::env::var("SEED_OUT").expect("SEED_OUT"));
    let user_pages = env_or("SEED_OBJECTS", 1300);
    let partner_pages = env_or("SEED_PARTNER_OBJECTS", 300);
    let base_count = env_or("SEED_BASES", 230);
    let user = Keys::generate();
    let partner = Keys::generate();
    let engine = Engine::native().unwrap();
    async_io::block_on(async {
        // The partner's space, the user a writer in it.
        let (p, p_store) = replica(&out.join("partner"), &partner).await;
        let family = mutate(&p, "channel_create", json!({ "name": "Family" })).await;
        mutate(
            &p,
            "channel_member_add",
            json!({ "channel_id": family, "npub": user.npub(), "role": "writer" }),
        )
        .await;
        let mut spaces = Spaces::from([(family.clone(), family.clone())]);
        let family_channels = [family.clone()];
        write_pages(
            &p,
            &family_channels,
            &family_channels,
            &mut spaces,
            partner_pages,
            2,
        )
        .await;
        let family_key = p.keyring().get(&family).unwrap().unwrap();
        let mut partner_groups: Vec<Vec<Event>> =
            delta_events(&engine, &p_store, &partner, &*p.keyring(), &spaces)
                .await
                .into_iter()
                .map(|e| vec![e])
                .collect();
        partner_groups.extend(
            base_events(
                &engine,
                &p_store,
                &partner,
                &*p.keyring(),
                &spaces,
                base_count / 6,
            )
            .await,
        );
        p.stop();
        drop(p);

        // The user: four spaces of their own, one shared with the partner.
        let (u, u_store) = replica(&out.join("user"), &user).await;
        let mut channels = Vec::new();
        for name in ["Personal", "Work", "Recipes", "Garden"] {
            channels.push(mutate(&u, "channel_create", json!({ "name": name })).await);
        }
        let shared = mutate(
            &u,
            "channel_create",
            json!({ "name": "Shared with partner" }),
        )
        .await;
        mutate(
            &u,
            "channel_member_add",
            json!({ "channel_id": shared, "npub": partner.npub(), "role": "writer" }),
        )
        .await;
        let mut spaces = Spaces::from([(shared.clone(), shared.clone())]);
        channels.push(shared.clone());
        write_pages(&u, &channels, &[shared], &mut spaces, user_pages, 1).await;
        let mut groups: Vec<Vec<Event>> =
            delta_events(&engine, &u_store, &user, &*u.keyring(), &spaces)
                .await
                .into_iter()
                .map(|e| vec![e])
                .collect();
        let changes = groups.len();
        let bases = base_events(&engine, &u_store, &user, &*u.keyring(), &spaces, base_count).await;
        eprintln!(
            "[seed] user: {changes} change events, {} bases; partner: {} groups",
            bases.len(),
            partner_groups.len()
        );
        groups.extend(bases);
        groups.extend(partner_groups);
        publish(&relay, groups);
        let mut keyfile = KeyFile::parse(&u.kv().get(SPACE_KEYS).unwrap()).unwrap();
        keyfile.channels.insert(
            family.clone(),
            SpaceKey {
                owner: Some(partner.pubkey().to_string()),
                ..family_key
            },
        );
        let browser = json!({
            "roostr-key": user.secret_hex(),
            "roostr-relays": serde_json::to_string(&[relay.as_str()]).unwrap(),
            "roostr-space-keys": keyfile.to_json(),
            "npub": user.npub(),
        });
        std::fs::write(
            out.join("browser.json"),
            serde_json::to_string_pretty(&browser).unwrap(),
        )
        .unwrap();
        eprintln!("[seed] wrote {}", out.join("browser.json").display());
        u.stop();
    });
}

#[test]
#[ignore = "manual: times a native first sync of the seeded vault (SEED_RELAY, SEED_OUT)"]
fn first_sync() {
    let out = std::path::PathBuf::from(std::env::var("SEED_OUT").expect("SEED_OUT"));
    let browser: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("browser.json")).unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let kv = roostr_replica::local::FileLocalKv::open(dir.path().join("local.json")).unwrap();
    for key in ["roostr-key", "roostr-relays", "roostr-space-keys"] {
        roostr_replica::local::LocalKv::set(&kv, key, browser[key].as_str());
    }
    drop(kv);
    async_io::block_on(async {
        let started = std::time::Instant::now();
        let backend = ReplicaBackend::open_native(dir.path()).await.unwrap();
        backend.start().await.unwrap();
        let limit = Duration::from_secs(3600);
        while !backend.status().bootstrapped && started.elapsed() < limit {
            roostr_nostr::sleep(Duration::from_millis(100)).await;
        }
        let elapsed = started.elapsed();
        let store = SqliteChangeStore::open(dir.path().join("replica.sqlite"))
            .await
            .unwrap();
        let changes: u64 = store.change_counts().await.unwrap().values().sum();
        eprintln!(
            "[first-sync] bootstrapped={} in {:.1}s: {changes} changes, imported {}",
            backend.status().bootstrapped,
            elapsed.as_secs_f64(),
            backend.status().imported
        );
        backend.stop();
    });
}

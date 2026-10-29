//! Record mode end to end against a local RoostrRelay (never the production
//! one): its stream cursor (`after`, EOSE heads, live seq) and its Blossom.
//!
//! ```sh
//! cargo test -p roostr-replica --test records -- --nocapture
//! # a prebuilt relay instead of building ../RoostrRelay with odin:
//! ROOSTR_RELAY_BIN=/path/to/roostr-relay cargo test -p roostr-replica --test records
//! ```
//!
//! Each test starts its own relay (random port, temp `DB_PATH`, open
//! writes) and a record-mode replica A on a throwaway key that writes pages
//! and, standing in for the pack builder (the record-mode daemon's role),
//! packs the personal stream at the relay's head, uploads the pack to the
//! relay's Blossom, announces it (kind 31081) and deletes the patches it
//! covers (retention), then writes more. Then:
//! 1. a fresh replica B loads pack + catch-up: its fingerprint root is A's;
//! 2. a Lite read fetches one object from its pack bucket by Range;
//! 3. B edits while the relay is down and sends the patch on reconnect.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::collections::{HashMap, HashSet};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use roostr_core::Backend;
use roostr_core::backend::JsonMap;
use roostr_engine::pack::pack_build;
use roostr_engine::record::{Record, record_canonical, record_view};
use roostr_nostr::relay::SubscriptionItem;
use roostr_nostr::{EventTemplate, Filter, Keys, Relay};
use roostr_replica::keyring::SPACE_KEYS;
use roostr_replica::local::{FileLocalKv, LocalKv};
use roostr_replica::records::{
    PACK_KIND, PATCH_KIND, PackAnnouncement, RecordHost, RecordSync, Records, StreamKey, seal_pack,
};
use roostr_replica::{RELAYS, ReplicaBackend, ReplicaParts};
use roostr_replica_store::{RecordStore, SqliteChangeStore};
use serde_json::{Value, json};
use support::{ENGINE_TURN, block_on, eventually};

const WAIT: Duration = Duration::from_secs(60);
const T: Duration = Duration::from_secs(10);

fn params(value: Value) -> JsonMap {
    value.as_object().cloned().unwrap()
}

// ── A local RoostrRelay ─────────────────────────────────────────────

fn relay_binary() -> PathBuf {
    if let Some(bin) = std::env::var_os("ROOSTR_RELAY_BIN") {
        return bin.into();
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../RoostrRelay/src");
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("roostr-relay");
    static BUILT: std::sync::Once = std::sync::Once::new();
    BUILT.call_once(|| {
        let status = Command::new(std::env::var("ODIN").unwrap_or_else(|_| "odin".into()))
            .arg("build")
            .arg(&source)
            .arg(format!("-out:{}", out.display()))
            .status()
            .expect("odin builds RoostrRelay (or set ROOSTR_RELAY_BIN)");
        assert!(status.success(), "building RoostrRelay from {}", source.display());
    });
    out
}

struct LocalRelay {
    child: Option<Child>,
    port: u16,
    data: tempfile::TempDir,
    url: String,
}

impl LocalRelay {
    fn start() -> Self {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut relay = Self {
            child: None,
            port,
            data: tempfile::tempdir().unwrap(),
            url: format!("ws://127.0.0.1:{port}"),
        };
        relay.up();
        relay
    }

    fn up(&mut self) {
        let child = Command::new(relay_binary())
            .env("PORT", self.port.to_string())
            .env("DB_PATH", self.data.path().join("relay.db"))
            .env("RELAY_ALLOWED_PUBKEYS", "")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start roostr-relay");
        self.child = Some(child);
        for _ in 0..200 {
            if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("roostr-relay did not listen on {}", self.port);
    }

    fn down(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn http(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for LocalRelay {
    fn drop(&mut self) {
        self.down();
    }
}

// ── Replicas ────────────────────────────────────────────────────────

async fn record_replica(dir: &Path, relays: &[&str], keys: &Keys) -> (ReplicaBackend, Arc<SqliteChangeStore>) {
    let store = Arc::new(SqliteChangeStore::open(dir.join("replica.sqlite")).await.unwrap());
    let kv = Arc::new(FileLocalKv::open(dir.join("local.json")).unwrap());
    kv.set(RELAYS, Some(&serde_json::to_string(relays).unwrap()));
    kv.set(roostr_replica::records::RECORDS_FLAG, Some("1"));
    let backend = ReplicaBackend::new(ReplicaParts {
        engine: roostr_engine::Engine::native().unwrap(),
        store: store.clone(),
        kv,
        label: "record test replica".into(),
    })
    .records_if_flagged(store.clone())
    .await
    .unwrap();
    assert!(backend.records().is_some(), "the flag turns record mode on");
    backend
        .mutate("nostr_key_import", params(json!({ "key": keys.secret_hex() })))
        .await
        .unwrap();
    backend.start().await.unwrap();
    (backend, store)
}

async fn journal_drains(store: &SqliteChangeStore) -> bool {
    eventually(WAIT, async || store.journal().await.unwrap().is_empty()).await
}

struct Nobody;

impl RecordHost for Nobody {
    fn on_records(&self, _: Vec<String>) {}
    fn space_of(&self, _: &str) -> Option<String> {
        None
    }
}

/// The objects' spaces, as a full replica learned them: where a Lite read looks.
struct Spaces(HashMap<String, String>);

impl RecordHost for Spaces {
    fn on_records(&self, _: Vec<String>) {}
    fn space_of(&self, object_id: &str) -> Option<String> {
        self.0.get(object_id).cloned()
    }
}

/// A bare record sync on an in-memory store (no backend, no engine).
async fn bare_sync(keys: &Keys, relay: &str) -> RecordSync {
    let store: Arc<dyn RecordStore> = Arc::new(SqliteChangeStore::in_memory().await.unwrap());
    let records = Arc::new(Records::open(store).await.unwrap());
    RecordSync::new(keys.clone(), vec![relay.to_string()], records, Arc::new(Nobody)).unwrap()
}

/// The stream's head on the relay (EOSE `{head, oldest}`).
async fn heads(relay: &str, stream: &StreamKey, keys: &Keys) -> (u64, u64) {
    let relay = Relay::new(relay);
    let filter = Filter::new().kinds([PATCH_KIND]).authors([keys.pubkey()]).after(0).limit(1);
    assert!(stream.s.is_none(), "personal stream");
    let mut sub = relay.subscribe(vec![filter]);
    loop {
        match sub.next().await {
            Some(Ok(SubscriptionItem::Cursor(value))) => {
                return (value["head"].as_u64().unwrap(), value["oldest"].as_u64().unwrap());
            }
            Some(Ok(_)) => {}
            other => panic!("no EOSE heads: {other:?}"),
        }
    }
}

/// What a pack builder does: pack `records` at `cursor`, upload the blob
/// to the relay's Blossom, announce it, delete the patches it covers.
async fn build_pack(relay: &LocalRelay, keys: &Keys, stream: &StreamKey, records: &[Record], cursor: u64) -> PackAnnouncement {
    let pack = pack_build(records, &stream.stream, cursor).unwrap();
    let blob = stream.pack_blob(&pack.plaintext, cursor);
    let sha = roostr_core::blossom::sha256_hex(&blob);
    let auth = keys.sign(roostr_nostr::blossom::upload_auth_template(&sha, "pack", roostr_nostr::now_s()));
    let url = roostr_core::blossom::upload_blob(
        &relay.http(),
        &blob,
        &sha,
        &roostr_nostr::blossom::authorization_header(&auth),
    )
    .await
    .unwrap();
    let announcement = PackAnnouncement {
        v: 1,
        stream: stream.stream.clone(),
        cursor,
        sha256: sha,
        size: blob.len() as u64,
        url,
        root: hex::encode(pack.root),
        buckets: pack.header.buckets.iter().map(|b| hex::encode(&b.hash[..8])).collect(),
        records: pack.header.records,
        built_at: 0,
        builder: keys.pubkey().to_string(),
        deleted_through: cursor,
    };
    let client = Relay::new(relay.url.as_str());
    let event = seal_pack(keys, stream, &announcement, roostr_nostr::now_s()).unwrap();
    client.publish(&event, T).await.unwrap();
    // Retention: the covered patches go (NIP-09).
    let covered = client
        .query(vec![Filter::new().kinds([PATCH_KIND]).authors([keys.pubkey()])], T)
        .await
        .unwrap();
    assert!(!covered.is_empty());
    let deletion = keys.sign(EventTemplate {
        kind: 5,
        created_at: roostr_nostr::now_s(),
        tags: covered.iter().map(|e| vec!["e".to_string(), e.id.clone()]).collect(),
        content: String::new(),
    });
    client.publish(&deletion, T).await.unwrap();
    let left = client
        .query(vec![Filter::new().kinds([PATCH_KIND]).authors([keys.pubkey()])], T)
        .await
        .unwrap();
    assert!(left.is_empty(), "the relay dropped the covered patches");
    // The announcement is on the relay.
    let announced = client
        .query(vec![Filter::new().kinds([PACK_KIND]).tag('d', [stream.d.clone()])], T)
        .await
        .unwrap();
    assert_eq!(announced.len(), 1);
    announcement
}

/// Replica A's vault: pages before the pack, the pack, edits after it.
struct World {
    relay: LocalRelay,
    keys: Keys,
    /// Renamed after the pack.
    edited: String,
    /// Created after the pack.
    late: String,
    root: [u8; 32],
    records: Vec<Record>,
    pack: PackAnnouncement,
}

async fn world() -> World {
    let relay = LocalRelay::start();
    let keys = Keys::generate();
    let dir = tempfile::tempdir().unwrap();
    let (a, store) = record_replica(dir.path(), &[relay.url.as_str()], &keys).await;
    let mut pages = Vec::new();
    for i in 0..3 {
        let page = a.mutate("create", params(json!({ "name": format!("Page {i}") }))).await.unwrap();
        pages.push(page["id"].as_str().unwrap().to_string());
    }
    assert!(journal_drains(&store).await, "A's patches reach the relay");
    // A record per page, as a view.
    let records = a.records().unwrap();
    for id in &pages {
        let view = records.view(id).await.unwrap().unwrap();
        assert_eq!(view["fields"]["name"]["stringValue"], format!("Page {}", pages.iter().position(|p| p == id).unwrap()), "{view}");
    }
    let sync = a.record_sync().unwrap();
    let stream = sync.streams()[0].clone();
    let (head, _) = heads(&relay.url, &stream, &keys).await;
    assert!(head >= 3, "every patch got a seq");
    // Everything at or below the head is in the records (journal and
    // record are written together, before the patch is sent).
    let snapshot = records.all().await.unwrap();
    let pack = build_pack(&relay, &keys, &stream, &snapshot, head).await;

    a.mutate(
        "set_field",
        params(json!({ "object_id": pages[0], "key": "name", "value": { "stringValue": "Renamed after the pack" } })),
    )
    .await
    .unwrap();
    let late = a.mutate("create", params(json!({ "name": "After the pack" }))).await.unwrap();
    let late = late["id"].as_str().unwrap().to_string();
    assert!(journal_drains(&store).await);
    let (after, oldest) = heads(&relay.url, &stream, &keys).await;
    assert!(after > head && oldest > head, "patches after the pack only: head {after}, oldest {oldest}");
    let root = records.root(|_| true).await.unwrap();
    let all = records.all().await.unwrap();
    a.stop();
    drop(a);
    World {
        relay,
        keys,
        edited: pages[0].clone(),
        late,
        root,
        records: all,
        pack,
    }
}

#[test]
fn a_new_device_loads_the_pack_and_catches_up_to_the_builders_root() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let w = world().await;
        let dir = tempfile::tempdir().unwrap();
        let (b, _store) = record_replica(dir.path(), &[w.relay.url.as_str()], &w.keys).await;
        let records = b.records().unwrap();
        assert!(
            eventually(WAIT, async || records.root(|_| true).await.unwrap() == w.root).await,
            "B's fingerprint root is A's"
        );
        // The pack's records came from Blossom (the relay no longer holds
        // their patches) and the later ones from the cursor.
        let stream = b.record_sync().unwrap().streams()[0].clone();
        let (head, _) = heads(&w.relay.url, &stream, &w.keys).await;
        assert_eq!(records.cursor(&stream).await.unwrap(), Some(head));
        assert!(head > w.pack.cursor);
        // Views are served from the records.
        let edited = b.object(&w.edited).await.unwrap();
        assert_eq!(edited.name(), "Renamed after the pack");
        assert!(b.object(&w.late).await.is_ok());
        b.stop();
    });
}

#[test]
fn a_lite_read_takes_one_object_from_its_pack_bucket() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let w = world().await;
        let lite = bare_sync(&w.keys, &w.relay.url).await;
        let stream = lite.streams()[0].clone();
        let announcement = lite.announcement(&stream).await.unwrap().unwrap();
        assert_eq!(announcement, w.pack);
        let want = w.records.iter().find(|r| r.object_id == w.edited).unwrap();
        // The bucket alone (Range requests), then the object's patches after the pack.
        let bucket = usize::from(roostr_engine::record::record_bucket(&w.edited));
        let from_pack = roostr_replica::records::fetch_pack_bucket(
            &roostr_core::blossom::blossom_server(&[w.relay.url.clone()]).await,
            &stream,
            &announcement,
            bucket,
        )
        .await
        .unwrap();
        assert!(from_pack.iter().any(|r| r.object_id == w.edited));
        let fetched = lite.fetch_object(&w.edited).await.unwrap().unwrap();
        assert_eq!(record_canonical(&fetched), record_canonical(want));
        // Nothing was stored: a Lite read is not a load.
        assert!(lite.records().all().await.unwrap().is_empty());
    });
}

#[test]
fn an_offline_edit_is_sent_on_reconnect() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let mut w = world().await;
        let dir = tempfile::tempdir().unwrap();
        let (b, store) = record_replica(dir.path(), &[w.relay.url.as_str()], &w.keys).await;
        let records = b.records().unwrap();
        assert!(eventually(WAIT, async || records.root(|_| true).await.unwrap() == w.root).await);

        w.relay.down();
        b.mutate(
            "set_field",
            params(json!({ "object_id": w.late, "key": "name", "value": { "stringValue": "Written offline" } })),
        )
        .await
        .unwrap();
        assert_eq!(b.object(&w.late).await.unwrap().name(), "Written offline");
        roostr_nostr::sleep(Duration::from_millis(300)).await;
        assert_eq!(store.journal().await.unwrap().len(), 1, "kept until a relay accepts it");

        w.relay.up();
        assert!(journal_drains(&store).await, "sent once the relay is back");
        let root = records.root(|_| true).await.unwrap();
        b.stop();

        // Another device sees it: pack, then the patches after it.
        let other = bare_sync(&w.keys, &w.relay.url).await;
        other.start().await.unwrap();
        let view = other.records().view(&w.late).await.unwrap().unwrap();
        assert_eq!(view["fields"]["name"]["stringValue"], "Written offline", "{view}");
        assert_eq!(other.records().root(|_| true).await.unwrap(), root);
        other.stop();
    });
}

/// A device whose cursor is below the newest pack's `deleted_through`
/// reloads the pack, though an older event of the stream (here a profile)
/// keeps the relay's `oldest` at 1 and the window looks unbroken.
#[test]
fn a_device_behind_a_retention_cut_reloads_the_pack_while_the_window_looks_open() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let relay = LocalRelay::start();
        let keys = Keys::generate();
        let profile = keys.sign(EventTemplate {
            kind: 0,
            created_at: roostr_nostr::now_s(),
            tags: Vec::new(),
            content: "{}".into(),
        });
        Relay::new(relay.url.as_str()).publish(&profile, T).await.unwrap();
        let dir_a = tempfile::tempdir().unwrap();
        let (a, a_store) = record_replica(dir_a.path(), &[relay.url.as_str()], &keys).await;
        let records = a.records().unwrap();
        let page = a.mutate("create", params(json!({ "name": "Before" }))).await.unwrap();
        let page = page["id"].as_str().unwrap().to_string();
        assert!(journal_drains(&a_store).await);

        // B catches up, then is away.
        let dir_b = tempfile::tempdir().unwrap();
        let (b, b_store) = record_replica(dir_b.path(), &[relay.url.as_str()], &keys).await;
        let before = records.root(|_| true).await.unwrap();
        let b_records = b.records().unwrap();
        assert!(eventually(WAIT, async || b_records.root(|_| true).await.unwrap() == before).await);
        let stream = b.record_sync().unwrap().streams()[0].clone();
        let away_at = b_records.cursor(&stream).await.unwrap().unwrap();
        b.stop();
        drop((b, b_store, b_records));

        // A writes on, packs at the head and deletes every patch it covers.
        a.mutate(
            "set_field",
            params(json!({ "object_id": page, "key": "name", "value": { "stringValue": "After" } })),
        )
        .await
        .unwrap();
        assert!(journal_drains(&a_store).await);
        let (head, _) = heads(&relay.url, &stream, &keys).await;
        let pack = build_pack(&relay, &keys, &stream, &records.all().await.unwrap(), head).await;
        assert!(pack.deleted_through > away_at);
        let (_, oldest) = heads(&relay.url, &stream, &keys).await;
        assert!(away_at + 1 >= oldest, "the profile holds the window open: oldest {oldest}, B at {away_at}");
        let root = records.root(|_| true).await.unwrap();
        a.stop();

        // B returns: the edit is only in the pack.
        let (b, _store) = record_replica(dir_b.path(), &[relay.url.as_str()], &keys).await;
        let b_records = b.records().unwrap();
        assert!(
            eventually(WAIT, async || b_records.root(|_| true).await.unwrap() == root).await,
            "B loaded the pack"
        );
        assert_eq!(b.object(&page).await.unwrap().name(), "After");
        b.stop();
    });
}

/// The shared space an object syncs in (`None`: personal): a channel is its
/// own, anything else its `channel` field (glonOdin `record_space_of`).
fn space_of_record(record: &Record) -> Option<String> {
    let view = record_view(record);
    let space = if view["typeKey"] == "channel" {
        view["id"].as_str()
    } else {
        view["fields"]["channel"]["stringValue"].as_str()
    };
    space.filter(|s| !s.is_empty()).map(str::to_string)
}

/// Against a record-mode daemon's stream: glonOdin
/// `bun harness/scripts/records-daemon.ts` packs and patches a local relay
/// from its personal stream and one shared space, then prints:
/// `RECORDS_DAEMON_RELAY` (ws://…), `RECORDS_DAEMON_NSEC`, the streams'
/// fingerprint roots `RECORDS_DAEMON_ROOT` (personal) and
/// `RECORDS_DAEMON_SPACE_ROOT`, the space `RECORDS_DAEMON_SPACE` and its key
/// `RECORDS_DAEMON_SPACE_KEY`, and `RECORDS_DAEMON_VIEWS`: a file of the
/// daemon's views of a sample of objects and the ids it vanished.
#[test]
#[ignore = "needs a record-mode daemon's relay (RECORDS_DAEMON_*)"]
fn a_replica_reaches_the_daemons_root() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
        let relay = var("RECORDS_DAEMON_RELAY");
        let keys = Keys::parse(&var("RECORDS_DAEMON_NSEC")).unwrap();
        let (personal_root, space, space_key, space_root) = (
            var("RECORDS_DAEMON_ROOT"),
            var("RECORDS_DAEMON_SPACE"),
            var("RECORDS_DAEMON_SPACE_KEY"),
            var("RECORDS_DAEMON_SPACE_ROOT"),
        );
        let daemon: Value = serde_json::from_str(&std::fs::read_to_string(var("RECORDS_DAEMON_VIEWS")).unwrap()).unwrap();

        let dir = tempfile::tempdir().unwrap();
        // The space's key, as the account's keyring backup delivers it (owner = us).
        FileLocalKv::open(dir.path().join("local.json")).unwrap().set(
            SPACE_KEYS,
            Some(
                &json!({ "version": 1, "channels": { &space: { "key": space_key, "keyId": 1, "createdAt": 0, "owner": keys.pubkey() } } })
                    .to_string(),
            ),
        );
        let (b, _store) = record_replica(dir.path(), &[relay.as_str()], &keys).await;
        let records = b.records().unwrap();
        let roots = async || {
            let all = records.all().await.unwrap();
            let in_space: HashSet<String> = all
                .iter()
                .filter(|r| space_of_record(r).as_deref() == Some(space.as_str()))
                .map(|r| r.object_id.clone())
                .collect();
            (
                hex::encode(records.root(|id| !in_space.contains(id)).await.unwrap()),
                hex::encode(records.root(|id| in_space.contains(id)).await.unwrap()),
            )
        };
        let reached = eventually(WAIT, async || roots().await == (personal_root.clone(), space_root.clone())).await;
        assert!(
            reached,
            "the replica's roots (personal, space) {:?} are the daemon's ({personal_root}, {space_root}); streams {:?}",
            roots().await,
            b.record_sync().unwrap().streams()
        );

        // Lite reads of the same objects: the daemon's pack buckets by Range,
        // then each object's patches after the pack, nothing stored.
        let spaces: HashMap<String, String> = records
            .all()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| Some((r.object_id.clone(), space_of_record(r)?)))
            .collect();
        let store: Arc<dyn RecordStore> = Arc::new(SqliteChangeStore::in_memory().await.unwrap());
        let lite = RecordSync::new(
            keys.clone(),
            vec![relay.clone()],
            Arc::new(Records::open(store).await.unwrap()),
            Arc::new(Spaces(spaces)),
        )
        .unwrap();
        lite.set_spaces(vec![(space.clone(), space_key.clone(), keys.pubkey().to_string())])
            .await
            .unwrap();

        for (id, want) in daemon["views"].as_object().unwrap() {
            let mut view = records.view(id).await.unwrap().unwrap_or_else(|| panic!("the replica holds {id}"));
            view.as_object_mut().unwrap().remove("heads");
            assert_eq!(&view, want, "the replica's view of {id} is the daemon's");
            let fetched = lite.fetch_object(id).await.unwrap().unwrap_or_else(|| panic!("a Lite read finds {id}"));
            assert_eq!(&record_view(&fetched), want, "the Lite read of {id} is the daemon's view");
        }
        for id in daemon["vanished"].as_array().unwrap() {
            let id = id.as_str().unwrap();
            assert!(records.get(id).await.unwrap().is_none(), "{id} vanished");
            assert!(lite.fetch_object(id).await.unwrap().is_none(), "a Lite read of {id} sees the vanish");
        }
        b.stop();
    });
}

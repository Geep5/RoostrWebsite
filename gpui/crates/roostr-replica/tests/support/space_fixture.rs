//! The shared-space world of the sync authority, checkpoint and history
//! tests: fixed owner / member / stranger keys (secrets of bytes 1, 2, 3),
//! space "space" under key bytes 4 (key id 1), changes addressed by the
//! engine, and relay events hand-assembled the way a publisher sends them
//! (whole, or split into `["c", gid, i, n]` parts).
//! Include with `#[path = "support/space_fixture.rs"] mod space_fixture;`.
#![allow(dead_code)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use roostr_engine::{Change, Engine, Op, RelayEvent};
use roostr_nostr::{Event, EventTemplate, Keys, kinds};
use roostr_replica::sync::SharedSpaceInfo;
use roostr_replica_store::ChangeRecord;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const SPACE_ID: &str = "space";

pub fn keys(byte: u8) -> Keys {
    Keys::from_secret_hex(&format!("{byte:02x}").repeat(32)).unwrap()
}

pub struct World {
    pub engine: Engine,
    pub owner: Keys,
    pub member: Keys,
    pub stranger: Keys,
    pub space_key: [u8; 32],
    /// `blind(key, "space:space")`: the space stream's `#h`.
    pub space_tag: String,
}

impl World {
    pub async fn new(engine: Engine) -> World {
        let space_key = [4u8; 32];
        let space_tag = engine
            .blind_space(&hex::encode(space_key), &format!("space:{SPACE_ID}"))
            .await
            .unwrap();
        World {
            engine,
            owner: keys(1),
            member: keys(2),
            stranger: keys(3),
            space_key,
            space_tag,
        }
    }

    pub fn key_hex(&self) -> String {
        hex::encode(self.space_key)
    }

    /// The owner's personal NIP-44 conversation key.
    pub fn personal_key(&self) -> [u8; 32] {
        roostr_nostr::nip44::conversation_key(&self.owner, self.owner.pubkey()).unwrap()
    }

    /// The space as the owner's replica installs it: writers owner and member.
    pub fn space(&self) -> SharedSpaceInfo {
        SharedSpaceInfo {
            space_id: SPACE_ID.into(),
            key_hex: self.key_hex(),
            key_id: 1,
            writers: vec![
                self.owner.pubkey().to_string(),
                self.member.pubkey().to_string(),
            ],
            owner: Some(self.owner.pubkey().to_string()),
        }
    }

    /// A change by the owner at timestamp 100, addressed by the engine.
    pub async fn change(
        &self,
        object_id: &str,
        ops: Value,
        parents: &[&str],
        snapshot: Option<Value>,
    ) -> Change {
        let ops: Vec<Op> =
            serde_json::from_value(roostr_engine::pack_core_value_maps(&ops)).expect("ops");
        let mut change = Change {
            id: String::new(),
            object_id: object_id.into(),
            parent_ids: parents.iter().map(|p| p.to_string()).collect(),
            ops,
            timestamp: 100,
            author: self.owner.pubkey().to_string(),
            snapshot: snapshot.map(|s| roostr_engine::pack_core_value_maps(&s)),
            b: None,
        };
        change.id = self.engine.change_id(&change).await.unwrap();
        change
    }

    /// The space object: a channel whose `members` makes `member` a writer.
    pub async fn create_space(&self) -> Change {
        let members = json!({"valuesValue": {"items": [{"mapValue": {"entries": {
            "npub": {"stringValue": self.member.npub()},
            "role": {"stringValue": "writer"},
        }}}]}});
        self.change(
            SPACE_ID,
            json!([
                {"objectCreate": {"typeKey": "channel"}},
                {"fieldSet": {"key": "members", "value": members}},
            ]),
            &[],
            None,
        )
        .await
    }

    pub async fn bytes(&self, change: &Change) -> Vec<u8> {
        self.engine.encode_change(change).await.unwrap()
    }

    pub async fn base64(&self, change: &Change) -> String {
        STANDARD.encode(self.bytes(change).await)
    }

    /// The stored row of a change, as an import writes it.
    pub async fn record(&self, change: &Change) -> ChangeRecord {
        crate::support::encode(&self.engine, change.clone()).await.0
    }

    /// The state `changes` replay to (the TS `computeObject`).
    pub async fn state(&self, changes: &[Change]) -> Value {
        self.engine.replay(changes, None).await.unwrap().unwrap()
    }

    /// A space-stream event: `part` (base64 payload text) sealed under `key`
    /// (the space key unless given), tagged into the space stream plus `tags`.
    pub fn event(
        &self,
        kind: u32,
        part: &str,
        signer: &Keys,
        tags: &[Vec<String>],
        key: Option<&[u8; 32]>,
        created_at: u64,
    ) -> Event {
        let key = key.unwrap_or(&self.space_key);
        let mut all = vec![vec!["h".to_string(), self.space_tag.clone()]];
        all.extend(tags.iter().cloned());
        signer.sign(EventTemplate {
            kind,
            created_at,
            tags: all,
            content: roostr_nostr::nip44::encrypt(part, key).unwrap(),
        })
    }

    /// A whole change as one space-stream 1078 at `created_at`.
    pub async fn change_event(&self, change: &Change, signer: &Keys, created_at: u64) -> Event {
        let part = self.base64(change).await;
        self.event(kinds::CHANGE, &part, signer, &[], None, created_at)
    }

    /// A change's payload split in two halves under the group id the wire
    /// derives from the whole payload: (parts, gid).
    pub async fn chunks(&self, change: &Change) -> (Vec<String>, String) {
        let full = self.base64(change).await;
        let cut = full.len() / 2;
        let gid = hex::encode(Sha256::digest(full.as_bytes()))[..16].to_string();
        (vec![full[..cut].to_string(), full[cut..].to_string()], gid)
    }

    /// `["c", gid, index, total]`.
    pub fn c_tag(gid: &str, index: &str, total: &str) -> Vec<Vec<String>> {
        vec![vec!["c".into(), gid.into(), index.into(), total.into()]]
    }

    /// Both halves of `change` as space-stream parts signed by `signer`.
    pub async fn chunk_events(
        &self,
        change: &Change,
        signer: &Keys,
        created_at: u64,
    ) -> Vec<Event> {
        let (parts, gid) = self.chunks(change).await;
        parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                self.event(
                    kinds::CHANGE,
                    part,
                    signer,
                    &Self::c_tag(&gid, &i.to_string(), "2"),
                    None,
                    created_at,
                )
            })
            .collect()
    }
}

/// An event as the engine session receives it.
pub fn relay_event(event: &Event) -> RelayEvent {
    RelayEvent {
        pubkey: event.pubkey.clone(),
        created_at: event.created_at as i64,
        kind: event.kind as u16,
        tags: event.tags.clone(),
        content: event.content.clone(),
    }
}

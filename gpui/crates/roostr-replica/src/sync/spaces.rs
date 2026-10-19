//! Shared-space duties on the wire (sync.ts "Gift wraps" and owner duties):
//! space keys in by gift wrap, join requests in, invites out, the relay
//! write-allowlist and the h-deletion of a vanished space.
//!
//! Device-local bookkeeping lives in [`crate::local::LocalKv`] under the
//! Svelte app's `localStorage` names, so a browser replica and this one
//! agree on what was already sent or seen.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use roostr_nostr::nip59::{self, JoinRequest as JoinRumor, SpaceInvite};
use roostr_nostr::relay::{publish_any, query_many};
use roostr_nostr::{Event, EventTemplate, Filter, kinds, profile};

use super::invite::{JoinRequest, record_join_request};
use super::{Inner, PAGE_TIMEOUT, PUBLISH_TIMEOUT, now_ms, now_s};

const WRAPS_SEEN: &str = "roostr-wraps-seen";
const WRAPS_SEEN_KEPT: usize = 2000;
const INVITES_SENT: &str = "roostr-invites-sent";
const SPACE_DELETIONS_SENT: &str = "roostr-space-deletions-sent";
/// How long a requester's kind-0 lookup may take.
const PROFILE_TIMEOUT: Duration = Duration::from_secs(8);

impl Inner {
    fn read_json<T: serde::de::DeserializeOwned + Default>(&self, key: &str) -> T {
        self.local
            .get(key)
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn write_json<T: serde::Serialize + ?Sized>(&self, key: &str, value: &T) {
        match serde_json::to_string(value) {
            Ok(raw) => self.local.set(key, Some(&raw)),
            Err(error) => log::warn!("[sync] {key}: {error}"),
        }
    }

    /// Every gift wrap addressed to us, oldest first.
    pub(super) async fn fetch_wraps(&self) {
        let filter = Filter::new()
            .kinds([kinds::GIFT_WRAP])
            .tag('p', [self.pk.clone()]);
        let Ok(mut wraps) = query_many(&self.relays, &[filter], PAGE_TIMEOUT).await else {
            return; // unreachable relays: the live subscription catches up
        };
        wraps.sort_by_key(|w| w.created_at);
        for wrap in &wraps {
            self.handle_wrap(wrap).await;
        }
    }

    pub(super) async fn handle_wrap(&self, event: &Event) {
        let mut seen: Vec<String> = self.read_json(WRAPS_SEEN);
        if seen.contains(&event.id) {
            return;
        }
        seen.push(event.id.clone());
        let skip = seen.len().saturating_sub(WRAPS_SEEN_KEPT);
        self.write_json(WRAPS_SEEN, &seen[skip..]);
        let Ok(rumor) = nip59::unwrap(event, &self.keys) else {
            return; // not ours / garbled
        };
        if let Some(invite) = SpaceInvite::parse(&rumor) {
            // A vanished or left space stays gone: a late key must not
            // replace the one its h-deletion is addressed with.
            if self.host.space_gone(&invite.space) {
                return;
            }
            match self.keyring.accept_invite(
                &invite.space,
                &rumor.pubkey,
                &invite.key,
                invite.key_id,
                &self.pk,
            ) {
                Ok(true) => self.host.on_space_key(),
                Ok(false) => {}
                Err(error) => log::warn!("[sync] space key import: {error}"),
            }
        } else if let Some(request) = JoinRumor::parse(&rumor) {
            // Only the administrator of the space collects requests.
            let administers = self
                .state
                .lock()
                .shared
                .get(&request.space)
                .is_some_and(|sp| sp.info.owner.as_deref().is_none_or(|o| o == self.pk));
            if !administers {
                return;
            }
            // The requester's public profile, so the owner can put a face to the knock.
            let profile = profile::fetch(&self.relays, &rumor.pubkey, PROFILE_TIMEOUT)
                .await
                .ok()
                .flatten();
            let name = profile
                .as_ref()
                .and_then(|p| profile::display_name(p))
                .map(str::to_string);
            let picture = profile
                .as_ref()
                .and_then(|p| p.get("picture")?.as_str().map(str::to_string));
            record_join_request(
                self.local.as_ref(),
                JoinRequest {
                    key: String::new(),
                    space: request.space,
                    space_name: String::new(),
                    requester_npub: roostr_nostr::bech32::encode(
                        "npub",
                        &hex::decode(&rumor.pubkey).unwrap_or_default(),
                    ),
                    requester: rumor.pubkey,
                    name,
                    picture,
                    at: now_ms(),
                },
            );
            self.host.on_join_request();
        }
    }

    /// Owner duty: gift-wrap the current key to every member that has not
    /// received this keyId yet. Idempotent per (member, keyId).
    pub(super) async fn send_invite_wraps(
        &self,
        space_id: String,
        key_hex: String,
        key_id: i64,
        name: String,
        members: Vec<String>,
    ) {
        let invite = SpaceInvite {
            space: space_id.clone(),
            name,
            key: key_hex,
            key_id: key_id.max(1) as u64,
        };
        for member in members {
            if member == self.pk {
                continue;
            }
            let key = format!("{space_id}/{member}");
            let sent: BTreeMap<String, i64> = self.read_json(INVITES_SENT);
            if sent.get(&key) == Some(&key_id) {
                continue;
            }
            let Ok(wrap) = invite.wrap(&self.keys, &member) else {
                continue; // not a valid pubkey
            };
            if publish_any(&self.relays, &wrap, PUBLISH_TIMEOUT)
                .await
                .is_ok()
            {
                // Re-read: another invite may have been recorded meanwhile.
                let mut sent: BTreeMap<String, i64> = self.read_json(INVITES_SENT);
                sent.insert(key, key_id);
                self.write_json(INVITES_SENT, &sent);
            }
            // Refused: retried on the next refresh.
        }
    }

    /// Owner duty: the relay write-allowlist (kind 30100).
    pub(super) async fn publish_allowlist(&self, writers: Vec<String>) {
        let mut tags = vec![vec!["d".to_string(), kinds::ALLOWLIST_D.to_string()]];
        tags.extend(
            writers
                .into_iter()
                .filter(|w| *w != self.pk)
                .map(|w| vec!["p".to_string(), w]),
        );
        let event = self.keys.sign(EventTemplate {
            kind: kinds::ALLOWLIST,
            created_at: now_s() as u64,
            tags,
            content: String::new(),
        });
        // Retried on the next refresh.
        let _ = publish_any(&self.relays, &event, PUBLISH_TIMEOUT).await;
    }

    /// The h-deletion of a vanished space: asks the relays to drop every
    /// event in the space this identity authored - and, from the owner,
    /// every event naming it as owner, which is also the signal members
    /// vanish the space on. Sent until a relay accepts it, then never again.
    pub(super) async fn publish_space_deletion(&self, space_id: String, key_hex: String) {
        let sent: BTreeSet<String> = self.read_json(SPACE_DELETIONS_SENT);
        if self.stopped() || sent.contains(&space_id) {
            return;
        }
        let tag = match self
            .engine
            .blind_space(&key_hex, &format!("space:{space_id}"))
            .await
        {
            Ok(tag) => tag,
            Err(error) => {
                log::warn!("[sync] space deletion of {space_id}: {error}");
                return;
            }
        };
        let mut tags = vec![vec!["h".to_string(), tag]];
        tags.extend(
            kinds::DAG
                .iter()
                .map(|k| vec!["k".to_string(), k.to_string()]),
        );
        let event = self.keys.sign(EventTemplate {
            kind: kinds::DELETION,
            created_at: now_s() as u64,
            tags,
            content: "space deleted".into(),
        });
        if publish_any(&self.relays, &event, PUBLISH_TIMEOUT)
            .await
            .is_ok()
        {
            let mut sent: BTreeSet<String> = self.read_json(SPACE_DELETIONS_SENT);
            sent.insert(space_id);
            self.write_json(SPACE_DELETIONS_SENT, &sent);
        }
    }
}

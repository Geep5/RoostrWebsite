//! Direct-line signals (kind [`kinds::SIGNAL`], `roostr_direct`): ephemeral
//! introductions between this device and the computers running agents.
//!
//! Sealed and routed like the live progress events: the account's own scope
//! with its self conversation key and no tags, a shared space's with the
//! space key and `h` = the space tag. Trust is by signer: one signed by this
//! account is personal scope wherever it travelled; another signer counts
//! only in a space it may write in.

use anyhow::{Context, anyhow};
use roostr_nostr::relay::publish_any;
use roostr_nostr::{Event, EventTemplate, Filter, kinds, nip44};
use serde_json::Value as Json;

use super::{Inner, PUBLISH_TIMEOUT, now_s};

/// Largest content opened (a 16 KiB sdp, sealed and base64'd, fits).
const CONTENT_LIMIT: usize = 64 * 1024;

fn key_bytes(hex_key: &str) -> Option<[u8; 32]> {
    hex::decode(hex_key).ok()?.try_into().ok()
}

impl Inner {
    /// Signals from now on: this account's and every shared space's.
    pub(super) fn signal_filters(&self) -> Vec<Filter> {
        let now = now_s() as u64;
        let mut filters = vec![
            Filter::new()
                .kinds([kinds::SIGNAL])
                .authors([self.pk.clone()])
                .since(now),
        ];
        let tags: Vec<String> = self
            .state
            .lock()
            .shared
            .values()
            .map(|sp| sp.tag.clone())
            .collect();
        if !tags.is_empty() {
            filters.push(
                Filter::new()
                    .kinds([kinds::SIGNAL])
                    .tag('h', tags)
                    .since(now),
            );
        }
        filters
    }

    /// A signal from the relays: opened, gated and handed to the host with
    /// its scope.
    pub(super) fn handle_signal(&self, event: &Event) {
        if let Some((payload, space)) = self.open_signal(event) {
            self.host.on_signal(payload, space);
        }
    }

    fn open_signal(&self, event: &Event) -> Option<(Json, Option<String>)> {
        if event.content.len() > CONTENT_LIMIT || !event.verify() {
            return None;
        }
        let own = event.pubkey == self.pk;
        let (key, space) = match event.tag("h") {
            None if own => (key_bytes(&self.conversation_key)?, None),
            None => return None,
            Some(tag) => {
                let st = self.state.lock();
                let sp = st.shared.values().find(|sp| sp.tag == tag)?;
                let writes = own
                    || sp.writers.contains(&event.pubkey)
                    || sp.info.owner.as_deref().unwrap_or(&self.pk) == event.pubkey;
                if !writes {
                    return None;
                }
                (key_bytes(&sp.info.key_hex)?, Some(sp.info.space_id.clone()))
            }
        };
        let text = nip44::decrypt(&event.content, &key).ok()?;
        let payload = serde_json::from_str::<Json>(&text).ok()?;
        payload
            .is_object()
            .then(|| (payload, if own { None } else { space }))
    }

    /// Seal and send one signal payload: in `space` (`None` = this
    /// account's own scope), or - a hello - in every scope.
    pub(super) async fn publish_signal(
        &self,
        payload: Json,
        space: Option<String>,
    ) -> anyhow::Result<()> {
        let hello = payload.get("type").and_then(Json::as_str) == Some("hello");
        let content = payload.to_string();
        let seals: Vec<([u8; 32], Vec<Vec<String>>)> = {
            let st = self.state.lock();
            let mut seals = Vec::new();
            if hello || space.is_none() {
                let key = key_bytes(&self.conversation_key).context("the self conversation key")?;
                seals.push((key, Vec::new()));
            }
            for sp in st.shared.values() {
                if (hello || space.as_deref() == Some(sp.info.space_id.as_str()))
                    && let Some(key) = key_bytes(&sp.info.key_hex)
                {
                    seals.push((key, vec![vec!["h".to_string(), sp.tag.clone()]]));
                }
            }
            seals
        };
        if seals.is_empty() {
            return Err(anyhow!("no key for space {}", space.unwrap_or_default()));
        }
        let mut sent = 0;
        let mut failure = None;
        for (key, tags) in seals {
            let sealed = nip44::encrypt(&content, &key).map_err(|e| anyhow!("nip44: {e}"))?;
            let event = self.keys.sign(EventTemplate {
                kind: kinds::SIGNAL,
                created_at: now_s() as u64,
                tags,
                content: sealed,
            });
            match publish_any(&self.relays, &event, PUBLISH_TIMEOUT).await {
                Ok(()) => sent += 1,
                Err(error) => failure = Some(error),
            }
        }
        match failure {
            Some(error) if sent == 0 => Err(anyhow!("no relay took the signal: {error}")),
            _ => Ok(()),
        }
    }
}

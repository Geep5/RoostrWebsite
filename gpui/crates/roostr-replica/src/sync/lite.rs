//! A lite session (`docs/state-sync.md`, "Directories"): the replica holds
//! only part of the vault. It walks whole only the streams it holds whole
//! ([`LiteScopes`]: the personal stream, chosen shared spaces) and follows
//! every other object it holds by that object's blinded `h` tag - in the
//! history pass ([`Inner::walk_followed`]) and on the live subscription.
//! Anything else is fetched by id when it is needed
//! ([`RelaySync::fetch_objects`]): bases first (`#h`, then their 1080 parts
//! by `#c`), then the live deltas of the base it now holds (`#b`) and those
//! since its base's day, or the whole history of an object no computer has
//! based yet - the device-sync rules for one object. A round in which a
//! shared space's rules refused a payload (it arrived ahead of what
//! authorizes it) is repeated, as a later reconcile would re-judge it.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use roostr_nostr::relay::query_many;
use roostr_nostr::{Filter, kinds};
use serde::{Deserialize, Serialize};

use super::history::{Pass, blind};
use super::{BASE_HASH_BATCH, BASE_LOOKBACK_S, Inner, NOTIFY_DEBOUNCE, PAGE_TIMEOUT, RelaySync};

/// Objects fetched by id per round of requests: each brings one `h` tag per
/// installed key (personal plus every shared space).
const FETCH_IDS_BATCH: usize = 40;
/// Rounds a fetch repeats while a space's rules refused something in the
/// last one (a payload that arrived ahead of what authorizes it).
const FETCH_ROUNDS: usize = 3;

/// What a lite session holds whole (persisted by the backend).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiteScopes {
    /// The personal stream: personal objects and every space that is not
    /// shared (those sync in the personal stream).
    #[serde(default)]
    pub personal: bool,
    /// Shared spaces held whole (their `#h` stream).
    #[serde(default)]
    pub spaces: BTreeSet<String>,
}

/// A lite session's sync state.
pub(super) struct Follow {
    pub(super) scopes: LiteScopes,
    /// The `h` tag of every held object outside the whole streams, from the
    /// store and the space each object is in ([`Inner::refresh_followed`]).
    held: BTreeSet<String>,
    /// The tags fetched objects answered to (their state may not be
    /// replayed yet, so their space is not known).
    fetched: BTreeSet<String>,
}

impl Follow {
    fn new(scopes: LiteScopes) -> Self {
        Self {
            scopes,
            held: BTreeSet::new(),
            fetched: BTreeSet::new(),
        }
    }
}

impl RelaySync {
    /// `Some`: a lite session holding `scopes` whole and following what it
    /// holds; `None`: the whole vault (one way: a lite replica that syncs
    /// fully). Before [`RelaySync::start`] it only sets how the session
    /// starts. Once started it moves the subscriptions and runs a history
    /// pass for what is now held whole; resolves with the pass (`true` when
    /// every relay covered every stream). Going full marks the device
    /// bootstrapped once that pass completes.
    pub async fn set_lite(&self, scopes: Option<LiteScopes>) -> anyhow::Result<bool> {
        self.inner.clone().set_lite(scopes).await
    }

    /// What this session holds whole; `None` when it holds the whole vault.
    pub fn lite_scopes(&self) -> Option<LiteScopes> {
        self.inner
            .state
            .lock()
            .lite
            .as_ref()
            .map(|f| f.scopes.clone())
    }

    /// Started and not stopped: fetches have a session to land in.
    pub fn running(&self) -> bool {
        let st = self.inner.state.lock();
        st.started && !st.stopped
    }

    /// Lite: fetch these objects from the relays and follow them from then
    /// on. Ids no relay has are skipped. `true` when every relay answered.
    pub async fn fetch_objects(&self, ids: Vec<String>) -> anyhow::Result<bool> {
        self.inner.clone().fetch_objects(ids).await
    }

    /// No relay holds any event this identity signed: a vault that was
    /// never written. An error when no relay answered.
    pub async fn personal_stream_empty(&self) -> anyhow::Result<bool> {
        let filter = Filter::new()
            .kinds(kinds::DAG)
            .authors([self.inner.pk.clone()])
            .limit(1);
        let events = query_many(&self.inner.relays, &[filter], PAGE_TIMEOUT)
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        Ok(events.is_empty())
    }
}

impl Inner {
    async fn set_lite(self: Arc<Self>, scopes: Option<LiteScopes>) -> anyhow::Result<bool> {
        let full = scopes.is_none();
        let started = {
            let mut st = self.state.lock();
            match scopes {
                Some(scopes) => match st.lite.as_mut() {
                    Some(follow) => follow.scopes = scopes,
                    None => st.lite = Some(Follow::new(scopes)),
                },
                None => st.lite = None,
            }
            st.started && !st.stopped
        };
        if !started {
            return Ok(false);
        }
        if !full {
            self.refresh_followed().await?;
        }
        if self.state.lock().live_up {
            self.subscribe_live();
        }
        // What is held whole now arrives by reconcile (whatever its cursor);
        // a device going full walks the vault as a fresh one does.
        let bootstrapped = self.store.bootstrapped().await?;
        let floor = if full && !bootstrapped {
            self.store.bootstrap_floor().await?
        } else {
            None
        };
        let complete = self.backfill(1, floor).await?;
        if complete && full && !bootstrapped && !self.stopped() {
            self.mark_bootstrapped().await;
        }
        if !self.stopped() {
            self.emit_live_status();
        }
        Ok(complete)
    }

    /// Every stream this session walks whole: (the personal stream, the
    /// shared spaces' `#h` tags). A full session: all of them.
    pub(super) fn whole_streams(&self) -> (bool, Vec<String>) {
        let st = self.state.lock();
        match &st.lite {
            None => (true, st.shared.values().map(|sp| sp.tag.clone()).collect()),
            Some(follow) => (
                follow.scopes.personal,
                st.shared
                    .values()
                    .filter(|sp| follow.scopes.spaces.contains(&sp.info.space_id))
                    .map(|sp| sp.tag.clone())
                    .collect(),
            ),
        }
    }

    /// The `h` tags a lite session follows outside its whole streams
    /// (sorted); empty in a full session.
    pub(super) fn followed_tags(&self) -> Vec<String> {
        let st = self.state.lock();
        st.lite
            .as_ref()
            .map(|follow| follow.held.union(&follow.fetched).cloned().collect())
            .unwrap_or_default()
    }

    /// Lite: follow every object the store holds outside the whole streams,
    /// by the tag of the stream it is in (its space's key when the space is
    /// shared here, else the personal secret).
    pub(super) async fn refresh_followed(self: &Arc<Self>) -> anyhow::Result<()> {
        if self.state.lock().lite.is_none() {
            return Ok(());
        }
        let ids = self.store.object_ids().await?;
        let secret = hex::decode(&self.secret_hex)?;
        // The host's lock, never under ours.
        let located: Vec<(String, String)> = ids
            .into_iter()
            .map(|id| {
                let space = self.host.space_of(&id);
                (id, space)
            })
            .collect();
        let mut st = self.state.lock();
        let keys: HashMap<String, String> = st
            .shared
            .values()
            .map(|sp| (sp.info.space_id.clone(), sp.info.key_hex.clone()))
            .collect();
        let Some(follow) = st.lite.as_mut() else {
            return Ok(());
        };
        let mut held = BTreeSet::new();
        for (id, space) in &located {
            match keys.get(space) {
                Some(key) => {
                    if !follow.scopes.spaces.contains(space) {
                        held.insert(blind(key.as_bytes(), id));
                    }
                }
                None => {
                    if !follow.scopes.personal {
                        held.insert(blind(&secret, id));
                    }
                }
            }
        }
        follow.held = held;
        Ok(())
    }

    /// Lite, in the history pass: everything new of the followed objects
    /// since the persisted cursor (the device held them up to it); returns
    /// that `since`, which is what the pass covered for sure.
    pub(super) async fn walk_followed(self: &Arc<Self>, pass: &Pass) -> anyhow::Result<i64> {
        let saved = self.store.cursor().await?;
        let since = if saved > 0 { saved + 1 } else { 1 };
        let tags = self.followed_tags();
        if tags.is_empty() || self.relays.is_empty() {
            return Ok(since);
        }
        let filters: Vec<Filter> = tags
            .chunks(BASE_HASH_BATCH)
            .map(|chunk| Filter::new().kinds(kinds::DAG).tag('h', chunk.iter().cloned()))
            .collect();
        self.walk_filters(pass, &filters, since).await?;
        self.fetch_base_parts(pass).await?;
        Ok(since)
    }

    /// Every relay × filter walked from `since` at once.
    async fn walk_filters(
        self: &Arc<Self>,
        pass: &Pass,
        filters: &[Filter],
        since: i64,
    ) -> anyhow::Result<()> {
        let jobs = (0..self.relays.len()).flat_map(|index| {
            filters
                .iter()
                .map(move |filter| self.walk(index, pass, filter, since, None, false))
        });
        for result in futures::future::join_all(jobs).await {
            result?;
        }
        Ok(())
    }

    async fn fetch_objects(self: Arc<Self>, ids: Vec<String>) -> anyhow::Result<bool> {
        if ids.is_empty() || self.relays.is_empty() {
            return Ok(true);
        }
        if self.stopped() {
            return Ok(false);
        }
        self.ensure_session().await?;
        let secret = hex::decode(&self.secret_hex)?;
        let keys: Vec<String> = self
            .state
            .lock()
            .shared
            .values()
            .map(|sp| sp.info.key_hex.clone())
            .collect();
        // tag → object id: the personal tag and one per installed space key
        // (which stream the object is in is not known before it arrives).
        let mut candidates: HashMap<String, String> = HashMap::new();
        let mut answered = BTreeSet::new();
        let mut complete = true;
        for chunk in ids.chunks(FETCH_IDS_BATCH) {
            let mut tags_of: Vec<(&str, Vec<String>)> = Vec::new();
            for id in chunk {
                let mut tags = vec![blind(&secret, id)];
                tags.extend(keys.iter().map(|key| blind(key.as_bytes(), id)));
                for tag in &tags {
                    candidates.insert(tag.clone(), id.clone());
                }
                tags_of.push((id.as_str(), tags));
            }
            // A space's rules judge a payload by the space object as held
            // then: one that arrived ahead of what authorizes it (the same
            // second, a later page) is refused, and asked again next round.
            let mut round_complete = true;
            for _ in 0..FETCH_ROUNDS {
                let refused = self.state.lock().stats.refused;
                let pass = Pass::new(self.relays.len(), 1, None).watching_tags();
                self.fetch_round(&pass, &tags_of).await?;
                answered.extend(pass.tags_seen());
                round_complete = pass.complete.load(Ordering::Relaxed);
                if self.stopped() || self.state.lock().stats.refused == refused {
                    break;
                }
            }
            complete &= round_complete;
            if self.stopped() {
                complete = false;
                break;
            }
        }
        // Follow what answered, under the tag it answered to.
        let answered: Vec<String> = answered
            .into_iter()
            .filter(|tag| candidates.contains_key(tag))
            .collect();
        let fresh = {
            let mut st = self.state.lock();
            match st.lite.as_mut() {
                Some(follow) => {
                    let before = follow.fetched.len();
                    follow.fetched.extend(answered);
                    follow.fetched.len() > before
                }
                None => false,
            }
        };
        if fresh {
            self.schedule_resubscribe();
        }
        self.drain_inflight().await;
        self.flush_object_notify();
        self.persist_cursor().await?;
        Ok(complete)
    }

    /// One round of [`RelaySync::fetch_objects`] for `tags_of` (each id with
    /// its candidate `h` tags): bases first, and the parts of a split one;
    /// then the deltas - of an object holding a base those on it and those
    /// since its base's day (an old client's, without `b`), of any other
    /// its whole history.
    async fn fetch_round(
        self: &Arc<Self>,
        pass: &Pass,
        tags_of: &[(&str, Vec<String>)],
    ) -> anyhow::Result<()> {
        let all: Vec<String> = tags_of.iter().flat_map(|(_, tags)| tags.iter().cloned()).collect();
        let bases: Vec<Filter> = all
            .chunks(BASE_HASH_BATCH)
            .map(|tags| Filter::new().kinds([kinds::BASE]).tag('h', tags.iter().cloned()))
            .collect();
        self.walk_filters(pass, &bases, 1).await?;
        self.fetch_base_parts(pass).await?;
        let mut based_tags = Vec::new();
        let mut hashes = BTreeSet::new();
        let mut floor = i64::MAX;
        let mut unbased_tags = Vec::new();
        for (id, tags) in tags_of {
            match self.store.current_base(id.to_string()).await? {
                Some(base) => {
                    based_tags.extend(tags.iter().cloned());
                    hashes.insert(base.hash);
                    floor = floor.min((base.created_at / 1000 - BASE_LOOKBACK_S).max(1));
                }
                None => unbased_tags.extend(tags.iter().cloned()),
            }
        }
        let mut deltas = Vec::new();
        let hashes: Vec<String> = hashes.into_iter().collect();
        for tags in based_tags.chunks(BASE_HASH_BATCH) {
            for chunk in hashes.chunks(BASE_HASH_BATCH) {
                deltas.push(
                    Filter::new()
                        .kinds([kinds::CHANGE])
                        .tag('h', tags.iter().cloned())
                        .tag('b', chunk.iter().cloned()),
                );
            }
            deltas.push(
                Filter::new()
                    .kinds([kinds::CHANGE])
                    .tag('h', tags.iter().cloned())
                    .since(floor.max(1) as u64),
            );
        }
        for tags in unbased_tags.chunks(BASE_HASH_BATCH) {
            deltas.push(
                Filter::new()
                    .kinds([kinds::CHECKPOINT, kinds::CHANGE])
                    .tag('h', tags.iter().cloned()),
            );
        }
        self.walk_filters(pass, &deltas, 1).await
    }

    /// The live subscription again shortly (new objects to follow), once
    /// for any number of fetches in between.
    fn schedule_resubscribe(self: &Arc<Self>) {
        {
            let mut st = self.state.lock();
            if st.resubscribe_scheduled || !st.live_up || st.stopped {
                return;
            }
            st.resubscribe_scheduled = true;
        }
        let this = self.clone();
        crate::rt::spawn(async move {
            roostr_nostr::sleep(NOTIFY_DEBOUNCE).await;
            this.state.lock().resubscribe_scheduled = false;
            if !this.stopped() {
                this.subscribe_live();
            }
        });
    }
}

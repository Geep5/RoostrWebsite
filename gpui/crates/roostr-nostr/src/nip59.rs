//! NIP-59 gift wraps, byte-compatible with nostr-tools' `nip59` (sync.ts
//! `wrapEvent`/`unwrapEvent`) and GiftWrap.swift: an unsigned rumor is
//! sealed (kind 13, NIP-44 under sender↔recipient, signed by the sender)
//! and the seal is wrapped (kind 1059, NIP-44 under a one-off key ↔
//! recipient, signed by that key, tagged `p` recipient). Both `created_at`
//! are randomized up to two days back so relays learn no timing.
//!
//! Also the two rumors Roostr sends this way: a space key to a member
//! ([`SpaceInvite`]) and a join request to a space owner ([`JoinRequest`]).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::event::{Event, EventTemplate, event_id};
use crate::keys::{KeyError, Keys, is_hex32};
use crate::nip44::{self, Nip44Error};
use crate::{kinds, random, time};

/// How far back a seal's and a wrap's `created_at` may be randomized.
pub const TWO_DAYS_S: u64 = 2 * 24 * 60 * 60;
/// Gift wraps randomize `created_at` up to two days back: a live
/// subscription for them looks back this far (sync.ts `WRAP_LOOKBACK_S`).
pub const WRAP_LOOKBACK_S: u64 = 3 * 86_400;

/// Why a gift wrap could not be made or opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Nip59Error {
    NotWrap(u32),
    NotSeal(u32),
    BadSealSignature,
    /// The rumor claims an author other than the seal's signer.
    RumorPubkeyMismatch,
    Key(KeyError),
    Nip44(Nip44Error),
    Malformed(String),
}

impl fmt::Display for Nip59Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotWrap(kind) => write!(f, "not a gift wrap (kind {kind})"),
            Self::NotSeal(kind) => write!(f, "not a seal (kind {kind})"),
            Self::BadSealSignature => f.write_str("the seal's signature does not verify"),
            Self::RumorPubkeyMismatch => f.write_str("the rumor's author is not the seal's signer"),
            Self::Key(e) => e.fmt(f),
            Self::Nip44(e) => e.fmt(f),
            Self::Malformed(what) => write!(f, "malformed gift wrap: {what}"),
        }
    }
}

impl std::error::Error for Nip59Error {}

impl From<KeyError> for Nip59Error {
    fn from(e: KeyError) -> Self {
        Self::Key(e)
    }
}

impl From<Nip44Error> for Nip59Error {
    fn from(e: Nip44Error) -> Self {
        Self::Nip44(e)
    }
}

/// An unsigned inner event; `id` is its NIP-01 hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rumor {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
}

impl Rumor {
    /// nostr-tools `createRumor`: authored by `sender`, stamped `created_at`.
    pub fn new(sender: &Keys, template: EventTemplate) -> Self {
        let EventTemplate {
            kind,
            created_at,
            tags,
            content,
        } = template;
        let id = event_id(sender.pubkey(), created_at, kind, &tags, &content);
        Self {
            id,
            pubkey: sender.pubkey().to_string(),
            created_at,
            kind,
            tags,
            content,
        }
    }
}

/// `now` minus up to two days, uniformly (nostr-tools `randomNow`).
pub fn random_past(now: u64) -> u64 {
    now.saturating_sub(random::up_to(TWO_DAYS_S))
}

/// Seal `rumor` from `sender` to the hex pubkey `recipient` (kind 13).
pub fn seal(
    rumor: &Rumor,
    sender: &Keys,
    recipient: &str,
    created_at: u64,
) -> Result<Event, Nip59Error> {
    let plaintext = serde_json::to_string(rumor).expect("a rumor always serializes");
    let content = nip44::encrypt(&plaintext, &nip44::conversation_key(sender, recipient)?)?;
    Ok(sender.sign(EventTemplate {
        kind: kinds::SEAL,
        created_at,
        tags: Vec::new(),
        content,
    }))
}

/// Wrap `seal` for `recipient` under a fresh one-off key (kind 1059).
pub fn wrap_seal(seal: &Event, recipient: &str, created_at: u64) -> Result<Event, Nip59Error> {
    let ephemeral = Keys::generate();
    let content = nip44::encrypt(
        &seal.to_json(),
        &nip44::conversation_key(&ephemeral, recipient)?,
    )?;
    Ok(ephemeral.sign(EventTemplate {
        kind: kinds::GIFT_WRAP,
        created_at,
        tags: vec![vec!["p".into(), recipient.to_string()]],
        content,
    }))
}

/// nostr-tools `wrapEvent`: rumor stamped now, seal and wrap randomized.
pub fn wrap(
    sender: &Keys,
    recipient: &str,
    kind: u32,
    tags: Vec<Vec<String>>,
    content: String,
) -> Result<Event, Nip59Error> {
    let now = time::now_s();
    let rumor = Rumor::new(
        sender,
        EventTemplate {
            kind,
            created_at: now,
            tags,
            content,
        },
    );
    let seal = seal(&rumor, sender, recipient, random_past(now))?;
    wrap_seal(&seal, recipient, random_past(now))
}

/// Open a wrap addressed to `recipient`. Stricter than nostr-tools
/// `unwrapEvent`, like the iOS host: the seal's signature must verify and
/// the rumor must name the seal's signer.
pub fn unwrap(wrap: &Event, recipient: &Keys) -> Result<Rumor, Nip59Error> {
    if wrap.kind != kinds::GIFT_WRAP {
        return Err(Nip59Error::NotWrap(wrap.kind));
    }
    let seal_json = nip44::decrypt(
        &wrap.content,
        &nip44::conversation_key(recipient, &wrap.pubkey)?,
    )?;
    let seal: Event = serde_json::from_str(&seal_json)
        .map_err(|e| Nip59Error::Malformed(format!("seal: {e}")))?;
    if seal.kind != kinds::SEAL {
        return Err(Nip59Error::NotSeal(seal.kind));
    }
    if !seal.verify() {
        return Err(Nip59Error::BadSealSignature);
    }
    let rumor_json = nip44::decrypt(
        &seal.content,
        &nip44::conversation_key(recipient, &seal.pubkey)?,
    )?;
    let rumor: Rumor = serde_json::from_str(&rumor_json)
        .map_err(|e| Nip59Error::Malformed(format!("rumor: {e}")))?;
    if rumor.pubkey != seal.pubkey {
        return Err(Nip59Error::RumorPubkeyMismatch);
    }
    Ok(rumor)
}

/// A space key for a member (rumor kind 24891):
/// `{t:"space-invite", space, name, key, keyId}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceInvite {
    pub space: String,
    pub name: String,
    /// 64 lowercase hex; doubles as the space's NIP-44 conversation key.
    pub key: String,
    pub key_id: u64,
}

#[derive(Serialize)]
struct InviteContent<'a> {
    t: &'static str,
    space: &'a str,
    name: &'a str,
    key: &'a str,
    #[serde(rename = "keyId")]
    key_id: u64,
}

#[derive(Serialize)]
struct JoinRequestContent<'a> {
    t: &'static str,
    space: &'a str,
}

/// `Number.isSafeInteger(v) && v >= 1` for a JSON number.
fn safe_key_id(value: &serde_json::Value) -> Option<u64> {
    const MAX_SAFE: u64 = (1 << 53) - 1;
    let id = value.as_u64().or_else(|| {
        let f = value.as_f64()?;
        (f.fract() == 0.0 && f >= 1.0 && f <= MAX_SAFE as f64).then_some(f as u64)
    })?;
    (1..=MAX_SAFE).contains(&id).then_some(id)
}

/// The rumor content's string field `name`, when present and non-empty.
fn text_field<'a>(raw: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    raw.get(name)?.as_str().filter(|s| !s.is_empty())
}

impl SpaceInvite {
    /// The rumor's content, in sync.ts' `JSON.stringify` key order.
    pub fn content(&self) -> String {
        serde_json::to_string(&InviteContent {
            t: "space-invite",
            space: &self.space,
            name: &self.name,
            key: &self.key,
            key_id: self.key_id,
        })
        .expect("an invite always serializes")
    }

    /// sync.ts `handleWrap`'s checks: kind 24891, `t`, a space id, a 64-hex
    /// key and a safe integer `keyId >= 1`. The rumor's author is the
    /// claimed owner; the keyring decides whether to believe it
    /// ([`crate::spacekeys::SpaceKeyring::accept_invite`]).
    pub fn parse(rumor: &Rumor) -> Option<Self> {
        if rumor.kind != kinds::SPACE_INVITE_RUMOR {
            return None;
        }
        let raw: serde_json::Value = serde_json::from_str(&rumor.content).ok()?;
        if raw.get("t")?.as_str()? != "space-invite" {
            return None;
        }
        Some(Self {
            space: text_field(&raw, "space")?.to_string(),
            name: raw
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string(),
            key: raw
                .get("key")?
                .as_str()
                .filter(|k| is_hex32(k))?
                .to_string(),
            key_id: safe_key_id(raw.get("keyId")?)?,
        })
    }

    /// Gift-wrap this invite from the space owner `sender` to `member`.
    pub fn wrap(&self, sender: &Keys, member: &str) -> Result<Event, Nip59Error> {
        wrap(
            sender,
            member,
            kinds::SPACE_INVITE_RUMOR,
            Vec::new(),
            self.content(),
        )
    }
}

/// A request to join a space (rumor kind 24892): `{t:"join-request", space}`,
/// addressed to the owner from a join link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinRequest {
    pub space: String,
}

impl JoinRequest {
    pub fn content(&self) -> String {
        serde_json::to_string(&JoinRequestContent {
            t: "join-request",
            space: &self.space,
        })
        .expect("a join request always serializes")
    }

    /// Kind 24892, `t: "join-request"` and a space id.
    pub fn parse(rumor: &Rumor) -> Option<Self> {
        if rumor.kind != kinds::JOIN_REQUEST_RUMOR {
            return None;
        }
        let raw: serde_json::Value = serde_json::from_str(&rumor.content).ok()?;
        if raw.get("t")?.as_str()? != "join-request" {
            return None;
        }
        Some(Self {
            space: text_field(&raw, "space")?.to_string(),
        })
    }

    /// Gift-wrap this request from `sender` to the space owner `owner`.
    pub fn wrap(&self, sender: &Keys, owner: &str) -> Result<Event, Nip59Error> {
        wrap(
            sender,
            owner,
            kinds::JOIN_REQUEST_RUMOR,
            Vec::new(),
            self.content(),
        )
    }
}

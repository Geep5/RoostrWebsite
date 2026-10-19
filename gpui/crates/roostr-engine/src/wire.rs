//! `wire`: relay wire helpers every host shares (`core/wire.odin`): blinded
//! tags, NIP-44, chunked sealing of change and base bytes, opening received
//! parts, raw content-address verification, bech32. Signing and ECDH stay
//! with the host (secp256k1).

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::codec::to_base64;
use crate::{Change, Engine, EngineError};

/// The space a shared seal is for: its key (hex) blinds the tags, and its
/// owner (hex pubkey) rides as `["owner", …]`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SealSpace<'a> {
    pub key_hex: &'a str,
    pub space_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<&'a str>,
}

/// One change to seal as kind-1078 parts. Personal seals pass `secret` (the
/// identity's secret key hex, which blinds the `h` tag); shared ones `space`.
#[derive(Debug, Clone, Copy)]
pub struct SealRequest<'a> {
    /// Change protobuf.
    pub change: &'a [u8],
    /// NIP-44 conversation key, hex.
    pub conversation_key: &'a str,
    pub secret: Option<&'a str>,
    pub space: Option<SealSpace<'a>>,
    pub object_id: &'a str,
    /// Hash of the base the change was written on: `["b", base]`.
    pub base: Option<&'a str>,
    /// Fixed NIP-44 nonce (hex) for reproducible vectors; random when `None`.
    pub nonce: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SealPayload<'a> {
    action: &'a str,
    change: String,
    conversation_key: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    space: Option<SealSpace<'a>>,
    object_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SealBasePayload<'a> {
    action: &'a str,
    base: String,
    conversation_key: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    space: Option<SealSpace<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<&'a str>,
}

/// Sealed parts, each to be signed as one event; `gid` is "" when unsplit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    pub gid: String,
    pub parts: Vec<SealedPart>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedPart {
    pub content: String,
    pub tags: Vec<Vec<String>>,
}

/// A sealed base: the addressable kind-31078 event plus its kind-1080 parts,
/// which the host publishes BEFORE the 31078.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SealedBase {
    pub hash: String,
    pub object_id: String,
    pub epoch: i64,
    /// The addressable `d` tag.
    pub d: String,
    pub gid: String,
    pub event: SealedBaseEvent,
    pub parts: Vec<SealedBaseEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SealedBaseEvent {
    pub kind: u16,
    pub content: String,
    pub tags: Vec<Vec<String>>,
}

/// `verify`: the raw content address and the decoded change.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Verified {
    pub id: String,
    pub change: Change,
}

/// A decoded bech32 string.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Bech32 {
    pub hrp: String,
    pub hex: String,
}

impl Engine {
    /// Personal blinded tag: first 16 hex of sha256(secret bytes ‖ utf8(id)).
    pub async fn blind_personal(&self, secret_hex: &str, id: &str) -> Result<String, EngineError> {
        self.call_as(
            "wire",
            &json!({"action": "blind", "secret": secret_hex, "id": id}),
            None,
        )
        .await
    }

    /// Shared blinded tag: first 16 hex of sha256(utf8(key hex) ‖ utf8(id));
    /// `"space:" + spaceId` is the space-wide tag.
    pub async fn blind_space(&self, key_hex: &str, id: &str) -> Result<String, EngineError> {
        self.call_as(
            "wire",
            &json!({"action": "blind", "keyHex": key_hex, "id": id}),
            None,
        )
        .await
    }

    /// NIP-44 conversation key (hex) from the 32-byte ECDH x coordinate (hex).
    pub async fn conversation_key(&self, shared_x_hex: &str) -> Result<String, EngineError> {
        self.call_as(
            "wire",
            &json!({"action": "conversation_key", "sharedX": shared_x_hex}),
            None,
        )
        .await
    }

    /// NIP-44 v2 encryption; `nonce` (hex) only for reproducible vectors.
    pub async fn nip44_encrypt(
        &self,
        conversation_key: &str,
        plaintext: &str,
        nonce: Option<&str>,
    ) -> Result<String, EngineError> {
        let mut payload = json!({"action": "encrypt", "conversationKey": conversation_key, "plaintext": plaintext});
        if let Some(nonce) = nonce {
            payload["nonce"] = nonce.into();
        }
        self.call_as("wire", &payload, None).await
    }

    pub async fn nip44_decrypt(
        &self,
        conversation_key: &str,
        payload: &str,
    ) -> Result<String, EngineError> {
        let payload =
            json!({"action": "decrypt", "conversationKey": conversation_key, "payload": payload});
        self.call_as("wire", &payload, None).await
    }

    /// Splits a change's base64 into ≤40k-character NIP-44 sealed parts with
    /// the stream tags (`h`, and for shared objects the space-wide `h` and
    /// `owner`), `["b", …]` and, when split, `["c", gid, i, n]`.
    pub async fn seal(&self, request: &SealRequest<'_>) -> Result<Sealed, EngineError> {
        let payload = SealPayload {
            action: "seal",
            change: to_base64(request.change),
            conversation_key: request.conversation_key,
            secret: request.secret,
            space: request.space,
            object_id: request.object_id,
            base: request.base,
            nonce: request.nonce,
        };
        self.call_as("wire", &payload, None).await
    }

    /// Seals a base (Checkpoint protobuf, epoch >= 1) as its 31078 event and 1080 parts.
    pub async fn seal_base(
        &self,
        base: &[u8],
        conversation_key: &str,
        secret: Option<&str>,
        space: Option<SealSpace<'_>>,
        nonce: Option<&str>,
    ) -> Result<SealedBase, EngineError> {
        let payload = SealBasePayload {
            action: "seal_base",
            base: to_base64(base),
            conversation_key,
            secret,
            space,
            nonce,
        };
        self.call_as("wire", &payload, None).await
    }

    /// Decrypts one received part; it must be a chunk of base64.
    pub async fn open(&self, content: &str, conversation_key: &str) -> Result<String, EngineError> {
        let payload =
            json!({"action": "open", "content": content, "conversationKey": conversation_key});
        self.call_as("wire", &payload, None).await
    }

    /// Verifies reassembled change base64 (and its group id when split): the
    /// decoded id must equal the raw content address, never re-encoded.
    pub async fn verify(
        &self,
        bytes_base64: &str,
        gid: Option<&str>,
    ) -> Result<Verified, EngineError> {
        let mut payload = json!({"action": "verify", "bytes": bytes_base64});
        if let Some(gid) = gid {
            payload["gid"] = gid.into();
        }
        self.call_as("wire", &payload, None).await
    }

    /// `npub1…` / `nsec1…` from a human-readable part and hex bytes.
    pub async fn bech32_encode(&self, hrp: &str, hex: &str) -> Result<String, EngineError> {
        self.call_as(
            "wire",
            &json!({"action": "bech32_encode", "hrp": hrp, "hex": hex}),
            None,
        )
        .await
    }

    pub async fn bech32_decode(&self, text: &str) -> Result<Bech32, EngineError> {
        self.call_as(
            "wire",
            &json!({"action": "bech32_decode", "text": text}),
            None,
        )
        .await
    }
}

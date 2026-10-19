//! `codec`: change protobuf ⇄ JSON, content addresses, the checkpoint store
//! rule, and the schema messages blocks carry (`proto.ts`, Engine.swift).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{Change, Engine, EngineError};

pub(crate) fn to_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub(crate) fn from_base64(text: &str) -> Result<Vec<u8>, EngineError> {
    STANDARD
        .decode(text)
        .map_err(|error| EngineError::Decode(format!("engine returned invalid base64: {error}")))
}

/// `#[serde(with = "crate::codec::base64_bytes")]`: bytes as standard base64 text.
pub(crate) mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        STANDARD
            .decode(text.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

/// Decodes an optional base64 member (`#[serde(default, deserialize_with = …)]`).
pub(crate) fn base64_option<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<u8>>, D::Error> {
    base64_bytes::deserialize(deserializer).map(Some)
}

/// A schema message a block carries, for [`Engine::decode_message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// A thread root's `Conversation`.
    Conversation,
    /// A mailbox entry's `Agent_Message`.
    AgentMessage,
}

impl MessageKind {
    fn name(self) -> &'static str {
        match self {
            MessageKind::Conversation => "conversation",
            MessageKind::AgentMessage => "agent_message",
        }
    }
}

#[derive(Serialize)]
struct WithChange<'a> {
    action: &'a str,
    change: &'a Change,
}

#[derive(Serialize)]
struct MessageEncode<'a> {
    action: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    value: &'a Value,
}

impl Engine {
    /// Protobuf change bytes → [`Change`] (ordered-pair maps).
    pub async fn decode_change(&self, bytes: &[u8]) -> Result<Change, EngineError> {
        self.call_as(
            "codec",
            &json!({"action": "decode", "bytes": to_base64(bytes)}),
            None,
        )
        .await
    }

    /// Recomputes the content address and returns the canonical protobuf
    /// bytes, id set (the change's own `id` is ignored).
    pub async fn encode_change(&self, change: &Change) -> Result<Vec<u8>, EngineError> {
        let text: String = self
            .call_as(
                "codec",
                &WithChange {
                    action: "encode",
                    change,
                },
                None,
            )
            .await?;
        from_base64(&text)
    }

    /// The content address: sha256 hex of the encoding with the id zeroed.
    pub async fn change_id(&self, change: &Change) -> Result<String, EngineError> {
        self.call_as(
            "codec",
            &WithChange {
                action: "hash",
                change,
            },
            None,
        )
        .await
    }

    /// The one store rule for checkpoints (`core.checkpoint_supersedes`):
    /// `candidate` covers a strict superset of `existing`, or the same set
    /// with the larger hash.
    pub async fn checkpoint_supersedes(
        &self,
        candidate: &[u8],
        existing: &[u8],
    ) -> Result<bool, EngineError> {
        let payload = json!({
            "action": "checkpoint_supersedes",
            "candidate": to_base64(candidate),
            "existing": to_base64(existing),
        });
        self.call_as("codec", &payload, None).await
    }

    /// A schema message's protobuf → JSON; unknown fields survive as `unknown` base64.
    pub async fn decode_message(
        &self,
        kind: MessageKind,
        bytes: &[u8],
    ) -> Result<Value, EngineError> {
        let payload = json!({"action": "decode", "type": kind.name(), "bytes": to_base64(bytes)});
        self.call_as("codec", &payload, None).await
    }

    /// A schema message's JSON → protobuf.
    pub async fn encode_message(
        &self,
        kind: MessageKind,
        value: &Value,
    ) -> Result<Vec<u8>, EngineError> {
        let payload = MessageEncode {
            action: "encode",
            kind: kind.name(),
            value,
        };
        let text: String = self.call_as("codec", &payload, None).await?;
        from_base64(&text)
    }
}

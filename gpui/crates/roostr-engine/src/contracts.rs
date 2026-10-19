//! Serde forms of the engine contracts in `src/lib/engine/contracts.ts`, in
//! the engine's ordered-pair encoding (what `codec decode` emits and every
//! host hands back): schema maps inside values, blocks and snapshots stay
//! `[key, value]` arrays in the [`Value`]s below.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A decoded change (`ChangeJSON`), as `/api/changes` and the codec shape it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// Hex content address (sha256 of the encoding with the id zeroed); empty
    /// on a planned change the codec has not addressed yet.
    #[serde(default)]
    pub id: String,
    pub object_id: String,
    /// Hex parent ids.
    #[serde(default)]
    pub parent_ids: Vec<String>,
    #[serde(default)]
    pub ops: Vec<Op>,
    #[serde(default)]
    pub timestamp: i64,
    #[serde(default)]
    pub author: String,
    /// Snapshot state (`{id, typeKey, fields, content, blocks, deleted, createdAt, updatedAt}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Value>,
    /// Hex hash of the base its event named (`["b", …]`); `None` =
    /// pre-migration or not on a base. Host metadata, not encoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<String>,
}

/// One operation (`OpJSON`, the `glon.Operation` oneof). An operation this
/// binding does not name round-trips untouched as [`Op::Other`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Op {
    ObjectCreate {
        type_key: String,
    },
    ObjectDelete {},
    FieldSet {
        key: String,
        /// A `ValueJSON`; absent keeps the payload absent on the wire.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<Value>,
    },
    FieldDelete {
        key: String,
    },
    BlockAdd {
        #[serde(default)]
        parent_id: String,
        #[serde(default)]
        after_id: String,
        #[serde(default)]
        target_id: String,
        #[serde(default)]
        position: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block: Option<BlockWire>,
    },
    BlockRemove {
        block_id: String,
    },
    BlockUpdate {
        block_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Value>,
    },
    BlockMove {
        block_id: String,
        #[serde(default)]
        new_parent_id: String,
        #[serde(default)]
        after_id: String,
        #[serde(default)]
        target_id: String,
        #[serde(default)]
        position: i64,
    },
    BlockSetAlign {
        block_id: String,
        #[serde(default)]
        align: i64,
    },
    BlockSetBackground {
        block_id: String,
        #[serde(default)]
        color: String,
    },
    /// An operation kind outside this list (or the empty `{}` of an unset oneof).
    #[serde(untagged)]
    Other(Value),
}

/// A block as an operation carries it (`BlockWire`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockWire {
    pub id: String,
    #[serde(default)]
    pub children_ids: Vec<String>,
    /// `{text: …}` or `{custom: {contentType, data, meta}}`.
    #[serde(default)]
    pub content: Value,
    /// Every other member (`align`, `backgroundColor`, `fields`), untouched.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// Authenticated transport context of a shared-space event, never inferred
/// from the inner author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedProvenance {
    pub space_id: String,
    pub key_id: i64,
    /// Hex pubkey that signed the event.
    pub signer: String,
}

/// One object's raw history for a corpus load (`ObjectHistory`): every held
/// change of one object as stored protobuf, plus its checkpoint when held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectHistory {
    pub checkpoint: Option<Vec<u8>>,
    pub changes: Vec<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn ops_round_trip_their_wire_shape() {
        let ops = json!([
            {"objectCreate": {"typeKey": "page"}},
            {"objectDelete": {}},
            {"fieldSet": {"key": "name"}},
            {"fieldSet": {"key": "name", "value": {"stringValue": "x"}}},
            {"blockAdd": {"parentId": "", "afterId": "a", "targetId": "", "position": 0,
                          "block": {"id": "b", "childrenIds": [], "content": {"text": {"text": "t"}}, "align": 1}}},
            {"blockMove": {"blockId": "b", "newParentId": "p", "afterId": "", "targetId": "", "position": 2}},
            {"futureOp": {"x": 1}},
            {},
        ]);
        let typed: Vec<Op> = serde_json::from_value(ops.clone()).unwrap();
        assert!(matches!(typed[2], Op::FieldSet { value: None, .. }));
        assert!(matches!(typed[6], Op::Other(_)));
        assert!(matches!(typed[7], Op::Other(_)));
        assert_eq!(serde_json::to_value(&typed).unwrap(), ops);
    }
}

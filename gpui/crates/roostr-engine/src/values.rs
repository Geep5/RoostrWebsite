//! Port of `core-values.ts` / Engine.swift `packCoreValueMaps`.
//!
//! Protobuf maps carry insertion order, unlike Odin's JSON object maps. Only
//! schema map slots become ordered pairs; user map keys are never interpreted
//! as schema properties. Transport encoding, not a second domain codec.

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

/// Turns the schema map slots of `value` (`entries`, `meta`, object/snapshot
/// `fields`) from JSON objects into the engine's ordered `[key, value]`
/// pairs. Replayed object states come back as plain maps; a state handed
/// back to the engine (query upserts, mutation overlays) is packed - the
/// typed calls do it while serialising, without this copy.
pub fn pack_core_value_maps(value: &Value) -> Value {
    serde_json::to_value(Packed::new(value)).expect("JSON values serialise")
}

/// The inverse: ordered pairs back to plain JSON objects (for display; the
/// order is lost, so never re-encode an unpacked change).
pub fn unpack_core_value_maps(value: Value) -> Value {
    unpack(value, "")
}

/// Serialises a value packed, straight from the borrowed original.
pub(crate) struct Packed<'a> {
    value: &'a Value,
    slot: &'a str,
}

impl<'a> Packed<'a> {
    pub(crate) fn new(value: &'a Value) -> Self {
        Packed { value, slot: "" }
    }
}

/// Whether an object's `fields` is a block's ValueMap wrapper rather than an
/// object/snapshot's direct map.
fn is_block(slot: &str, fields: &Map<String, Value>) -> bool {
    slot == "block" || slot == "blocks" || fields.contains_key("childrenIds")
}

impl Serialize for Packed<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.value {
            Value::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for value in items {
                    seq.serialize_element(&Packed {
                        value,
                        slot: self.slot,
                    })?;
                }
                seq.end()
            }
            Value::Object(fields) => {
                let block = is_block(self.slot, fields);
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for (key, value) in fields {
                    match key.as_str() {
                        // ValueMap's only field. Its keys are opaque: never recurse into the dictionary itself.
                        "entries" => map.serialize_entry(
                            key,
                            &PackedSlot {
                                value,
                                strings: false,
                            },
                        )?,
                        "meta" => map.serialize_entry(
                            key,
                            &PackedSlot {
                                value,
                                strings: true,
                            },
                        )?,
                        "fields" if block => {
                            map.serialize_entry(key, &Packed { value, slot: "" })?
                        }
                        "fields" => map.serialize_entry(
                            key,
                            &PackedSlot {
                                value,
                                strings: false,
                            },
                        )?,
                        _ => map.serialize_entry(key, &Packed { value, slot: key })?,
                    }
                }
                map.end()
            }
            other => other.serialize(serializer),
        }
    }
}

/// A schema map slot: an object becomes `[key, value]` pairs.
struct PackedSlot<'a> {
    value: &'a Value,
    strings: bool,
}

impl Serialize for PackedSlot<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Value::Object(fields) = self.value else {
            return self.value.serialize(serializer);
        };
        let mut seq = serializer.serialize_seq(Some(fields.len()))?;
        for (key, value) in fields {
            if self.strings {
                seq.serialize_element(&(key, value))?;
            } else {
                seq.serialize_element(&(key, Packed::new(value)))?;
            }
        }
        seq.end()
    }
}

fn unpack_slot(value: Value, strings: bool) -> Value {
    let Value::Array(pairs) = value else {
        return value;
    };
    let mut out = Map::new();
    for pair in pairs {
        if let Value::Array(mut entry) = pair
            && entry.len() == 2
            && let Value::String(key) = entry.swap_remove(0)
        {
            let item = entry.swap_remove(0);
            out.insert(key, if strings { item } else { unpack(item, "") });
        }
    }
    Value::Object(out)
}

fn unpack(value: Value, slot: &str) -> Value {
    match value {
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|item| unpack(item, slot)).collect())
        }
        Value::Object(fields) => {
            let block = is_block(slot, &fields);
            let mut out = Map::new();
            for (key, item) in fields {
                let item = match key.as_str() {
                    "entries" => unpack_slot(item, false),
                    "meta" => unpack_slot(item, true),
                    "fields" if block => unpack(item, ""),
                    "fields" => unpack_slot(item, false),
                    _ => unpack(item, &key),
                };
                out.insert(key, item);
            }
            Value::Object(out)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn packs_schema_slots_only() {
        let state = json!({
            "id": "o",
            "fields": {"tags": {"mapValue": {"entries": {"fields": {"stringValue": "user key"}}}}},
            "blocks": [{"id": "b", "childrenIds": [], "fields": {"entries": {"k": {"intValue": 1}}},
                        "content": {"custom": {"meta": {"m": "v"}}}}],
        });
        let packed = pack_core_value_maps(&state);
        assert_eq!(
            packed,
            json!({
                "id": "o",
                "fields": [["tags", {"mapValue": {"entries": [["fields", {"stringValue": "user key"}]]}}]],
                "blocks": [{"id": "b", "childrenIds": [], "fields": {"entries": [["k", {"intValue": 1}]]},
                            "content": {"custom": {"meta": [["m", "v"]]}}}],
            })
        );
        assert_eq!(unpack_core_value_maps(packed), state);
    }

    /// User map keys that collide with schema slots (`entries`) or JS
    /// prototypes (`__proto__`) are opaque: packing a change's nested maps and
    /// unpacking them gives the change back (`parity-codec.ts`).
    #[test]
    fn map_transport_preserves_opaque_keys() {
        let change = json!({
            "id": "", "objectId": "maps", "parentIds": [], "timestamp": 1, "author": "a",
            "ops": [{"fieldSet": {"key": "map", "value": {"mapValue": {"entries": {
                "z": {"bytesValue": "AAH/"},
                "entries": {"stringValue": "opaque map key"},
                "__proto__": {"boolValue": false},
                "a": {"mapValue": {"entries": {"z": {"intValue": 1}, "a": {"intValue": 2}}}},
            }}}}}],
        });
        let packed = pack_core_value_maps(&change);
        let entries = &packed["ops"][0]["fieldSet"]["value"]["mapValue"]["entries"];
        assert!(entries.is_array(), "the map slot is packed: {entries}");
        assert!(
            entries
                .as_array()
                .unwrap()
                .iter()
                .any(|pair| pair == &json!(["entries", {"stringValue": "opaque map key"}])),
            "a user key named like a schema slot is not recursed into: {entries}"
        );
        assert_eq!(unpack_core_value_maps(packed), change);
    }
}

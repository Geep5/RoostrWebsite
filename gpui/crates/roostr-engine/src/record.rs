//! State records (glonOdin `docs/state-objects.md`): an object as its current
//! state, every property, block and chat message stamped with a hybrid clock
//! time and a device, merged entry by entry (newer stamp wins). The Rust twin
//! of `core/record.odin`: same canonical protobuf bytes, same hashes, pinned
//! by the shared vectors in `glonOdin/core/testdata/record_vectors/`.
//!
//! Pure library: nothing here touches the linked engine except
//! `Engine::record_writers` (`replay.rs`), which asks the engine's replay who last wrote
//! each entry so [`from_object_state`] can stamp a converted record.

use std::collections::BTreeMap;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value as Json, json};
use sha2::{Digest, Sha256};

// ── Stamps and the hybrid logical clock ──────────────────────────────

/// Hybrid logical clock time (`unix ms << 16 | counter`) plus the writing
/// device's index in the space's device table. Ordered by time, then device.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    pub time: u64,
    pub device: u32,
}

impl Stamp {
    /// Unix ms of the stamp's time.
    pub fn millis(self) -> u64 {
        self.time >> 16
    }
}

/// Stamps further ahead of the local wall clock than this are refused.
pub const MAX_FUTURE_MS: u64 = 24 * 60 * 60 * 1000;

/// A device's hybrid logical clock: the highest time it wrote or saw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hlc {
    pub last: u64,
}

/// A received stamp more than [`MAX_FUTURE_MS`] ahead of the wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FutureStamp {
    pub stamp: Stamp,
    pub wall_ms: u64,
}

impl fmt::Display for FutureStamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "stamp at {} ms is more than 24 h ahead of the clock ({} ms)",
            self.stamp.millis(),
            self.wall_ms
        )
    }
}

impl std::error::Error for FutureStamp {}

/// The next local time: max(wall clock, highest time seen + 1).
pub fn hlc_now(clock: &mut Hlc, wall_ms: u64) -> u64 {
    let time = (wall_ms << 16).max(clock.last + 1);
    clock.last = time;
    time
}

/// Advances the clock past a received stamp, unless it lies more than 24 h
/// ahead of `wall_ms` (refused, clock untouched).
pub fn hlc_observe(clock: &mut Hlc, stamp: Stamp, wall_ms: u64) -> Result<(), FutureStamp> {
    if stamp.millis() > wall_ms.saturating_add(MAX_FUTURE_MS) {
        return Err(FutureStamp { stamp, wall_ms });
    }
    clock.last = clock.last.max(stamp.time);
    Ok(())
}

// ── Values and block content (glon.proto `Value`, `BlockContent`) ────

/// A typed property value, as `glon.proto` `Value`. Map entries keep their
/// order: it is part of the encoding.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Value {
    #[default]
    None,
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    StringList(Vec<String>),
    Map(Vec<(String, Value)>),
    List(Vec<Value>),
    Link { target: String, relation: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mark {
    pub from: i64,
    pub to: i64,
    pub kind: i64,
    pub param: String,
}

/// A block's content, as `BlockContent` (including the table kinds 4-6).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Content {
    #[default]
    None,
    Text {
        text: String,
        style: i64,
        marks: Vec<Mark>,
        checked: bool,
        color: String,
    },
    Custom {
        content_type: String,
        data: Vec<u8>,
        meta: Vec<(String, String)>,
    },
    Layout(i64),
    Table,
    TableColumn,
    TableRow(bool),
}

// ── Protobuf writer (proto3: defaults omitted, as `proto.odin`) ──────

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        self.0.push(value as u8);
    }

    fn tag(&mut self, field: u64, wire: u64) {
        self.varint(field << 3 | wire);
    }

    fn bytes(&mut self, field: u64, data: &[u8]) {
        self.tag(field, 2);
        self.varint(data.len() as u64);
        self.0.extend_from_slice(data);
    }

    fn string(&mut self, field: u64, text: &str) {
        if !text.is_empty() {
            self.bytes(field, text.as_bytes());
        }
    }

    fn uint(&mut self, field: u64, value: u64) {
        if value != 0 {
            self.tag(field, 0);
            self.varint(value);
        }
    }

    fn int(&mut self, field: u64, value: i64) {
        self.uint(field, value as u64);
    }

    fn bool(&mut self, field: u64, value: bool) {
        if value {
            self.tag(field, 0);
            self.varint(1);
        }
    }

    fn message(&mut self, field: u64, build: impl FnOnce(&mut Writer)) {
        let mut sub = Writer::default();
        build(&mut sub);
        self.bytes(field, &sub.0);
    }
}

fn encode_value(value: &Value, w: &mut Writer) {
    match value {
        Value::None => {}
        // oneof members are always emitted, even when default.
        Value::String(text) => w.bytes(1, text.as_bytes()),
        Value::Int(number) => {
            w.tag(2, 0);
            w.varint(*number as u64);
        }
        Value::Float(number) => {
            w.tag(3, 1);
            w.0.extend_from_slice(&number.to_le_bytes());
        }
        Value::Bool(flag) => {
            w.tag(4, 0);
            w.varint(u64::from(*flag));
        }
        Value::Bytes(data) => w.bytes(5, data),
        Value::StringList(items) => w.message(6, |sub| {
            for item in items {
                sub.bytes(1, item.as_bytes());
            }
        }),
        Value::Map(entries) => w.message(7, |sub| encode_value_entries(entries, sub)),
        Value::List(items) => w.message(8, |sub| {
            for item in items {
                sub.message(1, |iw| encode_value(item, iw));
            }
        }),
        Value::Link { target, relation } => w.message(9, |sub| {
            sub.string(1, target);
            sub.string(2, relation);
        }),
    }
}

/// `map<string, Value>` entries (ValueMap field 1), in order.
fn encode_value_entries(entries: &[(String, Value)], w: &mut Writer) {
    for (key, value) in entries {
        w.message(1, |entry| {
            entry.bytes(1, key.as_bytes());
            entry.message(2, |vw| encode_value(value, vw));
        });
    }
}

fn encode_content(content: &Content, w: &mut Writer) {
    match content {
        Content::None => {}
        Content::Text {
            text,
            style,
            marks,
            checked,
            color,
        } => w.message(1, |sub| {
            sub.string(1, text);
            sub.int(2, *style);
            for mark in marks {
                sub.message(3, |mw| {
                    mw.int(1, mark.from);
                    mw.int(2, mark.to);
                    mw.int(3, mark.kind);
                    mw.string(4, &mark.param);
                });
            }
            sub.bool(4, *checked);
            sub.string(5, color);
        }),
        Content::Custom {
            content_type,
            data,
            meta,
        } => w.message(2, |sub| {
            sub.string(1, content_type);
            if !data.is_empty() {
                sub.bytes(2, data);
            }
            for (key, value) in meta {
                sub.message(3, |pw| {
                    pw.bytes(1, key.as_bytes());
                    pw.bytes(2, value.as_bytes());
                });
            }
        }),
        Content::Layout(style) => w.message(3, |sub| sub.int(1, *style)),
        Content::Table => w.bytes(4, &[]),
        Content::TableColumn => w.bytes(5, &[]),
        Content::TableRow(header) => w.message(6, |sub| sub.bool(1, *header)),
    }
}

// ── JSON forms (`jsonx.odin` ValueJSON / BlockContent JSON) ──────────

fn json_i64(value: Option<&Json>) -> i64 {
    match value {
        Some(Json::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        _ => 0,
    }
}

fn json_str<'a>(value: &'a Json, key: &str) -> &'a str {
    value.get(key).and_then(Json::as_str).unwrap_or("")
}

fn json_bool(value: &Json, key: &str) -> bool {
    value.get(key).and_then(Json::as_bool).unwrap_or(false)
}

/// Map pairs from a plain object or the engine's ordered `[key, value]` pairs.
fn json_pairs(value: Option<&Json>) -> Vec<(&str, &Json)> {
    match value {
        Some(Json::Object(map)) => map.iter().map(|(k, v)| (k.as_str(), v)).collect(),
        Some(Json::Array(items)) => items
            .iter()
            .filter_map(|pair| match pair.as_array().map(Vec::as_slice) {
                Some([Json::String(key), value]) => Some((key.as_str(), value)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn base64_decode(text: &str) -> Vec<u8> {
    STANDARD.decode(text).unwrap_or_default()
}

impl Value {
    /// ValueJSON → Value (`value_from_json`).
    pub fn from_json(json: &Json) -> Value {
        let Some(object) = json.as_object() else {
            return Value::None;
        };
        if let Some(text) = object.get("stringValue") {
            return Value::String(text.as_str().unwrap_or("").to_owned());
        }
        if let Some(number) = object.get("intValue") {
            return Value::Int(json_i64(Some(number)));
        }
        if let Some(number) = object.get("floatValue") {
            return Value::Float(number.as_f64().unwrap_or(0.0));
        }
        if let Some(flag) = object.get("boolValue") {
            return Value::Bool(flag.as_bool().unwrap_or(false));
        }
        if let Some(text) = object.get("bytesValue") {
            return Value::Bytes(base64_decode(text.as_str().unwrap_or("")));
        }
        if let Some(list) = object.get("listValue") {
            let values = list
                .get("values")
                .and_then(Json::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            return Value::StringList(values);
        }
        if let Some(map) = object.get("mapValue") {
            return Value::Map(fields_from_json(map.get("entries")));
        }
        if let Some(list) = object.get("valuesValue") {
            let items = list
                .get("items")
                .and_then(Json::as_array)
                .map(|items| items.iter().map(Value::from_json).collect())
                .unwrap_or_default();
            return Value::List(items);
        }
        if let Some(link) = object.get("linkValue") {
            return Value::Link {
                target: json_str(link, "targetId").to_owned(),
                relation: json_str(link, "relationKey").to_owned(),
            };
        }
        Value::None
    }

    /// Value → ValueJSON (`value_to_json`).
    pub fn to_json(&self) -> Json {
        match self {
            Value::None => json!({}),
            Value::String(text) => json!({"stringValue": text}),
            Value::Int(number) => json!({"intValue": number}),
            Value::Float(number) => {
                let number = if number.is_finite() { *number } else { 0.0 };
                json!({"floatValue": number})
            }
            Value::Bool(flag) => json!({"boolValue": flag}),
            Value::Bytes(data) => json!({"bytesValue": STANDARD.encode(data)}),
            Value::StringList(values) => json!({"listValue": {"values": values}}),
            Value::Map(entries) => json!({"mapValue": {"entries": fields_to_json(entries)}}),
            Value::List(items) => {
                json!({"valuesValue": {"items": items.iter().map(Value::to_json).collect::<Vec<_>>()}})
            }
            Value::Link { target, relation } => {
                json!({"linkValue": {"targetId": target, "relationKey": relation}})
            }
        }
    }
}

fn fields_from_json(json: Option<&Json>) -> Vec<(String, Value)> {
    json_pairs(json)
        .into_iter()
        .map(|(key, value)| (key.to_owned(), Value::from_json(value)))
        .collect()
}

fn fields_to_json(entries: &[(String, Value)]) -> Json {
    Json::Object(
        entries
            .iter()
            .map(|(key, value)| (key.clone(), value.to_json()))
            .collect(),
    )
}

/// Custom kinds whose `data` is shown on the state wire (`SELF_DESCRIBING_BLOCKS`).
const SELF_DESCRIBING_BLOCKS: [&str; 2] = ["discussion", "agent_message"];

impl Content {
    /// BlockContent JSON → Content (`content_from_json`).
    pub fn from_json(json: &Json) -> Content {
        if let Some(text) = json.get("text") {
            let marks = text
                .get("marks")
                .and_then(Json::as_array)
                .map(|marks| {
                    marks
                        .iter()
                        .map(|mark| Mark {
                            from: json_i64(mark.get("from")),
                            to: json_i64(mark.get("to")),
                            kind: json_i64(mark.get("type")),
                            param: json_str(mark, "param").to_owned(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            return Content::Text {
                text: json_str(text, "text").to_owned(),
                style: json_i64(text.get("style")),
                marks,
                checked: json_bool(text, "checked"),
                color: json_str(text, "color").to_owned(),
            };
        }
        if let Some(layout) = json.get("layout") {
            return Content::Layout(json_i64(layout.get("style")));
        }
        if json.get("table").is_some() {
            return Content::Table;
        }
        if json.get("tableColumn").is_some() {
            return Content::TableColumn;
        }
        if let Some(row) = json.get("tableRow") {
            return Content::TableRow(json_bool(row, "isHeader"));
        }
        if let Some(custom) = json.get("custom") {
            return Content::Custom {
                content_type: json_str(custom, "contentType").to_owned(),
                data: base64_decode(json_str(custom, "data")),
                meta: json_pairs(custom.get("meta"))
                    .into_iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|text| (key.to_owned(), text.to_owned()))
                    })
                    .collect(),
            };
        }
        Content::None
    }

    /// Content → BlockContent JSON (`block_to_json`, state form). `wire`
    /// keeps every custom payload (record JSON); the state view hides
    /// opaque ones as replay does.
    pub fn to_json(&self, wire: bool) -> Json {
        match self {
            Content::None => json!({}),
            Content::Text {
                text,
                style,
                marks,
                checked,
                color,
            } => {
                let marks: Vec<Json> = marks
                    .iter()
                    .map(|mark| {
                        let mut object = json!({"from": mark.from, "to": mark.to, "type": mark.kind});
                        if !mark.param.is_empty() {
                            object["param"] = json!(mark.param);
                        }
                        object
                    })
                    .collect();
                json!({"text": {"text": text, "style": style, "marks": marks, "checked": checked, "color": color}})
            }
            Content::Custom {
                content_type,
                data,
                meta,
            } => {
                let mut custom = Map::new();
                custom.insert("contentType".into(), json!(content_type));
                if wire
                    || (SELF_DESCRIBING_BLOCKS.contains(&content_type.as_str()) && !data.is_empty())
                {
                    custom.insert("data".into(), json!(STANDARD.encode(data)));
                }
                custom.insert(
                    "meta".into(),
                    Json::Object(
                        meta.iter()
                            .map(|(key, value)| (key.clone(), json!(value)))
                            .collect(),
                    ),
                );
                json!({"custom": custom})
            }
            Content::Layout(style) => json!({"layout": {"style": style}}),
            Content::Table => json!({"table": {}}),
            Content::TableColumn => json!({"tableColumn": {}}),
            Content::TableRow(header) => json!({"tableRow": {"isHeader": header}}),
        }
    }
}

// ── Entries, records and patches ─────────────────────────────────────

/// A block's or chat message's placement and payload (`BlockBody` /
/// `MessageBody`, the same message).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Body {
    /// Parent block id; "" = top level.
    pub parent: String,
    /// Fractional key ([`key_between`]) ordering siblings.
    pub position: String,
    pub content: Content,
    pub fields: Vec<(String, Value)>,
    pub align: i64,
    pub background_color: String,
}

/// What an entry carries; tombstones keep a body only to name their kind.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum EntryBody {
    #[default]
    None,
    Value(Value),
    Block(Body),
    Message(Body),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub key: String,
    pub stamp: Stamp,
    /// Tombstone: the entry was removed at `stamp`.
    pub gone: bool,
    pub body: EntryBody,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record {
    pub object_id: String,
    /// Key "type": the type key as a string value.
    pub kind: Option<Entry>,
    /// Key "deleted": the object tombstone as a bool value.
    pub deleted: Option<Entry>,
    pub fields: Vec<Entry>,
    pub blocks: Vec<Entry>,
    pub messages: Vec<Entry>,
    /// Unix ms; informational (min of non-zero on merge).
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Patch {
    pub object_id: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    ObjectMismatch { left: String, right: String },
    /// A patch entry outside "type"/"deleted" without a body.
    NoBody(String),
    Json(String),
    Conversion(String),
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordError::ObjectMismatch { left, right } => {
                write!(f, "records describe different objects: {left} vs {right}")
            }
            RecordError::NoBody(key) => write!(f, "patch entry {key} has no body"),
            RecordError::Json(message) => write!(f, "invalid record JSON: {message}"),
            RecordError::Conversion(message) => write!(f, "record conversion: {message}"),
        }
    }
}

impl std::error::Error for RecordError {}

// ── Canonical encoding ───────────────────────────────────────────────

/// Map entries sorted by key bytewise, duplicate keys last-wins.
fn canonical_pairs<V>(pairs: &[(String, V)]) -> Vec<(&str, &V)> {
    let mut sorted: BTreeMap<&str, &V> = BTreeMap::new();
    for (key, value) in pairs {
        sorted.insert(key, value);
    }
    sorted.into_iter().collect()
}

fn canonical_value(value: &Value, w: &mut Writer) {
    match value {
        Value::Map(entries) => w.message(7, |sub| canonical_value_entries(entries, sub)),
        Value::List(items) => w.message(8, |sub| {
            for item in items {
                sub.message(1, |iw| canonical_value(item, iw));
            }
        }),
        other => encode_value(other, w),
    }
}

fn canonical_value_entries(entries: &[(String, Value)], w: &mut Writer) {
    for (key, value) in canonical_pairs(entries) {
        w.message(1, |entry| {
            entry.bytes(1, key.as_bytes());
            entry.message(2, |vw| canonical_value(value, vw));
        });
    }
}

fn canonical_content(content: &Content, w: &mut Writer) {
    match content {
        Content::Custom {
            content_type,
            data,
            meta,
        } => {
            let meta: Vec<(String, String)> = canonical_pairs(meta)
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value.clone()))
                .collect();
            encode_content(
                &Content::Custom {
                    content_type: content_type.clone(),
                    data: data.clone(),
                    meta,
                },
                w,
            );
        }
        other => encode_content(other, w),
    }
}

fn canonical_body(body: &Body, w: &mut Writer) {
    w.string(1, &body.parent);
    w.string(2, &body.position);
    if body.content != Content::None {
        w.message(3, |cw| canonical_content(&body.content, cw));
    }
    if !body.fields.is_empty() {
        w.message(4, |fw| canonical_value_entries(&body.fields, fw));
    }
    w.int(5, body.align);
    w.string(6, &body.background_color);
}

fn canonical_entry(entry: &Entry, w: &mut Writer) {
    w.string(1, &entry.key);
    w.message(2, |sw| {
        sw.uint(1, entry.stamp.time);
        sw.uint(2, u64::from(entry.stamp.device));
    });
    w.bool(3, entry.gone);
    match (&entry.body, entry.gone) {
        (EntryBody::None, _) => {}
        (EntryBody::Value(_), true) => w.bytes(4, &[]),
        (EntryBody::Block(_), true) => w.bytes(5, &[]),
        (EntryBody::Message(_), true) => w.bytes(6, &[]),
        (EntryBody::Value(value), false) => w.message(4, |vw| canonical_value(value, vw)),
        (EntryBody::Block(body), false) => w.message(5, |bw| canonical_body(body, bw)),
        (EntryBody::Message(body), false) => w.message(6, |bw| canonical_body(body, bw)),
    }
}

/// The canonical `Entry` protobuf (also the last merge tie-break).
pub fn entry_canonical(entry: &Entry) -> Vec<u8> {
    let mut w = Writer::default();
    canonical_entry(entry, &mut w);
    w.0
}

fn sorted_entries(entries: &[Entry]) -> Vec<&Entry> {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
    sorted
}

/// The canonical `Record` protobuf: entries sorted by key in each list,
/// maps sorted by key, tombstone bodies emptied, defaults omitted.
pub fn record_canonical(record: &Record) -> Vec<u8> {
    let mut w = Writer::default();
    w.string(1, &record.object_id);
    if let Some(entry) = &record.kind {
        w.message(2, |ew| canonical_entry(entry, ew));
    }
    if let Some(entry) = &record.deleted {
        w.message(3, |ew| canonical_entry(entry, ew));
    }
    for (field, list) in [(4, &record.fields), (5, &record.blocks), (6, &record.messages)] {
        for entry in sorted_entries(list) {
            w.message(field, |ew| canonical_entry(entry, ew));
        }
    }
    w.int(7, record.created_at);
    w.0
}

/// The canonical `Patch` protobuf (entries sorted by key).
pub fn patch_canonical(patch: &Patch) -> Vec<u8> {
    let mut w = Writer::default();
    w.string(1, &patch.object_id);
    for entry in sorted_entries(&patch.entries) {
        w.message(2, |ew| canonical_entry(entry, ew));
    }
    w.0
}

fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// sha256 of the canonical encoding.
pub fn record_hash(record: &Record) -> [u8; 32] {
    sha256(&record_canonical(record))
}

/// Pack bucket of an object: the first byte of sha256(object_id).
pub fn record_bucket(object_id: &str) -> u8 {
    sha256(object_id.as_bytes())[0]
}

/// sha256 of the records' hashes concatenated in object id order
/// (sha256 of nothing for an empty bucket).
pub fn bucket_hash(records: &[&Record]) -> [u8; 32] {
    let mut sorted: Vec<&&Record> = records.iter().collect();
    sorted.sort_by(|a, b| a.object_id.as_bytes().cmp(b.object_id.as_bytes()));
    let mut hasher = Sha256::new();
    for record in sorted {
        hasher.update(record_hash(record));
    }
    hasher.finalize().into()
}

/// The 256 bucket hashes of a set of records.
pub fn bucket_hashes(records: &[Record]) -> Vec<[u8; 32]> {
    let mut buckets: Vec<Vec<&Record>> = vec![Vec::new(); 256];
    for record in records {
        buckets[usize::from(record_bucket(&record.object_id))].push(record);
    }
    buckets.iter().map(|bucket| bucket_hash(bucket)).collect()
}

/// sha256 of the 256 bucket hashes concatenated.
pub fn root_hash(buckets: &[[u8; 32]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for bucket in buckets {
        hasher.update(bucket);
    }
    hasher.finalize().into()
}

// ── Merge ────────────────────────────────────────────────────────────

/// The winner of two entries for one key: larger (time, device), then
/// larger canonical bytes.
fn newer<'a>(a: &'a Entry, b: &'a Entry) -> &'a Entry {
    match a.stamp.cmp(&b.stamp) {
        std::cmp::Ordering::Greater => a,
        std::cmp::Ordering::Less => b,
        std::cmp::Ordering::Equal => {
            if entry_canonical(a) >= entry_canonical(b) {
                a
            } else {
                b
            }
        }
    }
}

fn merge_slot(slot: &mut Option<Entry>, incoming: &Entry) {
    match slot {
        Some(current) => {
            if !std::ptr::eq(newer(current, incoming), current) {
                *current = incoming.clone();
            }
        }
        None => *slot = Some(incoming.clone()),
    }
}

/// Merges `incoming` into a key-sorted list.
fn merge_into(list: &mut Vec<Entry>, incoming: &Entry) {
    match list.binary_search_by(|entry| entry.key.as_bytes().cmp(incoming.key.as_bytes())) {
        Ok(index) => {
            if !std::ptr::eq(newer(&list[index], incoming), &list[index]) {
                list[index] = incoming.clone();
            }
        }
        Err(index) => list.insert(index, incoming.clone()),
    }
}

/// A list's entries, one per key (merged), sorted by key.
fn normalized(entries: &[Entry]) -> Vec<Entry> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        merge_into(&mut out, entry);
    }
    out
}

fn merge_created(a: i64, b: i64) -> i64 {
    match (a, b) {
        (0, other) | (other, 0) => other,
        (a, b) => a.min(b),
    }
}

fn adopt_id(a: &str, b: &str) -> Result<String, RecordError> {
    if a.is_empty() || a == b {
        Ok(b.to_owned())
    } else if b.is_empty() {
        Ok(a.to_owned())
    } else {
        Err(RecordError::ObjectMismatch {
            left: a.to_owned(),
            right: b.to_owned(),
        })
    }
}

/// Entry-by-entry merge: per key the newer stamp wins. Commutative,
/// associative, idempotent; lists come out key-sorted.
pub fn record_merge(a: &Record, b: &Record) -> Result<Record, RecordError> {
    let mut out = Record {
        object_id: adopt_id(&a.object_id, &b.object_id)?,
        kind: a.kind.clone(),
        deleted: a.deleted.clone(),
        fields: normalized(&a.fields),
        blocks: normalized(&a.blocks),
        messages: normalized(&a.messages),
        created_at: merge_created(a.created_at, b.created_at),
    };
    if let Some(entry) = &b.kind {
        merge_slot(&mut out.kind, entry);
    }
    if let Some(entry) = &b.deleted {
        merge_slot(&mut out.deleted, entry);
    }
    for entry in &b.fields {
        merge_into(&mut out.fields, entry);
    }
    for entry in &b.blocks {
        merge_into(&mut out.blocks, entry);
    }
    for entry in &b.messages {
        merge_into(&mut out.messages, entry);
    }
    Ok(out)
}

/// Applies a patch: "type"/"deleted" go to their slots, other entries
/// route by body kind.
pub fn record_apply_patch(record: &Record, patch: &Patch) -> Result<Record, RecordError> {
    let mut out = record_merge(record, &Record::default())?;
    out.object_id = adopt_id(&out.object_id, &patch.object_id)?;
    for entry in &patch.entries {
        match (entry.key.as_str(), &entry.body) {
            ("type", _) => merge_slot(&mut out.kind, entry),
            ("deleted", _) => merge_slot(&mut out.deleted, entry),
            (_, EntryBody::Value(_)) => merge_into(&mut out.fields, entry),
            (_, EntryBody::Block(_)) => merge_into(&mut out.blocks, entry),
            (_, EntryBody::Message(_)) => merge_into(&mut out.messages, entry),
            (key, EntryBody::None) => return Err(RecordError::NoBody(key.to_owned())),
        }
    }
    Ok(out)
}

// ── Record JSON (the shared vectors' form) ───────────────────────────

fn json_error(message: impl Into<String>) -> RecordError {
    RecordError::Json(message.into())
}

impl Body {
    fn from_json(json: &Json) -> Body {
        Body {
            parent: json_str(json, "parent").to_owned(),
            position: json_str(json, "position").to_owned(),
            content: json.get("content").map(Content::from_json).unwrap_or_default(),
            fields: fields_from_json(json.get("fields")),
            align: json_i64(json.get("align")),
            background_color: json_str(json, "backgroundColor").to_owned(),
        }
    }

    fn to_json(&self) -> Json {
        let mut object = Map::new();
        object.insert("parent".into(), json!(self.parent));
        object.insert("position".into(), json!(self.position));
        if self.content != Content::None {
            object.insert("content".into(), self.content.to_json(true));
        }
        if !self.fields.is_empty() {
            object.insert("fields".into(), fields_to_json(&self.fields));
        }
        if self.align != 0 {
            object.insert("align".into(), json!(self.align));
        }
        if !self.background_color.is_empty() {
            object.insert("backgroundColor".into(), json!(self.background_color));
        }
        Json::Object(object)
    }
}

impl Entry {
    pub fn from_json(json: &Json) -> Result<Entry, RecordError> {
        let stamp = json.get("stamp");
        let time = match stamp.and_then(|stamp| stamp.get("time")) {
            None => 0,
            Some(Json::Number(number)) => number
                .as_u64()
                .ok_or_else(|| json_error("stamp time must be a non-negative integer"))?,
            Some(Json::String(text)) => text
                .parse()
                .map_err(|_| json_error("stamp time must be an integer"))?,
            Some(_) => return Err(json_error("stamp time must be an integer")),
        };
        let device = stamp
            .and_then(|stamp| stamp.get("device"))
            .map(|device| {
                device
                    .as_u64()
                    .and_then(|device| u32::try_from(device).ok())
                    .ok_or_else(|| json_error("stamp device must be a u32"))
            })
            .transpose()?
            .unwrap_or(0);
        // A tombstone keeps only its kind.
        let gone = json_bool(json, "gone");
        let body = if let Some(value) = json.get("value") {
            EntryBody::Value(if gone { Value::None } else { Value::from_json(value) })
        } else if let Some(block) = json.get("block") {
            EntryBody::Block(if gone { Body::default() } else { Body::from_json(block) })
        } else if let Some(message) = json.get("message") {
            EntryBody::Message(if gone { Body::default() } else { Body::from_json(message) })
        } else {
            EntryBody::None
        };
        Ok(Entry {
            key: json_str(json, "key").to_owned(),
            stamp: Stamp { time, device },
            gone,
            body,
        })
    }

    pub fn to_json(&self) -> Json {
        let mut object = Map::new();
        object.insert("key".into(), json!(self.key));
        object.insert(
            "stamp".into(),
            json!({"time": self.stamp.time, "device": self.stamp.device}),
        );
        if self.gone {
            object.insert("gone".into(), json!(true));
        }
        let (name, body) = match &self.body {
            EntryBody::None => return Json::Object(object),
            EntryBody::Value(value) => ("value", value.to_json()),
            EntryBody::Block(body) => ("block", body.to_json()),
            EntryBody::Message(body) => ("message", body.to_json()),
        };
        object.insert(name.into(), if self.gone { json!({}) } else { body });
        Json::Object(object)
    }
}

fn entries_from_json(json: Option<&Json>) -> Result<Vec<Entry>, RecordError> {
    match json {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Array(items)) => items.iter().map(Entry::from_json).collect(),
        Some(_) => Err(json_error("entry lists must be arrays")),
    }
}

/// The "type" / "deleted" slot: a value entry under its slot key.
fn slot_from_json(json: Option<&Json>, key: &str) -> Result<Option<Entry>, RecordError> {
    match json {
        None | Some(Json::Null) => Ok(None),
        Some(entry) => {
            let mut entry = Entry::from_json(entry)?;
            if !matches!(entry.body, EntryBody::Value(_)) {
                return Err(json_error(format!("{key} must be a value entry")));
            }
            entry.key = key.to_owned();
            Ok(Some(entry))
        }
    }
}

impl Record {
    pub fn from_json(json: &Json) -> Result<Record, RecordError> {
        if !json.is_object() {
            return Err(json_error("a record must be an object"));
        }
        Ok(Record {
            object_id: json_str(json, "objectId").to_owned(),
            kind: slot_from_json(json.get("type"), "type")?,
            deleted: slot_from_json(json.get("deleted"), "deleted")?,
            fields: entries_from_json(json.get("fields"))?,
            blocks: entries_from_json(json.get("blocks"))?,
            messages: entries_from_json(json.get("messages"))?,
            created_at: json_i64(json.get("createdAt")),
        })
    }

    pub fn to_json(&self) -> Json {
        let list = |entries: &[Entry]| -> Json {
            Json::Array(sorted_entries(entries).into_iter().map(Entry::to_json).collect())
        };
        let mut object = Map::new();
        object.insert("objectId".into(), json!(self.object_id));
        if let Some(entry) = &self.kind {
            object.insert("type".into(), entry.to_json());
        }
        if let Some(entry) = &self.deleted {
            object.insert("deleted".into(), entry.to_json());
        }
        object.insert("fields".into(), list(&self.fields));
        object.insert("blocks".into(), list(&self.blocks));
        object.insert("messages".into(), list(&self.messages));
        object.insert("createdAt".into(), json!(self.created_at));
        Json::Object(object)
    }
}

impl Patch {
    pub fn from_json(json: &Json) -> Result<Patch, RecordError> {
        if !json.is_object() {
            return Err(json_error("a patch must be an object"));
        }
        Ok(Patch {
            object_id: json_str(json, "objectId").to_owned(),
            entries: entries_from_json(json.get("entries"))?,
        })
    }

    pub fn to_json(&self) -> Json {
        json!({
            "objectId": self.object_id,
            "entries": self.entries.iter().map(Entry::to_json).collect::<Vec<_>>(),
        })
    }
}

// ── Fractional keys (`pins.odin pin_key_between`) ────────────────────

const KEY_DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn key_digit(c: u8) -> usize {
    KEY_DIGITS.iter().position(|&d| d == c).unwrap_or(0)
}

/// A key sorting strictly between `a` and `b` ("" / `None` = open ends).
pub fn key_between(a: &str, b: Option<&str>) -> String {
    let a = a.as_bytes();
    let mut out = Vec::new();
    key_between_into(a, b.map(str::as_bytes), &mut out);
    String::from_utf8(out).expect("key digits are ASCII")
}

fn key_between_into(a: &[u8], b: Option<&[u8]>, out: &mut Vec<u8>) {
    if let Some(b) = b {
        let mut n = 0;
        while n < b.len() && a.get(n).copied().unwrap_or(b'0') == b[n] {
            n += 1;
        }
        if n > 0 {
            out.extend_from_slice(&b[..n]);
            return key_between_into(a.get(n..).unwrap_or(&[]), Some(&b[n..]), out);
        }
    }
    let lo = a.first().map_or(0, |&c| key_digit(c));
    let hi = match b {
        Some(b) if !b.is_empty() => key_digit(b[0]),
        _ => KEY_DIGITS.len(),
    };
    if hi > lo + 1 {
        out.push(KEY_DIGITS[(lo + hi).div_ceil(2)]);
        return;
    }
    if let Some(b) = b
        && b.len() > 1
    {
        out.push(b[0]);
        return;
    }
    out.push(KEY_DIGITS[lo]);
    key_between_into(a.get(1..).unwrap_or(&[]), None, out);
}

/// Sibling positions for `n` blocks in order: bisection with [`key_between`].
pub fn sibling_positions(n: usize) -> Vec<String> {
    fn assign(lo: isize, hi: isize, a: &str, b: Option<&str>, out: &mut [String]) {
        if lo > hi {
            return;
        }
        let mid = (lo + hi) / 2;
        let key = key_between(a, b);
        assign(lo, mid - 1, a, Some(&key), out);
        assign(mid + 1, hi, &key, b, out);
        out[mid as usize] = key;
    }
    let mut out = vec![String::new(); n];
    assign(0, n as isize - 1, "", None, &mut out);
    out
}

// ── View: record → ObjectJSON ────────────────────────────────────────

fn live_value(entry: Option<&Entry>) -> Option<&Value> {
    match entry {
        Some(Entry {
            gone: false,
            body: EntryBody::Value(value),
            ..
        }) => Some(value),
        _ => None,
    }
}

/// One block as `block_to_json`: `wire` (lossless state) emits content only
/// when present and keeps custom data; the API form always has `content`.
fn block_json(id: &str, body: &Body, children: &[&str], wire: bool) -> Json {
    let mut object = Map::new();
    object.insert("id".into(), json!(id));
    object.insert("childrenIds".into(), json!(children));
    if !wire || body.content != Content::None {
        object.insert("content".into(), body.content.to_json(wire));
    }
    if !body.fields.is_empty() {
        object.insert(
            "fields".into(),
            json!({"entries": fields_to_json(&body.fields)}),
        );
    }
    if body.align != 0 {
        object.insert("align".into(), json!(body.align));
    }
    if !body.background_color.is_empty() {
        object.insert("backgroundColor".into(), json!(body.background_color));
    }
    Json::Object(object)
}

/// The object's ObjectJSON as replay serves it (`record_view` +
/// `object_to_json_value`): live fields, the block tree (blocks and messages
/// together, siblings by (position, id), preorder from the roots,
/// unreachable entries hidden, `__content__` internal), timestamps from the
/// stamps. The engine's derived `conversations` / `mailbox` projections are
/// not produced here.
pub fn record_view(record: &Record) -> Json {
    view_json(record, false)
}

/// The lossless state (`record_view` + `record_state_to_json`): blocks in
/// wire form, `__content__` included; what conversion consumes.
pub fn record_state(record: &Record) -> Json {
    view_json(record, true)
}

fn view_json(record: &Record, wire: bool) -> Json {
    let mut nodes: Vec<(&str, &Body)> = record
        .blocks
        .iter()
        .chain(&record.messages)
        .filter(|entry| !entry.gone)
        .filter_map(|entry| match &entry.body {
            EntryBody::Block(body) | EntryBody::Message(body) => Some((entry.key.as_str(), body)),
            _ => None,
        })
        .collect();
    nodes.sort_by(|a, b| {
        (a.1.parent.as_bytes(), a.1.position.as_bytes(), a.0.as_bytes())
            .cmp(&(b.1.parent.as_bytes(), b.1.position.as_bytes(), b.0.as_bytes()))
    });
    // A key in both lists (never written that way) shows once: the first in sort order.
    let mut by_key: BTreeMap<&str, &Body> = BTreeMap::new();
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (key, body) in nodes {
        if by_key.contains_key(key) {
            continue;
        }
        by_key.insert(key, body);
        children.entry(body.parent.as_str()).or_default().push(key);
    }
    let mut blocks = Vec::new();
    let mut emitted = std::collections::HashSet::new();
    let mut stack: Vec<&str> = children.get("").cloned().unwrap_or_default();
    stack.reverse();
    while let Some(id) = stack.pop() {
        if id.is_empty() || !emitted.insert(id) {
            continue;
        }
        let kids: &[&str] = children.get(id).map(Vec::as_slice).unwrap_or(&[]);
        if wire || id != "__content__" {
            blocks.push(block_json(id, by_key[id], kids, wire));
        }
        stack.extend(kids.iter().rev());
    }

    let stamps = record
        .kind
        .iter()
        .chain(&record.deleted)
        .chain(&record.fields)
        .chain(&record.blocks)
        .chain(&record.messages)
        .map(|entry| entry.stamp.millis() as i64);
    let (mut oldest, mut newest) = (None::<i64>, 0i64);
    for millis in stamps {
        oldest = Some(oldest.map_or(millis, |old| old.min(millis)));
        newest = newest.max(millis);
    }
    let created_at = if record.created_at != 0 {
        record.created_at
    } else {
        oldest.unwrap_or(0)
    };
    let type_key = match live_value(record.kind.as_ref()) {
        Some(Value::String(text)) => text.as_str(),
        _ => "",
    };
    let deleted = matches!(live_value(record.deleted.as_ref()), Some(Value::Bool(true)));
    let fields: Map<String, Json> = record
        .fields
        .iter()
        .filter_map(|entry| live_value(Some(entry)).map(|value| (entry.key.clone(), value.to_json())))
        .collect();
    json!({
        "id": record.object_id,
        "typeKey": type_key,
        "fields": fields,
        "blocks": blocks,
        "deleted": deleted,
        "createdAt": created_at,
        "updatedAt": newest,
    })
}

// ── Conversion from replayed DAG state ───────────────────────────────

/// The change that last wrote an entry during replay (`record` `writers`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct LastWriter {
    /// The change's timestamp, unix ms.
    pub time: i64,
    pub author: String,
    /// Written by the replay's seed (checkpoint / legacy snapshot): no
    /// author, device 0.
    #[serde(default)]
    pub seed: bool,
    /// The block was last seen as a chat message.
    #[serde(default)]
    pub message: bool,
}

/// Per entry key, the last change that wrote it, from one replay of the
/// object's DAG (the engine's replay hook).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Writers {
    /// The seed's writer (its updated_at): entries no replayed change wrote.
    #[serde(skip)]
    pub seed: Option<LastWriter>,
    #[serde(default, rename = "type")]
    pub kind: Option<LastWriter>,
    #[serde(default)]
    pub deleted: Option<LastWriter>,
    #[serde(default)]
    pub fields: BTreeMap<String, LastWriter>,
    #[serde(default)]
    pub blocks: BTreeMap<String, LastWriter>,
}

/// A replayed state plus who last wrote each entry (`record` `writers`).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct ReplayWriters {
    /// Lossless state (`record_state_to_json`).
    pub state: Json,
    pub writers: Writers,
    #[serde(default)]
    pub seed: Option<LastWriter>,
}

fn is_chat(content: &Content) -> bool {
    matches!(content, Content::Custom { content_type, .. } if content_type == "chat")
}

/// Stamps a replayed ObjectJSON into a record: every entry's stamp is the
/// time of the change that last wrote it and its author's device index
/// (`device_of`; an unknown author is an error). Keys a change wrote that
/// the state no longer holds become tombstones.
pub fn from_object_state(
    state: &Json,
    writers: &Writers,
    device_of: impl Fn(&str) -> Option<u32>,
) -> Result<Record, RecordError> {
    let stamp_of = |key: &str, writer: Option<&LastWriter>| -> Result<Stamp, RecordError> {
        let writer = writer
            .or(writers.seed.as_ref())
            .ok_or_else(|| RecordError::Conversion(format!("no writer for {key}")))?;
        if writer.seed {
            return Ok(Stamp {
                time: (writer.time.max(0) as u64) << 16,
                device: 0,
            });
        }
        let device = device_of(&writer.author).ok_or_else(|| {
            RecordError::Conversion(format!("no device index for author {:?}", writer.author))
        })?;
        Ok(Stamp {
            time: (writer.time.max(0) as u64) << 16,
            device,
        })
    };
    let object_id = json_str(state, "id").to_owned();
    let mut record = Record {
        object_id,
        created_at: json_i64(state.get("createdAt")),
        ..Record::default()
    };

    let type_key = json_str(state, "typeKey");
    if !type_key.is_empty() || writers.kind.is_some() {
        record.kind = Some(Entry {
            key: "type".into(),
            stamp: stamp_of("type", writers.kind.as_ref())?,
            gone: false,
            body: EntryBody::Value(Value::String(type_key.to_owned())),
        });
    }
    let deleted = json_bool(state, "deleted");
    if deleted || writers.deleted.is_some() {
        record.deleted = Some(Entry {
            key: "deleted".into(),
            stamp: stamp_of("deleted", writers.deleted.as_ref())?,
            gone: false,
            body: EntryBody::Value(Value::Bool(deleted)),
        });
    }

    let fields = fields_from_json(state.get("fields"));
    for (key, value) in &fields {
        record.fields.push(Entry {
            key: key.clone(),
            stamp: stamp_of(key, writers.fields.get(key))?,
            gone: false,
            body: EntryBody::Value(value.clone()),
        });
    }
    for (key, writer) in &writers.fields {
        if !fields.iter().any(|(held, _)| held == key) {
            record.fields.push(Entry {
                key: key.clone(),
                stamp: stamp_of(key, Some(writer))?,
                gone: true,
                body: EntryBody::Value(Value::None),
            });
        }
    }

    // The tree: a block not yet placed is a root; its listed children not
    // yet placed are its children, in list order (first parent wins).
    let blocks: &[Json] = state
        .get("blocks")
        .and_then(Json::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let exists: std::collections::HashSet<&str> =
        blocks.iter().map(|block| json_str(block, "id")).collect();
    let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
    let mut siblings: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for block in blocks {
        let id = json_str(block, "id");
        if !parent.contains_key(id) {
            parent.insert(id, "");
            siblings.entry("").or_default().push(id);
        }
        let kids = block.get("childrenIds").and_then(Json::as_array);
        for kid in kids.into_iter().flatten().filter_map(Json::as_str) {
            if exists.contains(kid) && !parent.contains_key(kid) {
                parent.insert(kid, id);
                siblings.entry(id).or_default().push(kid);
            }
        }
    }
    let mut position: BTreeMap<&str, String> = BTreeMap::new();
    for list in siblings.values() {
        for (&id, key) in list.iter().zip(sibling_positions(list.len())) {
            position.insert(id, key);
        }
    }
    for block in blocks {
        let id = json_str(block, "id");
        // First occurrence only.
        let Some(position) = position.remove(id) else {
            continue;
        };
        let body = Body {
            parent: parent[id].to_owned(),
            position,
            content: block.get("content").map(Content::from_json).unwrap_or_default(),
            fields: fields_from_json(block.get("fields").and_then(|fields| fields.get("entries"))),
            align: json_i64(block.get("align")),
            background_color: json_str(block, "backgroundColor").to_owned(),
        };
        let stamp = stamp_of(id, writers.blocks.get(id))?;
        let chat = is_chat(&body.content);
        let entry = Entry {
            key: id.to_owned(),
            stamp,
            gone: false,
            body: if chat {
                EntryBody::Message(body)
            } else {
                EntryBody::Block(body)
            },
        };
        if chat {
            record.messages.push(entry);
        } else {
            record.blocks.push(entry);
        }
    }
    for (key, writer) in &writers.blocks {
        if exists.contains(key.as_str()) {
            continue;
        }
        let entry = Entry {
            key: key.clone(),
            stamp: stamp_of(key, Some(writer))?,
            gone: true,
            body: if writer.message {
                EntryBody::Message(Body::default())
            } else {
                EntryBody::Block(Body::default())
            },
        };
        if writer.message {
            record.messages.push(entry);
        } else {
            record.blocks.push(entry);
        }
    }
    record.fields.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
    record.blocks.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
    record.messages.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
    Ok(record)
}

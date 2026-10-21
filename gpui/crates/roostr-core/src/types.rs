//! Wire types: JSON mirrors of glon's proto shapes, exactly as the daemon
//! serves them (`src/lib/types.ts`). Field names follow the JSON (camelCase);
//! every optional field round-trips unchanged.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub type Fields = BTreeMap<String, Value>;

/// One property value (`ValueJSON`). Exactly one variant is normally set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Value {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub string_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub int_value: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub float_value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bool_value: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_value: Option<ListValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_value: Option<MapValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values_value: Option<ValuesValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_value: Option<LinkValue>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ListValue {
    #[serde(default)]
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MapValue {
    #[serde(default)]
    pub entries: Fields,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ValuesValue {
    #[serde(default)]
    pub items: Vec<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkValue {
    #[serde(default)]
    pub target_id: String,
    #[serde(default)]
    pub relation_key: String,
}

impl Value {
    pub fn string(s: impl Into<String>) -> Self {
        Self {
            string_value: Some(s.into()),
            ..Self::default()
        }
    }

    pub fn int(n: i64) -> Self {
        Self {
            int_value: Some(n),
            ..Self::default()
        }
    }

    pub fn float(n: f64) -> Self {
        Self {
            float_value: Some(n),
            ..Self::default()
        }
    }

    pub fn bool(b: bool) -> Self {
        Self {
            bool_value: Some(b),
            ..Self::default()
        }
    }

    pub fn link(target_id: impl Into<String>, relation_key: impl Into<String>) -> Self {
        Self {
            link_value: Some(LinkValue {
                target_id: target_id.into(),
                relation_key: relation_key.into(),
            }),
            ..Self::default()
        }
    }

    pub fn values(items: Vec<Value>) -> Self {
        Self {
            values_value: Some(ValuesValue { items }),
            ..Self::default()
        }
    }

    pub fn map(entries: Fields) -> Self {
        Self {
            map_value: Some(MapValue { entries }),
            ..Self::default()
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.string_value.as_deref()
    }

    /// Integer or float as f64.
    pub fn as_number(&self) -> Option<f64> {
        self.float_value.or(self.int_value.map(|n| n as f64))
    }

    pub fn as_int(&self) -> Option<i64> {
        self.int_value.or(self.float_value.map(|n| n as i64))
    }

    pub fn items(&self) -> &[Value] {
        self.values_value.as_ref().map_or(&[], |v| &v.items)
    }

    pub fn entries(&self) -> Option<&Fields> {
        self.map_value.as_ref().map(|m| &m.entries)
    }

    /// Object ids this value links to: a link, a list of links, strings that
    /// hold ids (legacy single-string form), in order.
    pub fn link_ids(&self) -> Vec<String> {
        if let Some(link) = &self.link_value {
            return if link.target_id.is_empty() {
                vec![]
            } else {
                vec![link.target_id.clone()]
            };
        }
        if let Some(s) = &self.string_value {
            return if s.is_empty() {
                vec![]
            } else {
                vec![s.clone()]
            };
        }
        self.items().iter().flat_map(Value::link_ids).collect()
    }

    pub fn is_empty(&self) -> bool {
        self == &Value::default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub from: u32,
    pub to: u32,
    #[serde(rename = "type")]
    pub kind: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextContent {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub style: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marks: Option<Vec<Mark>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomContent {
    #[serde(default)]
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutContent {
    #[serde(default)]
    pub style: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRowContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_header: Option<bool>,
}

/// Empty marker object (`table`, `tableColumn`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Empty {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<CustomContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<LayoutContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<Empty>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_column: Option<Empty>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_row: Option<TableRowContent>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub id: String,
    #[serde(default)]
    pub children_ids: Vec<String>,
    #[serde(default)]
    pub content: BlockContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<MapValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
}

/// One conversation in an object, decoded by the core (`core/conversation.odin`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    /// "human" | "a2a" | "agent_private"; "" from a newer writer.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub participants: Vec<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub opened_by: String,
    #[serde(default)]
    pub about_message_id: String,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub message_count: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEndpoint {
    pub object_id: String,
    pub agent_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub exchange_id: String,
    pub sender: AgentEndpoint,
    #[serde(default)]
    pub recipients: Vec<AgentEndpoint>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub reply_to: String,
    #[serde(default)]
    pub sent_at: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub request_reply: bool,
    #[serde(default)]
    pub historical: bool,
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageDelivery {
    pub recipient: AgentEndpoint,
    /// "pending" | "delivered" | "failed"
    pub status: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageProcessing {
    /// "pending" | "awaiting_approval" | "processing" | "processed" | "failed" | "held"
    pub status: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxEntry {
    pub message: AgentMessage,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub incoming: bool,
    #[serde(default)]
    pub outgoing: bool,
    #[serde(default)]
    pub deliveries: Vec<MessageDelivery>,
    #[serde(default)]
    pub processing: MessageProcessing,
}

/// Computed object state (`ObjectJSON`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Object {
    pub id: String,
    #[serde(default)]
    pub type_key: String,
    #[serde(default)]
    pub fields: Fields,
    #[serde(default)]
    pub blocks: Vec<Block>,
    /// Absent when the object has never held a conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversations: Option<Vec<Conversation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailbox: Option<Vec<MailboxEntry>>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heads: Option<Vec<String>>,
}

impl Object {
    pub fn field(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    pub fn field_str(&self, key: &str) -> &str {
        field_str(&self.fields, key)
    }

    pub fn name(&self) -> &str {
        self.field_str("name")
    }

    pub fn icon(&self) -> &str {
        self.field_str("icon")
    }

    /// Scoping space id ("" = personal).
    pub fn channel(&self) -> &str {
        self.field_str("channel")
    }

    pub fn block(&self, id: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectSummary {
    pub id: String,
    #[serde(default)]
    pub type_key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub updated_at: i64,
    /// Scoping space object id ("" = personal/unassigned).
    #[serde(default)]
    pub channel_id: String,
    /// Emoji icon ("" = none, render the type glyph).
    #[serde(default)]
    pub icon: String,
    /// The bundled done relation (task-layout checkbox in lists).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpaceMember {
    pub npub: String,
    #[serde(default)]
    pub role: String,
}

/// A space (`SpaceJSON`, served by `/api/channels`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub pinned_ids: Vec<String>,
    #[serde(default)]
    pub members: Vec<SpaceMember>,
    #[serde(default)]
    pub key_id: i64,
    #[serde(default)]
    pub created_at: i64,
    /// "" when this identity administers the space, else the owner's hex pubkey.
    #[serde(default)]
    pub owner: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationOption {
    pub id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub order_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationDef {
    pub id: String,
    pub key: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_emoji: Option<String>,
    /// Owning space id; "" = bundled/system def, present in every space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub max_count: i64,
    /// Type ids the object picker limits candidates to. Empty = unrestricted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_types: Option<Vec<String>>,
    /// Always present: a missing or wrongly typed (non-array) value arrives
    /// as `[]`, like `normalizeRelations`, instead of failing the list.
    #[serde(default, deserialize_with = "options_or_empty")]
    pub options: Vec<RelationOption>,
}

/// `normalizeRelations`: an `options` array is decoded as sent; anything else
/// (null, an object, a string, a number) is replaced with `[]`.
fn options_or_empty<'de, D>(deserializer: D) -> Result<Vec<RelationOption>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};

    struct Options;

    impl<'de> Visitor<'de> for Options {
        type Value = Vec<RelationOption>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a relation options list")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
            Vec::deserialize(de::value::SeqAccessDeserializer::new(seq))
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(Vec::new())
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_any(Options)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRow {
    pub id: String,
    #[serde(default)]
    pub type_key: String,
    /// Only present when the query passed includeDeleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub fields: Fields,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub records: Vec<QueryRow>,
}

/// `/api/settings`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub has_key: bool,
    #[serde(default)]
    pub relays: Vec<String>,
    #[serde(default)]
    pub author_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npub: Option<String>,
    #[serde(default)]
    pub identity_pending: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncPhase {
    #[default]
    Idle,
    Backfill,
    Live,
    Error,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncStatus {
    pub phase: SyncPhase,
    pub imported: u64,
    pub detail: Option<String>,
    /// Changes written locally but not yet published to a relay.
    pub pending: Option<u64>,
    /// Items imported by the history pass in flight; `None` between passes.
    pub received: Option<u64>,
    /// Ids the pass's reconciles said are missing so far (approximate); `None` when unknown.
    pub expected: Option<u64>,
    pub bootstrapped: bool,
    /// `(prepared, total)`: a replica opened without a replay of every
    /// object (the first open after an import or an engine update) replays
    /// them in the background with the workspace already open; reads see
    /// what is prepared so far. `None` once every object is.
    pub preparing: Option<(u64, u64)>,
}

/// glon.Position values.
pub mod pos {
    pub const NONE: u32 = 0;
    pub const TOP: u32 = 1;
    pub const BOTTOM: u32 = 2;
    pub const LEFT: u32 = 3;
    pub const RIGHT: u32 = 4;
    pub const INNER: u32 = 5;
    pub const REPLACE: u32 = 6;
    pub const INNER_FIRST: u32 = 7;
}

/// glon.TextStyle values.
pub mod style {
    pub const PARAGRAPH: u32 = 0;
    pub const HEADER1: u32 = 1;
    pub const HEADER2: u32 = 2;
    pub const HEADER3: u32 = 3;
    pub const QUOTE: u32 = 4;
    pub const CODE: u32 = 5;
    pub const BULLET: u32 = 6;
    pub const NUMBERED: u32 = 7;
    pub const CHECKBOX: u32 = 8;
    pub const TITLE: u32 = 9;
    pub const TOGGLE: u32 = 10;
    pub const CALLOUT: u32 = 11;
    pub const DESCRIPTION: u32 = 12;
}

/// glon.LayoutStyle values.
pub mod layout {
    pub const ROW: u32 = 0;
    pub const COLUMN: u32 = 1;
    pub const DIV: u32 = 2;
    pub const HEADER: u32 = 3;
    pub const TABLE_ROWS: u32 = 4;
    pub const TABLE_COLUMNS: u32 = 5;
}

/// glon.MarkType values.
pub mod mark {
    pub const BOLD: u32 = 0;
    pub const ITALIC: u32 = 1;
    pub const STRIKETHROUGH: u32 = 2;
    pub const UNDERLINE: u32 = 3;
    pub const INLINE_CODE: u32 = 4;
    pub const LINK: u32 = 5;
    pub const TEXT_COLOR: u32 = 6;
    pub const BACKGROUND_COLOR: u32 = 7;
    pub const MENTION: u32 = 8;
    pub const OBJECT_MARK: u32 = 9;
}

/// Engine internals no one makes or edits (`SYSTEM_TYPE_KEYS`): a query with
/// no Source leaves only these out.
pub const SYSTEM_TYPE_KEYS: &[&str] = &[
    "typescript",
    "program",
    "json",
    "proto",
    "peer",
    "pinned_fact",
    "milestone",
    "user_prefs",
    "vanish_log",
    "descriptor",
    "install",
    "set",
    "",
];

pub fn is_system_type(type_key: &str) -> bool {
    SYSTEM_TYPE_KEYS.contains(&type_key)
}

pub fn field_str<'a>(fields: &'a Fields, key: &str) -> &'a str {
    fields.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The object's guest list: agent ids its `agent` property names.
pub fn guest_agents(fields: &Fields) -> Vec<String> {
    fields.get("agent").map(Value::link_ids).unwrap_or_default()
}

/// A Tool shipped with Roostr and seeded by the harness (`tool_builtin`).
pub fn is_builtin_tool(fields: &Fields) -> bool {
    fields.get("tool_builtin").and_then(|v| v.bool_value) == Some(true)
}

/// A built-in Tool from an older harness that runs its own code: read-only.
pub fn is_locked_tool(fields: &Fields) -> bool {
    is_builtin_tool(fields) && field_str(fields, "tool_runtime") != "object"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatFreq {
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl RepeatFreq {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "minute" => Self::Minute,
            "hour" => Self::Hour,
            "day" => Self::Day,
            "week" => Self::Week,
            "month" => Self::Month,
            "year" => Self::Year,
            _ => return None,
        })
    }

    pub fn is_sub_daily(self) -> bool {
        matches!(self, Self::Minute | Self::Hour)
    }
}

/// `repeat_set` rule params (`RepeatRuleJSON`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepeatRule {
    pub freq: RepeatFreq,
    /// 1..999; hour: 1..23.
    pub interval: u32,
    /// 0 = Sun … 6 = Sat.
    pub weekdays: Vec<u8>,
    /// Monthly only: "date" | "weekday".
    pub monthly: String,
    /// Minutes after local midnight.
    pub times: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<[u32; 2]>,
    pub tz: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepeatLastRun {
    pub at: i64,
    pub machine: String,
    pub conversation: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepeatMove {
    pub from: i64,
    pub at: i64,
    pub by: String,
    pub reason: String,
}

/// The engine's `repeat` field, decoded (`RepeatJSON`).
#[derive(Clone, Debug, PartialEq)]
pub struct Repeat {
    pub freq: RepeatFreq,
    pub interval: u32,
    pub weekdays: Vec<u8>,
    /// "date" | "weekday"
    pub monthly: String,
    pub times: Vec<u32>,
    pub window: Option<[u32; 2]>,
    pub tz: String,
    /// Local-day index the cadence counts from.
    pub anchor: i64,
    /// Epoch ms of the current occurrence.
    pub next: i64,
    pub fired_for: Option<i64>,
    pub fired_at: Option<i64>,
    pub fired_by: Option<String>,
    pub last_done: Option<i64>,
    pub count: Option<i64>,
    pub last_run: Option<RepeatLastRun>,
    pub moved: Option<RepeatMove>,
}

/// Decode the `repeat` field (`repeatOf`).
pub fn repeat_of(fields: &Fields) -> Option<Repeat> {
    let e = fields.get("repeat")?.entries()?;
    let int = |k: &str| e.get(k).and_then(|v| v.int_value);
    let str_ = |k: &str| e.get(k).and_then(|v| v.string_value.clone());
    let ints = |v: Option<&Value>| -> Vec<i64> {
        v.map(|v| v.items().iter().filter_map(|i| i.int_value).collect())
            .unwrap_or_default()
    };
    let freq = RepeatFreq::parse(&str_("freq")?)?;
    let anchor = int("anchor")?;
    let next = int("next")?;
    let times: Vec<u32> = match e.get("times") {
        Some(v) => ints(Some(v)).into_iter().map(|n| n as u32).collect(),
        // Rules stored before `times` carry a single `time`.
        None => int("time").map(|t| vec![t as u32]).unwrap_or_default(),
    };
    if !freq.is_sub_daily() && times.is_empty() {
        return None;
    }
    let span = ints(e.get("window"));
    let window = (freq.is_sub_daily() && span.len() == 2).then(|| [span[0] as u32, span[1] as u32]);
    let last_run = e.get("last_run").and_then(Value::entries).and_then(|run| {
        let at = run.get("at")?.int_value?;
        let s = |k: &str| run.get(k).and_then(|v| v.string_value.clone());
        Some(RepeatLastRun {
            at,
            machine: s("machine").unwrap_or_default(),
            conversation: s("conversation").unwrap_or_default(),
            error: s("error"),
        })
    });
    let moved = e.get("moved").and_then(Value::entries).and_then(|m| {
        let from = m.get("from")?.int_value?;
        Some(RepeatMove {
            from,
            at: m.get("at").and_then(|v| v.int_value).unwrap_or(0),
            by: m
                .get("by")
                .and_then(|v| v.string_value.clone())
                .unwrap_or_default(),
            reason: m
                .get("reason")
                .and_then(|v| v.string_value.clone())
                .unwrap_or_default(),
        })
    });
    Some(Repeat {
        freq,
        interval: int("interval").unwrap_or(1) as u32,
        weekdays: ints(e.get("weekdays"))
            .into_iter()
            .map(|n| n as u8)
            .collect(),
        monthly: if str_("monthly").as_deref() == Some("weekday") {
            "weekday".into()
        } else {
            "date".into()
        },
        times,
        window,
        tz: str_("tz").unwrap_or_default(),
        anchor,
        next,
        fired_for: int("fired_for"),
        fired_at: int("fired_at"),
        fired_by: str_("fired_by"),
        last_done: int("last_done"),
        count: int("count"),
        last_run,
        moved,
    })
}

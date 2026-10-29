//! Reader for the canonical `Record` protobuf ([`crate::record::record_canonical`]),
//! the twin of glonOdin `core/record_decode.odin`: packs carry records in
//! this form. Unknown fields are skipped (a newer writer's additions); a
//! truncated message, a wrong wire type on a known field or a string that is
//! not UTF-8 fails the decode.

use crate::record::{Body, Content, Entry, EntryBody, Mark, Record, Value};

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn more(&self) -> bool {
        self.pos < self.data.len()
    }

    fn varint(&mut self) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0u32;
        while let Some(&b) = self.data.get(self.pos) {
            self.pos += 1;
            if shift == 63 && b > 1 {
                return None;
            }
            result |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
        None
    }

    /// (field, wire type).
    fn tag(&mut self) -> Option<(u64, u64)> {
        let tag = self.varint()?;
        Some((tag >> 3, tag & 7))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let size = usize::try_from(self.varint()?).ok()?;
        let end = self.pos.checked_add(size)?;
        let out = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }

    fn string(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }

    fn fixed64(&mut self) -> Option<[u8; 8]> {
        let out = self.data.get(self.pos..self.pos + 8)?.try_into().ok()?;
        self.pos += 8;
        Some(out)
    }

    fn skip(&mut self, wire: u64) -> Option<()> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => {
                self.fixed64()?;
            }
            2 => {
                self.bytes()?;
            }
            5 => {
                self.data.get(self.pos..self.pos + 4)?;
                self.pos += 4;
            }
            _ => return None,
        }
        Some(())
    }

    /// Reads each field with `each(reader, field, wire)`; `each` returns
    /// `None` to fail, `Some(false)` for a field it does not know (skipped).
    fn fields(mut self, mut each: impl FnMut(&mut Self, u64, u64) -> Option<bool>) -> Option<()> {
        while self.more() {
            let (field, wire) = self.tag()?;
            if !each(&mut self, field, wire)? {
                self.skip(wire)?;
            }
        }
        Some(())
    }
}

fn decode_pair<T>(data: &[u8], value: impl Fn(&[u8]) -> Option<T>, default: T) -> Option<(String, T)> {
    let mut key = String::new();
    let mut out = default;
    Reader::new(data).fields(|r, field, wire| match (field, wire) {
        (1, 2) => r.string().map(|s| key = s).map(|()| true),
        (2, 2) => r.bytes().and_then(&value).map(|v| out = v).map(|()| true),
        _ => Some(false),
    })?;
    Some((key, out))
}

fn decode_value_entries(data: &[u8]) -> Option<Vec<(String, Value)>> {
    let mut out = Vec::new();
    Reader::new(data).fields(|r, field, wire| match (field, wire) {
        (1, 2) => {
            out.push(decode_pair(r.bytes()?, decode_value, Value::None)?);
            Some(true)
        }
        _ => Some(false),
    })?;
    Some(out)
}

fn decode_value(data: &[u8]) -> Option<Value> {
    let mut value = Value::None;
    Reader::new(data).fields(|r, field, wire| {
        value = match (field, wire) {
            (1, 2) => Value::String(r.string()?),
            (2, 0) => Value::Int(r.varint()? as i64),
            (3, 1) => Value::Float(f64::from_le_bytes(r.fixed64()?)),
            (4, 0) => Value::Bool(r.varint()? != 0),
            (5, 2) => Value::Bytes(r.bytes()?.to_vec()),
            (6, 2) => {
                let mut items = Vec::new();
                Reader::new(r.bytes()?).fields(|s, f, w| match (f, w) {
                    (1, 2) => s.string().map(|item| items.push(item)).map(|()| true),
                    _ => Some(false),
                })?;
                Value::StringList(items)
            }
            (7, 2) => Value::Map(decode_value_entries(r.bytes()?)?),
            (8, 2) => {
                let mut items = Vec::new();
                Reader::new(r.bytes()?).fields(|s, f, w| match (f, w) {
                    (1, 2) => decode_value(s.bytes()?).map(|item| items.push(item)).map(|()| true),
                    _ => Some(false),
                })?;
                Value::List(items)
            }
            (9, 2) => {
                let (mut target, mut relation) = (String::new(), String::new());
                Reader::new(r.bytes()?).fields(|s, f, w| match (f, w) {
                    (1, 2) => s.string().map(|t| target = t).map(|()| true),
                    (2, 2) => s.string().map(|t| relation = t).map(|()| true),
                    _ => Some(false),
                })?;
                Value::Link { target, relation }
            }
            _ => return Some(false),
        };
        Some(true)
    })?;
    Some(value)
}

fn decode_mark(data: &[u8]) -> Option<Mark> {
    let mut mark = Mark::default();
    Reader::new(data).fields(|r, field, wire| {
        match (field, wire) {
            (1, 0) => mark.from = r.varint()? as i64,
            (2, 0) => mark.to = r.varint()? as i64,
            (3, 0) => mark.kind = r.varint()? as i64,
            (4, 2) => mark.param = r.string()?,
            _ => return Some(false),
        }
        Some(true)
    })?;
    Some(mark)
}

fn decode_content(data: &[u8]) -> Option<Content> {
    let mut content = Content::None;
    Reader::new(data).fields(|r, field, wire| {
        content = match (field, wire) {
            (1, 2) => {
                let (mut text, mut style, mut marks, mut checked, mut color) =
                    (String::new(), 0, Vec::new(), false, String::new());
                Reader::new(r.bytes()?).fields(|s, f, w| {
                    match (f, w) {
                        (1, 2) => text = s.string()?,
                        (2, 0) => style = s.varint()? as i64,
                        (3, 2) => marks.push(decode_mark(s.bytes()?)?),
                        (4, 0) => checked = s.varint()? != 0,
                        (5, 2) => color = s.string()?,
                        _ => return Some(false),
                    }
                    Some(true)
                })?;
                Content::Text {
                    text,
                    style,
                    marks,
                    checked,
                    color,
                }
            }
            (2, 2) => {
                let (mut content_type, mut data, mut meta) = (String::new(), Vec::new(), Vec::new());
                Reader::new(r.bytes()?).fields(|s, f, w| {
                    match (f, w) {
                        (1, 2) => content_type = s.string()?,
                        (2, 2) => data = s.bytes()?.to_vec(),
                        (3, 2) => meta.push(decode_pair(
                            s.bytes()?,
                            |b| String::from_utf8(b.to_vec()).ok(),
                            String::new(),
                        )?),
                        _ => return Some(false),
                    }
                    Some(true)
                })?;
                Content::Custom {
                    content_type,
                    data,
                    meta,
                }
            }
            (3, 2) => {
                let mut style = 0;
                Reader::new(r.bytes()?).fields(|s, f, w| match (f, w) {
                    (1, 0) => s.varint().map(|v| style = v as i64).map(|()| true),
                    _ => Some(false),
                })?;
                Content::Layout(style)
            }
            (4, 2) => {
                r.bytes()?;
                Content::Table
            }
            (5, 2) => {
                r.bytes()?;
                Content::TableColumn
            }
            (6, 2) => {
                let mut header = false;
                Reader::new(r.bytes()?).fields(|s, f, w| match (f, w) {
                    (1, 0) => s.varint().map(|v| header = v != 0).map(|()| true),
                    _ => Some(false),
                })?;
                Content::TableRow(header)
            }
            _ => return Some(false),
        };
        Some(true)
    })?;
    Some(content)
}

fn decode_body(data: &[u8]) -> Option<Body> {
    let mut body = Body::default();
    Reader::new(data).fields(|r, field, wire| {
        match (field, wire) {
            (1, 2) => body.parent = r.string()?,
            (2, 2) => body.position = r.string()?,
            (3, 2) => body.content = decode_content(r.bytes()?)?,
            (4, 2) => body.fields = decode_value_entries(r.bytes()?)?,
            (5, 0) => body.align = r.varint()? as i64,
            (6, 2) => body.background_color = r.string()?,
            _ => return Some(false),
        }
        Some(true)
    })?;
    Some(body)
}

fn decode_entry(data: &[u8]) -> Option<Entry> {
    let mut entry = Entry::default();
    Reader::new(data).fields(|r, field, wire| {
        match (field, wire) {
            (1, 2) => entry.key = r.string()?,
            (2, 2) => Reader::new(r.bytes()?).fields(|s, f, w| {
                match (f, w) {
                    (1, 0) => entry.stamp.time = s.varint()?,
                    (2, 0) => entry.stamp.device = s.varint()? as u32,
                    _ => return Some(false),
                }
                Some(true)
            })?,
            (3, 0) => entry.gone = r.varint()? != 0,
            (4, 2) => entry.body = EntryBody::Value(decode_value(r.bytes()?)?),
            (5, 2) => entry.body = EntryBody::Block(decode_body(r.bytes()?)?),
            (6, 2) => entry.body = EntryBody::Message(decode_body(r.bytes()?)?),
            _ => return Some(false),
        }
        Some(true)
    })?;
    Some(entry)
}

/// The inverse of [`crate::record::record_canonical`]; `None` on malformed bytes.
pub fn record_decode(data: &[u8]) -> Option<Record> {
    let mut record = Record::default();
    Reader::new(data).fields(|r, field, wire| {
        match (field, wire) {
            (1, 2) => record.object_id = r.string()?,
            (2, 2) => record.kind = Some(decode_entry(r.bytes()?)?),
            (3, 2) => record.deleted = Some(decode_entry(r.bytes()?)?),
            (4, 2) => record.fields.push(decode_entry(r.bytes()?)?),
            (5, 2) => record.blocks.push(decode_entry(r.bytes()?)?),
            (6, 2) => record.messages.push(decode_entry(r.bytes()?)?),
            (7, 0) => record.created_at = r.varint()? as i64,
            _ => return Some(false),
        }
        Some(true)
    })?;
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Stamp;

    #[test]
    fn stamp_defaults_survive() {
        let entry = Entry {
            key: "k".into(),
            stamp: Stamp { time: 0, device: 0 },
            gone: true,
            body: EntryBody::Block(Body::default()),
        };
        let record = Record {
            object_id: "o".into(),
            blocks: vec![entry],
            ..Record::default()
        };
        let bytes = crate::record::record_canonical(&record);
        assert_eq!(record_decode(&bytes), Some(record));
        assert_eq!(record_decode(&bytes[..bytes.len() - 1]), None);
    }
}

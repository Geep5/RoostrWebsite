//! Whole-vault export (`src/lib/export.ts`), modeled on Anytype's export
//! popup: Markdown (readable notes) or JSON (the full computed objects —
//! fields, blocks, provenance), one file per object grouped by type, zipped.
//! Everything is read through the backend and written to the Downloads
//! folder (the browser's download in the web build); nothing leaves the
//! machine.

use std::collections::{HashMap, HashSet};
#[cfg(not(target_family = "wasm"))]
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(not(target_family = "wasm"))]
use anyhow::Context as _;
use anyhow::Result;
use roostr_core::io::MaybeSend;
use roostr_core::store::query_all;
use roostr_core::types::{Block, Object, Value};
use roostr_core::{Backend, Fields};
use serde::Serialize as _;
use serde_json::json;

/// Text block styles (`Style` in types.ts).
mod style {
    pub const HEADER1: u32 = 1;
    pub const HEADER2: u32 = 2;
    pub const HEADER3: u32 = 3;
    pub const QUOTE: u32 = 4;
    pub const CODE: u32 = 5;
    pub const BULLET: u32 = 6;
    pub const NUMBERED: u32 = 7;
    pub const CHECKBOX: u32 = 8;
    pub const TOGGLE: u32 = 10;
    pub const CALLOUT: u32 = 11;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
}

impl ExportFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::Json => "JSON",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Json => "json",
        }
    }
}

/// Every object, fully loaded. An export is a backup: a cap here silently
/// ships an incomplete one; unloadable objects are skipped.
async fn fetch_all_objects(backend: Arc<dyn Backend>) -> Result<Vec<Object>> {
    let records = query_all(backend.clone(), json!({})).await?;
    let mut objects = Vec::with_capacity(records.len());
    for record in &records {
        if let Ok(object) = backend.object(&record.id).await {
            objects.push(object);
        }
    }
    Ok(objects)
}

/// `String(n)` for a JS number.
fn js_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0. && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// A value as plain text (`fieldPlain`).
fn field_plain(value: Option<&Value>) -> String {
    let Some(v) = value else { return String::new() };
    if let Some(s) = &v.string_value {
        return s.clone();
    }
    if let Some(n) = v.int_value {
        return n.to_string();
    }
    if let Some(n) = v.float_value {
        return js_number(n);
    }
    if let Some(b) = v.bool_value {
        return b.to_string();
    }
    if let Some(values) = &v.values_value {
        return values
            .items
            .iter()
            .map(|i| i.string_value.clone().unwrap_or_default())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
    }
    String::new()
}

fn md_block(
    b: &Block,
    by_id: &HashMap<&str, &Block>,
    names: &HashMap<String, String>,
    depth: usize,
) -> Vec<String> {
    let pad = "  ".repeat(depth);
    let mut out = Vec::new();
    let text = b.content.text.as_ref();
    if let Some(t) = text {
        let s = t.style;
        let body = &t.text;
        match s {
            style::HEADER1 => out.push(format!("# {body}")),
            style::HEADER2 => out.push(format!("## {body}")),
            style::HEADER3 => out.push(format!("### {body}")),
            style::BULLET => out.push(format!("{pad}- {body}")),
            style::NUMBERED => out.push(format!("{pad}1. {body}")),
            style::CHECKBOX => {
                let mark = if t.checked == Some(true) { "x" } else { " " };
                out.push(format!("{pad}- [{mark}] {body}"));
            }
            style::TOGGLE => out.push(format!("{pad}- ▸ {body}")),
            style::QUOTE => out.push(format!("> {body}")),
            style::CODE => {
                out.push("```".into());
                out.push(body.clone());
                out.push("```".into());
            }
            style::CALLOUT => out.push(format!("> **!** {body}")),
            _ if !body.is_empty() => out.push(format!("{pad}{body}")),
            _ => {}
        }
    } else if let Some(custom) = &b.content.custom {
        let meta = |key: &str| {
            custom
                .meta
                .as_ref()
                .and_then(|m| m.get(key))
                .cloned()
                .unwrap_or_default()
        };
        if custom.content_type == "link" {
            let target = meta("target");
            let name = names
                .get(&target)
                .cloned()
                .unwrap_or_else(|| target.clone());
            out.push(format!("{pad}→ [[{name}]]"));
        } else if custom.content_type == "bookmark" {
            out.push(format!("{pad}<{}>", meta("url")));
        }
    }
    for cid in &b.children_ids {
        let Some(c) = by_id.get(cid.as_str()) else {
            continue;
        };
        if c.content.layout.is_none() {
            out.extend(md_block(
                c,
                by_id,
                names,
                if text.is_some() { depth + 1 } else { depth },
            ));
        } else {
            for gc in &c.children_ids {
                if let Some(g) = by_id.get(gc.as_str()) {
                    out.extend(md_block(g, by_id, names, depth));
                }
            }
        }
    }
    out
}

fn md_for(o: &Object, names: &HashMap<String, String>) -> String {
    let name = field_plain(o.fields.get("name"));
    let name = if name.is_empty() {
        "Untitled".to_string()
    } else {
        name
    };
    let mut lines: Vec<String> = Vec::new();
    // Frontmatter: type + the object's plain property values.
    lines.push("---".into());
    lines.push(format!("type: {}", o.type_key));
    let fields: &Fields = &o.fields;
    for (k, v) in fields {
        if k == "name" || k == "type_key" {
            continue;
        }
        let plain = field_plain(Some(v));
        if !plain.is_empty() {
            lines.push(format!("{k}: {}", plain.replace('\n', " ")));
        }
    }
    lines.extend([
        "---".into(),
        String::new(),
        format!("# {name}"),
        String::new(),
    ]);
    let by_id: HashMap<&str, &Block> = o.blocks.iter().map(|b| (b.id.as_str(), b)).collect();
    let referenced: HashSet<&str> = o
        .blocks
        .iter()
        .flat_map(|b| b.children_ids.iter().map(String::as_str))
        .collect();
    for b in &o.blocks {
        if referenced.contains(b.id.as_str()) || b.id == "__content__" || b.id == "__discussion__" {
            continue;
        }
        lines.extend(md_block(b, &by_id, names, 0));
    }
    lines.join("\n") + "\n"
}

// ── Store-only zip writer (no deps) ───────────────────────────────

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (n, slot) in t.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    t
}

fn crc32(table: &[u32; 256], data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &byte in data {
        c = table[((c ^ byte as u32) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

struct Entry {
    name: String,
    data: Vec<u8>,
}

fn zip(entries: &[Entry]) -> Vec<u8> {
    let table = crc_table();
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let name = e.name.as_bytes();
        let crc = crc32(&table, &e.data);
        let offset = out.len() as u32;
        let len = e.data.len() as u32;
        // Local file header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&[0; 8]); // flags, method (store), time, date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(name);
        out.extend_from_slice(&e.data);
        // Central directory record.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&[0; 8]); // flags, method, time, date
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 12]); // extra, comment, disk, internal attrs, external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // disk numbers
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    out
}

/// `safeName`: ASCII word characters, spaces, `-`, `.` only; 60 chars; id prefix.
fn safe_name(name: &str, id: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ' ' | '-' | '.'))
        .collect();
    let base: String = kept.trim().chars().take(60).collect();
    let base = if base.is_empty() {
        "Untitled".to_string()
    } else {
        base
    };
    let prefix: String = id.chars().take(8).collect();
    format!("{base} {prefix}")
}

fn json_tabs(object: &Object) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut ser = serde_json::Serializer::with_formatter(&mut out, formatter);
    object.serialize(&mut ser)?;
    Ok(out)
}

/// A browser-style download path: `name.zip`, else `name (1).zip`, ….
#[cfg(not(target_family = "wasm"))]
fn download_path(dir: &Path, file: &str) -> PathBuf {
    let first = dir.join(file);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .expect("an unused download name")
}

/// Export every object as Markdown or JSON into one zip in the Downloads
/// folder (a browser download on the web). Run it through
/// [`roostr_core::io::spawn`]; `progress(done, total)` reports each
/// prepared object. Returns the number of objects.
pub async fn export_all(
    backend: Arc<dyn Backend>,
    format: ExportFormat,
    progress: impl Fn(usize, usize) + MaybeSend,
) -> Result<usize> {
    let objects = fetch_all_objects(backend).await?;
    let names: HashMap<String, String> = objects
        .iter()
        .map(|o| {
            let name = field_plain(o.fields.get("name"));
            (
                o.id.clone(),
                if name.is_empty() {
                    "Untitled".to_string()
                } else {
                    name
                },
            )
        })
        .collect();
    let mut entries = Vec::with_capacity(objects.len());
    for (i, o) in objects.iter().enumerate() {
        let fname = safe_name(names.get(&o.id).map_or("Untitled", String::as_str), &o.id);
        let dir = if o.type_key.is_empty() {
            "other"
        } else {
            o.type_key.as_str()
        };
        match format {
            ExportFormat::Markdown => entries.push(Entry {
                name: format!("{dir}/{fname}.md"),
                data: md_for(o, &names).into_bytes(),
            }),
            ExportFormat::Json => entries.push(Entry {
                name: format!("{dir}/{fname}.json"),
                data: json_tabs(o)?,
            }),
        }
        progress(i + 1, objects.len());
    }
    let bytes = zip(&entries);
    let file = format!(
        "roostr-export-{}-{}.zip",
        format.slug(),
        chrono::Utc::now().format("%Y-%m-%d")
    );
    #[cfg(not(target_family = "wasm"))]
    {
        let dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .context("no Downloads folder")?;
        let path = download_path(&dir, &file);
        std::fs::write(&path, bytes)
            .with_context(|| format!("could not write {}", path.display()))?;
    }
    #[cfg(target_family = "wasm")]
    roostr_core::web::download(&bytes, &file, "application/zip")?;
    Ok(objects.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_reference() {
        assert_eq!(crc32(&crc_table(), b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn safe_names() {
        assert_eq!(
            safe_name("Hello/World: ✨ ok", "abcdef123"),
            "HelloWorld  ok abcdef12"
        );
        assert_eq!(safe_name("✨", "x"), "Untitled x");
    }

    #[test]
    fn numbers_print_like_js() {
        assert_eq!(js_number(2.), "2");
        assert_eq!(js_number(1.5), "1.5");
    }
}

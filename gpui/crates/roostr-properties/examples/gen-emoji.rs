//! Regenerate `src/emoji_data.json` (the emoji picker's data) from Unicode's
//! emoji-test.txt and CLDR's English annotations. Port of the old
//! `scripts/gen-emoji.ts`; output is byte-identical for the same inputs.
//!
//! ```sh
//! curl -o /tmp/emoji-test.txt https://unicode.org/Public/emoji/latest/emoji-test.txt
//! curl -o /tmp/annotations.json https://raw.githubusercontent.com/unicode-org/cldr-json/main/cldr-json/cldr-annotations-full/annotations/en/annotations.json
//! cargo run -p roostr-properties --example gen-emoji -- /tmp/emoji-test.txt /tmp/annotations.json \
//!     > crates/roostr-properties/src/emoji_data.json
//! ```
//!
//! emoji-test.txt is the canonical list: every emoji in standard order with a
//! qualification status and CLDR name, split into the standard groups. Kept:
//! fully-qualified sequences only. Dropped: the "Component" group (skin-tone
//! and hair modifiers) and skin-tone/hair variants of a base sequence. The
//! fourth tuple field is the CLDR keywords not already in the name (CLDR keys
//! drop the VS16 selector U+FE0F, so both forms are looked up).
//!
//! The fetch happens once by hand and the result is committed: the build never
//! depends on unicode.org being reachable.

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Cldr {
    annotations: CldrInner,
}
#[derive(Deserialize)]
struct CldrInner {
    annotations: HashMap<String, Annotation>,
}
#[derive(Deserialize)]
struct Annotation {
    #[serde(default)]
    default: Option<Vec<String>>,
}

#[derive(Serialize)]
struct Out {
    groups: Vec<String>,
    emoji: Vec<(String, String, usize, String)>,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [test, cldr] = args.as_slice() else {
        anyhow::bail!("usage: gen-emoji <emoji-test.txt> <annotations.json>");
    };
    let txt = std::fs::read_to_string(test)?;
    let cldr: Cldr = serde_json::from_str(&std::fs::read_to_string(cldr)?)?;
    let annotations = cldr.annotations.annotations;

    let line_re = Regex::new(r"^([0-9A-F ]+);\s*([a-z-]+)\s*#\s*(\S+)\s+E\d+\.\d+\s+(.+)$")?;
    let variant_re = Regex::new(r" skin tone|: .*hair\b")?;

    let mut groups: Vec<String> = Vec::new();
    let mut emoji = Vec::new();
    let mut group: Option<usize> = None;
    for line in txt.split('\n') {
        if let Some(label) = line.strip_prefix("# group:") {
            let label = label.trim();
            group = (label != "Component").then(|| {
                groups.push(label.to_owned());
                groups.len() - 1
            });
            continue;
        }
        let Some(g) = group else { continue };
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(m) = line_re.captures(line) else {
            continue;
        };
        if &m[2] != "fully-qualified" || variant_re.is_match(&m[4]) {
            continue;
        }
        let (e, name) = (m[3].to_owned(), m[4].to_owned());
        let keywords = annotations
            .get(&e)
            .and_then(|a| a.default.as_ref())
            .or_else(|| {
                annotations
                    .get(&e.replace('\u{FE0F}', ""))
                    .and_then(|a| a.default.as_ref())
            })
            .map(|ks| {
                ks.iter()
                    .map(|k| k.to_lowercase())
                    .filter(|k| !name.contains(k.as_str()))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        emoji.push((e, name, g, keywords));
    }
    anyhow::ensure!(
        emoji.len() >= 1000,
        "only parsed {} emoji — source format changed?",
        emoji.len()
    );

    print!("{}", serde_json::to_string(&Out { groups, emoji })?);
    Ok(())
}

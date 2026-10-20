//! Every non-ASCII character in a string or char literal of the UI crates is
//! drawable on the web: the browser build lays text out with only its
//! bundled fonts, so a symbol none of them has renders as a NO GLYPH box.
//! Emoji are exempt (Twemoji, else the browser's own emoji font).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The fonts `theme::bundled_font_data` registers on the web.
const TEXT_FONTS: &[&str] = &[
    "../static/fonts/Inter.ttf",
    "crates/roostr-core/fonts/IBMPlexSans-Regular.ttf",
    "crates/roostr-core/fonts/JetBrainsMono-Regular.ttf",
    "crates/roostr-core/fonts/RoostrSymbols.ttf",
];
const EMOJI_FONT: &str = "crates/roostr-core/fonts/TwemojiMozilla.ttf";

fn covers(path: &str) -> impl Fn(char) -> bool {
    let data = std::fs::read(root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    let face = ttf_parser::Face::parse(&data, 0).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut set = std::collections::HashSet::new();
    for sub in face.tables().cmap.into_iter().flat_map(|c| c.subtables) {
        if sub.is_unicode() {
            sub.codepoints(|c| {
                if sub.glyph_index(c).is_some_and(|g| g.0 != 0) {
                    set.insert(c);
                }
            });
        }
    }
    move |c| set.contains(&(c as u32))
}

/// Joiners, variation selectors and emoji tags draw nothing; private-use
/// characters are in-band sentinels, never drawn.
fn ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{2060}'..='\u{2064}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{E000}'..='\u{F8FF}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// The contents of every string and char literal in `src`, with
/// `\u{…}` escapes decoded; stops at the unit tests (`mod tests`).
fn literals(src: &str) -> Vec<(usize, String)> {
    let src = src.split("\nmod tests {").next().unwrap_or(src);
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    while i < chars.len() {
        match chars[i] {
            '\n' => line += 1,
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            'r' if matches!(chars.get(i + 1), Some('#' | '"'))
                && !chars
                    .get(i.wrapping_sub(1))
                    .is_some_and(|c| c.is_alphanumeric() || *c == '_') =>
            {
                let hashes = chars[i + 1..].iter().take_while(|c| **c == '#').count();
                if chars.get(i + 1 + hashes) == Some(&'"') {
                    let start = i + 2 + hashes;
                    let mut j = start;
                    while j < chars.len()
                        && !(chars[j] == '"'
                            && chars[j + 1..]
                                .iter()
                                .take(hashes)
                                .filter(|c| **c == '#')
                                .count()
                                == hashes)
                    {
                        j += 1;
                    }
                    let s: String = chars[start..j.min(chars.len())].iter().collect();
                    line += s.matches('\n').count();
                    out.push((line, s));
                    i = j + 1 + hashes;
                    continue;
                }
            }
            '"' => {
                let (s, end) = quoted(&chars, i + 1, '"');
                out.push((line, s.clone()));
                line += chars[i..end].iter().filter(|c| **c == '\n').count();
                i = end + 1;
                continue;
            }
            '\'' if chars.get(i + 2) == Some(&'\'') || chars.get(i + 1) == Some(&'\\') => {
                let (s, end) = quoted(&chars, i + 1, '\'');
                out.push((line, s));
                i = end + 1;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The literal from `start` to its closing `quote`, and that quote's index.
fn quoted(chars: &[char], start: usize, quote: char) -> (String, usize) {
    let mut s = String::new();
    let mut j = start;
    while j < chars.len() && chars[j] != quote {
        if chars[j] == '\\' {
            if chars.get(j + 1) == Some(&'u') && chars.get(j + 2) == Some(&'{') {
                let close = j + chars[j..].iter().position(|c| *c == '}').unwrap();
                let hex: String = chars[j + 3..close].iter().filter(|c| **c != '_').collect();
                s.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                j = close + 1;
                continue;
            }
            j += 1;
        }
        s.extend(chars.get(j));
        j += 1;
    }
    (s, j)
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn bundled_fonts_cover_ui_text() {
    let text: Vec<_> = TEXT_FONTS.iter().map(|f| covers(f)).collect();
    let emoji = covers(EMOJI_FONT);
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root().join("crates")).unwrap().flatten() {
        let src = krate.path().join("src");
        if src.is_dir() {
            sources(&src, &mut files);
        }
    }
    assert!(files.len() > 50, "found only {} sources", files.len());
    let mut missing: BTreeMap<char, Vec<String>> = BTreeMap::new();
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap();
        let rel = file
            .strip_prefix(root())
            .unwrap_or(file)
            .display()
            .to_string();
        for (line, s) in literals(&src) {
            for c in s.chars().filter(|c| !c.is_ascii() && !ignorable(*c)) {
                if !text.iter().any(|f| f(c)) && !emoji(c) {
                    missing.entry(c).or_default().push(format!("{rel}:{line}"));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "no bundled font draws these (add them to fonts/RoostrSymbols.ttf or use a Glyph):\n{}",
        missing
            .iter()
            .map(|(c, at)| format!("  {c:?} U+{:04X}: {}", *c as u32, at.join(", ")))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

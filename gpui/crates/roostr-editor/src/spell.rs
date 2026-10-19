//! Basic English spellcheck over the editor's text blocks (port of `spell.ts`).
//!
//! Word lists are embedded at compile time; the caller owns the user's ignore list.

use std::collections::{HashMap, HashSet};

use anyhow::bail;

const HEADWORDS: &str = include_str!("../../../../static/dict-en.txt");
const COMMON: &str = include_str!("../../../../static/dict-en-common.txt");

/// web2 is a headword list - common inflections and contractions are
/// missing (it has "have" but not "has", "say" but not "says").
const SUPPLEMENT: &[&str] = &[
    "has",
    "says",
    "did",
    "done",
    "gone",
    "went",
    "been",
    "am",
    "are",
    "were",
    "cannot",
    "ok",
    "okay",
    "email",
    "emails",
    "todo",
    "todos",
    "app",
    "apps",
    "don't",
    "doesn't",
    "isn't",
    "aren't",
    "wasn't",
    "weren't",
    "can't",
    "won't",
    "wouldn't",
    "couldn't",
    "shouldn't",
    "didn't",
    "hasn't",
    "haven't",
    "hadn't",
    "it's",
    "that's",
    "there's",
    "here's",
    "what's",
    "who's",
    "let's",
    "i'm",
    "i've",
    "i'll",
    "i'd",
    "you're",
    "you've",
    "you'll",
    "you'd",
    "we're",
    "we've",
    "we'll",
    "we'd",
    "they're",
    "they've",
    "they'll",
    "they'd",
    "he's",
    "she's",
    "ain't",
    "y'all",
    "o'clock",
];

const STRIPS: &[(&str, &str)] = &[
    ("ies", "y"),
    ("es", ""),
    ("s", ""),
    ("ed", ""),
    ("ed", "e"),
    ("d", ""),
    ("ing", ""),
    ("ing", "e"),
    ("ly", ""),
    ("er", ""),
    ("er", "e"),
    ("est", ""),
    ("est", "e"),
];

const LETTERS: &str = "abcdefghijklmnopqrstuvwxyz";

/// Split halves must both rank below this ("ed" 1584, "wi" 2323 would produce "occur ed").
const SPLIT_MAX_RANK: u64 = 1500;
/// A split ranks behind common single words but ahead of obscure headwords ("a lot" over "alod").
const SPLIT_PENALTY: u64 = 5000;
/// JS `Number.MAX_SAFE_INTEGER`, the rank of words outside the frequency list.
const UNRANKED: u64 = 9_007_199_254_740_991;

/// The loaded word lists (`dict` + `frequency` in spell.ts).
pub struct Dictionary {
    dict: HashSet<String>,
    frequency: HashMap<String, u64>,
}

/// One misspelled word in a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Misspelling {
    /// Byte offset of the word start.
    pub from: usize,
    /// Byte offset one past the word end.
    pub to: usize,
    /// The misspelled word as written.
    pub word: String,
}

impl Dictionary {
    /// Parse the embedded lists: lowercased headwords + SUPPLEMENT + every common word;
    /// frequency rank by line. Errors when fewer than 100_000 headwords parse.
    pub fn load() -> anyhow::Result<Self> {
        let mut dict = HashSet::with_capacity(260_000);
        for line in HEADWORDS.split('\n') {
            let w = line.trim();
            if !w.is_empty() {
                dict.insert(w.to_lowercase());
            }
        }
        // A plausible list has six figures of entries; anything smaller is a wrong body.
        if dict.len() < 100_000 {
            bail!("dictionary has {} words", dict.len());
        }
        dict.extend(SUPPLEMENT.iter().map(|w| (*w).to_string()));
        // Common words are also known: web2 lacks many inflections the frequency list has.
        let mut frequency = HashMap::new();
        for (i, line) in COMMON.split('\n').enumerate() {
            let w = line.trim().to_lowercase();
            if w.is_empty() {
                continue;
            }
            frequency.insert(w.clone(), i as u64);
            dict.insert(w);
        }
        Ok(Self { dict, frequency })
    }

    /// `isKnown`: base + possessive + the inflection strips table, exactly as spell.ts.
    /// `ignored` holds lowercase words the user added.
    pub fn is_known(&self, word: &str, ignored: &HashSet<String>) -> bool {
        let w = word.to_lowercase();
        if js_len(&w) <= 1 || ignored.contains(&w) || self.dict.contains(&w) {
            return true;
        }
        if let Some(base) = w.strip_suffix("'s")
            && self.dict.contains(base)
        {
            return true;
        }
        let len = js_len(&w);
        STRIPS.iter().any(|(suf, add)| {
            len > suf.len() + 1
                && w.strip_suffix(suf)
                    .is_some_and(|stem| self.dict.contains(&format!("{stem}{add}")))
        })
    }

    /// `checkText`: misspelled tokens. Offsets are UTF-8 byte offsets into `text`.
    pub fn check_text(&self, text: &str, ignored: &HashSet<String>) -> Vec<Misspelling> {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if !bytes[i].is_ascii_alphabetic() {
                i += 1;
                continue;
            }
            let at = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'\'') {
                i += 1;
            }
            let word = text[at..i].trim_end_matches('\'');
            if word.len() <= 1 || word == word.to_uppercase() {
                continue; // single letters, acronyms
            }
            // Inside a URL/path/email? Look back for schemes or separators.
            let start = text[..at]
                .char_indices()
                .rev()
                .take(8)
                .last()
                .map_or(at, |(p, _)| p);
            let before = &text[start..at];
            if before.ends_with(['/', '@', '.', '\\', '-'])
                || before.ends_with("http:")
                || before.ends_with("https:")
            {
                continue;
            }
            if !self.is_known(word, ignored) {
                out.push(Misspelling {
                    from: at,
                    to: at + word.len(),
                    word: word.to_string(),
                });
            }
        }
        out
    }

    /// `suggestions(word, limit)`: edits1 + splits ranked by frequency, two-edit fallback,
    /// matchCase, exactly as spell.ts.
    pub fn suggestions(&self, word: &str, limit: usize, ignored: &HashSet<String>) -> Vec<String> {
        let w = word.to_lowercase();
        if js_len(&w) <= 1 {
            return Vec::new();
        }
        let one = edits1(&w);
        let mut first: Vec<String> = one.clone();
        first.extend(self.splits(&w));
        let mut found = self.rank(first, &w, ignored);
        // Obscure-only hits at distance 1 should not hide a common word at distance 2.
        let best = found.first().map_or(UNRANKED, |c| self.rank_of(c));
        if best == UNRANKED && js_len(&w) <= 12 {
            let mut two = Vec::new();
            let mut seen = HashSet::new();
            for e in &one {
                for f in edits1(e) {
                    if self.frequency.contains_key(&f) && seen.insert(f.clone()) {
                        two.push(f);
                    }
                }
                if two.len() > 200 {
                    break;
                }
            }
            two.extend(found);
            found = self.rank(two, &w, ignored);
        }
        found
            .into_iter()
            .take(limit)
            .map(|s| match_case(word, &s))
            .collect()
    }

    fn rank_of(&self, c: &str) -> u64 {
        match c.split_once(' ') {
            None => self.frequency.get(c).copied().unwrap_or(UNRANKED),
            Some((a, b)) => {
                let fa = self.frequency.get(a).copied().unwrap_or(0);
                let fb = self.frequency.get(b).copied().unwrap_or(0);
                SPLIT_PENALTY + fa.max(fb)
            }
        }
    }

    fn rank(&self, candidates: Vec<String>, w: &str, ignored: &HashSet<String>) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut list: Vec<String> = candidates
            .into_iter()
            .filter(|c| seen.insert(c.clone()))
            .filter(|c| c != w && (c.contains(' ') || self.is_known(c, ignored)))
            .collect();
        let w0 = w.chars().next();
        let wl = js_len(w);
        list.sort_by_cached_key(|c| {
            (
                self.rank_of(c),
                c.chars().next() != w0,
                js_len(c).abs_diff(wl),
                c.clone(),
            )
        });
        list
    }

    /// Split into two very common words ("alot" -> "a lot").
    fn splits(&self, w: &str) -> Vec<String> {
        let rank = |s: &str| self.frequency.get(s).copied().unwrap_or(u64::MAX);
        w.char_indices()
            .skip(1)
            .map(|(i, _)| (&w[..i], &w[i..]))
            .filter(|(a, b)| {
                rank(a) < SPLIT_MAX_RANK
                    && rank(b) < SPLIT_MAX_RANK
                    && (js_len(a) > 1 || *a == "a")
                    && js_len(b) > 1
            })
            .map(|(a, b)| format!("{a} {b}"))
            .collect()
    }
}

/// Character length (JS `.length` for the BMP text this sees).
fn js_len(s: &str) -> usize {
    s.chars().count()
}

/// Every string one edit away, deduplicated in JS `Set` insertion order.
fn edits1(w: &str) -> Vec<String> {
    let chars: Vec<char> = w.chars().collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut add = |s: String| {
        if seen.insert(s.clone()) {
            out.push(s);
        }
    };
    for i in 0..=chars.len() {
        let head: String = chars[..i].iter().collect();
        let tail = &chars[i..];
        let rest: String = tail.iter().skip(1).collect();
        if !tail.is_empty() {
            add(format!("{head}{rest}"));
        }
        if tail.len() > 1 {
            let after: String = tail[2..].iter().collect();
            add(format!("{head}{}{}{after}", tail[1], tail[0]));
        }
        let tail_s: String = tail.iter().collect();
        for c in LETTERS.chars() {
            if !tail.is_empty() {
                add(format!("{head}{c}{rest}"));
            }
            add(format!("{head}{c}{tail_s}"));
        }
    }
    out
}

/// Restore the misspelling's casing pattern onto a lowercase suggestion.
fn match_case(source: &str, suggestion: &str) -> String {
    if source == source.to_uppercase() && js_len(source) > 1 {
        return suggestion.to_uppercase();
    }
    let first_upper = source
        .chars()
        .next()
        .is_some_and(|c| c.to_uppercase().eq(std::iter::once(c)));
    if first_upper {
        let mut cs = suggestion.chars();
        if let Some(f) = cs.next() {
            return f.to_uppercase().chain(cs).collect();
        }
    }
    suggestion.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behaviour() {
        let d = Dictionary::load().expect("load");
        assert!(d.dict.len() > 100_000);
        let ig = HashSet::new();
        for w in ["has", "running", "notes", "quickly", "don't", "Ada's"] {
            assert!(d.is_known(w, &ig), "{w}");
        }
        assert!(!d.is_known("brwn", &ig));
        let t = "Teh quick brwn fox at https://example.com/foo and HTML";
        let m = d.check_text(t, &ig);
        let words: Vec<_> = m.iter().map(|m| (m.from, m.to, m.word.as_str())).collect();
        assert_eq!(words, vec![(0, 3, "Teh"), (10, 14, "brwn")]);
        assert_eq!(d.suggestions("teh", 5, &ig)[0], "the");
        assert_eq!(d.suggestions("Teh", 5, &ig)[0], "The");
        assert!(
            d.suggestions("tommorow", 5, &ig)
                .contains(&"tomorrow".to_string())
        );
        assert!(d.suggestions("alot", 5, &ig).contains(&"a lot".to_string()));
        let mut ig2 = HashSet::new();
        ig2.insert("brwn".to_string());
        assert_eq!(d.check_text(t, &ig2).len(), 1);
    }
}

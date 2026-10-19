//! Ranged-mark algebra for text blocks (`src/lib/marks.ts`).
//!
//! Model: text + marks over `[from, to)` ranges. The wire stores offsets in
//! UTF-16 code units (the web app's string indices); the editor works in
//! UTF-8 byte offsets of the live text and converts at the boundary.

use std::ops::Range;

use roostr_core::types::Mark;

/// A mark over the byte range `[from, to)` of a block's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub from: usize,
    pub to: usize,
    pub kind: u32,
    /// Link URL, mention target, color; `None` when the mark has none.
    pub param: Option<String>,
}

/// Byte offset of the UTF-16 offset `offset` in `text` (clamped to the end).
pub fn utf16_to_byte(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (i, ch) in text.char_indices() {
        if units >= offset {
            return i;
        }
        units += ch.len_utf16();
    }
    text.len()
}

/// UTF-16 offset of the byte offset `byte` in `text`.
pub fn byte_to_utf16(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    text[..floor_char(text, byte)]
        .chars()
        .map(char::len_utf16)
        .sum()
}

/// The char boundary at or before `byte`.
pub fn floor_char(text: &str, byte: usize) -> usize {
    let mut b = byte.min(text.len());
    while !text.is_char_boundary(b) {
        b -= 1;
    }
    b
}

fn norm_param(param: &Option<String>) -> Option<String> {
    param.as_ref().filter(|p| !p.is_empty()).cloned()
}

/// Wire marks → byte spans (clamped, empty ones dropped, merged like `fromDom`).
pub fn spans_from_marks(text: &str, marks: &[Mark]) -> Vec<Span> {
    let spans = marks
        .iter()
        .map(|m| Span {
            from: utf16_to_byte(text, m.from as usize),
            to: utf16_to_byte(text, m.to as usize),
            kind: m.kind,
            param: norm_param(&m.param),
        })
        .filter(|s| s.from < s.to)
        .collect();
    merge_adjacent(spans)
}

/// Byte spans → wire marks.
pub fn marks_from_spans(text: &str, spans: &[Span]) -> Vec<Mark> {
    spans
        .iter()
        .filter(|s| s.from < s.to)
        .map(|s| Mark {
            from: byte_to_utf16(text, s.from) as u32,
            to: byte_to_utf16(text, s.to) as u32,
            kind: s.kind,
            param: s.param.clone(),
        })
        .collect()
}

/// Wire marks in canonical form (what `fromDom` would read back), for comparisons.
pub fn normalized_marks(text: &str, marks: &[Mark]) -> Vec<Mark> {
    marks_from_spans(text, &spans_from_marks(text, marks))
}

/// `mergeAdjacent`: sort by type then start, fuse touching/overlapping marks of the same type and param.
pub fn merge_adjacent(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.from.cmp(&b.from)));
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for m in spans {
        if let Some(last) = out.last_mut()
            && last.kind == m.kind
            && last.param == m.param
            && last.to >= m.from
        {
            last.to = last.to.max(m.to);
            continue;
        }
        out.push(m);
    }
    out
}

/// `toggleMark`: when the whole `[from, to)` already carries `kind`, remove it
/// there; otherwise apply it.
pub fn toggle_mark(
    spans: &[Span],
    from: usize,
    to: usize,
    kind: u32,
    param: Option<String>,
) -> Vec<Span> {
    if from >= to {
        return spans.to_vec();
    }
    let others = spans.iter().filter(|m| m.kind != kind).cloned();
    let mut same: Vec<Span> = spans.iter().filter(|m| m.kind == kind).cloned().collect();
    let covered = {
        let mut sorted = same.clone();
        sorted.sort_by_key(|m| m.from);
        let mut pos = from;
        let mut done = false;
        for m in &sorted {
            if m.from > pos {
                break;
            }
            pos = pos.max(m.to);
            if pos >= to {
                done = true;
                break;
            }
        }
        done || pos >= to
    };
    let mut result = Vec::new();
    if covered {
        for m in same.drain(..) {
            if m.to <= from || m.from >= to {
                result.push(m);
                continue;
            }
            if m.from < from {
                result.push(Span {
                    to: from,
                    ..m.clone()
                });
            }
            if m.to > to {
                result.push(Span { from: to, ..m });
            }
        }
    } else {
        result.extend(same);
        result.push(Span {
            from,
            to,
            kind,
            param: param.filter(|p| !p.is_empty()),
        });
    }
    merge_adjacent(others.chain(result).collect())
}

/// `toRuns`: contiguous byte ranges with a uniform mark set (one entry per
/// mark type, the last mark of a type wins like the web's Map).
pub fn to_runs(len: usize, spans: &[Span]) -> Vec<(Range<usize>, Vec<Span>)> {
    if len == 0 {
        return Vec::new();
    }
    let mut cuts = vec![0, len];
    for m in spans {
        cuts.push(m.from.min(len));
        cuts.push(m.to.min(len));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b {
            continue;
        }
        let mut set: Vec<Span> = Vec::new();
        for m in spans.iter().filter(|m| m.from <= a && m.to >= b) {
            if let Some(held) = set.iter_mut().find(|s| s.kind == m.kind) {
                *held = m.clone();
            } else {
                set.push(m.clone());
            }
        }
        runs.push((a..b, set));
    }
    runs
}

/// Typing `len` bytes at `at`: the text lands in the node left of the caret,
/// so a mark ending at `at` grows (and one starting at 0 when typing at 0).
pub fn adjust_insert(spans: &mut [Span], at: usize, len: usize) {
    if len == 0 {
        return;
    }
    for m in spans.iter_mut() {
        if (m.from < at && m.to >= at) || (at == 0 && m.from == 0) {
            m.to += len;
        } else if m.from >= at {
            m.from += len;
            m.to += len;
        }
    }
}

/// Deleting `range`: marks shrink around it, emptied ones go.
pub fn adjust_delete(spans: &mut Vec<Span>, range: Range<usize>) {
    if range.is_empty() {
        return;
    }
    let gone = range.end - range.start;
    let map = |x: usize| {
        if x <= range.start {
            x
        } else if x >= range.end {
            x - gone
        } else {
            range.start
        }
    };
    for m in spans.iter_mut() {
        m.from = map(m.from);
        m.to = map(m.to);
    }
    spans.retain(|m| m.from < m.to);
}

/// Marks of the head `[0, at)` (split).
pub fn head(spans: &[Span], at: usize) -> Vec<Span> {
    spans
        .iter()
        .filter(|m| m.from < at)
        .map(|m| Span {
            to: m.to.min(at),
            ..m.clone()
        })
        .filter(|m| m.from < m.to)
        .collect()
}

/// Marks of the tail `[at, ..)`, rebased to 0 (split).
pub fn tail(spans: &[Span], at: usize) -> Vec<Span> {
    spans
        .iter()
        .filter(|m| m.to > at)
        .map(|m| Span {
            from: m.from.saturating_sub(at),
            to: m.to - at,
            ..m.clone()
        })
        .filter(|m| m.from < m.to)
        .collect()
}

/// Marks moved right by `by` bytes (merge).
pub fn shifted(spans: &[Span], by: usize) -> Vec<Span> {
    spans
        .iter()
        .map(|m| Span {
            from: m.from + by,
            to: m.to + by,
            ..m.clone()
        })
        .collect()
}

/// The mark of `kind` covering byte `at` (a click on a link or mention).
pub fn mark_at(spans: &[Span], at: usize, kind: u32) -> Option<&Span> {
    spans
        .iter()
        .find(|m| m.kind == kind && m.from <= at && at < m.to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use roostr_core::types::mark;

    fn s(from: usize, to: usize, kind: u32) -> Span {
        Span {
            from,
            to,
            kind,
            param: None,
        }
    }

    #[test]
    fn utf16_round_trip() {
        let t = "a😀b";
        assert_eq!(utf16_to_byte(t, 1), 1);
        assert_eq!(utf16_to_byte(t, 3), 5);
        assert_eq!(byte_to_utf16(t, 5), 3);
        assert_eq!(byte_to_utf16(t, t.len()), 4);
    }

    #[test]
    fn toggle_applies_then_removes() {
        let on = toggle_mark(&[], 2, 5, mark::BOLD, None);
        assert_eq!(on, vec![s(2, 5, mark::BOLD)]);
        let off = toggle_mark(&on, 3, 4, mark::BOLD, None);
        assert_eq!(off, vec![s(2, 3, mark::BOLD), s(4, 5, mark::BOLD)]);
    }

    #[test]
    fn typing_extends_left_mark() {
        let mut m = vec![s(0, 3, mark::BOLD), s(5, 7, mark::ITALIC)];
        adjust_insert(&mut m, 3, 2);
        assert_eq!(m, vec![s(0, 5, mark::BOLD), s(7, 9, mark::ITALIC)]);
        adjust_delete(&mut m, 1..8);
        assert_eq!(m, vec![s(0, 1, mark::BOLD), s(1, 2, mark::ITALIC)]);
    }

    #[test]
    fn split_and_merge() {
        let m = vec![s(1, 6, mark::BOLD)];
        assert_eq!(head(&m, 3), vec![s(1, 3, mark::BOLD)]);
        assert_eq!(tail(&m, 3), vec![s(0, 3, mark::BOLD)]);
        assert_eq!(shifted(&tail(&m, 3), 3), vec![s(3, 6, mark::BOLD)]);
    }
}

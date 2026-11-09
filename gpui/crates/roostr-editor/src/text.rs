//! The live text of one block (the web's contenteditable DOM): a Rune
//! [`TextEdit`] - which owns the caret, selection, typing and the input
//! method - plus the block's marks as byte [`Span`]s.
//!
//! Rune edits the text; [`TextState::sync`] then reads what changed (one
//! replaced range, found by diffing against the text it last saw) and moves
//! the marks the way the web's typing does ([`marks::adjust_delete`] and
//! [`marks::adjust_insert`]). The spans are the truth; the edit's own marks
//! are rebuilt from them after every change for the two kinds Rune draws
//! itself: strikethrough and links (mentions are links to `roostr:object/<id>`,
//! so Rune's link press opens them).

use std::ops::Range;

use roostr_core::types::mark;
use rune::{Mark, MarkRange, TextEdit};

use crate::marks::{self, Span};

/// The link a mention mark is drawn and pressed as.
pub const MENTION_SCHEME: &str = "roostr:object/";

/// What a change to the text was: `removed` (in the old text) became `inserted`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub removed: Range<usize>,
    pub inserted: String,
    /// The text before the change.
    pub before: String,
}

/// The text a block shows right now: what the person typed, not yet saved.
#[derive(Clone, Debug)]
pub struct TextState {
    pub edit: TextEdit,
    pub spans: Vec<Span>,
    /// `edit.text` as the spans last matched it.
    seen: String,
}

impl TextState {
    pub fn new(text: String, spans: Vec<Span>) -> Self {
        let mut t = Self {
            edit: TextEdit::new(text.clone()),
            spans,
            seen: text,
        };
        t.edit.set_caret(0);
        t.apply_marks();
        t
    }

    pub fn text(&self) -> &str {
        &self.edit.text
    }

    /// The selection, ordered; empty = caret.
    pub fn sel(&self) -> Range<usize> {
        let (a, b) = self.edit.range();
        a..b
    }

    /// Where the caret is (the moving end of a selection).
    pub fn cursor(&self) -> usize {
        marks::floor_char(&self.edit.text, self.edit.focus.min(self.edit.text.len()))
    }

    pub fn focused(&self) -> bool {
        self.edit.focused
    }

    /// An input method is composing here (its text isn't in `text` yet).
    pub fn composing(&self) -> bool {
        self.edit.is_composing()
    }

    /// Replace the content (the web's `el.innerHTML = toHtml(...)`), keeping
    /// the caret inside the new text and the field's focus.
    pub fn set_content(&mut self, text: String, spans: Vec<Span>) {
        let (a, f) = (self.edit.anchor, self.edit.focus);
        if self.edit.text != text {
            self.edit.replace_text(text.clone());
        }
        let len = text.len();
        let a = marks::floor_char(&text, a.min(len));
        let f = marks::floor_char(&text, f.min(len));
        self.edit.set_selection(a, f);
        self.seen = text;
        self.spans = spans;
        self.apply_marks();
    }

    pub fn move_to(&mut self, offset: usize) {
        let o = marks::floor_char(&self.edit.text, offset.min(self.edit.text.len()));
        self.edit.set_caret(o);
        self.edit.break_history_group();
    }

    /// Replace `range` with `new` from code (not typing), marks following the edit; caret after it.
    pub fn replace(&mut self, range: Range<usize>, new: &str) {
        let text = &self.edit.text;
        let start = marks::floor_char(text, range.start.min(text.len()));
        let end = marks::floor_char(text, range.end.min(text.len()).max(start));
        marks::adjust_delete(&mut self.spans, start..end);
        marks::adjust_insert(&mut self.spans, start, new.len());
        let mut next = self.edit.text.clone();
        next.replace_range(start..end, new);
        let at = start + new.len();
        self.edit.replace_text(next.clone());
        self.edit.set_caret(at);
        self.seen = next;
        self.apply_marks();
    }

    /// After Rune changed `edit.text`: the change, with the marks moved to match.
    /// `None` when the text is what the spans already match.
    pub fn sync(&mut self) -> Option<Change> {
        if self.edit.text == self.seen {
            return None;
        }
        let old = std::mem::take(&mut self.seen);
        let new = &self.edit.text;
        let (removed, inserted) = diff(&old, new);
        marks::adjust_delete(&mut self.spans, removed.clone());
        marks::adjust_insert(&mut self.spans, removed.start, inserted.len());
        self.seen = new.clone();
        self.apply_marks();
        Some(Change {
            removed,
            inserted,
            before: old,
        })
    }

    /// Rebuild the edit's own marks from the spans: strikethrough, and links
    /// (web links and mentions) so the field draws and presses them.
    pub fn apply_marks(&mut self) {
        let len = self.edit.text.len();
        let mut out = Vec::new();
        for s in &self.spans {
            let (from, to) = (s.from.min(len), s.to.min(len));
            if from >= to {
                continue;
            }
            let m = match s.kind {
                mark::STRIKETHROUGH => Mark::Strike,
                mark::LINK => match s.param.as_deref().map(str::trim) {
                    Some(url) if !url.is_empty() => Mark::Link(url.to_string()),
                    _ => continue,
                },
                mark::MENTION => match s.param.as_deref() {
                    Some(id) if !id.is_empty() => Mark::Link(format!("{MENTION_SCHEME}{id}")),
                    _ => continue,
                },
                _ => continue,
            };
            out.push(MarkRange::new(from..to, m));
        }
        if self.edit.marks() != out.as_slice() {
            self.edit.set_marks(out);
        }
    }
}

/// The one replaced range that turns `old` into `new`: the common prefix
/// and suffix stay (both on character boundaries).
fn diff(old: &str, new: &str) -> (Range<usize>, String) {
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
        start -= 1;
    }
    let max_tail = (old.len() - start).min(new.len() - start);
    let mut tail = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_tail)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - tail) || !new.is_char_boundary(new.len() - tail) {
        tail -= 1;
    }
    (
        start..old.len() - tail,
        new[start..new.len() - tail].to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_finds_the_one_replaced_range() {
        assert_eq!(diff("hello", "hello!"), (5..5, "!".into()));
        assert_eq!(diff("hello", "help"), (3..5, "p".into()));
        assert_eq!(diff("aaa", "aaaa"), (3..3, "a".into()));
        assert_eq!(diff("abc", "ac"), (1..2, String::new()));
        assert_eq!(diff("x↗", "x↘"), (1..4, "↘".into()));
    }

    #[test]
    fn typing_moves_the_marks_like_the_web() {
        let bold = |from, to| Span {
            from,
            to,
            kind: mark::BOLD,
            param: None,
        };
        let mut t = TextState::new("ab cd".into(), vec![bold(3, 5)]);
        // Typing at the end of a mark extends it; before it shifts it.
        t.edit.text = "ab cdX".into();
        t.sync();
        assert_eq!(t.spans, vec![bold(3, 6)]);
        t.edit.text = "Yab cdX".into();
        t.sync();
        assert_eq!(t.spans, vec![bold(4, 7)]);
        t.edit.text = "Y".into();
        t.sync();
        assert!(t.spans.is_empty());
    }

    #[test]
    fn strike_and_links_become_the_fields_marks() {
        let t = TextState::new(
            "see it now".into(),
            vec![
                Span {
                    from: 0,
                    to: 3,
                    kind: mark::STRIKETHROUGH,
                    param: None,
                },
                Span {
                    from: 4,
                    to: 6,
                    kind: mark::MENTION,
                    param: Some("obj-1".into()),
                },
                Span {
                    from: 7,
                    to: 10,
                    kind: mark::BOLD,
                    param: None,
                },
            ],
        );
        let marks: Vec<_> = t
            .edit
            .marks()
            .iter()
            .map(|m| (m.start, m.end, m.mark.clone()))
            .collect();
        assert_eq!(
            marks,
            vec![
                (0, 3, Mark::Strike),
                (4, 6, Mark::Link("roostr:object/obj-1".into()))
            ]
        );
    }
}

//! The keyboard bridge's model: what UIKit's text system is told about the focused Rune field,
//! and the Rune input each of its calls becomes.
//!
//! Rune keeps a field's text to itself; the host only hears where its caret is
//! (`Runtime::ime_area`). UIKit's keyboard, though, reads the text around the caret
//! (`UITextInput`) to capitalize a sentence, autocorrect the word just typed, offer predictions
//! and move the caret with the space bar. So the bridge keeps a [`Document`]: the text typed
//! into the field since its session began, with the caret and the input method's marked text in
//! it, in UTF-16 offsets as UIKit counts. Every change UIKit makes to it is mirrored into Rune as
//! the keys and input-method events a hardware keyboard would send:
//!
//! - typed text is `Key::Text`, a newline the Enter key, a tab the Tab key;
//! - `deleteBackward` is Backspace (at the field's start too: the editor joins the line to the
//!   block above, or turns a list item into a paragraph);
//! - marked text is `Ime::Preedit`, committing it `Ime::Commit`;
//! - a replacement (autocorrect, a prediction) selects its range with the arrow keys and types
//!   over it; moving the caret (the space bar's trackpad) is arrow keys.
//!
//! A session starts empty, so the keyboard sees the start of a document: whenever the bridge
//! can no longer vouch for the text around the caret (another field took focus, a touch moved
//! the caret, Enter split the block) the session is reset and UIKit is told to read it again.

use std::ops::Range;

use rune::{Ime, Key, KeyEvent, Mods};
use unicode_segmentation::UnicodeSegmentation as _;

/// One input event for the Rune runtime.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Key(KeyEvent),
    Ime(Ime),
}

fn key(key: Key) -> Input {
    Input::Key(KeyEvent {
        key,
        mods: Mods::default(),
    })
}

fn shift(key: Key) -> Input {
    Input::Key(KeyEvent {
        key,
        mods: Mods {
            shift: true,
            ..Mods::default()
        },
    })
}

/// The focused field's text as UIKit sees it (see the module docs).
#[derive(Debug, Default)]
pub struct Document {
    text: String,
    /// The selection, UTF-16 offsets into `text`.
    selection: Range<usize>,
    /// The input method's marked text, UTF-16 offsets into `text`.
    marked: Option<Range<usize>>,
    /// The field hides its text: no input method, no text read back.
    secure: bool,
    /// The session's identity: UIKit's positions and ranges carry it, so one made for an
    /// earlier session is refused.
    session: u64,
    /// The session restarted since UIKit last read the document.
    reset: bool,
}

impl Document {
    /// The current session's identity.
    pub fn session(&self) -> u64 {
        self.session
    }

    /// Whether the field is a secure (password) field.
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// The document's length in UTF-16 code units.
    pub fn len(&self) -> usize {
        utf16_len(&self.text)
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// The text in `range` (UTF-16), if the range lies in the document and the field is not
    /// secure.
    pub fn text_in(&self, range: Range<usize>) -> Option<String> {
        if self.secure || !self.contains(&range) {
            return None;
        }
        let (a, b) = (self.byte(range.start), self.byte(range.end));
        Some(self.text[a..b].to_string())
    }

    /// The composed character (grapheme) around UTF-16 `offset`, as a UTF-16 range.
    pub fn character_at(&self, offset: usize) -> Option<Range<usize>> {
        if offset >= self.len() {
            return None;
        }
        let byte = self.byte(offset);
        let (start, grapheme) = self
            .text
            .grapheme_indices(true)
            .take_while(|(start, _)| *start <= byte)
            .last()?;
        let start16 = utf16_len(&self.text[..start]);
        Some(start16..start16 + utf16_len(grapheme))
    }

    /// Whether `range` lies in the document.
    pub fn contains(&self, range: &Range<usize>) -> bool {
        range.start <= range.end && range.end <= self.len()
    }

    /// Takes the "session restarted" flag: true once after each reset, when UIKit must be told
    /// the document changed under it.
    pub fn take_reset(&mut self) -> bool {
        std::mem::take(&mut self.reset)
    }

    /// Starts a new, empty session for a field (`secure` when it hides its text). Marked text
    /// still open is committed, as a native field commits it when the caret moves away.
    pub fn restart(&mut self, secure: bool) -> Vec<Input> {
        let out = self.commit_marked();
        self.text.clear();
        self.selection = 0..0;
        self.marked = None;
        self.secure = secure;
        self.session += 1;
        self.reset = true;
        out
    }

    /// `insertText:`. Over marked text it commits `text` in its place.
    pub fn insert_text(&mut self, text: &str) -> Vec<Input> {
        let mut out = Vec::new();
        let range = match self.marked.take() {
            Some(marked) => {
                let first = first_line(text);
                out.push(Input::Ime(if first.is_empty() {
                    Ime::Preedit(String::new(), None)
                } else {
                    Ime::Commit(first.to_string())
                }));
                let rest = &text[first.len()..];
                self.replace(marked, first);
                self.type_text(rest, &mut out);
                return out;
            }
            None => self.selection.clone(),
        };
        if text.is_empty() {
            if !range.is_empty() {
                out.push(key(Key::Backspace));
                self.replace(range, "");
            }
            return out;
        }
        self.type_text(text, &mut out);
        out
    }

    /// Types `text` over the selection: lines as `Key::Text`, newlines as Enter, tabs as Tab.
    /// Enter and Tab end the session: Rune may split the block, send the message or move focus.
    fn type_text(&mut self, text: &str, out: &mut Vec<Input>) {
        let mut rest = text;
        while !rest.is_empty() {
            let piece = first_line(rest);
            if !piece.is_empty() {
                out.push(key(Key::Text(piece.to_string())));
                let range = self.selection.clone();
                self.replace(range, piece);
                rest = &rest[piece.len()..];
                continue;
            }
            let c = rest.chars().next().expect("rest is not empty");
            out.push(key(if c == '\t' { Key::Tab } else { Key::Enter }));
            rest = &rest[c.len_utf8()..];
            if rest.starts_with('\n') && c == '\r' {
                rest = &rest[1..];
            }
            out.extend(self.restart(self.secure));
        }
    }

    /// `deleteBackward`.
    pub fn delete_backward(&mut self) -> Vec<Input> {
        if let Some(marked) = self.marked.clone() {
            let caret = self.selection.start.clamp(marked.start, marked.end);
            let Some(previous) = (caret > marked.start)
                .then(|| self.character_at(caret - 1))
                .flatten()
            else {
                return Vec::new();
            };
            let removed = previous.len();
            self.replace(previous.clone(), "");
            let marked = marked.start..marked.end - removed;
            self.selection = previous.start..previous.start;
            if marked.is_empty() {
                self.marked = None;
                return vec![Input::Ime(Ime::Preedit(String::new(), None))];
            }
            self.marked = Some(marked);
            return vec![self.preedit()];
        }
        let range = if self.selection.is_empty() {
            match self.selection.start.checked_sub(1) {
                Some(before) => self.character_at(before).unwrap_or(before..before + 1),
                None => 0..0,
            }
        } else {
            self.selection.clone()
        };
        self.replace(range, "");
        vec![key(Key::Backspace)]
    }

    /// `setMarkedText:selectedRange:`: `text` replaces the marked text (else the selection),
    /// with `selected` (UTF-16, within `text`) as its cursor. Empty text ends the composition.
    /// A secure field has no input method: the text is typed.
    pub fn set_marked_text(&mut self, text: &str, selected: Range<usize>) -> Vec<Input> {
        if self.secure {
            self.marked = None;
            return self.insert_text(text);
        }
        let mut out = Vec::new();
        let range = match self.marked.clone() {
            Some(marked) => marked,
            None => {
                let selection = self.selection.clone();
                if !selection.is_empty() {
                    out.push(key(Key::Backspace));
                    self.replace(selection.clone(), "");
                }
                selection.start..selection.start
            }
        };
        self.replace(range.clone(), text);
        if text.is_empty() {
            if self.marked.take().is_some() {
                out.push(Input::Ime(Ime::Preedit(String::new(), None)));
            }
            return out;
        }
        let len = utf16_len(text);
        let start = range.start;
        self.marked = Some(start..start + len);
        let cursor = selected.start.min(len)..selected.end.min(len).max(selected.start.min(len));
        self.selection = start + cursor.start..start + cursor.end;
        out.push(self.preedit());
        out
    }

    /// `unmarkText`: the marked text becomes the field's.
    pub fn unmark_text(&mut self) -> Vec<Input> {
        self.commit_marked()
    }

    fn commit_marked(&mut self) -> Vec<Input> {
        let Some(marked) = self.marked.take() else {
            return Vec::new();
        };
        let text = self.text_range(marked.clone()).to_string();
        self.selection = marked.end..marked.end;
        vec![Input::Ime(Ime::Commit(text))]
    }

    /// `replaceRange:withText:` (autocorrect, a prediction, smart punctuation): the arrows
    /// select `range`, then `text` is typed over it.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) -> Vec<Input> {
        if !self.contains(&range) {
            return Vec::new();
        }
        if self.marked.as_ref() == Some(&range) {
            return self.insert_text(text);
        }
        let mut out = self.commit_marked();
        out.extend(self.select(range));
        out.extend(self.insert_text(text));
        out
    }

    /// `setSelectedTextRange:`. Inside marked text it moves the composition's cursor;
    /// elsewhere the arrow keys move Rune's caret or selection there.
    pub fn set_selection(&mut self, range: Range<usize>) -> Vec<Input> {
        if !self.contains(&range) || range == self.selection {
            return Vec::new();
        }
        if let Some(marked) = self.marked.clone() {
            if marked.start <= range.start && range.end <= marked.end {
                self.selection = range;
                return vec![self.preedit()];
            }
            let mut out = self.commit_marked();
            out.extend(self.select(range));
            return out;
        }
        self.select(range)
    }

    /// The arrow keys that take Rune's selection from `self.selection` to `range` (no marked
    /// text): Right collapses a selection to its end, Left/Right walk the caret to the range's
    /// start a grapheme at a time, Shift+Right extends over it.
    fn select(&mut self, range: Range<usize>) -> Vec<Input> {
        let mut out = Vec::new();
        if range == self.selection {
            return out;
        }
        let mut caret = self.selection.end;
        if !self.selection.is_empty() {
            out.push(key(Key::Right));
        }
        let to = range.start;
        let (from, to_, step) = if to < caret {
            (to, caret, Key::Left)
        } else {
            (caret, to, Key::Right)
        };
        for _ in 0..self.graphemes(from..to_) {
            out.push(key(step.clone()));
        }
        caret = to;
        for _ in 0..self.graphemes(caret..range.end) {
            out.push(shift(Key::Right));
        }
        self.selection = range;
        out
    }

    /// The marked text as an `Ime::Preedit`, its cursor a byte range within it.
    fn preedit(&self) -> Input {
        let marked = self.marked.clone().unwrap_or(0..0);
        let text = self.text_range(marked.clone());
        let local = |offset: usize| {
            let offset = offset.clamp(marked.start, marked.end) - marked.start;
            utf16_to_byte(text, offset)
        };
        let cursor = (local(self.selection.start), local(self.selection.end));
        Input::Ime(Ime::Preedit(text.to_string(), Some(cursor)))
    }

    fn text_range(&self, range: Range<usize>) -> &str {
        &self.text[self.byte(range.start)..self.byte(range.end)]
    }

    /// Graphemes in UTF-16 `range`: the arrow presses that cross it.
    fn graphemes(&self, range: Range<usize>) -> usize {
        self.text_range(range).graphemes(true).count()
    }

    /// Replaces UTF-16 `range` with `text`, leaving the caret after it and moving the marked
    /// range along.
    fn replace(&mut self, range: Range<usize>, text: &str) {
        let (a, b) = (self.byte(range.start), self.byte(range.end));
        let start = utf16_len(&self.text[..a]);
        let removed = utf16_len(&self.text[a..b]);
        self.text.replace_range(a..b, text);
        let added = utf16_len(text);
        self.selection = start + added..start + added;
        if let Some(marked) = &mut self.marked
            && marked.start >= start + removed
        {
            *marked = marked.start - removed + added..marked.end - removed + added;
        }
    }

    /// UTF-16 `offset` as a byte index, on a character boundary at or before it.
    fn byte(&self, offset: usize) -> usize {
        utf16_to_byte(&self.text, offset)
    }
}

/// `text` up to its first newline or tab.
fn first_line(text: &str) -> &str {
    text.find(['\n', '\r', '\t']).map_or(text, |i| &text[..i])
}

pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// UTF-16 `offset` in `text` as a byte index, clamped to the text and rounded down to a
/// character boundary.
pub fn utf16_to_byte(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units + c.len_utf16() > offset {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

/// `location..location + length` if it lies within `len` (UIKit's `NSRange` checks).
pub fn checked_range(location: usize, length: usize, len: usize) -> Option<Range<usize>> {
    let end = location.checked_add(length)?;
    (end <= len).then_some(location..end)
}

/// `position + offset` if it stays within `0..=len`.
pub fn checked_offset(position: usize, offset: isize, len: usize) -> Option<usize> {
    let position = position.checked_add_signed(offset)?;
    (position <= len).then_some(position)
}

/// A caret rect UIKit can show: Rune reports the caret's drawn width, which may be zero.
pub fn visible_caret(rect: [f32; 4]) -> [f32; 4] {
    [rect[0], rect[1], rect[2].max(1.), rect[3].max(1.)]
}

/// A hardware key press as UIKit reports it (`UIKey`).
#[derive(Clone, Debug, Default)]
pub struct Press {
    /// `UIKeyboardHIDUsage`.
    pub code: isize,
    /// `charactersIgnoringModifiers`.
    pub base: String,
    /// `characters`: what the key types with its modifiers.
    pub characters: String,
    pub mods: Mods,
}

/// What a hardware key press does.
#[derive(Clone, Debug, PartialEq)]
pub enum Pressed {
    /// Send Rune this key.
    Key(KeyEvent),
    /// ⌘V: the host pastes the system pasteboard (UIKit's own Edit ▸ Paste key command never
    /// reaches a custom text input view).
    Paste,
    /// Leave it to UIKit: its text system types, deletes or composes through the bridge.
    Text,
}

/// `UIKeyboardHIDUsage` values the bridge maps.
mod hid {
    pub const ENTER: isize = 0x28;
    pub const ESCAPE: isize = 0x29;
    pub const BACKSPACE: isize = 0x2A;
    pub const TAB: isize = 0x2B;
    pub const F1: isize = 0x3A;
    pub const F12: isize = 0x45;
    pub const HOME: isize = 0x4A;
    pub const PAGE_UP: isize = 0x4B;
    pub const DELETE: isize = 0x4C;
    pub const END: isize = 0x4D;
    pub const PAGE_DOWN: isize = 0x4E;
    pub const RIGHT: isize = 0x4F;
    pub const LEFT: isize = 0x50;
    pub const DOWN: isize = 0x51;
    pub const UP: isize = 0x52;
    pub const KEYPAD_ENTER: isize = 0x58;
}

/// What hardware key `press` does. `typing`: the bridge is the first responder for a field, so
/// UIKit's text system types its characters; `composing`: marked text is open, and every key
/// is the input method's.
pub fn hardware_key(press: &Press, typing: bool, composing: bool) -> Pressed {
    if composing {
        return Pressed::Text;
    }
    let m = press.mods;
    let named = match press.code {
        hid::ESCAPE => Some(Key::Escape),
        hid::TAB => Some(Key::Tab),
        hid::HOME => Some(Key::Home),
        hid::END => Some(Key::End),
        hid::PAGE_UP => Some(Key::PageUp),
        hid::PAGE_DOWN => Some(Key::PageDown),
        hid::DELETE => Some(Key::Delete),
        hid::RIGHT => Some(Key::Right),
        hid::LEFT => Some(Key::Left),
        hid::DOWN => Some(Key::Down),
        hid::UP => Some(Key::Up),
        hid::F1..=hid::F12 => Some(Key::Function((press.code - hid::F1 + 1) as u8)),
        // Plain Return and Backspace go through UIKit while typing, so a composition or an
        // autocorrection settles first; with a modifier they are the app's keys.
        hid::ENTER | hid::KEYPAD_ENTER if !typing || m.shift || m.cmd || m.alt || m.ctrl => {
            Some(Key::Enter)
        }
        hid::BACKSPACE if !typing || m.cmd || m.alt || m.ctrl => Some(Key::Backspace),
        _ => None,
    };
    if let Some(named) = named {
        return Pressed::Key(KeyEvent {
            key: named,
            mods: m,
        });
    }
    if m.cmd {
        let Some(c) = press.base.chars().next() else {
            return Pressed::Text;
        };
        let c = c.to_lowercase().next().unwrap_or(c);
        if c == 'v' && !m.shift && !m.alt && !m.ctrl {
            return Pressed::Paste;
        }
        return Pressed::Key(KeyEvent {
            key: Key::Command(c),
            mods: m,
        });
    }
    if typing || press.characters.is_empty() || press.characters.chars().any(char::is_control) {
        return Pressed::Text;
    }
    // Nothing takes text: the window's keys hear the character.
    Pressed::Key(KeyEvent {
        key: Key::Text(press.characters.clone()),
        mods: m,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> Input {
        key(Key::Text(t.into()))
    }

    fn preedit(t: &str, cursor: (usize, usize)) -> Input {
        Input::Ime(Ime::Preedit(t.into(), Some(cursor)))
    }

    fn commit(t: &str) -> Input {
        Input::Ime(Ime::Commit(t.into()))
    }

    #[test]
    fn typing_is_text_keys_and_return_is_enter() {
        let mut d = Document::default();
        assert_eq!(d.insert_text("H"), [text("H")]);
        assert_eq!(d.insert_text("i"), [text("i")]);
        assert_eq!(d.text_in(0..2).as_deref(), Some("Hi"));
        assert_eq!(d.selection(), 2..2);
        let session = d.session();
        assert_eq!(d.insert_text("\n"), [key(Key::Enter)]);
        // Enter may have split the block or sent the message: a new, empty session.
        assert!(d.session() > session);
        assert!(d.take_reset());
        assert!(!d.take_reset());
        assert!(d.is_empty());
    }

    #[test]
    fn multi_line_text_types_lines_between_enters() {
        let mut d = Document::default();
        assert_eq!(
            d.insert_text("one\ntwo\r\nthree\tfour"),
            [
                text("one"),
                key(Key::Enter),
                text("two"),
                key(Key::Enter),
                text("three"),
                key(Key::Tab),
                text("four")
            ]
        );
        assert_eq!(d.text_in(0..4).as_deref(), Some("four"));
    }

    #[test]
    fn delete_backward_is_backspace_and_follows_graphemes() {
        let mut d = Document::default();
        d.insert_text("a👨‍👩‍👧");
        assert_eq!(d.len(), 1 + 8);
        assert_eq!(d.delete_backward(), [key(Key::Backspace)]);
        assert_eq!(d.text_in(0..1).as_deref(), Some("a"));
        assert_eq!(d.delete_backward(), [key(Key::Backspace)]);
        assert!(d.is_empty());
        // At the start there is no text of ours, but the field may have some (or join the
        // block above): the key still goes.
        assert_eq!(d.delete_backward(), [key(Key::Backspace)]);
    }

    #[test]
    fn marked_text_is_a_preedit_and_unmarking_commits_it() {
        let mut d = Document::default();
        d.insert_text("x");
        assert_eq!(d.set_marked_text("か", 1..1), [preedit("か", (3, 3))]);
        assert_eq!(d.marked(), Some(1..2));
        assert_eq!(d.set_marked_text("かな", 2..2), [preedit("かな", (6, 6))]);
        assert_eq!(d.marked(), Some(1..3));
        // The candidate bar moves the composition's cursor.
        assert_eq!(d.set_selection(1..2), [preedit("かな", (0, 3))]);
        assert_eq!(d.unmark_text(), [commit("かな")]);
        assert_eq!(d.marked(), None);
        assert_eq!(d.selection(), 3..3);
        assert_eq!(d.text_in(0..3).as_deref(), Some("xかな"));
    }

    #[test]
    fn inserting_over_marked_text_commits_the_insertion() {
        let mut d = Document::default();
        d.set_marked_text("nihon", 5..5);
        assert_eq!(d.insert_text("日本"), [commit("日本")]);
        assert_eq!(d.text_in(0..2).as_deref(), Some("日本"));
        assert_eq!(d.marked(), None);
        // Clearing the marked text cancels the composition.
        d.set_marked_text("ka", 2..2);
        assert_eq!(
            d.set_marked_text("", 0..0),
            [Input::Ime(Ime::Preedit(String::new(), None))]
        );
        assert_eq!(d.text_in(0..2).as_deref(), Some("日本"));
    }

    #[test]
    fn delete_backward_in_marked_text_shortens_the_preedit() {
        let mut d = Document::default();
        d.set_marked_text("ab", 2..2);
        assert_eq!(d.delete_backward(), [preedit("a", (1, 1))]);
        assert_eq!(
            d.delete_backward(),
            [Input::Ime(Ime::Preedit(String::new(), None))]
        );
        assert_eq!(d.marked(), None);
        assert!(d.is_empty());
    }

    #[test]
    fn autocorrect_selects_the_word_with_arrows_and_types_over_it() {
        let mut d = Document::default();
        d.insert_text("I saw teh");
        assert_eq!(
            d.replace_range(6..9, "the"),
            [
                key(Key::Left),
                key(Key::Left),
                key(Key::Left),
                shift(Key::Right),
                shift(Key::Right),
                shift(Key::Right),
                text("the")
            ]
        );
        assert_eq!(d.text_in(0..9).as_deref(), Some("I saw the"));
        assert_eq!(d.selection(), 9..9);
        // Removing a range types nothing over the selection: Backspace.
        let out = d.replace_range(5..9, "");
        assert_eq!(out.last(), Some(&key(Key::Backspace)));
        assert_eq!(d.text_in(0..5).as_deref(), Some("I saw"));
    }

    #[test]
    fn moving_the_caret_is_arrow_keys_by_grapheme() {
        let mut d = Document::default();
        d.insert_text("é👍z");
        assert_eq!(d.len(), 4);
        assert_eq!(
            d.set_selection(1..1),
            [key(Key::Left), key(Key::Left)],
            "the emoji's two UTF-16 units are one arrow press"
        );
        assert_eq!(d.set_selection(4..4), [key(Key::Right), key(Key::Right)]);
        assert_eq!(d.set_selection(4..4), []);
        assert_eq!(
            d.set_selection(0..1),
            [
                key(Key::Left),
                key(Key::Left),
                key(Key::Left),
                shift(Key::Right)
            ]
        );
        // A selection collapses to its end first.
        assert_eq!(
            d.set_selection(4..4),
            [key(Key::Right), key(Key::Right), key(Key::Right)]
        );
        assert_eq!(d.set_selection(9..9), [], "outside the document");
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut d = Document::default();
        d.insert_text("abc");
        d.set_selection(0..3);
        assert_eq!(d.insert_text("x"), [text("x")]);
        assert_eq!(d.text_in(0..1).as_deref(), Some("x"));
        d.set_selection(0..1);
        assert_eq!(d.delete_backward(), [key(Key::Backspace)]);
        assert!(d.is_empty());
    }

    #[test]
    fn secure_fields_type_without_an_input_method_and_hide_their_text() {
        let mut d = Document::default();
        d.restart(true);
        assert!(d.secure());
        assert_eq!(d.set_marked_text("p", 1..1), [text("p")]);
        assert_eq!(d.marked(), None);
        assert_eq!(d.text_in(0..1), None);
    }

    #[test]
    fn restarting_commits_open_marked_text_and_refuses_old_positions() {
        let mut d = Document::default();
        d.set_marked_text("ni", 2..2);
        let old = d.session();
        assert_eq!(d.restart(false), [commit("ni")]);
        assert_ne!(d.session(), old);
        assert!(d.is_empty());
        assert_eq!(d.marked(), None);
        assert_eq!(d.text_in(0..2), None);
    }

    #[test]
    fn composed_characters_and_utf16_ranges() {
        let mut d = Document::default();
        d.insert_text("a😀e\u{301}👨‍👩‍👧‍👦");
        assert_eq!(d.character_at(1), Some(1..3));
        assert_eq!(d.character_at(2), Some(1..3));
        assert_eq!(d.character_at(4), Some(3..5));
        assert_eq!(d.character_at(6), Some(5..16));
        assert_eq!(d.character_at(16), None);
        assert!(d.contains(&(4..4)));
        assert!(!d.contains(&(15..17)));
        let reversed = Range { start: 6, end: 5 };
        assert!(!d.contains(&reversed));
        let length = utf16_len("a😀e\u{301}");
        assert_eq!(length, 5);
        assert_eq!(checked_range(1, 2, length), Some(1..3));
        assert_eq!(checked_range(length, 0, length), Some(length..length));
        assert_eq!(checked_range(usize::MAX, 1, length), None);
        assert_eq!(checked_range(4, 2, length), None);
        assert_eq!(checked_offset(3, -2, length), Some(1));
        assert_eq!(checked_offset(0, -1, length), None);
        assert_eq!(checked_offset(length, 1, length), None);
        assert_eq!(checked_offset(0, isize::MIN, length), None);
        assert_eq!(
            utf16_to_byte("a😀b", 2),
            1,
            "inside a surrogate pair: its start"
        );
        assert_eq!(utf16_to_byte("a😀b", 3), 5);
        assert_eq!(utf16_to_byte("a😀b", 9), 6);
    }

    #[test]
    fn insertion_geometry_has_a_visible_caret_width() {
        assert_eq!(visible_caret([12., 20., 0., 30.]), [12., 20., 1., 30.]);
        assert_eq!(visible_caret([12., 20., 2., 30.]), [12., 20., 2., 30.]);
    }

    fn press(code: isize, base: &str, mods: Mods) -> Press {
        Press {
            code,
            base: base.into(),
            characters: base.into(),
            mods,
        }
    }

    #[test]
    fn hardware_keys() {
        let none = Mods::default();
        let cmd = Mods {
            cmd: true,
            logo: true,
            ..Mods::default()
        };
        let shift_mods = Mods {
            shift: true,
            ..Mods::default()
        };
        // Arrows and Escape are Rune's keys, with their modifiers.
        assert_eq!(
            hardware_key(&press(hid::LEFT, "", shift_mods), true, false),
            Pressed::Key(KeyEvent {
                key: Key::Left,
                mods: shift_mods
            })
        );
        assert_eq!(
            hardware_key(&press(hid::ESCAPE, "\u{1b}", none), false, false),
            Pressed::Key(KeyEvent {
                key: Key::Escape,
                mods: none
            })
        );
        // While typing, plain characters, Return and Backspace go through UIKit's text system.
        assert_eq!(
            hardware_key(&press(0x04, "a", none), true, false),
            Pressed::Text
        );
        assert_eq!(
            hardware_key(&press(hid::ENTER, "\r", none), true, false),
            Pressed::Text
        );
        assert_eq!(
            hardware_key(&press(hid::BACKSPACE, "\u{8}", none), true, false),
            Pressed::Text
        );
        // Shift+Return and ⌥⌫ are the app's.
        assert_eq!(
            hardware_key(&press(hid::ENTER, "\r", shift_mods), true, false),
            Pressed::Key(KeyEvent {
                key: Key::Enter,
                mods: shift_mods
            })
        );
        // ⌘ chords are commands; ⌘V pastes the system pasteboard.
        assert_eq!(
            hardware_key(&press(0x0E, "K", cmd), true, false),
            Pressed::Key(KeyEvent {
                key: Key::Command('k'),
                mods: cmd
            })
        );
        assert_eq!(
            hardware_key(&press(0x19, "v", cmd), true, false),
            Pressed::Paste
        );
        // With nothing to type into, characters are the window's keys.
        assert_eq!(
            hardware_key(&press(0x38, "/", none), false, false),
            Pressed::Key(KeyEvent {
                key: Key::Text("/".into()),
                mods: none
            })
        );
        // A composition owns every key.
        assert_eq!(
            hardware_key(&press(hid::LEFT, "", none), true, true),
            Pressed::Text
        );
        assert_eq!(
            hardware_key(&press(hid::F1 + 4, "", none), false, false),
            Pressed::Key(KeyEvent {
                key: Key::Function(5),
                mods: none
            })
        );
    }
}

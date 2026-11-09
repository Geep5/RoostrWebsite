//! Keyboard, text input and mouse-in-text handling (`Editor.svelte`
//! onKeydown / onBeforeInput / onSelect / selection provider).
//!
//! Each block's text is a Rune field: Rune types, moves the caret, selects,
//! composes (IME), copies and cuts. The editor hears a field's keys first
//! ([`input_key`]: Enter splits, Tab nests, Backspace at the start merges,
//! arrows at the edges move between blocks, the mark and undo shortcuts),
//! then every change Rune made ([`input_changed`]: marks follow, "/" opens
//! the slash menu, "--- " makes a divider, typing saves), and once a frame
//! where the keyboard is ([`on_poll`]: the block that holds the caret, the
//! marks toolbar over a selection). With blocks selected the editor's own
//! element has the keyboard ([`editor_key`]).

use std::collections::HashSet;

use roostr_core::State;
use roostr_core::types::{TextContent, mark, pos, style};
use rune::{Chord, ClipboardContent, Key, KeyEvent, Tx};

use crate::doc::Op;
use crate::editor::{
    DragSel, Editor, Editors, SelMode, Slash, StructuralKey, Toolbar, new_id, place, text_block,
    text_content, with_editor,
};
use crate::marks::{self, Span};
use crate::text::MENTION_SCHEME;
use crate::toggles;

const LIST: [u32; 4] = [
    style::BULLET,
    style::NUMBERED,
    style::CHECKBOX,
    style::TOGGLE,
];
const DIVIDERS: [&str; 3] = ["---", "—-", "***"];

/// The element key of block `id`'s text field.
pub(crate) fn text_key(id: &str) -> String {
    format!("text-{id}")
}

/// The element key of block `id`'s row.
pub(crate) fn row_key(id: &str) -> String {
    format!("block-{id}")
}

/// The element key of `object_id`'s editor (it takes the keyboard while blocks are selected).
pub(crate) fn editor_key(object_id: &str) -> String {
    format!("ed-{object_id}")
}

fn chord(spec: &str) -> Chord {
    Chord::parse(spec).expect("a valid chord")
}

/// Once a frame (`State::on_poll`): follow the keyboard between blocks in every open editor.
pub(crate) fn on_poll(s: &mut State) -> bool {
    let ids: Vec<String> = s
        .get::<Editors>()
        .map(|e| e.map.keys().cloned().collect())
        .unwrap_or_default();
    let mut changed = false;
    for id in ids {
        changed |= with_editor(s, &id, |e, s| e.poll(s)).unwrap_or(false);
    }
    changed
}

/// A key in block `id`'s focused field, before Rune's editing: true when the editor took it.
pub(crate) fn input_key(s: &mut State, oid: &str, id: &str, ev: &KeyEvent, tx: &mut Tx) -> bool {
    with_editor(s, oid, |e, s| e.input_key(id, ev, s, tx)).unwrap_or(false)
}

/// After Rune changed block `id`'s text.
pub(crate) fn input_changed(s: &mut State, oid: &str, id: &str) {
    with_editor(s, oid, |e, s| e.text_changed(id, s));
}

/// A paste into block `id`'s field: true when the editor took it (blocks, images, a URL).
pub(crate) fn input_paste(
    s: &mut State,
    oid: &str,
    id: &str,
    content: &ClipboardContent,
    tx: &mut Tx,
) -> bool {
    with_editor(s, oid, |e, s| e.paste_into(id, content, s, tx)).unwrap_or(false)
}

/// A press on a link or mention in a block's text: a mention opens its object, a web link
/// opens in the browser (only http(s), mailto and tel).
pub(crate) fn open_link(s: &mut State, url: &str, tx: &mut Tx) {
    if let Some(target) = url.strip_prefix(MENTION_SCHEME) {
        roostr_core::nav::navigate(roostr_core::Route::Object(target.to_string()), s);
        return;
    }
    // The stored param is the URL itself (the web decodes its attribute back to it).
    let raw = url.trim().to_string();
    let has_scheme =
        regex::Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.-]*:").is_ok_and(|re| re.is_match(&raw));
    let href = if has_scheme {
        raw
    } else {
        format!("https://{raw}")
    };
    let lower = href.to_lowercase();
    if lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
    {
        if let Err(error) = tx.open_url(&href) {
            log::warn!("[editor] could not open {href}: {error}");
        }
    }
}

/// A key while the editor itself has the keyboard (blocks selected, no caret).
pub(crate) fn editor_keys(s: &mut State, oid: &str, ev: &KeyEvent, tx: &mut Tx) -> bool {
    with_editor(s, oid, |e, s| e.selection_key(ev, s, tx)).unwrap_or(false)
}

impl Editor {
    /// A table cell (a text block under a table row): its own keys apply.
    pub(crate) fn is_cell(&self, id: &str) -> bool {
        crate::doc::parent_of(&self.blocks, id).is_some_and(|p| p.content.table_row.is_some())
    }

    pub(crate) fn composing(&self) -> bool {
        self.active
            .as_ref()
            .and_then(|id| self.texts.get(id))
            .is_some_and(|t| t.composing())
    }

    // ───────────────────────────────────────────── where the keyboard is

    /// Follow Rune's focus: the block whose field has the keyboard holds the
    /// caret (leaving a block saves its typing), a ranged selection shows the
    /// marks toolbar, and a pending caret request lands once its block exists.
    pub(crate) fn poll(&mut self, s: &mut State) -> bool {
        let mut changed = false;
        if let Some((id, offset)) = self.focus_request.clone()
            && self.texts.contains_key(&id)
        {
            self.focus_request = None;
            self.focus_block(&id, offset, s);
            changed = true;
        }
        let focused = self
            .texts
            .iter()
            .find(|(_, t)| t.focused())
            .map(|(id, _)| id.clone());
        if focused != self.active {
            changed = true;
            if let Some(old) = self.active.take() {
                self.flush_save(&old, s);
            }
            match focused {
                Some(id) => {
                    self.selected.clear();
                    self.focus_self = false;
                    self.last_focused = id.clone();
                    self.active = Some(id);
                }
                None => {
                    // The editor lost the keyboard (the web's blur).
                    self.toolbar = None;
                    self.sync_texts();
                }
            }
        }
        let want = self.active.as_deref().and_then(|id| {
            let t = self.texts.get(id)?;
            let sel = t.sel();
            (!sel.is_empty() && !self.read_only && !self.is_cell(id)).then(|| Toolbar {
                block: id.to_string(),
                from: sel.start,
                to: sel.end,
            })
        });
        if want.is_none() {
            self.toolbar_dismissed = None;
        }
        if want != self.toolbar && (want.is_none() || want != self.toolbar_dismissed) {
            self.toolbar = want;
            changed = true;
        }
        if let Some(id) = self.active.clone()
            && let Some(slash) = &self.slash
            && slash.block == id
        {
            let before = (slash.filter.clone(), slash.start);
            self.update_slash(&id);
            changed |= self
                .slash
                .as_ref()
                .is_none_or(|sl| (sl.filter.clone(), sl.start) != before);
        }
        changed
    }

    // ───────────────────────────────────────────── keys in a field

    fn input_key(&mut self, id: &str, ev: &KeyEvent, s: &mut State, tx: &mut Tx) -> bool {
        if self.read_only {
            return false;
        }
        if self.active.as_deref() != Some(id) {
            self.active = Some(id.to_string());
        }
        let m = ev.mods;
        let plain = !m.shift && !m.cmd && !m.alt && !m.ctrl;
        if chord("mod+z").matches_tx(ev, tx) {
            self.undo_redo(false, s);
            return true;
        }
        if chord("mod+shift+z").matches_tx(ev, tx)
            || (!cfg!(target_os = "macos") && chord("ctrl+y").matches_tx(ev, tx))
        {
            self.undo_redo(true, s);
            return true;
        }
        for (spec, kind) in [
            ("mod+b", mark::BOLD),
            ("mod+i", mark::ITALIC),
            ("mod+u", mark::UNDERLINE),
            ("mod+shift+s", mark::STRIKETHROUGH),
            ("mod+e", mark::INLINE_CODE),
        ] {
            if chord(spec).matches_tx(ev, tx) {
                self.apply_mark(id, kind, None, s);
                return true;
            }
        }
        if self.composing() {
            return false;
        }
        match &ev.key {
            Key::Enter if plain => {
                if self.slash_owns_key() {
                    self.slash_confirm(s);
                } else {
                    self.structural(StructuralKey::Enter, s);
                }
                self.reveal(tx);
                true
            }
            Key::Tab if !m.cmd && !m.alt && !m.ctrl => {
                if !m.shift && self.slash_owns_key() {
                    self.slash_confirm(s);
                } else {
                    self.structural(StructuralKey::Tab { shift: m.shift }, s);
                }
                self.reveal(tx);
                true
            }
            Key::Backspace => {
                let Some(t) = self.texts.get(id) else {
                    return false;
                };
                if t.sel() == (0..0) && !self.is_cell(id) {
                    self.backspace_at_start(id, s);
                    self.reveal(tx);
                    return true;
                }
                false
            }
            Key::Escape => {
                if self.slash.is_some() {
                    self.slash = None;
                    return true;
                }
                if self.paste_menu.is_some() || self.spell_menu.is_some() {
                    self.paste_menu = None;
                    self.spell_menu = None;
                    return true;
                }
                self.toolbar_dismissed = self.toolbar.take();
                self.selected.clear();
                true
            }
            Key::Up | Key::Down if !m.cmd && !m.alt && !m.ctrl => {
                let down = ev.key == Key::Down;
                if !m.shift && self.slash_owns_key() {
                    if let Some(k) = self.slash_move(if down { 1 } else { -1 }) {
                        tx.scroll_into_view(k);
                    }
                    return true;
                }
                let Some(t) = self.texts.get(id) else {
                    return false;
                };
                let sel = t.sel();
                let at_edge = if down {
                    sel.end == t.text().len()
                } else {
                    sel.start == 0
                };
                if !(at_edge && !m.shift && !self.is_cell(id)) {
                    return false;
                }
                let flat = self.flat_text();
                let idx = flat.iter().position(|x| x == id).unwrap_or(0);
                let next = if down {
                    flat.get(idx + 1)
                } else {
                    idx.checked_sub(1).and_then(|i| flat.get(i))
                };
                if let Some(next) = next.cloned() {
                    let offset = if down {
                        0
                    } else {
                        self.texts.get(&next).map_or(0, |t| t.text().len())
                    };
                    self.focus_block(&next, offset, s);
                    tx.scroll_into_view(&row_key(&next));
                }
                true
            }
            _ => false,
        }
    }

    /// Scroll the block with the caret into view after a key moved it.
    fn reveal(&self, tx: &mut Tx) {
        if let Some((id, _)) = &self.focus_request {
            tx.scroll_into_view(&row_key(id));
        } else if let Some(id) = &self.active {
            tx.scroll_into_view(&row_key(id));
        }
    }

    /// Rune changed block `id`'s text (typing, IME commit, paste, cut, deletion).
    fn text_changed(&mut self, id: &str, s: &mut State) {
        if self.read_only {
            return;
        }
        let cell = self.is_cell(id);
        let Some(t) = self.texts.get_mut(id) else {
            return;
        };
        let Some(change) = t.sync() else { return };
        // A space after ---, —- or *** (the whole line) makes a divider.
        if change.inserted == " "
            && !cell
            && DIVIDERS.contains(&change.before.trim())
            && self.convert_to_divider(id, &change.before, s)
        {
            return;
        }
        // The slash menu opens only at the start of a line or after a space,
        // newline, "(", "[", '"' or "'" - never inside a word ("and/or").
        let start = change.removed.start;
        let opens = change.inserted == "/" && !cell && {
            let before = change.before[..start.min(change.before.len())]
                .chars()
                .next_back();
            self.style_of(id) != Some(style::CODE)
                && (start == 0 || matches!(before, Some(' ' | '\n' | '(' | '[' | '"' | '\'')))
        };
        self.on_input(id, s);
        if opens {
            self.slash = Some(Slash {
                block: id.to_string(),
                start,
                filter: String::new(),
                selected: 0,
            });
        }
    }

    /// After typing: slash filter, debounced save, spelling, lint.
    pub(crate) fn on_input(&mut self, id: &str, s: &mut State) {
        if self.composing() {
            return;
        }
        self.update_slash(id);
        self.toolbar = None;
        self.schedule_save(id, s);
        self.schedule_spell(s);
        self.schedule_lint(s);
    }

    /// Replace `range` of a block's live text from code: the edit path for menus and paste.
    pub(crate) fn edit(
        &mut self,
        id: &str,
        range: std::ops::Range<usize>,
        new: &str,
        s: &mut State,
    ) {
        let Some(t) = self.texts.get_mut(id) else {
            return;
        };
        t.replace(range, new);
        self.on_input(id, s);
    }

    // ───────────────────────────────────────────── structural keys

    /// Enter and Tab reshape the tree: while an async structural op runs, a
    /// key queues and replays (in order) on whatever line has the caret.
    fn structural(&mut self, key: StructuralKey, s: &mut State) {
        if self.structural_busy {
            self.structural_queue.push_back(key);
            return;
        }
        self.run_structural(key, s);
    }

    pub(crate) fn run_structural(&mut self, key: StructuralKey, s: &mut State) {
        let Some(id) = self.active.clone() else {
            self.structural_queue.clear();
            return;
        };
        if self.is_cell(&id) {
            self.cell_key(&id, key, s);
            return;
        }
        match key {
            StructuralKey::Enter => self.enter(&id, s),
            StructuralKey::Tab { shift } => self.tab(&id, shift, s),
        }
    }

    /// TableBlock cellKeydown: Enter commits, Tab / Shift-Tab walk cells row-major.
    fn cell_key(&mut self, id: &str, key: StructuralKey, s: &mut State) {
        match key {
            StructuralKey::Enter => {
                self.flush_save(id, s);
                if let Some(t) = self.texts.get_mut(id) {
                    t.edit.focused = false;
                }
                self.active = None;
                self.sync_texts();
            }
            StructuralKey::Tab { shift } => {
                let Some((rows, cols, r, c)) = self.cell_position(id) else {
                    return;
                };
                let flat = (r * cols.len() + c) as isize + if shift { -1 } else { 1 };
                if flat < 0 || flat as usize >= rows.len() * cols.len() {
                    return;
                }
                let (nr, nc) = (flat as usize / cols.len(), flat as usize % cols.len());
                let next = format!("{}-{}", rows[nr], cols[nc]);
                self.focus_block(&next, 0, s);
            }
        }
    }

    /// (row ids, column ids, row index, column index) of a table cell.
    pub(crate) fn cell_position(
        &self,
        cell: &str,
    ) -> Option<(Vec<String>, Vec<String>, usize, usize)> {
        let row = crate::doc::parent_of(&self.blocks, cell)?.id.clone();
        let rows_layout = crate::doc::parent_of(&self.blocks, &row)?;
        let table = crate::doc::parent_of(&self.blocks, &rows_layout.id)?;
        let (cols, rows) = self.table_shape(&table.id);
        let r = rows.iter().position(|x| *x == row)?;
        let c = cols.iter().position(|col| format!("{row}-{col}") == cell)?;
        Some((rows, cols, r, c))
    }

    /// Anytype markdown: a line of exactly ---, —- or *** becomes a divider
    /// (*** gives dots), and a fresh paragraph takes the caret.
    fn convert_to_divider(&mut self, id: &str, text: &str, s: &mut State) -> bool {
        if !DIVIDERS.contains(&text.trim()) || self.style_of(id) == Some(style::CODE) {
            return false;
        }
        let kind = if text.trim() == "***" { "dots" } else { "line" };
        let new = new_id();
        self.cancel_pending(id);
        self.commit(
            vec![
                Op::Content {
                    id: id.to_string(),
                    content: crate::editor::custom_content("divider", &[("style", kind)]),
                },
                Op::Insert {
                    block: text_block(&new, "", style::PARAGRAPH),
                    at: place(id, pos::BOTTOM),
                },
            ],
            true,
            s,
        );
        self.focus_block(&new, 0, s);
        true
    }

    fn enter(&mut self, id: &str, s: &mut State) {
        // Enter inside a code block inserts a newline, never splits.
        if self.style_of(id) == Some(style::CODE) {
            if let Some(t) = self.texts.get_mut(id) {
                let sel = t.sel();
                t.edit.set_selection(sel.start, sel.end);
                t.edit.insert("\n");
            }
            self.text_changed(id, s);
            return;
        }
        if self.block(id).is_none() {
            log::warn!(
                "[editor] block {} vanished from state before the split; re-reading",
                &id[..id.len().min(8)]
            );
            self.refresh(s).detach();
            return;
        }
        let at = self.texts.get(id).map_or(0, |t| t.sel().start);
        let (text, spans) = self.live(id);
        let at = at.min(text.len());
        if self.convert_to_divider(id, &text, s) {
            return;
        }
        let cur_style = self.style_of(id).unwrap_or(style::PARAGRAPH);
        let is_list = LIST.contains(&cur_style);
        let is_quote = cur_style == style::QUOTE;
        let is_callout = cur_style == style::CALLOUT;
        let is_header = matches!(cur_style, style::HEADER1 | style::HEADER2 | style::HEADER3);
        let can_toggle = cur_style == style::TOGGLE;
        let is_open = can_toggle && toggles::is_toggle_open(&self.object_id, id, &s.ui);
        let kids = self
            .block(id)
            .map(|b| b.children_ids.clone())
            .unwrap_or_default();
        let at_end = at == text.len();
        let parent_id = self.parent_id_of(id);
        let parent_style = self.style_of(&parent_id);

        // Enter on an EMPTY list/quote/callout item: nested under a list item
        // it outdents; anywhere else it turns into a paragraph in place.
        if text.is_empty() && (is_list || is_quote || is_callout) {
            if parent_style.is_some_and(|ps| LIST.contains(&ps))
                && self.outdent_block(id, 0, "", &[], s)
            {
                return;
            }
            let color = self
                .text_of(id)
                .and_then(|t| t.color.clone())
                .unwrap_or_default();
            self.cancel_pending(id);
            self.commit(
                vec![Op::Content {
                    id: id.to_string(),
                    content: text_content("", &[], style::PARAGRAPH, false, color),
                }],
                true,
                s,
            );
            self.focus_block(id, 0, s);
            return;
        }

        // A closed toggle never splits: at the end it takes a paragraph below,
        // at the start (with children) one above. A callout at the end too.
        if (can_toggle && !is_open && at_end) || (is_callout && at_end) {
            self.add_sibling(id, pos::BOTTOM, style::PARAGRAPH, s);
            return;
        }
        if can_toggle && !is_open && !kids.is_empty() && at == 0 {
            self.add_sibling(id, pos::TOP, style::PARAGRAPH, s);
            return;
        }

        // An empty paragraph that is the LAST line inside a toggle/callout/quote escapes its parent.
        if cur_style == style::PARAGRAPH
            && text.is_empty()
            && let Some(ps) = parent_style
        {
            let nested = matches!(ps, style::TOGGLE | style::CALLOUT | style::QUOTE);
            let siblings = self
                .block(&parent_id)
                .map(|b| b.children_ids.clone())
                .unwrap_or_default();
            if nested
                && siblings.last().map(String::as_str) == Some(id)
                && self.outdent_block(id, 0, "", &[], s)
            {
                return;
            }
        }

        // blockSplit (Anytype editor/page.tsx:2872).
        let mut new_style = style::PARAGRAPH;
        let mut mode = pos::BOTTOM;
        if is_list || (!is_header && !at_end) {
            // A list continues at any caret but offset 0, where only a checkbox continues.
            new_style = if at > 0 || cur_style == style::CHECKBOX {
                cur_style
            } else {
                style::PARAGRAPH
            };
            mode = if at > 0 { pos::BOTTOM } else { pos::TOP };
        }
        if is_header {
            new_style = style::PARAGRAPH;
            mode = if at > 0 { pos::BOTTOM } else { pos::TOP };
        }
        if is_open {
            new_style = style::PARAGRAPH;
            mode = pos::INNER_FIRST;
        }
        // A block with visible children splits upward: the head moves into a
        // new block above, this one keeps the tail, its children and the caret.
        if !can_toggle && !kids.is_empty() {
            mode = pos::TOP;
        }
        if (is_callout || is_quote) && !at_end {
            new_style = cur_style;
        }

        let new = new_id();
        self.cancel_pending(id);
        let head = text[..at].to_string();
        let tail = text[at..].to_string();
        let head_marks = marks::head(&spans, at);
        let tail_marks = marks::tail(&spans, at);
        let cur = self.text_of(id).cloned().unwrap_or_default();
        let checked = cur.checked.unwrap_or(false);
        let color = cur.color.clone().unwrap_or_default();
        let piece = |text: &str, spans: &[Span], st: u32| TextContent {
            text: text.to_string(),
            style: st,
            marks: Some(marks::marks_from_spans(text, spans)),
            checked: Some(!text.is_empty() && checked),
            color: None,
        };

        if mode == pos::TOP {
            let above = roostr_core::types::Block {
                id: new.clone(),
                content: roostr_core::types::BlockContent {
                    text: Some(piece(&head, &head_marks, new_style)),
                    ..Default::default()
                },
                ..Default::default()
            };
            self.commit(
                vec![
                    Op::Insert {
                        block: above,
                        at: place(id, pos::TOP),
                    },
                    Op::Content {
                        id: id.to_string(),
                        content: text_content(
                            &tail,
                            &tail_marks,
                            cur_style,
                            !tail.is_empty() && checked,
                            color,
                        ),
                    },
                ],
                true,
                s,
            );
            self.set_live(id, tail, tail_marks);
            self.focus_block(id, 0, s);
            return;
        }

        let born = roostr_core::types::Block {
            id: new.clone(),
            content: roostr_core::types::BlockContent {
                text: Some(piece(&tail, &tail_marks, new_style)),
                ..Default::default()
            },
            ..Default::default()
        };
        self.commit(
            vec![
                Op::Content {
                    id: id.to_string(),
                    content: text_content(
                        &head,
                        &head_marks,
                        cur_style,
                        !head.is_empty() && checked,
                        color,
                    ),
                },
                Op::Insert {
                    block: born,
                    at: place(
                        id,
                        if mode == pos::INNER_FIRST {
                            pos::INNER_FIRST
                        } else {
                            pos::BOTTOM
                        },
                    ),
                },
            ],
            true,
            s,
        );
        self.set_live(id, head, head_marks);
        self.focus_block(&new, 0, s);
    }

    /// Anytype onTabBlock shift: the block becomes the sibling right below its
    /// parent, keeping its style and caret. Also Enter on an empty nested list item.
    pub(crate) fn outdent_block(
        &mut self,
        id: &str,
        caret: usize,
        live_text: &str,
        live_spans: &[Span],
        s: &mut State,
    ) -> bool {
        let parent = self.parent_id_of(id);
        if parent.is_empty() || parent == "__content__" {
            return false;
        }
        let Some(cur) = self.block(id).cloned() else {
            return false;
        };
        self.cancel_pending(id);
        let mut ops = vec![Op::Move {
            id: id.to_string(),
            to: place(&parent, pos::BOTTOM),
        }];
        if cur.content.text.is_some() {
            ops.insert(
                0,
                Op::Content {
                    id: id.to_string(),
                    content: self.content_for(id, live_text, live_spans),
                },
            );
        }
        self.commit(ops, true, s);
        self.set_live(id, live_text.to_string(), live_spans.to_vec());
        self.focus_block(id, caret, s);
        true
    }

    /// Anytype blockCreate: an empty text block above/below, caret in it.
    pub(crate) fn add_sibling(&mut self, id: &str, position: u32, st: u32, s: &mut State) {
        let new = new_id();
        self.cancel_pending(id);
        self.commit(
            vec![Op::Insert {
                block: text_block(&new, "", st),
                at: place(id, position),
            }],
            true,
            s,
        );
        self.focus_block(&new, 0, s);
    }

    fn tab(&mut self, id: &str, shift: bool, s: &mut State) {
        let at = self.texts.get(id).map_or(0, |t| t.sel().start);
        // A move carries no text: capture the live text into state + the write.
        let (text, spans) = self.live(id);
        if shift {
            self.outdent_block(id, at, &text, &spans, s);
            return;
        }
        let parent = self.parent_id_of(id);
        let siblings = if parent.is_empty() {
            self.root_ids()
        } else {
            self.block(&parent)
                .map(|b| b.children_ids.clone())
                .unwrap_or_default()
        };
        let Some(idx) = siblings.iter().position(|x| x == id) else {
            return;
        };
        if idx == 0 {
            return;
        }
        let prev = siblings[idx - 1].clone();
        self.cancel_pending(id);
        // Tucking under a closed toggle would make the block vanish.
        if self.style_of(&prev) == Some(style::TOGGLE) {
            toggles::set_toggle_open(&self.object_id, &prev, true, &mut s.ui);
        }
        let content = self.content_for(id, &text, &spans);
        self.commit(
            vec![
                Op::Content {
                    id: id.to_string(),
                    content,
                },
                Op::Move {
                    id: id.to_string(),
                    to: place(&prev, pos::INNER),
                },
            ],
            true,
            s,
        );
        self.set_live(id, text, spans);
        self.focus_block(id, at, s);
    }

    // ───────────────────────────────────────────── deletion

    /// Backspace at offset 0: first demote the style, then merge into the previous text block.
    fn backspace_at_start(&mut self, id: &str, s: &mut State) {
        let Some(cur) = self.text_of(id).cloned() else {
            return;
        };
        if cur.style != style::PARAGRAPH {
            let (text, spans) = self.live(id);
            self.cancel_pending(id);
            self.commit(
                vec![Op::Content {
                    id: id.to_string(),
                    content: text_content(
                        &text,
                        &spans,
                        style::PARAGRAPH,
                        false,
                        cur.color.clone().unwrap_or_default(),
                    ),
                }],
                true,
                s,
            );
            self.focus_block(id, 0, s);
            return;
        }
        let flat = self.flat_text();
        let Some(idx) = flat.iter().position(|x| x == id) else {
            return;
        };
        if idx == 0 {
            return;
        }
        let prev_id = flat[idx - 1].clone();
        let (prev_text, prev_spans) = self.live(&prev_id);
        let (text, spans) = self.live(id);
        let shift = prev_text.len();
        let mut merged_spans = prev_spans;
        merged_spans.extend(marks::shifted(&spans, shift));
        let merged_spans = marks::merge_adjacent(merged_spans);
        let merged = format!("{prev_text}{text}");
        self.cancel_pending(id);
        self.cancel_pending(&prev_id);
        let content = self.content_for(&prev_id, &merged, &merged_spans);
        self.commit(
            vec![
                Op::Content {
                    id: prev_id.clone(),
                    content,
                },
                Op::Remove { id: id.to_string() },
            ],
            true,
            s,
        );
        self.set_live(&prev_id, merged, merged_spans);
        self.focus_block(&prev_id, shift, s);
    }

    /// Delete the block selection (Backspace/Delete with blocks selected).
    pub(crate) fn delete_selection_blocks(&mut self, s: &mut State) {
        if self.selected.is_empty() {
            return;
        }
        let tops = self.topmost_selected();
        self.selected.clear();
        for id in &tops {
            self.cancel_pending(id);
        }
        self.commit(
            tops.into_iter().map(|id| Op::Remove { id }).collect(),
            true,
            s,
        );
    }

    // ───────────────────────────────────────────── marks

    /// Toggle a mark over the toolbar's (or the live) selection.
    pub(crate) fn apply_mark(&mut self, id: &str, kind: u32, param: Option<String>, s: &mut State) {
        let range = match &self.toolbar {
            Some(tb) if tb.block == id => tb.from..tb.to,
            _ => match self.texts.get(id) {
                Some(t) => t.sel(),
                None => return,
            },
        };
        if range.is_empty() {
            return;
        }
        let (text, spans) = self.live(id);
        let next = marks::toggle_mark(&spans, range.start, range.end, kind, param);
        self.cancel_pending(id);
        let content = self.content_for(id, &text, &next);
        self.commit(
            vec![Op::Content {
                id: id.to_string(),
                content,
            }],
            true,
            s,
        );
        self.set_live(id, text, next);
        self.toolbar = None;
        self.focus_block(id, range.end, s);
    }

    // ───────────────────────────────────────────── slash

    pub(crate) fn slash_owns_key(&self) -> bool {
        matches!((&self.slash, &self.active), (Some(sl), Some(a)) if sl.block == *a)
    }

    /// Track the filter typed after "/"; close if the slash was deleted.
    pub(crate) fn update_slash(&mut self, id: &str) {
        let Some(slash) = &self.slash else { return };
        if slash.block != id {
            return;
        }
        let Some(t) = self.texts.get(id) else {
            self.slash = None;
            return;
        };
        if t.text().get(slash.start..slash.start + 1) != Some("/") {
            self.slash = None;
            return;
        }
        let end = t.sel().end;
        if end <= slash.start {
            self.slash = None;
            return;
        }
        let filter = t.text().get(slash.start + 1..end).unwrap_or("").to_string();
        if filter == slash.filter {
            return;
        }
        let count = self.slash_entries(&filter).len();
        self.slash_set_filter(filter);
        // Anytype auto-closes once the filter clearly matches nothing.
        if count == 0
            && self
                .slash
                .as_ref()
                .is_some_and(|sl| sl.filter.chars().count() > 3)
        {
            self.slash = None;
        }
    }

    // ───────────────────────────────────────────── block selection

    /// The selectable row for a block: a table cell selects as its table.
    pub(crate) fn selection_row_of(&self, id: &str) -> String {
        if !self.is_cell(id) {
            return id.to_string();
        }
        let mut cur = id.to_string();
        while let Some(p) = crate::doc::parent_of(&self.blocks, &cur) {
            if p.content.table.is_some() {
                return p.id.clone();
            }
            cur = p.id.clone();
        }
        id.to_string()
    }

    pub(crate) fn toggle_select(&mut self, id: &str, s: &mut State) {
        if let Some(i) = self.selected.iter().position(|x| x == id) {
            self.selected.remove(i);
        } else {
            self.selected.push(id.to_string());
        }
        self.deactivate_for_selection(s);
    }

    /// Anytype shift-click: the contiguous tree-order range from the anchor.
    pub(crate) fn range_select(&mut self, id: &str, s: &mut State) {
        let anchor = self
            .selected
            .first()
            .cloned()
            .unwrap_or_else(|| self.last_focused.clone());
        let flat = self.flat_ids();
        let a = flat.iter().position(|x| *x == anchor);
        let b = flat.iter().position(|x| x == id);
        match (a, b) {
            (Some(a), Some(b)) if a != b => self.selected = flat[a.min(b)..=a.max(b)].to_vec(),
            _ => self.selected = vec![id.to_string()],
        }
        self.deactivate_for_selection(s);
    }

    /// Selecting blocks clears text focus (the focused block would eat keys).
    pub(crate) fn deactivate_for_selection(&mut self, s: &mut State) {
        if self.selected.is_empty() {
            return;
        }
        self.toolbar = None;
        self.slash = None;
        self.blur_texts(s);
    }

    /// A press on a row outside its text, or on the page background: modifiers select, else a
    /// rubber band arms (`row` None: the background).
    pub(crate) fn arm_drag(
        &mut self,
        row: Option<&str>,
        at: (f32, f32),
        mods: rune::Mods,
        s: &mut State,
    ) {
        if self.read_only {
            return;
        }
        self.spell_menu = None;
        self.paste_menu = None;
        if let Some(id) = row
            && (mods.shift || mods.cmd || mods.ctrl)
        {
            let row = self.selection_row_of(id);
            if mods.shift {
                self.range_select(&row, s);
            } else {
                self.toggle_select(&row, s);
            }
            return;
        }
        if let Some(a) = self.active.take() {
            self.flush_save(&a, s);
            if let Some(t) = self.texts.get_mut(&a) {
                t.edit.focused = false;
            }
            self.sync_texts();
        }
        self.toolbar = None;
        self.drag_sel = Some(DragSel {
            anchor: at,
            base: self.selected.clone(),
            mode: if mods.cmd || mods.ctrl {
                SelMode::Toggle
            } else if mods.alt {
                SelMode::Remove
            } else {
                SelMode::Replace
            },
            moved: false,
            background: row.is_none(),
            last: at,
        });
    }

    /// The rubber band moved to `cursor` (window points): hit-test it against block rows
    /// (Anytype selection provider).
    pub(crate) fn sel_update(&mut self, cursor: (f32, f32), s: &mut State) {
        let Some(drag) = self.drag_sel.as_ref() else {
            return;
        };
        let anchor = drag.anchor;
        let w = (cursor.0 - anchor.0).abs();
        let h = (cursor.1 - anchor.1).abs();
        // Ignore only when BOTH axes are tiny, so a thin drag across one line still selects.
        if !drag.moved && w < 20. && h < 20. {
            return;
        }
        let rect = [anchor.0.min(cursor.0), anchor.1.min(cursor.1), w, h];
        let mut hits = Vec::new();
        for id in self.flat_ids() {
            if self.is_cell(&id) {
                continue;
            }
            let Some(geom) = self.rows.get(&id) else {
                continue;
            };
            let [x, y, rw, rh] = geom.bounds;
            if x < rect[0] + rect[2]
                && x + rw > rect[0]
                && y < rect[1] + rect[3]
                && y + rh > rect[1]
            {
                hits.push(id);
            }
        }
        let drag = self.drag_sel.as_mut().expect("drag");
        drag.moved = true;
        drag.last = cursor;
        self.sel_rect = Some(rect);
        let mode = drag.mode;
        let base = drag.base.clone();
        self.selected = match mode {
            SelMode::Toggle => {
                let base_set: HashSet<&String> = base.iter().collect();
                base.iter()
                    .filter(|i| !hits.contains(i))
                    .cloned()
                    .chain(hits.iter().filter(|i| !base_set.contains(i)).cloned())
                    .collect()
            }
            SelMode::Remove => base.into_iter().filter(|i| !hits.contains(i)).collect(),
            SelMode::Replace => hits,
        };
        self.deactivate_for_selection(s);
        self.toolbar = None;
    }

    /// The rubber band was released at `at` (window points).
    pub(crate) fn drag_release(&mut self, at: (f32, f32), s: &mut State) {
        if let Some(drag) = self.drag_sel.take() {
            if !drag.moved && drag.mode == SelMode::Replace {
                self.selected.clear();
            }
            if !drag.moved && drag.background && !self.read_only {
                self.background_click(at.1, s);
            }
        }
        self.sel_rect = None;
    }

    /// A click on the page's own background or beside a row: beside the
    /// lines the caret goes to the end of the nearest one; below them to the
    /// last line when it is an empty paragraph, else a new line is added.
    pub(crate) fn background_click(&mut self, y: f32, s: &mut State) {
        let roots: Vec<String> = self
            .root_ids()
            .into_iter()
            .filter(|r| self.texts.contains_key(r))
            .collect();
        if roots.is_empty() {
            self.append_block(s);
            return;
        }
        let last_bottom = roots
            .last()
            .and_then(|r| self.rows.get(r))
            .map(|g| g.bounds[1] + g.bounds[3]);
        if let Some(bottom) = last_bottom
            && y < bottom
        {
            let mut best = roots[0].clone();
            let mut gap = f32::INFINITY;
            for r in &roots {
                let Some(g) = self.rows.get(r) else { continue };
                let (top, bot) = (g.bounds[1], g.bounds[1] + g.bounds[3]);
                let d = if y < top {
                    top - y
                } else if y > bot {
                    y - bot
                } else {
                    0.
                };
                if d < gap {
                    best = r.clone();
                    gap = d;
                }
            }
            let len = self.texts.get(&best).map_or(0, |t| t.text().len());
            self.focus_block(&best, len, s);
            return;
        }
        let all_roots = self.root_ids();
        let Some(last) = all_roots.last().cloned() else {
            return;
        };
        if let Some(t) = self.text_of(&last)
            && t.text.is_empty()
            && t.style == style::PARAGRAPH
        {
            self.focus_block(&last, 0, s);
            return;
        }
        self.add_sibling(&last, pos::BOTTOM, style::PARAGRAPH, s);
    }

    pub(crate) fn append_block(&mut self, s: &mut State) {
        let new = new_id();
        let add = self.w_block_add(text_block(&new, "", style::PARAGRAPH), "", pos::NONE, s);
        self.focus_request = Some((new, 0));
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let _ = add.await;
            crate::editor::update_editor(&cx, &oid, |e, s| e.refresh(s).detach());
        })
        .detach();
    }

    // ───────────────────────────────────────────── keys with blocks selected

    /// Backspace/Delete remove the selected blocks, Escape clears them,
    /// ⌘C/⌘X/⌘V copy, cut and paste them, ⌘Z/⇧⌘Z undo and redo.
    fn selection_key(&mut self, ev: &KeyEvent, s: &mut State, tx: &mut Tx) -> bool {
        if self.read_only || self.editing().is_some() {
            return false;
        }
        if chord("mod+z").matches_tx(ev, tx) {
            self.undo_redo(false, s);
            return true;
        }
        if chord("mod+shift+z").matches_tx(ev, tx)
            || (!cfg!(target_os = "macos") && chord("ctrl+y").matches_tx(ev, tx))
        {
            self.undo_redo(true, s);
            return true;
        }
        if chord("mod+c").matches_tx(ev, tx) {
            self.copy_blocks(false, s, tx);
            return true;
        }
        if chord("mod+x").matches_tx(ev, tx) {
            self.copy_blocks(true, s, tx);
            return true;
        }
        if chord("mod+v").matches_tx(ev, tx) {
            let oid = self.object_id.clone();
            tx.read_clipboard(move |s: &mut State, content, _tx| {
                with_editor(s, &oid, |e, s| e.paste_after_selection(content, s));
            });
            return true;
        }
        match ev.key {
            Key::Backspace | Key::Delete if !self.selected.is_empty() => {
                self.delete_selection_blocks(s);
                true
            }
            Key::Escape => {
                self.slash = None;
                self.paste_menu = None;
                self.spell_menu = None;
                self.toolbar = None;
                self.selected.clear();
                true
            }
            _ => false,
        }
    }
}

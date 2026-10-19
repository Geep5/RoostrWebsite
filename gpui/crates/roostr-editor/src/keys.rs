//! Keyboard, text input and mouse-in-text handling (`Editor.svelte`
//! onKeydown / onBeforeInput / onSelect / selection provider).

use std::collections::HashSet;
use std::ops::Range;

use gpui_kit::*;
use roostr_core::types::{TextContent, mark, pos, style};

use crate::doc::Op;
use crate::editor::{
    DragSel, Editor, SelMode, Slash, StructuralKey, Toolbar, new_id, place, text_block,
    text_content,
};
use crate::marks::{self, Span};
use crate::toggles;
use crate::{
    Backspace, Bold, CharacterPalette, Delete, DeleteLineLeft, DeleteLineRight, DeleteWordLeft,
    DeleteWordRight, DocEnd, DocStart, Down, Enter, Escape, InlineCode, Italic, Left, LineEnd,
    LineStart, Redo, Right, SelectAll, SelectDocEnd, SelectDocStart, SelectDown, SelectLeft,
    SelectLineEnd, SelectLineStart, SelectRight, SelectUp, SelectWordLeft, SelectWordRight,
    ShiftEnter, ShiftTab, Tab, Underline, Undo, Up, WordLeft, WordRight,
};

const LIST: [u32; 4] = [
    style::BULLET,
    style::NUMBERED,
    style::CHECKBOX,
    style::TOGGLE,
];
const DIVIDERS: [&str; 3] = ["---", "—-", "***"];

impl Editor {
    /// The editor itself (not a menu's input inside it) has keyboard focus.
    fn keys_here(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    pub(crate) fn composing(&self) -> bool {
        self.active
            .as_ref()
            .and_then(|id| self.texts.get(id))
            .is_some_and(|t| t.marked.is_some())
    }

    /// A table cell (a text block under a table row): its own keys apply.
    pub(crate) fn is_cell(&self, id: &str) -> bool {
        crate::doc::parent_of(&self.blocks, id).is_some_and(|p| p.content.table_row.is_some())
    }

    /// Run `f` for a key the editor owns; anything else keeps propagating.
    fn with_key(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>),
    ) {
        if !self.keys_here(window) || self.read_only {
            cx.propagate();
            return;
        }
        f(self, window, cx);
        self.caret_on = true;
        cx.notify();
    }

    // ───────────────────────────────────────────── structural keys

    pub(crate) fn on_enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        self.with_key(window, cx, |this, window, cx| {
            if this.slash_owns_key() {
                this.slash_confirm(window, cx);
                return;
            }
            if this.active.is_none() || this.composing() {
                return;
            }
            this.structural(StructuralKey::Enter, cx);
        });
    }

    pub(crate) fn on_shift_enter(
        &mut self,
        _: &ShiftEnter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_key(window, cx, |this, _, cx| {
            if this.active.is_some() && !this.composing() {
                this.insert_text(None, "\n", cx);
            }
        });
    }

    pub(crate) fn on_tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.with_key(window, cx, |this, window, cx| {
            if this.slash_owns_key() {
                this.slash_confirm(window, cx);
                return;
            }
            if this.active.is_some() && !this.composing() {
                this.structural(StructuralKey::Tab { shift: false }, cx);
            }
        });
    }

    pub(crate) fn on_shift_tab(
        &mut self,
        _: &ShiftTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_key(window, cx, |this, _, cx| {
            if this.active.is_some() && !this.composing() {
                this.structural(StructuralKey::Tab { shift: true }, cx);
            }
        });
    }

    /// Enter and Tab reshape the tree: while an async structural op runs, a
    /// key queues and replays (in order) on whatever line has the caret.
    fn structural(&mut self, key: StructuralKey, cx: &mut Context<Self>) {
        if self.structural_busy {
            self.structural_queue.push_back(key);
            return;
        }
        self.run_structural(key, cx);
    }

    pub(crate) fn run_structural(&mut self, key: StructuralKey, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            self.structural_queue.clear();
            return;
        };
        if self.is_cell(&id) {
            self.cell_key(&id, key, cx);
            return;
        }
        match key {
            StructuralKey::Enter => self.enter(&id, cx),
            StructuralKey::Tab { shift } => self.tab(&id, shift, cx),
        }
    }

    /// TableBlock cellKeydown: Enter commits, Tab / Shift-Tab walk cells row-major.
    fn cell_key(&mut self, id: &str, key: StructuralKey, cx: &mut Context<Self>) {
        match key {
            StructuralKey::Enter => {
                self.flush_save(id, cx);
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
                self.focus_block(&next, 0, cx);
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
    fn convert_to_divider(&mut self, id: &str, text: &str, cx: &mut Context<Self>) -> bool {
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
            cx,
        );
        self.focus_block(&new, 0, cx);
        true
    }

    fn enter(&mut self, id: &str, cx: &mut Context<Self>) {
        // Enter inside a code block inserts a newline, never splits.
        if self.style_of(id) == Some(style::CODE) {
            self.insert_text(None, "\n", cx);
            return;
        }
        if self.block(id).is_none() {
            log::warn!(
                "[editor] block {} vanished from state before the split; re-reading",
                &id[..id.len().min(8)]
            );
            self.refresh(cx).detach();
            return;
        }
        let at = self.texts.get(id).map_or(0, |t| t.sel.start);
        let (text, spans) = self.live(id);
        let at = at.min(text.len());
        if self.convert_to_divider(id, &text, cx) {
            return;
        }
        let cur_style = self.style_of(id).unwrap_or(style::PARAGRAPH);
        let is_list = LIST.contains(&cur_style);
        let is_quote = cur_style == style::QUOTE;
        let is_callout = cur_style == style::CALLOUT;
        let is_header = matches!(cur_style, style::HEADER1 | style::HEADER2 | style::HEADER3);
        let can_toggle = cur_style == style::TOGGLE;
        let is_open = can_toggle && toggles::is_toggle_open(&self.object_id, id, cx);
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
            if parent_style.is_some_and(|s| LIST.contains(&s))
                && self.outdent_block(id, 0, "", &[], cx)
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
                cx,
            );
            self.focus_block(id, 0, cx);
            return;
        }

        // A closed toggle never splits: at the end it takes a paragraph below,
        // at the start (with children) one above. A callout at the end too.
        if (can_toggle && !is_open && at_end) || (is_callout && at_end) {
            self.add_sibling(id, pos::BOTTOM, style::PARAGRAPH, cx);
            return;
        }
        if can_toggle && !is_open && !kids.is_empty() && at == 0 {
            self.add_sibling(id, pos::TOP, style::PARAGRAPH, cx);
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
                && self.outdent_block(id, 0, "", &[], cx)
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
                cx,
            );
            self.set_live(id, tail, tail_marks);
            self.focus_block(id, 0, cx);
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
            cx,
        );
        self.set_live(id, head, head_marks);
        self.focus_block(&new, 0, cx);
    }

    /// Anytype onTabBlock shift: the block becomes the sibling right below its
    /// parent, keeping its style and caret. Also Enter on an empty nested list item.
    pub(crate) fn outdent_block(
        &mut self,
        id: &str,
        caret: usize,
        live_text: &str,
        live_spans: &[Span],
        cx: &mut Context<Self>,
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
        self.commit(ops, true, cx);
        self.set_live(id, live_text.to_string(), live_spans.to_vec());
        self.focus_block(id, caret, cx);
        true
    }

    /// Anytype blockCreate: an empty text block above/below, caret in it.
    pub(crate) fn add_sibling(&mut self, id: &str, position: u32, st: u32, cx: &mut Context<Self>) {
        let new = new_id();
        self.cancel_pending(id);
        self.commit(
            vec![Op::Insert {
                block: text_block(&new, "", st),
                at: place(id, position),
            }],
            true,
            cx,
        );
        self.focus_block(&new, 0, cx);
    }

    fn tab(&mut self, id: &str, shift: bool, cx: &mut Context<Self>) {
        let at = self.texts.get(id).map_or(0, |t| t.sel.start);
        // A move carries no text: capture the live text into state + the write.
        let (text, spans) = self.live(id);
        if shift {
            self.outdent_block(id, at, &text, &spans, cx);
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
        let Some(idx) = siblings.iter().position(|s| s == id) else {
            return;
        };
        if idx == 0 {
            return;
        }
        let prev = siblings[idx - 1].clone();
        self.cancel_pending(id);
        // Tucking under a closed toggle would make the block vanish.
        if self.style_of(&prev) == Some(style::TOGGLE) {
            toggles::set_toggle_open(&self.object_id, &prev, true, cx);
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
            cx,
        );
        self.set_live(id, text, spans);
        self.focus_block(id, at, cx);
    }

    // ───────────────────────────────────────────── deletion

    pub(crate) fn on_backspace(
        &mut self,
        _: &Backspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_key(window, cx, |this, _, cx| {
            if this.active.is_none() {
                this.delete_selection_blocks(cx);
                return;
            }
            let id = this.active.clone().unwrap_or_default();
            let Some(t) = this.texts.get(&id) else { return };
            if t.sel == (0..0) && !this.is_cell(&id) {
                this.backspace_at_start(&id, cx);
                return;
            }
            let range = if t.sel.is_empty() {
                t.prev_boundary(t.sel.start)..t.sel.start
            } else {
                t.sel.clone()
            };
            this.edit(&id, range, "", cx);
        });
    }

    pub(crate) fn on_delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.with_key(window, cx, |this, _, cx| {
            if this.active.is_none() {
                this.delete_selection_blocks(cx);
                return;
            }
            let id = this.active.clone().unwrap_or_default();
            let Some(t) = this.texts.get(&id) else { return };
            let range = if t.sel.is_empty() {
                t.sel.start..t.next_boundary(t.sel.start)
            } else {
                t.sel.clone()
            };
            this.edit(&id, range, "", cx);
        });
    }

    pub(crate) fn on_delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_by(window, cx, |t| t.prev_word(t.sel.start)..t.sel.end, true);
    }

    pub(crate) fn on_delete_line_left(
        &mut self,
        _: &DeleteLineLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_by(
            window,
            cx,
            |t| t.text[..t.sel.start].rfind('\n').map_or(0, |i| i + 1)..t.sel.end,
            true,
        );
    }

    pub(crate) fn on_delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_by(window, cx, |t| t.sel.start..t.next_word(t.sel.end), false);
    }

    pub(crate) fn on_delete_line_right(
        &mut self,
        _: &DeleteLineRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_by(
            window,
            cx,
            |t| {
                t.sel.start
                    ..t.text[t.sel.end..]
                        .find('\n')
                        .map_or(t.text.len(), |i| t.sel.end + i)
            },
            false,
        );
    }

    fn delete_by(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        range: impl FnOnce(&crate::text::TextState) -> Range<usize>,
        backward: bool,
    ) {
        self.with_key(window, cx, |this, _, cx| {
            let Some(id) = this.active.clone() else {
                this.delete_selection_blocks(cx);
                return;
            };
            let Some(t) = this.texts.get(&id) else { return };
            if backward && t.sel == (0..0) && !this.is_cell(&id) {
                this.backspace_at_start(&id, cx);
                return;
            }
            let r = if t.sel.is_empty() {
                range(t)
            } else {
                t.sel.clone()
            };
            this.edit(&id, r, "", cx);
        });
    }

    /// Backspace at offset 0: first demote the style, then merge into the previous text block.
    fn backspace_at_start(&mut self, id: &str, cx: &mut Context<Self>) {
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
                cx,
            );
            self.focus_block(id, 0, cx);
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
            cx,
        );
        self.set_live(&prev_id, merged, merged_spans);
        self.focus_block(&prev_id, shift, cx);
    }

    /// Delete the block selection (Backspace/Delete with blocks selected).
    pub(crate) fn delete_selection_blocks(&mut self, cx: &mut Context<Self>) {
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
            cx,
        );
    }

    /// Replace `range` of a block's live text: the one typing path.
    pub(crate) fn edit(
        &mut self,
        id: &str,
        range: Range<usize>,
        new: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.texts.get_mut(id) else {
            return;
        };
        t.replace(range, new);
        t.marked = None;
        self.on_input(id, cx);
    }

    /// After typing: slash filter, debounced save, spelling, lint.
    pub(crate) fn on_input(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.composing() {
            return;
        }
        self.update_slash(id);
        self.toolbar = None;
        self.schedule_save(id, cx);
        self.schedule_spell(cx);
        self.schedule_lint(cx);
        self.restart_blink(cx);
        cx.notify();
    }

    /// Insert typed text at the selection (or `range`): "/" may open the
    /// slash menu, a space after ---/***/—- makes a divider.
    pub(crate) fn insert_text(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let cell = self.is_cell(&id);
        let Some(t) = self.texts.get(&id) else { return };
        let range = range_utf16
            .map(|r| t.range_from_utf16(&r))
            .or(t.marked.clone())
            .unwrap_or(t.sel.clone());
        let composing = t.marked.is_some();
        let text = t.text.clone();
        if new == " "
            && !composing
            && !cell
            && DIVIDERS.contains(&text.trim())
            && self.convert_to_divider(&id, &text, cx)
        {
            return;
        }
        // The slash menu opens only at the start of a line or after a space,
        // newline, "(", "[", '"' or "'" - never inside a word ("and/or").
        let opens = new == "/" && !composing && !cell && !self.read_only && {
            let start = range.start.min(text.len());
            let before = text[..start].chars().next_back();
            self.style_of(&id) != Some(style::CODE)
                && (start == 0 || matches!(before, Some(' ' | '\n' | '(' | '[' | '"' | '\'')))
        };
        let start = range.start;
        self.edit(&id, range, new, cx);
        if opens {
            self.slash = Some(Slash {
                block: id,
                start,
                filter: String::new(),
                pos: None,
                sel: 0,
            });
        }
    }

    // ───────────────────────────────────────────── caret movement

    fn move_caret(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        select: bool,
        to: impl FnOnce(&Self, &str, &crate::text::TextState) -> usize,
    ) {
        self.with_key(window, cx, |this, _, cx| {
            let Some(id) = this.active.clone() else {
                return;
            };
            let Some(t) = this.texts.get(&id) else { return };
            let target = to(this, &id, t);
            let t = this.texts.get_mut(&id).expect("active text");
            if select {
                t.select_to(target);
            } else {
                t.move_to(target);
            }
            this.on_selection_changed(&id, cx);
        });
    }

    pub(crate) fn on_left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        self.move_caret(window, cx, false, |_, _, t| {
            if t.sel.is_empty() {
                t.prev_boundary(t.cursor())
            } else {
                t.sel.start
            }
        });
    }

    pub(crate) fn on_right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        self.move_caret(window, cx, false, |_, _, t| {
            if t.sel.is_empty() {
                t.next_boundary(t.cursor())
            } else {
                t.sel.end
            }
        });
    }

    pub(crate) fn on_select_left(
        &mut self,
        _: &SelectLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, true, |_, _, t| t.prev_boundary(t.cursor()));
    }

    pub(crate) fn on_select_right(
        &mut self,
        _: &SelectRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, true, |_, _, t| t.next_boundary(t.cursor()));
    }

    pub(crate) fn on_word_left(
        &mut self,
        _: &WordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, false, |_, _, t| t.prev_word(t.cursor()));
    }

    pub(crate) fn on_word_right(
        &mut self,
        _: &WordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, false, |_, _, t| t.next_word(t.cursor()));
    }

    pub(crate) fn on_select_word_left(
        &mut self,
        _: &SelectWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, true, |_, _, t| t.prev_word(t.cursor()));
    }

    pub(crate) fn on_select_word_right(
        &mut self,
        _: &SelectWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, true, |_, _, t| t.next_word(t.cursor()));
    }

    pub(crate) fn on_line_start(
        &mut self,
        _: &LineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, false, |this, id, t| {
            this.row_edge(id, t.cursor(), false)
        });
    }

    pub(crate) fn on_line_end(&mut self, _: &LineEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.move_caret(window, cx, false, |this, id, t| {
            this.row_edge(id, t.cursor(), true)
        });
    }

    pub(crate) fn on_select_line_start(
        &mut self,
        _: &SelectLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, true, |this, id, t| {
            this.row_edge(id, t.cursor(), false)
        });
    }

    pub(crate) fn on_select_line_end(
        &mut self,
        _: &SelectLineEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, true, |this, id, t| {
            this.row_edge(id, t.cursor(), true)
        });
    }

    pub(crate) fn on_doc_start(
        &mut self,
        _: &DocStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, false, |_, _, _| 0);
    }

    pub(crate) fn on_doc_end(&mut self, _: &DocEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.move_caret(window, cx, false, |_, _, t| t.text.len());
    }

    pub(crate) fn on_select_doc_start(
        &mut self,
        _: &SelectDocStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, true, |_, _, _| 0);
    }

    pub(crate) fn on_select_doc_end(
        &mut self,
        _: &SelectDocEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_caret(window, cx, true, |_, _, t| t.text.len());
    }

    pub(crate) fn on_select_all(
        &mut self,
        _: &SelectAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_key(window, cx, |this, _, cx| {
            let Some(id) = this.active.clone() else {
                return;
            };
            if let Some(t) = this.texts.get_mut(&id) {
                t.sel = 0..t.text.len();
                t.reversed = false;
            }
            this.on_selection_changed(&id, cx);
        });
    }

    /// Start or end of the visual row holding `at`.
    fn row_edge(&self, id: &str, at: usize, end: bool) -> usize {
        let layouts = self.layouts.borrow();
        let Some(layout) = layouts.get(id) else {
            return if end {
                self.texts.get(id).map_or(0, |t| t.text.len())
            } else {
                0
            };
        };
        let p = layout.position_for_index(at);
        let x = if end {
            layout.bounds.right() + px(10_000.)
        } else {
            layout.bounds.left() - px(10_000.)
        };
        layout.index_for_point(point(x, p.y + layout.line_height / 2.))
    }

    pub(crate) fn on_up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        self.vertical(false, false, window, cx);
    }

    pub(crate) fn on_down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        self.vertical(true, false, window, cx);
    }

    pub(crate) fn on_select_up(
        &mut self,
        _: &SelectUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.vertical(false, true, window, cx);
    }

    pub(crate) fn on_select_down(
        &mut self,
        _: &SelectDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.vertical(true, true, window, cx);
    }

    /// ArrowUp/Down: the slash menu owns them while open; at the text's edge
    /// the caret moves to the previous/next text block, otherwise a row.
    fn vertical(&mut self, down: bool, select: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.with_key(window, cx, |this, _, cx| {
            if !select && this.slash_owns_key() {
                this.slash_move(if down { 1 } else { -1 });
                return;
            }
            let Some(id) = this.active.clone() else {
                return;
            };
            let Some(t) = this.texts.get(&id) else { return };
            let at_edge = if down {
                t.sel.end == t.text.len()
            } else {
                t.sel.start == 0
            };
            if at_edge && !select && !this.is_cell(&id) {
                let flat = this.flat_text();
                let idx = flat.iter().position(|x| *x == id).unwrap_or(0);
                let next = if down {
                    flat.get(idx + 1)
                } else {
                    idx.checked_sub(1).and_then(|i| flat.get(i))
                };
                if let Some(next) = next.cloned() {
                    let offset = if down {
                        0
                    } else {
                        this.texts.get(&next).map_or(0, |t| t.text.len())
                    };
                    this.focus_block(&next, offset, cx);
                }
                return;
            }
            let cursor = t.cursor();
            let len = t.text.len();
            let target = {
                let layouts = this.layouts.borrow();
                match layouts.get(&id) {
                    Some(layout) => {
                        layout
                            .vertical(cursor, down)
                            .unwrap_or(if down { len } else { 0 })
                    }
                    None => {
                        if down {
                            len
                        } else {
                            0
                        }
                    }
                }
            };
            let t = this.texts.get_mut(&id).expect("active text");
            if select {
                t.select_to(target);
            } else {
                t.move_to(target);
            }
            this.on_selection_changed(&id, cx);
        });
    }

    fn on_selection_changed(&mut self, id: &str, cx: &mut Context<Self>) {
        self.update_slash(id);
        self.update_toolbar(id);
        self.restart_blink(cx);
    }

    // ───────────────────────────────────────────── marks

    pub(crate) fn on_bold(&mut self, _: &Bold, window: &mut Window, cx: &mut Context<Self>) {
        self.with_key(window, cx, |this, _, cx| this.mark_shortcut(mark::BOLD, cx));
    }

    pub(crate) fn on_italic(&mut self, _: &Italic, window: &mut Window, cx: &mut Context<Self>) {
        self.with_key(window, cx, |this, _, cx| {
            this.mark_shortcut(mark::ITALIC, cx)
        });
    }

    pub(crate) fn on_underline(
        &mut self,
        _: &Underline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_key(window, cx, |this, _, cx| {
            this.mark_shortcut(mark::UNDERLINE, cx)
        });
    }

    pub(crate) fn on_inline_code(
        &mut self,
        _: &InlineCode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_key(window, cx, |this, _, cx| {
            this.mark_shortcut(mark::INLINE_CODE, cx)
        });
    }

    fn mark_shortcut(&mut self, kind: u32, cx: &mut Context<Self>) {
        if let Some(id) = self.active.clone() {
            self.apply_mark(&id, kind, None, cx);
        }
    }

    /// Toggle a mark over the toolbar's (or the live) selection.
    pub(crate) fn apply_mark(
        &mut self,
        id: &str,
        kind: u32,
        param: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let range = match &self.toolbar {
            Some(tb) if tb.block == id => tb.from..tb.to,
            _ => match self.texts.get(id) {
                Some(t) => t.sel.clone(),
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
            cx,
        );
        self.set_live(id, text, next);
        self.toolbar = None;
        self.focus_block(id, range.end, cx);
    }

    /// Show the marks toolbar over a ranged selection (`onSelect`).
    pub(crate) fn update_toolbar(&mut self, id: &str) {
        if self.read_only || self.is_cell(id) {
            self.toolbar = None;
            return;
        }
        let Some(t) = self.texts.get(id) else { return };
        if t.sel.is_empty() {
            self.toolbar = None;
            return;
        }
        let layouts = self.layouts.borrow();
        let Some(layout) = layouts.get(id) else {
            return;
        };
        let a = layout.position_for_index(t.sel.start);
        let b = layout.position_for_index(t.sel.end);
        let (left, right) = if a.y == b.y {
            (a.x, b.x)
        } else {
            (layout.bounds.left(), layout.bounds.right())
        };
        let tb = Toolbar {
            block: id.to_string(),
            from: t.sel.start,
            to: t.sel.end,
            x: (left + right) / 2.,
            y: a.y,
        };
        drop(layouts);
        self.toolbar = Some(tb);
    }

    // ───────────────────────────────────────────── escape / undo

    pub(crate) fn on_escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        self.with_key(window, cx, |this, _, _| {
            if this.slash.is_some() {
                this.slash = None;
                return;
            }
            if this.paste_menu.is_some() || this.spell_menu.is_some() {
                this.paste_menu = None;
                this.spell_menu = None;
                return;
            }
            this.toolbar = None;
            this.selected.clear();
        });
    }

    pub(crate) fn on_undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.with_key(window, cx, |this, _, cx| this.undo_redo(false, cx));
    }

    pub(crate) fn on_redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.with_key(window, cx, |this, _, cx| this.undo_redo(true, cx));
    }

    pub(crate) fn on_character_palette(
        &mut self,
        _: &CharacterPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.keys_here(window) {
            window.show_character_palette();
        } else {
            cx.propagate();
        }
    }

    // ───────────────────────────────────────────── slash keys

    fn slash_owns_key(&self) -> bool {
        matches!((&self.slash, &self.active), (Some(s), Some(a)) if s.block == *a)
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
        if t.text.get(slash.start..slash.start + 1) != Some("/") {
            self.slash = None;
            return;
        }
        let end = t.sel.end;
        if end <= slash.start {
            self.slash = None;
            return;
        }
        let filter = t.text.get(slash.start + 1..end).unwrap_or("").to_string();
        let count = self.slash_entries(&filter).len();
        let slash = self.slash.as_mut().expect("slash");
        slash.filter = filter;
        if slash.sel >= count {
            slash.sel = count.saturating_sub(1);
        }
        // Anytype auto-closes once the filter clearly matches nothing.
        if count == 0 && slash.filter.chars().count() > 3 {
            self.slash = None;
        }
    }

    fn slash_move(&mut self, delta: isize) {
        let filter = self
            .slash
            .as_ref()
            .map(|s| s.filter.clone())
            .unwrap_or_default();
        let n = self.slash_entries(&filter).len() as isize;
        if n == 0 {
            return;
        }
        if let Some(s) = self.slash.as_mut() {
            s.sel = ((s.sel as isize + delta + n) % n) as usize;
        }
    }

    fn slash_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(slash) = self.slash.clone() else {
            return;
        };
        match self.slash_entries(&slash.filter).into_iter().nth(slash.sel) {
            Some(entry) => self.apply_slash(entry.pick, window, cx),
            None => self.slash = None,
        }
    }

    // ───────────────────────────────────────────── mouse in text

    /// Mouse down on a block's text: caret / word / all, or block selection with modifiers.
    pub(crate) fn text_mouse_down(
        &mut self,
        id: &str,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.spell_menu = None;
        self.paste_menu = None;
        let m = ev.modifiers;
        if !self.read_only && (m.shift || m.platform || m.control) {
            let row = self.selection_row_of(id);
            if m.shift {
                self.range_select(&row);
            } else {
                self.toggle_select(&row);
            }
            cx.notify();
            return;
        }
        if self.read_only {
            self.text_drag = Some((id.to_string(), false));
            return;
        }
        self.selected.clear();
        self.last_focused = id.to_string();
        let index = self
            .layouts
            .borrow()
            .get(id)
            .map(|l| l.index_for_point(ev.position));
        let index = index.unwrap_or_else(|| self.texts.get(id).map_or(0, |t| t.text.len()));
        self.focus_block(id, index, cx);
        window.focus(&self.focus_handle, cx);
        if let Some(t) = self.texts.get_mut(id) {
            match ev.click_count {
                2 => {
                    let w = t.word_at(index);
                    t.sel = w;
                    t.reversed = false;
                }
                n if n >= 3 => {
                    t.sel = 0..t.text.len();
                    t.reversed = false;
                }
                _ => {}
            }
        }
        self.toolbar = None;
        self.text_drag = Some((id.to_string(), false));
        // The text drag proceeds - but if it escapes this block it becomes a block selection.
        self.arm_drag(ev, true, false);
        cx.notify();
    }

    /// The selectable row for a text: a table cell selects as its table.
    fn selection_row_of(&self, id: &str) -> String {
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

    pub(crate) fn toggle_select(&mut self, id: &str) {
        if let Some(i) = self.selected.iter().position(|x| x == id) {
            self.selected.remove(i);
        } else {
            self.selected.push(id.to_string());
        }
        self.deactivate_for_selection();
    }

    /// Anytype shift-click: the contiguous tree-order range from the anchor.
    pub(crate) fn range_select(&mut self, id: &str) {
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
        self.deactivate_for_selection();
    }

    /// Selecting blocks clears text focus (the focused block would eat keys).
    pub(crate) fn deactivate_for_selection(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        self.text_drag = None;
        self.toolbar = None;
        self.slash = None;
        self.active = None;
        self.focus_pending = true;
    }

    pub(crate) fn arm_drag(&mut self, ev: &MouseDownEvent, text_origin: bool, background: bool) {
        let m = ev.modifiers;
        let editor_top = self.bounds.get().top();
        self.drag_sel = Some(DragSel {
            anchor: ev.position,
            base: self.selected.clone(),
            mode: if m.platform || m.control {
                SelMode::Toggle
            } else if m.alt {
                SelMode::Remove
            } else {
                SelMode::Replace
            },
            moved: false,
            text_origin,
            background,
            last: ev.position,
            editor_top,
        });
    }

    /// Mouse moved with a text drag or rubber band armed.
    pub(crate) fn drag_mouse_move(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((id, _)) = self.text_drag.clone() {
            let index = self
                .layouts
                .borrow()
                .get(&id)
                .map(|l| l.index_for_point(position));
            if let Some(index) = index
                && !self.read_only
                && let Some(t) = self.texts.get_mut(&id)
                && self.active.as_deref() == Some(id.as_str())
            {
                t.select_to(index);
                if let Some(drag) = self.text_drag.as_mut() {
                    drag.1 = true;
                }
                cx.notify();
            }
        }
        if let Some(drag) = self.drag_sel.as_mut() {
            drag.last = position;
            self.sel_update(position, cx);
            if self.drag_sel.as_ref().is_some_and(|d| d.moved) && self.autoscroll.is_none() {
                self.start_autoscroll(window, cx);
            }
        }
    }

    /// Hit-test the rubber band against block rows (Anytype selection provider).
    pub(crate) fn sel_update(&mut self, cursor: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.drag_sel.as_ref() else {
            return;
        };
        let top_now = self.bounds.get().top();
        let anchor = point(drag.anchor.x, drag.anchor.y + (top_now - drag.editor_top));
        let w = (cursor.x - anchor.x).abs();
        let h = (cursor.y - anchor.y).abs();
        // Ignore only when BOTH axes are tiny, so a thin drag across one line still selects.
        if !drag.moved && w < px(20.) && h < px(20.) {
            return;
        }
        let rect = Bounds::new(
            point(anchor.x.min(cursor.x), anchor.y.min(cursor.y)),
            size(w, h),
        );
        let mut hits = Vec::new();
        for id in self.flat_ids() {
            if self.is_cell(&id) {
                continue;
            }
            let Some(geom) = self.rows.borrow().get(&id).copied() else {
                continue;
            };
            let r = geom.bounds;
            if r.left() < rect.right()
                && r.right() > rect.left()
                && r.top() < rect.bottom()
                && r.bottom() > rect.top()
            {
                hits.push(id);
            }
        }
        let drag = self.drag_sel.as_mut().expect("drag");
        if drag.text_origin && !drag.moved {
            // The text selection owns the drag until a second block is hit.
            if hits.len() < 2 {
                return;
            }
            self.text_drag = None;
            if let Some(id) = self.active.clone() {
                self.flush_save(&id, cx);
                if let Some(t) = self.texts.get_mut(&id) {
                    let c = t.cursor();
                    t.move_to(c);
                }
            }
            self.active = None;
        }
        let drag = self.drag_sel.as_mut().expect("drag");
        drag.moved = true;
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
        self.deactivate_for_selection();
        self.toolbar = None;
        cx.notify();
    }

    /// Edge autoscroll while the rubber band is out: the pointer parked near
    /// an edge keeps the page scrolling (a scroll event at the pointer).
    fn start_autoscroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.autoscroll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let Ok(Some((last, area))) = this.update(cx, |this, _| {
                    let area = this.visible.get();
                    this.drag_sel
                        .as_ref()
                        .filter(|d| d.moved)
                        .map(|d| (d.last, area))
                }) else {
                    return;
                };
                let scrolled = cx
                    .update(|window, cx| {
                        let h = window.viewport_size().height;
                        let b = px(48.);
                        let dy = if last.y < b {
                            (f32::from(b - last.y) / 6.).ceil().min(12.)
                        } else if last.y > h - b {
                            -(f32::from(last.y - (h - b)) / 6.).ceil().min(12.)
                        } else {
                            0.
                        };
                        if dy != 0. {
                            scroll_at(last, dy, area, window, cx);
                        }
                        dy != 0.
                    })
                    .unwrap_or(false);
                if scrolled {
                    let _ = this.update(cx, |this, cx| this.sel_update(last, cx));
                }
            }
        }));
    }

    /// Mouse released after a text drag or rubber band.
    pub(crate) fn drag_mouse_up(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some((id, moved)) = self.text_drag.take() {
            if !self.read_only {
                self.update_toolbar(&id);
            }
            let collapsed = self.texts.get(&id).is_none_or(|t| t.sel.is_empty());
            if !moved && collapsed {
                self.mention_click(&id, position, cx);
            }
        }
        if let Some(drag) = self.drag_sel.take() {
            if !drag.moved && drag.mode == SelMode::Replace && !drag.text_origin {
                self.selected.clear();
            }
            if !drag.moved && drag.background && !self.read_only {
                self.background_click(position.y, cx);
            }
        }
        self.sel_rect = None;
        self.autoscroll = None;
        cx.notify();
    }

    /// A plain click on a mention opens its object; on a web link opens the
    /// URL in the browser (only http(s), mailto and tel).
    fn mention_click(&mut self, id: &str, position: Point<Pixels>, cx: &mut Context<Self>) {
        let index = self
            .layouts
            .borrow()
            .get(id)
            .map(|l| l.index_for_point(position));
        let Some(index) = index else { return };
        let (text, spans) = self.live(id);
        // A click lands between characters; the mark is the one under the pointer.
        let probe = if index > 0 && index == text.len() {
            index - 1
        } else {
            index
        };
        if let Some(link) = marks::mark_at(&spans, probe, mark::LINK).and_then(|m| m.param.clone())
        {
            // The stored param is the URL itself (the web decodes its attribute back to it).
            let raw = link.trim().to_string();
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
                cx.open_url(&href);
            }
            return;
        }
        if let Some(target) =
            marks::mark_at(&spans, probe, mark::MENTION).and_then(|m| m.param.clone())
        {
            roostr_core::nav::navigate(roostr_core::Route::Object(target), cx);
        }
    }

    /// A click on the page's own background: beside the lines the caret goes
    /// to the end of the nearest one; below them to the last line when it is
    /// an empty paragraph, else a new line is added.
    pub(crate) fn background_click(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let roots: Vec<String> = self
            .root_ids()
            .into_iter()
            .filter(|r| self.texts.contains_key(r))
            .collect();
        if roots.is_empty() {
            self.append_block(cx);
            return;
        }
        let rows = self.rows.borrow().clone();
        let last_bottom = roots
            .last()
            .and_then(|r| rows.get(r))
            .map(|g| g.bounds.bottom());
        if let Some(bottom) = last_bottom
            && y < bottom
        {
            let mut best = roots[0].clone();
            let mut gap = f32::INFINITY;
            for r in &roots {
                let Some(g) = rows.get(r) else { continue };
                let b = g.bounds;
                let d = if y < b.top() {
                    f32::from(b.top() - y)
                } else if y > b.bottom() {
                    f32::from(y - b.bottom())
                } else {
                    0.
                };
                if d < gap {
                    best = r.clone();
                    gap = d;
                }
            }
            let len = self.texts.get(&best).map_or(0, |t| t.text.len());
            self.focus_block(&best, len, cx);
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
            self.focus_block(&last, 0, cx);
            return;
        }
        self.add_sibling(&last, pos::BOTTOM, style::PARAGRAPH, cx);
    }

    pub(crate) fn append_block(&mut self, cx: &mut Context<Self>) {
        let new = new_id();
        let add = self.w_block_add(text_block(&new, "", style::PARAGRAPH), "", pos::NONE, cx);
        self.focus_request = Some((new, 0));
        cx.spawn(async move |this, cx| {
            let _ = add.await;
            let _ = this.update(cx, |this, cx| this.refresh(cx).detach());
        })
        .detach();
    }
}

/// Scroll the editor's scrolling ancestor by `dy` (positive = content moves
/// down), like the web's `scrollBy` on the nearest overflow ancestor. The
/// pointer may sit outside that ancestor (a frame margin, the title bar), so
/// the wheel event is aimed inside the editor's visible part `area`, then the
/// pointer is put back where it really is.
pub(crate) fn scroll_at(
    at: Point<Pixels>,
    dy: f32,
    area: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    if area.size.width <= px(2.) || area.size.height <= px(2.) {
        return;
    }
    let clamp = |v: Pixels, lo: Pixels, hi: Pixels| {
        px(f32::from(v).clamp(f32::from(lo) + 1., f32::from(hi) - 1.))
    };
    let aim = point(
        clamp(at.x, area.left(), area.right()),
        clamp(at.y, area.top(), area.bottom()),
    );
    window.dispatch_event(
        PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: aim,
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        }),
        cx,
    );
    if aim != at {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: at,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            cx,
        );
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let t = self.texts.get(self.active.as_ref()?)?;
        let range = t.range_from_utf16(&range_utf16);
        actual.replace(t.range_to_utf16(&range));
        t.text.get(range).map(str::to_string)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let t = self.texts.get(self.active.as_ref()?)?;
        Some(UTF16Selection {
            range: t.range_to_utf16(&t.sel),
            reversed: t.reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let t = self.texts.get(self.active.as_ref()?)?;
        t.marked.as_ref().map(|r| t.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        if let Some(t) = self.texts.get_mut(&id)
            && t.marked.take().is_some()
        {
            // The committed text is now real input: save, slash and spell see it.
            self.on_input(&id, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        self.insert_text(range, text, cx);
        self.caret_on = true;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let Some(id) = self.active.clone() else {
            return;
        };
        let Some(t) = self.texts.get_mut(&id) else {
            return;
        };
        let range = range_utf16
            .map(|r| t.range_from_utf16(&r))
            .or(t.marked.clone())
            .unwrap_or(t.sel.clone());
        t.replace(range.clone(), new_text);
        t.marked = if new_text.is_empty() {
            None
        } else {
            Some(range.start..range.start + new_text.len())
        };
        if let Some(sel) = new_selected {
            let local =
                marks::utf16_to_byte(new_text, sel.start)..marks::utf16_to_byte(new_text, sel.end);
            t.sel = range.start + local.start..range.start + local.end;
        }
        self.caret_on = true;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let id = self.active.clone()?;
        let t = self.texts.get(&id)?;
        let range = t.range_from_utf16(&range_utf16);
        let layouts = self.layouts.borrow();
        let Some(layout) = layouts.get(&id) else {
            return Some(element_bounds);
        };
        let a = layout.position_for_index(range.start);
        let b = layout.position_for_index(range.end);
        Some(Bounds::from_corners(
            a,
            point(b.x.max(a.x + px(1.)), a.y + layout.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let id = self.active.clone()?;
        let index = self.layouts.borrow().get(&id)?.index_for_point(point);
        Some(self.texts.get(&id)?.offset_to_utf16(index))
    }
}

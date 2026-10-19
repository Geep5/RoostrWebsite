//! Rich text editing for one block: the live text + marks (the web's
//! contenteditable DOM), its caret/selection, and the element that shapes,
//! paints and hit-tests it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::*;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::marks::{self, Span};

/// The text a block shows right now: what the person typed, not yet saved.
#[derive(Clone, Debug, Default)]
pub struct TextState {
    pub text: String,
    pub spans: Vec<Span>,
    /// Selection in bytes; empty = caret.
    pub sel: Range<usize>,
    /// The caret sits at `sel.start` (selection made leftwards).
    pub reversed: bool,
    /// Input-method composition in progress.
    pub marked: Option<Range<usize>>,
}

impl TextState {
    pub fn new(text: String, spans: Vec<Span>) -> Self {
        Self {
            text,
            spans,
            ..Self::default()
        }
    }

    pub fn cursor(&self) -> usize {
        if self.reversed {
            self.sel.start
        } else {
            self.sel.end
        }
    }

    /// Replace the content (the web's `el.innerHTML = toHtml(...)`), keeping
    /// the caret inside the new text.
    pub fn set_content(&mut self, text: String, spans: Vec<Span>) {
        self.text = text;
        self.spans = spans;
        let len = self.text.len();
        let start = marks::floor_char(&self.text, self.sel.start.min(len));
        let end = marks::floor_char(&self.text, self.sel.end.min(len));
        self.sel = start..end;
        self.marked = None;
    }

    pub fn move_to(&mut self, offset: usize) {
        let o = marks::floor_char(&self.text, offset.min(self.text.len()));
        self.sel = o..o;
        self.reversed = false;
    }

    pub fn select_to(&mut self, offset: usize) {
        let o = marks::floor_char(&self.text, offset.min(self.text.len()));
        if self.reversed {
            self.sel.start = o;
        } else {
            self.sel.end = o;
        }
        if self.sel.end < self.sel.start {
            self.reversed = !self.reversed;
            self.sel = self.sel.end..self.sel.start;
        }
    }

    /// Replace `range` with `new`, marks following the edit; caret after it.
    pub fn replace(&mut self, range: Range<usize>, new: &str) {
        let start = marks::floor_char(&self.text, range.start.min(self.text.len()));
        let end = marks::floor_char(&self.text, range.end.min(self.text.len()).max(start));
        marks::adjust_delete(&mut self.spans, start..end);
        marks::adjust_insert(&mut self.spans, start, new.len());
        self.text.replace_range(start..end, new);
        let at = start + new.len();
        self.sel = at..at;
        self.reversed = false;
    }

    pub fn prev_boundary(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .rev()
            .find_map(|(i, _)| (i < offset).then_some(i))
            .unwrap_or(0)
    }

    pub fn next_boundary(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(i, _)| (i > offset).then_some(i))
            .unwrap_or(self.text.len())
    }

    pub fn prev_word(&self, offset: usize) -> usize {
        let mut last = 0;
        for (i, w) in self.text.split_word_bound_indices() {
            if i >= offset {
                break;
            }
            if w.chars().any(char::is_alphanumeric) {
                last = i;
            }
        }
        last
    }

    pub fn next_word(&self, offset: usize) -> usize {
        for (i, w) in self.text.split_word_bound_indices() {
            let end = i + w.len();
            if end > offset && w.chars().any(char::is_alphanumeric) {
                return end;
            }
        }
        self.text.len()
    }

    /// The word around `offset` (double click).
    pub fn word_at(&self, offset: usize) -> Range<usize> {
        for (i, w) in self.text.split_word_bound_indices() {
            if offset >= i && offset < i + w.len() {
                return i..i + w.len();
            }
        }
        self.text.len()..self.text.len()
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        marks::byte_to_utf16(&self.text, offset)
    }

    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        marks::utf16_to_byte(&self.text, offset)
    }

    pub fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end)
    }

    pub fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end)
    }
}

/// One visual row of laid-out text.
#[derive(Clone, Debug)]
struct Row {
    /// Global byte range of the row (a hard line's last row ends before its `\n`).
    start: usize,
    end: usize,
    /// Which shaped (hard) line, and the x where this row starts in it.
    line: usize,
    line_start: usize,
    start_x: Pixels,
    width: Pixels,
    top: Pixels,
}

/// Where a block's text was laid out on the last frame (for mouse and caret math).
pub struct TextLayout {
    /// The text box in window coordinates.
    pub bounds: Bounds<Pixels>,
    pub line_height: Pixels,
    align: TextAlign,
    lines: Rc<Vec<WrappedLine>>,
    rows: Vec<Row>,
}

/// Shared per-frame text geometry, by block id.
pub type Layouts = Rc<RefCell<HashMap<String, TextLayout>>>;

impl TextLayout {
    fn new(
        bounds: Bounds<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        lines: Rc<Vec<WrappedLine>>,
    ) -> Self {
        let mut rows = Vec::new();
        let mut offset = 0;
        let mut top = px(0.);
        for (li, line) in lines.iter().enumerate() {
            let layout = &line.unwrapped_layout;
            let mut start = (0usize, px(0.));
            for b in line.wrap_boundaries.iter() {
                let glyph = &layout.runs[b.run_ix].glyphs[b.glyph_ix];
                rows.push(Row {
                    start: offset + start.0,
                    end: offset + glyph.index,
                    line: li,
                    line_start: offset,
                    start_x: start.1,
                    width: glyph.position.x - start.1,
                    top,
                });
                top += line_height;
                start = (glyph.index, glyph.position.x);
            }
            rows.push(Row {
                start: offset + start.0,
                end: offset + line.len(),
                line: li,
                line_start: offset,
                start_x: start.1,
                width: layout.width - start.1,
                top,
            });
            top += line_height;
            offset += line.len() + 1;
        }
        Self {
            bounds,
            line_height,
            align,
            lines,
            rows,
        }
    }

    fn align_offset(&self, width: Pixels) -> Pixels {
        match self.align {
            TextAlign::Left => px(0.),
            TextAlign::Center => (self.bounds.size.width - width) / 2.,
            TextAlign::Right => self.bounds.size.width - width,
        }
    }

    fn row_for_index(&self, index: usize) -> usize {
        // A wrap boundary index belongs to the row it starts.
        let mut found = 0;
        for (i, r) in self.rows.iter().enumerate() {
            if index >= r.start && index <= r.end {
                found = i;
                if index < r.end || self.rows.get(i + 1).is_none_or(|n| n.line != r.line) {
                    return i;
                }
            }
        }
        found
    }

    fn x_in_row(&self, row: &Row, index: usize) -> Pixels {
        let line = &self.lines[row.line];
        let local = index.saturating_sub(row.line_start).min(line.len());
        line.unwrapped_layout.x_for_index(local) - row.start_x + self.align_offset(row.width)
    }

    /// Byte index nearest to a window point.
    pub fn index_for_point(&self, p: Point<Pixels>) -> usize {
        let local = p - self.bounds.origin;
        let Some(row) = self
            .rows
            .iter()
            .find(|r| local.y < r.top + self.line_height)
            .or(self.rows.last())
        else {
            return 0;
        };
        let line = &self.lines[row.line];
        let x = local.x - self.align_offset(row.width) + row.start_x;
        let idx = row.line_start + line.unwrapped_layout.closest_index_for_x(x.max(px(0.)));
        idx.clamp(row.start, row.end)
    }

    /// Top-left of the caret slot before byte `index`, in window coordinates.
    pub fn position_for_index(&self, index: usize) -> Point<Pixels> {
        let Some(row) = self.rows.get(self.row_for_index(index)) else {
            return self.bounds.origin;
        };
        self.bounds.origin + point(self.x_in_row(row, index), row.top)
    }

    /// The index on the row above/below `index` nearest its x (ArrowUp/Down inside a block).
    pub fn vertical(&self, index: usize, down: bool) -> Option<usize> {
        let row = self.row_for_index(index);
        let target = if down { row + 1 } else { row.checked_sub(1)? };
        let r = self.rows.get(target)?;
        let p = self.position_for_index(index);
        Some(self.index_for_point(point(
            p.x,
            self.bounds.origin.y + r.top + self.line_height / 2.,
        )))
    }

    /// Selection rectangles (window coordinates), one per row crossed.
    fn selection_rects(&self, range: &Range<usize>) -> Vec<Bounds<Pixels>> {
        let mut out = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            let a = range.start.max(r.start);
            let b = range.end.min(r.end);
            let continues = range.end > r.end && range.start <= r.end;
            if a > b || (a == b && !continues) {
                continue;
            }
            let x1 = self.x_in_row(r, a);
            let mut x2 = self.x_in_row(r, b);
            if continues {
                // The selection runs on past this row: cover its end (and the
                // newline of a hard line, like a browser's selection does).
                let wraps = self.rows.get(i + 1).is_some_and(|n| n.line == r.line);
                x2 = if wraps {
                    x2.max(self.align_offset(r.width) + r.width)
                } else {
                    x2 + px(4.)
                };
            }
            let top = self.bounds.origin.y + r.top;
            out.push(Bounds::from_corners(
                point(self.bounds.origin.x + x1, top),
                point(
                    self.bounds.origin.x + x2.max(x1 + px(1.)),
                    top + self.line_height,
                ),
            ));
        }
        out
    }
}

/// One paragraph of shaped text: wraps to its box, paints marks, selection,
/// caret and placeholder, and takes keyboard input when it holds the caret.
pub struct RichText {
    pub key: String,
    pub text: SharedString,
    pub runs: Vec<TextRun>,
    pub font_size: Pixels,
    pub line_height: Pixels,
    pub align: TextAlign,
    /// Shown while the text is empty.
    pub placeholder: Option<(SharedString, TextRun)>,
    pub selection: Option<(Range<usize>, Hsla)>,
    pub caret: Option<(usize, Hsla)>,
    /// Registers the IME/text input handler while `FocusHandle` is focused.
    pub input: Option<(FocusHandle, InputHook)>,
    pub layouts: Layouts,
}

/// Registers the editor as the window's input handler for these bounds.
pub type InputHook = Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut App)>;

impl IntoElement for RichText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub struct RichPrepaint {
    lines: Rc<Vec<WrappedLine>>,
    placeholder: Option<Vec<WrappedLine>>,
    selection: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
}

fn shape(
    window: &Window,
    text: &SharedString,
    runs: &[TextRun],
    font_size: Pixels,
    width: Option<Pixels>,
) -> Vec<WrappedLine> {
    let shaped = window
        .text_system()
        .shape_text(text.clone(), font_size, runs, width, None)
        .map(|l| l.into_vec())
        .unwrap_or_default();
    if shaped.is_empty() {
        let run: Vec<TextRun> = runs
            .first()
            .map(|r| TextRun {
                len: 0,
                ..r.clone()
            })
            .into_iter()
            .collect();
        return window
            .text_system()
            .shape_text(SharedString::default(), font_size, &run, width, None)
            .map(|l| l.into_vec())
            .unwrap_or_default();
    }
    shaped
}

impl Element for RichText {
    type RequestLayoutState = ();
    type PrepaintState = RichPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let text = self.text.clone();
        let runs = self.runs.clone();
        let font_size = self.font_size;
        let line_height = self.line_height;
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, _cx| {
                let wrap = known.width.or(match available.width {
                    AvailableSpace::Definite(w) => Some(w),
                    _ => None,
                });
                let lines = shape(window, &text, &runs, font_size, wrap);
                let mut height = px(0.);
                let mut width = px(0.);
                for l in &lines {
                    let s = l.size(line_height);
                    height += s.height;
                    width = width.max(s.width);
                }
                size(known.width.unwrap_or(width), height.max(line_height))
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> RichPrepaint {
        let lines = Rc::new(shape(
            window,
            &self.text,
            &self.runs,
            self.font_size,
            Some(bounds.size.width),
        ));
        let layout = TextLayout::new(bounds, self.line_height, self.align, lines.clone());
        let selection = match &self.selection {
            Some((range, color)) if !range.is_empty() => layout
                .selection_rects(range)
                .into_iter()
                .map(|b| fill(b, *color))
                .collect(),
            _ => Vec::new(),
        };
        let caret = self.caret.map(|(at, color)| {
            let p = layout.position_for_index(at);
            fill(Bounds::new(p, size(px(1.5), self.line_height)), color)
        });
        let placeholder = if self.text.is_empty() {
            self.placeholder.as_ref().map(|(text, run)| {
                shape(
                    window,
                    text,
                    std::slice::from_ref(&TextRun {
                        len: text.len(),
                        ..run.clone()
                    }),
                    self.font_size,
                    Some(bounds.size.width),
                )
            })
        } else {
            None
        };
        self.layouts.borrow_mut().insert(self.key.clone(), layout);
        RichPrepaint {
            lines,
            placeholder,
            selection,
            caret,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut RichPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some((focus, input)) = &self.input
            && focus.is_focused(window)
        {
            input(bounds, window, cx);
        }
        for quad in state.selection.drain(..) {
            window.paint_quad(quad);
        }
        let mut origin = bounds.origin;
        if let Some(lines) = &state.placeholder {
            for line in lines.iter() {
                let _ = line.paint(
                    origin,
                    self.line_height,
                    self.align,
                    Some(bounds),
                    window,
                    cx,
                );
                origin.y += line.size(self.line_height).height;
            }
        } else {
            for line in state.lines.iter() {
                let _ = line.paint_background(
                    origin,
                    self.line_height,
                    self.align,
                    Some(bounds),
                    window,
                    cx,
                );
                let _ = line.paint(
                    origin,
                    self.line_height,
                    self.align,
                    Some(bounds),
                    window,
                    cx,
                );
                origin.y += line.size(self.line_height).height;
            }
        }
        if let Some(caret) = state.caret.take() {
            window.paint_quad(caret);
        }
    }
}

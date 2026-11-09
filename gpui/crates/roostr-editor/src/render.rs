//! Drawing the editor (`Editor.svelte` markup + `BlockNode.svelte` +
//! `TableBlock.svelte` + `RelationBlock.svelte`): every block kind, the
//! drag rail, drop hints, selection, and the floating menus.

use std::collections::{BTreeMap, HashMap, HashSet};

use roostr_core::State;
use roostr_core::nav::Route;
use roostr_core::types::{Block, BlockContent, layout, pos, style};
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    Button, Glyph, PopupMenu, PopupMenuItem, Theme, glyph, motion, radius, shadow, ty,
};
use rune::{Align, Color, Cursor, Cx, El, Phase, RichStyle, col, el, row, text, text_input};

use crate::editor::{Editor, RowGeom, edit_of, editor, with_editor};
use crate::keys::{self, editor_key, row_key, text_key};
use crate::look::{self, Look, Squiggle};
use crate::toggles;

/// How far past the window (in window heights) a long page still builds its blocks.
const BUILD_MARGIN: f32 = 1.0;
/// The window position steps that re-run the view while the page scrolls.
const SCROLL_STEP: f32 = 200.0;

/// Data shared by every block while one frame renders.
struct Frame<'a> {
    t: &'a Theme,
    e: &'a Editor,
    by_id: HashMap<&'a str, &'a Block>,
    roots: Vec<String>,
    all_roots: Vec<String>,
    space: String,
    narrow: bool,
}

impl Frame<'_> {
    /// `.gutter`: the 48pt drag rail; at ≤720pt it collapses to the shared 16pt gutter.
    fn gutter_w(&self) -> f32 {
        if self.narrow { 16. } else { crate::GUTTER_W }
    }

    /// `.handle`'s `right`: 6pt, 2pt in the 16pt gutter.
    fn handle_right(&self) -> f32 {
        if self.narrow { 2. } else { 6. }
    }

    fn oid(&self) -> String {
        self.e.object_id.clone()
    }
}

/// The body editor of `object_id` (opened with [`crate::open`]): its blocks, and its
/// floating parts pushed into `layer`.
pub fn view(s: &State, cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>, object_id: &str) -> El {
    let Some(e) = editor(s, object_id) else {
        return el();
    };
    let narrow = cx
        .layout_with("kit-root", |r| roostr_ui::layout::is_mobile(r[2]))
        .unwrap_or(false);
    let f = Frame {
        t,
        e,
        by_id: e.blocks.iter().map(|b| (b.id.as_str(), b)).collect(),
        roots: e.root_ids(),
        all_roots: crate::doc::root_order(&e.blocks),
        space: roostr_core::relations::object_space_id(&s.store, &e.object_fields()),
        narrow,
    };
    let oid = f.oid();
    let key = editor_key(&oid);
    let unscrolled_top = cx.layout_with(&key, |r| r[1]).unwrap_or(0.);
    let window_h = cx.layout_with("kit-root", |r| r[3]).unwrap_or(900.);
    let blocks = windowed_blocks(&f, s, cx, layer, &key);
    let empty = f.roots.is_empty();
    let loaded = e.object.is_some();

    let mut body = col()
        .key(key.clone())
        .w_full()
        .min_h(240.)
        .pb(120.)
        .cursor(if e.read_only {
            Cursor::Default
        } else {
            Cursor::Text
        })
        .children(blocks);
    if !e.read_only {
        // With blocks selected the editor itself takes the keyboard.
        let (k, d, u, r, l, drop, hover, mv) = (
            oid.clone(),
            oid.clone(),
            oid.clone(),
            oid.clone(),
            oid.clone(),
            oid.clone(),
            oid.clone(),
            oid.clone(),
        );
        body = body
            .focusable(true)
            .focus_ring(0., 0., Color::TRANSPARENT)
            .on_key(move |s: &mut State, ev, tx| keys::editor_keys(s, &k, ev, tx))
            // The page background: a rubber band, or a click that puts the caret on a line.
            .on_drag(move |s: &mut State, drag, tx| {
                let [x, y] = drag.window_point();
                with_editor(s, &d, |e, s| match drag.phase {
                    Phase::Down => e.arm_drag(None, (x, y), tx.mods(), s),
                    Phase::Move => {
                        e.sel_update((x, y), s);
                        if !e.selected.is_empty() {
                            tx.focus(editor_key(&e.object_id), false);
                        }
                    }
                    Phase::Up => e.drag_release((x, y), s),
                });
            })
            .on_layout_tx(move |s: &mut State, rect, tx| {
                with_editor(s, &l, |e, _| {
                    e.bounds = rect;
                    e.narrow = narrow;
                    e.unscrolled_top = unscrolled_top;
                    e.window_h = window_h;
                });
                tx.live_only();
            })
            .on_drop_hover(move |s: &mut State, over, _| {
                with_editor(s, &hover, |e, _| {
                    e.file_drag = over;
                    if over {
                        e.build_drag_rects();
                    } else {
                        e.drag_rects.clear();
                        e.drop_hint = None;
                        e.last_valid_drop = None;
                    }
                });
            })
            .on_drop_move(move |s: &mut State, p, _| {
                with_editor(s, &mv, |e, _| {
                    let hit = e.resolve_drop((p.window_x, p.window_y));
                    if let Some(h) = &hit {
                        e.last_valid_drop = Some(h.clone());
                    }
                    e.drop_hint = hit;
                });
            })
            .on_drop_files(move |s: &mut State, files, _| {
                let files = files.to_vec();
                with_editor(s, &drop, |e, s| e.drop_external(files, s));
            });
        let _ = (u, r);
    }
    if empty && loaded && !e.read_only {
        let o = oid.clone();
        body = body.child(
            el().key(format!("ed-{oid}-empty-hint"))
                .ml(f.gutter_w())
                .py(8.)
                .cursor(Cursor::Text)
                .child(
                    text("Click to start writing…")
                        .font(ty::ui(15.))
                        .color(t.muted),
                )
                .on_press(move |s: &mut State| {
                    with_editor(s, &o, |e, s| e.append_block(s));
                }),
        );
    }
    if !loaded && let Some(err) = &e.load_error {
        body = body.child(
            el().ml(f.gutter_w())
                .child(text(err.clone()).font(ty::ui(13.)).color(t.muted)),
        );
    }
    overlays(&f, s, cx, layer);
    body
}

/// The root blocks to build this frame, in order, with spacers sized by the
/// blocks' last heights standing in for the rest: a long page builds only the
/// blocks in, or a window's height around, the window. Blocks in use (typing,
/// selected, open menus) are always built, and every block is while
/// something is dragged (drop targets come from what's laid out).
fn windowed_blocks(
    f: &Frame,
    s: &State,
    cx: &mut Cx<State>,
    layer: &mut Vec<El>,
    key: &str,
) -> Vec<El> {
    let e = f.e;
    let dragging = e.dragging.is_some() || e.file_drag || e.drag_sel.is_some();
    // The root each id lives under (blocks nest under toggles, lists, columns).
    let mut root_of: HashMap<&str, &str> = HashMap::new();
    for root in &f.roots {
        let mut stack = vec![root.as_str()];
        while let Some(id) = stack.pop() {
            root_of.insert(id, root.as_str());
            if let Some(b) = f.by_id.get(id) {
                stack.extend(b.children_ids.iter().map(String::as_str));
            }
        }
    }
    let mut pinned: HashSet<&str> = HashSet::new();
    for id in e
        .active
        .iter()
        .chain(e.selected.iter())
        .chain(std::iter::once(&e.last_focused))
        .chain(e.focus_request.as_ref().map(|(id, _)| id))
        .chain(e.slash.as_ref().map(|sl| &sl.block))
        .chain(e.block_menu.as_ref().map(|m| &m.block))
        .chain(e.property_suggest.as_ref().map(|p| &p.block))
        .chain(e.toolbar.as_ref().map(|tb| &tb.block))
        .chain(
            e.texts
                .iter()
                .filter(|(_, t)| t.focused())
                .map(|(id, _)| id),
        )
    {
        if let Some(root) = root_of.get(id.as_str()) {
            pinned.insert(root);
        }
    }
    // The window, in the editor's own coordinates.
    let view_h = cx.layout_with("kit-root", |r| r[3]).unwrap_or(900.);
    let top = cx
        .window_layout_with(key, |r| (r[1] / SCROLL_STEP).floor() * SCROLL_STEP)
        .unwrap_or(0.);
    let (lo, hi) = (
        -top - BUILD_MARGIN * view_h,
        -top + (1. + BUILD_MARGIN) * view_h,
    );
    let typical = if e.block_heights.is_empty() {
        32.
    } else {
        e.block_heights.values().sum::<f32>() / e.block_heights.len() as f32
    };
    let mut blocks = Vec::new();
    let mut gap = 0.;
    let mut y = 0.;
    for id in &f.roots {
        let h = e.block_heights.get(id).copied().unwrap_or(typical);
        let near = y + h >= lo && y <= hi;
        y += h;
        if !(dragging || near || pinned.contains(id.as_str())) {
            gap += h;
            continue;
        }
        if gap > 0. {
            blocks.push(el().h(gap));
            gap = 0.;
        }
        if let Some(block) = render_block(id, f, s, cx, layer) {
            let (oid, rid) = (f.oid(), id.clone());
            blocks.push(col().w_full().child(block).on_layout_tx(
                move |s: &mut State, rect, tx| {
                    with_editor(s, &oid, |e, _| {
                        e.block_heights.insert(rid.clone(), rect[3]);
                    });
                    tx.live_only();
                },
            ));
        }
    }
    if gap > 0. {
        blocks.push(el().h(gap));
    }
    blocks
}

/// Record a row's laid-out rect in window points (drag targeting, rubber band).
fn geom(el: El, f: &Frame, id: &str, gutter: bool, bot: bool) -> El {
    let (oid, id) = (f.oid(), id.to_string());
    el.on_layout_tx(move |s: &mut State, rect, tx| {
        with_editor(s, &oid, |e, _| {
            let g = RowGeom {
                bounds: rect,
                gutter,
            };
            if bot {
                e.bot_strips.insert(id.clone(), g);
            } else {
                e.rows.insert(id.clone(), g);
            }
        });
        tx.live_only();
    })
}

/// What a dragged block shows: its text, else its custom content type.
pub(crate) fn drag_label(c: &BlockContent) -> String {
    c.text.as_ref().map(|t| t.text.clone()).unwrap_or_else(|| {
        c.custom
            .as_ref()
            .map(|c| c.content_type.clone())
            .unwrap_or_default()
    })
}

/// The 48pt rail with the drag handle (Anytype .icon.commonDnd): it shows
/// while the row is hovered; a click opens the block menu, a drag moves the block.
fn gutter(f: &Frame, id: &str) -> El {
    let t = f.t;
    let rail = el().w(f.gutter_w()).self_stretch();
    if f.e.read_only {
        return rail;
    }
    let dots = col()
        .gap(3.)
        .children((0..3).map(|_| el().size(2., 2.).radius(1.).fill(t.muted)));
    let (open_oid, open_id) = (f.oid(), id.to_string());
    let (drag_oid, drag_id) = (f.oid(), id.to_string());
    let edge = f.gutter_w() - f.handle_right();
    let handle = col()
        .key(format!("handle-{id}"))
        .pin(None, Some(0.), Some(f.handle_right()), Some(0.))
        .min_h(24.)
        .w(12.)
        .border(1., t.border)
        .radius(6.)
        .center()
        .cursor(Cursor::Grab)
        .hover(t.hover)
        .group_opacity(0., 1.)
        .hover_motion(motion::HOVER_REVEAL.anim(), motion::HOVER_REVEAL.anim())
        .help("Click for actions; drag to move")
        .child(dots)
        .on_press(move |s: &mut State| {
            with_editor(s, &open_oid, |e, s| {
                if std::mem::take(&mut e.handle_dragged) {
                    return;
                }
                let at = e
                    .rows
                    .get(&open_id)
                    .map(|g| (g.bounds[0] + edge + 8., g.bounds[1]))
                    .unwrap_or_default();
                e.open_block_menu(&open_id, at, s);
            });
        })
        .on_drag(move |s: &mut State, drag, tx| {
            let [x, y] = drag.window_point();
            with_editor(s, &drag_oid, |e, s| match drag.phase {
                Phase::Down => e.handle_dragged = false,
                Phase::Move => {
                    if e.dragging.is_none() && drag.dx.abs() + drag.dy.abs() > 4. {
                        e.handle_dragged = true;
                        e.begin_drag(&drag_id);
                    }
                    if e.dragging.is_some() {
                        e.drag_move_to((x, y));
                        edge_scroll(e, s, tx);
                    }
                }
                Phase::Up => {
                    if e.dragging.is_some() {
                        // The pointer can be between targets on the frame the drop lands.
                        let hint = e.drop_hint.clone().or_else(|| {
                            e.last_valid_drop
                                .clone()
                                .filter(|h| !e.drop_forbidden(&h.id))
                        });
                        let hint = if drag.cancelled() { None } else { hint };
                        e.finish_drag(hint, s);
                    }
                }
            });
        });
    rail.child(handle)
}

/// While a block is dragged near the window's top or bottom edge, the page scrolls (and keeps
/// scrolling while the pointer rests there).
fn edge_scroll(e: &mut Editor, _s: &mut State, tx: &mut rune::Tx) {
    let (_, y) = e.drag_point;
    let h = e.window_h;
    let dy = Editor::edge_scroll(y, h);
    if dy != 0. {
        let offset = e.unscrolled_top - e.bounds[1];
        tx.scroll_to_y(editor_key(&e.object_id), (offset + dy).max(0.));
        tx.drag_repeat_after(std::time::Duration::from_millis(50));
    }
}

/// Drop-hint lines and the selection overlay over a row.
fn row_decor(f: &Frame, id: &str) -> Vec<El> {
    let t = f.t;
    let mut out = Vec::new();
    let zone = match &f.e.drop_hint {
        Some(h) if h.id == id && !h.bot => h.position,
        _ => 0,
    };
    let line = |e: El| e.fill(t.accent);
    match zone {
        pos::TOP => out.push(line(el().pin(Some(0.), Some(-2.), Some(0.), None).h(2.))),
        pos::BOTTOM => out.push(line(el().pin(Some(0.), None, Some(0.), Some(-2.)).h(2.))),
        pos::LEFT => out.push(line(el().pin(Some(-3.), Some(0.), None, Some(0.)).w(3.))),
        pos::RIGHT => out.push(line(el().pin(None, Some(0.), Some(-3.), Some(0.)).w(3.))),
        pos::INNER_FIRST => out.push(
            el().pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .radius(4.)
                .fill(t.block_selection),
        ),
        _ => {}
    }
    if f.e.selected.iter().any(|x| x == id) {
        out.push(
            el().key(format!("selected-{id}"))
                .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .radius(2.)
                .fill(t.block_selection)
                .inert(),
        );
    }
    out
}

/// The `.block` row wrapper shared by every kind: a press beside the text
/// selects (with modifiers) or arms a rubber band; right-click opens the block menu.
fn block_row(f: &Frame, id: &str, gutter: bool) -> El {
    let r = row()
        .key(row_key(id))
        .w_full()
        .items_start()
        .py(1.)
        .radius(4.)
        .hover_group();
    let r = if f.narrow { r.pr(16.) } else { r };
    let r = geom(r, f, id, gutter, false);
    if f.e.read_only {
        return r;
    }
    let (doid, did) = (f.oid(), id.to_string());
    let (moid, mid) = (f.oid(), id.to_string());
    r.on_drag(move |s: &mut State, drag, tx| {
        let [x, y] = drag.window_point();
        with_editor(s, &doid, |e, s| match drag.phase {
            Phase::Down => e.arm_drag(Some(&did), (x, y), tx.mods(), s),
            Phase::Move => e.sel_update((x, y), s),
            Phase::Up => {
                if let Some(drag) = e.drag_sel.take() {
                    if !drag.moved && drag.mode == crate::editor::SelMode::Replace {
                        e.selected.clear();
                    }
                }
                e.sel_rect = None;
            }
        });
        if with_editor(s, &doid, |e, _| !e.selected.is_empty()).unwrap_or(false) {
            tx.focus(editor_key(&doid), false);
        }
    })
    .on_secondary_press(move |s: &mut State, p, tx| {
        with_editor(s, &moid, |e, s| e.row_context_menu(&mid, (p.x, p.y), s, tx));
    })
}

impl Editor {
    /// Right-click: a live multi-selection owns it; the focused text keeps the
    /// field's own menu; anything else opens the block action menu.
    fn row_context_menu(&mut self, id: &str, at: (f32, f32), s: &mut State, tx: &mut rune::Tx) {
        if self.read_only {
            return;
        }
        if self.texts.contains_key(id) && self.open_spell_menu(id, s) {
            return;
        }
        if self.selected.len() > 1 {
            let hit = if self.selected.iter().any(|x| x == id) {
                id.to_string()
            } else {
                self.selected[0].clone()
            };
            self.open_block_menu(&hit, at, s);
            return;
        }
        if self.editing() == Some(id) {
            return;
        }
        let _ = tx;
        self.open_block_menu(id, at, s);
    }
}

/// One block's text: an editable Rune field, or (read-only) selectable text.
fn rich_text(
    f: &Frame,
    id: &str,
    look: &Look,
    align: Align,
    placeholder: Option<&'static str>,
) -> El {
    let (t, e) = (f.t, f.e);
    let Some(live) = e.texts.get(id) else {
        return el();
    };
    let mut squiggles: Vec<Squiggle> = Vec::new();
    if let Some(ranges) = e.misspelled.get(id) {
        for r in ranges.iter().filter(|r| r.end <= live.text().len()) {
            squiggles.push(Squiggle {
                range: r.clone(),
                color: roostr_ui::color::rgb(0xe2400c),
            });
        }
    }
    if let Some(problems) = e.lint.get(id) {
        for p in problems.iter().filter(|p| p.to <= live.text().len()) {
            let color = if p.severity == crate::codelint::Severity::Error {
                t.red
            } else {
                t.lint_warning
            };
            squiggles.push(Squiggle {
                range: p.from..p.to,
                color,
            });
        }
    }
    let paints = look::paints(t, live.text(), &live.spans, look, &squiggles);
    let rich = RichStyle {
        code_fg: None,
        code_bg: t.panel,
        code_block_bg: t.panel,
        link: t.accent,
        quote_bar: t.accent,
        marker: Some(t.muted),
    };
    if e.read_only {
        let content = live.text().to_string();
        let mut out = text(content)
            .key(text_key(id))
            .font(look.style)
            .color(look.color)
            .w_full()
            .align(align)
            .spans(paints)
            .rich_style(rich)
            .selectable(true)
            .selection_color(t.mark)
            .on_link(|s: &mut State, url: &str, tx| keys::open_link(s, url, tx));
        for m in live.edit.marks() {
            match &m.mark {
                rune::Mark::Strike => out = out.strike(m.start, m.end, None),
                rune::Mark::Link(url) => {
                    out = out.link(rune::Link::new(m.start..m.end, url.clone()));
                }
                _ => {}
            }
        }
        return out;
    }
    let focused = live.focused();
    let placeholder = placeholder
        .filter(|_| focused && live.text().is_empty())
        .unwrap_or("");
    let (eo, ei) = (f.oid(), id.to_string());
    let (ko, ki) = (f.oid(), id.to_string());
    let (co, ci) = (f.oid(), id.to_string());
    let (po, pi) = (f.oid(), id.to_string());
    text_input(&live.edit, look.style)
        .key(text_key(id))
        .w_full()
        .align(align)
        .color(look.color)
        .caret(t.fg, t.mark)
        .placeholder(placeholder, t.muted.alpha(0.6))
        .rich_style(rich)
        .spans(paints)
        .on_edit(move |s: &mut State| edit_of(s, &eo, &ei))
        .on_input_key(move |s: &mut State, ev, tx| keys::input_key(s, &ko, &ki, ev, tx))
        .on_input_change(move |s: &mut State, _| keys::input_changed(s, &co, &ci))
        .on_input_paste(move |s: &mut State, content, tx| {
            keys::input_paste(s, &po, &pi, content, tx)
        })
        .on_link(|s: &mut State, url: &str, tx| keys::open_link(s, url, tx))
}

fn render_block(
    id: &str,
    f: &Frame,
    s: &State,
    cx: &mut Cx<State>,
    layer: &mut Vec<El>,
) -> Option<El> {
    let block = *f.by_id.get(id)?;
    let c = &block.content;
    let t = f.t;
    if let Some(l) = &c.layout {
        if l.style == layout::ROW {
            // Columns share the row by their `width` field (else equally).
            let kids: Vec<El> = block
                .children_ids
                .iter()
                .filter_map(|cid| {
                    let width = f
                        .by_id
                        .get(cid.as_str())
                        .and_then(|b| b.fields.as_ref())
                        .and_then(|m| m.entries.get("width"))
                        .and_then(|v| v.as_number())
                        .unwrap_or(0.);
                    let el = render_block(cid, f, s, cx, layer)?;
                    Some(if width > 0. {
                        col().min_w(0.).w_frac(width as f32).child(el)
                    } else {
                        col().min_w(0.).grow(1.).child(el)
                    })
                })
                .collect();
            return Some(row().w_full().gap(24.).my(2.).children(kids));
        }
        if l.style == layout::COLUMN {
            let kids: Vec<El> = block
                .children_ids
                .iter()
                .filter_map(|cid| render_block(cid, f, s, cx, layer))
                .collect();
            return Some(col().w_full().min_w(0.).children(kids));
        }
    }
    let dragging = f.e.dragging.as_deref() == Some(id);
    if c.table.is_some() {
        let r = block_row(f, id, true)
            .opacity(if dragging { 0.4 } else { 1. })
            .child(gutter(f, id))
            .child(render_table(block, f))
            .children(row_decor(f, id));
        return Some(r);
    }
    if let Some(text) = &c.text {
        return Some(render_text_block(block, text, f, s, cx, layer, dragging));
    }
    let custom = c.custom.as_ref()?;
    let meta = custom.meta.clone().unwrap_or_default();
    let body: El = match custom.content_type.as_str() {
        "divider" => {
            let dots = meta.get("style").map(String::as_str) == Some("dots");
            let line = row().grow(1.).min_w(0.).h(16.).items_center();
            if dots {
                line.justify_center()
                    .gap(18.)
                    .children((0..3).map(|_| el().size(2., 2.).radius(1.).fill(t.accent)))
            } else {
                line.child(el().w_full().h(1.).fill(t.border))
            }
        }
        "relation" => render_relation(block, &meta, f, s, cx, layer),
        "file" => col()
            .grow(1.)
            .min_w(0.)
            .child(crate::file_block::file_block_view(s, cx, t, f.e, id, layer)),
        "embed" => crate::embed_view::embed_view(s, cx, t, f.e, &meta, id),
        "bookmark" => crate::embed_view::bookmark_view(s, cx, t, &meta, id),
        "link" => render_link(block, &meta, f, s),
        other => {
            // Unknown custom content: a chip, children nested below.
            let kids: Vec<El> = block
                .children_ids
                .iter()
                .filter_map(|cid| render_block(cid, f, s, cx, layer))
                .collect();
            let chip = geom(
                row().key(row_key(id)).py(1.).child(
                    row().radius(6.).border(1., t.border).px(8.).py(2.).child(
                        text(other.to_string())
                            .font(ty::ui(11.).lh(15.))
                            .color(t.muted),
                    ),
                ),
                f,
                id,
                false,
                false,
            )
            .children(row_decor(f, id));
            let mut out = col().w_full().radius(4.).child(chip);
            if !kids.is_empty() {
                out = out.child(col().w_full().pl(26.).children(kids));
            }
            return Some(out);
        }
    };
    Some(
        block_row(f, id, true)
            .opacity(if dragging { 0.4 } else { 1. })
            .child(gutter(f, id))
            .child(body)
            .children(row_decor(f, id)),
    )
}

fn render_text_block(
    block: &Block,
    tc: &roostr_core::types::TextContent,
    f: &Frame,
    s: &State,
    cx: &mut Cx<State>,
    layer: &mut Vec<El>,
    dragging: bool,
) -> El {
    let t = f.t;
    let id = block.id.as_str();
    let st = tc.style;
    let checked = tc.checked.unwrap_or(false);
    let look = Look::for_style(t, st, checked, tc.color.as_deref().unwrap_or(""));
    let align = match block.align.unwrap_or(0) {
        1 => Align::Center,
        2 => Align::End,
        _ => Align::Start,
    };
    let is_toggle = st == style::TOGGLE;
    let open = is_toggle && toggles::is_toggle_open(&f.e.object_id, id, &s.ui);
    let placeholder = if st == style::TITLE {
        "Untitled"
    } else {
        "Type / for commands"
    };
    let field = rich_text(f, id, &look, align, Some(placeholder));
    let mut text_box = col().grow(1.).min_w(60.).px(2.).py(3.);
    match st {
        style::QUOTE => {
            text_box = text_box
                .border_sides([0., 0., 0., 3.], t.accent)
                .border_layout()
                .pl(12.)
        }
        style::CODE => text_box = text_box.fill(t.panel).radius(6.).px(10.).py(8.),
        style::CALLOUT => text_box = text_box.fill(t.panel).radius(8.).px(12.).py(10.),
        _ => {}
    }
    let mut r = block_row(f, id, true).child(gutter(f, id));
    match st {
        style::CHECKBOX => {
            let (oid, bid) = (f.oid(), id.to_string());
            let mut check = roostr_ui::checkbox(format!("check-{id}"), checked)
                .size(roostr_ui::KitSize::Medium);
            check = if f.e.read_only {
                check.tab_stop(false)
            } else {
                check.on_change(move |s: &mut State, done: bool| {
                    with_editor(s, &oid, |e, s| e.toggle_checked(&bid, done, s));
                })
            };
            r = r.child(row().size(24., 24.).mt(3.).center().child(check.build(t)));
        }
        style::BULLET => {
            r = r.child(
                el().pt(4.)
                    .child(text("•").font(ty::ui(16.).lh(19.)).color(t.muted)),
            )
        }
        style::NUMBERED => {
            let n = ordinal(id, f);
            r = r.child(
                el().pt(4.).child(
                    text(format!("{n}."))
                        .font(ty::ui(16.).lh(19.))
                        .color(t.muted),
                ),
            );
        }
        style::TOGGLE => {
            let (oid, bid) = (f.oid(), id.to_string());
            let arrow = glyph(Glyph::ChevronRight, 16.)
                .color(t.fg)
                .rotate(if open {
                    std::f32::consts::FRAC_PI_2
                } else {
                    0.
                })
                .rotate_motion(motion::DISCLOSURE.anim());
            r = r.child(
                Button::new(format!("toggle-{id}"))
                    .ghost()
                    .xsmall()
                    .square(24.)
                    .p(0.)
                    .accessibility_label(if open { "Collapse" } else { "Expand" })
                    .child(arrow)
                    .on_press(move |s: &mut State| {
                        with_editor(s, &oid, |e, s| e.flip_toggle(&bid, s));
                    })
                    .build(t)
                    .mr(6.),
            );
        }
        _ => {}
    }
    r = r.child(text_box.child(field));
    if st == style::CODE {
        let copied = f.e.copied.contains(id);
        let (oid, bid) = (f.oid(), id.to_string());
        r = r.child(
            el().pin(None, Some(7.), Some(6.), None)
                .w(58.)
                .group_opacity(if copied { 1. } else { 0. }, 1.)
                .hover_motion(motion::FADE.anim(), motion::FADE.anim())
                .child(
                    Button::new(format!("copy-{id}"))
                        .outline()
                        .xsmall()
                        .w(58.)
                        .accessibility_label("Copy code")
                        .child(
                            text(if copied { "Copied" } else { "Copy" })
                                .font(ty::ui(11.))
                                .color(t.fg),
                        )
                        .on_press_tx(move |s: &mut State, tx| {
                            with_editor(s, &oid, |e, s| e.copy_code(&bid, s, tx));
                        })
                        .build(t),
                ),
        );
    }
    r = r.children(row_decor(f, id));

    // A toggle's children are its collapsible content (shown only while
    // open, so a closed toggle neither holds the caret nor reports row
    // geometry).
    let shown = !is_toggle || open;
    let mut tree = col()
        .w_full()
        .radius(4.)
        .opacity(if dragging { 0.4 } else { 1. });
    if let Some(bg) = block.background_color.as_deref().and_then(look::css_color) {
        tree = tree.fill(bg);
    }
    tree = tree.child(r);
    if st == style::CODE
        && let Some(problems) = f.e.lint.get(id)
        && !problems.is_empty()
    {
        tree = tree.child(col().ml(f.gutter_w()).mt(4.).mb(2.).px(10.).children(
            problems.iter().map(|p| {
                let dot = if p.severity == crate::codelint::Severity::Error {
                    t.red
                } else {
                    t.lint_warning
                };
                row()
                    .gap(6.)
                    .items_center()
                    .child(text("●").font(ty::ui(9.)).color(dot))
                    .child(
                        text(format!("Line {}:{}", p.line, p.col))
                            .font(ty::mono(12.).lh(18.))
                            .color(t.muted),
                    )
                    .child(
                        text(p.message.clone())
                            .font(ty::ui(12.).lh(18.))
                            .color(t.muted)
                            .wrap(),
                    )
            }),
        ));
    }
    if !block.children_ids.is_empty() && shown {
        let kids: Vec<El> = block
            .children_ids
            .iter()
            .filter_map(|cid| render_block(cid, f, s, cx, layer))
            .collect();
        tree = tree.child(
            col()
                .w_full()
                .pl(26.)
                .children(kids)
                .transition(rune::Transition::opacity())
                .opacity_motion(motion::DISCLOSURE.anim()),
        );
        // Anytype targetBot: a thin strip below the children; a drop there lands after the subtree.
        let over = matches!(&f.e.drop_hint, Some(h) if h.id == block.id && h.bot);
        let strip = el().w_full().h(10.).mt(-4.).mb(-6.);
        let strip = if over {
            strip.child(
                el().pin(Some(0.), None, Some(0.), Some(0.))
                    .h(2.)
                    .fill(t.accent),
            )
        } else {
            strip
        };
        tree = tree.child(geom(strip, f, id, false, true));
    } else if is_toggle && shown && !f.e.read_only {
        let (oid, bid) = (f.oid(), id.to_string());
        tree = tree.child(
            el().key(format!("empty-toggle-{id}"))
                .ml(30.)
                .py(2.)
                .cursor(Cursor::Pointer)
                .hover_group()
                .child(
                    text("Empty toggle. Click or drop Block inside")
                        .font(ty::ui(15.))
                        .color(t.muted)
                        .group_color(t.fg)
                        .truncate(),
                )
                .on_press(move |s: &mut State| {
                    with_editor(s, &oid, |e, s| e.on_empty_toggle(&bid, s));
                }),
        );
    }
    tree
}

/// A numbered item's number: its place in the run of numbered siblings.
fn ordinal(id: &str, f: &Frame) -> usize {
    let siblings = f
        .by_id
        .values()
        .find(|b| b.children_ids.iter().any(|c| c == id))
        .map(|b| b.children_ids.clone())
        .unwrap_or_else(|| f.all_roots.clone());
    let mut n = 0;
    for sid in &siblings {
        n = if f
            .by_id
            .get(sid.as_str())
            .and_then(|b| b.content.text.as_ref())
            .map(|t| t.style)
            == Some(style::NUMBERED)
        {
            n + 1
        } else {
            0
        };
        if sid == id {
            return n;
        }
    }
    1
}

/// TableBlock: cells are ordinary text blocks "<rowId>-<colId>".
fn render_table(block: &Block, f: &Frame) -> El {
    let (t, e) = (f.t, f.e);
    let (cols, rows) = e.table_shape(&block.id);
    let table_id = block.id.clone();
    let ro = e.read_only;
    // `×` row/column deletes: muted marks whose `×` turns red under the pointer.
    let mini = |key: String, enabled: bool, tip: &'static str| {
        let mark = text("×").font(ty::ui(12.).lh(14.)).color(t.muted);
        let mark = if enabled {
            mark.group_color(t.red)
        } else {
            mark
        };
        el().key(key)
            .h(14.)
            .px(4.)
            .center()
            .radius(radius::KIT)
            .hover_group()
            .help(tip)
            .label(tip)
            .opacity(if enabled { 1. } else { 0.3 })
            .cursor(if enabled {
                Cursor::Pointer
            } else {
                Cursor::Default
            })
            .child(mark)
    };
    // Dashed `+` edges that add a row or a column; under the pointer the border and the `+` turn accent.
    let add = |key: String, tip: &'static str| {
        el().key(key)
            .center()
            .radius(6.)
            .border(1., t.border)
            .border_dash(3., 3.)
            .hover_group()
            .help(tip)
            .label(tip)
            .cursor(Cursor::Pointer)
            .group_opacity(0., 1.)
            .hover_motion(motion::HOVER_REVEAL.anim(), motion::HOVER_REVEAL.anim())
            .child(
                text("+")
                    .font(ty::ui(13.))
                    .color(t.muted)
                    .group_color(t.accent),
            )
    };
    let mut wrap = col()
        .key(format!("table-{}", block.id))
        .grow(1.)
        .min_w(0.)
        .py(4.)
        .hover_group();
    if !ro {
        let can = cols.len() > 1;
        wrap = wrap.child(
            row()
                .h(14.)
                .mr(20.)
                .group_opacity(0., 1.)
                .hover_motion(motion::HOVER_REVEAL.anim(), motion::HOVER_REVEAL.anim())
                .children(cols.iter().enumerate().map(|(i, c)| {
                    let (oid, tid, c) = (f.oid(), table_id.clone(), c.clone());
                    let m = mini(format!("delcol-{table_id}-{i}"), can, "Delete column");
                    let m = if can {
                        m.on_press(move |s: &mut State| {
                            with_editor(s, &oid, |e, s| e.table_col_remove(&tid, &c, s));
                        })
                    } else {
                        m
                    };
                    row().grow(1.).justify_center().child(m)
                })),
        );
    }
    let mut grid = col()
        .grow(1.)
        .min_w(0.)
        .border_sides([1., 0., 0., 1.], t.border)
        .border_layout();
    for row_id in &rows {
        let header = f
            .by_id
            .get(row_id.as_str())
            .and_then(|b| b.content.table_row.as_ref())
            .and_then(|r| r.is_header)
            .unwrap_or(false);
        let mut tr = row().w_full().items_stretch();
        for c in &cols {
            let cell = format!("{row_id}-{c}");
            let has_text = f
                .by_id
                .get(cell.as_str())
                .is_some_and(|b| b.content.text.is_some());
            let look = Look::table_cell(t, header);
            let mut td = col()
                .key(row_key(&cell))
                .grow(1.)
                .min_w(60.)
                .border_sides([0., 1., 1., 0.], t.border)
                .border_layout();
            if header {
                td = td.fill(t.hover);
            }
            td = geom(td, f, &cell, false, false).children(row_decor(f, &cell));
            if has_text {
                td = td.child(col().px(8.).py(6.).min_h(20.).child(rich_text(
                    f,
                    &cell,
                    &look,
                    Align::Start,
                    None,
                )));
            }
            tr = tr.child(td);
        }
        grid = grid.child(tr);
    }
    let mut grid_row = row().w_full().items_stretch().gap(2.).child(grid);
    if !ro {
        // Row deletes sit in a column beside the grid, one per row.
        let can = rows.len() > 1;
        let first_col = cols.first().cloned().unwrap_or_default();
        grid_row = grid_row.child(
            col()
                .w(18.)
                .group_opacity(0., 1.)
                .hover_motion(motion::HOVER_REVEAL.anim(), motion::HOVER_REVEAL.anim())
                .children(rows.iter().enumerate().map(|(i, r)| {
                    let h = e
                        .rows
                        .get(&format!("{r}-{first_col}"))
                        .map(|g| g.bounds[3])
                        .unwrap_or(33.);
                    let (oid, r) = (f.oid(), r.clone());
                    let m = mini(format!("delrow-{}-{i}", block.id), can, "Delete row");
                    let m = if can {
                        m.on_press(move |s: &mut State| {
                            with_editor(s, &oid, |e, s| e.table_row_remove(&r, s));
                        })
                    } else {
                        m
                    };
                    row().h(h).center().child(m)
                })),
        );
        let (oid, tid) = (f.oid(), table_id.clone());
        grid_row = grid_row.child(
            add(format!("addcol-{}", block.id), "Add column")
                .w(16.)
                .self_stretch()
                .mr(2.)
                .on_press(move |s: &mut State| {
                    with_editor(s, &oid, |e, s| e.table_col_add(&tid, s));
                }),
        );
    }
    wrap = wrap.child(grid_row);
    if !ro {
        let (oid, tid) = (f.oid(), table_id.clone());
        wrap = wrap.child(
            add(format!("addrow-{}", block.id), "Add row")
                .mt(2.)
                .h(16.)
                .mr(20.)
                .on_press(move |s: &mut State| {
                    with_editor(s, &oid, |e, s| e.table_row_add(&tid, s));
                }),
        );
    }
    wrap
}

/// RelationBlock: "Name  value", edited through the shared PropertyValue.
fn render_relation(
    block: &Block,
    meta: &BTreeMap<String, String>,
    f: &Frame,
    s: &State,
    cx: &mut Cx<State>,
    layer: &mut Vec<El>,
) -> El {
    let t = f.t;
    let key = meta.get("key").cloned().unwrap_or_default();
    let rel = roostr_core::relations::space_relations(&s.store.relations, &f.space)
        .into_iter()
        .find(|r| r.key == key)
        .cloned();
    let Some(rel) = rel else {
        return row().grow(1.).child(
            text(format!("Unknown property \"{key}\""))
                .font(rune::TextStyle {
                    italic: true,
                    ..ty::ui(14.)
                })
                .color(t.muted),
        );
    };
    let value =
        f.e.object
            .as_ref()
            .and_then(|o| o.fields.get(&key))
            .cloned();
    let name = if rel.name.is_empty() {
        rel.key.clone()
    } else {
        rel.name.clone()
    };
    let format = rel.format.clone();
    let oid = f.oid();
    let value_el =
        roostr_properties::PropertyValue::new(format!("rel-value-{}", block.id), rel, value)
            .space(f.space.clone())
            .read_only(f.e.read_only)
            .on_save(move |s: &mut State, v| {
                let key = key.clone();
                with_editor(s, &oid, |e, s| {
                    let task = e.w_set_field(&key, v, s);
                    e.await_then_refresh(vec![task], s);
                });
            })
            .build(s, cx, t, layer);
    row()
        .grow(1.)
        .min_w(0.)
        .items_center()
        .gap(12.)
        .py(3.)
        .child(roostr_ui::tooltip(
            el().key(format!("rel-name-{}", block.id))
                .w(140.)
                .child(text(name).font(ty::ui(13.)).color(t.muted).truncate()),
            &format,
        ))
        .child(col().grow(1.).min_w(0.).child(value_el))
}

/// Anytype BlockType.Link: icon + name (text) or a bordered card; a ghost row when the target is gone.
fn render_link(block: &Block, meta: &BTreeMap<String, String>, f: &Frame, s: &State) -> El {
    let t = f.t;
    let store = &s.store;
    let target_id = meta.get("target").cloned().unwrap_or_default();
    let target = store.summary(&target_id).cloned();
    let card = meta.get("style").map(String::as_str) == Some("card");
    let wrap = row().grow(1.).min_w(0.).px(2.).py(3.);
    let wrap = match block.align.unwrap_or(0) {
        1 => wrap.justify_center(),
        2 => wrap.justify_end(),
        _ => wrap,
    };
    let Some(target) = target else {
        return wrap.child(
            row()
                .items_center()
                .gap(8.)
                .child(text("⌀").font(ty::ui(18.).lh(20.)).color(t.muted))
                .child(
                    text("Deleted object")
                        .font(ty::ui(15.).medium())
                        .color(t.muted),
                ),
        );
    };
    let icon = roostr_core::objects::object_icon(
        store,
        &target.icon,
        &target.type_key,
        &target.channel_id,
    );
    let name = if target.name.is_empty() {
        "Untitled".to_string()
    } else {
        target.name.clone()
    };
    let type_name = store
        .types
        .iter()
        .find(|ty| ty.key == target.type_key)
        .map(|ty| ty.name.clone())
        .unwrap_or_else(|| target.type_key.clone());
    let images = s.get::<roostr_ui::Images>();
    let icon_el = match images {
        Some(images) => roostr_ui::emoji_or_image_icon(&icon, 18., images),
        None => roostr_ui::emoji_text(&icon, ty::ui(16.)),
    };
    let open = target.id.clone();
    let click = move |s: &mut State, tx: &mut rune::Tx| {
        if tx.mods().cmd {
            roostr_core::nav::open_tab(Route::Object(open.clone()), true, s);
        } else {
            roostr_core::nav::navigate(Route::Object(open.clone()), s);
        }
    };
    let body = if card {
        // `.link-card:hover` (`transition: border-color 0.15s`).
        col()
            .key(format!("link-{}", block.id))
            .justify_center()
            .gap(2.)
            .border(1., t.border)
            .hover(t.muted.alpha(0.0))
            .radius(8.)
            .padding(16.)
            .min_w(240.)
            .cursor(Cursor::Pointer)
            .child(
                row()
                    .items_center()
                    .gap(8.)
                    .min_w(0.)
                    .child(icon_el)
                    .child(text(name).font(ty::ui(15.).medium()).color(t.fg).truncate()),
            )
            .child(text(type_name).font(ty::ui(12.).lh(16.)).color(t.muted))
            .on_press_tx(click)
    } else {
        row()
            .key(format!("link-{}", block.id))
            .items_center()
            .gap(8.)
            .min_w(0.)
            .cursor(Cursor::Pointer)
            .hover_group()
            .child(icon_el)
            .child(
                el().border_sides([0., 0., 1., 0.], t.border)
                    .child(text(name).font(ty::ui(15.).medium()).color(t.fg).truncate()),
            )
            .on_press_tx(click)
    };
    wrap.child(body)
}

/// Floating pieces: menus, toolbar, notice, rubber band, the dragged block.
fn overlays(f: &Frame, s: &State, cx: &mut Cx<State>, layer: &mut Vec<El>) {
    let (t, e) = (f.t, f.e);
    let oid = f.oid();
    let (win_w, win_h) = cx
        .layout_with("kit-root", |r| (r[2], r[3]))
        .unwrap_or((1280., 900.));
    if !e.file_notice.is_empty() {
        let w = 560f32.min(win_w - 32.);
        let o = oid.clone();
        layer.push(
            row()
                .key(format!("ed-{oid}-file-notice"))
                .absolute((win_w - w) / 2., win_h - 24. - 64.)
                .w(w)
                .items_center()
                .gap(12.)
                .px(14.)
                .py(10.)
                .border(1., t.border)
                .radius(10.)
                .fill(t.bg)
                .shadow(6., 48., Color::BLACK.alpha(0.18))
                .child(
                    text(e.file_notice.clone())
                        .font(ty::ui(13.).lh(19.))
                        .color(t.fg)
                        .wrap()
                        .grow(1.),
                )
                .child(
                    Button::new(format!("ed-{oid}-file-notice-dismiss"))
                        .outline()
                        .small()
                        .px(10.)
                        .accessibility_label("Dismiss")
                        .child(text("Dismiss").font(ty::ui(13.)).color(t.muted))
                        .on_press(move |s: &mut State| {
                            with_editor(s, &o, |e, _| e.file_notice.clear());
                        })
                        .build(t),
                ),
        );
    }
    // The caret's rect in window points, for menus opened at it.
    let caret = |cx: &mut Cx<State>, id: &str| cx.caret(&text_key(id));
    if let Some(menu) = &e.spell_menu {
        let at = caret(cx, &menu.block)
            .map(|c| (c[0], c[1] + c[3] + 6.))
            .unwrap_or_else(|| e.anchor_under(&menu.block));
        let mut m = PopupMenu::new().min_w(180.);
        if menu.options.is_empty() {
            m = m.item(
                PopupMenuItem::element(
                    text(format!("No suggestions for \"{}\"", menu.word))
                        .font(ty::ui(13.))
                        .color(t.muted)
                        .truncate()
                        .max_w(240.),
                )
                .disabled(true),
            );
        } else {
            for (i, option) in menu.options.iter().enumerate() {
                let (o, pick) = (oid.clone(), option.clone());
                m = m.item(
                    PopupMenuItem::element(
                        text(option.clone())
                            .key(format!("spell-opt-{i}"))
                            .font(ty::ui(13.).medium())
                            .color(t.fg),
                    )
                    .on_press(move |s: &mut State| {
                        with_editor(s, &o, |e, s| e.replace_misspelling(&pick, s));
                    }),
                );
            }
            m = m.separator();
        }
        let o = oid.clone();
        m = m.item(
            PopupMenuItem::element(
                text(format!("Add \"{}\" to dictionary", menu.word))
                    .key("spell-add")
                    .font(ty::ui(13.))
                    .color(t.muted),
            )
            .on_press(move |s: &mut State| {
                with_editor(s, &o, |e, s| e.spell_add_word(s));
            }),
        );
        let o = oid.clone();
        roostr_ui::DropdownMenu::new(format!("ed-{oid}-spell"))
            .menu(m)
            .open(true)
            .at(at.0, at.1)
            .on_close(move |s: &mut State| {
                with_editor(s, &o, |e, _| e.spell_menu = None);
            })
            .build(t, layer);
    }
    if let Some(rect) = e.sel_rect {
        layer.push(
            el().absolute(rect[0], rect[1])
                .size(rect[2], rect[3])
                .fill(t.block_selection)
                .border(1., roostr_ui::color::rgb(0x2aa7ee))
                .radius(2.)
                .inert(),
        );
    }
    if e.block_menu.is_some() {
        crate::block_menu::block_menu_view(s, cx, t, e, layer);
    }
    if e.link_picker.is_some() {
        crate::link_picker::link_picker_view(s, cx, t, e, layer);
    }
    if let Some(open) = &e.property_suggest {
        // PropertySuggest.svelte: a 280×320 picker kept on screen, flipped
        // above the caret when there's no room below.
        let (w, h) = (280., 320.);
        let (x, y) = open.at;
        let top = if y + h > win_h - 12. {
            (y - h - 28.).max(12.)
        } else {
            y
        };
        let (lo, eo) = (oid.clone(), oid.clone());
        let menu =
            roostr_properties::PropertySuggest::new(format!("ed-{oid}-prop-suggest"), &open.state)
                .on_state(move |s: &mut State| {
                    &mut s
                        .slot::<crate::Editors>()
                        .map
                        .get_mut(&lo)
                        .and_then(|e| e.property_suggest.as_mut())
                        .expect("the property picker is open")
                        .state
                })
                .on_event(move |s: &mut State, ev| {
                    with_editor(s, &eo, |e, s| e.property_picked(ev, s));
                })
                .build(s, cx, t, layer);
        layer.push(
            menu.absolute(x.min(win_w - w - 12.).max(8.), top)
                .w(w)
                .max_h(h),
        );
    }
    if let Some(menu) = &e.paste_menu {
        let at = caret(cx, &menu.block)
            .map(|c| (c[0], c[1] + c[3] + 6.))
            .unwrap_or((120., 126.));
        let mut m = PopupMenu::new().min_w(160.).item(
            PopupMenuItem::element(text("PASTE AS").font(ty::ui(10.)).color(t.muted).pt(2.))
                .disabled(true),
        );
        // (key, label, as a block, embed / link)
        let choices: [(&'static str, &'static str, bool, bool); 4] = [
            ("paste-embed", "Embed", true, true),
            ("paste-bookmark", "Bookmark", true, false),
            ("paste-link", "Link", false, true),
            ("paste-text", "Text", false, false),
        ];
        for (key, label, block, flag) in choices {
            if key == "paste-embed" && menu.processor.is_none() {
                continue;
            }
            let o = oid.clone();
            m = m.item(
                PopupMenuItem::element(text(label).key(key).font(ty::ui(13.)).color(t.fg))
                    .on_press(move |s: &mut State| {
                        with_editor(s, &o, |e, s| {
                            if block {
                                e.paste_as_block(flag, s)
                            } else {
                                e.paste_as_text(flag, s)
                            }
                        });
                    }),
            );
        }
        let o = oid.clone();
        roostr_ui::DropdownMenu::new(format!("ed-{oid}-paste"))
            .menu(m)
            .open(true)
            .at(at.0, at.1)
            .on_close(move |s: &mut State| {
                with_editor(s, &o, |e, s| {
                    if let Some(menu) = e.paste_menu.take() {
                        e.focus_block(&menu.block, menu.at, s);
                    }
                });
            })
            .build(t, layer);
    }
    if let Some(tb) = &e.toolbar
        && e.link_prompt.is_none()
        && e.turn_into.is_none()
        && let Some(c) = caret(cx, &tb.block)
    {
        layer.push(marks_toolbar(f, tb, c, win_w));
    }
    if let Some(open) = &e.turn_into {
        let (x, y) = open.pos;
        let (lo, eo) = (oid.clone(), oid.clone());
        let menu = roostr_properties::TypeSuggest::new(format!("ed-{oid}-turn-into"), &open.state)
            .on_state(move |s: &mut State| {
                &mut s
                    .slot::<crate::Editors>()
                    .map
                    .get_mut(&lo)
                    .and_then(|e| e.turn_into.as_mut())
                    .expect("the type picker is open")
                    .state
            })
            .on_event(move |s: &mut State, ev| {
                with_editor(s, &eo, |e, s| e.turn_into_picked(ev, s));
            })
            .build(s, cx, t, layer);
        layer.push(
            col()
                .key(format!("ed-{oid}-turn-pop"))
                .absolute((x - 130.).max(8.).min(win_w - 268.), y + 8.)
                .w(260.)
                .fill(t.kit.popover)
                .radius(radius::KIT)
                .with_shadows(shadow::kit_popover(t))
                .padding(6.)
                .child(menu),
        );
    }
    if e.link_prompt.is_some() {
        crate::link_picker::link_prompt_view(s, cx, t, e, layer);
    }
    crate::slash_menu::slash_view(s, cx, t, e, layer);
    if let Some(id) = &e.dragging
        && let Some(b) = e.block(id)
    {
        let (x, y) = e.drag_point;
        layer.push(
            el().absolute(x + 8., y + 8.)
                .max_w(360.)
                .px(10.)
                .py(6.)
                .radius(6.)
                .fill(t.panel)
                .box_shadow(shadow::drag())
                .opacity(0.95)
                .inert()
                .child(
                    text(drag_label(&b.content))
                        .font(ty::ui(15.))
                        .color(t.fg)
                        .truncate(),
                ),
        );
    }
}

/// The marks toolbar over a text selection: centred over the caret, 44pt above it.
fn marks_toolbar(f: &Frame, tb: &crate::editor::Toolbar, caret: [f32; 4], win_w: f32) -> El {
    use roostr_core::types::mark;
    let t = f.t;
    let oid = f.oid();
    let tool = |key: &'static str, tip: &'static str, label: El, wide: bool| {
        let b = Button::new(key).ghost().h(30.).text_size(14.).child(label);
        let b = if wide { b.px(8.) } else { b.w(30.).p(0.) };
        (b, tip)
    };
    let mk = |kind: u32| {
        let (o, block) = (oid.clone(), tb.block.clone());
        move |s: &mut State| {
            with_editor(s, &o, |e, s| e.apply_mark(&block, kind, None, s));
        }
    };
    let label = |s: &str, style: rune::TextStyle| text(s.to_string()).font(style).color(t.fg);
    let mut bar = row()
        .key(format!("ed-{oid}-marks-toolbar"))
        .gap(2.)
        .fill(t.panel)
        .border(1., t.border)
        .radius(8.)
        .padding(4.)
        .shadow(8., 48., Color::BLACK.alpha(0.35));
    let items: Vec<(roostr_ui::Button, &'static str)> = vec![
        {
            let (b, tip) = tool(
                "tb-bold",
                "Bold (⌘B)",
                label("B", ty::ui(14.).bold()),
                false,
            );
            (b.on_press(mk(mark::BOLD)), tip)
        },
        {
            let (b, tip) = tool(
                "tb-italic",
                "Italic (⌘I)",
                label(
                    "I",
                    rune::TextStyle {
                        italic: true,
                        ..ty::ui(14.)
                    },
                ),
                false,
            );
            (b.on_press(mk(mark::ITALIC)), tip)
        },
        {
            let u = label("U", ty::ui(14.)).spans(vec![rune::Span {
                underline: Some(t.fg),
                ..rune::Span::new(0, 1)
            }]);
            let (b, tip) = tool("tb-underline", "Underline (⌘U)", u, false);
            (b.on_press(mk(mark::UNDERLINE)), tip)
        },
        {
            let s_el = label("S", ty::ui(14.)).strike(0, 1, None);
            let (b, tip) = tool("tb-strike", "Strikethrough (⌘⇧S)", s_el, false);
            (b.on_press(mk(mark::STRIKETHROUGH)), tip)
        },
        {
            let (b, tip) = tool("tb-code", "Code (⌘E)", label("<>", ty::ui(14.)), false);
            (b.on_press(mk(mark::INLINE_CODE)), tip)
        },
        {
            let o = oid.clone();
            let (b, tip) = tool("tb-link", "Link", label("🔗", ty::ui(14.)), false);
            (
                b.on_press(move |s: &mut State| {
                    with_editor(s, &o, |e, s| e.add_link(s));
                }),
                tip,
            )
        },
        {
            let o = oid.clone();
            let pos = (caret[0], caret[1] + caret[3]);
            let (b, tip) = tool(
                "tb-object",
                "Turn into object",
                label("↗ Object", ty::ui(12.)),
                true,
            );
            (
                b.on_press(move |s: &mut State| {
                    with_editor(s, &o, |e, s| e.open_turn_into(pos, s));
                }),
                tip,
            )
        },
    ];
    for (b, tip) in items {
        bar = bar.child(roostr_ui::tooltip(b.build(t), tip));
    }
    // translateX(-50%) at the selection, 44pt above it.
    let w = 7. * 32. + 50.;
    let x = (caret[0] - w / 2.).clamp(4., (win_w - w - 4.).max(4.));
    bar.absolute(x, (caret[1] - 44.).max(4.))
}

/// Several shadows on one element.
trait WithShadows {
    fn with_shadows(self, shadows: impl IntoIterator<Item = rune::Shadow>) -> Self;
}

impl WithShadows for El {
    fn with_shadows(self, shadows: impl IntoIterator<Item = rune::Shadow>) -> Self {
        shadows.into_iter().fold(self, El::box_shadow)
    }
}

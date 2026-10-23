//! Drawing the editor (`Editor.svelte` markup + `BlockNode.svelte` +
//! `TableBlock.svelte` + `RelationBlock.svelte`): every block kind, the
//! drag rail, drop hints, selection, and the floating menus.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::toolbar::Toolbar;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::motion::{self, HoverMotion};
use roostr_core::types::{Block, BlockContent, layout, mark, pos, style};
use roostr_core::{Glyph, Route, Store, theme};

use crate::editor::{Editor, RowGeom};
use crate::file_block::FileBlock;
use crate::look::{self, Look, Squiggle};
use crate::text::RichText;
use crate::toggles;

/// The block being dragged by its handle.
#[derive(Clone)]
pub struct DraggedBlock {
    pub id: String,
    pub label: SharedString,
}

/// The floating copy that follows the pointer while a block is dragged.
pub struct DragPreview {
    label: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(360.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(theme::panel())
            .shadow(roostr_core::ui::drag_shadow())
            .opacity(0.95)
            .text_size(px(15.))
            .text_color(theme::fg())
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(self.label.clone())
    }
}

/// Data shared by every block while one frame renders.
struct Frame {
    by_id: HashMap<String, Block>,
    roots: Vec<String>,
    all_roots: Vec<String>,
    space: String,
}

impl Editor {
    fn row_id(prefix: &str, id: &str) -> ElementId {
        ElementId::Name(format!("{prefix}-{id}").into())
    }

    /// Record a row's bounds every frame (drag targeting, rubber band).
    fn geom(&self, id: &str, gutter: bool, bot: bool) -> impl IntoElement {
        let rows = if bot {
            self.bot_strips.clone()
        } else {
            self.rows.clone()
        };
        let id = id.to_string();
        canvas(
            move |bounds, _, _| {
                rows.borrow_mut()
                    .insert(id.clone(), RowGeom { bounds, gutter });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// `.gutter`: the 48px drag rail; at ≤720px it collapses to the shared
    /// 16px gutter (BlockNode.svelte `@media (max-width: 720px)`).
    pub(crate) fn gutter_w(&self) -> Pixels {
        if self.narrow { px(16.) } else { px(48.) }
    }

    /// `.handle`'s `right`: 6px, 2px in the 16px gutter (6px there pushed
    /// the 12px handle to x = -2, clipped at the screen edge).
    fn handle_right(&self) -> Pixels {
        if self.narrow { px(2.) } else { px(6.) }
    }

    /// The handle's hover reveal (`.block:hover > .gutter .handle`,
    /// `transition: opacity 0.1s`) and the state the row's on_hover feeds:
    /// none on a touch screen (`@media (hover: hover)`) or without a handle.
    fn handle_reveal(
        &self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (f32, Option<Entity<HoverMotion>>) {
        if self.read_only || roostr_core::layout::is_touch() {
            return (0., None);
        }
        let (t, state) = motion::hover(
            Self::row_id("handle-reveal", id),
            motion::HOVER_REVEAL,
            window,
            cx,
        );
        (t, Some(state))
    }

    /// The 48px rail with the drag handle (Anytype .icon.commonDnd), at
    /// `reveal` ([`Self::handle_reveal`]).
    fn gutter(
        &self,
        id: &str,
        label: SharedString,
        reveal: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editor = cx.entity();
        let open_id = id.to_string();
        let drag_id = id.to_string();
        // `@media (hover: none)`: a touch screen can't hover, so the handle
        // shows only on the ACTIVE block (its text focused), at 0.7.
        let touch = roostr_core::layout::is_touch();
        let active = touch && self.has_focus && self.active.as_deref() == Some(id);
        let dots = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .children((0..3).map(|_| {
                div()
                    .size(px(2.))
                    .rounded_full()
                    .bg(gpui_kit::transparent_black())
                    .bg(theme::muted())
            }));
        div()
            .w(self.gutter_w())
            .flex_none()
            .relative()
            .self_stretch()
            .when(!self.read_only, |d| {
                d.child(
                    div()
                        .id(Self::row_id("handle", id))
                        .absolute()
                        .right(self.handle_right())
                        .top_0()
                        .h_full()
                        .min_h(px(24.))
                        .w(px(12.))
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme::muted())
                        .cursor_grab()
                        .opacity(if active { 0.7 } else { reveal })
                        .hover(|s| {
                            s.bg(theme::hover())
                                .border_color(theme::hover())
                                .text_color(theme::fg())
                        })
                        // A `title`: a touch screen never shows it (a long
                        // press there lifts the row).
                        .when(!touch, |d| {
                            d.tooltip(roostr_core::ui::tooltip("Click for actions; drag to move"))
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let edge = this.gutter_w() - this.handle_right();
                            let at =
                                this.rows.borrow().get(&open_id).map(|g| {
                                    point(g.bounds.left() + edge + px(8.), g.bounds.top())
                                });
                            this.open_block_menu(&open_id, at.unwrap_or_default(), window, cx);
                        }))
                        .on_drag(
                            DraggedBlock { id: drag_id, label },
                            move |drag, _, _, cx| {
                                let id = drag.id.clone();
                                editor.update(cx, |e, cx| {
                                    e.begin_drag(&id);
                                    cx.notify();
                                });
                                cx.new(|_| DragPreview {
                                    label: drag.label.clone(),
                                })
                            },
                        )
                        .child(dots),
                )
            })
            .into_any_element()
    }

    /// Drop-hint lines and the selection overlay over a row.
    fn row_decor(&self, id: &str) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let zone = match &self.drop_hint {
            Some(h) if h.id == id && !h.bot => h.position,
            _ => 0,
        };
        let line = |d: Div| d.absolute().bg(theme::accent()).into_any_element();
        match zone {
            pos::TOP => out.push(line(div().left_0().right_0().top(px(-2.)).h(px(2.)))),
            pos::BOTTOM => out.push(line(div().left_0().right_0().bottom(px(-2.)).h(px(2.)))),
            pos::LEFT => out.push(line(div().top_0().bottom_0().left(px(-3.)).w(px(3.)))),
            pos::RIGHT => out.push(line(div().top_0().bottom_0().right(px(-3.)).w(px(3.)))),
            pos::INNER_FIRST => out.push(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(4.))
                    .bg(theme::block_selection())
                    .into_any_element(),
            ),
            _ => {}
        }
        if self.selected.iter().any(|s| s == id) {
            out.push(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(2.))
                    .bg(theme::block_selection())
                    .into_any_element(),
            );
        }
        out
    }

    /// The `.block` row wrapper shared by every kind; its hover feeds `hovers`.
    fn row(
        &self,
        id: &str,
        gutter: bool,
        hovers: Vec<Entity<HoverMotion>>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let down_id = id.to_string();
        let menu_id = id.to_string();
        div()
            .id(Self::row_id("row", id))
            .when(!hovers.is_empty(), |d| {
                d.on_hover(move |hovered: &bool, _, cx| {
                    for state in &hovers {
                        HoverMotion::set(state, *hovered, cx);
                    }
                })
            })
            .relative()
            .flex()
            .items_start()
            .py(px(1.))
            // `@media (max-width: 720px)`: `.block { padding-right: 16px }`.
            .when(self.narrow, |d| d.pr(px(16.)))
            .rounded(px(4.))
            .child(self.geom(id, gutter, false))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.row_mouse_down(&down_id, ev, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.row_context_menu(&menu_id, ev.position, window, cx);
                }),
            )
    }

    /// Mouse down on a row outside its text: modifiers select, else a rubber band arms.
    pub(crate) fn row_mouse_down(
        &mut self,
        id: &str,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        self.spell_menu = None;
        self.paste_menu = None;
        window.focus(&self.focus_handle, cx);
        let m = ev.modifiers;
        if m.shift || m.platform || m.control {
            if m.shift {
                self.range_select(id);
            } else {
                self.toggle_select(id);
            }
            cx.notify();
            return;
        }
        if let Some(a) = self.active.take() {
            self.flush_save(&a, cx);
            self.sync_texts();
        }
        self.toolbar = None;
        self.arm_drag(ev, false, false);
        cx.notify();
    }

    /// Right-click: a live multi-selection owns it; the focused text keeps the
    /// platform's menu (none here); anything else opens the block action menu.
    pub(crate) fn row_context_menu(
        &mut self,
        id: &str,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        if self.texts.contains_key(id) && self.open_spell_menu(id, at, cx) {
            return;
        }
        if self.selected.len() > 1 {
            let hit = if self.selected.iter().any(|s| s == id) {
                id.to_string()
            } else {
                self.selected[0].clone()
            };
            self.open_block_menu(&hit, at, window, cx);
            return;
        }
        if self.has_focus && self.active.as_deref() == Some(id) {
            return;
        }
        self.open_block_menu(id, at, window, cx);
    }

    fn input_hook(&self, cx: &mut Context<Self>) -> crate::text::InputHook {
        let editor = cx.entity();
        let focus = self.focus_handle.clone();
        Rc::new(move |bounds, window, cx| {
            window.handle_input(&focus, ElementInputHandler::new(bounds, editor.clone()), cx)
        })
    }

    /// One block's editable text.
    fn rich_text(
        &self,
        id: &str,
        look: &Look,
        align: TextAlign,
        placeholder: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(t) = self.texts.get(id) else {
            return div().into_any_element();
        };
        let mut squiggles: Vec<Squiggle> = Vec::new();
        if let Some(ranges) = self.misspelled.get(id) {
            for r in ranges.iter().filter(|r| r.end <= t.text.len()) {
                squiggles.push(Squiggle {
                    range: r.clone(),
                    color: rgb(0xe2400c).into(),
                    thickness: px(1.),
                    wavy: true,
                });
            }
        }
        if let Some(problems) = self.lint.get(id) {
            for p in problems.iter().filter(|p| p.to <= t.text.len()) {
                let color = if p.severity == crate::codelint::Severity::Error {
                    theme::red()
                } else {
                    theme::lint_warning()
                };
                squiggles.push(Squiggle {
                    range: p.from..p.to,
                    color,
                    thickness: px(1.5),
                    wavy: true,
                });
            }
        }
        if let Some(m) = &t.marked {
            squiggles.push(Squiggle {
                range: m.clone(),
                color: look.color,
                thickness: px(1.),
                wavy: false,
            });
        }
        let runs = look::runs(&t.text, &t.spans, look, &squiggles);
        let active = self.active.as_deref() == Some(id) && !self.read_only;
        let focused = active && self.has_focus;
        let placeholder = placeholder
            .filter(|_| focused && t.text.is_empty())
            .map(|p| {
                (
                    SharedString::from(p),
                    look.plain_run(p.len(), theme::muted().opacity(0.6)),
                )
            });
        let caret =
            (focused && self.caret_on && t.sel.is_empty()).then(|| (t.cursor(), theme::fg()));
        let selection = (active && !t.sel.is_empty()).then(|| (t.sel.clone(), theme::mark()));
        let el = RichText {
            key: id.to_string(),
            text: t.text.clone().into(),
            runs,
            font_size: look.size,
            line_height: look.line_height,
            align,
            placeholder,
            selection,
            caret,
            input: active.then(|| (self.focus_handle.clone(), self.input_hook(cx))),
            layouts: self.layouts.clone(),
        };
        let down = id.to_string();
        let right = id.to_string();
        div()
            .id(Self::row_id("text", id))
            .w_full()
            .when(!self.read_only, |d| d.cursor(CursorStyle::IBeam))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.text_mouse_down(&down, ev, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.row_context_menu(&right, ev.position, window, cx);
                }),
            )
            .child(el)
            .into_any_element()
    }

    fn render_block(
        &self,
        id: &str,
        f: &Frame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let block = f.by_id.get(id)?.clone();
        let c = &block.content;
        if let Some(l) = &c.layout {
            if l.style == layout::ROW {
                // Columns share the row by their `width` field (else equally).
                let kids: Vec<AnyElement> = block
                    .children_ids
                    .iter()
                    .filter_map(|cid| {
                        let width = f
                            .by_id
                            .get(cid)
                            .and_then(|b| b.fields.as_ref())
                            .and_then(|m| m.entries.get("width"))
                            .and_then(|v| v.as_number())
                            .unwrap_or(0.);
                        let el = self.render_block(cid, f, window, cx)?;
                        Some(if width > 0. {
                            div()
                                .min_w_0()
                                .w(relative(width as f32))
                                .child(el)
                                .into_any_element()
                        } else {
                            div().min_w_0().flex_1().child(el).into_any_element()
                        })
                    })
                    .collect();
                return Some(
                    div()
                        .flex()
                        .gap(px(24.))
                        .my(px(2.))
                        .children(kids)
                        .into_any_element(),
                );
            }
            if l.style == layout::COLUMN {
                let kids: Vec<AnyElement> = block
                    .children_ids
                    .iter()
                    .filter_map(|cid| self.render_block(cid, f, window, cx))
                    .collect();
                return Some(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .children(kids)
                        .into_any_element(),
                );
            }
        }
        let label = drag_label(c);
        let dragging = self.dragging.as_deref() == Some(id);
        if c.table.is_some() {
            let (reveal, hover) = self.handle_reveal(id, window, cx);
            let row = self
                .row(id, true, hover.into_iter().collect(), cx)
                .when(dragging, |d| d.opacity(0.4))
                .child(self.gutter(id, label, reveal, cx))
                .child(self.render_table(&block, f, window, cx))
                .children(self.row_decor(id));
            return Some(row.into_any_element());
        }
        if let Some(t) = &c.text {
            return Some(self.render_text_block(&block, t, f, dragging, label, window, cx));
        }
        let custom = c.custom.as_ref()?;
        let meta = custom.meta.clone().unwrap_or_default();
        let body: AnyElement = match custom.content_type.as_str() {
            "divider" => {
                let dots = meta.get("style").map(String::as_str) == Some("dots");
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(16.))
                    .relative()
                    .flex()
                    .items_center()
                    .when(dots, |d| {
                        d.justify_center().gap(px(18.)).children(
                            (0..3).map(|_| div().size(px(2.)).rounded_full().bg(theme::accent())),
                        )
                    })
                    .when(!dots, |d| {
                        d.child(div().w_full().h(px(1.)).bg(theme::border()))
                    })
                    .into_any_element()
            }
            "relation" => self.render_relation(&block, &meta, f, cx),
            "file" => match self.file_views.get(id) {
                Some(view) => div()
                    .flex_1()
                    .min_w_0()
                    .child(view.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            },
            "embed" => self.render_embed(&meta, id, cx),
            "bookmark" => render_bookmark(&meta, id),
            "link" => self.render_link(&block, &meta, f, window, cx),
            other => {
                // Unknown custom content: a chip, children nested below.
                let kids: Vec<AnyElement> = block
                    .children_ids
                    .iter()
                    .filter_map(|cid| self.render_block(cid, f, window, cx))
                    .collect();
                let chip = div()
                    .relative()
                    .flex()
                    .py(px(1.))
                    .child(self.geom(id, false, false))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .border_1()
                            .border_color(theme::border())
                            .rounded(px(6.))
                            .px(px(8.))
                            .py(px(2.))
                            .child(other.to_string()),
                    )
                    .children(self.row_decor(id));
                return Some(
                    div()
                        .relative()
                        .rounded(px(4.))
                        .child(chip)
                        .when(!kids.is_empty(), |d| {
                            d.child(div().pl(px(26.)).children(kids))
                        })
                        .into_any_element(),
                );
            }
        };
        let (reveal, hover) = self.handle_reveal(id, window, cx);
        let row = self
            .row(id, true, hover.into_iter().collect(), cx)
            .when(dragging, |d| d.opacity(0.4))
            .child(self.gutter(id, label, reveal, cx))
            .child(body)
            .children(self.row_decor(id));
        Some(row.into_any_element())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_text_block(
        &self,
        block: &Block,
        t: &roostr_core::types::TextContent,
        f: &Frame,
        dragging: bool,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = block.id.as_str();
        let s = t.style;
        let checked = t.checked.unwrap_or(false);
        let look = Look::for_style(s, checked, t.color.as_deref().unwrap_or(""));
        let align = match block.align.unwrap_or(0) {
            1 => TextAlign::Center,
            2 => TextAlign::Right,
            _ => TextAlign::Left,
        };
        let is_toggle = s == style::TOGGLE;
        let open = is_toggle && toggles::is_toggle_open(&self.object_id, id, cx);
        // `.toggle-arrow` (`transition: transform 0.2s ease`); the opened
        // children fade in with it and leave at once (they may hold the caret).
        let turn = if is_toggle {
            motion::progress(
                Self::row_id("toggle-turn", id),
                open,
                motion::DISCLOSURE,
                window,
                cx,
            )
        } else {
            1.
        };
        let placeholder = if s == style::TITLE {
            "Untitled"
        } else {
            "Type / for commands"
        };
        let text = self.rich_text(id, &look, align, Some(placeholder), cx);
        let mut text_box = div().flex_1().min_w(px(60.)).px(px(2.)).py(px(3.));
        match s {
            style::QUOTE => {
                text_box = text_box
                    .border_l(px(3.))
                    .border_color(theme::accent())
                    .pl(px(12.))
            }
            style::CODE => {
                text_box = text_box
                    .bg(theme::panel())
                    .rounded(px(6.))
                    .px(px(10.))
                    .py(px(8.))
            }
            style::CALLOUT => {
                text_box = text_box
                    .bg(theme::panel())
                    .rounded(px(8.))
                    .px(px(12.))
                    .py(px(10.))
            }
            _ => {}
        }
        let (reveal, handle_hover) = self.handle_reveal(id, window, cx);
        let mut hovers: Vec<_> = handle_hover.into_iter().collect();
        // `.block:hover .code-copy` (`transition: opacity 0.12s`).
        let copy_reveal = if s == style::CODE {
            let (t, state) =
                motion::hover(Self::row_id("copy-reveal", id), motion::FADE, window, cx);
            hovers.push(state);
            t
        } else {
            0.
        };
        let mut row = self
            .row(id, true, hovers, cx)
            .child(self.gutter(id, label, reveal, cx));
        match s {
            style::CHECKBOX => {
                let toggle_id = id.to_string();
                row = row.child(
                    div()
                        .id(Self::row_id("check", id))
                        .size(px(24.))
                        .flex_none()
                        .mt(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(!self.read_only, |d| d.cursor_pointer())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_checked(&toggle_id, !checked, cx)
                        }))
                        .child(if checked {
                            div()
                                .size(px(16.))
                                .rounded_full()
                                .bg(theme::accent())
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(Glyph::Check)
                                        .size(px(11.))
                                        .text_color(theme::white()),
                                )
                        } else {
                            div()
                                .size(px(15.))
                                .rounded_full()
                                .border_1()
                                .border_color(theme::muted())
                                .hover(|s| s.bg(rgba(0x4f4f4f14)))
                        }),
                );
            }
            style::BULLET => {
                row = row.child(
                    div()
                        .pt(px(4.))
                        .text_color(theme::muted())
                        .flex_none()
                        .text_size(px(16.))
                        .line_height(px(19.))
                        .child("•"),
                )
            }
            style::NUMBERED => {
                let n = self.ordinal(id, f);
                row = row.child(
                    div()
                        .pt(px(4.))
                        .text_color(theme::muted())
                        .flex_none()
                        .text_size(px(16.))
                        .line_height(px(19.))
                        .child(format!("{n}.")),
                );
            }
            style::TOGGLE => {
                let flip = id.to_string();
                row = row.child(
                    div()
                        .id(Self::row_id("toggle", id))
                        .size(px(24.))
                        .flex_none()
                        .mr(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(theme::fg())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| this.flip_toggle(&flip, cx)))
                        .child(
                            Icon::new(Glyph::ChevronRight)
                                .size(px(16.))
                                .when(turn > 0., |i| {
                                    i.rotate(radians(std::f32::consts::FRAC_PI_2 * turn))
                                }),
                        ),
                );
            }
            _ => {}
        }
        row = row.child(text_box.child(text));
        if s == style::CODE {
            let copied = self.copied.contains(id);
            let copy_id = id.to_string();
            row = row.child(
                div()
                    .id(Self::row_id("copy", id))
                    .absolute()
                    .top(px(7.))
                    .right(px(6.))
                    .w(px(58.))
                    .px(px(8.))
                    .py(px(2.))
                    .text_size(px(11.))
                    .text_center()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::panel())
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .opacity(if copied { 1. } else { copy_reveal })
                    .hover(|s| s.text_color(theme::fg()).bg(theme::hover()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| this.copy_code(&copy_id, cx)))
                    .child(if copied { "Copied" } else { "Copy" }),
            );
        }
        row = row.children(self.row_decor(id));

        let mut tree = div()
            .relative()
            .rounded(px(4.))
            .when(dragging, |d| d.opacity(0.4));
        if let Some(bg) = block.background_color.as_deref().and_then(look::css_color) {
            tree = tree.bg(bg);
        }
        tree = tree.child(row);
        if s == style::CODE
            && let Some(problems) = self.lint.get(id)
            && !problems.is_empty()
        {
            tree = tree.child(
                div()
                    .ml(self.gutter_w())
                    .mt(px(4.))
                    .mb(px(2.))
                    .px(px(10.))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .children(problems.iter().map(|p| {
                        let dot = if p.severity == crate::codelint::Severity::Error {
                            theme::red()
                        } else {
                            theme::lint_warning()
                        };
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(6.))
                            .text_color(theme::muted())
                            .child(div().text_size(px(9.)).text_color(dot).child("●"))
                            .child(
                                div().child(
                                    div()
                                        .font_family(theme::fonts::MONO)
                                        .child(format!("Line {}:{}", p.line, p.col)),
                                ),
                            )
                            .child(div().child(p.message.clone()))
                    })),
            );
        }
        if !block.children_ids.is_empty() && (!is_toggle || open) {
            let kids: Vec<AnyElement> = block
                .children_ids
                .iter()
                .filter_map(|cid| self.render_block(cid, f, window, cx))
                .collect();
            tree = tree.child(div().pl(px(26.)).opacity(turn).children(kids));
            // Anytype targetBot: a thin strip below the children; a drop there lands after the subtree.
            let over = matches!(&self.drop_hint, Some(h) if h.id == block.id && h.bot);
            tree = tree.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(-4.))
                    .h(px(10.))
                    .child(self.geom(id, false, true))
                    .when(over, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .h(px(2.))
                                .bg(theme::accent()),
                        )
                    }),
            );
        } else if is_toggle && open && block.children_ids.is_empty() && !self.read_only {
            let empty_id = id.to_string();
            tree = tree.child(
                div()
                    .id(Self::row_id("empty-toggle", id))
                    .ml(px(30.))
                    .opacity(turn)
                    .py(px(2.))
                    .text_size(px(15.))
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .hover(|s| s.text_color(theme::fg()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.on_empty_toggle(&empty_id, cx)),
                    )
                    .child("Empty toggle. Click or drop Block inside"),
            );
        }
        tree.into_any_element()
    }

    /// A numbered item's number: its place in the run of numbered siblings.
    fn ordinal(&self, id: &str, f: &Frame) -> usize {
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
                .get(sid)
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
    fn render_table(
        &self,
        block: &Block,
        f: &Frame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (cols, rows) = self.table_shape(&block.id);
        let table_id = block.id.clone();
        let ro = self.read_only;
        // `.table-wrap:hover` reveals the gutters and add edges
        // (`transition: opacity 0.1s`).
        let (reveal, hover) = if ro {
            (0., None)
        } else {
            let (t, state) = motion::hover(
                Self::row_id("table-reveal", &block.id),
                motion::HOVER_REVEAL,
                window,
                cx,
            );
            (t, Some(state))
        };
        let mini = |id: ElementId, enabled: bool| {
            div()
                .id(id)
                .px(px(4.))
                .text_size(px(12.))
                .line_height(px(14.))
                .text_color(theme::muted())
                .when(enabled, |d| {
                    d.cursor_pointer().hover(|s| s.text_color(theme::red()))
                })
                .when(!enabled, |d| d.opacity(0.3))
                .child("×")
        };
        let mut wrap = div()
            .id(Self::row_id("table", &block.id))
            .when_some(hover, |d, state| {
                d.on_hover(move |hovered: &bool, _, cx| HoverMotion::set(&state, *hovered, cx))
            })
            .flex_1()
            .min_w_0()
            .py(px(4.))
            .flex()
            .flex_col();
        if !ro {
            let can = cols.len() > 1;
            wrap = wrap.child(
                div()
                    .flex()
                    .h(px(14.))
                    .mr(px(20.))
                    .opacity(reveal)
                    .children(cols.iter().enumerate().map(|(i, col)| {
                        let (t, c) = (table_id.clone(), col.clone());
                        div().flex_1().flex().justify_center().child(
                            mini(
                                ElementId::Name(format!("delcol-{}-{i}", table_id).into()),
                                can,
                            )
                            .tooltip(roostr_core::ui::tooltip("Delete column"))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .when(can, |d| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    this.table_col_remove(&t, &c, cx)
                                }))
                            }),
                        )
                    })),
            );
        }
        let mut grid = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_l_1()
            .border_color(theme::border());
        for (ri, row_id) in rows.iter().enumerate() {
            let header = f
                .by_id
                .get(row_id)
                .and_then(|b| b.content.table_row.as_ref())
                .and_then(|r| r.is_header)
                .unwrap_or(false);
            let mut tr = div().flex().w_full();
            for col in &cols {
                let cell = format!("{row_id}-{col}");
                let has_text = f.by_id.get(&cell).is_some_and(|b| b.content.text.is_some());
                let look = Look::table_cell(header);
                tr =
                    tr.child(
                        div()
                            .flex_1()
                            .flex_basis(px(0.))
                            .min_w(px(60.))
                            .border_r_1()
                            .border_b_1()
                            .border_color(theme::border())
                            .when(header, |d| d.bg(theme::hover()))
                            .relative()
                            .child(self.geom(&cell, false, false))
                            .children(self.row_decor(&cell))
                            .when(has_text, |d| {
                                d.child(
                                    div().px(px(8.)).py(px(6.)).min_h(px(20.)).child(
                                        self.rich_text(&cell, &look, TextAlign::Left, None, cx),
                                    ),
                                )
                            }),
                    );
            }
            let _ = ri;
            grid = grid.child(tr);
        }
        let mut grid_row = div().flex().items_stretch().gap(px(2.)).child(grid);
        if !ro {
            // Row deletes sit in a column beside the grid, one per row.
            let can = rows.len() > 1;
            grid_row = div().flex().items_stretch().gap(px(2.)).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .child(grid_row.flex_1())
                    .child(
                        div()
                            .w(px(18.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .opacity(reveal)
                            .children(rows.iter().enumerate().map(|(i, r)| {
                                let r = r.clone();
                                let h = self
                                    .rows
                                    .borrow()
                                    .get(&format!(
                                        "{r}-{}",
                                        cols.first().cloned().unwrap_or_default()
                                    ))
                                    .map(|g| g.bounds.size.height)
                                    .unwrap_or(px(33.));
                                div().h(h).flex().items_center().justify_center().child(
                                    mini(
                                        ElementId::Name(format!("delrow-{}-{i}", block.id).into()),
                                        can,
                                    )
                                    .tooltip(roostr_core::ui::tooltip("Delete row"))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .when(can, |d| {
                                        d.on_click(cx.listener(move |this, _, _, cx| {
                                            this.table_row_remove(&r, cx)
                                        }))
                                    }),
                                )
                            })),
                    ),
            );
            let t = table_id.clone();
            grid_row = grid_row.child(
                div()
                    .id(Self::row_id("addcol", &block.id))
                    .w(px(16.))
                    .mr(px(2.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_dashed()
                    .border_color(theme::border())
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .opacity(reveal)
                    .hover(|s| s.text_color(theme::accent()).border_color(theme::accent()))
                    .tooltip(roostr_core::ui::tooltip("Add column"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| this.table_col_add(&t, cx)))
                    .child("+"),
            );
        }
        wrap = wrap.child(grid_row);
        if !ro {
            let t = table_id.clone();
            wrap = wrap.child(
                div()
                    .id(Self::row_id("addrow", &block.id))
                    .mt(px(2.))
                    .h(px(16.))
                    .mr(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_dashed()
                    .border_color(theme::border())
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .opacity(reveal)
                    .hover(|s| s.text_color(theme::accent()).border_color(theme::accent()))
                    .tooltip(roostr_core::ui::tooltip("Add row"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| this.table_row_add(&t, cx)))
                    .child("+"),
            );
        }
        wrap.into_any_element()
    }

    /// RelationBlock: "Name  value", edited through the shared PropertyValue.
    fn render_relation(
        &self,
        block: &Block,
        meta: &std::collections::BTreeMap<String, String>,
        f: &Frame,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = meta.get("key").cloned().unwrap_or_default();
        let store = Store::global(cx).read(cx);
        let rel = roostr_core::relations::space_relations(&store.relations, &f.space)
            .into_iter()
            .find(|r| r.key == key)
            .cloned();
        let Some(rel) = rel else {
            return div()
                .flex_1()
                .italic()
                .text_color(theme::muted())
                .child(format!("Unknown property \"{key}\""))
                .into_any_element();
        };
        let value = self
            .object
            .as_ref()
            .and_then(|o| o.fields.get(&key))
            .cloned();
        let editor = cx.entity();
        let save_key = key.clone();
        let name = if rel.name.is_empty() {
            rel.key.clone()
        } else {
            rel.name.clone()
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(3.))
            .text_size(px(14.))
            .child(
                div()
                    .id(Self::row_id("rel-name", &block.id))
                    .w(px(140.))
                    .flex_none()
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .tooltip(roostr_core::ui::tooltip(rel.format.clone()))
                    .child(name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        roostr_properties::PropertyValue::new(
                            Self::row_id("rel-value", &block.id),
                            rel,
                            value,
                        )
                        .space(f.space.clone())
                        .read_only(self.read_only)
                        .on_save(move |v, _, cx| {
                            let v = v.clone();
                            let key = save_key.clone();
                            editor.update(cx, |e, cx| {
                                let task = e.w_set_field(&key, v, cx);
                                e.await_then_refresh(vec![task], cx);
                            });
                        }),
                    ),
            )
            .into_any_element()
    }

    /// Anytype BlockType.Link: icon + name (text) or a bordered card; a ghost row when the target is gone.
    fn render_link(
        &self,
        block: &Block,
        meta: &std::collections::BTreeMap<String, String>,
        f: &Frame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let store = Store::global(cx).read(cx);
        let target_id = meta.get("target").cloned().unwrap_or_default();
        let target = store.summary(&target_id).cloned();
        let card = meta.get("style").map(String::as_str) == Some("card");
        let justify = block.align.unwrap_or(0);
        let wrap = div()
            .flex_1()
            .min_w_0()
            .flex()
            .px(px(2.))
            .py(px(3.))
            .when(justify == 1, |d| d.justify_center())
            .when(justify == 2, |d| d.justify_end());
        let Some(target) = target else {
            return wrap
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_color(theme::muted())
                        .child(div().text_size(px(18.)).line_height(px(20.)).child("⌀"))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Deleted object"),
                        ),
                )
                .into_any_element();
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
            .find(|t| t.key == target.type_key)
            .map(|t| t.name.clone())
            .unwrap_or_else(|| target.type_key.clone());
        let _ = f;
        let open = target.id.clone();
        let group: SharedString = format!("link-{}", block.id).into();
        let click = move |ev: &ClickEvent, _: &mut Window, cx: &mut App| {
            if ev.modifiers().platform {
                roostr_core::nav::open_tab(Route::Object(open.clone()), true, cx);
            } else {
                roostr_core::nav::navigate(Route::Object(open.clone()), cx);
            }
        };
        let body = if card {
            // `.link-card:hover` (`transition: border-color 0.15s`).
            let (tint, hover) = motion::hover(
                Self::row_id("link-tint", &block.id),
                motion::CONTROL,
                window,
                cx,
            );
            div()
                .id(Self::row_id("link", &block.id))
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(2.))
                .border_1()
                .border_color(motion::mix(theme::border(), theme::muted(), tint))
                .rounded(px(8.))
                .p(px(16.))
                .min_w(px(240.))
                .max_w_full()
                .text_color(theme::fg())
                .cursor_pointer()
                .on_hover(move |hovered: &bool, _, cx| HoverMotion::set(&hover, *hovered, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(click)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_w_0()
                        .child(
                            div()
                                .flex_none()
                                .child(roostr_core::ui::emoji_or_image_icon(&icon, px(18.))),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(name),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(theme::muted())
                        .child(type_name),
                )
        } else {
            div()
                .id(Self::row_id("link", &block.id))
                .group(group.clone())
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .text_color(theme::fg())
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(click)
                .child(
                    div()
                        .flex_none()
                        .child(roostr_core::ui::emoji_or_image_icon(&icon, px(18.))),
                )
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .border_b_1()
                        .border_color(theme::border())
                        .group_hover(group, |s| s.border_color(theme::fg()))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(name),
                )
        };
        wrap.child(body).into_any_element()
    }

    /// `.touch-ghost`: the lifted row's copy, riding just ABOVE the
    /// fingertip so neither the thumb nor the copy hides the drop line under
    /// the finger, while the original stays dimmed in place.
    fn touch_ghost(&self) -> Option<AnyElement> {
        let lift = self.touch_lift.as_ref()?;
        let origin = self.bounds.get().origin;
        Some(
            deferred(
                div()
                    .absolute()
                    .left(lift.row.left() + (lift.at.x - lift.x0) - origin.x)
                    .top(lift.at.y - lift.row.size.height - px(12.) - origin.y)
                    .w(lift.row.size.width)
                    .min_h(lift.row.size.height)
                    .pl(self.gutter_w())
                    .py(px(1.))
                    .rounded(px(6.))
                    .bg(theme::panel())
                    .shadow(vec![BoxShadow {
                        color: hsla(0., 0., 0., 0.45),
                        offset: point(px(0.), px(10.)),
                        blur_radius: px(28.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .opacity(0.95)
                    .text_size(px(15.))
                    .text_color(theme::fg())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(lift.label.clone()),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// Floating pieces: menus, toolbar, notice, rubber band.
    fn overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let viewport = window.viewport_size();
        if !self.file_notice.is_empty() {
            let w = px(560.).min(viewport.width - px(32.));
            out.push(
                deferred(
                    anchored()
                        .position(point(
                            (viewport.width - w) / 2.,
                            viewport.height - px(24.) - px(64.),
                        ))
                        .child(
                            div()
                                .id("file-notice")
                                .occlude()
                                .w(w)
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .px(px(14.))
                                .py(px(10.))
                                .border_1()
                                .border_color(theme::border())
                                .rounded(px(10.))
                                .bg(theme::bg())
                                .shadow(vec![BoxShadow {
                                    color: hsla(0., 0., 0., 0.18),
                                    offset: point(px(0.), px(6.)),
                                    blur_radius: px(24.),
                                    spread_radius: px(0.),
                                    inset: false,
                                }])
                                .text_color(theme::fg())
                                .text_size(px(13.))
                                .line_height(px(19.))
                                .child(div().flex_1().child(self.file_notice.clone()))
                                .child(
                                    div()
                                        .id("file-notice-dismiss")
                                        .flex_none()
                                        .border_1()
                                        .border_color(theme::border())
                                        .rounded(px(7.))
                                        .px(px(10.))
                                        .py(px(3.))
                                        .text_color(theme::muted())
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(theme::fg()))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.file_notice.clear();
                                            cx.notify();
                                        }))
                                        .child("Dismiss"),
                                ),
                        ),
                )
                .with_priority(1)
                .into_any_element(),
            );
        }
        if let Some(menu) = self.spell_menu.clone() {
            let popup = match menu.popup.clone() {
                Some(popup) => popup,
                None => {
                    let popup = self.build_spell_popup(&menu, window, cx);
                    if let Some(m) = self.spell_menu.as_mut() {
                        m.popup = Some(popup.clone());
                    }
                    popup
                }
            };
            out.push(
                deferred(
                    anchored()
                        .position(point(menu.pos.x, menu.pos.y + px(6.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            popup_frame(
                                "spell-menu",
                                px(8.),
                                vec![BoxShadow {
                                    color: hsla(0., 0., 0., 0.5),
                                    offset: point(px(0.), px(8.)),
                                    blur_radius: px(24.),
                                    spread_radius: px(0.),
                                    inset: false,
                                }],
                            )
                            .child(popup),
                        ),
                )
                .with_priority(5)
                .into_any_element(),
            );
        }
        if let Some(rect) = self.sel_rect {
            out.push(
                deferred(
                    anchored().position(rect.origin).child(
                        div()
                            .w(rect.size.width)
                            .h(rect.size.height)
                            .bg(theme::block_selection())
                            .border_1()
                            .border_color(rgb(0x2aa7ee))
                            .rounded(px(2.)),
                    ),
                )
                .with_priority(1)
                .into_any_element(),
            );
        }
        if let Some(open) = &self.block_menu {
            out.push(open.menu.clone().into_any_element());
        }
        if let Some((_, picker, at, _)) = &self.link_picker {
            out.push(
                anchored()
                    .position(*at)
                    .snap_to_window_with_margin(px(8.))
                    .child(picker.clone())
                    .into_any_element(),
            );
        }
        if let Some((_, view, at, _)) = &self.property_suggest {
            // PropertySuggest.svelte: a 280×320 picker kept on screen, flipped
            // above the caret when there's no room below.
            let (w, h) = (px(280.), px(320.));
            let top = if at.y + h > viewport.height - px(12.) {
                (at.y - h - px(28.)).max(px(12.))
            } else {
                at.y
            };
            out.push(
                anchored()
                    .position(point(at.x.min(viewport.width - w - px(12.)), top))
                    .snap_to_window_with_margin(px(8.))
                    .child(view.clone())
                    .into_any_element(),
            );
        }
        if let Some(menu) = self.paste_menu.clone() {
            let popup = match menu.popup.clone() {
                Some(popup) => popup,
                None => {
                    let popup = self.build_paste_popup(&menu, window, cx);
                    if let Some(m) = self.paste_menu.as_mut() {
                        m.popup = Some(popup.clone());
                    }
                    popup
                }
            };
            out.push(
                deferred(
                    anchored()
                        .position(menu.pos)
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            popup_frame(
                                "paste-menu",
                                px(10.),
                                vec![BoxShadow {
                                    color: hsla(0., 0., 0., 0.45),
                                    offset: point(px(0.), px(12.)),
                                    blur_radius: px(32.),
                                    spread_radius: px(0.),
                                    inset: false,
                                }],
                            )
                            .child(popup),
                        ),
                )
                .with_priority(5)
                .into_any_element(),
            );
        }
        if let Some(tb) = self.toolbar.clone() {
            let block = tb.block.clone();
            let mk = |kind: u32| {
                let block = block.clone();
                cx.listener(move |this: &mut Editor, _: &ClickEvent, _, cx| {
                    this.apply_mark(&block, kind, None, cx)
                })
            };
            let bar = Toolbar::new("marks-toolbar")
                .h_auto()
                .gap(px(2.))
                .bg(theme::panel())
                .border_1()
                .border_color(theme::border())
                .rounded(px(8.))
                .p(px(4.))
                .shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., 0.35),
                    offset: point(px(0.), px(8.)),
                    blur_radius: px(24.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .content(tool_button(
                    "tb-bold",
                    "Bold (⌘B)",
                    div()
                        .font_weight(FontWeight::BOLD)
                        .child("B")
                        .into_any_element(),
                    mk(mark::BOLD),
                ))
                .content(tool_button(
                    "tb-italic",
                    "Italic (⌘I)",
                    div().italic().child("I").into_any_element(),
                    mk(mark::ITALIC),
                ))
                .content(tool_button(
                    "tb-underline",
                    "Underline (⌘U)",
                    div().underline().child("U").into_any_element(),
                    mk(mark::UNDERLINE),
                ))
                .content(tool_button(
                    "tb-strike",
                    "Strikethrough",
                    div().line_through().child("S").into_any_element(),
                    mk(mark::STRIKETHROUGH),
                ))
                .content(tool_button(
                    "tb-code",
                    "Code (⌘E)",
                    div().child("<>").into_any_element(),
                    mk(mark::INLINE_CODE),
                ))
                .content(tool_button(
                    "tb-link",
                    "Link",
                    div().child("🔗").into_any_element(),
                    cx.listener(|this, _, window, cx| this.add_link(window, cx)),
                ))
                .content(tool_button_wide(
                    "tb-object",
                    "Turn into object",
                    div()
                        .text_size(px(12.))
                        .whitespace_nowrap()
                        .child("↗ Object")
                        .into_any_element(),
                    cx.listener(|this, _, window, cx| this.open_turn_into(window, cx)),
                ));
            let bar = div().occlude().child(bar);
            // translateX(-50%) at the selection's center, 44px above it.
            let w = px(7. * 32. + 50.);
            out.push(
                deferred(
                    anchored()
                        .position(point(tb.x - w / 2., tb.y - px(44.)))
                        .snap_to_window_with_margin(px(4.))
                        .child(bar),
                )
                .with_priority(4)
                .into_any_element(),
            );
        }
        if let Some((tb, view, _)) = &self.turn_into {
            out.push(
                anchored()
                    .position(point(px(8.).max(tb.x - px(130.)), tb.y + px(8.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("turn-pop")
                            .occlude()
                            .w(px(260.))
                            .bg(theme::panel())
                            .border_1()
                            .border_color(theme::border())
                            .rounded(px(10.))
                            .p(px(6.))
                            .shadow(vec![BoxShadow {
                                color: hsla(0., 0., 0., 0.35),
                                offset: point(px(0.), px(8.)),
                                blur_radius: px(24.),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                            .child(view.clone()),
                    )
                    .into_any_element(),
            );
        }
        // Pickers with inputs stay on the focus path (no `deferred`): their keys and actions must reach them.
        if let Some((tb, prompt, _)) = &self.link_prompt {
            out.push(
                anchored()
                    .position(point(tb.x - px(160.), tb.y - px(130.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(prompt.clone())
                    .into_any_element(),
            );
        }
        if let Some(slash) = self.render_slash(window, cx) {
            out.push(slash);
        }
        out
    }

    fn table_row_add(&mut self, table: &str, cx: &mut Context<Self>) {
        let (oid, t) = (self.object_id.clone(), table.to_string());
        let task = self
            .writes
            .run(cx, move |cx| roostr_core::api::table::row_add(&oid, &t, cx));
        self.await_then_refresh(vec![task], cx);
    }

    fn table_col_add(&mut self, table: &str, cx: &mut Context<Self>) {
        let (oid, t) = (self.object_id.clone(), table.to_string());
        let task = self
            .writes
            .run(cx, move |cx| roostr_core::api::table::col_add(&oid, &t, cx));
        self.await_then_refresh(vec![task], cx);
    }

    fn table_row_remove(&mut self, row: &str, cx: &mut Context<Self>) {
        let task = self.w_block_remove(row, cx);
        self.await_then_refresh(vec![task], cx);
    }

    fn table_col_remove(&mut self, table: &str, col: &str, cx: &mut Context<Self>) {
        let (oid, t, c) = (self.object_id.clone(), table.to_string(), col.to_string());
        let task = self.writes.run(cx, move |cx| {
            roostr_core::api::table::col_remove(&oid, &t, &c, cx)
        });
        self.await_then_refresh(vec![task], cx);
    }

    /// Keep one FileBlock view per file block (they hold the loaded bytes).
    fn sync_file_views(&mut self, cx: &mut Context<Self>) {
        let mut live = std::collections::HashSet::new();
        for b in &self.blocks {
            let Some(custom) = &b.content.custom else {
                continue;
            };
            if custom.content_type != "file" {
                continue;
            }
            live.insert(b.id.clone());
            let meta = custom.meta.clone().unwrap_or_default();
            match self.file_views.get(&b.id) {
                Some(view) => view.update(cx, |v, cx| v.set_meta(meta, self.read_only, cx)),
                None => {
                    let editor = cx.entity().downgrade();
                    let block_id = b.id.clone();
                    let ro = self.read_only;
                    let view = cx.new(|cx| FileBlock::new(block_id, meta, ro, editor, cx));
                    self.file_views.insert(b.id.clone(), view);
                }
            }
        }
        self.file_views.retain(|id, _| live.contains(id));
    }

    fn update_present_relations(&mut self, cx: &App) {
        let store = Store::global(cx).read(cx);
        let fields = self.object_fields();
        let space = roostr_core::relations::object_space_id(store, &fields);
        self.present_relations = roostr_core::relations::space_relations(&store.relations, &space)
            .into_iter()
            .filter(|r| {
                !r.hidden
                    && !roostr_core::relations::is_reserved_key(&r.key)
                    && fields.contains_key(&r.key)
            })
            .map(|r| (r.key.clone(), r.name.clone(), r.format.clone()))
            .collect();
    }
}

impl Editor {
    /// The poster for an embed URL, fetched once per URL in the background.
    fn poster(
        &self,
        processor: crate::embed::Processor,
        url: &str,
        cx: &mut Context<Self>,
    ) -> Option<crate::embed::Poster> {
        if let Some(p) = self.posters.borrow().get(url) {
            return p.clone();
        }
        self.posters.borrow_mut().insert(url.to_string(), None);
        let url = url.to_string();
        cx.spawn(async move |this, cx| {
            let fetch_url = url.clone();
            let poster = roostr_core::io::spawn_async(cx, async move {
                crate::embed::fetch_poster(processor, &fetch_url).await
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.posters.borrow_mut().insert(url, Some(poster));
                cx.notify();
            });
        })
        .detach();
        None
    }

    /// A web embed (YouTube, Vimeo, Spotify). GPUI has no web view: the
    /// player is a card with the player's still and title that opens the
    /// URL in the browser; sized like the iframe (16:9, Spotify 152px).
    fn render_embed(
        &self,
        meta: &std::collections::BTreeMap<String, String>,
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let url = meta.get("url").cloned().unwrap_or_default();
        let processor = meta
            .get("processor")
            .and_then(|p| crate::embed::Processor::from_key(p));
        let poster = processor
            .and_then(|p| self.poster(p, &url, cx))
            .unwrap_or_default();
        let label = processor.map_or("Embed", |p| p.label());
        let open = url.clone();
        let card = div()
            .id(ElementId::Name(format!("embed-{id}").into()))
            .flex_1()
            .min_w_0()
            .my(px(4.))
            .rounded(px(10.))
            .overflow_hidden()
            .relative()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| cx.open_url(&open));
        let play = |size: f32, bg: Hsla, fg: Hsla| {
            div()
                .size(px(size))
                .flex_none()
                .rounded_full()
                .bg(bg)
                .flex()
                .items_center()
                .justify_center()
                .text_color(fg)
                .text_size(px(size * 0.4))
                .child("▶")
        };
        if processor == Some(crate::embed::Processor::Spotify) {
            // Spotify's compact player: cover art, title, provider, play.
            return card
                .h(px(152.))
                .bg(theme::panel())
                .flex()
                .items_center()
                .gap(px(16.))
                .p(px(16.))
                .child(
                    div()
                        .size(px(120.))
                        .flex_none()
                        .rounded(px(8.))
                        .overflow_hidden()
                        .bg(theme::hover())
                        .when_some(poster.thumb, |d, t| {
                            d.child(
                                img(SharedUri::from(t))
                                    .size_full()
                                    .rounded(px(8.))
                                    .object_fit(ObjectFit::Cover),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_size(px(15.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme::fg())
                                        .truncate()
                                        .child(poster.title.unwrap_or_else(|| url.clone())),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(theme::muted())
                                        .child(label),
                                ),
                        )
                        .child(div().flex().justify_end().child(play(
                            32.,
                            theme::white(),
                            rgb(0x000000).into(),
                        ))),
                )
                .into_any_element();
        }
        card.aspect_ratio(16. / 9.)
            .bg(rgb(0x000000))
            .when_some(poster.thumb, |d, t| {
                d.child(
                    img(SharedUri::from(t))
                        .absolute()
                        .inset_0()
                        .size_full()
                        .rounded(px(10.))
                        .object_fit(ObjectFit::Cover),
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .child(play(56., hsla(0., 0., 0., 0.6), theme::white()))
                    .child(
                        div()
                            .max_w(relative(0.9))
                            .px(px(10.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .bg(hsla(0., 0., 0., 0.6))
                            .text_size(px(12.))
                            .text_color(theme::white())
                            .truncate()
                            .child(format!(
                                "{label} · {}",
                                poster.title.unwrap_or_else(|| url.clone())
                            )),
                    ),
            )
            .into_any_element()
    }
}

/// What a dragged block shows: its text, else its custom content type.
pub(crate) fn drag_label(c: &BlockContent) -> SharedString {
    c.text
        .as_ref()
        .map(|t| t.text.clone())
        .unwrap_or_else(|| {
            c.custom
                .as_ref()
                .map(|c| c.content_type.clone())
                .unwrap_or_default()
        })
        .into()
}

/// A bookmark card: favicon, title (else host), URL; opens in the browser.
fn render_bookmark(meta: &std::collections::BTreeMap<String, String>, id: &str) -> AnyElement {
    let url = meta.get("url").cloned().unwrap_or_default();
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.clone());
    let title = meta
        .get("title")
        .cloned()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| host.clone());
    let favicon = format!("https://www.google.com/s2/favicons?domain={host}&sz=32");
    let open = url.clone();
    div()
        .id(ElementId::Name(format!("bookmark-{id}").into()))
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(10.))
        .border_1()
        .border_color(theme::border())
        .rounded(px(10.))
        .px(px(12.))
        .py(px(10.))
        .my(px(4.))
        .cursor_pointer()
        .text_color(theme::fg())
        .hover(|s| s.bg(theme::hover()))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| cx.open_url(&open))
        .child(
            img(SharedUri::from(favicon))
                .size(px(20.))
                .rounded(px(4.))
                .flex_none(),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight(550.))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme::muted())
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(url),
                ),
        )
        .into_any_element()
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.narrow = roostr_core::layout::is_mobile(window);
        if self.file_drag.is_some() && !cx.has_active_drag() {
            // The file drag left the window (or was cancelled) without a drop.
            self.file_drag = None;
            self.drag_rects.clear();
            self.drop_hint = None;
            self.last_valid_drop = None;
        }
        if let Some((id, offset)) = self.focus_request.clone()
            && self.texts.contains_key(&id)
        {
            self.focus_request = None;
            self.focus_block(&id, offset, cx);
        }
        if self.focus_pending {
            self.focus_pending = false;
            cx.defer_in(window, |this, window, cx| {
                window.focus(&this.focus_handle, cx)
            });
        }
        self.update_present_relations(cx);
        self.sync_file_views(cx);
        self.rows.borrow_mut().clear();
        self.bot_strips.borrow_mut().clear();

        let store = Store::global(cx).read(cx);
        let frame = Frame {
            by_id: self
                .blocks
                .iter()
                .map(|b| (b.id.clone(), b.clone()))
                .collect(),
            roots: self.root_ids(),
            all_roots: crate::doc::root_order(&self.blocks),
            space: roostr_core::relations::object_space_id(store, &self.object_fields()),
        };
        let blocks: Vec<AnyElement> = frame
            .roots
            .clone()
            .iter()
            .filter_map(|id| self.render_block(id, &frame, window, cx))
            .collect();
        let empty = frame.roots.is_empty();
        let loaded = self.object.is_some();
        let overlays = self.overlays(window, cx);
        let ghost = self.touch_ghost();

        let entity = cx.entity();
        let bounds_cell = self.bounds.clone();
        let visible_cell = self.visible.clone();
        div()
            .id("roostr-editor")
            .key_context("RoostrEditor")
            .track_focus(&self.focus_handle)
            .relative()
            .w_full()
            .min_h(px(240.))
            .pb(px(120.))
            .text_color(theme::fg())
            .cursor(if self.read_only {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            })
            .on_action(cx.listener(Self::on_enter))
            .on_action(cx.listener(Self::on_shift_enter))
            .on_action(cx.listener(Self::on_tab))
            .on_action(cx.listener(Self::on_shift_tab))
            .on_action(cx.listener(Self::on_backspace))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_delete_word_left))
            .on_action(cx.listener(Self::on_delete_line_left))
            .on_action(cx.listener(Self::on_delete_word_right))
            .on_action(cx.listener(Self::on_delete_line_right))
            .on_action(cx.listener(Self::on_left))
            .on_action(cx.listener(Self::on_right))
            .on_action(cx.listener(Self::on_select_left))
            .on_action(cx.listener(Self::on_select_right))
            .on_action(cx.listener(Self::on_word_left))
            .on_action(cx.listener(Self::on_word_right))
            .on_action(cx.listener(Self::on_select_word_left))
            .on_action(cx.listener(Self::on_select_word_right))
            .on_action(cx.listener(Self::on_line_start))
            .on_action(cx.listener(Self::on_line_end))
            .on_action(cx.listener(Self::on_select_line_start))
            .on_action(cx.listener(Self::on_select_line_end))
            .on_action(cx.listener(Self::on_doc_start))
            .on_action(cx.listener(Self::on_doc_end))
            .on_action(cx.listener(Self::on_select_doc_start))
            .on_action(cx.listener(Self::on_select_doc_end))
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_select_up))
            .on_action(cx.listener(Self::on_select_down))
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_bold))
            .on_action(cx.listener(Self::on_italic))
            .on_action(cx.listener(Self::on_underline))
            .on_action(cx.listener(Self::on_inline_code))
            .on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_cut))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_character_palette))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.spell_menu = None;
                    this.paste_menu = None;
                    if this.read_only {
                        return;
                    }
                    window.focus(&this.focus_handle, cx);
                    if let Some(a) = this.active.take() {
                        this.flush_save(&a, cx);
                        this.sync_texts();
                    }
                    this.toolbar = None;
                    this.arm_drag(ev, false, true);
                    cx.notify();
                }),
            )
            .on_drag_move(
                cx.listener(|this, ev: &DragMoveEvent<DraggedBlock>, window, cx| {
                    if this.dragging.is_none() {
                        let id = ev.drag(cx).id.clone();
                        this.begin_drag(&id);
                    }
                    this.drag_move_to(ev.event.position, window, cx);
                }),
            )
            .on_drag_move(
                cx.listener(|this, ev: &DragMoveEvent<ExternalPaths>, _, cx| {
                    if this.read_only {
                        return;
                    }
                    let inside = ev.bounds.contains(&ev.event.position);
                    if !inside {
                        if this.file_drag.is_some() {
                            this.file_drag = None;
                            this.drag_rects.clear();
                            this.drop_hint = None;
                            this.last_valid_drop = None;
                            cx.notify();
                        }
                        return;
                    }
                    if this.file_drag.is_none() {
                        this.file_drag = Some(ev.drag(cx).paths().to_vec());
                        this.build_drag_rects();
                    }
                    let hit = this.resolve_drop(ev.event.position);
                    if let Some(h) = &hit {
                        this.last_valid_drop = Some(h.clone());
                    }
                    if hit != this.drop_hint {
                        this.drop_hint = hit;
                        cx.notify();
                    }
                }),
            )
            .child(
                canvas(
                    move |bounds, window, _| {
                        bounds_cell.set(bounds);
                        visible_cell.set(bounds.intersect(&window.content_mask().bounds));
                        window.insert_hitbox(bounds, HitboxBehavior::Normal)
                    },
                    move |_, hitbox, window, _| {
                        // A touch hold on a row lifts it (Editor.svelte onTouchPress),
                        // unless something over the editor took the touch.
                        let e = entity.clone();
                        window.on_mouse_event(move |ev: &LongPressEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            match ev.phase {
                                TouchPhase::Started => {
                                    if !hitbox.is_hovered(window) {
                                        return;
                                    }
                                    let lifted = e.update(cx, |this, cx| {
                                        this.touch_lift_at(
                                            ev.start_position,
                                            ev.position,
                                            window,
                                            cx,
                                        )
                                    });
                                    if lifted {
                                        window.capture_long_press(&e);
                                        window.prevent_default();
                                        cx.stop_propagation();
                                    }
                                }
                                TouchPhase::Moved if window.has_long_press_capture(&e) => {
                                    e.update(cx, |this, cx| {
                                        this.touch_move(ev.position, window, cx)
                                    });
                                }
                                TouchPhase::Ended if window.has_long_press_capture(&e) => {
                                    e.update(cx, |this, cx| this.touch_drop(Some(ev.position), cx));
                                }
                                TouchPhase::Cancelled if window.has_long_press_capture(&e) => {
                                    e.update(cx, |this, cx| this.touch_drop(None, cx));
                                }
                                _ => {}
                            }
                        });
                        // Registered every frame (a press and release can land before the
                        // next paint), acting only while a drag of some kind is armed.
                        let e = entity.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            e.update(cx, |this, cx| {
                                if this.text_drag.is_none() && this.drag_sel.is_none() {
                                    return;
                                }
                                if ev.pressed_button == Some(MouseButton::Left) {
                                    this.drag_mouse_move(ev.position, window, cx);
                                } else {
                                    // The release happened outside the window.
                                    this.drag_mouse_up(ev.position, cx);
                                }
                            });
                        });
                        let e = entity.clone();
                        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            e.update(cx, |this, cx| {
                                if this.text_drag.is_some() || this.drag_sel.is_some() {
                                    this.drag_mouse_up(ev.position, cx);
                                }
                                if this.dragging.is_some() {
                                    // The pointer can be between targets on the frame the drop lands.
                                    let hint = this.drop_hint.clone().or_else(|| {
                                        this.last_valid_drop
                                            .clone()
                                            .filter(|h| !this.drop_forbidden(&h.id))
                                    });
                                    this.finish_drag(hint, cx);
                                }
                                if this.file_drag.is_some() && cx.has_active_drag() {
                                    this.drop_external(cx);
                                    cx.stop_propagation();
                                }
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
            .children(blocks)
            .when(empty && loaded && !self.read_only, |d| {
                d.child(
                    div()
                        .id("empty-hint")
                        // `.empty-hint`: 48px, 16px at ≤720px.
                        .ml(self.gutter_w())
                        .py(px(8.))
                        .text_size(px(15.))
                        .text_color(theme::muted())
                        .cursor(CursorStyle::IBeam)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, _, cx| this.append_block(cx)))
                        .child("Click to start writing…"),
                )
            })
            .when_some(self.load_error.clone().filter(|_| !loaded), |d, e| {
                d.child(
                    div()
                        .ml(self.gutter_w())
                        .text_color(theme::muted())
                        .text_size(px(13.))
                        .child(e),
                )
            })
            .children(ghost)
            .children(overlays)
    }
}

impl Editor {
    /// The spelling menu: suggestions, then "Add … to dictionary" with a
    /// close button. Outside clicks and picks dismiss it.
    fn build_spell_popup(
        &self,
        menu: &crate::editor::SpellMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PopupMenu> {
        let editor = cx.entity().downgrade();
        let word = menu.word.clone();
        let options = menu.options.clone();
        let popup = PopupMenu::build(window, cx, move |mut m, _, _| {
            m = m.min_w(px(180.));
            if options.is_empty() {
                let word = word.clone();
                m = m.item(
                    PopupMenuItem::element(move |_, _| {
                        div()
                            .text_size(px(13.))
                            .text_color(theme::muted())
                            .max_w(px(240.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(format!("No suggestions for \"{word}\""))
                    })
                    .disabled(true),
                );
            } else {
                for (i, option) in options.iter().enumerate() {
                    let label = option.clone();
                    let pick = option.clone();
                    let editor = editor.clone();
                    m = m.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .id(ElementId::Name(format!("spell-opt-{i}").into()))
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(label.clone())
                        })
                        .on_click(move |_, _, cx| {
                            let _ =
                                editor.update(cx, |this, cx| this.replace_misspelling(&pick, cx));
                        }),
                    );
                }
                m = m.separator();
            }
            let add = editor.clone();
            let close = editor.clone();
            let word = word.clone();
            m.item(
                PopupMenuItem::element(move |_, _| {
                    let close = close.clone();
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .text_size(px(13.))
                        .text_color(theme::muted())
                        .child(
                            div()
                                .id("spell-add")
                                .child(format!("Add \"{word}\" to dictionary")),
                        )
                        .child(
                            div()
                                .id("spell-close")
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(5.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::hl_med()).text_color(theme::fg()))
                                .on_click(move |_, _, cx| {
                                    // Not the row's "Add".
                                    cx.stop_propagation();
                                    let _ = close.update(cx, |this, cx| {
                                        this.spell_menu = None;
                                        cx.notify();
                                    });
                                })
                                .child("✕"),
                        )
                })
                .on_click(move |_, _, cx| {
                    let _ = add.update(cx, |this, cx| this.spell_add_word(cx));
                }),
            )
        });
        cx.subscribe(&popup, |this, popup, _: &DismissEvent, cx| {
            if this.spell_menu.as_ref().and_then(|m| m.popup.as_ref()) == Some(&popup) {
                this.spell_menu = None;
                cx.notify();
            }
        })
        .detach();
        popup
    }

    /// "Paste as": Embed (when a processor knows the URL), Bookmark, Link, Text.
    fn build_paste_popup(
        &self,
        menu: &crate::editor::PasteMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PopupMenu> {
        let editor = cx.entity().downgrade();
        let embed = menu.processor.is_some();
        let popup = PopupMenu::build(window, cx, move |mut m, _, _| {
            m = m.min_w(px(160.)).item(
                PopupMenuItem::element(|_, _| {
                    div()
                        .text_size(px(10.))
                        .text_color(theme::muted())
                        .pt(px(2.))
                        .child("PASTE AS")
                })
                .disabled(true),
            );
            // (id, label, as a block, embed / link)
            let choices: [(&'static str, &'static str, bool, bool); 4] = [
                ("paste-embed", "Embed", true, true),
                ("paste-bookmark", "Bookmark", true, false),
                ("paste-link", "Link", false, true),
                ("paste-text", "Text", false, false),
            ];
            for (id, label, block, flag) in choices {
                if id == "paste-embed" && !embed {
                    continue;
                }
                let editor = editor.clone();
                m = m.item(
                    PopupMenuItem::element(move |_, _| {
                        div().id(id).text_size(px(13.)).child(label)
                    })
                    .on_click(move |_, _, cx| {
                        let _ = editor.update(cx, |this, cx| {
                            if block {
                                this.paste_as_block(flag, cx)
                            } else {
                                this.paste_as_text(flag, cx)
                            }
                        });
                    }),
                );
            }
            m
        });
        cx.subscribe(&popup, |this, popup, _: &DismissEvent, cx| {
            if this.paste_menu.as_ref().and_then(|m| m.popup.as_ref()) == Some(&popup) {
                this.paste_menu = None;
                cx.notify();
            }
        })
        .detach();
        popup
    }
}

/// Roostr's frame around a kit popup menu: its border, radius and shadow
/// (the kit draws a ring at the theme radius, clipped away here). The editor
/// keeps focus: a click in the menu must not move it there.
fn popup_frame(id: &'static str, radius: Pixels, shadow: Vec<BoxShadow>) -> Stateful<Div> {
    div()
        .id(id)
        .occlude()
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .rounded(radius)
        .overflow_hidden()
        .p(px(2.))
        .shadow(shadow)
        .text_color(theme::fg())
        .capture_any_mouse_down(|_, window, _| window.prevent_default())
}

/// A marks-toolbar control: a kit ghost button (30px square) under Roostr's
/// tooltip — the kit Button takes only its own tooltip — whose mouse-down
/// stays off the text so the selection survives the click.
fn tool_button(
    id: &'static str,
    tip: &'static str,
    label: AnyElement,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    tool_button_with(id, tip, Button::new(id).w(px(30.)).p_0(), label, on_click)
}

/// [`tool_button`] sized to its label.
fn tool_button_wide(
    id: &'static str,
    tip: &'static str,
    label: AnyElement,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    tool_button_with(id, tip, Button::new(id).px(px(8.)), label, on_click)
}

fn tool_button_with(
    id: &'static str,
    tip: &'static str,
    button: Button,
    label: AnyElement,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("{id}-tip")))
        .flex_none()
        .tooltip(roostr_core::ui::tooltip(tip))
        .child(
            button
                .ghost()
                .h(px(30.))
                .text_size(px(14.))
                .text_color(theme::fg())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(on_click)
                .child(label),
        )
        .into_any_element()
}

//! Gallery view (GalleryView.svelte): a responsive grid of cards, each with
//! icon + name and the view's visible relation values as compact cells.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::dates::{DateStyle, show_date_with};
use roostr_core::options::badge_colors;
use roostr_core::{QueryRow, RelationDef, api, theme};
use roostr_properties::{CredentialCell, credential_cell_badge, reconnect_target};
use serde_json::json;

use crate::common::{self, cached_object, engine_body, tag_pill};
use crate::config;
use crate::context_menu::{MenuEvent, RowContextMenu};
use crate::views::ViewInput;

const CARD_MIN: f32 = 180.;
const GAP: f32 = 10.;

pub(crate) struct GalleryView {
    object_id: String,
    input: ViewInput,
    rows: Vec<QueryRow>,
    generation: u64,
    ctx_menu: Option<Entity<RowContextMenu>>,
    collection: Option<String>,
    /// The grid's width last frame (auto-fill columns).
    width: Rc<Cell<Pixels>>,
    columns: usize,
    /// The grid's top (window y) and a line of cards' height, gap included,
    /// last frame: only the lines in and near the window are built.
    top: Rc<Cell<f32>>,
    line_h: Rc<Cell<f32>>,
}

/// A line of cards' height before one is drawn (a card's 100px minimum + gap).
const LINE_H_GUESS: f32 = 110.;

impl GalleryView {
    pub(crate) fn new(
        object_id: String,
        collection: Option<String>,
        input: ViewInput,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            object_id,
            input,
            rows: Vec::new(),
            generation: 0,
            ctx_menu: None,
            collection,
            width: Rc::new(Cell::new(px(0.))),
            columns: 4,
            top: Rc::new(Cell::new(0.)),
            line_h: Rc::new(Cell::new(LINE_H_GUESS)),
        };
        this.load(cx);
        this
    }

    pub(crate) fn set_input(&mut self, input: ViewInput, cx: &mut Context<Self>) {
        if input != self.input {
            self.input = input;
            self.load(cx);
        }
    }

    pub(crate) fn load(&mut self, cx: &mut Context<Self>) {
        let Some(body) = self.input.body.clone() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let task = api::query(
            engine_body(
                &body,
                json!({"sorts": common::view_sorts_json(&self.input.sorts)}),
            ),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let Ok(res) = task.await else { return };
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.rows = res.records;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn row_ids(&self) -> impl Iterator<Item = &str> {
        self.rows.iter().map(|r| r.id.as_str())
    }

    /// Escape: close the context menu.
    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.ctx_menu = None;
        cx.notify();
    }

    fn open_menu(
        &mut self,
        id: String,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = cx.new(|cx| {
            RowContextMenu::new(
                position,
                vec![id],
                String::new(),
                self.collection.clone(),
                Vec::new(),
                Default::default(),
                window,
                cx,
            )
        });
        cx.subscribe(&menu, |this, _, event: &MenuEvent, cx| {
            match event {
                MenuEvent::Close => this.ctx_menu = None,
                MenuEvent::Changed => this.load(cx),
            }
            cx.notify();
        })
        .detach();
        self.ctx_menu = Some(menu);
        cx.notify();
    }
}

fn cell_text(r: &QueryRow, rel: &RelationDef) -> String {
    let Some(v) = r.fields.get(&rel.key) else {
        return String::new();
    };
    if let Some(s) = &v.string_value {
        return s.clone();
    }
    if let Some(b) = v.bool_value {
        return if b { "✓".into() } else { String::new() };
    }
    if let Some(n) = v.int_value {
        return if rel.format == "date" && n > 0 {
            show_date_with(n, DateStyle::MonthDay)
        } else {
            n.to_string()
        };
    }
    if let Some(f) = v.float_value {
        return js_number_string(f);
    }
    if v.values_value.is_some() {
        return v
            .items()
            .iter()
            .filter_map(|i| i.string_value.clone())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
    }
    String::new()
}

/// `String(n)` for a JS number.
pub(crate) fn js_number_string(f: f64) -> String {
    if f.fract() == 0. && f.abs() < 1e21 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

impl Render for GalleryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = cached_object(&self.object_id, cx) else {
            return div().into_any_element();
        };
        let relations = common::scoped_relations(&object, cx);
        // The view's visible relations (same list the table shows), name excluded.
        let visible: Vec<RelationDef> = config::visible_keys(&object.fields)
            .iter()
            .filter_map(|k| relations.iter().find(|r| r.key == *k).cloned())
            .collect();

        // CSS grid `repeat(auto-fill, minmax(180px, 1fr))`.
        let w = f32::from(self.width.get());
        let cols = if w > 0. {
            (((w + GAP) / (CARD_MIN + GAP)).floor() as usize).max(1)
        } else {
            self.columns
        };
        if cols != self.columns {
            self.columns = cols;
        }
        let width_cell = self.width.clone();
        let top_cell = self.top.clone();
        let entity = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                top_cell.set(f32::from(bounds.top()));
                if width_cell.get() != bounds.size.width {
                    width_cell.set(bounds.size.width);
                    let entity = entity.clone();
                    cx.defer(move |cx| {
                        let _ = entity.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        // Lines of `cols` cards (the CSS grid's rows): only those in, or a
        // window's height around, the window are built - the page scrolls the
        // gallery, and each frame of it built every card. Spacers keep the
        // gallery's full height.
        let lines = self.rows.len().div_ceil(cols);
        let line_h = self.line_h.get().max(1.);
        let (first, last) = {
            let view_h = f32::from(window.viewport_size().height);
            let top = self.top.get() + 4.;
            let first = (((-view_h - top) / line_h).floor().max(0.) as usize).min(lines);
            let last = ((((2. * view_h - top) / line_h).ceil().max(0.)) as usize).clamp(first, lines);
            (first, last)
        };
        let line_h_cell = self.line_h.clone();
        let mut grid = div()
            .flex()
            .flex_col()
            .pt(px(4.))
            .pb(px(24.))
            .on_children_prepainted(move |bounds, _, _| {
                // Children: the top spacer, the drawn lines, the bottom spacer.
                let drawn = bounds.get(1..1 + (last - first)).unwrap_or(&[]);
                if !drawn.is_empty() {
                    let h = drawn.iter().map(|b| f32::from(b.size.height)).sum::<f32>();
                    line_h_cell.set(h / drawn.len() as f32);
                }
            })
            .child(div().flex_none().h(px(line_h * first as f32)));
        let mut line = div().flex().gap(px(GAP)).pb(px(GAP));
        let mut in_line = 0;
        let end = (last * cols).min(self.rows.len());
        for r in &self.rows[first * cols..end] {
            let id = r.id.clone();
            let id2 = r.id.clone();
            let mut card = div()
                .id(SharedString::from(format!("gcard-{}", r.id)))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(6.))
                .min_w_0()
                .min_h(px(100.))
                .p(px(12.))
                .rounded(px(12.))
                .bg(theme::hl_light())
                .border_1()
                .border_color(theme::border())
                .text_color(theme::fg())
                .cursor_pointer()
                .hover(|s| s.border_color(theme::accent()))
                .on_click(move |_, _, cx| common::open_object(&id, cx))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                        this.open_menu(id2.clone(), e.position, window, cx)
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .w_full()
                        .child(div().flex_none().child(common::row_icon(r, cx)))
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(common::row_name(r)),
                        ),
                );
            for rel in &visible {
                let t = cell_text(r, rel);
                let target = reconnect_target(r, &rel.key);
                let cell = || {
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .max_w_full()
                        .overflow_hidden()
                        .text_size(px(12.))
                        .text_color(theme::muted())
                };
                if let Some(target) = target.clone()
                    && r.type_key == "credential"
                    && rel.key == "status"
                {
                    let (text, glyph, color) = credential_cell_badge(r, cx);
                    let colors = badge_colors(color);
                    let name = common::row_name(r);
                    card = card.child(
                        cell().child(
                            CredentialCell::new(
                                SharedString::from(format!("gcred-{}", r.id)),
                                r.clone(),
                                target,
                                format!("Reconnect {name}: {text}").into(),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .text_color(colors.fg)
                                    .child(Icon::new(glyph).size(px(13.)).text_color(colors.icon))
                                    .child(text),
                            ),
                        ),
                    );
                } else if let Some(target) = target
                    && rel.key == "error"
                {
                    card = card.child(
                        cell().child(
                            CredentialCell::new(
                                SharedString::from(format!("gerr-{}", r.id)),
                                r.clone(),
                                target,
                                format!("Reconnect: {t}").into(),
                            )
                            .child(t.clone()),
                        ),
                    );
                } else if !t.is_empty() {
                    if rel.format == "tag" || rel.format == "status" {
                        let mut c = cell();
                        for tg in t.split(", ") {
                            let color = rel
                                .options
                                .iter()
                                .find(|o| o.text == tg)
                                .map(|o| o.color.clone())
                                .unwrap_or_default();
                            c = c.child(tag_pill(tg.to_string(), &color, true, theme::muted()));
                        }
                        card = card.child(c);
                    } else {
                        card = card.child(cell().child(t));
                    }
                }
            }
            line = line.child(div().flex_1().min_w_0().flex().child(card.flex_1()));
            in_line += 1;
            if in_line == cols {
                grid = grid.child(std::mem::replace(&mut line, div().flex().gap(px(GAP)).pb(px(GAP))));
                in_line = 0;
            }
        }
        if in_line > 0 {
            // A short last line keeps the columns' width.
            for _ in in_line..cols {
                line = line.child(div().flex_1().min_w_0());
            }
            grid = grid.child(line);
        }
        grid = grid.child(div().flex_none().h(px(line_h * (lines - last) as f32)));
        let empty = self.rows.is_empty();
        div()
            .relative()
            .w_full()
            .child(measure)
            .child(grid)
            .when(empty, |d| {
                d.child(common::empty_state("No objects match.", 13.))
            })
            .into_any_element()
    }
}

impl GalleryView {
    /// The open row menu; the data view paints it above everything.
    pub(crate) fn ctx_menu(&self) -> Option<Entity<RowContextMenu>> {
        self.ctx_menu.clone()
    }
}

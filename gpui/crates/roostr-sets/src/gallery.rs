//! Gallery view (GalleryView.svelte): a responsive grid of cards, each with
//! icon + name and the view's visible relation values as compact cells.

use std::collections::HashMap;

use roostr_core::dates::{DateStyle, show_date_with};
use roostr_core::{QueryRow, RelationDef, State, api};
use roostr_properties::{CredentialCell, credential_cell_badge, reconnect_target};
use roostr_ui::motion;
use roostr_ui::tags::badge_colors;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Theme, glyph, ty};
use rune::{Cursor, Cx, El, PointerPress, Tx, col, el, row, text};
use serde_json::json;

use crate::common::{self, engine_body, tag_pill};
use crate::views::ViewInput;
use crate::{Active, DataView, config, context_menu, dv_mut};

const CARD_MIN: f32 = 180.;
const GAP: f32 = 10.;

/// A line of cards' height before one is drawn (a card's 100px minimum + gap).
const LINE_H_GUESS: f32 = 110.;

pub(crate) struct Gallery {
    input: ViewInput,
    rows: Vec<QueryRow>,
    generation: u64,
}

impl Gallery {
    pub(crate) fn new(input: ViewInput) -> Self {
        Self {
            input,
            rows: Vec::new(),
            generation: 0,
        }
    }

    pub(crate) fn row_ids(&self) -> impl Iterator<Item = &str> {
        self.rows.iter().map(|r| r.id.as_str())
    }

    /// Escape: the gallery has nothing of its own to close (its row menu
    /// lives on the data view, which closes it).
    pub(crate) fn dismiss(&mut self) -> bool {
        false
    }
}

fn gallery_mut<'a>(s: &'a mut State, id: &str) -> Option<&'a mut Gallery> {
    match &mut dv_mut(s, id)?.active {
        Some(Active::Gallery(g)) => Some(g),
        _ => None,
    }
}

pub(crate) fn set_input(s: &mut State, id: &str, input: ViewInput) {
    let Some(g) = gallery_mut(s, id) else { return };
    if input != g.input {
        g.input = input;
        load(s, id);
    }
}

pub(crate) fn load(s: &mut State, id: &str) {
    let Some(g) = gallery_mut(s, id) else { return };
    let Some(body) = g.input.body.clone() else {
        return;
    };
    g.generation += 1;
    let generation = g.generation;
    let sorts = common::view_sorts_json(&g.input.sorts);
    let task = api::query(engine_body(&body, json!({ "sorts": sorts })), s);
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(res) = task.await else { return };
        cx.update(|s| {
            if let Some(g) = gallery_mut(s, &view)
                && g.generation == generation
            {
                g.rows = res.records;
            }
        });
    })
    .detach();
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

/// A 1px `color` ring over a card's border that shows while the card (the
/// nearest hover group) is hovered: CSS `:hover { border-color }` (Rune's
/// `hover` is a fill overlay). For a card with a 1px layout border and
/// corner `radius`.
pub(crate) fn hover_border(radius: f32, color: rune::Color) -> El {
    el().pin(Some(-1.0), Some(-1.0), Some(-1.0), Some(-1.0))
        .radius(radius)
        .border(1.0, color)
        .group_opacity(0.0, 1.0)
        .a11y_hidden()
}

/// The gallery of the data view `v`: lines of cards, only those in or a
/// window's height around the window built.
pub(crate) fn view(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    v: &DataView,
) -> El {
    let Some(Active::Gallery(g)) = &v.active else {
        return el();
    };
    let Some(object) = s.store.object(&v.id) else {
        return el();
    };
    let id = v.id.clone();
    let relations = common::scoped_relations(s, &object);
    // The view's visible relations (same list the table shows), name excluded.
    let visible: Vec<&RelationDef> = config::visible_keys(&object.fields)
        .iter()
        .filter_map(|k| relations.iter().find(|r| r.key == *k))
        .collect();

    let key = format!("{id}/gallery");
    // CSS grid `repeat(auto-fill, minmax(180px, 1fr))`: the view re-runs only
    // when the column count changes.
    let cols = cx
        .layout_with(&key, |r| {
            (((r[2] + GAP) / (CARD_MIN + GAP)).floor() as usize).max(1)
        })
        .unwrap_or(4);
    // Lines of `cols` cards (the CSS grid's rows): only those in, or a
    // window's height around, the window are built - the page scrolls the
    // gallery, and each frame of it built every card. Spacers keep the
    // gallery's full height. The gallery's top is read in 256pt bands (the
    // view re-runs when it crosses one, well inside the window's margin), a
    // line's height from the first line drawn.
    let probe = format!("{id}/gline-first");
    let rows = &g.rows;
    let lines = rows.len().div_ceil(cols);
    let line_h = cx
        .layout_with(&probe, |r| r[3])
        .unwrap_or(LINE_H_GUESS)
        .max(1.);
    let (first, last) = {
        let view_h = cx.layout_with("kit-root", |r| r[3]).unwrap_or(0.);
        let band = cx
            .window_layout_with(&key, |r| (r[1] / 256.0).floor() as i32)
            .unwrap_or(0);
        let top = band as f32 * 256.0 + 4.;
        let first = (((-view_h - top) / line_h).floor().max(0.) as usize).min(lines);
        let last = ((((2. * view_h - top) / line_h).ceil().max(0.)) as usize).clamp(first, lines);
        (first, last)
    };
    let mut grid = col().pt(4.).pb(24.).child(el().h(line_h * first as f32));
    let new_line = |index: usize| {
        let key = if index == first {
            probe.clone()
        } else {
            format!("{id}/gline-{index}")
        };
        row().key(key).items_stretch().gap(GAP).pb(GAP)
    };
    let mut line = new_line(first);
    let mut line_index = first;
    let mut in_line = 0;
    let end = (last * cols).min(rows.len());
    for r in &rows[first * cols..end] {
        let card = card(s, cx, t, layer, &id, r, &visible);
        line = line.child(row().grow(1.).min_w(0.).child(card.grow(1.)));
        in_line += 1;
        if in_line == cols {
            line_index += 1;
            grid = grid.child(std::mem::replace(&mut line, new_line(line_index)));
            in_line = 0;
        }
    }
    if in_line > 0 {
        // A short last line keeps the columns' width.
        for _ in in_line..cols {
            line = line.child(el().grow(1.).min_w(0.));
        }
        grid = grid.child(line);
    }
    grid = grid.child(el().h(line_h * (lines - last) as f32));
    let mut out = col().key(key).w_full().child(grid);
    if rows.is_empty() {
        out = out.child(common::empty_state(t, "No objects match.", 13.));
    }
    out
}

/// One card: icon + name, then a compact cell per visible relation value.
fn card(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    view: &str,
    r: &QueryRow,
    visible: &[&RelationDef],
) -> El {
    let (open_id, menu_id) = (r.id.clone(), r.id.clone());
    let menu_view = view.to_string();
    let name = common::row_name(r);
    let mut card = col()
        .key(format!("{view}/gcard-{}", r.id))
        .items_start()
        .gap(6.)
        .min_w(0.)
        .min_h(100.)
        .padding(12.)
        .radius(12.)
        .fill(t.hl_light)
        .border(1., t.border)
        .border_layout()
        .cursor(Cursor::Pointer)
        .hover_group()
        .child(hover_border(12., t.accent))
        .on_press(move |s: &mut State| common::open_object(&open_id, s))
        .on_secondary_press(move |s: &mut State, p: PointerPress, _: &mut Tx| {
            context_menu::open(
                s,
                &menu_view,
                vec![menu_id.clone()],
                (p.x, p.y),
                HashMap::new(),
                Vec::new(),
            )
        })
        .child(
            row()
                .center_y()
                .gap(8.)
                .self_stretch()
                .child(common::icon(s, &common::row_icon(s, r), ty::ui(14.)))
                .child(
                    text(name.clone())
                        .font(ty::ui(14.).semibold())
                        .color(t.fg)
                        .truncate()
                        .shrink(),
                ),
        );
    for rel in visible {
        let value = cell_text(r, rel);
        let target = reconnect_target(r, &rel.key);
        let cell = || row().flex_wrap().gap(4.).self_stretch().min_w(0.);
        if let Some(target) = target.clone()
            && r.type_key == "credential"
            && rel.key == "status"
        {
            let (label, icon, color) = credential_cell_badge(r, s);
            let colors = badge_colors(t, color);
            card = card.child(
                cell().child(
                    CredentialCell::new(
                        format!("{view}/gcred-{}", r.id),
                        r.clone(),
                        target,
                        format!("Reconnect {name}: {label}"),
                    )
                    .child(
                        row()
                            .center_y()
                            .gap(4.)
                            .child(glyph(icon, 13.).color(colors.icon))
                            .child(text(label).font(ty::ui(12.)).color(colors.fg)),
                    )
                    .build(s, cx, t, layer),
                ),
            );
        } else if let Some(target) = target
            && rel.key == "error"
        {
            card = card.child(
                cell().child(
                    CredentialCell::new(
                        format!("{view}/gerr-{}", r.id),
                        r.clone(),
                        target,
                        format!("Reconnect: {value}"),
                    )
                    .child(text(value.clone()).font(ty::ui(12.)).color(t.muted).wrap())
                    .build(s, cx, t, layer),
                ),
            );
        } else if !value.is_empty() {
            if rel.format == "tag" || rel.format == "status" {
                let mut c = cell();
                for tg in value.split(", ") {
                    let color = rel
                        .options
                        .iter()
                        .find(|o| o.text == tg)
                        .map(|o| o.color.as_str())
                        .unwrap_or_default();
                    c = c.child(tag_pill(t, tg, color, true, t.muted));
                }
                card = card.child(c);
            } else {
                card =
                    card.child(cell().child(text(value).font(ty::ui(12.)).color(t.muted).wrap()));
            }
        }
    }
    motion::pressable(card)
}

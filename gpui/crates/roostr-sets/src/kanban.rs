//! Board view (KanbanView.svelte): columns are the group relation's options
//! plus "No <name>" (checkbox: Not done / Done); a multi-select card shows
//! in every group it belongs to; dragging a card between columns rewrites
//! its group value.

use std::collections::HashMap;

use roostr_core::{QueryRow, RelationDef, State, Value, api};
use roostr_ui::ty::Weights as _;
use roostr_ui::{Theme, color::rgb, motion, shadow, ty};
use rune::{Color, Cursor, Cx, Drag, El, Phase, PointerPress, Transition, Tx, col, el, row, text};
use serde_json::json;

use crate::common::{self, engine_body};
use crate::gallery::hover_border;
use crate::views::ViewInput;
use crate::{Active, DataView, config, context_menu, dv, dv_mut};

/// Boards with more cards than this never slide (Avoid: large lists).
const SLIDE_MAX_CARDS: usize = 200;

/// One card to the next before any is drawn (a one-line card + the 6px gap).
const PITCH_GUESS: f32 = 42.;
/// A column's header, before its first card.
const HEAD_H: f32 = 30.;
/// Pointer travel (points) that turns a press on a card into a drag.
const DRAG_THRESHOLD: f32 = 2.;

#[derive(Clone)]
struct Column {
    /// Option text, "true"/"false" for checkbox, "" for none.
    id: String,
    name: String,
    color: String,
}

/// The card a press is on: what a drag of it carries.
#[derive(Clone)]
struct CardRef {
    id: String,
    from: String,
    name: String,
    icon: String,
}

/// A card being dragged: the card, the pointer (window points) and where
/// on the card it was grabbed.
struct CardDrag {
    card: CardRef,
    at: [f32; 2],
    grab: [f32; 2],
}

pub(crate) struct Kanban {
    input: ViewInput,
    rows: Vec<QueryRow>,
    generation: u64,
    drop_col: Option<String>,
    drag: Option<CardDrag>,
    /// A card was dropped: the next rows to arrive slide into place.
    slide_next: bool,
    /// The rows on show arrived after a drop (refreshes from sync snap).
    cards_slide: bool,
    /// Each column's rect (window points, after scrolling) as last laid
    /// out: the drop targets of a card drag.
    col_rects: HashMap<String, [f32; 4]>,
}

impl Kanban {
    pub(crate) fn new(input: ViewInput) -> Self {
        Self {
            input,
            rows: Vec::new(),
            generation: 0,
            drop_col: None,
            drag: None,
            slide_next: false,
            cards_slide: false,
            col_rects: HashMap::new(),
        }
    }

    pub(crate) fn row_ids(&self) -> impl Iterator<Item = &str> {
        self.rows.iter().map(|r| r.id.as_str())
    }

    /// Escape: the board has nothing of its own to close (its row menu
    /// lives on the data view, which closes it; Escape cancels a card drag
    /// by itself).
    pub(crate) fn dismiss(&mut self) -> bool {
        false
    }
}

fn kanban<'a>(s: &'a State, id: &str) -> Option<&'a Kanban> {
    match &dv(s, id)?.active {
        Some(Active::Kanban(k)) => Some(k),
        _ => None,
    }
}

fn kanban_mut<'a>(s: &'a mut State, id: &str) -> Option<&'a mut Kanban> {
    match &mut dv_mut(s, id)?.active {
        Some(Active::Kanban(k)) => Some(k),
        _ => None,
    }
}

pub(crate) fn set_input(s: &mut State, id: &str, input: ViewInput) {
    let Some(k) = kanban_mut(s, id) else { return };
    if input != k.input {
        k.input = input;
        load(s, id);
    }
}

pub(crate) fn load(s: &mut State, id: &str) {
    let Some(k) = kanban_mut(s, id) else { return };
    let Some(body) = k.input.body.clone() else {
        return;
    };
    k.generation += 1;
    let generation = k.generation;
    let sorts = common::view_sorts_json(&k.input.sorts);
    let task = api::query(engine_body(&body, json!({ "sorts": sorts })), s);
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(res) = task.await else { return };
        cx.update(|s| {
            if let Some(k) = kanban_mut(s, &view)
                && k.generation == generation
            {
                k.rows = res.records;
                k.cards_slide =
                    std::mem::take(&mut k.slide_next) && k.rows.len() <= SLIDE_MAX_CARDS;
            }
        });
    })
    .detach();
}

/// The relation the board groups by.
fn rel_of(s: &State, id: &str) -> Option<RelationDef> {
    let object = s.store.object(id)?;
    let relations = common::scoped_relations(s, &object);
    let key = config::group_key(&object.fields);
    relations.into_iter().find(|r| r.key == key)
}

fn columns(rel: &RelationDef) -> Vec<Column> {
    if rel.format == "checkbox" {
        return vec![
            Column {
                id: "false".into(),
                name: "Not done".into(),
                color: String::new(),
            },
            Column {
                id: "true".into(),
                name: "Done".into(),
                color: String::new(),
            },
        ];
    }
    let mut out: Vec<Column> = rel
        .options
        .iter()
        .map(|o| Column {
            id: o.text.clone(),
            name: o.text.clone(),
            color: o.color.clone(),
        })
        .collect();
    out.push(Column {
        id: String::new(),
        name: format!(
            "No {}",
            if rel.name.is_empty() {
                &rel.key
            } else {
                &rel.name
            }
        ),
        color: String::new(),
    });
    out
}

fn values_of(rel: &RelationDef, r: &QueryRow) -> Vec<String> {
    let Some(v) = r.fields.get(&rel.key) else {
        return Vec::new();
    };
    if rel.format == "checkbox" {
        return vec![if v.bool_value == Some(true) {
            "true".into()
        } else {
            "false".into()
        }];
    }
    if let Some(s) = &v.string_value {
        return if s.is_empty() {
            vec![]
        } else {
            vec![s.clone()]
        };
    }
    v.items()
        .iter()
        .filter_map(|i| i.string_value.clone())
        .filter(|s| !s.is_empty())
        .collect()
}

fn cards_for<'a>(rows: &'a [QueryRow], rel: &RelationDef, col: &Column) -> Vec<&'a QueryRow> {
    rows.iter()
        .filter(|r| {
            let vals = values_of(rel, r);
            if rel.format == "checkbox" {
                return vals.first() == Some(&col.id) || (col.id == "false" && vals.is_empty());
            }
            if col.id.is_empty() {
                vals.is_empty()
            } else {
                vals.contains(&col.id)
            }
        })
        .collect()
}

/// The board column under a window point, from the rects last laid out.
fn column_at(s: &State, id: &str, x: f32, y: f32) -> Option<Column> {
    let k = kanban(s, id)?;
    let rel = rel_of(s, id)?;
    columns(&rel).into_iter().find(|c| {
        k.col_rects
            .get(&c.id)
            .is_some_and(|r| common::inside(*r, x, y))
    })
}

/// A press, drag and release on a card: a press released without travel
/// opens the record; past the threshold the card drags (the column under
/// the pointer lights up) and a release over another column moves it there.
fn card_drag(s: &mut State, id: &str, card: &CardRef, d: &Drag) {
    let [x, y] = d.window_point();
    match d.phase {
        Phase::Down => {}
        Phase::Move => {
            let over = column_at(s, id, x, y).map(|c| c.id);
            let Some(k) = kanban_mut(s, id) else { return };
            match &mut k.drag {
                Some(drag) => drag.at = [x, y],
                None if d.dx.hypot(d.dy) > DRAG_THRESHOLD => {
                    k.drag = Some(CardDrag {
                        card: card.clone(),
                        at: [x, y],
                        grab: [d.start_x, d.start_y],
                    });
                }
                None => return,
            }
            if k.drop_col != over {
                k.drop_col = over;
            }
        }
        Phase::Up => {
            let Some(k) = kanban_mut(s, id) else { return };
            k.drop_col = None;
            if k.drag.take().is_none() {
                let inside = d.x >= 0. && d.y >= 0. && d.x <= d.w && d.y <= d.h;
                if !d.cancelled() && inside {
                    common::open_object(&card.id, s);
                }
                return;
            }
            if d.cancelled() {
                return;
            }
            if let Some(col) = column_at(s, id, x, y) {
                drop_on(s, id, card, &col);
            }
        }
    }
}

fn drop_on(s: &mut State, view: &str, drag: &CardRef, col: &Column) {
    let Some(rel) = rel_of(s, view) else { return };
    if drag.from == col.id {
        return;
    }
    let Some(k) = kanban_mut(s, view) else { return };
    let Some(row) = k.rows.iter().find(|r| r.id == drag.id) else {
        return;
    };
    let current = values_of(&rel, row);
    k.slide_next = true;
    let id = drag.id.as_str();
    let task = if rel.format == "checkbox" {
        api::note::set_field(id, &rel.key, Value::bool(col.id == "true"), s)
    } else if rel.format == "status" {
        if col.id.is_empty() {
            api::note::delete_field(id, &rel.key, s)
        } else {
            api::note::set_field(id, &rel.key, Value::values(vec![Value::string(&col.id)]), s)
        }
    } else {
        // Multi-select: leave other memberships intact (Anytype board semantics).
        let mut vals: Vec<String> = current.into_iter().filter(|v| *v != drag.from).collect();
        if !col.id.is_empty() && !vals.contains(&col.id) {
            vals.push(col.id.clone());
        }
        if vals.is_empty() {
            api::note::delete_field(id, &rel.key, s)
        } else {
            api::note::set_field(
                id,
                &rel.key,
                Value::values(vals.into_iter().map(Value::string).collect()),
                s,
            )
        }
    };
    let view = view.to_string();
    s.rt.spawn(async move |cx| {
        let _ = task.await;
        cx.update(|s| load(s, &view));
    })
    .detach();
}

/// The CSS named colour a palette name resolves to in the browser (`ice` is not one).
fn css_named_color(name: &str) -> Option<Color> {
    let value = match name {
        "grey" => 0x808080,
        "yellow" => 0xffff00,
        "orange" => 0xffa500,
        "red" => 0xff0000,
        "pink" => 0xffc0cb,
        "purple" => 0x800080,
        "blue" => 0x0000ff,
        "teal" => 0x008080,
        "lime" => 0x00ff00,
        _ => return None,
    };
    Some(rgb(value))
}

fn card_body(s: &State, t: &Theme, icon: &str, name: &str) -> El {
    row()
        .center_y()
        .gap(8.)
        .fill(t.panel)
        .border(1., t.border)
        .border_layout()
        .radius(8.)
        .px(10.)
        .py(8.)
        .child(common::icon(s, icon, ty::ui(13.)))
        .child(
            text(name.to_string())
                .font(ty::ui(13.))
                .color(t.fg)
                .truncate()
                .shrink(),
        )
}

/// The board of the data view `v`; a dragged card follows the pointer in `layer`.
pub(crate) fn view(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    v: &DataView,
) -> El {
    let Some(Active::Kanban(k)) = &v.active else {
        return el();
    };
    let Some(rel) = rel_of(s, &v.id) else {
        return common::empty_state(
            t,
            "Pick a property to group by (a status, tag, or checkbox).",
            13.,
        );
    };
    let id = v.id.clone();
    let board_key = format!("{id}/board");
    let mut board = row()
        .key(board_key.clone())
        .items_start()
        .gap(10.)
        .scroll_x()
        .pt(4.)
        .pb(16.);
    // Each column builds only the cards in, or a window's height around,
    // the window (the page scrolls the board, and each frame of it built
    // every card); spacers keep the columns' heights. The board's top is
    // read in 256pt bands (the view re-runs when it crosses one, well inside
    // the window's margin), the distance from one card to the next from the
    // first card drawn.
    let probe = format!("{id}/kprobe");
    let pitch = cx
        .layout_with(&probe, |r| r[3] + 6.)
        .unwrap_or(PITCH_GUESS)
        .max(1.);
    let view_h = cx.layout_with("kit-root", |r| r[3]).unwrap_or(0.);
    let band = cx
        .window_layout_with(&board_key, |r| (r[1] / 256.0).floor() as i32)
        .unwrap_or(0);
    let cards_top = band as f32 * 256.0 + 4. + 8. + HEAD_H;
    let mut probed = false;
    for group in columns(&rel) {
        let cards = cards_for(&k.rows, &rel, &group);
        let n = cards.len();
        let first = (((-view_h - cards_top) / pitch).floor().max(0.) as usize).min(n);
        let last = ((((2. * view_h - cards_top) / pitch).ceil().max(0.)) as usize).clamp(first, n);
        let over = k.drop_col.as_deref() == Some(group.id.as_str());
        // The web sets `color: <option colour name>`: the browser reads it as a CSS colour.
        let color = css_named_color(&group.color).unwrap_or(t.fg);
        let (layout_view, layout_col) = (id.clone(), group.id.clone());
        let mut column = col()
            .key(format!("{id}/col-{}", group.id))
            .w(236.)
            .min_h(96.)
            .items_stretch()
            .gap(6.)
            .padding(8.)
            .radius(12.)
            .fill(t.hl_light)
            .border(1., if over { t.accent } else { Color::TRANSPARENT })
            .border_layout()
            .on_layout_tx(move |s: &mut State, r: [f32; 4], tx: &mut Tx| {
                if let Some(k) = kanban_mut(s, &layout_view) {
                    k.col_rects.insert(layout_col.clone(), r);
                }
                tx.live_only();
            })
            .child(
                row()
                    .center_y()
                    .justify_between()
                    .px(4.)
                    .pb(4.)
                    .child(
                        text(group.name.clone())
                            .font(ty::ui(12.).semibold())
                            .color(color),
                    )
                    .child(motion::rolling_count(
                        t,
                        format!("{id}/count-{}", group.id),
                        n,
                        ty::ui(11.),
                        t.muted,
                    )),
            );
        // The gap after the spacer stands in for the hidden cards' last one.
        column = column.child(el().h((pitch * first as f32 - 6.).max(0.)));
        for r in &cards[first..last] {
            let card = CardRef {
                id: r.id.clone(),
                from: group.id.clone(),
                name: common::row_name(r),
                icon: common::row_icon(s, r),
            };
            // The first card drawn on the board measures the pitch.
            let key = if probed {
                format!("{id}/card-{}-{}", group.id, r.id)
            } else {
                probed = true;
                probe.clone()
            };
            let (drag_view, menu_view, menu_id) = (id.clone(), id.clone(), r.id.clone());
            let dragged = k.drag.as_ref().is_some_and(|d| d.card.id == r.id);
            let mut card_el = card_body(s, t, &card.icon, &card.name)
                .key(key)
                .cursor(Cursor::Grab)
                .hover_group()
                .child(hover_border(8., t.accent))
                .on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| {
                    card_drag(s, &drag_view, &card, d)
                })
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
                // A card moved to another column is a new card there: it
                // lands at once.
                .transition(Transition::none());
            // After a drop the cards below the moved one slide up (or down)
            // to their places; any other refresh snaps.
            card_el = if k.cards_slide {
                card_el.motion(motion::CONTROL.spring())
            } else {
                card_el.instant_changes()
            };
            // Held cards shrink; a dragged one stays put at rest size (its
            // ghost follows the pointer).
            if !dragged {
                card_el = motion::pressable(card_el);
            }
            column = column.child(card_el);
        }
        column = column.child(el().h((pitch * (n - last) as f32 - 6.).max(0.)));
        if n == 0 {
            column = column.child(
                row()
                    .justify_center()
                    .padding(6.)
                    .child(text("—").font(ty::ui(12.)).color(t.muted)),
            );
        }
        board = board.child(column);
    }
    if let Some(drag) = &k.drag {
        // The drag preview: the card at 204px, under the pointer where it was grabbed.
        layer.push(
            card_body(s, t, &drag.card.icon, &drag.card.name)
                .w(204.)
                .opacity(0.9)
                .box_shadow(shadow::drag())
                .absolute(drag.at[0] - drag.grab[0], drag.at[1] - drag.grab[1])
                .transition(Transition::none())
                .a11y_hidden(),
        );
    }
    col().child(board)
}

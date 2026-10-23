//! Board view (KanbanView.svelte): columns are the group relation's options
//! plus "No <name>" (checkbox: Not done / Done); a multi-select card shows
//! in every group it belongs to; dragging a card between columns rewrites
//! its group value.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::motion::{self, LayoutGroup};
use roostr_core::{QueryRow, RelationDef, Value, api, theme};
use serde_json::json;

use crate::common::{self, cached_object, engine_body};
use crate::config;
use crate::context_menu::{MenuEvent, RowContextMenu};
use crate::views::{ViewInput, ViewMsg};

/// Boards with more cards than this never slide (Avoid: large lists).
const SLIDE_MAX_CARDS: usize = 200;

#[derive(Clone)]
struct Column {
    /// Option text, "true"/"false" for checkbox, "" for none.
    id: String,
    name: String,
    color: String,
}

/// Payload of a card drag.
#[derive(Clone)]
struct CardDrag {
    id: String,
    from: String,
    name: SharedString,
    icon: String,
}

impl Render for CardDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        card_body(&self.icon, self.name.clone())
            .w(px(204.))
            .opacity(0.9)
            .shadow(roostr_core::ui::drag_shadow())
    }
}

fn card_body(icon: &str, name: SharedString) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .rounded(px(8.))
        .px(px(10.))
        .py(px(8.))
        .text_size(px(13.))
        .text_color(theme::fg())
        .child(div().flex_none().child(icon.to_string()))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(name),
        )
}

pub(crate) struct KanbanView {
    object_id: String,
    input: ViewInput,
    rows: Vec<QueryRow>,
    generation: u64,
    drop_col: Option<String>,
    ctx_menu: Option<Entity<RowContextMenu>>,
    collection: Option<String>,
    /// Card positions: the cards slide to their places after a drop.
    cards_layout: LayoutGroup,
    /// A card was dropped: the next rows to arrive slide into place.
    slide_next: bool,
    /// The rows on show arrived after a drop (refreshes from sync snap).
    cards_slide: bool,
}

impl EventEmitter<ViewMsg> for KanbanView {}

impl KanbanView {
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
            drop_col: None,
            ctx_menu: None,
            collection,
            cards_layout: LayoutGroup::default(),
            slide_next: false,
            cards_slide: false,
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
                    this.cards_slide =
                        std::mem::take(&mut this.slide_next) && this.rows.len() <= SLIDE_MAX_CARDS;
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

    fn rel(&self, cx: &App) -> Option<RelationDef> {
        let object = cached_object(&self.object_id, cx)?;
        let relations = common::scoped_relations(&object, cx);
        let key = config::group_key(&object.fields).to_string();
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

    fn cards_for<'a>(&'a self, rel: &RelationDef, col: &Column) -> Vec<&'a QueryRow> {
        self.rows
            .iter()
            .filter(|r| {
                let vals = Self::values_of(rel, r);
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

    fn drop_on(&mut self, drag: &CardDrag, col: Column, cx: &mut Context<Self>) {
        self.drop_col = None;
        cx.notify();
        let Some(rel) = self.rel(cx) else { return };
        if drag.from == col.id {
            return;
        }
        let Some(row) = self.rows.iter().find(|r| r.id == drag.id) else {
            return;
        };
        self.slide_next = true;
        let id = drag.id.clone();
        let task = if rel.format == "checkbox" {
            api::note::set_field(&id, &rel.key, Value::bool(col.id == "true"), cx)
        } else if rel.format == "status" {
            if col.id.is_empty() {
                api::note::delete_field(&id, &rel.key, cx)
            } else {
                api::note::set_field(
                    &id,
                    &rel.key,
                    Value::values(vec![Value::string(&col.id)]),
                    cx,
                )
            }
        } else {
            // Multi-select: leave other memberships intact (Anytype board semantics).
            let mut vals: Vec<String> = Self::values_of(&rel, row)
                .into_iter()
                .filter(|v| *v != drag.from)
                .collect();
            if !col.id.is_empty() && !vals.contains(&col.id) {
                vals.push(col.id.clone());
            }
            if vals.is_empty() {
                api::note::delete_field(&id, &rel.key, cx)
            } else {
                api::note::set_field(
                    &id,
                    &rel.key,
                    Value::values(vals.into_iter().map(Value::string).collect()),
                    cx,
                )
            }
        };
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            let _ = this.update(cx, |this, cx| {
                this.load(cx);
                cx.emit(ViewMsg::Changed);
            });
        })
        .detach();
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

/// The CSS named colour a palette name resolves to in the browser (`ice` is not one).
fn css_named_color(name: &str) -> Option<Hsla> {
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
    Some(rgb(value).into())
}

impl Render for KanbanView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(rel) = self.rel(cx) else {
            return common::empty_state(
                "Pick a property to group by (a status, tag, or checkbox).",
                13.,
            )
            .into_any_element();
        };
        let mut board = div()
            .id("board")
            .flex()
            .items_start()
            .gap(px(10.))
            .overflow_x_scroll()
            .pt(px(4.))
            .pb(px(16.));
        for col in Self::columns(&rel) {
            let cards = self.cards_for(&rel, &col);
            let over = self.drop_col.as_deref() == Some(col.id.as_str());
            // The web sets `color: <option colour name>`: the browser reads it as a CSS colour.
            let color = css_named_color(&col.color);
            let col_id = col.id.clone();
            let col_drop = col.clone();
            let mut column = div()
                .id(SharedString::from(format!("col-{}", col.id)))
                .flex_none()
                .w(px(236.))
                .min_h(px(96.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(8.))
                .rounded(px(12.))
                .bg(theme::hl_light())
                .border_1()
                .border_color(if over {
                    theme::accent()
                } else {
                    transparent_black()
                })
                .on_drag_move(
                    cx.listener(move |this, e: &DragMoveEvent<CardDrag>, _, cx| {
                        let inside = e.bounds.contains(&e.event.position);
                        if inside && this.drop_col.as_deref() != Some(col_id.as_str()) {
                            this.drop_col = Some(col_id.clone());
                            cx.notify();
                        } else if !inside && this.drop_col.as_deref() == Some(col_id.as_str()) {
                            this.drop_col = None;
                            cx.notify();
                        }
                    }),
                )
                .on_drop(cx.listener(move |this, drag: &CardDrag, _, cx| {
                    this.drop_on(drag, col_drop.clone(), cx)
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(4.))
                        .pb(px(4.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .when_some(color, |d, c| d.text_color(c))
                                .child(col.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme::muted())
                                .child(cards.len().to_string()),
                        ),
                );
            let empty = cards.is_empty();
            for r in cards {
                let icon = common::row_icon(r, cx);
                let name = common::row_name(r);
                let id = r.id.clone();
                let id2 = r.id.clone();
                let drag = CardDrag {
                    id: r.id.clone(),
                    from: col.id.clone(),
                    name: name.clone(),
                    icon: icon.clone(),
                };
                let card = card_body(&icon, name)
                    .id(SharedString::from(format!("card-{}-{}", col.id, r.id)))
                    .cursor_grab()
                    .hover(|s| s.border_color(theme::accent()))
                    .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
                    .on_click(move |_, _, cx| common::open_object(&id, cx))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                            this.open_menu(id2.clone(), e.position, window, cx)
                        }),
                    );
                // After a drop the cards below the moved one slide up (or
                // down) to their places; the moved card lands at once.
                column = column.child(
                    self.cards_layout
                        .position(
                            SharedString::from(format!("card-pos-{}-{}", col.id, r.id)),
                            card,
                        )
                        .enabled(self.cards_slide)
                        .transition(motion::CONTROL.transition()),
                );
            }
            if empty {
                column = column.child(
                    div()
                        .text_center()
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .p(px(6.))
                        .child("—"),
                );
            }
            board = board.child(column);
        }
        // Cards measure their slides from the board: the page moving it slides nothing.
        let board = self
            .cards_layout
            .position("board-layout", board)
            .enabled(false);
        div().child(board).into_any_element()
    }
}

impl KanbanView {
    /// The open row menu; the data view paints it above everything.
    pub(crate) fn ctx_menu(&self) -> Option<Entity<RowContextMenu>> {
        self.ctx_menu.clone()
    }
}

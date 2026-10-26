//! A pinned object's sidebar widget body (`lib/components/PinnedWidget.svelte`):
//! sets render a mini version of their current view (rows, gallery cards,
//! kanban group counts, a mini month); any other object lists what it links to.

use std::sync::Arc;

use chrono::{Datelike, Local, NaiveDate, TimeZone};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::filters::{engine_filters_of, space_filter_of};
use roostr_core::motion::{self, NumberEffect, NumberMotionState};
use roostr_core::nav::{Navigator, Route, navigate};
use roostr_core::objects::object_icon;
use roostr_core::relations::{object_space_id, space_relations};
use roostr_core::store::Store;
use roostr_core::types::{Object, QueryRow, RelationDef, field_str};
use roostr_core::{StoreEvent, lite_ui, theme};
use serde_json::{Value as Json, json};

use crate::rail::mix;

/// Rows a list widget fetches: all of them, short of a query matching thousands.
const LIST_FETCH: usize = 500;
const GALLERY_LIMIT: usize = 8;
const DOW: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
/// A kanban group's count rolling to a new value.
const COUNT: motion::Tween = motion::Tween::ease(250);

pub(crate) struct PinnedWidget {
    id: String,
    obj: Option<Arc<Object>>,
    rows: Vec<QueryRow>,
    month_off: i32,
    /// The sidebar width, set by the shell each frame (gallery column fit).
    pub(crate) side_width: f32,
    seq: u64,
    /// Lite: the query/set's space, while it is not held whole (its rows
    /// would be partial); it downloads and the widget shows the progress line.
    partial_space: Option<String>,
    /// Lite: ids last asked of the relays (collection members, link
    /// targets), so the refresh after the fetch does not ask again.
    fetched: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

impl PinnedWidget {
    pub(crate) fn new(id: String, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let subscriptions = vec![
            // Its rows are other objects: any of them may have synced in or
            // changed since (a phone's first history walk runs under it), so
            // every collection refresh - throttled, after commits - reloads it.
            cx.subscribe(&store, |this, _, event: &StoreEvent, cx| {
                let reload = match event {
                    StoreEvent::Committed(oid) => *oid == this.id,
                    StoreEvent::Refreshed | StoreEvent::Reconnected => true,
                };
                if reload {
                    this.load(cx);
                }
            }),
            cx.observe(&Navigator::global(cx), |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            id,
            obj: None,
            rows: Vec::new(),
            month_off: 0,
            side_width: 220.,
            seq: 0,
            partial_space: None,
            fetched: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.seq += 1;
        let seq = self.seq;
        let fetch = Store::global(cx).read(cx).fetch_object(&self.id, cx);
        cx.spawn(async move |this, cx| {
            let object = match fetch.await {
                Ok(object) => Arc::new(object),
                Err(_) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.seq == seq {
                            this.obj = None;
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            let body = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| this.lite(&object, cx));
                query_body(&object, cx)
            });
            let rows = match body {
                Some(body) => cx
                    .update(|cx| roostr_core::api::query(body, cx))
                    .await
                    .ok()
                    .map(|res| res.records),
                None => Some(Vec::new()),
            };
            let _ = this.update(cx, |this, cx| {
                if this.seq != seq {
                    return;
                }
                match rows {
                    Some(rows) => {
                        this.obj = Some(object);
                        this.rows = rows;
                    }
                    None => this.obj = None,
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Lite: a query/set over a space not held whole downloads that space
    /// (once; the refresh when it lands re-evaluates); a collection or a
    /// link widget fetches the ids it shows.
    fn lite(&mut self, o: &Object, cx: &mut Context<Self>) {
        if roostr_core::lite::status(cx).is_none() {
            self.partial_space = None;
            return;
        }
        let ids = if o.type_key == "collection" {
            members(o)
        } else if is_set(o) {
            let space = object_space_id(Store::global(cx).read(cx), &o.fields);
            if lite_ui::space_held(&space, cx) {
                self.partial_space = None;
            } else {
                lite_ui::ensure_space(&space, cx);
                self.partial_space = Some(space);
            }
            return;
        } else {
            link_targets(o)
        };
        if !ids.is_empty() && ids != self.fetched {
            self.fetched = ids.clone();
            lite_ui::fetch(ids, cx);
        }
    }
}

fn is_set(o: &Object) -> bool {
    matches!(o.type_key.as_str(), "query" | "set" | "collection")
}

fn space_rels(store: &Store, o: &Object) -> Vec<RelationDef> {
    space_relations(&store.relations, &object_space_id(store, &o.fields))
        .into_iter()
        .cloned()
        .collect()
}

/// A collection's member ids.
fn members(o: &Object) -> Vec<String> {
    o.field("collectionIds")
        .map(|v| {
            v.items()
                .iter()
                .filter_map(|i| i.string_value.clone())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// What a non-set object links to (the tree widget's rows).
fn link_targets(o: &Object) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    let mut add = |id: &str| {
        if !id.is_empty() && !targets.iter().any(|t| t == id) {
            targets.push(id.to_string());
        }
    };
    for (key, v) in &o.fields {
        if key == "channel" {
            continue;
        }
        if let Some(link) = v.link_value.as_ref().filter(|l| !l.target_id.is_empty()) {
            add(&link.target_id);
        } else if v.values_value.is_some() {
            for item in v.items() {
                if let Some(link) = item.link_value.as_ref().filter(|l| !l.target_id.is_empty()) {
                    add(&link.target_id);
                } else if key == "collectionIds"
                    && let Some(s) = &item.string_value
                {
                    add(s);
                }
            }
        }
    }
    targets
}

/// The widget's query (`None` = nothing to ask: no members / no links).
fn query_body(o: &Object, cx: &App) -> Option<Json> {
    let store = Store::global(cx).read(cx);
    if is_set(o) {
        let members = members(o);
        let mut filters = engine_filters_of(&o.fields, &space_rels(store, o));
        filters.push(space_filter_of(
            &o.fields,
            store.channels.first().map_or("", |c| c.id.as_str()),
        ));
        let mut body = if o.type_key == "collection" {
            if members.is_empty() {
                return None;
            }
            let mut all = vec![json!({ "key": "id", "condition": "in", "value": members })];
            all.extend(filters);
            json!({ "filters": all })
        } else {
            json!({ "setId": o.id, "filters": filters })
        };
        let sorts: Vec<Json> = o
            .field("viewSorts")
            .map(|v| {
                v.items()
                    .iter()
                    .filter_map(|i| {
                        let e = i.entries()?;
                        let key = e.get("key").and_then(|v| v.as_str()).unwrap_or("");
                        if key.is_empty() {
                            return None;
                        }
                        let kind = if e.get("type").and_then(|v| v.as_str()) == Some("asc") {
                            "asc"
                        } else {
                            "desc"
                        };
                        Some(json!({ "key": key, "type": kind, "emptyPlacement": "end" }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let sorts = if sorts.is_empty() {
            vec![json!({ "key": "updatedAt", "type": "desc", "emptyPlacement": "end" })]
        } else {
            sorts
        };
        let shown: Vec<&str> = [
            "name",
            "iconEmoji",
            "done",
            o.field_str("viewGroupKey"),
            o.field_str("viewDateKey"),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
        let map = body.as_object_mut().expect("object body");
        map.insert("sorts".into(), json!(sorts));
        // Every row shows (no "+N more"); a cap only against a query matching thousands.
        map.insert("limit".into(), json!(LIST_FETCH));
        map.insert("fields".into(), json!(shown));
        return Some(body);
    }
    // Tree widget: outbound links.
    let targets = link_targets(o);
    if targets.is_empty() {
        return None;
    }
    let limit = targets.len();
    Some(
        json!({ "filters": [{ "key": "id", "condition": "in", "value": targets }], "limit": limit }),
    )
}

struct Cell {
    n: u32,
    cur: bool,
    today: bool,
    marked: bool,
}

/// The navigable Monday-first month: label, year and padded cells.
fn month(rows: &[QueryRow], date_key: &str, month_off: i32) -> (String, i32, Vec<Cell>) {
    let now = Local::now().date_naive();
    let total = now.year() * 12 + now.month0() as i32 + month_off;
    let (y, m0) = (total.div_euclid(12), total.rem_euclid(12) as u32);
    let first = NaiveDate::from_ymd_opt(y, m0 + 1, 1).expect("valid month");
    let next_first = if m0 == 11 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m0 + 2, 1)
    }
    .expect("valid month");
    let days = (next_first - first).num_days() as u32;
    let prev_days = (first
        - first
            .pred_opt()
            .expect("has previous day")
            .with_day(1)
            .expect("day 1"))
    .num_days() as u32;
    let ts_of = |r: &QueryRow| -> i64 {
        match date_key {
            "createdDate" => r.created_at,
            "modifiedDate" => r.updated_at,
            key => r
                .fields
                .get(key)
                .and_then(|v| v.int_value.or(v.float_value.map(|f| f as i64)))
                .unwrap_or(0),
        }
    };
    let mut marked = std::collections::HashSet::new();
    for r in rows {
        let ts = ts_of(r);
        if ts == 0 {
            continue;
        }
        if let Some(d) = Local.timestamp_millis_opt(ts).earliest() {
            let d = d.date_naive();
            if d.year() == y && d.month0() == m0 {
                marked.insert(d.day());
            }
        }
    }
    let offset = first.weekday().num_days_from_monday();
    let is_this = y == now.year() && m0 == now.month0();
    let mut cells = Vec::new();
    for i in 0..offset {
        cells.push(Cell {
            n: prev_days - offset + 1 + i,
            cur: false,
            today: false,
            marked: false,
        });
    }
    for i in 1..=days {
        cells.push(Cell {
            n: i,
            cur: true,
            today: is_this && i == now.day(),
            marked: marked.contains(&i),
        });
    }
    let mut next = 1;
    while cells.len() % 7 != 0 {
        cells.push(Cell {
            n: next,
            cur: false,
            today: false,
            marked: false,
        });
        next += 1;
    }
    (first.format("%B").to_string(), y, cells)
}

fn open(id: String) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| navigate(Route::Object(id.clone()), cx)
}

impl PinnedWidget {
    /// Renders nothing: no object yet, or a preview with no rows (notes,
    /// pages; a calendar always shows its month) and no space downloading.
    pub(crate) fn is_empty(&self) -> bool {
        let Some(obj) = &self.obj else {
            return true;
        };
        let calendar = is_set(obj) && obj.field_str("viewType") == "calendar";
        self.rows.is_empty() && !calendar && self.partial_space.is_none()
    }
}

impl Render for PinnedWidget {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(obj) = self.obj.clone() else {
            return div();
        };
        let set = is_set(&obj);
        let view_type = if set && !obj.field_str("viewType").is_empty() {
            obj.field_str("viewType")
        } else {
            "table"
        };
        // Lite: the space is downloading - its progress (`lite_ui`'s text,
        // shimmering) over skeleton rows instead of partial rows.
        if let Some(space) = &self.partial_space {
            let load = roostr_core::lite::status(cx)
                .and_then(|status| status.loading(space).map(lite_ui::loading_text));
            return match load {
                Some(text) => div()
                    .px(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pb(px(6.))
                    .child(
                        ShimmerText::new(text)
                            .id(SharedString::from(format!("pin-load-{}", self.id)))
                            .py(px(6.))
                            .text_size(px(12.))
                            .text_color(theme::muted()),
                    )
                    .child(Skeleton::new().h(px(10.)).w(relative(0.8)).rounded(px(5.)))
                    .child(Skeleton::new().h(px(10.)).w(relative(0.55)).rounded(px(5.))),
                None => div().px(px(8.)).children(lite_ui::space_notice(
                    format!("pin-{}", self.id),
                    space,
                    cx,
                )),
            };
        }
        if self.rows.is_empty() && view_type != "calendar" {
            return div();
        }
        let current_id = match Navigator::global(cx).read(cx).route() {
            Route::Object(id) => id.clone(),
            _ => String::new(),
        };
        let active = Navigator::global(cx).read(cx).space().to_string();
        let group_key = obj.field_str("viewGroupKey").to_string();
        let date_key = if obj.field_str("viewDateKey").is_empty() {
            "createdDate".to_string()
        } else {
            obj.field_str("viewDateKey").to_string()
        };
        let store = Store::global(cx).read(cx);
        let group_rel = space_rels(store, &obj)
            .into_iter()
            .find(|r| r.key == group_key);
        let row_icon = |r: &QueryRow| {
            object_icon(
                store,
                field_str(&r.fields, "iconEmoji"),
                &r.type_key,
                &active,
            )
        };
        let row_name = |r: &QueryRow| {
            let n = field_str(&r.fields, "name");
            if n.is_empty() {
                "Untitled".to_string()
            } else {
                n.to_string()
            }
        };
        let body = div().flex().flex_col().gap(px(1.)).pt(px(2.));
        let set_id = self.id.clone();

        // `.w-line`: a kit list row, 24px, 12px muted (fg under the pointer
        // and while current, the current one on `hl-med`).
        let line = |id: ElementId, current: bool, content: Div| {
            ListItem::new(id)
                .group("w-line")
                .h(px(24.))
                .px(px(8.))
                .py_0()
                .rounded(theme::radius::sm())
                .text_size(px(12.))
                .overflow_hidden()
                .cursor_pointer()
                .text_color(if current { theme::fg() } else { theme::muted() })
                .when(current, |d| d.bg(theme::hl_med()))
                .child(
                    content
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .min_w_0()
                        .group_hover("w-line", |s| s.text_color(theme::fg())),
                )
        };

        if view_type == "kanban"
            && let Some(rel) = group_rel
        {
            let checkbox = rel.format == "checkbox";
            let vals_of = |r: &QueryRow| -> Vec<String> {
                let Some(v) = r.fields.get(&group_key) else {
                    return vec![];
                };
                if checkbox {
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
                if v.values_value.is_some() {
                    return v
                        .items()
                        .iter()
                        .filter_map(|i| i.string_value.clone())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                vec![]
            };
            let defs: Vec<(String, String)> = if checkbox {
                vec![
                    ("false".into(), "Not done".into()),
                    ("true".into(), "Done".into()),
                ]
            } else {
                let mut d: Vec<(String, String)> = rel
                    .options
                    .iter()
                    .map(|o| (o.text.clone(), o.text.clone()))
                    .collect();
                let label = if rel.name.is_empty() {
                    group_key.clone()
                } else {
                    rel.name.clone()
                };
                d.push((String::new(), format!("No {label}")));
                d
            };
            let mut body = body;
            for (i, (gid, name)) in defs.into_iter().enumerate() {
                let count = self
                    .rows
                    .iter()
                    .filter(|r| {
                        let vals = vals_of(r);
                        if checkbox {
                            vals.first() == Some(&gid) || (gid == "false" && vals.is_empty())
                        } else if gid.is_empty() {
                            vals.is_empty()
                        } else {
                            vals.contains(&gid)
                        }
                    })
                    .count();
                if count == 0 {
                    continue;
                }
                // A count rolls to its new value (Svelte swapped the text);
                // the first render shows it as is.
                let shown = motion::use_motion(
                    crate::eid("w-group-count", &gid),
                    window,
                    cx,
                    NumberMotionState::default,
                );
                let shown = shown
                    .read(cx)
                    .element(crate::eid("w-group-n", &gid), count as i64)
                    .effect(NumberEffect::Roll)
                    .transition(COUNT.transition())
                    .animate_initial(false);
                body = body.child(
                    line(
                        ("w-group", i).into(),
                        false,
                        div()
                            .child(div().flex_1().min_w_0().truncate().child(name))
                            .child(div().text_size(px(11.)).child(shown)),
                    )
                    .on_click(open(set_id.clone())),
                );
            }
            return body;
        }

        if view_type == "calendar" {
            let (label, year, cells) = month(&self.rows, &date_key, self.month_off);
            let weeks = cells.len() / 7;
            let mut grid = div()
                .id("w-cal-grid")
                .flex()
                .flex_col()
                .cursor_pointer()
                .on_click(open(set_id.clone()));
            let mut head_row = div().flex();
            for (i, d) in DOW.iter().enumerate() {
                head_row = head_row.child(
                    div()
                        .flex_1()
                        .py(px(4.))
                        .flex()
                        .justify_center()
                        .text_size(px(11.))
                        .text_color(theme::muted())
                        .when(i >= 5, |c| c.bg(theme::hl_light()))
                        .when(i == 5, |c| c.rounded_tl(px(8.)))
                        .when(i == 6, |c| c.rounded_tr(px(8.)))
                        .child(*d),
                );
            }
            grid = grid.child(head_row);
            for w in 0..weeks {
                let mut row = div().flex();
                for (i, c) in cells[w * 7..w * 7 + 7].iter().enumerate() {
                    let last = w + 1 == weeks;
                    row = row.child(
                        div()
                            .relative()
                            .flex_1()
                            .py(px(5.))
                            .flex()
                            .justify_center()
                            .text_size(px(12.5))
                            .text_color(theme::fg())
                            .when(i >= 5, |d| d.bg(theme::hl_light()))
                            .when(last && i == 5, |d| d.rounded_bl(px(8.)))
                            .when(last && i == 6, |d| d.rounded_br(px(8.)))
                            .when(!c.cur, |d| d.text_color(theme::muted()).opacity(0.6))
                            .when(c.today, |d| {
                                d.text_color(theme::accent()).font_weight(FontWeight::BOLD)
                            })
                            .child(c.n.to_string())
                            .when(c.marked, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .bottom(px(1.))
                                        .left(relative(0.5))
                                        .ml(px(-1.5))
                                        .size(px(3.))
                                        .rounded_full()
                                        .bg(theme::accent()),
                                )
                            }),
                    );
                }
                grid = grid.child(row);
            }
            let nav_btn =
                |id: &'static str, glyph: &'static str, delta: i32, cx: &mut Context<Self>| {
                    crate::fill_button(id, transparent_black(), cx)
                        .h_auto()
                        .px(px(5.))
                        .py(px(2.))
                        .accessibility_label(if delta < 0 {
                            "Previous month"
                        } else {
                            "Next month"
                        })
                        .child(div().text_size(px(16.)).line_height(px(16.)).child(glyph))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.month_off += delta;
                            cx.notify();
                        }))
                };
            return body.child(
                div()
                    .pt(px(2.))
                    .pr(px(8.))
                    .pb(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .pt(px(2.))
                            .pr(px(2.))
                            .pb(px(6.))
                            .pl(px(6.))
                            .child(
                                crate::fill_button("w-cal-title", transparent_black(), cx)
                                    .h_auto()
                                    .text_color(theme::fg())
                                    .child(
                                        div()
                                            .flex()
                                            .text_size(px(15.))
                                            .font_weight(FontWeight::BOLD)
                                            .line_height(relative(1.2))
                                            .child(label)
                                            .child(div().ml(px(4.)).child(year.to_string())),
                                    )
                                    .on_click(open(set_id.clone())),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(2.))
                                    .child(nav_btn("w-cal-prev", "‹", -1, cx))
                                    .child(nav_btn("w-cal-next", "›", 1, cx)),
                            ),
                    )
                    .child(grid),
            );
        }

        if view_type == "gallery" {
            let shown: Vec<&QueryRow> = self.rows.iter().take(GALLERY_LIMIT).collect();
            // Two columns only while every card's content fits half the width.
            let inner = self.side_width - 6. - 16. - 18. - 8.;
            let half = (inner - 8.) / 2.;
            let fits = shown.iter().all(|r| {
                let need = 2.
                    + 24.
                    + text_width(&row_icon(r), 15., FontWeight::MEDIUM, window)
                    + 8.
                    + text_width(&row_name(r), 13., FontWeight::MEDIUM, window);
                need <= half + 0.5
            });
            let cols = if fits { 2 } else { 1 };
            let card = |id: ElementId, current: bool| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .flex_1()
                    .px(px(12.))
                    .py(px(11.))
                    .rounded(theme::radius::menu())
                    .border_1()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::fg())
                    .overflow_hidden()
                    .cursor_pointer()
                    .map(|d| {
                        if current {
                            d.bg(theme::hl_med()).border_color(mix(
                                theme::border(),
                                0.4,
                                theme::fg(),
                            ))
                        } else {
                            d.bg(theme::hl_light())
                                .border_color(theme::border())
                                .hover(|s| s.border_color(mix(theme::border(), 0.55, theme::fg())))
                        }
                    })
            };
            let mut items: Vec<AnyElement> = shown
                .iter()
                .map(|r| {
                    card(crate::eid("w-card", &r.id), r.id == current_id)
                        .child(div().flex_none().text_size(px(15.)).child(row_icon(r)))
                        .child(div().min_w_0().truncate().child(row_name(r)))
                        .on_click(open(r.id.clone()))
                        .into_any_element()
                })
                .collect();
            if self.rows.len() > GALLERY_LIMIT {
                items.push(
                    card("w-card-more".into(), false)
                        .justify_center()
                        .border_dashed()
                        .text_color(theme::muted())
                        .font_weight(FontWeight::NORMAL)
                        .child(format!("+{} more", self.rows.len() - GALLERY_LIMIT))
                        .on_click(open(set_id.clone()))
                        .into_any_element(),
                );
            }
            let mut grid = div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .pt(px(4.))
                .pr(px(8.))
                .pb(px(6.));
            let mut items = items.into_iter().peekable();
            while items.peek().is_some() {
                let mut row = div().flex().gap(px(8.));
                for _ in 0..cols {
                    match items.next() {
                        Some(item) => row = row.child(div().flex_1().min_w_0().flex().child(item)),
                        None => row = row.child(div().flex_1()),
                    }
                }
                grid = grid.child(row);
            }
            return body.child(grid);
        }

        let mut body = body;
        for r in &self.rows {
            body = body.child(
                line(
                    crate::eid("w-row", &r.id),
                    r.id == current_id,
                    div()
                        .debug_selector(|| format!("w-row-{}", r.id))
                        .child(div().flex_none().text_size(px(11.)).child(row_icon(r)))
                        .child(div().min_w_0().truncate().child(row_name(r))),
                )
                .on_click(open(r.id.clone())),
            );
        }
        body
    }
}

/// Rendered width of `text` in the UI font.
fn text_width(text: &str, size: f32, weight: FontWeight, window: &mut Window) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let font = Font {
        family: theme::fonts::UI.into(),
        weight,
        ..Font::default()
    };
    let run = TextRun {
        len: text.len(),
        font,
        color: theme::fg(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .layout_line(text, px(size), &[run], None);
    f32::from(line.width)
}

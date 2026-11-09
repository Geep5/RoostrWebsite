//! Calendar view (CalendarView.svelte): a Monday-first month grid of six
//! weeks, records placed on their date property, prev / Today / next.

use std::collections::HashMap;

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use roostr_core::{QueryRow, State, api};
use roostr_ui::ty::Weights as _;
use roostr_ui::{Button, Tag, Theme, tooltip, ty};
use rune::{Color, Cursor, Cx, El, col, el, row, text};
use serde_json::json;

use crate::common::{self, engine_body};
use crate::views::ViewInput;
use crate::{Active, DataView, config, dv_mut};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

pub(crate) struct Calendar {
    input: ViewInput,
    rows: Vec<QueryRow>,
    generation: u64,
    year: i32,
    /// 0-based.
    month: u32,
}

impl Calendar {
    pub(crate) fn new(input: ViewInput) -> Self {
        let today = Local::now();
        Self {
            input,
            rows: Vec::new(),
            generation: 0,
            year: today.year(),
            month: today.month0(),
        }
    }

    pub(crate) fn row_ids(&self) -> impl Iterator<Item = &str> {
        self.rows.iter().map(|r| r.id.as_str())
    }

    fn step(&mut self, dir: i32) {
        let total = self.year * 12 + self.month as i32 + dir;
        self.year = total.div_euclid(12);
        self.month = total.rem_euclid(12) as u32;
    }
}

fn calendar_mut<'a>(s: &'a mut State, id: &str) -> Option<&'a mut Calendar> {
    match &mut dv_mut(s, id)?.active {
        Some(Active::Calendar(c)) => Some(c),
        _ => None,
    }
}

pub(crate) fn set_input(s: &mut State, id: &str, input: ViewInput) {
    let Some(c) = calendar_mut(s, id) else { return };
    if input != c.input {
        c.input = input;
        load(s, id);
    }
}

pub(crate) fn load(s: &mut State, id: &str) {
    let Some(c) = calendar_mut(s, id) else { return };
    let Some(body) = c.input.body.clone() else {
        return;
    };
    c.generation += 1;
    let generation = c.generation;
    let task = api::query(engine_body(&body, json!({})), s);
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(res) = task.await else { return };
        cx.update(|s| {
            if let Some(c) = calendar_mut(s, &view)
                && c.generation == generation
            {
                c.rows = res.records;
            }
        });
    })
    .detach();
}

fn ts_of(r: &QueryRow, date_key: &str) -> i64 {
    match date_key {
        "createdDate" => r.created_at,
        "modifiedDate" => r.updated_at,
        _ => r
            .fields
            .get(date_key)
            .and_then(|v| v.int_value.or(v.float_value.map(|f| f as i64)))
            .unwrap_or(0),
    }
}

/// A month-nav button (`.cal-nav button`), the kit outline button; the 12px
/// label sits in a child (the kit sizes its own label text).
fn nav_button(t: &Theme, key: String, label: &'static str) -> Button {
    Button::new(key)
        .child(text(label).font(ty::ui(12.)).color(t.fg))
        .outline()
        .h_auto()
        .px(10.)
        .py(3.)
        .radius(8.)
        .border_color(t.border)
        .bg(Color::TRANSPARENT)
        .text_color(t.fg)
        .cursor(Cursor::Pointer)
}

/// The month grid of the data view `v`.
pub(crate) fn view(
    s: &State,
    _cx: &mut Cx<State>,
    t: &Theme,
    _layer: &mut Vec<El>,
    v: &DataView,
) -> El {
    let Some(Active::Calendar(c)) = &v.active else {
        return el();
    };
    let Some(object) = s.store.object(&v.id) else {
        return el();
    };
    let id = v.id.clone();
    let date_key = config::date_key(&object.fields);
    let today = Local::now().date_naive();

    let mut by_day: HashMap<NaiveDate, Vec<&QueryRow>> = HashMap::new();
    for r in &c.rows {
        let ts = ts_of(r, date_key);
        if ts == 0 {
            continue;
        }
        if let Some(d) = Local.timestamp_millis_opt(ts).earliest() {
            by_day.entry(d.date_naive()).or_default().push(r);
        }
    }

    // 42 cells starting the Monday-week of the 1st.
    let first = NaiveDate::from_ymd_opt(c.year, c.month + 1, 1).unwrap_or(today);
    let start = first - Duration::days(first.weekday().num_days_from_monday() as i64);

    let mut grid = el().equal_columns(7).gap(4.);
    for i in 0..42 {
        let d = start + Duration::days(i);
        let in_month = d.month0() == c.month;
        let is_today = d == today;
        let items = by_day.get(&d).map(Vec::as_slice).unwrap_or(&[]);
        let number = ty::ui(11.);
        let mut day = col()
            .min_h(104.)
            .min_w(0.)
            .padding(6.)
            .radius(8.)
            .fill(t.hl_light)
            .gap(3.)
            .clip_rounded(8.)
            .border(
                1.,
                if is_today {
                    t.accent
                } else {
                    Color::TRANSPARENT
                },
            )
            .border_layout()
            .opacity(if in_month { 1. } else { 0.45 })
            .child(
                text(d.day().to_string())
                    .font(if is_today { number.bold() } else { number })
                    .color(if is_today { t.accent } else { t.muted }),
            );
        for r in items.iter().take(4) {
            let open = r.id.clone();
            let name = common::row_name(r);
            let item = row()
                .key(format!("{id}/cal-{i}-{}", r.id))
                .min_w(0.)
                .clip_rounded(0.)
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut State| common::open_object(&open, s))
                .child(
                    Tag::custom(t.panel, t.fg, t.border)
                        .rounded(6.)
                        .gap(4.)
                        .px(6.)
                        .py(2.)
                        .text_size(11.)
                        .child(common::icon(s, &common::row_icon(s, r), ty::ui(11.)))
                        // A flex button clips its text node: no ellipsis in the web either.
                        .child(text(name.clone()).font(ty::ui(11.)).color(t.fg))
                        .build(t),
                );
            day = day.child(tooltip(item, name));
        }
        if items.len() > 4 {
            day = day.child(
                text(format!("+{}", items.len() - 4))
                    .font(ty::ui(10.))
                    .color(t.muted)
                    .pl(4.),
            );
        }
        grid = grid.child(day);
    }

    let mut days = el().equal_columns(7).gap(4.);
    for d in DAYS {
        days = days.child(text(d).font(ty::ui(11.)).color(t.muted).px(6.));
    }

    let (prev, today_view, next) = (id.clone(), id.clone(), id.clone());
    col()
        .gap(8.)
        .pb(24.)
        .child(
            row()
                .center_y()
                .justify_between()
                .child(
                    text(format!("{} {}", MONTHS[c.month as usize], c.year))
                        .font(ty::ui(15.).semibold())
                        .color(t.fg),
                )
                .child(
                    row()
                        .gap(4.)
                        .child(
                            nav_button(t, format!("{id}/cal-prev"), "‹")
                                .accessibility_label("Previous month")
                                .on_press(move |s: &mut State| {
                                    if let Some(c) = calendar_mut(s, &prev) {
                                        c.step(-1);
                                    }
                                })
                                .build(t),
                        )
                        .child(
                            nav_button(t, format!("{id}/cal-today"), "Today")
                                .on_press(move |s: &mut State| {
                                    if let Some(c) = calendar_mut(s, &today_view) {
                                        let today = Local::now();
                                        c.year = today.year();
                                        c.month = today.month0();
                                    }
                                })
                                .build(t),
                        )
                        .child(
                            nav_button(t, format!("{id}/cal-next"), "›")
                                .accessibility_label("Next month")
                                .on_press(move |s: &mut State| {
                                    if let Some(c) = calendar_mut(s, &next) {
                                        c.step(1);
                                    }
                                })
                                .build(t),
                        ),
                ),
        )
        .child(days)
        .child(grid)
}

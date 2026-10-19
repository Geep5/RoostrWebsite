//! Calendar view (CalendarView.svelte): a Monday-first month grid of six
//! weeks, records placed on their date property, prev / Today / next.

use std::collections::HashMap;

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::{QueryRow, api, theme};
use serde_json::json;

use crate::common::{self, cached_object, engine_body};
use crate::config;
use crate::views::ViewInput;

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

pub(crate) struct CalendarView {
    object_id: String,
    input: ViewInput,
    rows: Vec<QueryRow>,
    generation: u64,
    year: i32,
    /// 0-based.
    month: u32,
}

impl CalendarView {
    pub(crate) fn new(object_id: String, input: ViewInput, cx: &mut Context<Self>) -> Self {
        let today = Local::now();
        let mut this = Self {
            object_id,
            input,
            rows: Vec::new(),
            generation: 0,
            year: today.year(),
            month: today.month0(),
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
        let task = api::query(engine_body(&body, json!({})), cx);
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

    fn step(&mut self, dir: i32, cx: &mut Context<Self>) {
        let total = self.year * 12 + self.month as i32 + dir;
        self.year = total.div_euclid(12);
        self.month = total.rem_euclid(12) as u32;
        cx.notify();
    }
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

fn nav_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .border_1()
        .border_color(theme::border())
        .rounded(px(8.))
        .px(px(10.))
        .py(px(3.))
        .text_size(px(12.))
        .text_color(theme::fg())
        .cursor_pointer()
        .hover(|s| s.border_color(theme::accent()))
        .child(label)
}

impl Render for CalendarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = cached_object(&self.object_id, cx) else {
            return div();
        };
        let date_key = config::date_key(&object.fields).to_string();
        let today = Local::now().date_naive();

        let mut by_day: HashMap<NaiveDate, Vec<&QueryRow>> = HashMap::new();
        for r in &self.rows {
            let ts = ts_of(r, &date_key);
            if ts == 0 {
                continue;
            }
            if let Some(d) = Local.timestamp_millis_opt(ts).earliest() {
                by_day.entry(d.date_naive()).or_default().push(r);
            }
        }

        // 42 cells starting the Monday-week of the 1st.
        let first = NaiveDate::from_ymd_opt(self.year, self.month + 1, 1).unwrap_or(today);
        let start = first - Duration::days(first.weekday().num_days_from_monday() as i64);

        let mut grid = div().grid().grid_cols(7).gap(px(4.));
        for i in 0..42 {
            let d = start + Duration::days(i);
            let in_month = d.month0() == self.month;
            let is_today = d == today;
            let items = by_day.get(&d).map(Vec::as_slice).unwrap_or(&[]);
            let mut day = div()
                .min_h(px(104.))
                .min_w_0()
                .p(px(6.))
                .rounded(px(8.))
                .bg(theme::hl_light())
                .flex()
                .flex_col()
                .gap(px(3.))
                .overflow_hidden()
                .border_1()
                .border_color(if is_today {
                    theme::accent()
                } else {
                    transparent_black()
                })
                .when(!in_month, |d| d.opacity(0.45))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(if is_today {
                            theme::accent()
                        } else {
                            theme::muted()
                        })
                        .when(is_today, |d| d.font_weight(FontWeight::BOLD))
                        .child(d.day().to_string()),
                );
            for r in items.iter().take(4) {
                let id = r.id.clone();
                let name = common::row_name(r);
                day = day.child(
                    div()
                        .id(SharedString::from(format!("cal-{}-{}", i, r.id)))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .min_w_0()
                        .bg(theme::panel())
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(6.))
                        .px(px(6.))
                        .py(px(2.))
                        .text_size(px(11.))
                        .text_color(theme::fg())
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .cursor_pointer()
                        .hover(|s| s.border_color(theme::accent()))
                        .tooltip(roostr_core::ui::tooltip(name.clone()))
                        .on_click(move |_, _, cx| common::open_object(&id, cx))
                        .child(div().flex_none().child(common::row_icon(r, cx)))
                        // A flex button clips its text node: no ellipsis in the web either.
                        .child(div().flex_none().child(name)),
                );
            }
            if items.len() > 4 {
                day = day.child(
                    div()
                        .text_size(px(10.))
                        .text_color(theme::muted())
                        .pl(px(4.))
                        .child(format!("+{}", items.len() - 4)),
                );
            }
            grid = grid.child(day);
        }

        let mut days = div().grid().grid_cols(7).gap(px(4.));
        for d in DAYS {
            days = days.child(
                div()
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .px(px(6.))
                    .child(d),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .pb(px(24.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} {}", MONTHS[self.month as usize], self.year)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(
                                nav_button("cal-prev", "‹")
                                    .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
                            )
                            .child(nav_button("cal-today", "Today").on_click(cx.listener(
                                |this, _, _, cx| {
                                    let today = Local::now();
                                    this.year = today.year();
                                    this.month = today.month0();
                                    cx.notify();
                                },
                            )))
                            .child(
                                nav_button("cal-next", "›")
                                    .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
                            ),
                    ),
            )
            .child(days)
            .child(grid)
    }
}

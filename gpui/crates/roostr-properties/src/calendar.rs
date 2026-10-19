//! Anytype's CalendarSelect, compact (`CalendarPicker.svelte`): month grid
//! with adjacent-month fill, today ring, selected day, prev/next month,
//! Today + Clear footer. Emits local-midnight unix ms, or `None` for clear.

use chrono::{Datelike as _, Local, NaiveDate, TimeZone as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::theme;

use crate::widgets::pop_shadow;

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
const WEEKDAYS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

pub(crate) enum CalendarEvent {
    /// Local midnight in ms, or `None` (Clear).
    Pick(Option<i64>),
    Close,
}

pub(crate) struct CalendarPicker {
    value: i64,
    year: i32,
    /// 1-12.
    month: u32,
    focus: FocusHandle,
}

impl EventEmitter<CalendarEvent> for CalendarPicker {}

/// Local midnight of a calendar day as unix ms.
pub(crate) fn local_midnight_ms(date: NaiveDate) -> i64 {
    let naive = date.and_hms_opt(0, 0, 0).expect("midnight");
    Local.from_local_datetime(&naive).earliest().map_or_else(
        || naive.and_utc().timestamp_millis(),
        |d| d.timestamp_millis(),
    )
}

/// The local calendar day of unix ms.
pub(crate) fn local_date(ms: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map_or_else(|| Local::now().date_naive(), |d| d.date_naive())
}

impl CalendarPicker {
    /// `value`: unix ms, 0 = unset. Seeded once: the picker is made per open.
    pub(crate) fn new(value: i64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let seed = if value != 0 {
            local_date(value)
        } else {
            Local::now().date_naive()
        };
        let focus = cx.focus_handle();
        let f = focus.clone();
        window.defer(cx, move |window, cx| window.focus(&f, cx));
        Self {
            value,
            year: seed.year(),
            month: seed.month(),
            focus,
        }
    }

    fn step(&mut self, delta: i32, cx: &mut Context<Self>) {
        let idx = self.year * 12 + self.month as i32 - 1 + delta;
        self.year = idx.div_euclid(12);
        self.month = idx.rem_euclid(12) as u32 + 1;
        cx.notify();
    }

    fn days(&self) -> Vec<(NaiveDate, bool)> {
        let first = NaiveDate::from_ymd_opt(self.year, self.month, 1).expect("valid month");
        let lead = first.weekday().num_days_from_monday() as i64;
        (0..42)
            .map(|i| {
                let d = first + chrono::Duration::days(i - lead);
                (d, d.month() != self.month)
            })
            .collect()
    }
}

impl Focusable for CalendarPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn foot_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(6.))
        .py(px(2.))
        .rounded(theme::radius::sm())
        .text_size(px(12.))
        .text_color(theme::muted())
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
        .child(label)
}

impl Render for CalendarPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let today = Local::now().date_naive();
        let selected = (self.value != 0).then(|| local_date(self.value));
        let nav = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px(px(8.))
                .py(px(2.))
                .rounded(theme::radius::sm())
                .text_size(px(16.))
                .text_color(theme::muted())
                .cursor_pointer()
                .hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
                .child(label)
        };
        // A 7-column grid: one flex row per week, equal cells (1px gaps like the web grid).
        let mut head = div().flex().gap(px(1.));
        for w in WEEKDAYS {
            head = head.child(
                div()
                    .flex_1()
                    .py(px(3.))
                    .text_center()
                    .text_size(px(10.))
                    .text_color(theme::muted())
                    .child(w),
            );
        }
        let mut grid = div().flex().flex_col().gap(px(1.)).child(head);
        for week in self.days().chunks(7) {
            let mut row = div().flex().gap(px(1.));
            for &(date, other) in week {
                let is_sel = selected == Some(date);
                let is_today = date == today;
                row = row.child(
                    div()
                        .id(SharedString::from(date.to_string()))
                        .flex_1()
                        .py(px(5.))
                        .text_center()
                        .text_size(px(12.))
                        .rounded(theme::radius::sm())
                        .cursor_pointer()
                        .when(other && !is_sel, |d| {
                            d.text_color(theme::muted()).opacity(0.5)
                        })
                        .when(is_today, |d| d.border_1().border_color(theme::accent()))
                        .when(is_sel, |d| {
                            d.bg(theme::accent())
                                .text_color(rgb(0x14100a))
                                .font_weight(FontWeight::SEMIBOLD)
                        })
                        .when(!is_sel, |d| d.hover(|s| s.bg(theme::hover())))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(CalendarEvent::Pick(Some(local_midnight_ms(date))))
                        }))
                        .child(date.day().to_string()),
                );
            }
            grid = grid.child(row);
        }
        div()
            .id("calendar")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    cx.emit(CalendarEvent::Close);
                }
            }))
            .w(px(252.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::menu())
            .p(px(10.))
            .shadow(pop_shadow())
            .text_color(theme::fg())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(8.))
                    .child(
                        nav("prev", "‹").on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} {}", MONTHS[self.month as usize - 1], self.year)),
                    )
                    .child(
                        nav("next", "›").on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
                    ),
            )
            .child(grid)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .mt(px(8.))
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(theme::border())
                    .child(foot_button("today", "Today").on_click(cx.listener(
                        move |_, _, _, cx| {
                            cx.emit(CalendarEvent::Pick(Some(local_midnight_ms(today))))
                        },
                    )))
                    .child(
                        foot_button("clear", "Clear").on_click(
                            cx.listener(|_, _, _, cx| cx.emit(CalendarEvent::Pick(None))),
                        ),
                    ),
            )
    }
}

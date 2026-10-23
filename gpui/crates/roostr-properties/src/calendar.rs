//! Anytype's CalendarSelect, compact (`CalendarPicker.svelte`) on the kit
//! calendar: month grid (Monday first) with adjacent-month fill, today ring,
//! selected day, prev/next month, Today + Clear footer. Emits local-midnight
//! unix ms, or `None` for clear.
//!
//! The kit owns the calendar's state and structure (month paging, the month
//! and year pickers behind the title, day activation); Roostr draws its cells
//! through the calendar's item slot and wraps it in its own surface, footer
//! and Escape handling. The kit's styled `time::Calendar` facade hardcodes its
//! cell look (filled today, 32px chrome), so the base calendar it wraps is
//! used directly here with Roostr's cells.

use chrono::{Local, NaiveDate, TimeZone as _, Weekday};
use gpui_kit::base::{Calendar, CalendarItemKind};
use gpui_kit::component::calendar::{CalendarEvent as KitCalendarEvent, CalendarState, Date};
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
/// Indexed from Sunday, as the kit labels weekdays.
const WEEKDAYS: [&str; 7] = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
/// One day column: the 252px surface less its padding and border, over 7.
const CELL_W: f32 = 32.;

pub(crate) enum CalendarEvent {
    /// Local midnight in ms, or `None` (Clear).
    Pick(Option<i64>),
    Close,
}

pub(crate) struct CalendarPicker {
    state: Entity<CalendarState>,
    focus: FocusHandle,
    _sub: Subscription,
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
        let state = cx.new(|cx| {
            let mut state = CalendarState::new(window, cx);
            if value != 0 {
                // Also pages the grid to the value's month; unset opens on today's.
                state.apply_date(Date::Single(Some(local_date(value))));
            }
            state
        });
        let sub = cx.subscribe(&state, |_, _, ev: &KitCalendarEvent, cx| {
            let KitCalendarEvent::Selected(date) = ev;
            if let Some(day) = date.start() {
                cx.emit(CalendarEvent::Pick(Some(local_midnight_ms(day))));
            }
        });
        let focus = cx.focus_handle();
        let f = focus.clone();
        window.defer(cx, move |window, cx| window.focus(&f, cx));
        Self {
            state,
            focus,
            _sub: sub,
        }
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

/// What a calendar control reads as.
fn label(kind: CalendarItemKind, value: i32) -> SharedString {
    match kind {
        CalendarItemKind::Previous => "‹".into(),
        CalendarItemKind::Next => "›".into(),
        CalendarItemKind::Weekday => WEEKDAYS[value.rem_euclid(7) as usize].into(),
        CalendarItemKind::MonthToggle | CalendarItemKind::Month => MONTHS
            .get((value - 1).max(0) as usize)
            .copied()
            .unwrap_or_default()
            .into(),
        _ => value.to_string().into(),
    }
}

impl Render for CalendarPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let calendar = Calendar::new("calendar-grid", &self.state)
            .first_day_of_week(Weekday::Mon)
            .label(label)
            .item(|item, st, _, _| {
                let kind = st.kind();
                match kind {
                    CalendarItemKind::Previous | CalendarItemKind::Next => item
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(theme::radius::sm())
                        .text_size(px(16.))
                        .text_color(theme::muted())
                        .when(st.is_disabled(), |d| d.opacity(0.3))
                        .when(!st.is_disabled(), |d| {
                            d.cursor_pointer()
                                .hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
                        })
                        .into_any_element(),
                    // "October 2026": the two halves meet in the middle; each
                    // opens the kit's month / year picker.
                    CalendarItemKind::MonthToggle | CalendarItemKind::YearToggle => item
                        .flex_1()
                        .when(kind == CalendarItemKind::MonthToggle, |d| {
                            d.text_right().pr(px(2.))
                        })
                        .when(kind == CalendarItemKind::YearToggle, |d| {
                            d.text_left().pl(px(2.))
                        })
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .when(st.is_active(), |d| d.text_color(theme::accent()))
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme::accent()))
                        .into_any_element(),
                    CalendarItemKind::Weekday => item
                        .w(px(CELL_W))
                        .mx(px(0.5))
                        .py(px(3.))
                        .text_center()
                        .text_size(px(10.))
                        .text_color(theme::muted())
                        .into_any_element(),
                    CalendarItemKind::Day => {
                        let (sel, other) = (st.is_active(), st.is_muted());
                        item.w(px(CELL_W))
                            .m(px(0.5))
                            .py(px(5.))
                            .text_center()
                            .text_size(px(12.))
                            .rounded(theme::radius::sm())
                            .cursor_pointer()
                            .when(other && !sel, |d| d.text_color(theme::muted()).opacity(0.5))
                            .when(st.is_today(), |d| {
                                d.border_1().border_color(theme::accent())
                            })
                            .when(sel, |d| {
                                d.bg(theme::accent())
                                    .text_color(rgb(0x14100a))
                                    .font_weight(FontWeight::SEMIBOLD)
                            })
                            .when(!sel, |d| d.hover(|s| s.bg(theme::hover())))
                            .into_any_element()
                    }
                    // The month and year pickers' cells.
                    _ => item
                        .py(px(5.))
                        .my(px(1.))
                        .text_center()
                        .text_size(px(12.))
                        .rounded(theme::radius::sm())
                        .cursor_pointer()
                        .when(st.is_active(), |d| {
                            d.bg(theme::accent())
                                .text_color(rgb(0x14100a))
                                .font_weight(FontWeight::SEMIBOLD)
                        })
                        .when(!st.is_active(), |d| d.hover(|s| s.bg(theme::hover())))
                        .into_any_element(),
                }
            })
            .gap(px(8.));
        let today = Local::now().date_naive();
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
            .child(calendar)
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

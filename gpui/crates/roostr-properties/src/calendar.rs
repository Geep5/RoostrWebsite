//! Anytype's CalendarSelect, compact (`CalendarPicker.svelte`): month grid
//! (Monday first) with adjacent-month fill, today ring, selected day,
//! prev/next month, Today + Clear footer. Emits local-midnight unix ms, or
//! `None` for clear.
//!
//! roostr-ui's [`CalendarState`] owns the paging (month, the month and year
//! pickers behind the title, day activation) and its [`DatePicker`] surface
//! draws Roostr's cells, surface and footer; this module seeds it from a
//! stored value, converts days to local-midnight ms and closes on Escape or
//! a press outside.

use std::rc::Rc;

use chrono::{Local, NaiveDate, TimeZone as _, Weekday};
use roostr_core::State;
use roostr_ui::{CalendarState, Date, DatePicker, Theme};
use rune::El;

use crate::widgets::Lens;

pub(crate) enum CalendarEvent {
    /// Local midnight in ms, or `None` (Clear).
    Pick(Option<i64>),
    Close,
}

/// One open calendar: made per open, seeded with the value it shows.
pub(crate) struct CalendarPickerState {
    calendar: CalendarState,
}

impl CalendarPickerState {
    /// `value_ms`: unix ms, 0 = unset. A set value also pages the grid to its
    /// month; unset opens on today's.
    pub(crate) fn new(value_ms: i64) -> Self {
        let mut calendar = CalendarState::new();
        if value_ms != 0 {
            calendar.set_date(Date::Single(Some(local_date(value_ms))));
        }
        Self { calendar }
    }
}

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

/// The picker surface (252 wide, panel, menu radius, pop shadow, Today /
/// Clear footer), keyed `{key}-surface`; days are `{key}-calendar-YYYY-MM-DD`,
/// the footer `{key}-today` / `{key}-clear`. It holds the keyboard while
/// open: Escape or a press outside it emits [`CalendarEvent::Close`]. The
/// caller places it (floating under its trigger, or in the flow).
pub(crate) fn calendar_picker(
    t: &Theme,
    key: &str,
    state: &CalendarPickerState,
    lens: Lens<CalendarPickerState>,
    on_event: Rc<dyn Fn(&mut State, CalendarEvent)>,
) -> El {
    let pick = on_event.clone();
    DatePicker::new(key, &state.calendar, move |s: &mut State| {
        &mut lens(s).calendar
    })
    .first_day_of_week(Weekday::Mon)
    .on_pick(move |s: &mut State, day: Option<NaiveDate>| {
        pick(s, CalendarEvent::Pick(day.map(local_midnight_ms)))
    })
    .surface(t)
    .modal(true, move |s: &mut State| on_event(s, CalendarEvent::Close))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midnight_round_trips_through_the_local_day() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let ms = local_midnight_ms(day);
        assert_eq!(local_date(ms), day);
        assert_eq!(local_date(ms + 23 * 3_600_000), day);
    }

    #[test]
    fn a_set_value_pages_to_its_month() {
        let day = NaiveDate::from_ymd_opt(2024, 2, 29).unwrap();
        let st = CalendarPickerState::new(local_midnight_ms(day));
        assert_eq!(
            (st.calendar.current_year(), st.calendar.current_month()),
            (2024, 2)
        );
        assert_eq!(st.calendar.date(), Date::Single(Some(day)));
    }
}

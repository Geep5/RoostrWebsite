//! Dates as the app shows them (`src/lib/dates.ts`): the month's emoji, then
//! the en-US locale date, in local time.

use chrono::{Datelike, Local, TimeZone};

/// One emoji per month (index = month0).
pub const MONTH_EMOJI: [&str; 12] = [
    "🍾", "❤️", "🍀", "🐰", "🌷", "☀️", "🧨", "🌊", "🎒", "🎃", "🦃", "⛄",
];

/// The `Intl.DateTimeFormatOptions` the web passes to `showDate`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateStyle {
    /// No options: "10/18/2026".
    #[default]
    Numeric,
    /// `{year: "numeric", month: "short", day: "numeric"}`: "Oct 18, 2026".
    MonthDayYear,
    /// `{month: "short", day: "numeric"}`: "Oct 18".
    MonthDay,
}

/// `showDate(ms)`: "🎃 10/18/2026".
pub fn show_date(ms: i64) -> String {
    show_date_with(ms, DateStyle::Numeric)
}

pub fn show_date_with(ms: i64, style: DateStyle) -> String {
    let Some(d) = Local.timestamp_millis_opt(ms).earliest() else {
        return String::new();
    };
    let date = match style {
        DateStyle::Numeric => d.format("%-m/%-d/%Y").to_string(),
        DateStyle::MonthDayYear => d.format("%b %-d, %Y").to_string(),
        DateStyle::MonthDay => d.format("%b %-d").to_string(),
    };
    format!("{} {date}", MONTH_EMOJI[d.month0() as usize])
}

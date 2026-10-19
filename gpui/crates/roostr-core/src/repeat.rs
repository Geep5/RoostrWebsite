//! Words for a recurring object's rule (`src/lib/repeat.ts`), shared by the
//! Repeat editor and the table's Repeat column ("Every week on Wed at 9:00 AM").
//! The engine owns the rule and the occurrence math. Dates are local time,
//! formatted like the web's en-US locale.

use chrono::{Datelike, Local, NaiveDate, NaiveTime, TimeZone, Timelike};

use crate::types::{Repeat, RepeatFreq};

pub const DAY: i64 = 86_400_000;
pub const MIN: i64 = 60_000;
pub const DAY_MINUTES: u32 = 1440;
pub const WD: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub const ORD: [&str; 6] = ["", "1st", "2nd", "3rd", "4th", "last"];

/// The editable shape: the stored rule with `anchor` as local start-of-day ms.
#[derive(Clone, Debug, PartialEq)]
pub struct RepeatDraft {
    pub freq: RepeatFreq,
    pub interval: u32,
    pub weekdays: Vec<u8>,
    /// "date" | "weekday"
    pub monthly: String,
    /// Local midnight (epoch ms) of the day the cadence counts from.
    pub anchor: i64,
    /// Day/week/month/year: occurrence times (minutes after local midnight).
    pub times: Vec<u32>,
    /// Minute/hour: the minutes the grid runs in; `None` = the whole day.
    pub window: Option<[u32; 2]>,
}

pub fn freq_name(freq: RepeatFreq) -> &'static str {
    match freq {
        RepeatFreq::Minute => "minute",
        RepeatFreq::Hour => "hour",
        RepeatFreq::Day => "day",
        RepeatFreq::Week => "week",
        RepeatFreq::Month => "month",
        RepeatFreq::Year => "year",
    }
}

fn local(ms: i64) -> chrono::DateTime<Local> {
    Local
        .timestamp_millis_opt(ms)
        .earliest()
        .unwrap_or_else(|| Local.timestamp_millis_opt(0).unwrap())
}

fn midnight(date: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&date.and_time(NaiveTime::MIN))
        .earliest()
        .map_or(0, |d| d.timestamp_millis())
}

/// Local start of the day containing `ms`.
pub fn sod(ms: i64) -> i64 {
    midnight(local(ms).date_naive())
}

/// Local midnight of a wall-clock day index (the engine's `anchor`).
pub fn day_to_local(day: i64) -> i64 {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid date");
    midnight(epoch + chrono::Duration::days(day))
}

/// "Oct 18".
pub fn fmt(ms: i64) -> String {
    local(ms).format("%b %-d").to_string()
}

/// "9:00 AM" for minutes after midnight.
pub fn fmt_time(minutes: u32) -> String {
    clock(minutes / 60 % 24, minutes % 60)
}

/// "9:00 AM" for the local time of `ms`.
pub fn fmt_clock(ms: i64) -> String {
    let t = local(ms);
    clock(t.hour(), t.minute())
}

fn clock(hour: u32, minute: u32) -> String {
    let (h, ampm) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!("{h}:{minute:02} {ampm}")
}

/// Which weekday-of-month `ms` is: 1..4, or 5 = the last one.
pub fn ordinal_of(ms: i64) -> usize {
    let d = local(ms);
    let n = d.day().div_ceil(7) as usize;
    let (y, m) = if d.month() == 12 {
        (d.year() + 1, 1)
    } else {
        (d.year(), d.month() + 1)
    };
    let last = NaiveDate::from_ymd_opt(y, m, 1)
        .and_then(|f| f.pred_opt())
        .map_or(31, |l| l.day());
    if d.day() + 7 > last { 5 } else { n }
}

pub fn suffix(n: u32) -> &'static str {
    if n % 10 == 1 && n != 11 {
        "st"
    } else if n % 10 == 2 && n != 12 {
        "nd"
    } else if n % 10 == 3 && n != 13 {
        "rd"
    } else {
        "th"
    }
}

/// Times as the engine stores them: ascending, each once.
pub fn sorted_times(times: &[u32]) -> Vec<u32> {
    let mut t = times.to_vec();
    t.sort_unstable();
    t.dedup();
    t
}

/// Runs more than once a day, so "when next" needs the time.
pub fn many_a_day(freq: RepeatFreq, times: &[u32]) -> bool {
    freq.is_sub_daily() || times.len() > 1
}

pub fn to_draft(r: &Repeat) -> RepeatDraft {
    RepeatDraft {
        freq: r.freq,
        interval: r.interval,
        weekdays: r.weekdays.clone(),
        monthly: r.monthly.clone(),
        anchor: day_to_local(r.anchor),
        // A minute/hour rule has no times; switching it to daily starts from 9:00.
        times: if r.times.is_empty() {
            vec![9 * 60]
        } else {
            r.times.clone()
        },
        // The engine stores the whole day as [0, 1439]: that is no window.
        window: r.window.filter(|w| w[0] > 0 || w[1] < DAY_MINUTES - 1),
    }
}

/// "Mon–Fri" for a run of three or more days, else "Mon, Wed".
fn day_list(weekdays: &[u8]) -> String {
    let mut wd = weekdays.to_vec();
    wd.sort_unstable();
    wd.dedup();
    let run = wd.len() >= 3 && wd.windows(2).all(|p| p[1] == p[0] + 1);
    let name = |x: u8| WD.get(x as usize).copied().unwrap_or("");
    if run {
        format!("{}–{}", name(wd[0]), name(wd[wd.len() - 1]))
    } else {
        wd.iter().map(|x| name(*x)).collect::<Vec<_>>().join(", ")
    }
}

pub fn describe_draft(d: &RepeatDraft) -> String {
    let every = if d.interval == 1 {
        "Every".to_string()
    } else {
        format!("Every {}", d.interval)
    };
    let freq = freq_name(d.freq);
    let unit = if d.interval == 1 {
        freq.to_string()
    } else {
        format!("{freq}s")
    };
    if d.freq.is_sub_daily() {
        let days = if d.weekdays.is_empty() {
            String::new()
        } else {
            format!(" on {}", day_list(&d.weekdays))
        };
        let span = d.window.map_or(String::new(), |w| {
            format!(", {}–{}", fmt_time(w[0]), fmt_time(w[1]))
        });
        return format!("{every} {unit}{days}{span}");
    }
    let anchor = local(d.anchor);
    let weekday = WD[anchor.weekday().num_days_from_sunday() as usize];
    let s = match d.freq {
        RepeatFreq::Week => {
            let mut wd = d.weekdays.clone();
            wd.sort_unstable();
            if wd == [1, 2, 3, 4, 5] && d.interval == 1 {
                "Every weekday".to_string()
            } else {
                format!(
                    "{every} {unit} on {}",
                    if wd.is_empty() {
                        weekday.to_string()
                    } else {
                        day_list(&wd)
                    }
                )
            }
        }
        RepeatFreq::Month if d.monthly == "weekday" => format!(
            "{every} {unit} on the {} {weekday}",
            ORD[ordinal_of(d.anchor)]
        ),
        RepeatFreq::Month => format!(
            "{every} {unit} on the {}{}",
            anchor.day(),
            suffix(anchor.day())
        ),
        RepeatFreq::Year => format!("{every} {unit} on {}", fmt(d.anchor)),
        _ => format!("{every} {unit}"),
    };
    let times: Vec<String> = sorted_times(&d.times).into_iter().map(fmt_time).collect();
    format!("{s} at {}", times.join(", "))
}

/// A table cell's words: the rule, when it next runs, and why that run moved.
pub fn describe_repeat(rule: &Repeat) -> String {
    let when = if many_a_day(rule.freq, &rule.times) {
        format!("{}, {}", fmt(rule.next), fmt_clock(rule.next))
    } else {
        fmt(rule.next)
    };
    let moved = match &rule.moved {
        Some(m) if !m.reason.is_empty() => format!(" (moved: {})", m.reason),
        Some(m) => format!(" (moved from {})", fmt(m.from)),
        None => String::new(),
    };
    format!("{} · next {when}{moved}", describe_draft(&to_draft(rule)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(freq: RepeatFreq, interval: u32, weekdays: &[u8], times: &[u32]) -> RepeatDraft {
        // 2026-10-14 is a Wednesday.
        let anchor = midnight(NaiveDate::from_ymd_opt(2026, 10, 14).unwrap());
        RepeatDraft {
            freq,
            interval,
            weekdays: weekdays.to_vec(),
            monthly: "date".into(),
            anchor,
            times: times.to_vec(),
            window: None,
        }
    }

    #[test]
    fn descriptions() {
        assert_eq!(
            describe_draft(&draft(RepeatFreq::Week, 1, &[], &[540])),
            "Every week on Wed at 9:00 AM"
        );
        assert_eq!(
            describe_draft(&draft(RepeatFreq::Week, 1, &[5, 1, 2, 3, 4], &[540])),
            "Every weekday at 9:00 AM"
        );
        assert_eq!(
            describe_draft(&draft(RepeatFreq::Week, 2, &[1, 3], &[600, 540, 600])),
            "Every 2 weeks on Mon, Wed at 9:00 AM, 10:00 AM"
        );
        assert_eq!(
            describe_draft(&draft(RepeatFreq::Week, 1, &[0, 1, 2], &[0])),
            "Every week on Sun–Tue at 12:00 AM"
        );
        assert_eq!(
            describe_draft(&draft(RepeatFreq::Month, 1, &[], &[780])),
            "Every month on the 14th at 1:00 PM"
        );
        let mut by_weekday = draft(RepeatFreq::Month, 3, &[], &[540]);
        by_weekday.monthly = "weekday".into();
        assert_eq!(
            describe_draft(&by_weekday),
            "Every 3 months on the 2nd Wed at 9:00 AM"
        );
        assert_eq!(
            describe_draft(&draft(RepeatFreq::Year, 1, &[], &[540])),
            "Every year on Oct 14 at 9:00 AM"
        );
        let mut hourly = draft(RepeatFreq::Hour, 2, &[1, 2, 3, 4, 5], &[]);
        hourly.window = Some([540, 1020]);
        assert_eq!(
            describe_draft(&hourly),
            "Every 2 hours on Mon–Fri, 9:00 AM–5:00 PM"
        );
        assert_eq!(suffix(11), "th");
        assert_eq!(suffix(22), "nd");
    }
}

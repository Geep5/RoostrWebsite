//! Recurring objects (`Repeat.svelte`): the engine owns the rule
//! (`fields.repeat`) and the occurrence math; this renders when it next
//! runs and lets a person edit the cadence, run it now, skip the open run or
//! turn repeating off.

use std::collections::HashMap;
use std::rc::Rc;

use chrono::{Datelike as _, Local, NaiveDate, TimeZone as _};
use roostr_core::repeat::{
    DAY, DAY_MINUTES, RepeatDraft, WD, after_next, describe_draft, draft_slots, fit_more,
    last_message_ms, ordinal_of, period_name, slotted, sod, sorted_times, to_draft,
};
use roostr_core::types::{guest_agents, repeat_of};
use roostr_core::{Fields, Object, RepeatFreq, RepeatRule, State, Value, api};
use roostr_rt::Task;
use roostr_ui::{
    Button, ButtonCustomVariant, Glyph, Input, InputState, Select, SelectItem, SelectState, Theme,
    Toggle, ToggleGroup, radius, ty,
};
use rune::{Color, Cursor, Cx, El, col, el, row, text};

use crate::calendar::{
    CalendarEvent, CalendarPickerState, calendar_picker, local_date, local_midnight_ms,
};
use crate::widgets::{
    FieldSkin, backdrop, below, error_alert, field, lens, note, placeholder, pop_shadow, tip,
};

const FREQS: [RepeatFreq; 6] = [
    RepeatFreq::Minute,
    RepeatFreq::Hour,
    RepeatFreq::Day,
    RepeatFreq::Week,
    RepeatFreq::Month,
    RepeatFreq::Year,
];

fn freq_word(freq: RepeatFreq, plural: bool) -> String {
    let w = roostr_core::repeat::freq_name(freq);
    if plural {
        format!("{w}s")
    } else {
        w.to_string()
    }
}

/// The units a quiet timer waits in.
const AFTER_FREQS: [RepeatFreq; 3] = [RepeatFreq::Minute, RepeatFreq::Hour, RepeatFreq::Day];

/// The largest interval: hourly calendar rules stay within a day; a wait does not.
fn max_interval(freq: RepeatFreq, after: bool) -> u32 {
    if freq == RepeatFreq::Hour && !after {
        23
    } else {
        999
    }
}

fn date_of(ms: i64) -> NaiveDate {
    local_date(ms)
}

fn weekday(ms: i64) -> u8 {
    date_of(ms).weekday().num_days_from_sunday() as u8
}

/// "Sat, Oct 18, 9:00 AM" (`toLocaleString` weekday/month short, day, hour, 2-digit minute).
pub(crate) fn fmt_long(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map_or_else(String::new, |d| {
            d.format("%a, %b %-d, %-I:%M %p").to_string()
        })
}

fn hhmm(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// "HH:MM" as minutes after midnight, or None while incomplete.
fn minutes_of(value: &str) -> Option<u32> {
    let (h, m) = value.trim().split_once(':')?;
    let (h, m) = (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Local midnight `n` days after local midnight `ms`: the half day of slack absorbs a DST hour.
fn add_days(ms: i64, n: i64) -> i64 {
    sod(ms + n * DAY + DAY / 2)
}

fn add_months(ms: i64, n: i64, day: u32) -> i64 {
    let d = date_of(ms);
    let idx = d.year() as i64 * 12 + d.month0() as i64 + n;
    let (y, m) = (idx.div_euclid(12) as i32, idx.rem_euclid(12) as u32 + 1);
    let last = last_day(y, m);
    local_midnight_ms(NaiveDate::from_ymd_opt(y, m, day.min(last)).expect("valid"))
}

fn last_day(y: i32, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|f| f.pred_opt())
        .map_or(31, |l| l.day())
}

/// "2nd Tuesday" of the month containing `ms`; ordinal 5 = last.
fn nth_weekday(ms: i64, wd: u8, ordinal: usize) -> i64 {
    let d = date_of(ms);
    if ordinal == 5 {
        let mut day = NaiveDate::from_ymd_opt(d.year(), d.month(), last_day(d.year(), d.month()))
            .expect("valid");
        while day.weekday().num_days_from_sunday() as u8 != wd {
            day = day.pred_opt().expect("valid");
        }
        return local_midnight_ms(day);
    }
    let mut day = NaiveDate::from_ymd_opt(d.year(), d.month(), 1).expect("valid");
    while day.weekday().num_days_from_sunday() as u8 != wd {
        day = day.succ_opt().expect("valid");
    }
    local_midnight_ms(day + chrono::Duration::days(7 * (ordinal as i64 - 1)))
}

/// Whether the rule runs on the local day starting at `ms`.
fn day_fits(d: &RepeatDraft, ms: i64) -> bool {
    if d.freq.is_sub_daily() {
        return d.weekdays.is_empty() || d.weekdays.contains(&weekday(ms));
    }
    d.freq != RepeatFreq::Week || d.weekdays.contains(&weekday(ms))
}

/// Next occurrence day strictly after `from` (preview only; the engine computes `next`).
fn step_from(d: &RepeatDraft, from: i64) -> i64 {
    // Each period its own times: every period is a candidate.
    let interval = if slotted(d) { 1 } else { d.interval };
    let d = &RepeatDraft {
        interval,
        ..d.clone()
    };
    match d.freq {
        RepeatFreq::Minute | RepeatFreq::Hour => {
            let mut cur = add_days(from, 1);
            for _ in 0..7 {
                if day_fits(d, cur) {
                    break;
                }
                cur = add_days(cur, 1);
            }
            cur
        }
        RepeatFreq::Day => add_days(from, d.interval as i64),
        RepeatFreq::Week => {
            let days = if d.weekdays.is_empty() {
                vec![weekday(d.anchor)]
            } else {
                d.weekdays.clone()
            };
            let week_start = |ms: i64| add_days(ms, -(weekday(ms) as i64));
            let w0 = week_start(d.anchor);
            let mut cur = add_days(from, 1);
            for _ in 0..400 {
                let weeks = ((week_start(cur) - w0) as f64 / (7 * DAY) as f64).round() as i64;
                if weeks % d.interval as i64 == 0 && days.contains(&weekday(cur)) {
                    return cur;
                }
                cur = add_days(cur, 1);
            }
            cur
        }
        RepeatFreq::Month => {
            if d.monthly == "weekday" {
                let m = add_months(from, d.interval as i64, 1);
                return nth_weekday(m, weekday(d.anchor), ordinal_of(d.anchor));
            }
            add_months(from, d.interval as i64, date_of(d.anchor).day())
        }
        RepeatFreq::Year => {
            let f = date_of(from);
            let y = f.year() + d.interval as i32;
            let date = NaiveDate::from_ymd_opt(y, f.month(), f.day())
                .unwrap_or_else(|| NaiveDate::from_ymd_opt(y, 3, 1).expect("valid"));
            local_midnight_ms(date)
        }
    }
}

/// Which period of the cycle the day `ms` is in (0 = the anchor's).
fn period_of(d: &RepeatDraft, ms: i64) -> usize {
    let (a, b) = (date_of(d.anchor), date_of(ms));
    let periods = match d.freq {
        RepeatFreq::Week => {
            let start =
                |x: NaiveDate| x - chrono::Days::new(x.weekday().num_days_from_sunday() as u64);
            (start(b) - start(a)).num_days() / 7
        }
        RepeatFreq::Month => {
            (b.year() as i64 * 12 + b.month() as i64) - (a.year() as i64 * 12 + a.month() as i64)
        }
        RepeatFreq::Year => (b.year() - a.year()) as i64,
        _ => (b - a).num_days(),
    };
    periods.rem_euclid(d.interval.max(1) as i64) as usize
}

/// The occurrence times on the day `ms` (none: its period is off).
fn day_times(d: &RepeatDraft, ms: i64) -> Vec<u32> {
    if !d.freq.is_sub_daily() {
        return match period_of(d, ms) {
            k if k > 0 && slotted(d) => sorted_times(&d.more[k - 1]),
            _ => sorted_times(&d.times),
        };
    }
    let [from, until] = d.window.unwrap_or([0, DAY_MINUTES - 1]);
    let period = d.interval * if d.freq == RepeatFreq::Hour { 60 } else { 1 };
    (from..=until).step_by(period.max(1) as usize).collect()
}

/// The next five occurrences from now; a quiet timer's one next run, the wait
/// after the newest message (`last_message`) or now, whichever is later.
fn preview(d: &RepeatDraft, last_message: i64) -> Vec<i64> {
    let now = chrono::Utc::now().timestamp_millis();
    if d.after {
        return vec![after_next(d, last_message.max(now))];
    }
    let mut out = Vec::new();
    let mut day = if day_fits(d, d.anchor) {
        d.anchor
    } else {
        step_from(d, d.anchor)
    };
    for _ in 0..400 {
        if out.len() >= 5 {
            break;
        }
        let date = date_of(day);
        for t in &day_times(d, day) {
            let naive = date.and_hms_opt(t / 60, t % 60, 0).expect("valid time");
            let at = Local
                .from_local_datetime(&naive)
                .earliest()
                .map_or(0, |x| x.timestamp_millis());
            if out.len() < 5 && at > now {
                out.push(at);
            }
        }
        day = step_from(d, day);
    }
    out
}

fn same_rule(a: &RepeatDraft, b: &RepeatDraft) -> bool {
    let mut aw = a.weekdays.clone();
    let mut bw = b.weekdays.clone();
    aw.sort_unstable();
    bw.sort_unstable();
    a.freq == b.freq
        && a.interval == b.interval
        && a.monthly == b.monthly
        && a.anchor == b.anchor
        && sorted_times(&a.times) == sorted_times(&b.times)
        && draft_slots(a) == draft_slots(b)
        && a.window == b.window
        && a.after == b.after
        && aw == bw
}

fn seed() -> RepeatDraft {
    let anchor = sod(chrono::Utc::now().timestamp_millis());
    RepeatDraft {
        freq: RepeatFreq::Week,
        interval: 1,
        weekdays: vec![weekday(anchor)],
        monthly: "date".into(),
        anchor,
        times: vec![9 * 60],
        more: Vec::new(),
        window: None,
        after: false,
    }
}

/// A time field typed into and not yet committed (GPUI committed on blur / Enter).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    /// Period `k`'s `i`th time.
    Slot(usize, usize),
    /// The Hours window's start (0) or end (1).
    Window(usize),
}

/// One object's Repeat row and editor (GPUI's `RepeatView` entity).
struct RepeatState {
    open: bool,
    draft: RepeatDraft,
    assignee: String,
    busy: bool,
    error: String,
    agent_select: SelectState,
    freq_select: SelectState,
    calendar: Option<CalendarPickerState>,
    interval: InputState,
    /// The time inputs of each period: [0] the first (the plain rule's
    /// times), then Day 2 … Day N when the interval is 2 or more.
    slot_inputs: Vec<Vec<InputState>>,
    window_from: InputState,
    window_until: InputState,
    dirty: Vec<Edge>,
    /// The window's width, measured while the editor is open (0 = unknown).
    viewport_w: f32,
}

impl Default for RepeatState {
    fn default() -> Self {
        Self {
            open: false,
            draft: seed(),
            assignee: String::new(),
            busy: false,
            error: String::new(),
            agent_select: SelectState::default(),
            freq_select: SelectState::default(),
            calendar: None,
            interval: InputState::new(),
            slot_inputs: Vec::new(),
            window_from: InputState::new(),
            window_until: InputState::new(),
            dirty: Vec::new(),
            viewport_w: 0.0,
        }
    }
}

/// Every object's Repeat state, by object id.
#[derive(Default)]
struct Repeats(HashMap<String, RepeatState>);

fn st<'a>(s: &'a mut State, id: &str) -> &'a mut RepeatState {
    s.slot::<Repeats>().0.entry(id.to_string()).or_default()
}

impl RepeatState {
    fn sync_inputs(&mut self) {
        self.interval.set_value(self.draft.interval.to_string());
        self.slot_inputs = std::iter::once(&self.draft.times)
            .chain(&self.draft.more)
            .map(|times| {
                times
                    .iter()
                    .map(|t| InputState::with_value(hhmm(*t)))
                    .collect()
            })
            .collect();
        self.dirty.retain(|e| matches!(e, Edge::Window(_)));
        if let Some([a, b]) = self.draft.window {
            self.window_from.set_value(hhmm(a));
            self.window_until.set_value(hhmm(b));
        }
    }

    fn set_freq(&mut self, freq: RepeatFreq) {
        if freq.is_sub_daily() != self.draft.freq.is_sub_daily() {
            self.draft.weekdays = if freq.is_sub_daily() {
                vec![]
            } else {
                vec![weekday(self.draft.anchor)]
            };
        }
        self.draft.freq = freq;
        self.draft.interval = self
            .draft
            .interval
            .min(max_interval(freq, self.draft.after));
        self.interval.set_value(self.draft.interval.to_string());
        self.fit_slots();
    }

    /// On a schedule, or a quiet timer after the last message: the wait keeps
    /// a minute/hour/day unit (a day from a longer calendar unit).
    fn set_after(&mut self, after: bool) {
        if after == self.draft.after {
            return;
        }
        self.draft.after = after;
        if after {
            if !AFTER_FREQS.contains(&self.draft.freq) {
                self.draft.freq = RepeatFreq::Day;
            }
            self.draft.weekdays.clear();
            self.set_freq(self.draft.freq);
        } else {
            let freq = self.draft.freq;
            self.draft.weekdays = if freq.is_sub_daily() {
                vec![]
            } else {
                vec![weekday(self.draft.anchor)]
            };
            if freq == RepeatFreq::Day && self.draft.times.is_empty() {
                self.draft.times = vec![9 * 60];
            }
            self.set_freq(freq);
        }
        self.sync_inputs();
    }

    /// One time slot per period of the cycle (Day 1 … Day N); a resize keeps
    /// what was set and starts new periods off.
    fn fit_slots(&mut self) {
        let before = self.draft.more.len();
        fit_more(&mut self.draft);
        if self.draft.more.len() != before {
            self.sync_inputs();
        }
    }

    /// Period `k`'s times: 0 the first (the plain rule's), then Day 2 ….
    fn slot_mut(&mut self, k: usize) -> Option<&mut Vec<u32>> {
        if k == 0 {
            Some(&mut self.draft.times)
        } else {
            self.draft.more.get_mut(k - 1)
        }
    }

    fn set_anchor(&mut self, next: i64) {
        let old = weekday(self.draft.anchor);
        if self.draft.freq == RepeatFreq::Week && self.draft.weekdays == [old] {
            self.draft.weekdays = vec![weekday(next)];
        }
        self.draft.anchor = next;
    }

    fn toggle_weekday(&mut self, d: u8) {
        if self.draft.weekdays.contains(&d) {
            self.draft.weekdays.retain(|x| *x != d);
        } else {
            self.draft.weekdays.push(d);
        }
        if self.draft.weekdays.is_empty() && self.draft.freq == RepeatFreq::Week {
            self.draft.weekdays = vec![d];
        }
    }

    fn add_time(&mut self, k: usize) {
        // An empty period starts at the first period's time (9:00 by default).
        let first = self.draft.times.first().copied().unwrap_or(9 * 60);
        let Some(slot) = self.slot_mut(k) else {
            return;
        };
        let mut t = slot.last().map_or(first, |last| (last + 60) % DAY_MINUTES);
        for _ in 0..24 {
            if !slot.contains(&t) {
                break;
            }
            t = (t + 60) % DAY_MINUTES;
        }
        slot.push(t);
        self.sync_inputs();
    }

    fn remove_time(&mut self, k: usize, i: usize) {
        if let Some(slot) = self.slot_mut(k)
            && i < slot.len()
        {
            slot.remove(i);
        }
        self.sync_inputs();
    }

    fn invalid(&self) -> &'static str {
        let d = &self.draft;
        match d.window {
            Some([a, b]) if (d.freq.is_sub_daily() || d.after) && a > b => {
                "The window ends before it starts."
            }
            _ if d.after => "",
            _ if !d.freq.is_sub_daily() && d.times.is_empty() && !slotted(d) => {
                "Give at least one period a time."
            }
            _ => "",
        }
    }

    fn input_mut(&mut self, edge: Edge) -> &mut InputState {
        match edge {
            Edge::Window(0) => &mut self.window_from,
            Edge::Window(_) => &mut self.window_until,
            Edge::Slot(k, i) => {
                if self.slot_inputs.len() <= k {
                    self.slot_inputs.resize_with(k + 1, Vec::new);
                }
                let inputs = &mut self.slot_inputs[k];
                if inputs.len() <= i {
                    inputs.resize_with(i + 1, InputState::new);
                }
                &mut inputs[i]
            }
        }
    }

    /// A time field's text into the draft (GPUI's Blur / PressEnter).
    fn commit(&mut self, edge: Edge) {
        self.dirty.retain(|e| *e != edge);
        let Some(t) = minutes_of(self.input_mut(edge).value()) else {
            return;
        };
        match edge {
            Edge::Window(e) => {
                if let Some(w) = &mut self.draft.window {
                    w[e] = t;
                }
            }
            Edge::Slot(k, i) => {
                if let Some(slot) = self.slot_mut(k)
                    && i < slot.len()
                {
                    slot[i] = t;
                }
            }
        }
    }
}

/// Commits the time fields typed into that lost focus (GPUI's `InputEvent::Blur`).
fn commit_blurred(s: &mut State) -> bool {
    let Some(repeats) = s.get::<Repeats>() else {
        return false;
    };
    if repeats.0.values().all(|r| r.dirty.is_empty()) {
        return false;
    }
    let mut any = false;
    for r in s.slot::<Repeats>().0.values_mut() {
        for edge in r.dirty.clone() {
            if !r.input_mut(edge).is_focused() {
                r.commit(edge);
                any = true;
            }
        }
    }
    any
}

/// Installs the blur commits of the time fields.
pub(crate) fn register(s: &mut State) {
    s.on_poll(commit_blurred);
}

/// Drops `object_id`'s Repeat state (GPUI dropped the view).
pub(crate) fn forget(s: &mut State, object_id: &str) {
    s.slot::<Repeats>().0.remove(object_id);
}

/// The editor is open (a rule being picked): the pane then shows Repeat instructions.
pub(crate) fn repeat_is_open(s: &State, object_id: &str) -> bool {
    s.get::<Repeats>()
        .and_then(|r| r.0.get(object_id))
        .is_some_and(|r| r.open)
}

fn open_editor(s: &mut State, id: &str) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    let rule = repeat_of(&object.fields);
    let guests = guest_agents(&object.fields);
    let current = object.field_str("assignee").to_string();
    let r = st(s, id);
    r.draft = rule.as_ref().map(to_draft).unwrap_or_else(seed);
    r.assignee = if guests.contains(&current) {
        current
    } else {
        guests.first().cloned().unwrap_or_default()
    };
    r.calendar = None;
    r.sync_inputs();
    r.open = true;
}

fn run(s: &mut State, id: &str, op: Task<anyhow::Result<()>>) {
    let r = st(s, id);
    r.busy = true;
    r.error.clear();
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        let result = op.await;
        cx.update(|s| {
            if let Some(r) = s.slot::<Repeats>().0.get_mut(&id) {
                if let Err(e) = result {
                    r.error = e.to_string();
                }
                r.busy = false;
            }
        });
    })
    .detach();
}

/// Who runs each occurrence: the chosen guest, or clear the field.
fn save_assignee(
    s: &mut State,
    object: &Object,
    assignee: &str,
) -> Option<Task<anyhow::Result<roostr_core::JsonMap>>> {
    if assignee == object.field_str("assignee") {
        return None;
    }
    Some(if assignee.is_empty() {
        api::note::delete_field(&object.id, "assignee", s)
    } else {
        api::note::set_field(&object.id, "assignee", Value::string(assignee), s)
    })
}

/// Runs the assignee change (if any), then the rule.
fn run_set(
    s: &mut State,
    id: &str,
    assignee: Option<Task<anyhow::Result<roostr_core::JsonMap>>>,
    rule: &RepeatRule,
) {
    let set = api::repeat::set(id, rule, s);
    let op = s.rt.spawn(async move |_| {
        if let Some(a) = assignee {
            a.await?;
        }
        set.await?;
        Ok(())
    });
    run(s, id, op);
}

fn save(s: &mut State, id: &str) {
    if !st(s, id).invalid().is_empty() {
        return;
    }
    st(s, id).open = false;
    let Some(object) = s.store.object(id) else {
        return;
    };
    let view = repeat_of(&object.fields).as_ref().map(to_draft);
    let (d, who) = {
        let r = st(s, id);
        (r.draft.clone(), r.assignee.clone())
    };
    let assignee = save_assignee(s, &object, &who);
    // Re-saving an identical rule could resurrect a completed occurrence.
    if view.as_ref().is_some_and(|v| same_rule(v, &d)) {
        if let Some(task) = assignee {
            let op = s.rt.spawn(async move |_| task.await.map(|_| ()));
            run(s, id, op);
        }
        return;
    }
    let tz = iana_time_zone::get_timezone().unwrap_or_default();
    if d.after {
        let rule = RepeatRule {
            freq: d.freq,
            interval: d.interval,
            weekdays: vec![],
            monthly: d.monthly.clone(),
            times: vec![],
            slots: vec![],
            window: d.window,
            tz,
            anchor_ms: None,
            after: true,
        };
        run_set(s, id, assignee, &rule);
        return;
    }
    let sub = d.freq.is_sub_daily();
    let rule = RepeatRule {
        freq: d.freq,
        interval: d.interval,
        weekdays: if d.freq == RepeatFreq::Week || sub {
            d.weekdays.clone()
        } else {
            vec![]
        },
        monthly: d.monthly.clone(),
        // With slots, the times are their union (the engine checks it).
        times: if sub {
            vec![]
        } else {
            sorted_times(&[d.times.clone(), d.more.concat()].concat())
        },
        slots: if sub { vec![] } else { draft_slots(&d) },
        window: if sub { d.window } else { None },
        tz,
        // Noon, so the engine lands on the same wall-clock day across DST.
        anchor_ms: Some(d.anchor + DAY / 2),
        after: false,
    };
    run_set(s, id, assignee, &rule);
}

fn clear(s: &mut State, id: &str) {
    st(s, id).open = false;
    let task = api::repeat::clear(id, s);
    let op = s.rt.spawn(async move |_| task.await.map(|_| ()));
    run(s, id, op);
}

fn run_now(s: &mut State, id: &str) {
    st(s, id).open = false;
    let v = Value::map(Fields::from([(
        "at".to_string(),
        Value::int(chrono::Utc::now().timestamp_millis()),
    )]));
    let task = api::note::set_field(id, "run_now", v, s);
    let op = s.rt.spawn(async move |_| task.await.map(|_| ()));
    run(s, id, op);
}

fn skip_run(s: &mut State, id: &str) {
    st(s, id).open = false;
    let task = api::note::set_field(id, "done", Value::bool(true), s);
    let op = s.rt.spawn(async move |_| task.await.map(|_| ()));
    run(s, id, op);
}

fn open_calendar(s: &mut State, id: &str) {
    let r = st(s, id);
    r.calendar = match r.calendar {
        Some(_) => None,
        None => Some(CalendarPickerState::new(r.draft.anchor)),
    };
}

/// A form row's caption: 12px muted, a 52px column.
fn lbl(t: &Theme, caption: impl Into<String>) -> El {
    note(t, caption, 12.0).w(52.0)
}

fn fieldrow() -> El {
    row().flex_wrap().center_y().gap(6.0)
}

/// A footer button: kit outline [`Button`] (kit hover fill), small, 12px
/// label in `fg`; `hover_fg` recolours the label while the button is hovered.
fn small_btn(t: &Theme, key: &str, label: &str, fg: Color, hover_fg: Option<Color>) -> Button {
    let mut line = text(label.to_string()).font(ty::ui(12.0)).color(fg);
    if let Some(c) = hover_fg {
        line = line.group_color(c);
    }
    Button::new(key)
        .outline()
        .small()
        .accessibility_label(label)
        .h_auto()
        .px(9.0)
        .py(3.0)
        .radius(radius::SM)
        .bg(Color::TRANSPARENT)
        .border_color(t.border)
        .text_color(t.fg)
        .child(line)
}

/// The Starts date trigger: kit outline [`Button`] dressed as the selects
/// (white 5% fill, radius 6, 3px 6px, 12.5px) with a chevron.
fn date_button(t: &Theme, key: &str, label: String) -> Button {
    Button::new(key)
        .outline()
        .small()
        .accessibility_label(label.clone())
        .h_auto()
        .px(6.0)
        .py(3.0)
        .radius(radius::SM)
        .bg(Color::WHITE.alpha(0.05))
        .border_color(t.border)
        .text_color(t.fg)
        .child(
            row()
                .center_y()
                .gap(6.0)
                .child(text(label).font(ty::ui(12.5)).color(t.fg))
                .child(roostr_ui::glyph(Glyph::ChevronDown, 10.0).color(t.muted)),
        )
}

type SelectLens = Rc<dyn for<'a> Fn(&'a mut State) -> &'a mut SelectState>;

/// The Agent / Every select: the kit `Select` dressed as `select_box` (white
/// 5% fill, 1px border, radius 6, 3px 6px, 12.5px), its list `menu_w` wide.
/// Its items and value follow the draft (`choices`, `value`).
#[allow(clippy::too_many_arguments)]
fn choice_select(
    t: &Theme,
    layer: &mut Vec<El>,
    key: &str,
    state: &SelectState,
    choices: Vec<SelectItem>,
    value: &str,
    access: impl for<'a> Fn(&'a mut State) -> &'a mut SelectState + 'static,
    on_pick: impl Fn(&mut State, &str) + 'static,
    menu_w: f32,
) -> El {
    // Re-seeded only when they change, so an open list is left alone.
    let sync = move |st: &mut SelectState, choices: &[SelectItem], value: &str| {
        if st.items() != choices {
            st.set_items(choices.to_vec());
        }
        if st.selected_value() != Some(value) {
            st.set_selected_value(value);
        }
    };
    let mut shown = state.clone();
    sync(&mut shown, &choices, value);
    let value = value.to_string();
    let access: SelectLens = Rc::new(move |s: &mut State| {
        let st = access(s);
        sync(st, &choices, &value);
        st
    });
    el().child(
        Select::new(key, &shown)
            .on_state(move |s: &mut State| access(s))
            .on_change(move |s: &mut State, v: Option<&str>| {
                if let Some(v) = v {
                    on_pick(s, v);
                }
            })
            .focus_ring(false)
            // Like the old box: nothing chosen reads as blank, not "Select…".
            .placeholder("")
            .menu_width(menu_w)
            .fill(Color::WHITE.alpha(0.05))
            .border_color(t.border)
            .radius(radius::SM)
            .px(6.0)
            .py(3.0)
            .gap(6.0)
            .text_size(12.5)
            .text_color(t.fg)
            .build(t, layer),
    )
}

/// A joined single-choice control (kit segmented outline [`ToggleGroup`]).
fn seg_group(key: &str) -> ToggleGroup {
    ToggleGroup::new(key).segmented().outline()
}

/// One segment of [`seg_group`]: 12px, muted when off, blue tint when on.
fn seg(t: &Theme, key: &str, label: impl Into<String>, on: bool) -> Toggle {
    Toggle::new(key)
        .checked(on)
        .label(label)
        .h_auto()
        .min_w(0.0)
        .px(9.0)
        .py(3.0)
        .text_size(12.0)
        .pointer()
        .border_color(t.border)
        .text_color(if on { t.fg } else { t.muted })
        .bg(if on {
            roostr_ui::color::rgba(0x7896ff2e)
        } else {
            Color::TRANSPARENT
        })
}

/// The toggle a [`ToggleGroup`] press flipped: where `next` differs from
/// the states it was drawn with.
fn clicked(before: &[bool], next: &[bool]) -> Option<usize> {
    before.iter().zip(next).position(|(a, b)| a != b)
}

/// A time field of the editor (compact skin), committing on Enter and blur.
fn time_field(t: &Theme, id: &str, key: String, input: &InputState, edge: Edge, w: f32) -> El {
    let (a, b, c) = (id.to_string(), id.to_string(), id.to_string());
    field(
        t,
        Input::new(key, input)
            .placeholder(if matches!(edge, Edge::Window(_)) {
                "HH:MM"
            } else {
                ""
            })
            .on_edit(move |s: &mut State| st(s, &a).input_mut(edge))
            .on_change(move |s: &mut State, _| {
                let r = st(s, &b);
                if !r.dirty.contains(&edge) {
                    r.dirty.push(edge);
                }
            })
            .on_submit(move |s: &mut State, _| st(s, &c).commit(edge)),
        input.is_focused(),
        FieldSkin::Compact,
    )
    .w(w)
}

/// The Repeat row of `object_id`'s properties (root keyed `prop-row-__repeat__`)
/// and, while it's being set, its editor popover in `layer`.
pub(crate) fn repeat_view(
    s: &State,
    _cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
) -> El {
    let Some(object) = s.store.object(object_id) else {
        return el().key("prop-row-__repeat__");
    };
    let fallback;
    let me = match s.get::<Repeats>().and_then(|r| r.0.get(object_id)) {
        Some(me) => me,
        None => {
            fallback = RepeatState::default();
            &fallback
        }
    };
    let id = object_id.to_string();
    let rule = repeat_of(&object.fields);
    let view = rule.as_ref().map(to_draft);
    let requested = object.fields.contains_key("run_now");
    let active_run = object
        .field("run_active")
        .and_then(|v| v.entries())
        .is_some();
    let open_run = rule
        .as_ref()
        .is_some_and(|r| r.fired_for == Some(r.next) && !r.freq.is_sub_daily() && !r.after);
    let guest_ids = guest_agents(&object.fields);
    let guests: Vec<_> = guest_ids
        .iter()
        .filter_map(|g| s.store.agent(g).cloned())
        .collect();
    let has_guests = !guest_ids.is_empty();

    let value_el = match &view {
        Some(v) => text(describe_draft(v))
            .font(ty::ui(13.0).lh(22.0))
            .color(t.fg)
            .truncate(),
        None => placeholder(t, "Off"),
    };
    let open_id = id.clone();
    let trigger = row()
        .key("repeat-row")
        .role("button")
        .label("Repeat")
        .w_full()
        .min_h(30.0)
        .pl(6.0)
        .pr(24.0)
        .py(4.0)
        .radius(radius::SM)
        .hover(t.hover)
        .hover_group()
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| open_editor(s, &open_id))
        .child(
            row()
                .w_full()
                .gap(10.0)
                .child(
                    row()
                        .w_frac(0.4)
                        .max_w(120.0)
                        .center_y()
                        .gap(7.0)
                        .min_h(22.0)
                        .child(roostr_ui::glyph(Glyph::PropRepeat, 14.0).color(t.muted))
                        .child(text("Repeat").font(ty::ui(13.0).lh(22.0)).color(t.muted)),
                )
                .child(
                    row()
                        .grow(1.0)
                        .min_w(0.0)
                        .center_y()
                        .min_h(22.0)
                        .child(value_el),
                ),
        );
    let mut root = col()
        .key("prop-row-__repeat__")
        .gap(2.0)
        .child(tip(trigger, "Repeat"));
    if !me.error.is_empty() {
        root = root.child(error_alert(t, "repeat-error", me.error.clone()));
    }
    if !me.open {
        return root;
    }

    // ── The editor popover ──
    let d = me.draft.clone();
    // A quiet timer has no calendar: only its wait and its hours.
    let after = d.after;
    let sub = d.freq.is_sub_daily() && !after;
    // `.pop`: 360px; `min(360px, calc(100vw - 32px))` at ≤720px.
    let vw = me.viewport_w;
    let width = if vw > 0.0 && roostr_ui::layout::is_mobile(vw) {
        360f32.min(vw - 32.0)
    } else {
        360.0
    };
    let mut pop = col()
        .key("repeat-pop")
        .w(width)
        .gap(10.0)
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(radius::MENU)
        .pt(10.0)
        .px(12.0)
        .pb(12.0)
        .box_shadow(pop_shadow())
        .child(note(t, "REPEAT", 11.0));
    let mut calendar = None;
    // The selects' lists anchor to triggers inside the popover, so they go into the layer
    // after it: an anchored overlay finds only an anchor laid out before it.
    let mut menus = Vec::new();
    if !has_guests {
        pop = pop.child(note(
            t,
            "Add an agent to this object's Agent property first. A repeat needs an agent on the guest list to run each occurrence.",
            12.5,
        ));
    } else {
        // Agent
        let agents: Vec<SelectItem> = guests
            .iter()
            .map(|a| {
                SelectItem::new(
                    a.id.clone(),
                    format!(
                        "{}{}",
                        if a.icon.is_empty() {
                            String::new()
                        } else {
                            format!("{} ", a.icon)
                        },
                        if a.name.is_empty() { "Agent" } else { &a.name }
                    ),
                )
            })
            .collect();
        let (i1, i2) = (id.clone(), id.clone());
        pop = pop.child(fieldrow().child(lbl(t, "Agent")).child(choice_select(
            t,
            &mut menus,
            "agent-select",
            &me.agent_select,
            agents,
            &me.assignee,
            move |s| &mut st(s, &i1).agent_select,
            move |s, v| st(s, &i2).assignee = v.to_string(),
            180.0,
        )));

        // On a schedule | After the last message
        let when = [!after, after];
        let i = id.clone();
        pop = pop.child(
            fieldrow().child(lbl(t, "When")).child(
                seg_group("when")
                    .child(seg(t, "on-schedule", "On a schedule", when[0]))
                    .child(
                        seg(t, "after-message", "After last message", when[1]).tooltip(
                            "Wait after the newest message in this object's chat, anyone's; every message starts the wait again",
                        ),
                    )
                    .on_press(move |s: &mut State, next: Vec<bool>| {
                        match clicked(&when, &next) {
                            Some(0) => st(s, &i).set_after(false),
                            Some(_) => st(s, &i).set_after(true),
                            None => {}
                        }
                    })
                    .build(t),
            ),
        );

        // Every N <freq>, or Wait N <unit> after the last message
        let plural = d.interval != 1;
        let freqs: Vec<SelectItem> = (if after { &AFTER_FREQS[..] } else { &FREQS[..] })
            .iter()
            .map(|&f| SelectItem::new(roostr_core::repeat::freq_name(f), freq_word(f, plural)))
            .collect();
        let (i1, i2) = (id.clone(), id.clone());
        let interval = field(
            t,
            Input::new("repeat-interval", &me.interval)
                .on_edit(move |s: &mut State| &mut st(s, &i1).interval)
                .on_change(move |s: &mut State, _| {
                    let r = st(s, &i2);
                    let raw = r.interval.value().to_string();
                    let n = raw.trim().parse::<f64>().unwrap_or(1.).floor().max(1.) as u32;
                    r.draft.interval = n.min(max_interval(r.draft.freq, r.draft.after));
                    // Every 3 days: a time slot for each of Day 1, 2, 3.
                    r.fit_slots();
                }),
            me.interval.is_focused(),
            FieldSkin::Compact,
        )
        .w(48.0);
        let (i4, i5) = (id.clone(), id.clone());
        pop = pop.child(
            fieldrow()
                .child(lbl(t, if after { "Wait" } else { "Every" }))
                .child(el().key("repeat-every").child(interval))
                .child(choice_select(
                    t,
                    &mut menus,
                    "freq-select",
                    &me.freq_select,
                    freqs,
                    roostr_core::repeat::freq_name(d.freq),
                    move |s| &mut st(s, &i4).freq_select,
                    move |s, name| {
                        if let Some(&f) = FREQS
                            .iter()
                            .find(|&&f| roostr_core::repeat::freq_name(f) == name)
                        {
                            st(s, &i5).set_freq(f);
                        }
                    },
                    140.0,
                )),
        );

        if !sub && !after {
            let date = date_of(d.anchor).format("%m/%d/%Y").to_string();
            let i = id.clone();
            let starts = el().key("repeat-anchor").child(
                date_button(t, "anchor", date)
                    .on_press(move |s: &mut State| open_calendar(s, &i))
                    .build(t),
            );
            if let Some(cal) = &me.calendar {
                let (i1, i2) = (id.clone(), id.clone());
                calendar = Some(calendar_picker(
                    t,
                    "anchor-calendar",
                    cal,
                    lens(move |s: &mut State| {
                        let r = st(s, &i1);
                        let a = r.draft.anchor;
                        r.calendar
                            .get_or_insert_with(|| CalendarPickerState::new(a))
                    }),
                    Rc::new(move |s: &mut State, ev: CalendarEvent| {
                        let r = st(s, &i2);
                        if let CalendarEvent::Pick(Some(ms)) = ev {
                            r.set_anchor(ms);
                        }
                        r.calendar = None;
                    }),
                ));
            }
            pop = pop.child(fieldrow().child(lbl(t, "Starts")).child(starts));
        }

        if sub || after {
            let before = [d.window.is_none(), d.window.is_some()];
            let i = id.clone();
            pop = pop.child(
                fieldrow().child(lbl(t, "Hours")).child(
                    seg_group("hours")
                        .child(seg(t, "all-day", "All day", before[0]))
                        .child(seg(t, "between", "Only between", before[1]))
                        .on_press(move |s: &mut State, next: Vec<bool>| {
                            let r = st(s, &i);
                            match clicked(&before, &next) {
                                Some(0) => r.draft.window = None,
                                Some(_) if r.draft.window.is_none() => {
                                    r.draft.window = Some([8 * 60, 20 * 60]);
                                    r.sync_inputs();
                                }
                                _ => {}
                            }
                        })
                        .build(t),
                ),
            );
            if d.window.is_some() {
                pop = pop.child(
                    fieldrow()
                        .child(lbl(t, ""))
                        .child(time_field(
                            t,
                            &id,
                            "repeat-from".into(),
                            &me.window_from,
                            Edge::Window(0),
                            64.0,
                        ))
                        .child(note(t, "–", 12.0))
                        .child(time_field(
                            t,
                            &id,
                            "repeat-until".into(),
                            &me.window_until,
                            Edge::Window(1),
                            64.0,
                        )),
                );
            }
        }

        if !after && (d.freq == RepeatFreq::Week || sub) {
            let before: Vec<bool> = (0..WD.len())
                .map(|i| d.weekdays.contains(&(i as u8)))
                .collect();
            let i = id.clone();
            let marks = before.clone();
            let days = ToggleGroup::new("days")
                .gap(3.0)
                .children(WD.iter().enumerate().map(|(n, name)| {
                    let on = marks[n];
                    let tog = Toggle::new(format!("day-{n}"))
                        .checked(on)
                        .label(name[..1].to_string())
                        .square(26.0)
                        .min_w(26.0)
                        .p(0.0)
                        .rounded_full()
                        .border_w(1.0)
                        .text_size(11.0)
                        .pointer();
                    if on {
                        tog.bg(t.accent).border_color(t.accent).text_color(t.white)
                    } else {
                        tog.border_color(t.border).text_color(t.muted)
                    }
                }))
                .on_press(move |s: &mut State, next: Vec<bool>| {
                    if let Some(n) = clicked(&before, &next) {
                        st(s, &i).toggle_weekday(n as u8)
                    }
                })
                .build(t);
            let mut line = fieldrow().child(lbl(t, "On")).child(days);
            if d.weekdays.is_empty() {
                line = line.child(note(t, "every day", 12.0));
            }
            pop = pop.child(line);
        }

        if !after && d.freq == RepeatFreq::Month {
            let anchor = date_of(d.anchor);
            let ord = ["", "First", "Second", "Third", "Fourth", "Last"][ordinal_of(d.anchor)];
            let before = [d.monthly == "date", d.monthly == "weekday"];
            let i = id.clone();
            pop = pop.child(
                fieldrow()
                    .child(lbl(t, "On"))
                    .child(
                        seg_group("monthly")
                            .child(
                                seg(t, "by-date", format!("Day {}", anchor.day()), before[0])
                                    .tooltip("The same date every month"),
                            )
                            .child(
                                seg(
                                    t,
                                    "by-weekday",
                                    format!("{ord} {}", anchor.format("%A")),
                                    before[1],
                                )
                                .tooltip("The same weekday of the same week every month"),
                            )
                            .on_press(move |s: &mut State, next: Vec<bool>| {
                                let r = st(s, &i);
                                match clicked(&before, &next) {
                                    Some(0) => r.draft.monthly = "date".into(),
                                    Some(_) => r.draft.monthly = "weekday".into(),
                                    None => {}
                                }
                            })
                            .build(t),
                    )
                    .child(note(t, "of the month", 12.0)),
            );
        }

        if !sub && !after {
            // One row of times per period of the cycle: "At" for a plain
            // rule, Day 1 … Day N when the interval is 2 or more.
            let cycle = me.slot_inputs.len() > 1;
            for (k, inputs) in me.slot_inputs.iter().enumerate() {
                let mut list = row().grow(1.0).min_w(0.0).flex_wrap().center_y().gap(6.0);
                // A plain rule keeps one time; a period may be off.
                let removable = cycle || inputs.len() > 1;
                if inputs.is_empty() {
                    list = list.child(note(t, "Off", 12.0));
                }
                for (n, input) in inputs.iter().enumerate() {
                    let mut item = row().center_y().gap(2.0).child(time_field(
                        t,
                        &id,
                        format!("repeat-time-{k}-{n}"),
                        input,
                        Edge::Slot(k, n),
                        64.0,
                    ));
                    if removable {
                        let i = id.clone();
                        item = item.child(tip(
                            Button::new(format!("time-rm-{}", k * 100 + n))
                                .custom(ButtonCustomVariant::new(t).foreground(t.muted))
                                .accessibility_label("Remove time")
                                .h_auto()
                                .px(3.0)
                                .on_press(move |s: &mut State| st(s, &i).remove_time(k, n))
                                .child(
                                    text("×")
                                        .font(ty::ui(14.0))
                                        .color(t.muted)
                                        .group_color(t.red),
                                )
                                .build(t)
                                .hover_group(),
                            "Remove time",
                        ));
                    }
                    list = list.child(item);
                }
                let i = id.clone();
                list = list.child(
                    el().key(format!("repeat-add-time-{k}")).child(
                        Button::new(format!("add-time-{k}"))
                            .custom(ButtonCustomVariant::new(t).foreground(t.muted))
                            .accessibility_label("Add time")
                            .h_auto()
                            .px(4.0)
                            .py(3.0)
                            .on_press(move |s: &mut State| st(s, &i).add_time(k))
                            .child(
                                text("+ Add time")
                                    .font(ty::ui(12.0))
                                    .color(t.muted)
                                    .group_color(t.fg),
                            )
                            .build(t)
                            .hover_group(),
                    ),
                );
                let label = if cycle {
                    format!("{} {}", period_name(d.freq), k + 1)
                } else {
                    "At".to_string()
                };
                pop = pop.child(
                    row()
                        .key(format!("repeat-slot-{k}"))
                        .items_start()
                        .gap(6.0)
                        .child(lbl(t, label).pt(5.0))
                        .child(list),
                );
            }
        }

        let invalid = me.invalid();
        let next = preview(&d, last_message_ms(&object));
        let mut nexts = row().flex_wrap().gaps(4.0, 10.0);
        for (i, p) in next.iter().enumerate() {
            let line = note(t, fmt_long(*p), 11.5).key(format!("repeat-next-{i}"));
            nexts = nexts.child(if i == 0 { line.color(t.fg) } else { line });
        }
        nexts = nexts.child(note(
            t,
            if after {
                "Each new message in the chat starts the wait again."
            } else {
                "…"
            },
            11.5,
        ));
        pop = pop.child(
            col()
                .gap(4.0)
                .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
                .pt(8.0)
                .child(
                    text(if invalid.is_empty() {
                        describe_draft(&d)
                    } else {
                        invalid.to_string()
                    })
                    .font(ty::ui(12.5))
                    .color(if invalid.is_empty() { t.fg } else { t.red })
                    .wrap()
                    .key("repeat-summary"),
                )
                .child(nexts),
        );
    }
    // Footer
    let busy = me.busy;
    // Turn off / Skip: bordered like the other footer buttons, red on hover.
    let rm = |key: &str, label: &str| {
        roostr_ui::disabled(
            small_btn(t, key, label, t.muted, (!busy).then_some(t.red)),
            busy,
        )
    };
    let mut foot = row().center_y().gap(6.0);
    if rule.is_some() {
        let i = id.clone();
        foot = foot.child(
            rm("clear", "Turn off repeating")
                .on_press(move |s: &mut State| {
                    if !busy {
                        clear(s, &i)
                    }
                })
                .build(t)
                .hover_group(),
        );
    }
    if rule.is_some() && open_run && !active_run {
        let i = id.clone();
        foot = foot.child(
            rm("skip", "Skip this run")
                .tooltip("Finish the open run without doing it; the next run follows on schedule")
                .on_press(move |s: &mut State| {
                    if !busy {
                        skip_run(s, &i)
                    }
                })
                .build(t)
                .hover_group(),
        );
    }
    if rule.is_some() {
        let disabled = busy || requested || active_run;
        let title = if active_run {
            "A run is in progress"
        } else if open_run {
            "Run the open occurrence again from the start; when it finishes, the schedule moves on"
        } else {
            "Start an extra run now, exactly like a scheduled one; the next occurrence stays where it is"
        };
        let label = if active_run {
            "Running…"
        } else if requested {
            "Starting…"
        } else if open_run {
            "Retry run"
        } else {
            "Run now"
        };
        let i = id.clone();
        foot = foot.child(
            roostr_ui::disabled(
                small_btn(t, "run-now", label, t.fg, None).tooltip(title),
                disabled,
            )
            .on_press(move |s: &mut State| {
                if !disabled {
                    run_now(s, &i)
                }
            })
            .build(t),
        );
    }
    let save_disabled = busy || !has_guests || !me.invalid().is_empty();
    let (i1, i2) = (id.clone(), id.clone());
    foot = foot
        .child(el().grow(1.0))
        .child(
            small_btn(t, "cancel", "Cancel", t.fg, None)
                .on_press(move |s: &mut State| st(s, &i1).open = false)
                .build(t),
        )
        .child(
            el().key("repeat-save").child(
                roostr_ui::disabled(
                    small_btn(
                        t,
                        "save",
                        if rule.is_some() { "Update" } else { "Repeat" },
                        t.fg,
                        None,
                    ),
                    save_disabled,
                )
                .on_press(move |s: &mut State| {
                    if !save_disabled {
                        save(s, &i2)
                    }
                })
                .build(t),
            ),
        );
    pop = pop.child(foot);

    // Measures the window for the popover's phone width (the layer spans it).
    let i = id.clone();
    layer.push(
        el().key("repeat-viewport")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .inert()
            .on_layout(move |s: &mut State, r: [f32; 4]| {
                let me = st(s, &i);
                if (me.viewport_w - r[2]).abs() > 0.01 {
                    me.viewport_w = r[2];
                }
            }),
    );
    let i = id.clone();
    backdrop(layer, "repeat-backdrop", move |s: &mut State| {
        st(s, &i).open = false
    });
    below(layer, "prop-row-__repeat__", 6.0, pop);
    if let Some(cal) = calendar {
        below(layer, "repeat-anchor", 4.0, cal);
    }
    layer.extend(menus);
    root
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{App, Vault, object};
    use roostr_ui::ThemePref;
    use serde_json::json;

    fn view(s: &State, cx: &mut Cx<State>) -> El {
        let t = roostr_ui::theme(ThemePref::Dark, cx);
        let mut layer = vec![];
        let body = col()
            .w(420.0)
            .px(20.0)
            .py(20.0)
            .child(repeat_view(s, cx, t, &mut layer, "q1"));
        roostr_ui::root(body, layer)
    }

    /// q1 (a query) with `rule` and, when `agent`, agent-1 on its guest list.
    fn app(rule: Option<serde_json::Value>, agent: bool) -> App {
        let mut pairs = vec![("name", Value::string("Tasks"))];
        if let Some(rule) = rule {
            pairs.push(("repeat", serde_json::from_value(rule).expect("rule")));
        }
        if agent {
            pairs.push((
                "agent",
                Value::values(vec![Value::link("agent-1", "agent")]),
            ));
        }
        let vault = Vault::new(
            vec![
                object("q1", "query", &pairs),
                object("agent-1", "agent", &[("name", Value::string("Ada"))]),
            ],
            vec![],
        );
        let mut app = App::new(vault, view);
        app.s().store.ensure_object("q1");
        app.until("q1", |s| s.store.object("q1").is_some());
        app
    }

    fn daily() -> serde_json::Value {
        json!({"mapValue": {"entries": {
            "freq": {"stringValue": "day"}, "interval": {"intValue": 1},
            "anchor": {"intValue": 0}, "next": {"intValue": 4_102_444_800_000_i64},
            "times": {"valuesValue": {"items": [{"intValue": 540}]}}
        }}})
    }

    fn saved_rule(app: &mut App) -> serde_json::Value {
        app.step();
        let vault = app.vault.clone();
        app.until("the rule saved", move |_| {
            !vault.calls("repeat_set").is_empty()
        });
        let calls = app.vault.calls("repeat_set");
        calls.last().expect("saved")["rule"].clone()
    }

    /// filter_by_object's Repeat part: every 2 days gets a time slot per
    /// period; a new slot starts at the first period's time.
    #[test]
    fn interval_two_gets_two_slots_and_saves_them() {
        let mut app = app(Some(daily()), true);
        assert!(!app.shown("repeat-every"));
        app.click("prop-row-__repeat__");
        assert!(repeat_is_open(app.s(), "q1"));
        assert!(app.shown("repeat-every"));
        assert!(app.shown("repeat-slot-0"));
        assert!(!app.shown("repeat-slot-1"), "one slot at an interval of 1");
        app.click("repeat-every");
        app.key(rune::Key::Backspace);
        app.type_text("2");
        assert!(app.shown("repeat-slot-0") && app.shown("repeat-slot-1"));
        assert!(!app.shown("repeat-slot-2"));
        app.click("repeat-add-time-1");
        app.click("repeat-save");
        assert!(!repeat_is_open(app.s(), "q1"));
        let rule = saved_rule(&mut app);
        assert_eq!(rule["interval"], json!(2));
        assert_eq!(rule["slots"], json!([[540], [540]]), "{rule}");
    }

    /// Without an agent the editor explains and can't save.
    #[test]
    fn needs_an_agent() {
        let mut app = app(Some(daily()), false);
        app.click("prop-row-__repeat__");
        assert!(!app.shown("repeat-every"));
        app.click("repeat-save");
        app.step();
        assert!(app.vault.calls("repeat_set").is_empty());
    }

    /// After last message: a quiet timer with an hours window, one next run.
    #[test]
    fn after_last_message_saves_a_quiet_timer() {
        let mut app = app(None, true);
        app.click("prop-row-__repeat__");
        app.click("after-message");
        assert!(app.shown("repeat-next-0"));
        assert!(
            !app.shown("repeat-next-1"),
            "a quiet timer shows its one next run"
        );
        assert!(!app.shown("repeat-anchor"), "no Starts for a wait");
        app.click("between");
        assert!(app.shown("repeat-from") && app.shown("repeat-until"));
        app.click("repeat-save");
        let rule = saved_rule(&mut app);
        assert_eq!(rule["after"], json!(true), "{rule}");
        assert_eq!(rule["freq"], json!("day"), "{rule}");
        assert_eq!(rule["interval"], json!(1), "{rule}");
        assert_eq!(rule["window"], json!([480, 1200]), "{rule}");
        assert!(rule.get("slots").is_none(), "{rule}");
    }

    /// A new rule (weekly, today, 9:00) previews its next five runs.
    #[test]
    fn previews_the_next_five_runs() {
        let mut app = app(None, true);
        app.click("prop-row-__repeat__");
        for i in 0..5 {
            assert!(app.shown(&format!("repeat-next-{i}")), "run {i}");
        }
        assert!(!app.shown("repeat-next-5"));
        let d = st(app.s(), "q1").draft.clone();
        let first = fmt_long(preview(&d, 0)[0]);
        let texts: Vec<String> = app
            .rt
            .elements()
            .into_iter()
            .filter(|e| e.key.as_deref() == Some("repeat-next-0"))
            .filter_map(|e| e.text)
            .collect();
        assert_eq!(texts, vec![first]);
    }

    #[test]
    fn minutes_parse_and_format() {
        assert_eq!(minutes_of("09:30"), Some(570));
        assert_eq!(minutes_of(" 9 : 5 "), Some(545));
        assert_eq!(minutes_of("24:00"), None);
        assert_eq!(minutes_of("9"), None);
        assert_eq!(hhmm(570), "09:30");
    }

    #[test]
    fn clicked_finds_the_flipped_toggle() {
        assert_eq!(clicked(&[true, false], &[false, true]), Some(0));
        assert_eq!(clicked(&[true, false], &[true, true]), Some(1));
        assert_eq!(clicked(&[true], &[true]), None);
    }

    #[test]
    fn add_time_fills_a_new_period_with_the_first_time() {
        let mut r = RepeatState::default();
        r.draft.freq = RepeatFreq::Day;
        r.draft.interval = 2;
        r.fit_slots();
        assert_eq!(r.slot_inputs.len(), 2);
        r.add_time(1);
        assert_eq!(draft_slots(&r.draft), vec![vec![540], vec![540]]);
        r.add_time(1);
        assert_eq!(r.draft.more[0], vec![540, 600]);
    }

    #[test]
    fn after_keeps_a_wait_unit() {
        let mut r = RepeatState::default();
        r.set_after(true);
        assert_eq!(r.draft.freq, RepeatFreq::Day);
        assert!(r.draft.weekdays.is_empty());
        assert_eq!(r.invalid(), "");
        r.draft.window = Some([600, 500]);
        assert_eq!(r.invalid(), "The window ends before it starts.");
    }

    #[test]
    fn daily_preview_has_five_future_runs() {
        let mut d = seed();
        d.freq = RepeatFreq::Day;
        d.weekdays.clear();
        let runs = preview(&d, 0);
        assert_eq!(runs.len(), 5);
        assert!(runs.windows(2).all(|w| w[1] - w[0] >= DAY - 3_600_000));
    }
}

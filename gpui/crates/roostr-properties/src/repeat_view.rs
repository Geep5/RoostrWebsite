//! Recurring objects (`Repeat.svelte`): the engine owns the rule
//! (`fields.repeat`) and the occurrence math; this renders when it next
//! runs and lets a person edit the cadence, run it now, skip the open run or
//! turn repeating off.

use chrono::{Datelike as _, Local, NaiveDate, TimeZone as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::repeat::{
    DAY, DAY_MINUTES, RepeatDraft, WD, describe_draft, ordinal_of, sod, sorted_times, to_draft,
};
use roostr_core::serving::{
    MachineRow, fetch_machines, machine_name, resolve_serving, serving_copy,
};
use roostr_core::types::{guest_agents, repeat_of};
use roostr_core::{Fields, Glyph, Object, RepeatFreq, RepeatRule, Store, Value, api, theme};

use crate::calendar::{CalendarEvent, CalendarPicker, local_date, local_midnight_ms};
use crate::widgets::{FieldSkin, backdrop, below, field, placeholder, pop_shadow, tip};
use gpui_kit::component::Icon;

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

fn max_interval(freq: RepeatFreq) -> u32 {
    if freq == RepeatFreq::Hour { 23 } else { 999 }
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

/// The occurrence times on a day the rule runs.
fn day_times(d: &RepeatDraft) -> Vec<u32> {
    if !d.freq.is_sub_daily() {
        return sorted_times(&d.times);
    }
    let [from, until] = d.window.unwrap_or([0, DAY_MINUTES - 1]);
    let period = d.interval * if d.freq == RepeatFreq::Hour { 60 } else { 1 };
    (from..=until).step_by(period.max(1) as usize).collect()
}

/// The next five occurrences from now.
fn preview(d: &RepeatDraft) -> Vec<i64> {
    let times = day_times(d);
    let now = chrono::Utc::now().timestamp_millis();
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
        for t in &times {
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
        && a.window == b.window
        && aw == bw
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Agent,
    Freq,
}

pub(crate) struct RepeatView {
    object_id: String,
    open: bool,
    draft: RepeatDraft,
    assignee: String,
    busy: bool,
    error: String,
    serving_name: String,
    serving_warning: String,
    machines_known: Vec<MachineRow>,
    /// The object state serving was last resolved for.
    resolved_for: Option<(String, i64)>,
    menu: Option<Menu>,
    calendar: Option<Entity<CalendarPicker>>,
    interval: Entity<InputState>,
    times: Vec<Entity<InputState>>,
    window_from: Entity<InputState>,
    window_until: Entity<InputState>,
    _subs: Vec<Subscription>,
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
        window: None,
    }
}

impl RepeatView {
    pub(crate) fn new(object_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let interval = cx.new(|cx| InputState::new(window, cx));
        let window_from = cx.new(|cx| InputState::new(window, cx).placeholder("HH:MM"));
        let window_until = cx.new(|cx| InputState::new(window, cx).placeholder("HH:MM"));
        let roster = fetch_machines(cx);
        cx.spawn(async move |this, cx| {
            if let Ok((_, machines)) = roster.await {
                let _ = this.update(cx, |this, cx| {
                    this.machines_known = machines;
                    cx.notify();
                });
            }
        })
        .detach();
        let subs = vec![
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
            cx.subscribe(&interval, |this, input, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    let raw = input.read(cx).value().to_string();
                    let n = raw.trim().parse::<f64>().unwrap_or(1.).floor().max(1.) as u32;
                    this.draft.interval = n.min(max_interval(this.draft.freq));
                    cx.notify();
                }
            }),
            cx.subscribe(&window_from, |this, input, ev: &InputEvent, cx| {
                this.window_edge(0, input, ev, cx)
            }),
            cx.subscribe(&window_until, |this, input, ev: &InputEvent, cx| {
                this.window_edge(1, input, ev, cx)
            }),
        ];
        Self {
            object_id,
            open: false,
            draft: seed(),
            assignee: String::new(),
            busy: false,
            error: String::new(),
            serving_name: String::new(),
            serving_warning: String::new(),
            machines_known: vec![],
            resolved_for: None,
            menu: None,
            calendar: None,
            interval,
            times: vec![],
            window_from,
            window_until,
            _subs: subs,
        }
    }

    fn window_edge(
        &mut self,
        edge: usize,
        input: Entity<InputState>,
        ev: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) {
            return;
        }
        let Some(t) = minutes_of(&input.read(cx).value()) else {
            return;
        };
        if let Some(w) = &mut self.draft.window {
            w[edge] = t;
        }
        cx.notify();
    }

    fn object(&self, cx: &App) -> Option<std::sync::Arc<Object>> {
        Store::global(cx).read(cx).object(&self.object_id)
    }

    /// Re-resolve which computer runs the object's occurrences when it changes.
    fn resolve(&mut self, object: &Object, cx: &mut Context<Self>) {
        let key = (object.id.clone(), object.updated_at);
        if self.resolved_for.as_ref() == Some(&key) {
            return;
        }
        self.resolved_for = Some(key);
        let agent_owned =
            object.fields.contains_key("assignee") || object.fields.contains_key("agent");
        if !agent_owned {
            self.serving_name.clear();
            self.serving_warning.clear();
            return;
        }
        let task = resolve_serving(&object.id, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((serving, machines)) => {
                        this.serving_name = machine_name(&machines, &serving.machine_id);
                        let copy = serving_copy(&serving, &machines);
                        this.serving_warning = if copy.warning {
                            copy.text
                        } else {
                            String::new()
                        };
                    }
                    Err(_) => {
                        this.serving_name.clear();
                        this.serving_warning.clear();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.draft.interval.to_string();
        self.interval.update(cx, |s, cx| s.set_value(n, window, cx));
        self.times = self
            .draft
            .times
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(hhmm(*t)));
                let sub = cx.subscribe(
                    &input,
                    move |this: &mut Self, input, ev: &InputEvent, cx| {
                        if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. })
                            && let Some(t) = minutes_of(&input.read(cx).value())
                            && i < this.draft.times.len()
                        {
                            this.draft.times[i] = t;
                            cx.notify();
                        }
                    },
                );
                self._subs.push(sub);
                input
            })
            .collect();
        if let Some([a, b]) = self.draft.window {
            self.window_from
                .update(cx, |s, cx| s.set_value(hhmm(a), window, cx));
            self.window_until
                .update(cx, |s, cx| s.set_value(hhmm(b), window, cx));
        }
    }

    fn open_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let rule = repeat_of(&object.fields);
        self.draft = rule.as_ref().map(to_draft).unwrap_or_else(seed);
        let guests = guest_agents(&object.fields);
        let current = object.field_str("assignee").to_string();
        self.assignee = if guests.contains(&current) {
            current
        } else {
            guests.first().cloned().unwrap_or_default()
        };
        self.menu = None;
        self.calendar = None;
        self.sync_inputs(window, cx);
        self.open = true;
        cx.notify();
    }

    fn set_freq(&mut self, freq: RepeatFreq, window: &mut Window, cx: &mut Context<Self>) {
        if freq.is_sub_daily() != self.draft.freq.is_sub_daily() {
            self.draft.weekdays = if freq.is_sub_daily() {
                vec![]
            } else {
                vec![weekday(self.draft.anchor)]
            };
        }
        self.draft.freq = freq;
        self.draft.interval = self.draft.interval.min(max_interval(freq));
        let n = self.draft.interval.to_string();
        self.interval.update(cx, |s, cx| s.set_value(n, window, cx));
        self.menu = None;
        cx.notify();
    }

    fn set_anchor(&mut self, next: i64, cx: &mut Context<Self>) {
        let old = weekday(self.draft.anchor);
        if self.draft.freq == RepeatFreq::Week && self.draft.weekdays == [old] {
            self.draft.weekdays = vec![weekday(next)];
        }
        self.draft.anchor = next;
        cx.notify();
    }

    fn toggle_weekday(&mut self, d: u8, cx: &mut Context<Self>) {
        if self.draft.weekdays.contains(&d) {
            self.draft.weekdays.retain(|x| *x != d);
        } else {
            self.draft.weekdays.push(d);
        }
        if self.draft.weekdays.is_empty() && self.draft.freq == RepeatFreq::Week {
            self.draft.weekdays = vec![d];
        }
        cx.notify();
    }

    fn add_time(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut t = (self.draft.times.last().copied().unwrap_or(9 * 60) + 60) % DAY_MINUTES;
        for _ in 0..24 {
            if !self.draft.times.contains(&t) {
                break;
            }
            t = (t + 60) % DAY_MINUTES;
        }
        self.draft.times.push(t);
        self.sync_inputs(window, cx);
        cx.notify();
    }

    fn remove_time(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.draft.times.remove(i);
        self.sync_inputs(window, cx);
        cx.notify();
    }

    fn invalid(&self) -> &'static str {
        match self.draft.window {
            Some([a, b]) if self.draft.freq.is_sub_daily() && a > b => {
                "The window ends before it starts."
            }
            _ => "",
        }
    }

    fn run(&mut self, op: Task<anyhow::Result<()>>, cx: &mut Context<Self>) {
        self.busy = true;
        self.error.clear();
        cx.spawn(async move |this, cx| {
            let result = op.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.error = e.to_string();
                }
                this.busy = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Who runs each occurrence: the chosen guest, or clear the field.
    fn save_assignee(
        &self,
        object: &Object,
        cx: &mut App,
    ) -> Option<Task<anyhow::Result<roostr_core::JsonMap>>> {
        let current = object.field_str("assignee");
        if self.assignee == current {
            return None;
        }
        Some(if self.assignee.is_empty() {
            api::note::delete_field(&object.id, "assignee", cx)
        } else {
            api::note::set_field(
                &object.id,
                "assignee",
                Value::string(self.assignee.clone()),
                cx,
            )
        })
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.invalid().is_empty() {
            return;
        }
        self.open = false;
        let Some(object) = self.object(cx) else {
            return;
        };
        let view = repeat_of(&object.fields).as_ref().map(to_draft);
        let assignee = self.save_assignee(&object, cx);
        // Re-saving an identical rule could resurrect a completed occurrence.
        if view.as_ref().is_some_and(|v| same_rule(v, &self.draft)) {
            if let Some(task) = assignee {
                self.run(
                    cx.background_spawn(async move { task.await.map(|_| ()) }),
                    cx,
                );
            }
            cx.notify();
            return;
        }
        let d = self.draft.clone();
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
            times: if sub { vec![] } else { sorted_times(&d.times) },
            window: if sub { d.window } else { None },
            tz: iana_time_zone::get_timezone().unwrap_or_default(),
            // Noon, so the engine lands on the same wall-clock day across DST.
            anchor_ms: Some(d.anchor + DAY / 2),
        };
        let set = api::repeat::set(&object.id, &rule, cx);
        self.run(
            cx.background_spawn(async move {
                if let Some(a) = assignee {
                    a.await?;
                }
                set.await?;
                Ok(())
            }),
            cx,
        );
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        let task = api::repeat::clear(&self.object_id, cx);
        self.run(
            cx.background_spawn(async move { task.await.map(|_| ()) }),
            cx,
        );
    }

    fn run_now(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        let v = Value::map(Fields::from([(
            "at".to_string(),
            Value::int(chrono::Utc::now().timestamp_millis()),
        )]));
        let task = api::note::set_field(&self.object_id, "run_now", v, cx);
        self.run(
            cx.background_spawn(async move { task.await.map(|_| ()) }),
            cx,
        );
    }

    fn skip_run(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        let task = api::note::set_field(&self.object_id, "done", Value::bool(true), cx);
        self.run(
            cx.background_spawn(async move { task.await.map(|_| ()) }),
            cx,
        );
    }

    fn open_calendar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.calendar.is_some() {
            self.calendar = None;
            cx.notify();
            return;
        }
        let cal = cx.new(|cx| CalendarPicker::new(self.draft.anchor, window, cx));
        let sub = cx.subscribe(&cal, |this, _, ev: &CalendarEvent, cx| {
            if let CalendarEvent::Pick(Some(ms)) = ev {
                this.set_anchor(*ms, cx);
            }
            this.calendar = None;
            cx.notify();
        });
        self._subs.push(sub);
        self.calendar = Some(cal);
        cx.notify();
    }
}

fn lbl(text: &'static str) -> Div {
    div()
        .flex_none()
        .w(px(52.))
        .text_size(px(12.))
        .text_color(theme::muted())
        .child(text)
}

fn fieldrow() -> Div {
    div().flex().flex_wrap().items_center().gap(px(6.))
}

fn small_btn(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id.into())
        .border_1()
        .border_color(theme::border())
        .rounded(theme::radius::sm())
        .px(px(9.))
        .py(px(3.))
        .text_size(px(12.))
        .text_color(theme::fg())
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
        .child(label.into())
}

fn select_box(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap(px(6.))
        .bg(hsla(0., 0., 1., 0.05))
        .border_1()
        .border_color(theme::border())
        .rounded(theme::radius::sm())
        .px(px(6.))
        .py(px(3.))
        .text_size(px(12.5))
        .cursor_pointer()
        .child(label.into())
        .child(
            div()
                .text_size(px(10.))
                .text_color(theme::muted())
                .child("▾"),
        )
}

fn menu_list() -> Div {
    div()
        .flex()
        .flex_col()
        .min_w(px(140.))
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .rounded(theme::radius::menu())
        .p(px(4.))
        .shadow(pop_shadow())
        .occlude()
}

fn menu_row(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    div()
        .id(id.into())
        .px(px(8.))
        .py(px(5.))
        .rounded(theme::radius::sm())
        .text_size(px(12.5))
        .cursor_pointer()
        .when(on, |d| d.bg(theme::hl_med()))
        .hover(|s| s.bg(theme::hover()))
        .child(label.into())
}

fn seg(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    first: bool,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .px(px(9.))
        .py(px(3.))
        .text_size(px(12.))
        .cursor_pointer()
        .when(!first, |d| d.border_l_1().border_color(theme::border()))
        .text_color(if on { theme::fg() } else { theme::muted() })
        .when(on, |d| d.bg(rgba(0x7896ff2e)))
        .child(label.into())
}

impl Render for RepeatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = self.object(cx) else {
            return div();
        };
        self.resolve(&object, cx);
        let rule = repeat_of(&object.fields);
        let view = rule.as_ref().map(to_draft);
        let now = chrono::Utc::now().timestamp_millis();
        let overdue = rule.as_ref().is_some_and(|r| r.next < now);
        let stuck = object
            .field_str("error")
            .starts_with("run never finished: ");
        let agent_owned =
            object.fields.contains_key("assignee") || object.fields.contains_key("agent");
        let requested = object.fields.contains_key("run_now");
        let active_run = object
            .field("run_active")
            .and_then(|v| v.entries())
            .is_some();
        let running_on = if active_run {
            self.serving_name.clone()
        } else {
            String::new()
        };
        let open_run = rule
            .as_ref()
            .is_some_and(|r| r.fired_for == Some(r.next) && !r.freq.is_sub_daily());
        let store = Store::global(cx).read(cx);
        let guests: Vec<_> = guest_agents(&object.fields)
            .iter()
            .filter_map(|id| store.agent(id).cloned())
            .collect();
        let has_guests = !guest_agents(&object.fields).is_empty();

        let label_col = div().flex_none().w(relative(0.4)).max_w(px(120.));
        let hover_group: SharedString = "repeat-row".into();
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.))
            .text_size(px(13.))
            .child(
                div()
                    .id("repeat-row")
                    .group(hover_group.clone())
                    .flex()
                    .gap(px(10.))
                    .min_h(px(30.))
                    .pl(px(6.))
                    .pr(px(24.))
                    .py(px(4.))
                    .rounded(theme::radius::sm())
                    .line_height(px(22.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hover()))
                    .tooltip(tip("Repeat"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_editor(window, cx)))
                    .child(
                        label_col
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .min_h(px(22.))
                            .text_color(theme::muted())
                            .child(
                                Icon::new(Glyph::PropRepeat)
                                    .size(px(14.))
                                    .text_color(theme::muted()),
                            )
                            .child("Repeat"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .min_h(px(22.))
                            .map(|d| match &view {
                                Some(v) => d.child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(describe_draft(v)),
                                ),
                                None => d.child(placeholder(hover_group.clone(), "Off")),
                            }),
                    ),
            );
        let meta = || {
            div()
                .flex()
                .flex_wrap()
                .gap_x(px(6.))
                .gap_y(px(2.))
                .pl(px(136.))
                .text_size(px(11.5))
                .text_color(theme::muted())
        };
        let red = theme::red();
        if let Some(rule) = &rule {
            if stuck || overdue || (agent_owned && !self.serving_warning.is_empty()) {
                let mut m = meta();
                if stuck || overdue {
                    m = m.child(div().text_color(red).child(if stuck {
                        format!("Stuck since {} - see Error", fmt_long(rule.next))
                    } else {
                        format!("Overdue since {}", fmt_long(rule.next))
                    }));
                }
                if agent_owned && !self.serving_warning.is_empty() {
                    m = m.child(div().text_color(red).child(self.serving_warning.clone()));
                }
                root = root.child(m);
            }
            if let Some(moved) = &rule.moved {
                let reason = if moved.reason.is_empty() {
                    String::new()
                } else {
                    format!(": {}", moved.reason)
                };
                root = root.child(meta().child(format!(
                    "Next run moved to {} (was {}){reason}",
                    fmt_long(rule.next),
                    fmt_long(moved.from)
                )));
            }
            if rule.fired_at.is_some() || rule.last_run.is_some() {
                let mut m = meta();
                let mut first = true;
                if let Some(at) = rule.fired_at {
                    let by = rule
                        .fired_by
                        .as_ref()
                        .filter(|b| !b.is_empty())
                        .map(|b| format!(" by {}", machine_name(&self.machines_known, b)))
                        .unwrap_or_default();
                    m = m.child(format!("fired {}{by}", fmt_long(at)));
                    first = false;
                }
                if let Some(last) = &rule.last_run {
                    let on = if last.machine.is_empty() {
                        String::new()
                    } else {
                        format!(" on {}", machine_name(&self.machines_known, &last.machine))
                    };
                    let mut span = div().flex().child(format!(
                        "{}last run {}{on}",
                        if first { "" } else { "· " },
                        fmt_long(last.at)
                    ));
                    if let Some(e) = last.error.as_ref().filter(|e| !e.is_empty()) {
                        span = span.child(div().text_color(red).child(format!(" · {e}")));
                    }
                    m = m.child(span);
                }
                root = root.child(m);
            }
        }
        if !self.error.is_empty() {
            root = root.child(
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(theme::radius::sm())
                    .bg(rgba(0xe5484d1a))
                    .border_1()
                    .border_color(rgba(0xe5484d4d))
                    .text_size(px(12.5))
                    .child(self.error.clone()),
            );
        }
        if active_run {
            let on = if running_on.is_empty() {
                String::new()
            } else {
                format!(" on {running_on}")
            };
            root = root.child(div().ml(px(26.)).mt(px(2.)).text_size(px(12.)).text_color(theme::muted()).child(format!(
                "Running now{on} - the result lands in the agent's chat and under “last run”."
            )));
        } else if requested {
            root = root.child(
                div()
                    .ml(px(26.))
                    .mt(px(2.))
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child("Run now asked for - waiting for its computer to start it."),
            );
        }
        if !self.open {
            return root;
        }

        // ── The editor popover ──
        let d = self.draft.clone();
        let sub = d.freq.is_sub_daily();
        let mut pop = div()
            // `.pop`: 360px; `min(360px, calc(100vw - 32px))` at ≤720px.
            .w(if roostr_core::layout::is_mobile(window) {
                px(360.).min(window.viewport_size().width - px(32.))
            } else {
                px(360.)
            })
            .flex()
            .flex_col()
            .gap(px(10.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::menu())
            .pt(px(10.))
            .px(px(12.))
            .pb(px(12.))
            .shadow(pop_shadow())
            .text_color(theme::fg())
            .text_size(px(13.))
            .occlude()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child("REPEAT"),
            );
        if !has_guests {
            pop = pop.child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme::muted())
                    .child("Add an agent to this object's Agent property first. A repeat needs an agent on the guest list to run each occurrence."),
            );
        } else {
            // Agent
            let current = guests.iter().find(|a| a.id == self.assignee);
            let agent_label = current.map_or_else(String::new, |a| {
                format!(
                    "{}{}",
                    if a.icon.is_empty() {
                        String::new()
                    } else {
                        format!("{} ", a.icon)
                    },
                    if a.name.is_empty() { "Agent" } else { &a.name }
                )
            });
            let mut agent_wrap =
                div()
                    .relative()
                    .child(
                        select_box("agent-select", agent_label).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.menu = if this.menu == Some(Menu::Agent) {
                                    None
                                } else {
                                    Some(Menu::Agent)
                                };
                                cx.notify();
                            },
                        )),
                    );
            if self.menu == Some(Menu::Agent) {
                let mut list = menu_list();
                for a in &guests {
                    let id = a.id.clone();
                    let label = format!(
                        "{}{}",
                        if a.icon.is_empty() {
                            String::new()
                        } else {
                            format!("{} ", a.icon)
                        },
                        if a.name.is_empty() { "Agent" } else { &a.name }
                    );
                    list = list.child(
                        menu_row(
                            SharedString::from(format!("agent-{}", a.id)),
                            label,
                            a.id == self.assignee,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.assignee = id.clone();
                            this.menu = None;
                            cx.notify();
                        })),
                    );
                }
                agent_wrap = agent_wrap.child(below("agent-menu", 4., list));
            }
            pop = pop.child(fieldrow().child(lbl("Agent")).child(agent_wrap));

            // Every N <freq>
            let plural = d.interval != 1;
            let mut freq_wrap = div().relative().child(
                select_box("freq-select", freq_word(d.freq, plural)).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.menu = if this.menu == Some(Menu::Freq) {
                            None
                        } else {
                            Some(Menu::Freq)
                        };
                        cx.notify();
                    },
                )),
            );
            if self.menu == Some(Menu::Freq) {
                let mut list = menu_list();
                for f in FREQS {
                    list = list.child(
                        menu_row(
                            SharedString::from(format!(
                                "freq-{}",
                                roostr_core::repeat::freq_name(f)
                            )),
                            freq_word(f, plural),
                            f == d.freq,
                        )
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.set_freq(f, window, cx)),
                        ),
                    );
                }
                freq_wrap = freq_wrap.child(below("freq-menu", 4., list));
            }
            pop = pop.child(
                fieldrow()
                    .child(lbl("Every"))
                    .child(field(&self.interval, FieldSkin::Compact, window, cx).w(px(48.)))
                    .child(freq_wrap),
            );

            if !sub {
                let date = date_of(d.anchor).format("%m/%d/%Y").to_string();
                let mut starts =
                    div().relative().child(select_box("anchor", date).on_click(
                        cx.listener(|this, _, window, cx| this.open_calendar(window, cx)),
                    ));
                if let Some(cal) = &self.calendar {
                    starts = starts.child(below("anchor-calendar", 4., cal.clone()));
                }
                pop = pop.child(fieldrow().child(lbl("Starts")).child(starts));
            }

            if sub {
                pop = pop.child(
                    fieldrow().child(lbl("Hours")).child(
                        div()
                            .flex()
                            .border_1()
                            .border_color(theme::border())
                            .rounded(theme::radius::sm())
                            .overflow_hidden()
                            .child(
                                seg("all-day", "All day", d.window.is_none(), true).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.draft.window = None;
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                seg("between", "Only between", d.window.is_some(), false).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        if this.draft.window.is_none() {
                                            this.draft.window = Some([8 * 60, 20 * 60]);
                                            this.sync_inputs(window, cx);
                                        }
                                        cx.notify();
                                    }),
                                ),
                            ),
                    ),
                );
                if d.window.is_some() {
                    pop = pop.child(
                        fieldrow()
                            .child(lbl(""))
                            .child(
                                field(&self.window_from, FieldSkin::Compact, window, cx).w(px(64.)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::muted())
                                    .child("–"),
                            )
                            .child(
                                field(&self.window_until, FieldSkin::Compact, window, cx)
                                    .w(px(64.)),
                            ),
                    );
                }
            }

            if d.freq == RepeatFreq::Week || sub {
                let mut days = div().flex().gap(px(3.));
                for (i, name) in WD.iter().enumerate() {
                    let on = d.weekdays.contains(&(i as u8));
                    days = days.child(
                        div()
                            .id(("day", i))
                            .size(px(26.))
                            .rounded_full()
                            .border_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.))
                            .cursor_pointer()
                            .map(|e| {
                                if on {
                                    e.bg(theme::accent())
                                        .border_color(theme::accent())
                                        .text_color(theme::white())
                                } else {
                                    e.border_color(theme::border()).text_color(theme::muted())
                                }
                            })
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_weekday(i as u8, cx)),
                            )
                            .child(name[..1].to_string()),
                    );
                }
                let mut row = fieldrow().child(lbl("On")).child(days);
                if d.weekdays.is_empty() {
                    row = row.child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .child("every day"),
                    );
                }
                pop = pop.child(row);
            }

            if d.freq == RepeatFreq::Month {
                let anchor = date_of(d.anchor);
                let ord = ["", "First", "Second", "Third", "Fourth", "Last"][ordinal_of(d.anchor)];
                pop = pop.child(
                    fieldrow()
                        .child(lbl("On"))
                        .child(
                            div()
                                .flex()
                                .border_1()
                                .border_color(theme::border())
                                .rounded(theme::radius::sm())
                                .overflow_hidden()
                                .child(
                                    seg(
                                        "by-date",
                                        format!("Day {}", anchor.day()),
                                        d.monthly == "date",
                                        true,
                                    )
                                    .tooltip(tip("The same date every month"))
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.draft.monthly = "date".into();
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    seg(
                                        "by-weekday",
                                        format!("{ord} {}", anchor.format("%A")),
                                        d.monthly == "weekday",
                                        false,
                                    )
                                    .tooltip(tip("The same weekday of the same week every month"))
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.draft.monthly = "weekday".into();
                                            cx.notify();
                                        },
                                    )),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child("of the month"),
                        ),
                );
            }

            if !sub {
                let mut list = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.));
                let many = self.times.len() > 1;
                for (i, input) in self.times.iter().enumerate() {
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(field(input, FieldSkin::Compact, window, cx).w(px(64.)))
                            .when(many, |e| {
                                e.child(
                                    div()
                                        .id(("time-rm", i))
                                        .px(px(3.))
                                        .text_size(px(14.))
                                        .text_color(theme::muted())
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(theme::red()))
                                        .tooltip(tip("Remove time"))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.remove_time(i, window, cx)
                                        }))
                                        .child("×"),
                                )
                            }),
                    );
                }
                list = list.child(
                    div()
                        .id("add-time")
                        .px(px(4.))
                        .py(px(3.))
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme::fg()))
                        .on_click(cx.listener(|this, _, window, cx| this.add_time(window, cx)))
                        .child("+ Add time"),
                );
                pop = pop.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(6.))
                        .child(lbl("At").pt(px(5.)))
                        .child(list),
                );
            }

            let invalid = self.invalid();
            let next = preview(&d);
            let mut nexts = div()
                .flex()
                .flex_wrap()
                .gap_x(px(10.))
                .gap_y(px(4.))
                .text_size(px(11.5))
                .text_color(theme::muted());
            for (i, p) in next.iter().enumerate() {
                nexts = nexts.child(
                    div()
                        .when(i == 0, |e| e.text_color(theme::fg()))
                        .child(fmt_long(*p)),
                );
            }
            nexts = nexts.child("…");
            pop = pop.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .border_t_1()
                    .border_color(theme::border())
                    .pt(px(8.))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .when(!invalid.is_empty(), |e| e.text_color(red))
                            .child(if invalid.is_empty() {
                                describe_draft(&d)
                            } else {
                                invalid.to_string()
                            }),
                    )
                    .child(nexts),
            );
        }
        // Footer
        let busy = self.busy;
        // Turn off / Skip: bordered like the other footer buttons, red on hover.
        let rm = |id: &'static str, label: &'static str| {
            small_btn(id, label)
                .text_color(theme::muted())
                .when(busy, |e| e.opacity(0.5).cursor_default())
                .when(!busy, |e| {
                    e.hover(|s| s.text_color(theme::red()).border_color(theme::red()))
                })
        };
        let mut foot = div().flex().items_center().gap(px(6.));
        if rule.is_some() {
            foot = foot.child(rm("clear", "Turn off repeating").on_click(cx.listener(
                move |this, _, _, cx| {
                    if !busy {
                        this.clear(cx)
                    }
                },
            )));
        }
        if rule.is_some() && open_run && !active_run {
            foot = foot.child(
                rm("skip", "Skip this run")
                    .tooltip(tip(
                        "Finish the open run without doing it; the next run follows on schedule",
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !busy {
                            this.skip_run(cx)
                        }
                    })),
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
            foot = foot.child(
                small_btn("run-now", label)
                    .tooltip(tip(title))
                    .when(disabled, |e| e.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !disabled {
                            this.run_now(cx)
                        }
                    })),
            );
        }
        let save_disabled = busy || !has_guests || !self.invalid().is_empty();
        foot = foot
            .child(div().flex_1())
            .child(
                small_btn("cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| {
                    this.open = false;
                    cx.notify();
                })),
            )
            .child(
                small_btn("save", if rule.is_some() { "Update" } else { "Repeat" })
                    .when(save_disabled, |e| e.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !save_disabled {
                            this.save(cx)
                        }
                    })),
            );
        pop = pop.child(foot);
        let close = cx.entity().downgrade();
        root.child(backdrop("repeat-backdrop", move |_, cx| {
            let _ = close.update(cx, |this, cx| {
                this.open = false;
                cx.notify();
            });
        }))
        .child(below("repeat-pop", 6., pop))
    }
}

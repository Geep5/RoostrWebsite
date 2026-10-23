//! Recurring objects (`Repeat.svelte`): the engine owns the rule
//! (`fields.repeat`) and the occurrence math; this renders when it next
//! runs and lets a person edit the cadence, run it now, skip the open run or
//! turn repeating off.

use chrono::{Datelike as _, Local, NaiveDate, TimeZone as _};
use gpui_kit::component::button::{
    Button, ButtonCustomVariant, ButtonVariants as _, Toggle, ToggleGroup, ToggleVariants as _,
};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{FocusableExt as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::repeat::{
    DAY, DAY_MINUTES, RepeatDraft, WD, describe_draft, ordinal_of, sod, sorted_times, to_draft,
};
use roostr_core::types::{guest_agents, repeat_of};
use roostr_core::{Fields, Glyph, Object, RepeatFreq, RepeatRule, Store, Value, api, theme, ui};

use crate::calendar::{CalendarEvent, CalendarPicker, local_date, local_midnight_ms};
use crate::widgets::{
    FieldSkin, backdrop, below, error_alert, field, note, placeholder, pop_shadow,
};
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

/// One choice of the Agent / Every selects: its value and how it reads.
#[derive(Clone, PartialEq)]
struct Choice {
    value: String,
    label: SharedString,
}

impl SearchableListItem for Choice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &String {
        &self.value
    }
}

type ChoiceSelect = Entity<SelectState<Vec<Choice>>>;

/// A kit select over `choices`, its items and value kept in step with the
/// draft: re-seeded only when they change, so an open list is left alone.
struct Picker {
    select: ChoiceSelect,
    choices: Vec<Choice>,
    value: String,
}

impl Picker {
    fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            select: cx.new(|cx| SelectState::new(Vec::<Choice>::new(), None, window, cx)),
            choices: vec![],
            value: String::new(),
        }
    }

    fn sync(&mut self, choices: Vec<Choice>, value: &str, window: &mut Window, cx: &mut App) {
        let changed = choices != self.choices;
        if changed {
            self.choices = choices.clone();
            self.select
                .update(cx, |s, cx| s.set_items(choices, window, cx));
        }
        if changed || value != self.value {
            self.value = value.to_string();
            let value = value.to_string();
            self.select
                .update(cx, |s, cx| s.set_selected_value(&value, window, cx));
        }
    }
}

pub(crate) struct RepeatView {
    object_id: String,
    open: bool,
    draft: RepeatDraft,
    assignee: String,
    busy: bool,
    error: String,
    agent_picker: Picker,
    freq_picker: Picker,
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
        let agent_picker = Picker::new(window, cx);
        let freq_picker = Picker::new(window, cx);
        let subs = subs
            .into_iter()
            .chain([
                cx.subscribe_in(
                    &agent_picker.select,
                    window,
                    |this, _, ev: &SelectEvent<Vec<Choice>>, _, cx| {
                        let SelectEvent::Confirm(Some(id)) = ev else {
                            return;
                        };
                        this.assignee = id.clone();
                        cx.notify();
                    },
                ),
                cx.subscribe_in(
                    &freq_picker.select,
                    window,
                    |this, _, ev: &SelectEvent<Vec<Choice>>, window, cx| {
                        let SelectEvent::Confirm(Some(name)) = ev else {
                            return;
                        };
                        if let Some(&f) = FREQS
                            .iter()
                            .find(|&&f| roostr_core::repeat::freq_name(f) == name.as_str())
                        {
                            this.set_freq(f, window, cx);
                        }
                    },
                ),
            ])
            .collect();
        Self {
            object_id,
            open: false,
            draft: seed(),
            assignee: String::new(),
            busy: false,
            error: String::new(),
            agent_picker,
            freq_picker,
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

/// A form row's caption (kit [`Label`]): 12px muted, a 52px column.
fn lbl(text: &'static str) -> Label {
    note(text, 12.).flex_none().w(px(52.))
}

fn fieldrow() -> Div {
    div().flex().flex_wrap().items_center().gap(px(6.))
}

/// A footer button: kit outline [`Button`] (kit hover fill), small, 12px
/// label; `hover_fg` recolours the label while the button is hovered.
fn small_btn(id: &'static str, label: impl Into<SharedString>, hover_fg: Option<Hsla>) -> Button {
    let label: SharedString = label.into();
    Button::new(id)
        .outline()
        .small()
        .group(id)
        .accessibility_label(label.clone())
        .h_auto()
        .px(px(9.))
        .py(px(3.))
        .rounded(theme::radius::sm())
        .bg(transparent_black())
        .border_color(theme::border())
        .text_color(theme::fg())
        .child(
            div()
                .text_size(px(12.))
                .when_some(hover_fg, |d, fg| d.group_hover(id, |s| s.text_color(fg)))
                .child(label),
        )
}

/// The Starts date trigger: kit outline [`Button`] dressed as the selects
/// (white 5% fill, radius 6, 3px 6px, 12.5px) with a chevron.
fn date_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    let label: SharedString = label.into();
    Button::new(id)
        .outline()
        .small()
        .accessibility_label(label.clone())
        .h_auto()
        .px(px(6.))
        .py(px(3.))
        .rounded(theme::radius::sm())
        .bg(hsla(0., 0., 1., 0.05))
        .border_color(theme::border())
        .text_color(theme::fg())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.5))
                .child(label)
                .child(
                    Icon::new(Glyph::ChevronDown)
                        .size(px(10.))
                        .text_color(theme::muted()),
                ),
        )
}

/// The Agent / Every select: the kit `Select` dressed as `select_box` (white
/// 5% fill, 1px border, radius 6, 3px 6px, 12.5px), its list `menu_w` wide.
fn choice_select(select: &ChoiceSelect, id: &'static str, menu_w: f32) -> impl IntoElement {
    div().flex().flex_none().child(
        Select::new(select)
            .id(id)
            .focus_ring(false)
            // Like the old box: nothing chosen reads as blank, not "Select…".
            .placeholder("")
            .menu_width(px(menu_w))
            .bg(hsla(0., 0., 1., 0.05))
            .border_color(theme::border())
            .rounded(theme::radius::sm())
            .h_auto()
            .px(px(6.))
            .py(px(3.))
            .gap(px(6.))
            .text_size(px(12.5))
            .text_color(theme::fg()),
    )
}

/// A joined single-choice control (kit segmented outline [`ToggleGroup`]);
/// its `on_click` gets the toggles' next states, see [`clicked`].
fn seg_group(id: impl Into<ElementId>) -> ToggleGroup {
    ToggleGroup::new(id).segmented().outline()
}

/// One segment of [`seg_group`]: 12px, muted when off, blue tint when on.
fn seg(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Toggle {
    Toggle::new(id)
        .checked(on)
        .label(label)
        .h_auto()
        .min_w_0()
        .px(px(9.))
        .py(px(3.))
        .text_size(px(12.))
        .cursor_pointer()
        .border_color(theme::border())
        .text_color(if on { theme::fg() } else { theme::muted() })
        .bg(if on {
            rgba(0x7896ff2e).into()
        } else {
            transparent_black()
        })
}

/// The toggle a [`ToggleGroup`] click flipped: where `next` differs from
/// the states it was drawn with.
fn clicked(before: &[bool], next: &[bool]) -> Option<usize> {
    before.iter().zip(next).position(|(a, b)| a != b)
}

impl Render for RepeatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = self.object(cx) else {
            return div();
        };
        let rule = repeat_of(&object.fields);
        let view = rule.as_ref().map(to_draft);
        let requested = object.fields.contains_key("run_now");
        let active_run = object
            .field("run_active")
            .and_then(|v| v.entries())
            .is_some();
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
                Button::new("repeat-row")
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .foreground(theme::fg())
                            .hover(theme::hover())
                            .active(theme::hover()),
                    )
                    .group(hover_group.clone())
                    .accessibility_label("Repeat")
                    .tooltip("Repeat")
                    .w_full()
                    .h_auto()
                    .min_h(px(30.))
                    .pl(px(6.))
                    .pr(px(24.))
                    .py(px(4.))
                    .rounded(theme::radius::sm())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.open_editor(window, cx)))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .gap(px(10.))
                            .text_size(px(13.))
                            .line_height(px(22.))
                            .child(
                                label_col
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .min_h(px(22.))
                                    .child(
                                        Icon::new(Glyph::PropRepeat)
                                            .size(px(14.))
                                            .text_color(theme::muted()),
                                    )
                                    .child(note("Repeat", 13.).line_height(px(22.))),
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
                    ),
            );
        if !self.error.is_empty() {
            root = root.child(error_alert("repeat-error", self.error.clone()).text_size(px(12.5)));
        }
        let red = theme::red();
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
            .child(note("REPEAT", 11.));
        if !has_guests {
            pop = pop.child(
                note("Add an agent to this object's Agent property first. A repeat needs an agent on the guest list to run each occurrence.", 12.5),
            );
        } else {
            // Agent
            let agents: Vec<Choice> = guests
                .iter()
                .map(|a| Choice {
                    value: a.id.clone(),
                    label: format!(
                        "{}{}",
                        if a.icon.is_empty() {
                            String::new()
                        } else {
                            format!("{} ", a.icon)
                        },
                        if a.name.is_empty() { "Agent" } else { &a.name }
                    )
                    .into(),
                })
                .collect();
            let assignee = self.assignee.clone();
            self.agent_picker.sync(agents, &assignee, window, cx);
            pop = pop.child(fieldrow().child(lbl("Agent")).child(choice_select(
                &self.agent_picker.select,
                "agent-select",
                180.,
            )));

            // Every N <freq>
            let plural = d.interval != 1;
            let freqs: Vec<Choice> = FREQS
                .iter()
                .map(|&f| Choice {
                    value: roostr_core::repeat::freq_name(f).to_string(),
                    label: freq_word(f, plural).into(),
                })
                .collect();
            self.freq_picker
                .sync(freqs, roostr_core::repeat::freq_name(d.freq), window, cx);
            pop = pop.child(
                fieldrow()
                    .child(lbl("Every"))
                    .child(field(&self.interval, FieldSkin::Compact, window, cx).w(px(48.)))
                    .child(choice_select(&self.freq_picker.select, "freq-select", 140.)),
            );

            if !sub {
                let date = date_of(d.anchor).format("%m/%d/%Y").to_string();
                let mut starts =
                    div().relative().child(date_button("anchor", date).on_click(
                        cx.listener(|this, _, window, cx| this.open_calendar(window, cx)),
                    ));
                if let Some(cal) = &self.calendar {
                    starts = starts.child(below("anchor-calendar", 4., cal.clone()));
                }
                pop = pop.child(fieldrow().child(lbl("Starts")).child(starts));
            }

            if sub {
                let before = [d.window.is_none(), d.window.is_some()];
                pop = pop.child(
                    fieldrow().child(lbl("Hours")).child(
                        seg_group("hours")
                            .child(seg("all-day", "All day", before[0]))
                            .child(seg("between", "Only between", before[1]))
                            .on_click(cx.listener(move |this, next: &Vec<bool>, window, cx| {
                                match clicked(&before, next) {
                                    Some(0) => this.draft.window = None,
                                    Some(_) if this.draft.window.is_none() => {
                                        this.draft.window = Some([8 * 60, 20 * 60]);
                                        this.sync_inputs(window, cx);
                                    }
                                    _ => {}
                                }
                                cx.notify();
                            })),
                    ),
                );
                if d.window.is_some() {
                    pop = pop.child(
                        fieldrow()
                            .child(lbl(""))
                            .child(
                                field(&self.window_from, FieldSkin::Compact, window, cx).w(px(64.)),
                            )
                            .child(note("–", 12.))
                            .child(
                                field(&self.window_until, FieldSkin::Compact, window, cx)
                                    .w(px(64.)),
                            ),
                    );
                }
            }

            if d.freq == RepeatFreq::Week || sub {
                let before: Vec<bool> = (0..WD.len())
                    .map(|i| d.weekdays.contains(&(i as u8)))
                    .collect();
                let days = ToggleGroup::new("days")
                    .gap(px(3.))
                    .children(WD.iter().enumerate().map(|(i, name)| {
                        let on = before[i];
                        Toggle::new(("day", i))
                            .checked(on)
                            .label(name[..1].to_string())
                            .size(px(26.))
                            .min_w(px(26.))
                            .p_0()
                            .rounded_full()
                            .border_1()
                            .text_size(px(11.))
                            .cursor_pointer()
                            .map(|t| {
                                if on {
                                    t.bg(theme::accent())
                                        .border_color(theme::accent())
                                        .text_color(theme::white())
                                } else {
                                    t.border_color(theme::border()).text_color(theme::muted())
                                }
                            })
                    }))
                    .on_click(cx.listener(move |this, next: &Vec<bool>, _, cx| {
                        if let Some(i) = clicked(&before, next) {
                            this.toggle_weekday(i as u8, cx)
                        }
                    }));
                let mut row = fieldrow().child(lbl("On")).child(days);
                if d.weekdays.is_empty() {
                    row = row.child(note("every day", 12.));
                }
                pop = pop.child(row);
            }

            if d.freq == RepeatFreq::Month {
                let anchor = date_of(d.anchor);
                let ord = ["", "First", "Second", "Third", "Fourth", "Last"][ordinal_of(d.anchor)];
                let before = [d.monthly == "date", d.monthly == "weekday"];
                pop = pop.child(
                    fieldrow()
                        .child(lbl("On"))
                        .child(
                            seg_group("monthly")
                                .child(
                                    seg("by-date", format!("Day {}", anchor.day()), before[0])
                                        .tooltip("The same date every month"),
                                )
                                .child(
                                    seg(
                                        "by-weekday",
                                        format!("{ord} {}", anchor.format("%A")),
                                        before[1],
                                    )
                                    .tooltip("The same weekday of the same week every month"),
                                )
                                .on_click(cx.listener(move |this, next: &Vec<bool>, _, cx| {
                                    match clicked(&before, next) {
                                        Some(0) => this.draft.monthly = "date".into(),
                                        Some(_) => this.draft.monthly = "weekday".into(),
                                        None => {}
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(note("of the month", 12.)),
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
                                    Button::new(("time-rm", i))
                                        .custom(
                                            ButtonCustomVariant::new(cx).foreground(theme::muted()),
                                        )
                                        .group("time-rm")
                                        .accessibility_label("Remove time")
                                        .tooltip("Remove time")
                                        .h_auto()
                                        .px(px(3.))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.remove_time(i, window, cx)
                                        }))
                                        .child(
                                            div()
                                                .text_size(px(14.))
                                                .group_hover("time-rm", |s| {
                                                    s.text_color(theme::red())
                                                })
                                                .child("×"),
                                        ),
                                )
                            }),
                    );
                }
                list = list.child(
                    Button::new("add-time")
                        .custom(ButtonCustomVariant::new(cx).foreground(theme::muted()))
                        .group("add-time")
                        .accessibility_label("Add time")
                        .h_auto()
                        .px(px(4.))
                        .py(px(3.))
                        .on_click(cx.listener(|this, _, window, cx| this.add_time(window, cx)))
                        .child(
                            div()
                                .text_size(px(12.))
                                .group_hover("add-time", |s| s.text_color(theme::fg()))
                                .child("+ Add time"),
                        ),
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
            let mut nexts = div().flex().flex_wrap().gap_x(px(10.)).gap_y(px(4.));
            for (i, p) in next.iter().enumerate() {
                nexts = nexts
                    .child(note(fmt_long(*p), 11.5).when(i == 0, |e| e.text_color(theme::fg())));
            }
            nexts = nexts.child(note("…", 11.5));
            pop = pop.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .border_t_1()
                    .border_color(theme::border())
                    .pt(px(8.))
                    .child(
                        Label::new(if invalid.is_empty() {
                            describe_draft(&d)
                        } else {
                            invalid.to_string()
                        })
                        .text_size(px(12.5))
                        .text_color(if invalid.is_empty() { theme::fg() } else { red }),
                    )
                    .child(nexts),
            );
        }
        // Footer
        let busy = self.busy;
        // Turn off / Skip: bordered like the other footer buttons, red on hover.
        let rm = |id: &'static str, label: &'static str| {
            let hover_fg = (!busy).then(theme::red);
            ui::disabled(
                small_btn(id, label, hover_fg).text_color(theme::muted()),
                busy,
            )
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
                    .tooltip(
                        "Finish the open run without doing it; the next run follows on schedule",
                    )
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
                ui::disabled(small_btn("run-now", label, None).tooltip(title), disabled).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if !disabled {
                            this.run_now(cx)
                        }
                    }),
                ),
            );
        }
        let save_disabled = busy || !has_guests || !self.invalid().is_empty();
        foot = foot
            .child(div().flex_1())
            .child(
                small_btn("cancel", "Cancel", None).on_click(cx.listener(|this, _, _, cx| {
                    this.open = false;
                    cx.notify();
                })),
            )
            .child(
                ui::disabled(
                    small_btn(
                        "save",
                        if rule.is_some() { "Update" } else { "Repeat" },
                        None,
                    ),
                    save_disabled,
                )
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

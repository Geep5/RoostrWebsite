//! The calendar and the date picker: gpui-kit's `Calendar` / `CalendarState` (base calendar
//! structure: month paging, the month and year pickers behind the title, day activation) drawn
//! with the cells Roostr gave it (`roostr-properties/src/calendar.rs`, Anytype's compact
//! CalendarSelect), and Roostr's `CalendarPicker` surface (today ring, selected day, Today and
//! Clear footer) behind a date trigger.
//!
//! The app keeps a [`CalendarState`] in its state and hands the builder a lens to it, as GPUI
//! kept an `Entity<CalendarState>`.

use std::rc::Rc;

use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};
use rune::{Cursor, El, col, el, row, text};

use crate::overlay::{self, Placement};
use crate::theme::{Theme, radius, shadow, ty};
use ty::Weights as _;

/// A lens from the app state to a part of it (`|s: &mut S| &mut s.calendar`).
pub(crate) type Lens<S, T> = Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut T>;

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
/// One day column: Roostr's 252 pt surface less its padding and border, over 7.
const CELL_W: f32 = 32.0;
/// The selected cell's ink (`rgb(0x14100a)` in Roostr's calendar).
const ON_ACCENT: u32 = 0x14100a;

/// gpui-kit's `calendar::Date`: one day, or a range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Date {
    Single(Option<NaiveDate>),
    Range(Option<NaiveDate>, Option<NaiveDate>),
}

impl Default for Date {
    fn default() -> Self {
        Date::Single(None)
    }
}

impl From<NaiveDate> for Date {
    fn from(d: NaiveDate) -> Self {
        Date::Single(Some(d))
    }
}

impl From<(NaiveDate, NaiveDate)> for Date {
    fn from((a, b): (NaiveDate, NaiveDate)) -> Self {
        Date::Range(Some(a), Some(b))
    }
}

impl Date {
    pub fn is_some(&self) -> bool {
        matches!(self, Date::Single(Some(_)) | Date::Range(Some(_), _))
    }
    /// A single day picked, or both ends of a range.
    pub fn is_complete(&self) -> bool {
        matches!(self, Date::Single(Some(_)) | Date::Range(Some(_), Some(_)))
    }
    pub fn start(&self) -> Option<NaiveDate> {
        match self {
            Date::Single(d) => *d,
            Date::Range(a, _) => *a,
        }
    }
    pub fn end(&self) -> Option<NaiveDate> {
        match self {
            Date::Single(d) => *d,
            Date::Range(_, b) => *b,
        }
    }
    /// The start (and end) formatted with `chrono`'s `format`, joined by " - " for a range.
    pub fn format(&self, format: &str) -> Option<String> {
        match self {
            Date::Single(Some(d)) => Some(d.format(format).to_string()),
            Date::Range(Some(a), Some(b)) => {
                Some(format!("{} - {}", a.format(format), b.format(format)))
            }
            _ => None,
        }
    }
    /// `value` is the picked day or an end of the range.
    pub fn is_active(&self, value: &NaiveDate) -> bool {
        match self {
            Date::Single(d) => d.as_ref() == Some(value),
            Date::Range(a, b) => a.as_ref() == Some(value) || b.as_ref() == Some(value),
        }
    }
    pub fn is_single(&self) -> bool {
        matches!(self, Date::Single(_))
    }
    /// `value` lies strictly between a range's ends.
    pub fn is_in_range(&self, value: &NaiveDate) -> bool {
        match self {
            Date::Range(Some(a), Some(b)) => value > a && value < b,
            _ => false,
        }
    }
}

/// Which grid the calendar shows: days, the twelve months, or a page of twenty years.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CalendarView {
    #[default]
    Day,
    Month,
    Year,
}

/// gpui-kit's `CalendarState`: the picked date, the month shown, the view, the year pages
/// (today ± 50 years, twenty a page) and the days that can't be picked.
#[derive(Clone)]
pub struct CalendarState {
    view: CalendarView,
    date: Date,
    current_year: i32,
    current_month: u8,
    years: Vec<Vec<i32>>,
    year_page: usize,
    today: NaiveDate,
    disabled: Option<Rc<dyn Fn(&NaiveDate) -> bool>>,
}

impl Default for CalendarState {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CalendarState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CalendarState")
            .field("view", &self.view)
            .field("date", &self.date)
            .field("current_year", &self.current_year)
            .field("current_month", &self.current_month)
            .field("today", &self.today)
            .finish_non_exhaustive()
    }
}

impl CalendarState {
    /// Nothing picked, showing this month (today from the local clock).
    pub fn new() -> Self {
        Self::with_today(Local::now().date_naive())
    }

    /// [`Self::new`] with `today` given (tests, and views that pin the clock).
    pub fn with_today(today: NaiveDate) -> Self {
        let mut s = Self {
            view: CalendarView::Day,
            date: Date::Single(None),
            current_year: today.year(),
            current_month: today.month() as u8,
            years: Vec::new(),
            year_page: 0,
            today,
            disabled: None,
        };
        s.set_year_range((today.year() - 50, today.year() + 50));
        s
    }

    /// Days `f` matches can't be picked (shown muted, no press).
    pub fn disabled_matcher(mut self, f: impl Fn(&NaiveDate) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }

    /// The years the year view pages through, `from..to`.
    pub fn set_year_range(&mut self, (from, to): (i32, i32)) {
        self.years = (from..to)
            .collect::<Vec<_>>()
            .chunks(20)
            .map(<[_]>::to_vec)
            .collect();
        self.year_page = self
            .years
            .iter()
            .position(|v| v.contains(&self.current_year))
            .unwrap_or(0);
    }

    fn is_disabled(&self, d: &NaiveDate) -> bool {
        self.disabled.as_ref().is_some_and(|f| f(d))
    }

    /// Sets the date and pages to its month; false (and unchanged) when a disabled day.
    pub fn set_date(&mut self, date: impl Into<Date>) -> bool {
        let date = date.into();
        if [date.start(), date.end()]
            .iter()
            .flatten()
            .any(|d| self.is_disabled(d))
        {
            return false;
        }
        self.date = date;
        if let Some(v) = date.start() {
            self.current_month = v.month() as u8;
            self.current_year = v.year();
        }
        true
    }

    /// Picks `value` as gpui-kit does (a range restarts or completes); true when the date is
    /// then complete.
    pub fn select_date(&mut self, value: NaiveDate) -> bool {
        if self.is_disabled(&value) {
            return false;
        }
        let next = match self.date {
            Date::Single(_) => Date::Single(Some(value)),
            Date::Range(None, _) => Date::Range(Some(value), None),
            Date::Range(Some(start), None) if value >= start => {
                Date::Range(Some(start), Some(value))
            }
            Date::Range(Some(_), _) => Date::Range(Some(value), None),
        };
        self.set_date(next);
        self.date.is_complete()
    }

    pub fn date(&self) -> Date {
        self.date
    }
    pub fn view(&self) -> CalendarView {
        self.view
    }
    pub fn set_view(&mut self, view: CalendarView) {
        self.view = view;
    }
    pub fn current_year(&self) -> i32 {
        self.current_year
    }
    pub fn current_month(&self) -> u8 {
        self.current_month
    }
    pub fn today(&self) -> NaiveDate {
        self.today
    }
    pub fn years_on_page(&self) -> &[i32] {
        self.years
            .get(self.year_page)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
    pub fn has_prev_year_page(&self) -> bool {
        self.year_page > 0
    }
    pub fn has_next_year_page(&self) -> bool {
        self.year_page + 1 < self.years.len()
    }
    pub fn prev_year_page(&mut self) -> bool {
        let ok = self.has_prev_year_page();
        if ok {
            self.year_page -= 1;
        }
        ok
    }
    pub fn next_year_page(&mut self) -> bool {
        let ok = self.has_next_year_page();
        if ok {
            self.year_page += 1;
        }
        ok
    }
    pub fn prev_month(&mut self) {
        if self.current_month == 1 {
            self.current_year -= 1;
            self.current_month = 12;
        } else {
            self.current_month -= 1;
        }
    }
    pub fn next_month(&mut self) {
        if self.current_month == 12 {
            self.current_year += 1;
            self.current_month = 1;
        } else {
            self.current_month += 1;
        }
    }
    /// Picks a month in the month view and returns to the days.
    pub fn select_month(&mut self, month: u8) {
        self.current_month = month;
        self.view = CalendarView::Day;
    }
    /// Picks a year in the year view and returns to the days.
    pub fn select_year(&mut self, year: i32) {
        self.current_year = year;
        self.view = CalendarView::Day;
    }
    /// The header's previous control: a month back in the day view, a year page back in the
    /// year view (nothing in the month view).
    pub fn previous(&mut self) {
        match self.view {
            CalendarView::Day => self.prev_month(),
            CalendarView::Year => {
                self.prev_year_page();
            }
            CalendarView::Month => {}
        }
    }
    /// The header's next control (see [`Self::previous`]).
    pub fn next(&mut self) {
        match self.view {
            CalendarView::Day => self.next_month(),
            CalendarView::Year => {
                self.next_year_page();
            }
            CalendarView::Month => {}
        }
    }
    /// The title's month or year half: opens that picker, or goes back to the days.
    pub fn toggle_view(&mut self, view: CalendarView) {
        self.view = if self.view == view {
            CalendarView::Day
        } else {
            view
        };
    }
    /// The shown month's weeks, starting on `first_day` (as many weeks as the month spans).
    pub fn weeks(&self, first_day: Weekday) -> Vec<Vec<NaiveDate>> {
        days_in_month(self.current_year, self.current_month as u32, first_day)
    }
}

fn days_in_month(year: i32, month: u32, first_day: Weekday) -> Vec<Vec<NaiveDate>> {
    let first = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .expect("valid month");
    let offset =
        (first.weekday().num_days_from_sunday() + 7 - first_day.num_days_from_sunday()) % 7;
    let start = first - Duration::days(offset as i64);
    let count = ((next - start).num_days() as usize).div_ceil(7) * 7;
    (0..count)
        .map(|n| start + Duration::days(n as i64))
        .collect::<Vec<_>>()
        .chunks(7)
        .map(<[_]>::to_vec)
        .collect()
}

/// A centred label in Roostr's UI face.
fn label(s: impl Into<String>, size: f32, color: rune::Color) -> El {
    text(s.into()).font(ty::ui(size)).color(color)
}

/// gpui-kit's `Calendar` with Roostr's cells: header (‹, month, year, ›), weekday row (Monday
/// first by default) and the day grid, or the month / year picker. Replaces
/// `gpui_kit::base::Calendar` + `CalendarState` as `roostr-properties` drew it.
pub struct Calendar<S> {
    key: String,
    state: CalendarState,
    lens: Lens<S, CalendarState>,
    first_day: Weekday,
    on_select: Option<Rc<dyn Fn(&mut S, Date)>>,
}

impl<S: 'static> Calendar<S> {
    /// `state` is the app's calendar state and `lens` reaches it in the handlers.
    pub fn new(
        key: impl Into<String>,
        state: &CalendarState,
        lens: impl for<'a> Fn(&'a mut S) -> &'a mut CalendarState + 'static,
    ) -> Self {
        Self {
            key: key.into(),
            state: state.clone(),
            lens: Rc::new(lens),
            first_day: Weekday::Mon,
            on_select: None,
        }
    }

    /// The column the weeks start on (Roostr: Monday).
    pub fn first_day_of_week(mut self, day: Weekday) -> Self {
        self.first_day = day;
        self
    }

    /// Runs with the date once it is complete (gpui-kit's `CalendarEvent::Selected`).
    pub fn on_select(mut self, f: impl Fn(&mut S, Date) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    /// A press on `e` that changes the state through `f`.
    fn act(&self, e: El, f: impl Fn(&mut CalendarState) + 'static) -> El {
        let lens = self.lens.clone();
        e.on_press(move |s: &mut S| f(lens(s)))
    }

    pub fn build(self, t: &Theme) -> El {
        let st = &self.state;
        let k = &self.key;
        let view = st.view();
        let nav = |id: &str, glyph: &str, disabled: bool, f: fn(&mut CalendarState)| {
            let mut e = row()
                .key(format!("{k}-{id}"))
                .role("button")
                .label(if id == "prev" { "Previous" } else { "Next" })
                .px(8.0)
                .py(2.0)
                .radius(radius::SM)
                .child(label(glyph, 16.0, t.muted).group_color(t.fg));
            if disabled {
                e = e.layer_opacity(0.3);
            } else {
                e = self.act(
                    e.hover_group()
                        .hover(t.hover)
                        .cursor(Cursor::Pointer)
                        .focusable(true),
                    f,
                );
            }
            e
        };
        let toggle = |id: &str, s: String, which: CalendarView, end: bool| {
            let active = view == which;
            let mut e = row()
                .key(format!("{k}-{id}"))
                .role("button")
                .grow(1.0)
                .hover_group()
                .cursor(Cursor::Pointer)
                .focusable(true)
                .child(
                    label(s, 13.0, if active { t.accent } else { t.fg })
                        .font(ty::ui(13.0).semibold())
                        .group_color(t.accent),
                );
            e = if end {
                e.justify_end().pr(2.0)
            } else {
                e.justify_start().pl(2.0)
            };
            self.act(e, move |c| c.toggle_view(which))
        };
        let header = row()
            .key(format!("{k}-header"))
            .w_full()
            .center_y()
            .justify_between()
            .child(nav(
                "prev",
                "‹",
                view == CalendarView::Month
                    || (view == CalendarView::Year && !st.has_prev_year_page()),
                CalendarState::previous,
            ))
            .child(toggle(
                "month",
                MONTHS[st.current_month() as usize - 1].to_owned(),
                CalendarView::Month,
                true,
            ))
            .child(toggle(
                "year",
                st.current_year().to_string(),
                CalendarView::Year,
                false,
            ))
            .child(nav(
                "next",
                "›",
                view == CalendarView::Month
                    || (view == CalendarView::Year && !st.has_next_year_page()),
                CalendarState::next,
            ));
        let body = match view {
            CalendarView::Day => self.days(t),
            CalendarView::Month => {
                let current = st.current_month();
                let cells = (1..=12u8)
                    .map(|m| {
                        let e = self.pick_cell(
                            t,
                            format!("{k}-m{m}"),
                            MONTHS[m as usize - 1],
                            m == current,
                        );
                        self.act(e, move |c| c.select_month(m))
                    })
                    .collect();
                pick_grid(cells, 3)
            }
            CalendarView::Year => {
                let current = st.current_year();
                let cells = st
                    .years_on_page()
                    .iter()
                    .map(|&y| {
                        let e =
                            self.pick_cell(t, format!("{k}-y{y}"), &y.to_string(), y == current);
                        self.act(e, move |c| c.select_year(y))
                    })
                    .collect();
                pick_grid(cells, 5)
            }
        };
        col().key(k.clone()).gap(8.0).child(header).child(body)
    }

    /// A month or year picker cell.
    fn pick_cell(&self, t: &Theme, key: String, s: &str, active: bool) -> El {
        let mut e = row()
            .key(key)
            .role("button")
            .grow(1.0)
            .py(5.0)
            .my(1.0)
            .center()
            .radius(radius::SM)
            .cursor(Cursor::Pointer)
            .focusable(true);
        if active {
            e = e
                .fill(t.accent)
                .child(label(s, 12.0, crate::color::rgb(ON_ACCENT)).font(ty::ui(12.0).semibold()));
        } else {
            e = e.hover(t.hover).child(label(s, 12.0, t.fg));
        }
        e
    }

    fn days(&self, t: &Theme) -> El {
        let st = &self.state;
        let k = &self.key;
        let mut month = col().key(format!("{k}-days"));
        let mut head = row();
        for i in 0..7u32 {
            let wd = (i + self.first_day.num_days_from_sunday()) % 7;
            head = head.child(row().w(CELL_W).mx(0.5).py(3.0).center().child(label(
                WEEKDAYS[wd as usize],
                10.0,
                t.muted,
            )));
        }
        month = month.child(head);
        let date = st.date();
        for week in st.weeks(self.first_day) {
            let mut r = row().items_start();
            for d in week {
                let sel = date.is_active(&d);
                let disabled = st.is_disabled(&d);
                let other = d.month() != st.current_month() as u32 || disabled;
                let mut cell = row()
                    .key(format!("{k}-{d}"))
                    .role("button")
                    .label(d.to_string())
                    .checked(sel)
                    .w(CELL_W)
                    .mx(0.5)
                    .my(0.5)
                    .py(5.0)
                    .center()
                    .radius(radius::SM)
                    .cursor(Cursor::Pointer);
                if d == st.today() {
                    cell = cell.border(1.0, t.accent).border_layout();
                }
                let n = d.day().to_string();
                cell = if sel {
                    cell.fill(t.accent).child(
                        label(n, 12.0, crate::color::rgb(ON_ACCENT)).font(ty::ui(12.0).semibold()),
                    )
                } else if other {
                    cell.hover(t.hover)
                        .child(label(n, 12.0, t.muted).opacity(0.5))
                } else {
                    cell.hover(t.hover).child(label(n, 12.0, t.fg))
                };
                if !disabled {
                    let lens = self.lens.clone();
                    let on_select = self.on_select.clone();
                    cell = cell.focusable(true).on_press(move |s: &mut S| {
                        let c = lens(s);
                        if c.select_date(d) {
                            let date = c.date();
                            if let Some(f) = &on_select {
                                f(s, date);
                            }
                        }
                    });
                }
                r = r.child(cell);
            }
            month = month.child(r);
        }
        month
    }
}

/// gpui-kit's month (3 columns) and year (5 columns) picker grid, 4 pt between columns.
fn pick_grid(cells: Vec<El>, columns: usize) -> El {
    let mut grid = col();
    let mut cells = cells.into_iter().peekable();
    while cells.peek().is_some() {
        let mut r = row().w_full().gap(4.0);
        for _ in 0..columns {
            r = r.child(match cells.next() {
                Some(c) => c,
                None => el().grow(1.0),
            });
        }
        grid = grid.child(r);
    }
    grid
}

/// Roostr's date field (`roostr-properties` value editor + `CalendarPicker`): an outline
/// trigger showing the date (or a muted placeholder) that opens the calendar surface below it,
/// with Today and Clear in its footer. Picking closes it.
pub struct DatePicker<S> {
    key: String,
    calendar: Calendar<S>,
    open: bool,
    value_label: Option<String>,
    placeholder: String,
    inline: bool,
    on_toggle: Option<Rc<dyn Fn(&mut S)>>,
    on_close: Option<Rc<dyn Fn(&mut S)>>,
    on_pick: Option<Rc<dyn Fn(&mut S, Option<NaiveDate>)>>,
}

impl<S: 'static> DatePicker<S> {
    /// The trigger is keyed `key`, the calendar `{key}-calendar`.
    pub fn new(
        key: impl Into<String>,
        state: &CalendarState,
        lens: impl for<'a> Fn(&'a mut S) -> &'a mut CalendarState + 'static,
    ) -> Self {
        let key = key.into();
        Self {
            calendar: Calendar::new(format!("{key}-calendar"), state, lens),
            key,
            open: false,
            value_label: None,
            placeholder: String::new(),
            inline: false,
            on_toggle: None,
            on_close: None,
            on_pick: None,
        }
    }
    fn key(&self) -> String {
        self.key.clone()
    }
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
    /// What the trigger reads for the current value (`None` shows the placeholder).
    pub fn value_label(mut self, label: Option<String>) -> Self {
        self.value_label = label;
        self
    }
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    /// Show the calendar in the flow under the trigger (Roostr's `inline_menus` panes) rather
    /// than floating.
    pub fn inline(mut self, inline: bool) -> Self {
        self.inline = inline;
        self
    }
    pub fn first_day_of_week(mut self, day: Weekday) -> Self {
        self.calendar = self.calendar.first_day_of_week(day);
        self
    }
    /// The trigger was pressed (open or close it).
    pub fn on_toggle(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }
    /// Close it (an outside press, Escape, or a pick).
    pub fn on_close(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }
    /// A day, Today (`Some(today)`) or Clear (`None`) was picked.
    pub fn on_pick(mut self, f: impl Fn(&mut S, Option<NaiveDate>) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }

    /// The trigger: kit outline button on `bg`, 10 × 5 padding, 13 pt label.
    fn trigger(&self, t: &Theme) -> El {
        let (s, c) = match &self.value_label {
            Some(v) => (v.clone(), t.fg),
            None => (self.placeholder.clone(), t.muted),
        };
        let mut e = row()
            .key(self.key())
            .role("button")
            .expanded(self.open)
            .center_y()
            .px(10.0)
            .py(5.0)
            .radius(radius::SM)
            .fill(t.bg)
            .border(1.0, t.border)
            .border_layout()
            .box_shadow(shadow::kit_xs())
            .hover(t.kit.secondary_hover)
            .cursor(Cursor::Pointer)
            .focusable(true)
            .child(label(s, 13.0, c));
        if let Some(f) = self.on_toggle.clone() {
            e = e.on_press(move |s: &mut S| f(s));
        }
        e
    }

    /// The calendar surface (Roostr's `CalendarPicker`): 252 wide, panel, border, menu radius,
    /// 10 padding, its pop shadow, and the Today / Clear footer.
    pub fn surface(self, t: &Theme) -> El {
        let key = self.key();
        let close = self.on_close.clone();
        let pick = self.on_pick.clone();
        let picked = move |s: &mut S, d: Option<NaiveDate>| {
            if let Some(f) = &close {
                f(s);
            }
            if let Some(f) = &pick {
                f(s, d);
            }
        };
        let picked = Rc::new(picked);
        let on_day = picked.clone();
        let calendar = self
            .calendar
            .on_select(move |s, date| on_day(s, date.start()));
        let today = calendar.state.today();
        let foot = |id: &str, name: &str, value: Option<NaiveDate>| {
            let picked = picked.clone();
            row()
                .key(format!("{key}-{id}"))
                .role("button")
                .label(name.to_owned())
                .px(6.0)
                .py(2.0)
                .radius(radius::SM)
                .hover_group()
                .hover(t.hover)
                .press(t.hover)
                .cursor(Cursor::Pointer)
                .focusable(true)
                .child(label(name, 12.0, t.muted).group_color(t.fg))
                .on_press(move |s: &mut S| picked(s, value))
        };
        col()
            .key(format!("{key}-surface"))
            .w(252.0)
            .fill(t.panel)
            .border(1.0, t.border)
            .border_layout()
            .radius(radius::MENU)
            .padding(10.0)
            .box_shadow(shadow::gpui(12.0, 32.0, 0.0, 0.45))
            .child(calendar.build(t))
            .child(
                row()
                    .justify_between()
                    .mt(8.0)
                    .pt(8.0)
                    .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
                    .border_layout()
                    .child(foot("today", "Today", Some(today)))
                    .child(foot("clear", "Clear", None)),
            )
    }

    /// The trigger in place; while open, the calendar below it (floating in `layer`, closed by
    /// an outside press or Escape, or inline under the trigger).
    pub fn build(self, t: &Theme, layer: &mut Vec<El>) -> El {
        let trigger = self.trigger(t);
        if !self.open {
            return trigger;
        }
        let key = self.key();
        let inline = self.inline;
        let close = self.on_close.clone();
        let surface = self.surface(t);
        if inline {
            return col().items_start().gap(6.0).child(trigger).child(surface);
        }
        let surface = overlay::anchored(surface, &key, Placement::BottomStart, 6.0);
        let surface = overlay::dismissable(
            surface,
            &key,
            move |s: &mut S| {
                if let Some(f) = &close {
                    f(s);
                }
            },
            layer,
        );
        layer.push(surface);
        trigger
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime};

    struct App {
        cal: CalendarState,
        picked: Vec<Date>,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let t = Theme::dark();
        col().padding(20.0).child(
            Calendar::new("cal", &s.cal, |s: &mut App| &mut s.cal)
                .on_select(|s, d| s.picked.push(d))
                .build(t),
        )
    }

    fn click(rt: &mut Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn month_navigation_crosses_the_year_and_a_day_selects() {
        let app = App {
            cal: CalendarState::with_today(ymd(2026, 12, 30)),
            picked: Vec::new(),
        };
        let mut rt = Runtime::new(
            app,
            view,
            (400.0, 400.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        click(&mut rt, "cal-next");
        assert_eq!(
            (rt.state.cal.current_year(), rt.state.cal.current_month()),
            (2027, 1)
        );
        click(&mut rt, "cal-prev");
        click(&mut rt, "cal-prev");
        assert_eq!(
            (rt.state.cal.current_year(), rt.state.cal.current_month()),
            (2026, 11)
        );
        click(&mut rt, "cal-2026-11-14");
        assert_eq!(rt.state.cal.date(), Date::Single(Some(ymd(2026, 11, 14))));
        assert_eq!(rt.state.picked, [Date::Single(Some(ymd(2026, 11, 14)))]);
        // The month picker behind the title.
        click(&mut rt, "cal-month");
        assert_eq!(rt.state.cal.view(), CalendarView::Month);
        click(&mut rt, "cal-m3");
        assert_eq!(
            (rt.state.cal.view(), rt.state.cal.current_month()),
            (CalendarView::Day, 3)
        );
    }

    #[test]
    fn weeks_start_monday_and_six_week_months_are_whole() {
        let c = CalendarState::with_today(ymd(2026, 8, 1));
        let weeks = c.weeks(Weekday::Mon);
        assert_eq!(weeks[0][0], ymd(2026, 7, 27));
        assert_eq!(weeks.len(), 6);
        assert!(weeks.iter().all(|w| w.len() == 7));
    }

    #[test]
    fn ranges_restart_and_complete() {
        let mut c = CalendarState::with_today(ymd(2026, 1, 1));
        c.set_date(Date::Range(None, None));
        assert!(!c.select_date(ymd(2026, 1, 10)));
        assert!(c.select_date(ymd(2026, 1, 12)));
        assert!(!c.select_date(ymd(2026, 1, 5)));
        assert_eq!(c.date(), Date::Range(Some(ymd(2026, 1, 5)), None));
    }
}

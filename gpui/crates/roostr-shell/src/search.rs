//! The search modal (`lib/components/SearchModal.svelte`): space-scoped
//! full-text search grouped by type (most matches first), type chips,
//! matching types, recents for an empty query, and `Create object "<q>"`.
//! At a phone width it is a bottom sheet (its @media 720 rules) that a
//! downward pull drags away. The query is answered by the store
//! (`roostr_core::api::query`), not filtered locally.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use roostr_core::create::{FALLBACK_TYPE, create_typed, default_type_key};
use roostr_core::lite_ui::{self, Notice};
use roostr_core::nav::{Route, navigate};
use roostr_core::objects::{kind_name, object_icon, type_icon};
use roostr_core::types::{QueryRow, SYSTEM_TYPE_KEYS, field_str};
use roostr_core::{State, StoreEvent, TypeDef};
use roostr_rt::Task;
use roostr_ui::color::rgba;
use roostr_ui::overlay;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Input, InputState, Theme, modal_frame, motion, outline_button, radius, ty};
use rune::{
    Color, Cursor, Cx, Drag, Effect, El, GesturePhase, Key, KeyEvent, Mods, Phase, ScrollPos,
    ScrollSpec, Span, Spring, SpringPair, Transition, Tx, Vec2, WheelEvent, col, el, rich, row,
    text,
};
use serde_json::{Value as Json, json};

use crate::shell::{icon_text, space_id};

const RECENT_LIMIT: u64 = 20;
const SEARCH_LIMIT: u64 = 200;
/// A typed query waits this long for the next key before it asks the store.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Past this pull the released sheet closes.
const SHEET_CLOSE: f32 = 110.;
/// A pull starts dragging the sheet past this much travel.
const SHEET_SLOP: f32 = 10.;
/// The sheet rises in when opened (Svelte popped it): as long as an iOS sheet's
/// decelerating 260 ms rise, settling without overshoot.
const SHEET_ENTER: Spring = Spring::new(0.26, 0.0);

/// The modal's state (a slot), while open and between openings.
#[derive(Default)]
struct Search {
    open: bool,
    input: InputState,
    /// The field's text as last searched.
    query: String,
    /// The highlighted row in keyboard order ([`ordered`], then the create row).
    selected: usize,
    results: Vec<QueryRow>,
    searching: bool,
    /// Picked type chips (type keys).
    chips: Vec<String>,
    /// The space's Default type (what `Create object` makes without one chip).
    space_default: String,
    /// The latest search: older answers are dropped.
    seq: u64,
    /// The search in flight (dropped, so cancelled, by the next one or a close).
    pending: Option<Task<()>>,
    /// The Default type's fetch.
    fetch: Option<Task<()>>,
    /// The chip row's scroll: [offset x, max x], as it last scrolled.
    chips_scroll: Rc<Cell<[f32; 2]>>,
    /// The results' vertical scroll offset, as they last scrolled.
    results_y: Rc<Cell<f32>>,
    /// The sheet drag in flight (`armed`/`dragging`), phone only.
    sheet_drag: Option<SheetDrag>,
    /// How far the sheet is pulled down (`sheetY`).
    sheet_y: f32,
    /// The store listener is installed (once per state).
    listening: bool,
}

/// The sheet's drag-to-close gesture, from the pan's accumulated movement.
#[derive(Default)]
struct SheetDrag {
    dx: f32,
    dy: f32,
    dragging: bool,
}

struct Group {
    key: String,
    name: String,
    rows: Vec<QueryRow>,
}

/// Open the search modal (nothing when it is open): an empty query lists the space's
/// recently edited objects.
pub(crate) fn open(s: &mut State) {
    if is_open(s) {
        return;
    }
    if !s.slot::<Search>().listening {
        s.slot::<Search>().listening = true;
        s.on_store(store_event);
    }
    let channel = space_id(s);
    let fetch = default_type_key(&channel, s);
    let task = s.rt.spawn(async move |cx| {
        let key = fetch.await;
        cx.update(|s| {
            if space_id(s) == channel {
                s.slot::<Search>().space_default = key;
            }
        });
    });
    let m = s.slot::<Search>();
    m.open = true;
    m.input = InputState::new();
    m.input.focus();
    m.query.clear();
    m.selected = 0;
    m.results.clear();
    m.searching = false;
    m.chips.clear();
    m.space_default = FALLBACK_TYPE.into();
    m.fetch = Some(task);
    m.chips_scroll.set([0.0, 0.0]);
    m.results_y.set(0.0);
    m.sheet_drag = None;
    m.sheet_y = 0.0;
    search(s);
}

fn is_open(s: &State) -> bool {
    s.get::<Search>().is_some_and(|m| m.open)
}

fn close(s: &mut State) {
    let m = s.slot::<Search>();
    m.open = false;
    m.pending = None;
    m.fetch = None;
    m.sheet_drag = None;
}

/// Lite: results come from what this device holds; once the space finished loading (a
/// refresh), search again over all of it.
fn store_event(s: &mut State, event: &StoreEvent) {
    if is_open(s)
        && matches!(event, StoreEvent::Refreshed)
        && roostr_core::lite::status(s).is_some()
    {
        search(s);
    }
}

/// Space scope (+ the system-type exclusion, + picked chips).
fn scope_filters(channel: &str, is_default: bool, chips: &[String]) -> Vec<Json> {
    let scope = if is_default {
        json!({ "operator": "or", "nested": [
            { "key": "channel", "condition": "equal", "value": channel },
            { "key": "channel", "condition": "empty" },
        ] })
    } else {
        json!({ "key": "channel", "condition": "equal", "value": channel })
    };
    let mut filters = vec![
        scope,
        json!({ "key": "type", "condition": "notIn", "value": SYSTEM_TYPE_KEYS }),
    ];
    if !chips.is_empty() {
        filters.push(json!({ "key": "type", "condition": "in", "value": chips }));
    }
    filters
}

/// Ask the store: recents (picked chips filter them) for an empty query, else a
/// full-text search after [`DEBOUNCE`].
fn search(s: &mut State) {
    let channel = space_id(s);
    let is_default = s.store.channels.first().is_some_and(|c| c.id == channel);
    let m = s.slot::<Search>();
    let q = m.query.trim().to_string();
    let chips = if q.is_empty() {
        m.chips.clone()
    } else {
        Vec::new()
    };
    m.seq += 1;
    let mine = m.seq;
    let filters = scope_filters(&channel, is_default, &chips);
    let body = if q.is_empty() {
        json!({ "filters": filters, "sorts": [{ "key": "updatedAt", "type": "desc" }], "limit": RECENT_LIMIT })
    } else {
        json!({ "textQuery": q, "filters": filters, "limit": SEARCH_LIMIT })
    };
    let delay = if q.is_empty() {
        Duration::ZERO
    } else {
        DEBOUNCE
    };
    let task = s.rt.spawn(async move |cx| {
        if !delay.is_zero() {
            cx.timer(delay).await;
        }
        let query = cx.update(|s| {
            s.slot::<Search>().searching = true;
            roostr_core::api::query(body, s)
        });
        let result = query.await;
        cx.update(|s| {
            let m = s.slot::<Search>();
            if m.seq != mine {
                return;
            }
            if let Ok(res) = result {
                m.results = res.records;
                m.selected = 0;
            }
            m.searching = false;
        });
    });
    s.slot::<Search>().pending = Some(task);
}

fn type_name(s: &State, key: &str) -> String {
    s.store
        .types
        .iter()
        .find(|t| t.key == key)
        .map(|t| t.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| kind_name(key))
}

/// The rows shown: with a query, the picked chips filter the results here.
fn shown(m: &Search) -> Vec<QueryRow> {
    if !m.query.trim().is_empty() && !m.chips.is_empty() {
        m.results
            .iter()
            .filter(|r| m.chips.contains(&r.type_key))
            .cloned()
            .collect()
    } else {
        m.results.clone()
    }
}

/// How well a title answers the (lowercased) query, as the store ranks rows
/// (core/query.odin `text_rank`): 0 the whole title, 1 its start, 2 the start
/// of a word in it, 3 anywhere in it, 4 not in it.
fn title_rank(name: Option<&str>, q: &str) -> u8 {
    let name = name.unwrap_or("").trim().to_lowercase();
    if q.is_empty() || !name.contains(q) {
        return 4;
    }
    if name == q {
        return 0;
    }
    if name.starts_with(q) {
        return 1;
    }
    let bytes = name.as_bytes();
    let word_start = name
        .match_indices(q)
        .any(|(at, _)| at == 0 || !bytes[at - 1].is_ascii_alphanumeric());
    if word_start { 2 } else { 3 }
}

/// Recents in one "Recently edited" group; results grouped by type.
fn groups(s: &State, m: &Search) -> Vec<Group> {
    let shown = shown(m);
    if m.query.trim().is_empty() {
        return if shown.is_empty() {
            vec![]
        } else {
            vec![Group {
                key: String::new(),
                name: "Recently edited".into(),
                rows: shown,
            }]
        };
    }
    let mut groups: Vec<Group> = Vec::new();
    for r in shown {
        match groups.iter_mut().find(|g| g.key == r.type_key) {
            Some(g) => g.rows.push(r),
            None => groups.push(Group {
                key: r.type_key.clone(),
                name: type_name(s, &r.type_key),
                rows: vec![r],
            }),
        }
    }
    // The type holding the best title match leads (the store ranks rows the same
    // way: core/query.odin `text_rank`), then most matches.
    let q = m.query.trim().to_lowercase();
    let best = |g: &Group| {
        g.rows
            .iter()
            .map(|r| title_rank(r.name.as_deref(), &q))
            .min()
            .unwrap_or(4)
    };
    groups.sort_by(|a, b| {
        best(a)
            .cmp(&best(b))
            .then_with(|| b.rows.len().cmp(&a.rows.len()))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    groups
}

/// The space's types whose name (or key) holds the query, at most five; none while
/// chips are picked.
fn type_matches(s: &State, m: &Search) -> Vec<TypeDef> {
    let q = m.query.trim().to_lowercase();
    if q.is_empty() || !m.chips.is_empty() {
        return vec![];
    }
    let channel = space_id(s);
    s.store
        .types
        .iter()
        .filter(|t| {
            t.space == channel
                && if t.name.is_empty() { &t.key } else { &t.name }
                    .to_lowercase()
                    .contains(&q)
        })
        .take(5)
        .cloned()
        .collect()
}

/// Keyboard order: matching types, then each group's rows (ids).
fn ordered(s: &State, m: &Search) -> Vec<String> {
    let mut out: Vec<String> = type_matches(s, m).into_iter().map(|t| t.id).collect();
    out.extend(
        groups(s, m)
            .into_iter()
            .flat_map(|g| g.rows.into_iter().map(|r| r.id)),
    );
    out
}

/// What `Create object` makes: the one picked chip's type, else the space's Default.
fn create_type(m: &Search) -> String {
    if m.chips.len() == 1 {
        m.chips[0].clone()
    } else {
        m.space_default.clone()
    }
}

fn open_object(s: &mut State, id: String) {
    close(s);
    navigate(Route::Object(id), s);
}

fn create_from_query(s: &mut State) {
    let m = s.slot::<Search>();
    let name = m.query.trim().to_string();
    if name.is_empty() {
        return;
    }
    let kind = create_type(m);
    let channel = space_id(s);
    close(s);
    let task = create_typed(&kind, &channel, &name, s);
    task.detach_and_log_err(&s.rt);
}

/// Enter: open the highlighted row, or create from the query on the create row.
fn open_selected(s: &mut State) {
    let Some(m) = s.get::<Search>() else {
        return;
    };
    let ordered = ordered(s, m);
    let selected = m.selected;
    let has_query = !m.query.trim().is_empty();
    if let Some(id) = ordered.get(selected).cloned() {
        open_object(s, id);
    } else if has_query && selected == ordered.len() {
        create_from_query(s);
    }
}

/// The row keyed for keyboard position `i` (to scroll it into view).
fn row_key(s: &State, m: &Search, i: usize) -> String {
    let types = type_matches(s, m);
    if let Some(t) = types.get(i) {
        return format!("search-type:{}", t.id);
    }
    let rows = groups(s, m).into_iter().flat_map(|g| g.rows);
    match rows.into_iter().nth(i - types.len()) {
        Some(r) => format!("search-row:{}", r.id),
        None => "search-create".into(),
    }
}

/// ↑/↓: move the highlight (stopping at the ends) and keep it in view.
fn step(s: &mut State, down: bool, tx: &mut Tx) {
    let Some(m) = s.get::<Search>() else {
        return;
    };
    let count = ordered(s, m).len() + usize::from(!m.query.trim().is_empty());
    let selected = if down {
        (m.selected + 1).min(count.saturating_sub(1))
    } else {
        m.selected.saturating_sub(1)
    };
    let key = row_key(s, m, selected);
    s.slot::<Search>().selected = selected;
    tx.color_motion(Spring::NONE);
    tx.scroll_into_view(key);
}

/// The pointer over row `i` highlights it.
fn hover_row(s: &mut State, i: usize, tx: &mut Tx) {
    let m = s.slot::<Search>();
    if m.selected != i {
        m.selected = i;
        tx.color_motion(Spring::NONE);
    }
}

/// A row clicked: open it (or create, for the create row).
fn press_row(s: &mut State, i: usize) {
    s.slot::<Search>().selected = i;
    open_selected(s);
}

fn toggle_chip(s: &mut State, key: &str) {
    let m = s.slot::<Search>();
    if let Some(ix) = m.chips.iter().position(|k| k == key) {
        m.chips.remove(ix);
    } else {
        m.chips.push(key.to_string());
    }
    if m.query.trim().is_empty() {
        search(s);
    }
}

/// The field's text changed: search for it.
fn query_changed(s: &mut State, _: &mut Tx) {
    let m = s.slot::<Search>();
    let value = m.input.value().to_string();
    if value != m.query {
        m.query = value;
        search(s);
    }
}

/// The phone's × in the field: empty it and search again (recents).
fn clear_query(s: &mut State) {
    let m = s.slot::<Search>();
    m.input.set_value("");
    m.input.focus();
    m.query.clear();
    search(s);
}

/// ↑ ↓ ↵ esc, wherever the focus is inside the modal (the field first).
fn modal_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.mods != Mods::default() {
        return false;
    }
    match ev.key {
        Key::Down => step(s, true, tx),
        Key::Up => step(s, false, tx),
        Key::Enter => open_selected(s),
        Key::Escape => close(s),
        _ => return false,
    }
    true
}

fn search_input(s: &mut State) -> &mut InputState {
    &mut s.slot::<Search>().input
}

/// A pan over the sheet: a downward pull while the results sit at their top drags it
/// (dropping the keyboard); released past [`SHEET_CLOSE`] it closes, else it snaps back.
/// Sideways (the chips) and upward pans are left to scroll. A pan locks to whoever takes
/// its first move, so its first move decides. True when the pan is the sheet's.
fn sheet_wheel(s: &mut State, e: WheelEvent, tx: &mut Tx) -> bool {
    let m = s.slot::<Search>();
    match e.phase {
        GesturePhase::Began => {
            m.sheet_drag = Some(SheetDrag::default());
            return false;
        }
        GesturePhase::Ended => {
            let Some(drag) = m.sheet_drag.take() else {
                return false;
            };
            if !drag.dragging {
                return false;
            }
            if m.sheet_y > SHEET_CLOSE {
                close(s);
            } else {
                m.sheet_y = 0.;
            }
            return true;
        }
        GesturePhase::Moved => {}
    }
    let at_top = m.results_y.get() <= 0.5;
    let Some(drag) = &mut m.sheet_drag else {
        return false;
    };
    let first = drag.dx == 0. && drag.dy == 0.;
    drag.dx += e.dx;
    drag.dy += e.dy;
    if first && (drag.dx.abs() > drag.dy.abs() || drag.dy < 0. || !at_top) {
        // Sideways: the chips'; upward, or the results scrolled: let it scroll.
        m.sheet_drag = None;
        return false;
    }
    if !drag.dragging {
        if drag.dy <= SHEET_SLOP {
            return true;
        }
        drag.dragging = true;
        tx.blur();
    }
    m.sheet_y = drag.dy.max(0.);
    tx.instant();
    true
}

/// A finger (or the pointer) dragging the sheet's panel, where nothing inside takes the
/// press: past 10 pt downward while the results sit at their top it pulls the sheet 1:1
/// (dropping the keyboard); released past [`SHEET_CLOSE`] it closes, else it snaps back.
/// Sideways and upward drags leave it alone.
fn sheet_drag(s: &mut State, d: &mut Drag, tx: &mut Tx) {
    let m = s.slot::<Search>();
    match d.phase {
        Phase::Down => m.sheet_drag = Some(SheetDrag::default()),
        Phase::Move => {
            let at_top = m.results_y.get() <= 0.5;
            let Some(drag) = &mut m.sheet_drag else {
                return;
            };
            drag.dx = d.dx;
            drag.dy = d.dy;
            if !drag.dragging {
                if (drag.dx.abs() > drag.dy.abs() && drag.dx.abs() > SHEET_SLOP)
                    || drag.dy < -SHEET_SLOP
                {
                    m.sheet_drag = None;
                    return;
                }
                if drag.dy <= SHEET_SLOP || !at_top {
                    return;
                }
                drag.dragging = true;
                tx.blur();
            }
            m.sheet_y = drag.dy.max(0.);
            tx.instant();
        }
        Phase::Up => {
            let Some(drag) = m.sheet_drag.take() else {
                return;
            };
            if !drag.dragging {
                return;
            }
            if m.sheet_y > SHEET_CLOSE && !d.cancelled() {
                close(s);
            } else {
                m.sheet_y = 0.;
            }
        }
    }
}

/// A plain (vertical) mouse wheel over the desktop chip row scrolls it sideways (nothing
/// around the chips scrolls); a mostly sideways swipe scrolls it natively.
fn chips_wheel(s: &mut State, e: WheelEvent, tx: &mut Tx) -> bool {
    let cell = s.slot::<Search>().chips_scroll.clone();
    let [offset, max] = cell.get();
    if e.dy.abs() <= e.dx.abs() || max <= 0. {
        return false;
    }
    let x = (offset - e.dy).clamp(0., max);
    if x != offset {
        cell.set([x, max]);
        tx.scroll_to_x("search-chips", x);
    }
    true
}

/// Case-insensitive first match of `q` in `text` (`highlight`'s split).
fn find_ci(text: &str, q: &str) -> Option<Range<usize>> {
    if q.is_empty() {
        return None;
    }
    let needle: Vec<char> = q.chars().flat_map(char::to_lowercase).collect();
    for (start, _) in text.char_indices() {
        let mut it = text[start..]
            .char_indices()
            .flat_map(|(i, c)| c.to_lowercase().map(move |l| (i, c, l)));
        let mut end = start;
        let mut ok = true;
        for want in &needle {
            match it.next() {
                Some((i, c, l)) if l == *want => end = start + i + c.len_utf8(),
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Some(start..end);
        }
    }
    None
}

/// `text-transform: capitalize`.
fn capitalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = true;
    for c in text.chars() {
        if start {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        start = c.is_whitespace();
    }
    out
}

/// `<mark>`: accent + semibold (white on the highlighted row); on a phone a translucent
/// blue wash instead.
fn highlighted(t: &Theme, content: String, q: &str, selected: bool, mobile: bool) -> El {
    let spans = match find_ci(&content, q) {
        Some(r) => {
            let span = Span::new(r.start, r.end).weight(ty::SEMIBOLD);
            vec![if mobile {
                span.bg(rgba(0x508cff59), 0.0)
            } else {
                span.color(if selected { t.white } else { t.accent })
            }]
        }
        None => Vec::new(),
    };
    rich(content, spans)
}

/// The secondary text colour (snippets, kinds): white at 72% on the highlighted row.
fn secondary(t: &Theme, selected: bool) -> Color {
    if selected {
        Color::WHITE.alpha(0.72)
    } else {
        t.muted
    }
}

/// The row's icon: 18 wide, centred, accent (white 72% on the highlighted row).
fn row_icon(s: &State, t: &Theme, icon: &str, selected: bool) -> El {
    let color = if selected {
        Color::WHITE.alpha(0.72)
    } else {
        t.accent
    };
    row()
        .w(18.0)
        .justify_center()
        .center_y()
        .child(icon_text(s, icon, 16.0, ty::ui(13.33)).color(color))
}

/// `.kind`: the row's type, at the end; on a phone on its own line under the name,
/// indented past the icon.
fn kind(t: &Theme, label: &str, selected: bool, mobile: bool) -> El {
    let color = secondary(t, selected);
    if mobile {
        row()
            .w_full()
            .pl(42.0)
            .child(text(capitalize(label)).font(ty::ui(12.0)).color(color))
    } else {
        text(label.to_string()).font(ty::ui(11.0)).color(color)
    }
}

/// A result row: the Roostr row (`selected` = accent fill), the pointer highlighting
/// it (keyboard position `i`), a press opening it.
fn result_row(t: &Theme, key: String, i: usize, selected: bool, mobile: bool) -> El {
    let r = row()
        .key(key)
        .role("option")
        .checked(selected)
        .gap(12.0)
        .min_w(0.0)
        .radius(radius::MD)
        .cursor(Cursor::Pointer)
        .fill(if selected { t.accent } else { t.panel })
        .on_hover_change(move |s: &mut State, inside: bool, tx: &mut Tx| {
            if inside {
                hover_row(s, i, tx);
            }
        })
        .on_press(move |s: &mut State| press_row(s, i));
    if mobile {
        // Stacked: the kind wraps under the name.
        r.flex_wrap().px(14.0).py(12.0)
    } else {
        r.center_y().px(10.0).py(8.0)
    }
}

/// The name (and snippet / note) column of a row.
fn row_text(name: El, sub: Option<El>) -> El {
    let mut c = col().gap(1.0).grow(1.0).shrink().min_w(0.0).child(name);
    if let Some(sub) = sub {
        c = c.child(sub);
    }
    c
}

/// A group's heading: the Roostr section header (11 bold uppercase) with its count,
/// edge to edge; a rule above every one but the first.
fn section_header(t: &Theme, first: bool, label: &str, count: Option<usize>) -> El {
    let mut h = row()
        .center_y()
        .gap(8.0)
        .pt(8.0)
        .px(16.0)
        .pb(6.0)
        .mb(4.0)
        .fill(t.panel)
        .child(
            text(label.to_uppercase())
                .font(ty::ui(11.0).bold())
                .color(t.fg),
        );
    if !first {
        h = h.mt(10.0).border_sides([1.0, 0.0, 0.0, 0.0], t.border);
    }
    if let Some(n) = count {
        h = h.child(
            row()
                .px(7.0)
                .py(1.0)
                .radius(999.0)
                .fill(Color::WHITE.alpha(0.08))
                .child(
                    text(n.to_string())
                        .font(ty::ui(10.5).semibold())
                        .color(t.muted),
                ),
        );
    }
    h
}

/// A lite notice: its text and its action, 12 pt muted.
fn notice_row(t: &Theme, notice: Notice) -> El {
    let mut out = row()
        .key("search-notice")
        .flex_wrap()
        .center_y()
        .gap(10.0)
        .py(6.0)
        .child(text(notice.text).font(ty::ui(12.0)).color(t.muted));
    if let Some(action) = notice.action {
        let label = action.label();
        out = out.child(
            outline_button("lite-load-search", label)
                .on_press(move |s: &mut State| action.run(s))
                .build(t),
        );
    }
    out
}

/// The search modal (desktop) or sheet (phone) while open.
pub(crate) fn overlay(s: &State, _cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) {
    let Some(m) = s.get::<Search>().filter(|m| m.open) else {
        return;
    };
    let mobile = crate::is_mobile(s);
    let (vw, vh) = (crate::window_width(s), crate::window_height(s));
    // The dim fades in; a press on it closes.
    layer.push(
        el().key("search-overlay")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(t.overlay)
            .transition(motion::appear(0.0)),
    );
    let frame = frame(s, m, t, mobile, vw, vh);
    let frame = overlay::dismissable(frame, "search", close, layer);
    let host = col()
        .key("search-host")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .items_center();
    let host = if mobile {
        // The sheet rises in from below the screen, follows a pull 1:1 and settles back
        // from a short one (`transition: transform 0.18s ease`).
        let enter = Transition {
            insertion: Effect {
                offset: Vec2::new(0.0, (vh - 56.0).max(0.0)),
                ..Effect::IDENTITY
            },
            removal: Effect::IDENTITY,
            springs: SpringPair::new(SHEET_ENTER, Spring::NONE),
            ..Transition::identity()
        };
        host.justify_end().child(
            col()
                .key("search-sheet")
                .w_full()
                .items_stretch()
                .offset(0.0, m.sheet_y)
                .offset_motion(motion::SHEET_SNAP.anim())
                .transition(enter)
                .on_wheel(sheet_wheel)
                .child(frame),
        )
    } else {
        // 12% of the window from the top.
        host.child(el().grow(12.0).h(0.0)).child(
            col()
                .grow(88.0)
                .h(0.0)
                .items_center()
                .child(frame.transition(motion::appear(8.0))),
        )
    };
    layer.push(host);
}

/// The panel: (phone: the grab handle), the field row, the chips, the lite notice, the
/// results and (desktop) the key hints.
fn frame(s: &State, m: &Search, t: &Theme, mobile: bool, vw: f32, vh: f32) -> El {
    let channel = space_id(s);
    let active = s.nav.space().to_string();
    let q = m.query.trim().to_string();
    let space_name = s
        .store
        .space(&channel)
        .map(|s| s.name.clone())
        .unwrap_or_default();

    let width = if mobile {
        vw
    } else {
        (vw - 48.0).min(620.0) + 2.0
    };
    let mut frame = modal_frame(t, width)
        .key("search-modal")
        .role("dialog")
        .hit()
        .on_key(modal_key)
        .instant_changes();
    if mobile {
        // Leaves the top strip free: a full-height sheet would park its grab handle in
        // the system-gesture edge.
        let corners = [18.0, 18.0, 0.0, 0.0];
        frame = frame
            .on_drag(sheet_drag)
            .w_full()
            .h((vh - 56.0).max(0.0))
            .corners(corners)
            .clip_corners(corners)
            // The grab handle.
            .child(
                row()
                    .justify_center()
                    .pt(12.0)
                    .pb(8.0)
                    .child(el().w(40.0).h(5.0).radius(3.0).fill(t.border)),
            );
    }
    frame = frame.child(field_row(t, m, mobile, space_name));
    if let Some(chips) = chip_row(s, m, t, mobile, &q, &active) {
        frame = frame.child(chips);
    }
    // Lite: results are what this device holds; offer the whole space.
    if let Some(notice) = lite_ui::space_notice(&channel, s) {
        frame = frame.child(row().px(14.0).child(notice_row(t, notice)));
    }
    let results = col()
        .key("search-results")
        .items_stretch()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .on_scroll_offset({
            let y = m.results_y.clone();
            move |at: ScrollPos| y.set(at.offset[1])
        })
        .child(result_list(s, m, t, mobile, &q, &active));
    let results = if mobile {
        // The sheet is fixed-height: results fill the rest.
        results.grow(1.0).min_h(0.0).on_wheel(sheet_wheel)
    } else if vh > 0.0 {
        results.max_h(vh * 0.46)
    } else {
        results
    };
    frame = frame.child(results);
    if !mobile {
        frame = frame.child(
            row()
                .gap(16.0)
                .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
                .px(14.0)
                .py(8.0)
                .children(
                    ["↑↓ navigate", "↵ open", "esc close"]
                        .map(|hint| text(hint).font(ty::ui(11.0)).color(t.muted)),
                ),
        );
    }
    frame
}

/// The field: on a desktop after the space's name tag, light 19 pt; on a phone a ⌕ and a
/// rounded 16 pt field with a clear button while it holds text.
fn field_row(t: &Theme, m: &Search, mobile: bool, space_name: String) -> El {
    let mut r = row().center_y().gap(10.0).px(14.0);
    let input = Input::new("search-input", &m.input)
        .placeholder("Search objects and content…")
        .appearance(false)
        .px(0.0)
        .py(0.0)
        .on_edit(search_input)
        .on_change(query_changed)
        .on_key(modal_key);
    if mobile {
        r = r
            .py(8.0)
            .child(text("⌕").font(ty::ui(16.0)).color(t.muted))
            .child(
                row()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .fill(t.hover)
                    .border(1.0, t.border)
                    .radius(12.0)
                    .px(12.0)
                    .py(10.0)
                    .child(
                        input
                            .text_size(16.0)
                            .line_height(16.0 * 1.2)
                            .h(16.0 * 1.2)
                            .build(t),
                    ),
            );
        if !m.query.is_empty() {
            r = r.child(
                row()
                    .key("search-clear")
                    .role("button")
                    .label("Clear")
                    .size(28.0, 28.0)
                    .ml(-36.0)
                    .center()
                    .radius(14.0)
                    .fill(t.hover)
                    .cursor(Cursor::Pointer)
                    .on_press(clear_query)
                    .child(text("×").font(ty::ui(15.0)).color(t.muted)),
            );
        }
    } else {
        r = r
            .py(12.0)
            .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
            .child(
                row()
                    .key("search-space")
                    .border(1.0, t.border)
                    .radius(radius::SM)
                    .px(8.0)
                    .py(2.0)
                    .child(text(space_name).font(ty::ui(12.0)).color(t.accent)),
            )
            .child(
                row().grow(1.0).shrink().min_w(0.0).child(
                    input
                        .text_size(19.0)
                        .weight(ty::LIGHT)
                        .line_height(19.0 * 1.2)
                        .build(t),
                ),
            );
    }
    r
}

/// Chips: every space type while the query is empty; else the result types with counts,
/// most first. `None` when there are none.
fn chip_row(s: &State, m: &Search, t: &Theme, mobile: bool, q: &str, active: &str) -> Option<El> {
    let channel = space_id(s);
    let chips: Vec<(String, String, Option<usize>)> = if q.is_empty() {
        s.store
            .types
            .iter()
            .filter(|t| t.space == channel)
            .map(|t| {
                (
                    t.key.clone(),
                    format!(
                        "{} {}",
                        t.icon,
                        if t.name.is_empty() { &t.key } else { &t.name }
                    ),
                    None,
                )
            })
            .collect()
    } else {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for r in &m.results {
            match counts.iter_mut().find(|(k, _)| *k == r.type_key) {
                Some((_, n)) => *n += 1,
                None => counts.push((r.type_key.clone(), 1)),
            }
        }
        let mut all: Vec<(String, String, usize)> = counts
            .into_iter()
            .map(|(k, n)| {
                let name = type_name(s, &k);
                (k, name, n)
            })
            .collect();
        all.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });
        all.into_iter()
            .map(|(k, name, n)| {
                let label = format!("{} {}", type_icon(&s.store, &k, active), name);
                (k, label, Some(n))
            })
            .collect()
    };
    if chips.is_empty() {
        return None;
    }
    let mut content = row().px(14.0);
    content = if mobile {
        content.gap(8.0).pt(4.0).pb(10.0)
    } else {
        content.gap(6.0).py(8.0)
    };
    for (key, label, count) in chips {
        let on = m.chips.contains(&key);
        let (bg, fg) = if on {
            (t.accent, t.white)
        } else {
            (t.hover, t.fg)
        };
        let size = if mobile { 14.0 } else { 12.5 };
        let mut chip = row()
            .key(format!("chip-{key}"))
            .role("button")
            .label(label.clone())
            .checked(on)
            .gap(4.0)
            .center_y()
            .radius(999.0)
            .fill(bg)
            .cursor(Cursor::Pointer)
            .child(text(label).font(ty::ui(size)).color(fg));
        chip = if mobile {
            chip.px(14.0).py(7.0)
        } else {
            chip.px(11.0).py(4.0)
        };
        if let Some(n) = count {
            chip = chip.child(text(n.to_string()).font(ty::ui(size)).color(t.muted));
        }
        content = content.child(chip.on_press(move |s: &mut State| toggle_chip(s, &key)));
    }
    let scroll = m.chips_scroll.clone();
    let viewport = row()
        .key("search-chips")
        .w_full()
        .scroll_x()
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
        .on_scroll_offset(move |at: ScrollPos| scroll.set([at.offset[0], at.max[0]]))
        .child(content);
    // Desktop: a vertical wheel turns the row sideways. Phone: sideways swipes pan it,
    // a downward pull drags the sheet.
    Some(if mobile {
        viewport.on_wheel(sheet_wheel)
    } else {
        viewport.on_wheel(chips_wheel)
    })
}

/// The results: matching types, each group under its heading, then `Create object "<q>"`
/// (once the search settled); "Nothing here yet." when there is nothing at all.
fn result_list(s: &State, m: &Search, t: &Theme, mobile: bool, q: &str, active: &str) -> El {
    let types = type_matches(s, m);
    let groups = groups(s, m);
    let ordered_len = types.len() + groups.iter().map(|g| g.rows.len()).sum::<usize>();
    let can_create = !q.is_empty();
    let mut list = col().key("search-list").items_stretch().pb(6.0);
    let mut first = true;
    // Rows sit 6 in from the panel's sides; headings span it.
    let rows = || col().items_stretch().px(6.0);
    if !types.is_empty() {
        list = list.child(section_header(t, first, "Types", None));
        first = false;
        let mut section = rows();
        for (i, def) in types.iter().enumerate() {
            let selected = i == m.selected;
            let icon = if def.icon.is_empty() {
                type_icon(&s.store, &def.key, active)
            } else {
                def.icon.clone()
            };
            let name = if def.name.is_empty() {
                def.key.clone()
            } else {
                def.name.clone()
            };
            let ink = if selected { t.white } else { t.fg };
            section = section.child(
                result_row(t, format!("search-type:{}", def.id), i, selected, mobile)
                    .child(row_icon(s, t, &icon, selected))
                    .child(row_text(
                        text(name).font(ty::ui(14.0)).color(ink).truncate(),
                        Some(
                            text("Type · opens with its objects")
                                .font(ty::ui(12.0))
                                .color(secondary(t, selected))
                                .truncate(),
                        ),
                    ))
                    .child(kind(t, "Type", selected, mobile)),
            );
        }
        list = list.child(section);
    }
    let mut offset = types.len();
    for g in &groups {
        let label = if g.key.is_empty() {
            g.name.clone()
        } else {
            format!("{} {}", type_icon(&s.store, &g.key, active), g.name)
        };
        list = list.child(section_header(
            t,
            first,
            &label,
            (!q.is_empty()).then_some(g.rows.len()),
        ));
        first = false;
        let mut section = rows();
        for (j, r) in g.rows.iter().enumerate() {
            let i = offset + j;
            let selected = i == m.selected;
            let icon = object_icon(
                &s.store,
                field_str(&r.fields, "iconEmoji"),
                &r.type_key,
                active,
            );
            let name = r
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "Untitled".into());
            let ink = if selected { t.white } else { t.fg };
            let snippet = r.snippet.clone().filter(|s| !s.is_empty()).map(|snippet| {
                highlighted(t, snippet, q, selected, mobile)
                    .font(ty::ui(12.0))
                    .color(secondary(t, selected))
                    .truncate()
            });
            let label = format!(
                "{} {}",
                type_icon(&s.store, &r.type_key, active),
                type_name(s, &r.type_key)
            );
            section = section.child(
                result_row(t, format!("search-row:{}", r.id), i, selected, mobile)
                    .child(row_icon(s, t, &icon, selected))
                    .child(row_text(
                        highlighted(t, name, q, selected, mobile)
                            .font(ty::ui(14.0))
                            .color(ink)
                            .truncate(),
                        snippet,
                    ))
                    .child(kind(t, &label, selected, mobile)),
            );
        }
        list = list.child(section);
        offset += g.rows.len();
    }
    if can_create && !m.searching {
        let selected = m.selected == ordered_len;
        let create = create_type(m);
        let label = format!(
            "{} {}",
            type_icon(&s.store, &create, active),
            type_name(s, &create)
        );
        let ink = if selected { t.white } else { t.fg };
        let mut section = rows();
        if first {
            section = section.pt(6.0);
        }
        section =
            section.child(
                result_row(t, "search-create".into(), ordered_len, selected, mobile)
                    .child(row().w(18.0).justify_center().center_y().child(
                        text("＋").font(ty::ui(13.33)).color(if selected {
                            t.white
                        } else {
                            t.accent
                        }),
                    ))
                    .child(
                        row().grow(1.0).shrink().min_w(0.0).child(
                            text(format!("Create object \"{q}\""))
                                .font(ty::ui(14.0))
                                .color(ink)
                                .truncate(),
                        ),
                    )
                    .child(kind(t, &label, selected, mobile)),
            );
        list = list.child(section);
        first = false;
    }
    if first && ordered_len == 0 && !m.searching && !can_create {
        list = list.child(
            row()
                .key("search-empty")
                .px(18.0)
                .pt(22.0)
                .pb(16.0)
                .child(text("Nothing here yet.").font(ty::ui(13.0)).color(t.muted)),
        );
    }
    list
}

#[cfg(test)]
mod tests {
    use super::{capitalize, find_ci, title_rank};

    #[test]
    fn finds_case_insensitively() {
        assert_eq!(find_ci("Launch Plan", "plan"), Some(7..11));
        assert_eq!(find_ci("Über", "über"), Some(0..5));
        assert_eq!(find_ci("abc", "x"), None);
        assert_eq!(find_ci("abc", ""), None);
    }

    #[test]
    fn ranks_titles_like_the_store() {
        assert_eq!(title_rank(Some("Plan"), "plan"), 0);
        assert_eq!(title_rank(Some("Planning"), "plan"), 1);
        assert_eq!(title_rank(Some("Launch plan"), "plan"), 2);
        assert_eq!(title_rank(Some("Airplane"), "plan"), 3);
        assert_eq!(title_rank(Some("Notes"), "plan"), 4);
        assert_eq!(title_rank(None, "plan"), 4);
    }

    #[test]
    fn capitalizes_each_word() {
        assert_eq!(capitalize("📄 meeting notes"), "📄 Meeting Notes");
    }
}

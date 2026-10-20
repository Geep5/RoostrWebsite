//! The search modal (`lib/components/SearchModal.svelte`): space-scoped
//! full-text search grouped by type (fewest matches first), type chips,
//! matching types, recents for an empty query, and `Create object "<q>"`.
//! At a phone width it is a bottom sheet (its @media 720 rules) that a
//! downward pull drags away.

use std::ops::Range;
use std::time::Duration;

use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::create::{create_typed, default_type_key};
use roostr_core::nav::{Route, navigate};
use roostr_core::objects::{kind_name, object_icon, type_icon};
use roostr_core::store::Store;
use roostr_core::types::{QueryRow, SYSTEM_TYPE_KEYS, field_str};
use roostr_core::{Navigator, TypeDef, motion, theme, ui};
use serde_json::{Value as Json, json};

use crate::workspace::Workspace;

actions!(
    roostr_shell_search,
    [SelectPrev, SelectNext, OpenSelected, CloseSearch]
);

const CONTEXT: &str = "SearchModal";
const RECENT_LIMIT: u64 = 20;
const SEARCH_LIMIT: u64 = 200;

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(CONTEXT)),
        KeyBinding::new("escape", CloseSearch, Some(CONTEXT)),
    ]);
}

pub(crate) struct Close;

struct Group {
    key: String,
    name: String,
    rows: Vec<QueryRow>,
}

pub(crate) struct SearchModal {
    input: Entity<InputState>,
    focus: FocusHandle,
    query: String,
    selected: usize,
    results: Vec<QueryRow>,
    searching: bool,
    chips: Vec<String>,
    space_default: String,
    seq: u64,
    pending: Option<Task<()>>,
    scroll: ScrollHandle,
    /// The sheet drag in flight (`armed`/`dragging`), mobile only.
    sheet_drag: Option<SheetDrag>,
    /// How far the sheet is pulled down (`sheetY`).
    sheet_y: f32,
    /// The sheet's drop as shown: 1:1 with a pull, settling to 0 otherwise.
    sheet: motion::ValueState,
    /// How the sheet settles: rising in when opened ([`SHEET_ENTER`]), then
    /// snapping back from a short pull (`transition: transform 0.18s ease`).
    sheet_settle: motion::Tween,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Close> for SearchModal {}

/// The sheet's drag-to-close gesture, from the pan's accumulated movement.
#[derive(Default)]
struct SheetDrag {
    dx: f32,
    dy: f32,
    dragging: bool,
}

/// Past this pull the released sheet closes.
const SHEET_CLOSE: f32 = 110.;
/// The sheet rises in when opened (Svelte popped it): a decelerating tween
/// like an iOS sheet, not a spring, since no gesture let go of it.
const SHEET_ENTER: motion::Tween = motion::Tween::bezier(260, [0.2, 0.8, 0.2, 1.]);

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

/// `<mark>`: accent + semibold; on a phone a translucent blue wash instead.
fn highlighted(text: String, q: &str, selected: bool, mobile: bool) -> StyledText {
    let range = find_ci(&text, q);
    let styled = StyledText::new(text);
    match range {
        Some(range) => styled.with_highlights(vec![(
            range,
            if mobile {
                HighlightStyle {
                    background_color: Some(rgba(0x508cff59).into()),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..HighlightStyle::default()
                }
            } else {
                HighlightStyle {
                    color: Some(if selected {
                        theme::white()
                    } else {
                        theme::accent()
                    }),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..HighlightStyle::default()
                }
            },
        )]),
        None => styled,
    }
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

impl SearchModal {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search objects and content…"));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscriptions = vec![cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().to_string();
                if value != this.query {
                    this.query = value;
                    this.search(cx);
                }
            }
        })];
        let channel = Self::channel_id(cx);
        // A phone's sheet starts below the screen and rises in.
        let sheet_from = if roostr_core::layout::is_mobile(window) {
            f32::from(roostr_core::layout::viewport(window).height - px(56.))
        } else {
            0.
        };
        let fetch = default_type_key(&channel, cx);
        cx.spawn(async move |this, cx| {
            let key = fetch.await;
            let _ = this.update(cx, |this, cx| {
                if Self::channel_id(cx) == channel {
                    this.space_default = key;
                    cx.notify();
                }
            });
        })
        .detach();
        let mut this = Self {
            input,
            focus: cx.focus_handle(),
            query: String::new(),
            selected: 0,
            results: Vec::new(),
            searching: false,
            chips: Vec::new(),
            space_default: "note".into(),
            seq: 0,
            pending: None,
            scroll: ScrollHandle::new(),
            sheet_drag: None,
            sheet_y: 0.,
            sheet: motion::ValueState::new(f64::from(sheet_from)),
            sheet_settle: SHEET_ENTER,
            _subscriptions: subscriptions,
        };
        this.search(cx);
        this
    }

    fn channel_id(cx: &App) -> String {
        Workspace::space_id(cx)
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

    fn search(&mut self, cx: &mut Context<Self>) {
        let q = self.query.trim().to_string();
        let chips = if q.is_empty() {
            self.chips.clone()
        } else {
            Vec::new()
        };
        self.seq += 1;
        let mine = self.seq;
        let channel = Self::channel_id(cx);
        let is_default = Store::global(cx)
            .read(cx)
            .channels
            .first()
            .is_some_and(|c| c.id == channel);
        let filters = Self::scope_filters(&channel, is_default, &chips);
        let body = if q.is_empty() {
            json!({ "filters": filters, "sorts": [{ "key": "updatedAt", "type": "desc" }], "limit": RECENT_LIMIT })
        } else {
            json!({ "textQuery": q, "filters": filters, "limit": SEARCH_LIMIT })
        };
        let delay = if q.is_empty() {
            Duration::ZERO
        } else {
            Duration::from_millis(150)
        };
        self.pending = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let Ok(task) = this.update(cx, |this, cx| {
                this.searching = true;
                cx.notify();
                roostr_core::api::query(body, cx)
            }) else {
                return;
            };
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.seq != mine {
                    return;
                }
                if let Ok(res) = result {
                    this.results = res.records;
                    this.selected = 0;
                }
                this.searching = false;
                cx.notify();
            });
        }));
    }

    fn type_name(key: &str, cx: &App) -> String {
        Store::global(cx)
            .read(cx)
            .types
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| kind_name(key))
    }

    fn shown(&self) -> Vec<QueryRow> {
        if !self.query.trim().is_empty() && !self.chips.is_empty() {
            self.results
                .iter()
                .filter(|r| self.chips.contains(&r.type_key))
                .cloned()
                .collect()
        } else {
            self.results.clone()
        }
    }

    fn groups(&self, cx: &App) -> Vec<Group> {
        let shown = self.shown();
        if self.query.trim().is_empty() {
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
                    name: Self::type_name(&r.type_key, cx),
                    rows: vec![r],
                }),
            }
        }
        groups.sort_by(|a, b| {
            a.rows
                .len()
                .cmp(&b.rows.len())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        groups
    }

    fn type_matches(&self, cx: &App) -> Vec<TypeDef> {
        let q = self.query.trim().to_lowercase();
        if q.is_empty() || !self.chips.is_empty() {
            return vec![];
        }
        let channel = Self::channel_id(cx);
        Store::global(cx)
            .read(cx)
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
    fn ordered(&self, cx: &App) -> Vec<String> {
        let mut out: Vec<String> = self.type_matches(cx).into_iter().map(|t| t.id).collect();
        out.extend(
            self.groups(cx)
                .into_iter()
                .flat_map(|g| g.rows.into_iter().map(|r| r.id)),
        );
        out
    }

    fn create_type(&self) -> String {
        if self.chips.len() == 1 {
            self.chips[0].clone()
        } else {
            self.space_default.clone()
        }
    }

    fn row_count(&self, cx: &App) -> usize {
        self.ordered(cx).len() + usize::from(!self.query.trim().is_empty())
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(Close);
    }

    fn open(&mut self, id: String, cx: &mut Context<Self>) {
        self.close(cx);
        navigate(Route::Object(id), cx);
    }

    fn create_from_query(&mut self, cx: &mut Context<Self>) {
        let name = self.query.trim().to_string();
        if name.is_empty() {
            return;
        }
        let kind = self.create_type();
        let channel = Self::channel_id(cx);
        self.close(cx);
        create_typed(&kind, &channel, &name, cx).detach_and_log_err(cx);
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let ordered = self.ordered(cx);
        if let Some(id) = ordered.get(self.selected).cloned() {
            self.open(id, cx);
        } else if !self.query.trim().is_empty() && self.selected == ordered.len() {
            self.create_from_query(cx);
        }
    }

    fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let count = self.row_count(cx);
        self.selected = if down {
            (self.selected + 1).min(count.saturating_sub(1))
        } else {
            self.selected.saturating_sub(1)
        };
        cx.notify();
    }

    fn toggle_chip(&mut self, key: String, cx: &mut Context<Self>) {
        if let Some(ix) = self.chips.iter().position(|k| *k == key) {
            self.chips.remove(ix);
        } else {
            self.chips.push(key);
        }
        if self.query.trim().is_empty() {
            self.search(cx);
        }
        cx.notify();
    }

    /// A pan over the sheet: a downward pull while the results sit at their
    /// top drags it (dropping the keyboard); released past
    /// [`SHEET_CLOSE`] it closes, else it snaps back. Sideways (the chips)
    /// and upward pans are left to scroll. True when the pan is the sheet's.
    fn sheet_pan(
        &mut self,
        phase: TouchPhase,
        delta: Point<Pixels>,
        inside: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match phase {
            TouchPhase::Started => {
                if !inside {
                    self.sheet_drag = None;
                    return false;
                }
                self.sheet_drag = Some(SheetDrag::default());
            }
            TouchPhase::Moved => {}
            TouchPhase::Ended | TouchPhase::Cancelled => {
                let Some(drag) = self.sheet_drag.take() else {
                    return false;
                };
                if !drag.dragging {
                    return false;
                }
                if self.sheet_y > SHEET_CLOSE {
                    self.close(cx);
                } else {
                    self.sheet_settle = motion::SHEET_SNAP;
                    self.sheet_y = 0.;
                    cx.notify();
                }
                return true;
            }
        }
        let at_top = self.scroll.offset().y >= px(0.);
        let Some(drag) = &mut self.sheet_drag else {
            return false;
        };
        drag.dx += f32::from(delta.x);
        drag.dy += f32::from(delta.y);
        if !drag.dragging {
            if drag.dx.abs() > drag.dy.abs() && drag.dx.abs() > 10. {
                // Horizontal: the chips', not ours.
                self.sheet_drag = None;
                return false;
            }
            if drag.dy < -10. {
                // Upward: let it scroll.
                self.sheet_drag = None;
                return false;
            }
            if drag.dy > 10. && at_top {
                drag.dragging = true;
                window.blur(cx);
            } else {
                return false;
            }
        }
        self.sheet_y = drag.dy.max(0.);
        cx.notify();
        true
    }

    /// The sheet's current drop: the pull, else settling to 0.
    fn sheet_offset(&mut self, window: &mut Window, cx: &App) -> f32 {
        let (target, transition) = if self.sheet_drag.as_ref().is_some_and(|d| d.dragging) {
            (self.sheet_y, motion::Transition::Instant)
        } else {
            (0., self.sheet_settle.transition())
        };
        self.sheet
            .frame(f64::from(target), transition, window, cx)
            .value() as f32
    }

    fn row(id: ElementId, selected: bool, mobile: bool) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .gap(px(12.))
            .w_full()
            .map(|d| {
                if mobile {
                    // Stacked: the kind wraps under the name.
                    d.items_start().flex_wrap().px(px(14.)).py(px(12.))
                } else {
                    d.items_center().px(px(10.)).py(px(8.))
                }
            })
            .rounded(theme::radius::md())
            .cursor_pointer()
            .text_color(if selected {
                theme::white()
            } else {
                theme::fg()
            })
            .when(selected, |d| d.bg(theme::accent()))
    }

    /// `.kind`: the row's type, at the end; on a phone on its own line
    /// under the name, indented past the icon.
    fn kind(text: impl Into<String>, selected: bool, mobile: bool) -> Div {
        let text = text.into();
        div()
            .flex_none()
            .text_color(Self::secondary(selected))
            .map(|d| {
                if mobile {
                    d.w_full()
                        .pl(px(42.))
                        .text_size(px(12.))
                        .child(capitalize(&text))
                } else {
                    d.text_size(px(11.)).child(text)
                }
            })
    }

    fn secondary(selected: bool) -> Hsla {
        if selected {
            hsla(0., 0., 1., 0.72)
        } else {
            theme::muted()
        }
    }

    fn icon(icon: String, selected: bool) -> Div {
        div()
            .flex_none()
            .w(px(18.))
            .text_size(px(13.33))
            .flex()
            .justify_center()
            .text_color(if selected {
                hsla(0., 0., 1., 0.72)
            } else {
                theme::accent()
            })
            .child(roostr_core::ui::icon_text(&icon, px(16.)))
    }

    fn section_header(first: bool, label: String, count: Option<usize>) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mx(px(-6.))
            .mb(px(4.))
            .map(|d| {
                if first {
                    d.mt(px(-6.))
                } else {
                    d.mt(px(10.)).border_t_1().border_color(theme::border())
                }
            })
            .pt(px(8.))
            .px(px(16.))
            .pb(px(6.))
            .bg(theme::panel())
            .text_size(px(11.))
            .font_weight(FontWeight::BOLD)
            .text_color(theme::fg())
            .child(label.to_uppercase())
            .when_some(count, |d, n| {
                d.child(
                    div()
                        .px(px(7.))
                        .py(px(1.))
                        .rounded_full()
                        .bg(hsla(0., 0., 1., 0.08))
                        .text_color(theme::muted())
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(n.to_string()),
                )
            })
    }
}

impl Render for SearchModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mobile = roostr_core::layout::is_mobile(window);
        let sheet_offset = if mobile {
            self.sheet_offset(window, cx)
        } else {
            0.
        };
        // The dim fades in; the desktop panel appears like a dialog.
        let enter = crate::modal::dialog_appear("search-appear", window, cx);
        let viewport = roostr_core::layout::viewport(window);
        let channel = Self::channel_id(cx);
        let active = Navigator::global(cx).read(cx).space().to_string();
        let q = self.query.trim().to_string();
        let space_name = Store::global(cx)
            .read(cx)
            .space(&channel)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let type_matches = self.type_matches(cx);
        let groups = self.groups(cx);
        let ordered_len = type_matches.len() + groups.iter().map(|g| g.rows.len()).sum::<usize>();
        let can_create = !q.is_empty();

        // Chips: every space type while empty; else the result types with counts, fewest first.
        let chips: Vec<(String, String, Option<usize>)> = {
            let store = Store::global(cx).read(cx);
            if q.is_empty() {
                store
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
                for r in &self.results {
                    match counts.iter_mut().find(|(k, _)| *k == r.type_key) {
                        Some((_, n)) => *n += 1,
                        None => counts.push((r.type_key.clone(), 1)),
                    }
                }
                let mut all: Vec<(String, String, usize)> = counts
                    .into_iter()
                    .map(|(k, n)| (k.clone(), Self::type_name(&k, cx), n))
                    .collect();
                all.sort_by(|a, b| {
                    a.2.cmp(&b.2)
                        .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
                });
                all.into_iter()
                    .map(|(k, name, n)| {
                        (
                            k.clone(),
                            format!("{} {}", type_icon(store, &k, &active), name),
                            Some(n),
                        )
                    })
                    .collect()
            }
        };

        let mut results = div()
            .id("search-results")
            .map(|d| {
                if mobile {
                    // The sheet is fixed-height: results fill the rest.
                    d.flex_1().min_h_0()
                } else {
                    d.max_h(viewport.height * 0.46)
                }
            })
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(px(6.))
            .flex()
            .flex_col();
        let mut first = true;
        if !type_matches.is_empty() {
            results = results.child(Self::section_header(first, "Types".into(), None));
            first = false;
            let store = Store::global(cx).read(cx);
            for (i, t) in type_matches.iter().enumerate() {
                let selected = i == self.selected;
                let icon = if t.icon.is_empty() {
                    type_icon(store, &t.key, &active)
                } else {
                    t.icon.clone()
                };
                let id = t.id.clone();
                results = results.child(
                    Self::row(crate::eid("search-type", &t.id), selected, mobile)
                        .child(Self::icon(icon, selected))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .flex_1()
                                .min_w_0()
                                .child(div().text_size(px(14.)).truncate().child(
                                    if t.name.is_empty() {
                                        t.key.clone()
                                    } else {
                                        t.name.clone()
                                    },
                                ))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .truncate()
                                        .text_color(Self::secondary(selected))
                                        .child("Type · opens with its objects"),
                                ),
                        )
                        .child(Self::kind("Type", selected, mobile))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.selected != i {
                                this.selected = i;
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), cx))),
                );
            }
        }
        let mut offset = type_matches.len();
        for g in &groups {
            let label = if g.key.is_empty() {
                g.name.clone()
            } else {
                format!(
                    "{} {}",
                    type_icon(Store::global(cx).read(cx), &g.key, &active),
                    g.name
                )
            };
            results = results.child(Self::section_header(
                first,
                label,
                (!q.is_empty()).then_some(g.rows.len()),
            ));
            first = false;
            for (j, row) in g.rows.iter().enumerate() {
                let i = offset + j;
                let selected = i == self.selected;
                let store = Store::global(cx).read(cx);
                let icon = object_icon(
                    store,
                    field_str(&row.fields, "iconEmoji"),
                    &row.type_key,
                    &active,
                );
                let name = row
                    .name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "Untitled".into());
                let kind = format!(
                    "{} {}",
                    type_icon(store, &row.type_key, &active),
                    Self::type_name(&row.type_key, cx)
                );
                let id = row.id.clone();
                results = results.child(
                    Self::row(crate::eid("search-row", &row.id), selected, mobile)
                        .child(Self::icon(icon, selected))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .truncate()
                                        .child(highlighted(name, &q, selected, mobile)),
                                )
                                .when_some(
                                    row.snippet.clone().filter(|s| !s.is_empty()),
                                    |d, snippet| {
                                        d.child(
                                            div()
                                                .text_size(px(12.))
                                                .truncate()
                                                .text_color(Self::secondary(selected))
                                                .child(highlighted(snippet, &q, selected, mobile)),
                                        )
                                    },
                                ),
                        )
                        .child(Self::kind(kind, selected, mobile))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.selected != i {
                                this.selected = i;
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), cx))),
                );
            }
            offset += g.rows.len();
        }
        if can_create && !self.searching {
            let selected = self.selected == ordered_len;
            let create_type = self.create_type();
            let kind = format!(
                "{} {}",
                type_icon(Store::global(cx).read(cx), &create_type, &active),
                Self::type_name(&create_type, cx)
            );
            results = results.child(
                Self::row("search-create".into(), selected, mobile)
                    .child(Self::icon("＋".into(), selected).text_color(if selected {
                        theme::white()
                    } else {
                        theme::accent()
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(14.))
                            .truncate()
                            .child(format!("Create object \"{q}\"")),
                    )
                    .child(Self::kind(kind, selected, mobile))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.selected != ordered_len {
                            this.selected = ordered_len;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.create_from_query(cx))),
            );
        }
        if ordered_len == 0 && !self.searching && !can_create {
            results = results.child(
                div()
                    .px(px(12.))
                    .py(px(16.))
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child("Nothing here yet."),
            );
        }

        let width = (viewport.width - px(48.)).min(px(620.)) + px(2.);
        let sheet_listener = mobile.then(|| {
            let this = cx.entity().downgrade();
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let this = this.clone();
                    window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                        // Capture: before the results scroll by this step.
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let ScrollDelta::Pixels(delta) = e.delta else {
                            return;
                        };
                        let Some(this) = this.upgrade() else {
                            return;
                        };
                        let inside = bounds.contains(&e.position);
                        let ours = this.update(cx, |this, cx| {
                            this.sheet_pan(e.touch_phase, delta, inside, window, cx)
                        });
                        if ours {
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .inset_0()
        });
        let has_query = !self.query.is_empty();
        div()
            .id("search-overlay")
            .line_height(relative(1.2))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(theme::overlay().opacity(enter.target().alpha()))
            .occlude()
            .flex()
            .justify_center()
            .map(|d| {
                if mobile {
                    d.items_end()
                } else {
                    d.items_start().pt(viewport.height * 0.12)
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close(cx)),
            )
            .child(
                ui::modal_frame(width)
                    .when(!mobile, |d| enter.apply(d))
                    .id("search-modal")
                    .debug_selector(|| "search-modal".into())
                    .key_context(CONTEXT)
                    .track_focus(&self.focus)
                    .when(mobile, |d| {
                        // Leaves the top strip free: a full-height sheet would park
                        // its grab handle in the system-gesture edge.
                        d.relative()
                            .top(px(sheet_offset))
                            .w_full()
                            .h(viewport.height - px(56.))
                            .rounded(px(0.))
                            .rounded_t(px(18.))
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                        cx.stop_propagation();
                        this.step(true, cx);
                    }))
                    .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                        cx.stop_propagation();
                        this.step(false, cx);
                    }))
                    .capture_action(cx.listener(|this, _: &Enter, _, cx| {
                        cx.stop_propagation();
                        this.open_selected(cx);
                    }))
                    .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                        cx.stop_propagation();
                        this.close(cx);
                    }))
                    .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(true, cx)))
                    .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step(false, cx)))
                    .on_action(cx.listener(|this, _: &OpenSelected, _, cx| this.open_selected(cx)))
                    .on_action(cx.listener(|this, _: &CloseSearch, _, cx| this.close(cx)))
                    .children(sheet_listener)
                    .when(mobile, |d| {
                        // The grab handle.
                        d.child(
                            div()
                                .flex()
                                .flex_none()
                                .justify_center()
                                .pt(px(12.))
                                .pb(px(8.))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(40.))
                                        .h(px(5.))
                                        .rounded(px(3.))
                                        .bg(theme::border()),
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(10.))
                            .px(px(14.))
                            .map(|d| {
                                if mobile {
                                    d.py(px(8.))
                                } else {
                                    d.py(px(12.)).border_b_1().border_color(theme::border())
                                }
                            })
                            .when(!mobile, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(12.))
                                        .text_color(theme::accent())
                                        .border_1()
                                        .border_color(theme::border())
                                        .rounded(theme::radius::sm())
                                        .px(px(8.))
                                        .py(px(2.))
                                        .child(space_name),
                                )
                            })
                            .when(mobile, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(16.))
                                        .text_color(theme::muted())
                                        .child("⌕"),
                                )
                            })
                            .child(if mobile {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .bg(theme::hover())
                                    .border_1()
                                    .border_color(theme::border())
                                    .rounded(px(12.))
                                    .px(px(12.))
                                    .py(px(10.))
                                    .text_size(px(16.))
                                    .child(
                                        Input::new(&self.input)
                                            .appearance(false)
                                            .text_size(px(16.))
                                            .px_0()
                                            .py_0(),
                                    )
                            } else {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(19.))
                                    .font_weight(FontWeight::LIGHT)
                                    .child(
                                        Input::new(&self.input)
                                            .appearance(false)
                                            .text_size(px(19.))
                                            .font_weight(FontWeight::LIGHT)
                                            .px_0()
                                            .py_0(),
                                    )
                            })
                            .when(mobile && has_query, |d| {
                                d.child(
                                    div()
                                        .id("search-clear")
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .justify_center()
                                        .size(px(28.))
                                        .ml(px(-36.))
                                        .rounded_full()
                                        .bg(theme::hover())
                                        .text_color(theme::muted())
                                        .text_size(px(15.))
                                        .cursor_pointer()
                                        .child("×")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.input.update(cx, |input, cx| {
                                                input.set_value("", window, cx);
                                                input.focus(window, cx);
                                            });
                                            this.query.clear();
                                            this.search(cx);
                                        })),
                                )
                            }),
                    )
                    .when(!chips.is_empty(), |d| {
                        let mut row = div()
                            .id("search-chips")
                            .flex()
                            .flex_none()
                            .overflow_x_scroll()
                            .px(px(14.))
                            .border_b_1()
                            .border_color(theme::border())
                            .map(|d| {
                                if mobile {
                                    d.gap(px(8.)).pt(px(4.)).pb(px(10.))
                                } else {
                                    d.gap(px(6.)).py(px(8.))
                                }
                            });
                        for (key, label, count) in chips {
                            let on = self.chips.contains(&key);
                            row = row.child(
                                div()
                                    .id(crate::eid("chip", &key))
                                    .flex()
                                    .flex_none()
                                    .gap(px(2.))
                                    .map(|d| {
                                        if mobile {
                                            d.px(px(14.)).py(px(7.)).text_size(px(14.))
                                        } else {
                                            d.px(px(11.)).py(px(4.)).text_size(px(12.5))
                                        }
                                    })
                                    .rounded_full()
                                    .whitespace_nowrap()
                                    .cursor_pointer()
                                    .map(|d| {
                                        if on {
                                            d.bg(theme::accent()).text_color(theme::white())
                                        } else {
                                            d.bg(theme::hover()).text_color(theme::fg())
                                        }
                                    })
                                    .child(label)
                                    .when_some(count, |d, n| {
                                        d.child(
                                            div()
                                                .ml(px(2.))
                                                .text_color(theme::muted())
                                                .child(n.to_string()),
                                        )
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_chip(key.clone(), cx)
                                    })),
                            );
                        }
                        d.child(row)
                    })
                    .child(results)
                    .when(!mobile, |d| {
                        d.child(
                            div()
                                .flex()
                                .gap(px(16.))
                                .border_t_1()
                                .border_color(theme::border())
                                .px(px(14.))
                                .py(px(8.))
                                .text_size(px(11.))
                                .text_color(theme::muted())
                                .child("↑↓ navigate")
                                .child("↵ open")
                                .child("esc close"),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::find_ci;

    #[test]
    fn finds_case_insensitively() {
        assert_eq!(find_ci("Launch Plan", "plan"), Some(7..11));
        assert_eq!(find_ci("Über", "über"), Some(0..5));
        assert_eq!(find_ci("abc", "x"), None);
        assert_eq!(find_ci("abc", ""), None);
    }
}

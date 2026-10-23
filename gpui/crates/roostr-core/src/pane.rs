//! The right "Chat & properties" pane (`discussionUI` in data.svelte.ts): the
//! object page publishes whether it has one and its counts. On a desktop it is
//! two columns beside the page, Chat then Properties, each expanded or
//! collapsed to a strip and each its own width; both persist in [`UiState`].
//! On a phone it is one full-screen sheet with a Chat / Properties switch.
//!
//! Migration from the single-pane state: the old pane opened on every start
//! and never stored its open flag or tab, so with no stored state both
//! columns start expanded; the old width (`disc-drawer-w`) is the Chat width.

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, Global};

use crate::ui_state::UiState;

const CHAT_WIDTH_KEY: &str = "disc-drawer-w";
const PROPS_WIDTH_KEY: &str = "pane-props-w";
const CHAT_OPEN_KEY: &str = "pane-chat-open";
const PROPS_OPEN_KEY: &str = "pane-props-open";
pub const DEFAULT_WIDTH: f32 = 380.;
pub const DEFAULT_PROPS_WIDTH: f32 = 320.;
pub const MIN_WIDTH: f32 = 300.;
/// A ceiling for very wide screens; below it the window decides (`max_width`).
pub const MAX_WIDTH: f32 = 1600.;
/// What the page beside the columns keeps: they grow into the rest.
pub const PAGE_MIN: f32 = 420.;
/// A collapsed column: a strip showing its name.
pub const STRIP_WIDTH: f32 = 36.;

/// The widest one column may be when `room` is the width it shares with the
/// page (the window less the rail, the sidebar and the other column): the
/// page keeps `PAGE_MIN`.
pub fn max_width(room: f32) -> f32 {
    (room - PAGE_MIN).clamp(MIN_WIDTH, MAX_WIDTH)
}

/// One of the pane's two parts: a desktop column, a tab of the phone's sheet.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum PaneTab {
    Chat,
    Properties,
}

impl PaneTab {
    pub fn name(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Properties => "Properties",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Chat => Self::Properties,
            Self::Properties => Self::Chat,
        }
    }

    /// The tab's icon: the chat type's (of the current space), the puzzle piece.
    pub fn icon(self, cx: &App) -> String {
        match self {
            Self::Chat => {
                let space = crate::Navigator::global(cx).read(cx).space().to_string();
                let icon =
                    crate::objects::type_icon(crate::Store::global(cx).read(cx), "chat", &space);
                if icon.is_empty() { "•".into() } else { icon }
            }
            Self::Properties => "🧩".into(),
        }
    }
}

/// How a desktop column shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Column {
    /// Not there: no object pane, or Chat for an object without a discussion.
    Hidden,
    /// Collapsed, or squeezed out by a window too narrow for both.
    Strip,
    /// Expanded at this width.
    Open(f32),
}

impl Column {
    pub fn width(self) -> f32 {
        match self {
            Self::Hidden => 0.,
            Self::Strip => STRIP_WIDTH,
            Self::Open(width) => width,
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, Self::Open(_))
    }
}

/// The two columns as laid out in a window.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PaneLayout {
    pub chat: Column,
    pub properties: Column,
}

impl PaneLayout {
    pub const HIDDEN: Self = Self {
        chat: Column::Hidden,
        properties: Column::Hidden,
    };

    pub fn get(&self, tab: PaneTab) -> Column {
        match tab {
            PaneTab::Chat => self.chat,
            PaneTab::Properties => self.properties,
        }
    }

    fn set(&mut self, tab: PaneTab, column: Column) {
        match tab {
            PaneTab::Chat => self.chat = column,
            PaneTab::Properties => self.properties = column,
        }
    }

    pub fn width(&self) -> f32 {
        self.chat.width() + self.properties.width()
    }
}

/// The views a page publishes for its pane (see `roostr_shell::set_pane`).
#[derive(Clone)]
pub struct PaneViews {
    /// The phone's sheet: a Chat / Properties switch over the active one.
    pub sheet: AnyView,
    /// The desktop Chat column; `None` for an object without a discussion.
    pub chat: Option<AnyView>,
    /// The desktop Properties column.
    pub properties: AnyView,
}

impl PaneViews {
    pub fn column(&self, tab: PaneTab) -> Option<AnyView> {
        match tab {
            PaneTab::Chat => self.chat.clone(),
            PaneTab::Properties => Some(self.properties.clone()),
        }
    }
}

pub struct PaneState {
    /// Desktop: the Chat column is expanded (else a strip). Persisted.
    pub chat_open: bool,
    /// Desktop: the Properties column is expanded (else a strip). Persisted.
    pub props_open: bool,
    /// Persisted widths (300–1600); [`PaneState::layout`] fits them to the window.
    pub chat_width: f32,
    pub props_width: f32,
    /// The column expanded last: it keeps its place when the window has
    /// room for one only, and Escape collapses it first.
    pub last: PaneTab,
    /// Phone: the full-screen sheet is up (each object opens without it).
    pub sheet: bool,
    /// The open object has a pane.
    pub available: bool,
    /// The open object has a discussion: the pane has a Chat part.
    pub chat: bool,
    /// Chat messages in the object's discussion (header badge).
    pub count: usize,
    /// Conversations (threads + the discussion).
    pub conv_count: usize,
}

struct GlobalPane(Entity<PaneState>);

impl Global for GlobalPane {}

impl PaneState {
    /// Read from [`UiState`] (also when a new app starts on the same file).
    pub fn init(cx: &mut App) -> Entity<Self> {
        let width = |cx: &App, key, default| {
            UiState::get::<f32>(cx, key)
                .unwrap_or(default)
                .clamp(MIN_WIDTH, MAX_WIDTH)
        };
        let chat_width = width(cx, CHAT_WIDTH_KEY, DEFAULT_WIDTH);
        let props_width = width(cx, PROPS_WIDTH_KEY, DEFAULT_PROPS_WIDTH);
        let chat_open = UiState::get::<bool>(cx, CHAT_OPEN_KEY).unwrap_or(true);
        let props_open = UiState::get::<bool>(cx, PROPS_OPEN_KEY).unwrap_or(true);
        let pane = cx.new(|_| Self {
            chat_open,
            props_open,
            chat_width,
            props_width,
            last: PaneTab::Chat,
            sheet: false,
            available: false,
            chat: false,
            count: 0,
            conv_count: 0,
        });
        cx.set_global(GlobalPane(pane.clone()));
        pane
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPane>().0.clone()
    }

    pub fn is_open(&self, tab: PaneTab) -> bool {
        match tab {
            PaneTab::Chat => self.chat_open,
            PaneTab::Properties => self.props_open,
        }
    }

    pub fn width(&self, tab: PaneTab) -> f32 {
        match tab {
            PaneTab::Chat => self.chat_width,
            PaneTab::Properties => self.props_width,
        }
    }

    /// The pane has this part for the open object.
    pub fn has(&self, tab: PaneTab) -> bool {
        self.available && (tab == PaneTab::Properties || self.chat)
    }

    /// The part the header's pane button opens: Chat, else Properties.
    pub fn primary(&self) -> PaneTab {
        if self.chat {
            PaneTab::Chat
        } else {
            PaneTab::Properties
        }
    }

    /// Expand (or collapse) a desktop column; expanding makes it [`Self::last`].
    pub fn set_open(&mut self, tab: PaneTab, open: bool, cx: &mut Context<Self>) {
        let changed = self.is_open(tab) != open || (open && self.last != tab);
        if open {
            self.last = tab;
        }
        if self.is_open(tab) != open {
            let (field, key) = match tab {
                PaneTab::Chat => (&mut self.chat_open, CHAT_OPEN_KEY),
                PaneTab::Properties => (&mut self.props_open, PROPS_OPEN_KEY),
            };
            *field = open;
            UiState::set(cx, key, &open);
        }
        if changed {
            cx.notify();
        }
    }

    /// Both columns at once (the edge drawer tab).
    pub fn set_all(&mut self, open: bool, cx: &mut Context<Self>) {
        let last = self.last;
        self.set_open(last.other(), open, cx);
        self.set_open(last, open, cx);
    }

    /// Any column of the open object's pane is expanded.
    pub fn any_open(&self) -> bool {
        [PaneTab::Chat, PaneTab::Properties]
            .into_iter()
            .any(|tab| self.has(tab) && self.is_open(tab))
    }

    /// Escape on a desktop: collapse the column expanded last if it is open,
    /// else the other one.
    pub fn collapse_last(&mut self, cx: &mut Context<Self>) {
        let last = self.last;
        if let Some(tab) = [last, last.other()]
            .into_iter()
            .find(|tab| self.has(*tab) && self.is_open(*tab))
        {
            self.set_open(tab, false, cx);
        }
    }

    /// The phone's sheet.
    pub fn set_sheet(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.sheet != open {
            self.sheet = open;
            cx.notify();
        }
    }

    /// Published by the object page for the object it shows.
    pub fn publish(
        &mut self,
        available: bool,
        chat: bool,
        count: usize,
        conv_count: usize,
        cx: &mut Context<Self>,
    ) {
        let next = (available, chat, count, conv_count);
        if (self.available, self.chat, self.count, self.conv_count) != next {
            self.available = available;
            self.chat = chat;
            self.count = count;
            self.conv_count = conv_count;
            cx.notify();
        }
    }

    /// A column's width while a resizer drags (see [`Self::save_width`]).
    pub fn set_width(&mut self, tab: PaneTab, width: f32, cx: &mut Context<Self>) {
        let width = width.clamp(MIN_WIDTH, MAX_WIDTH);
        let field = match tab {
            PaneTab::Chat => &mut self.chat_width,
            PaneTab::Properties => &mut self.props_width,
        };
        if *field != width {
            *field = width;
            cx.notify();
        }
    }

    /// Persist a column's width (the resizer let go).
    pub fn save_width(&self, tab: PaneTab, cx: &mut App) {
        let key = match tab {
            PaneTab::Chat => CHAT_WIDTH_KEY,
            PaneTab::Properties => PROPS_WIDTH_KEY,
        };
        UiState::set(cx, key, &self.width(tab));
    }

    /// The columns in `room` (the window less the rail and sidebar). A
    /// collapsed column takes no room: it slides shut, and the open column's
    /// header offers it back (both collapsed, both slide away; the edge tab
    /// brings them back). The page keeps `PAGE_MIN` beside them: two expanded
    /// columns that will not both fit at `MIN_WIDTH` show the one expanded
    /// last, the other as its strip (its stored state untouched, it comes
    /// back with the room); a lone column narrows to fit but never below
    /// `MIN_WIDTH`, as before.
    pub fn layout(&self, room: f32) -> PaneLayout {
        let mut layout = PaneLayout::HIDDEN;
        let mut open = Vec::with_capacity(2);
        for tab in [PaneTab::Chat, PaneTab::Properties] {
            if self.has(tab) && self.is_open(tab) {
                // A strip until it is given a width: the one squeezed out stays one.
                layout.set(tab, Column::Strip);
                open.push(tab);
            }
        }
        let strips = |layout: &PaneLayout| {
            [layout.chat, layout.properties]
                .into_iter()
                .filter(|c| *c == Column::Strip)
                .count() as f32
                * STRIP_WIDTH
        };
        match open[..] {
            [] => {}
            [tab] => {
                let others = strips(&layout) - STRIP_WIDTH;
                layout.set(
                    tab,
                    Column::Open(self.width(tab).min(max_width(room - others))),
                );
            }
            [..] => {
                let first = self.last;
                let second = first.other();
                let room_for_both = room - PAGE_MIN;
                if room_for_both >= 2. * MIN_WIDTH {
                    let a = self
                        .width(first)
                        .min(room_for_both - MIN_WIDTH)
                        .max(MIN_WIDTH);
                    let b = self.width(second).min(room_for_both - a).max(MIN_WIDTH);
                    layout.set(first, Column::Open(a));
                    layout.set(second, Column::Open(b));
                } else {
                    layout.set(
                        first,
                        Column::Open(self.width(first).min(max_width(room - STRIP_WIDTH))),
                    );
                }
            }
        }
        layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(chat_open: bool, props_open: bool) -> PaneState {
        PaneState {
            chat_open,
            props_open,
            chat_width: 380.,
            props_width: 320.,
            last: PaneTab::Chat,
            sheet: false,
            available: true,
            chat: true,
            count: 0,
            conv_count: 0,
        }
    }

    #[test]
    fn a_column_grows_into_the_window_leaving_the_page_its_minimum() {
        // A normal window: everything past the page's minimum.
        assert_eq!(max_width(1500.), 1500. - PAGE_MIN);
        // A narrow window still allows the column's own minimum.
        assert_eq!(max_width(500.), MIN_WIDTH);
        // A very wide screen stops at the ceiling.
        assert_eq!(max_width(4000.), MAX_WIDTH);
    }

    #[test]
    fn both_columns_show_at_their_widths_when_there_is_room() {
        let layout = state(true, true).layout(1400.);
        assert_eq!(layout.chat, Column::Open(380.));
        assert_eq!(layout.properties, Column::Open(320.));
    }

    #[test]
    fn wide_columns_narrow_so_the_page_keeps_its_minimum() {
        let mut pane = state(true, true);
        pane.chat_width = 900.;
        pane.props_width = 900.;
        let layout = pane.layout(1400.);
        // The one expanded last keeps all it can; the other gets the rest.
        assert_eq!(layout.chat, Column::Open(1400. - PAGE_MIN - MIN_WIDTH));
        assert_eq!(layout.properties, Column::Open(MIN_WIDTH));
        assert_eq!(layout.width(), 1400. - PAGE_MIN);
    }

    #[test]
    fn a_window_too_narrow_for_both_shows_the_last_expanded_and_a_strip() {
        let mut pane = state(true, true);
        let layout = pane.layout(900.);
        assert_eq!(layout.chat, Column::Open(380.));
        assert_eq!(layout.properties, Column::Strip);
        pane.last = PaneTab::Properties;
        let layout = pane.layout(900.);
        assert_eq!(layout.chat, Column::Strip);
        assert_eq!(layout.properties, Column::Open(320.));
        // Narrower still: the column keeps its minimum, as the single pane did.
        assert_eq!(pane.layout(500.).properties, Column::Open(MIN_WIDTH));
    }

    #[test]
    fn collapsed_columns_take_no_room_and_chat_needs_a_discussion() {
        // One collapsed: it slides shut, the other keeps its width.
        let layout = state(false, true).layout(1400.);
        assert_eq!(layout.chat, Column::Hidden);
        assert_eq!(layout.properties, Column::Open(320.));
        // Both collapsed: both go.
        assert_eq!(state(false, false).layout(1400.), PaneLayout::HIDDEN);
        let mut pane = state(true, true);
        pane.chat = false;
        let layout = pane.layout(1400.);
        assert_eq!(layout.chat, Column::Hidden);
        assert_eq!(layout.properties, Column::Open(320.));
        pane.available = false;
        assert_eq!(pane.layout(1400.), PaneLayout::HIDDEN);
    }
}

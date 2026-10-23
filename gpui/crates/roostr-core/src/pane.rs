//! The right "Chat & properties" pane (`discussionUI` in data.svelte.ts): the
//! object page publishes whether it has one and its counts; the shell header
//! and drawer tab toggle it. Open state survives navigation (not restarts).

use gpui_kit::{App, AppContext as _, Context, Entity, Global};

use crate::ui_state::UiState;

const WIDTH_KEY: &str = "disc-drawer-w";
pub const DEFAULT_WIDTH: f32 = 380.;
pub const MIN_WIDTH: f32 = 300.;
/// A ceiling for very wide screens; below it the window decides (`max_width`).
pub const MAX_WIDTH: f32 = 1600.;
/// What the page beside the pane always keeps: the pane grows into the rest.
pub const PAGE_MIN: f32 = 420.;

/// The widest the pane may be when `room` is the width it shares with the
/// page (the window less the rail and sidebar): the page keeps `PAGE_MIN`.
pub fn max_width(room: f32) -> f32 {
    (room - PAGE_MIN).clamp(MIN_WIDTH, MAX_WIDTH)
}

pub struct PaneState {
    pub open: bool,
    /// The open object has a pane.
    pub available: bool,
    /// Chat messages in the object's discussion (header badge).
    pub count: usize,
    /// Conversations (threads + the discussion).
    pub conv_count: usize,
    /// Persisted width (300–1600); `max_width` fits it to the window when shown.
    pub width: f32,
}

struct GlobalPane(Entity<PaneState>);

impl Global for GlobalPane {}

impl PaneState {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let width = UiState::get::<f32>(cx, WIDTH_KEY)
            .unwrap_or(DEFAULT_WIDTH)
            .clamp(MIN_WIDTH, MAX_WIDTH);
        let pane = cx.new(|_| Self {
            open: true,
            available: false,
            count: 0,
            conv_count: 0,
            width,
        });
        cx.set_global(GlobalPane(pane.clone()));
        pane
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPane>().0.clone()
    }

    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open != open {
            self.open = open;
            cx.notify();
        }
    }

    /// Published by the object page for the object it shows.
    pub fn publish(
        &mut self,
        available: bool,
        count: usize,
        conv_count: usize,
        cx: &mut Context<Self>,
    ) {
        if (self.available, self.count, self.conv_count) != (available, count, conv_count) {
            self.available = available;
            self.count = count;
            self.conv_count = conv_count;
            cx.notify();
        }
    }

    pub fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = width.clamp(MIN_WIDTH, MAX_WIDTH);
        if self.width != width {
            self.width = width;
            UiState::set(cx, WIDTH_KEY, &width);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_grows_into_the_window_leaving_the_page_its_minimum() {
        // A normal window: everything past the page's minimum.
        assert_eq!(max_width(1500.), 1500. - PAGE_MIN);
        // A narrow window still allows the pane's own minimum.
        assert_eq!(max_width(500.), MIN_WIDTH);
        // A very wide screen stops at the ceiling.
        assert_eq!(max_width(4000.), MAX_WIDTH);
    }
}

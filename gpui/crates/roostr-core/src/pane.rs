//! The right "Chat & properties" pane (`discussionUI` in data.svelte.ts): the
//! object page publishes whether it has one and its counts; the shell header
//! and drawer tab toggle it. Open state survives navigation (not restarts).

use gpui_kit::{App, AppContext as _, Context, Entity, Global};

use crate::ui_state::UiState;

const WIDTH_KEY: &str = "disc-drawer-w";
pub const DEFAULT_WIDTH: f32 = 380.;
pub const MIN_WIDTH: f32 = 300.;
pub const MAX_WIDTH: f32 = 640.;

pub struct PaneState {
    pub open: bool,
    /// The open object has a pane.
    pub available: bool,
    /// Chat messages in the object's discussion (header badge).
    pub count: usize,
    /// Conversations (threads + the discussion).
    pub conv_count: usize,
    /// Persisted width (300–640).
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

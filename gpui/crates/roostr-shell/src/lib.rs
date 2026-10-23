//! App shell (`src/routes/app/+layout.svelte`): space rail, sidebar (space
//! head, New menu, pinned widgets, Recently edited / Types / Properties),
//! per-space tab strip, page header with back/forward, search path, graph link
//! and the More menu, drawer tabs, resizers, the search modal (⌘K) and the
//! boot states. The main area shows whatever [`PageFactory`] builds for the
//! active route; the right "Chat & properties" columns show what the page
//! publishes with [`set_pane`]. At [`roostr_core::layout::is_mobile`] widths
//! the mobile shell replaces all of that (Spaces list → space home → object).
//!
//! Hooks for the app binary:
//! - [`Workspace::on_open_settings`]: the rail's account button and the
//!   [`OpenSettings`] action (open roostr-settings' modal).
//! - [`Workspace::identity_gate`]: the view shown instead of the workspace
//!   while the daemon has no identity.
//! - [`set_pane`]: the object page's right pane (ConversationDrawer's views).
//! - Actions [`Search`], [`NewObject`], [`OpenSettings`] for the app menu.

mod back_swipe;
mod create_menu;
mod header;
mod list_motion;
mod mobile;
mod modal;
mod pin_collapse;
mod pinned;
mod rail;
mod search;
mod sidebar;
mod tabs;
mod workspace;

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::{
    App, AppContext as _, ElementId, Entity, Global, Hsla, KeyBinding, SharedString, Styled as _,
    actions, transparent_black,
};
use roostr_core::PaneViews;
use roostr_core::theme;

pub use workspace::{PageFactory, Workspace};

actions!(
    roostr_shell,
    [
        /// Open the search modal (⌘K).
        Search,
        /// Open the sidebar's New menu.
        NewObject,
        /// Open Settings (through [`Workspace::on_open_settings`]).
        OpenSettings,
    ]
);

actions!(roostr_shell_focus, [FocusNext, FocusPrev]);

/// `[data-tip]` appears after this much hover (`animation: tip-in 0.12s ease 0.4s`).
pub(crate) const TIP_DELAY: Duration = Duration::from_millis(400);

/// The shell's `[data-tip]` on a kit [`Button`]: the kit tooltip, shown
/// after [`TIP_DELAY`].
pub(crate) trait Tip {
    fn tip(self, text: impl Into<SharedString>) -> Self;
}

impl Tip for Button {
    fn tip(self, text: impl Into<SharedString>) -> Self {
        self.tooltip(text).tooltip_show_delay(TIP_DELAY)
    }
}

/// A kit [`Button`] for the shell's bare controls: transparent and muted at
/// rest, filling `hover` (also while pressed) with `fg` text under the
/// pointer. Callers size and round it; content goes in children.
pub(crate) fn fill_button(id: impl Into<ElementId>, hover: Hsla, cx: &App) -> Button {
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(transparent_black())
                .foreground(theme::fg())
                .hover(hover)
                .active(hover)
                .shadow(false),
        )
        .flex_none()
        .p_0()
        .bg(transparent_black())
        .text_color(theme::muted())
        .cursor_pointer()
}

/// A per-object element id (`"<prefix>:<id>"`).
pub(crate) fn eid(prefix: &str, id: &str) -> gpui_kit::ElementId {
    SharedString::from(format!("{prefix}:{id}")).into()
}

/// Key bindings: ⌘K / Ctrl+K opens search; Tab / Shift-Tab move keyboard
/// focus between focusable shell controls (the tabs), like the browser.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-k", Search, None),
        KeyBinding::new("ctrl-k", Search, None),
        KeyBinding::new("tab", FocusNext, Some("Workspace")),
        KeyBinding::new("shift-tab", FocusPrev, Some("Workspace")),
    ]);
    create_menu::init(cx);
    search::init(cx);
    header::init(cx);
    modal::init(cx);
}

/// The right pane's content (`discussionUI` aside): the page shown in the
/// main area publishes its pane views here when it becomes the active route
/// (`None` = no pane). The shell clears it whenever it shows a route whose
/// page has not been built yet or that is not an object page.
pub fn set_pane(views: Option<PaneViews>, cx: &mut App) {
    let host = PaneHost::global(cx);
    host.update(cx, |host, cx| {
        let same = match (&host.views, &views) {
            (Some(a), Some(b)) => a.sheet.entity_id() == b.sheet.entity_id(),
            (None, None) => true,
            _ => false,
        };
        if !same {
            host.views = views;
            cx.notify();
        }
    });
}

/// Holder of the published pane views.
pub(crate) struct PaneHost {
    pub(crate) views: Option<PaneViews>,
}

struct GlobalPaneHost(Entity<PaneHost>);

impl Global for GlobalPaneHost {}

impl PaneHost {
    pub(crate) fn global(cx: &mut App) -> Entity<Self> {
        if let Some(host) = cx.try_global::<GlobalPaneHost>() {
            return host.0.clone();
        }
        let host = cx.new(|_| Self { views: None });
        cx.set_global(GlobalPaneHost(host.clone()));
        host
    }
}

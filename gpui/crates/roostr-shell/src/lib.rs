//! App shell (`src/routes/app/+layout.svelte`): space rail, sidebar (space
//! head, New menu, pinned widgets, Recently edited / Types / Properties),
//! per-space tab strip, page header with back/forward, search path, graph link
//! and the More menu, drawer tabs, resizers, the search modal (⌘K) and the
//! boot states. The main area shows whatever [`PageFactory`] builds for the
//! active route; the right "Chat & properties" pane shows what the page
//! publishes with [`set_pane`]. At [`roostr_core::layout::is_mobile`] widths
//! the mobile shell replaces all of that (Spaces list → space home → object).
//!
//! Hooks for the app binary:
//! - [`Workspace::on_open_settings`]: the rail's account button and the
//!   [`OpenSettings`] action (open roostr-settings' modal).
//! - [`Workspace::identity_gate`]: the view shown instead of the workspace
//!   while the daemon has no identity.
//! - [`set_pane`]: the object page's right pane (ConversationDrawer).
//! - Actions [`Search`], [`NewObject`], [`OpenSettings`] for the app menu.

mod create_menu;
mod header;
mod mobile;
mod modal;
mod pinned;
mod rail;
mod search;
mod sidebar;
mod tabs;
mod workspace;

use gpui_kit::{
    AnyView, App, AppContext as _, Entity, Global, KeyBinding, SharedString, Window, actions,
};

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

/// The `[data-tip]` tooltip.
pub(crate) fn tip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    roostr_core::ui::tooltip(text)
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
/// main area publishes its pane view here when it becomes the active route
/// (`None` = no pane). The shell clears it whenever it shows a route whose
/// page has not been built yet or that is not an object page.
pub fn set_pane(view: Option<AnyView>, cx: &mut App) {
    let host = PaneHost::global(cx);
    host.update(cx, |host, cx| {
        let same = match (&host.view, &view) {
            (Some(a), Some(b)) => a.entity_id() == b.entity_id(),
            (None, None) => true,
            _ => false,
        };
        if !same {
            host.view = view;
            cx.notify();
        }
    });
}

/// Holder of the published pane view.
pub(crate) struct PaneHost {
    pub(crate) view: Option<AnyView>,
}

struct GlobalPaneHost(Entity<PaneHost>);

impl Global for GlobalPaneHost {}

impl PaneHost {
    pub(crate) fn global(cx: &mut App) -> Entity<Self> {
        if let Some(host) = cx.try_global::<GlobalPaneHost>() {
            return host.0.clone();
        }
        let host = cx.new(|_| Self { view: None });
        cx.set_global(GlobalPaneHost(host.clone()));
        host
    }
}

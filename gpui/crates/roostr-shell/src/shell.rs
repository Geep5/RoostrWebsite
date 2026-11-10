//! The shell's own state ([`Shell`], a slot of the app state) and the small
//! helpers every part of the shell draws with.

use std::collections::HashMap;

use roostr_core::nav::Route;
use roostr_core::pane::PaneTab;
use roostr_core::types::Space;
use roostr_core::{State, UiState};
use roostr_rt::Task;
use roostr_ui::{Glyph, Images, Theme, glyph, ty};
use rune::{Cursor, El, TextStyle, el, row};

use crate::Pages;

pub const RAIL_MIN: f32 = 56.;
pub const RAIL_MAX: f32 = 320.;
pub(crate) const RAIL_WIDE_DEFAULT: f32 = 236.;
pub const SIDE_DEFAULT: f32 = 220.;
pub const SIDE_MIN: f32 = 180.;
pub const SIDE_MAX: f32 = 480.;

/// The drawer tab being rested on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DwellTab {
    Nav,
    Pane,
}

/// A drawer tab rested on: it toggles when the timer fires.
pub(crate) struct Dwell {
    pub(crate) which: DwellTab,
    pub(crate) _timer: Task<()>,
}

/// A resizer being dragged: what it resizes and the width at the press.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Resize {
    /// The rail; `moved` once the pointer travelled past 4 pt (else the press toggles it).
    Rail {
        start: f32,
        moved: bool,
    },
    Side {
        start: f32,
    },
    Pane {
        tab: PaneTab,
        start: f32,
    },
}

/// The shell's state ([`crate::register`] puts it in the app state).
pub(crate) struct Shell {
    pub(crate) pages: Pages,
    /// The window's logical width, as the root last laid out.
    pub(crate) width: f32,
    /// The window's logical height, as the root last laid out.
    pub(crate) height: f32,
    // Layout (persisted per device like the web's localStorage keys).
    pub(crate) rail_width: f32,
    pub(crate) rail_wide_width: f32,
    pub(crate) side_width: f32,
    pub(crate) nav_hidden: bool,
    pub(crate) resize: Option<Resize>,
    pub(crate) dwell: Option<Dwell>,
    /// Routes whose pages are open (GPUI's page cache): opened when first shown,
    /// closed when no tab shows them.
    pub(crate) live: Vec<Route>,
    /// The route shown before the latest navigation event.
    pub(crate) shown_route: Route,
    /// Folded sidebar sections by key (`recent`, `types`, `props`).
    pub(crate) section_collapsed: HashMap<String, bool>,
    /// The header's More menu is open.
    pub(crate) more: bool,
    /// The Change type flyout's search.
    pub(crate) type_search: roostr_ui::InputState,
    /// (object id, why it can't be deleted; empty: it can).
    pub(crate) delete_blocker: (String, String),
    /// The delete-property confirmation.
    pub(crate) delete_prop: Option<crate::header::DeleteProp>,
    pub(crate) space_order_moved: bool,
    /// A Personal space is being created for an empty vault.
    pub(crate) seeding_personal: bool,
    /// [`crate::open_when_ready`]'s route, waiting for the space pick.
    pub(crate) pending_route: Option<Route>,
}

impl Default for Shell {
    fn default() -> Self {
        Shell {
            pages: Pages::default(),
            width: 0.0,
            height: 0.0,
            rail_width: RAIL_MIN,
            rail_wide_width: RAIL_WIDE_DEFAULT,
            side_width: SIDE_DEFAULT,
            nav_hidden: false,
            resize: None,
            dwell: None,
            live: Vec::new(),
            shown_route: Route::Home,
            section_collapsed: HashMap::new(),
            more: false,
            type_search: roostr_ui::InputState::new(),
            delete_blocker: (String::new(), String::new()),
            delete_prop: None,
            space_order_moved: false,
            seeding_personal: false,
            pending_route: None,
        }
    }
}

impl Shell {
    /// The shell over `pages` with the layout this device stored.
    pub(crate) fn load(ui: &UiState, pages: Pages) -> Shell {
        let number = |key: &str| {
            ui.get::<serde_json::Value>(key)
                .and_then(|v| json_number(&v))
                .filter(|w| *w > 0.)
        };
        let flag = |key: &str| {
            ui.get::<serde_json::Value>(key)
                .is_some_and(|v| v.as_str() == Some("1") || v.as_bool() == Some(true))
        };
        let rail_width = number("rail-width")
            .unwrap_or(if flag("rail-wide") {
                RAIL_WIDE_DEFAULT
            } else {
                RAIL_MIN
            })
            .clamp(RAIL_MIN, RAIL_MAX);
        Shell {
            pages,
            rail_width,
            rail_wide_width: number("rail-width-wide").unwrap_or(RAIL_WIDE_DEFAULT),
            side_width: number("side-width").unwrap_or(SIDE_DEFAULT),
            nav_hidden: flag("nav-hidden"),
            section_collapsed: ui.get("section-collapsed").unwrap_or_default(),
            ..Shell::default()
        }
    }

    /// The rail is wide (names beside the icons).
    pub(crate) fn rail_wide(&self) -> bool {
        self.rail_width > 100.
    }
}

/// The shell's state, once [`crate::register`] put it there.
pub(crate) fn shell(s: &State) -> Option<&Shell> {
    s.get::<Shell>()
}

/// A persisted number stored as a JSON number or a numeric string (the web stores strings).
pub(crate) fn json_number(v: &serde_json::Value) -> Option<f32> {
    v.as_f64()
        .map(|n| n as f32)
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f32>().ok()))
}

/// The space whose sidebar shows (`current`): the active space, else the first.
pub(crate) fn current_space(s: &State) -> Option<&Space> {
    s.store
        .space(s.nav.space())
        .or_else(|| s.store.channels.first())
}

/// `activeSpace.id || defaultChannelId`.
pub(crate) fn space_id(s: &State) -> String {
    let active = s.nav.space();
    if !active.is_empty() {
        return active.to_string();
    }
    s.store
        .channels
        .first()
        .map(|c| c.id.clone())
        .unwrap_or_default()
}

/// The object open in the main area, if any (`objectId`).
pub(crate) fn open_object_id(s: &State) -> Option<String> {
    match s.nav.route() {
        Route::Object(id) => Some(id.clone()),
        _ => None,
    }
}

/// `name`, or "Untitled" when empty.
pub(crate) fn or_untitled(name: &str) -> String {
    if name.is_empty() {
        "Untitled".to_string()
    } else {
        name.to_string()
    }
}

/// An object/space icon as row text at `style` (an image square for a URL icon).
pub(crate) fn icon_text(s: &State, icon: &str, size: f32, style: TextStyle) -> El {
    match s.get::<Images>() {
        Some(images) => roostr_ui::icon_text(icon, size, style, images),
        None if roostr_ui::is_image_icon(icon) => el().size(size, size),
        None => roostr_ui::emoji_text(icon, style),
    }
}

/// An icon centred in a `size` square: emoji text at 0.9 × size, or the image.
pub(crate) fn emoji_or_image(s: &State, icon: &str, size: f32) -> El {
    match s.get::<Images>() {
        Some(images) => roostr_ui::emoji_or_image_icon(icon, size, images),
        None if roostr_ui::is_image_icon(icon) => el().size(size, size),
        None => row()
            .size(size, size)
            .center()
            .child(roostr_ui::emoji_text(icon, ty::ui(size * 0.9).lh(size))),
    }
}

/// `.obj-icon`: 20 px wide, centred, accent (emoji keep their own colours).
pub(crate) fn obj_icon(s: &State, t: &Theme, icon: &str) -> El {
    row()
        .w(20.)
        .justify_center()
        .items_center()
        .child(icon_text(s, icon, 16., ty::ui(16.)).color(t.accent))
}

/// A phone sheet rising open from `from_y` points below its place on
/// [`roostr_ui::motion::SHEET_OPEN`] (a slight bounce as it lands), gone at once when closed.
pub(crate) fn sheet_rise(from_y: f32) -> rune::Transition {
    rune::Transition {
        insertion: rune::Effect {
            offset: rune::Vec2::new(0.0, from_y),
            ..rune::Effect::IDENTITY
        },
        removal: rune::Effect::IDENTITY,
        springs: rune::SpringPair::new(roostr_ui::motion::SHEET_OPEN, rune::Spring::NONE),
        ..rune::Transition::identity()
    }
}

/// The Builtin mark (`.builtin-mark`): a muted 11 px lock pushed to the row's
/// end, explaining itself in a tooltip.
pub(crate) fn builtin_mark(t: &Theme, key: String) -> El {
    let mark = row()
        .key(key)
        .pl(6.)
        .opacity(0.55)
        .child(glyph(Glyph::Lock, 11.).color(t.muted));
    roostr_ui::tooltip(mark, "Roostr default - can't be deleted")
}

/// One of the shell's bare controls (`fill_button`): transparent at rest,
/// filling `hover` under the pointer (also while pressed). Callers size it,
/// round it, add its content (muted at rest; [`hover_fg`] turns it fg) and its
/// press.
pub(crate) fn fill(key: impl Into<String>, hover: rune::Color) -> El {
    row()
        .key(key)
        .center()
        .cursor(Cursor::Pointer)
        .hover(hover)
        .press(hover)
        .hover_group()
}

/// Content of a [`fill`] control: muted at rest, fg while its control is hovered.
pub(crate) fn hover_fg(e: El, t: &Theme) -> El {
    e.color(t.muted).group_color(t.fg)
}

/// A glyph in a [`fill`] control.
pub(crate) fn fill_glyph(t: &Theme, g: Glyph, size: f32) -> El {
    hover_fg(glyph(g, size), t)
}

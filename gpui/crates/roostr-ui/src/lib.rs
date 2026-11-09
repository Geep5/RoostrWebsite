//! Roostr's design system on Rune: the theme (dark and light palettes, gpui-kit's colours as
//! Roostr configured them, radii, type), fonts, icons, motion, and the shared components the
//! feature crates draw with.
//!
//! Components are builders generic over the app state `S` (Rune handlers are `Fn(&mut S)`),
//! ending in `.build(t)` (`t: &Theme`) or, for anything that floats above the view (menus,
//! popovers, selects, dialogs), `.build(t, &mut layer)` with the overlays mounted at the root by
//! [`root`]. A view starts with [`theme`]:
//!
//! ```ignore
//! fn view(s: &State, cx: &mut Cx<State>) -> El {
//!     let t = roostr_ui::theme(s.ui.theme, cx);
//!     let mut layer = Vec::new();
//!     let body = col().fill(t.bg).child(primary_button("save", "Save").on_press(save).build(t));
//!     roostr_ui::root(body, layer)
//! }
//! ```

pub mod color;
pub mod fonts;
pub mod icons;
pub mod layout;
pub mod motion;
pub mod overlay;
pub mod tags;
pub mod theme;
pub mod tooltip;

pub mod button;
pub mod calendar;
pub mod checkbox;
pub mod color_picker;
pub mod containers;
pub mod dialog;
pub mod display;
pub mod input;
pub mod list;
pub mod media;
pub mod menu;
pub mod popover;
pub mod scroll;
pub mod select;
pub mod switch;
pub mod tabs;
pub mod tag;
pub mod text;

pub use button::*;
pub use calendar::*;
pub use checkbox::*;
pub use color_picker::*;
pub use containers::*;
pub use dialog::*;
pub use display::*;
pub use input::*;
pub use list::*;
pub use media::*;
pub use menu::*;
pub use popover::*;
pub use scroll::*;
pub use select::*;
pub use switch::*;
pub use tabs::*;
pub use tag::*;
pub use text::*;

pub use icons::{Glyph, glyph};
pub use overlay::Placement;
pub use theme::{
    HEADER_H, KitColors, Theme, ThemePref, radius, rems, shadow, switch_motion, theme, ty,
};
pub use tooltip::tooltip;

/// Mounts overlays (menus, popovers, selects, dialogs, their scrims) above `base`, which fills
/// the window. Every view that builds an overlay ends with it.
pub fn root(base: rune::El, overlays: impl IntoIterator<Item = rune::El>) -> rune::El {
    rune_kit::root(base, overlays)
}

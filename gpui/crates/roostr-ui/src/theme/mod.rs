//! Roostr's design tokens (`src/routes/app/+layout.svelte:1669-1686`) and the gpui-kit theme
//! colours the screens read, in dark and light, plus radii, header height and the type scale.
//! Sizes are CSS px = Rune points.
//!
//! The dark palette is today's Roostr, value for value (the GPUI app was dark only). The light
//! palette is new: Apple's light system colours in the same roles, and the same gpui-kit token
//! map over them.
//!
//! A view gets its theme once per run and passes it down:
//!
//! ```ignore
//! fn view(s: &State, cx: &mut Cx<State>) -> El {
//!     let t = roostr_ui::theme(s.ui.theme, cx); // also installs Roostr's tooltip look
//!     ..
//! }
//! ```
//!
//! A handler that changes the preference calls [`switch_motion`] so the colours glide over
//! with Rune's calm theme spring; a system appearance change animates on its own.

mod kit_colors;

use std::sync::LazyLock;

use rune::{Color, Cx, Family, Shadow, TextStyle, Tx};
use rune_kit::{Kit, PaletteFocus, PaletteSeed};

pub use kit_colors::KitColors;

use crate::color::{rgb, rgba};

/// The page header and the Chat / Properties column headers: one height, so the dividers under
/// them line up (18 top + a 30px row + 8 bottom + the 1px divider).
pub const HEADER_H: f32 = 57.0;

/// CSS `line-height: normal` (≈1.2) as a factor of the font size: the line height every Roostr
/// text has unless it sets its own (the GPUI windows set it at their root).
pub const LINE_HEIGHT_NORMAL: f32 = 1.2;

/// gpui-kit's rem: Roostr's kit theme sets `font.size` 14, which the kit's root makes the
/// window's rem size. Every kit spacing and text utility is in rems (`px_2` = 0.5 rem = 7,
/// `h_8` = 2 rem = 28, `text_sm` = 0.875 rem = 12.25), so ported kit metrics go through [`rems`].
pub const REM: f32 = 14.0;

/// `r` rems in points.
pub const fn rems(r: f32) -> f32 {
    r * REM
}

/// Which palette the app shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThemePref {
    /// Today's Roostr look.
    #[default]
    Dark,
    Light,
    /// Follow the system appearance (`Cx::system_dark`).
    System,
}

impl ThemePref {
    /// Whether the dark palette shows, given the system appearance.
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            ThemePref::Dark => true,
            ThemePref::Light => false,
            ThemePref::System => system_dark,
        }
    }

    /// The stored name (`"dark"`, `"light"`, `"system"`).
    pub fn name(self) -> &'static str {
        match self {
            ThemePref::Dark => "dark",
            ThemePref::Light => "light",
            ThemePref::System => "system",
        }
    }

    /// The preference a stored name names; unknown names are the default.
    pub fn from_name(name: &str) -> ThemePref {
        match name {
            "light" => ThemePref::Light,
            "system" => ThemePref::System,
            _ => ThemePref::Dark,
        }
    }
}

/// Every colour Roostr draws with, for one appearance.
#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,
    /// Pane/card background, body, sticky headers.
    pub bg: Color,
    /// Window background behind panes (the gaps read as separators).
    pub gutter: Color,
    /// Raised surfaces: menus, popovers, modals, widget cards, tab strip, callouts, code.
    pub panel: Color,
    /// Hover fill on panel items, table row hover.
    pub hover: Color,
    /// 1px borders, separators, table rules.
    pub border: Color,
    /// Primary text.
    pub fg: Color,
    /// Secondary text, icons, placeholders, table headers.
    pub muted: Color,
    /// Links, focus, selection rows, primary button, badges, checked checkbox.
    pub accent: Color,
    pub green: Color,
    pub orange: Color,
    pub red: Color,
    pub indigo: Color,
    pub white: Color,
    /// Search-match highlight background.
    pub mark: Color,
    /// Faint hover (inactive tab hover, editable cell hover).
    pub hl_light: Color,
    /// Standard sidebar hover/current fill, active tab, create pill.
    pub hl_med: Color,
    /// Editor block selection overlay.
    pub block_selection: Color,
    /// Modal/search overlay.
    pub overlay: Color,
    /// Resize handle while dragging.
    pub resize_active: Color,
    pub lint_warning: Color,
    /// gpui-kit's theme colours as Roostr configured them (`cx.theme().x` → `t.kit.x`).
    pub kit: KitColors,
    /// rune-kit's tokens in this palette, for rune-kit components used as they are
    /// (`rune_kit::shell`, `Table`, `Tree`, scroll bars).
    pub rk: Kit,
}

impl Theme {
    /// Today's Roostr palette.
    pub fn dark() -> &'static Theme {
        &DARK
    }

    /// The light palette.
    pub fn light() -> &'static Theme {
        &LIGHT
    }

    /// The palette for `pref` under a system that is dark or not.
    pub fn for_pref(pref: ThemePref, system_dark: bool) -> &'static Theme {
        if pref.is_dark(system_dark) {
            Theme::dark()
        } else {
            Theme::light()
        }
    }

    /// Text in the UI family at `size` with Roostr's normal line height.
    pub fn text(&self, size: f32) -> TextStyle {
        ty::ui(size)
    }
}

/// The theme for `pref` this run of the view, with Roostr's tooltip look (`El::help` tooltips
/// draw as gpui-kit's `Tooltip`, shortcut keycaps included) and gpui-kit's keyboard focus ring
/// (3px outside the edge, the `ring` colour at 50%) installed for it.
pub fn theme<S>(pref: ThemePref, cx: &mut Cx<S>) -> &'static Theme {
    let t = Theme::for_pref(pref, cx.system_dark());
    cx.help_look(crate::tooltip::help_look(t));
    cx.focus_ring(3.0, 0.0, t.kit.ring.alpha(0.5));
    t
}

/// Make the colour changes of this handler's commit glide (a theme switch).
pub fn switch_motion(tx: &mut Tx) {
    tx.color_motion(crate::motion::THEME_SWITCH);
}

static DARK: LazyLock<Theme> = LazyLock::new(|| {
    build(Palette {
        dark: true,
        bg: rgb(0x1e1e20),
        gutter: rgb(0x141416),
        panel: rgb(0x2b2b2e),
        hover: rgb(0x3a3a3e),
        border: rgb(0x45454a),
        fg: rgb(0xf5f5f7),
        muted: rgb(0x98989d),
        accent: rgb(0x0a84ff),
        green: rgb(0x30d158),
        orange: rgb(0xff9f0a),
        red: rgb(0xff453a),
        indigo: rgb(0x7d7aff),
        mark: rgba(0x0a84ff66),
        hl_light: rgba(0xffffff08),
        hl_med: rgba(0xffffff0d),
        block_selection: rgba(0x377aff40),
        overlay: rgba(0x00000080),
        resize_active: rgb(0x47474d),
        lint_warning: rgb(0xffd60a),
        kit: KitColors::dark(),
    })
});

static LIGHT: LazyLock<Theme> = LazyLock::new(|| {
    build(Palette {
        dark: false,
        bg: rgb(0xffffff),
        gutter: rgb(0xf0f0f2),
        panel: rgb(0xf5f5f7),
        hover: rgb(0xe8e8ed),
        border: rgb(0xd2d2d7),
        fg: rgb(0x1d1d1f),
        muted: rgb(0x6e6e73),
        accent: rgb(0x007aff),
        green: rgb(0x34c759),
        orange: rgb(0xff9500),
        red: rgb(0xff3b30),
        indigo: rgb(0x5856d6),
        mark: rgba(0x007aff40),
        hl_light: rgba(0x00000008),
        hl_med: rgba(0x0000000d),
        block_selection: rgba(0x377aff33),
        overlay: rgba(0x00000033),
        resize_active: rgb(0xc7c7cc),
        lint_warning: rgb(0xffcc00),
        kit: KitColors::light(),
    })
});

struct Palette {
    dark: bool,
    bg: Color,
    gutter: Color,
    panel: Color,
    hover: Color,
    border: Color,
    fg: Color,
    muted: Color,
    accent: Color,
    green: Color,
    orange: Color,
    red: Color,
    indigo: Color,
    mark: Color,
    hl_light: Color,
    hl_med: Color,
    block_selection: Color,
    overlay: Color,
    resize_active: Color,
    lint_warning: Color,
    kit: KitColors,
}

fn build(p: Palette) -> Theme {
    let mut rk = Kit::from_palette_focus(
        PaletteSeed {
            bg: p.bg,
            fg: p.fg,
            accent: p.accent,
        },
        PaletteFocus::Accent,
    );
    {
        let c = &mut rk.c;
        c.page = p.gutter;
        c.bg_base = p.bg;
        c.layer_01 = p.panel;
        c.surface_raised = p.panel;
        c.surface_float = p.panel;
        c.text_base = p.fg;
        c.text_strong = p.fg;
        c.text_muted = p.muted;
        c.text_weak = p.muted;
        c.text_faint = p.muted;
        c.text_accent = p.accent;
        c.icon_base = p.muted;
        c.icon_muted = p.muted;
        c.icon_accent = p.accent;
        c.border_base = p.border;
        c.border_weak = p.border;
        c.std_border = p.border;
        c.border_focus = p.accent;
        c.hover = p.hl_med;
        c.surface_raised_hover = p.hover;
        c.scrim = p.overlay;
    }
    rk.ty = rune_kit::Type {
        ui: ty::ui(13.0),
        ui_medium: ty::ui(13.0).medium(),
        ui_compact: ty::ui(13.0).lh(16.0),
        small: ty::ui(11.0),
        small_medium: ty::ui(11.0).medium(),
        body: ty::ui(14.0),
        body_medium: ty::ui(14.0).medium(),
        text_small: ty::ui(13.0),
        text_small_medium: ty::ui(13.0).medium(),
        text_xsmall: ty::ui(12.0),
        text_xsmall_medium: ty::ui(12.0).medium(),
        mono: ty::mono(13.0),
    };
    rk.radius = rune_kit::Radius {
        xs: radius::XS,
        sm: radius::SM,
        md: radius::MD,
        lg: radius::MENU,
        xl: radius::PANE,
    };
    Theme {
        dark: p.dark,
        bg: p.bg,
        gutter: p.gutter,
        panel: p.panel,
        hover: p.hover,
        border: p.border,
        fg: p.fg,
        muted: p.muted,
        accent: p.accent,
        green: p.green,
        orange: p.orange,
        red: p.red,
        indigo: p.indigo,
        white: rgb(0xffffff),
        mark: p.mark,
        hl_light: p.hl_light,
        hl_med: p.hl_med,
        block_selection: p.block_selection,
        overlay: p.overlay,
        resize_active: p.resize_active,
        lint_warning: p.lint_warning,
        kit: p.kit,
        rk,
    }
}

/// Radius scale.
pub mod radius {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 6.0;
    pub const TAB: f32 = 7.0;
    pub const MD: f32 = 8.0;
    pub const PILL: f32 = 9.0;
    /// Menus, popovers, widget cards, tag pills.
    pub const MENU: f32 = 10.0;
    /// Rail/sidebar panes, small modals.
    pub const PANE: f32 = 12.0;
    pub const MODAL: f32 = 14.0;
    /// Main page card.
    pub const PAGE: f32 = 16.0;
    /// gpui-kit's own radius (`theme.radius`, Roostr set 6) and large radius (10).
    pub const KIT: f32 = 6.0;
    pub const KIT_LG: f32 = 10.0;
}

/// The type scale. Every style is in the UI family (`Family::Sans`, the app's sans slot: the
/// bundled Intern, see [`crate::fonts`]) at Roostr's normal line height (1.2 × size) unless
/// it says otherwise; chain `.medium()`/`.semibold()`/`.bold()`/`.light()` for weights.
pub mod ty {
    use rune::{Family, TextStyle};

    pub const LIGHT: f32 = 300.0;
    pub const NORMAL: f32 = 400.0;
    pub const MEDIUM: f32 = 500.0;
    pub const SEMIBOLD: f32 = 600.0;
    pub const BOLD: f32 = 700.0;

    /// Section labels (11, uppercase in [`crate::section_label`]).
    pub const XS: f32 = 11.0;
    /// Menu notes, captions, tag pills.
    pub const SM: f32 = 12.0;
    /// Menus, buttons, badges, most rows.
    pub const BASE: f32 = 13.0;
    /// gpui-kit's base size (Roostr's kit theme sets `font.size` 14): inputs, kit labels.
    pub const MD: f32 = 14.0;
    pub const LG: f32 = 16.0;

    /// The UI family at `size`, regular, line height 1.2 × size.
    pub const fn ui(size: f32) -> TextStyle {
        TextStyle::new(size, NORMAL).lh(size * super::LINE_HEIGHT_NORMAL)
    }

    /// Code: the mono slot at `size`.
    pub const fn mono(size: f32) -> TextStyle {
        ui(size).family(Family::Mono)
    }

    /// Weight helpers on [`TextStyle`].
    pub trait Weights {
        fn weight(self, w: f32) -> Self;
        fn light(self) -> Self;
        fn medium(self) -> Self;
        fn semibold(self) -> Self;
        fn bold(self) -> Self;
    }

    impl Weights for TextStyle {
        fn weight(self, w: f32) -> Self {
            TextStyle { weight: w, ..self }
        }
        fn light(self) -> Self {
            self.weight(LIGHT)
        }
        fn medium(self) -> Self {
            self.weight(MEDIUM)
        }
        fn semibold(self) -> Self {
            self.weight(SEMIBOLD)
        }
        fn bold(self) -> Self {
            self.weight(BOLD)
        }
    }
}

use ty::Weights as _;

/// Shadows as GPUI drew them. GPUI's shader takes a `BoxShadow`'s `blur_radius` as the
/// Gaussian's standard deviation σ, while Rune's `Shadow::blur` is CSS's blur radius (2σ), so
/// every preset here is the GPUI source value with its blur doubled ([`gpui`]): the same
/// pixels, not the CSS the comments in the GPUI code quote.
pub mod shadow {
    use super::*;

    /// A GPUI `BoxShadow` (offset `y`, `blur_radius` σ, `spread_radius`, black at `alpha`) as a
    /// Rune shadow.
    pub fn gpui(y: f32, blur_radius: f32, spread: f32, alpha: f32) -> Shadow {
        Shadow {
            y,
            blur: 2.0 * blur_radius,
            spread,
            color: Color::BLACK.alpha(alpha),
            ..Shadow::default()
        }
    }

    /// Menus / popovers (`roostr_core::ui::menu_shadow`: y 16, blur 48, 50%).
    pub fn menu() -> Shadow {
        gpui(16.0, 48.0, 0.0, 0.5)
    }
    /// Modals (`modal_shadow`: y 24, blur 80, 60%).
    pub fn modal() -> Shadow {
        gpui(24.0, 80.0, 0.0, 0.6)
    }
    /// Tooltips (`tooltip_shadow`: y 4, blur 14, 25%).
    pub fn tooltip() -> Shadow {
        gpui(4.0, 14.0, 0.0, 0.25)
    }
    /// Lifted drag card (`drag_shadow`: y 10, blur 28, 45%).
    pub fn drag() -> Shadow {
        gpui(10.0, 28.0, 0.0, 0.45)
    }
    /// GPUI's `shadow_xs` (buttons, inputs, checkboxes).
    pub fn kit_xs() -> Shadow {
        gpui(1.0, 2.0, 0.0, 0.05)
    }
    /// GPUI's `shadow_sm`.
    pub fn kit_sm() -> [Shadow; 2] {
        [gpui(1.0, 3.0, 0.0, 0.1), gpui(1.0, 2.0, -1.0, 0.1)]
    }
    /// GPUI's `shadow_md`.
    pub fn kit_md() -> [Shadow; 2] {
        [gpui(4.0, 6.0, -1.0, 0.1), gpui(2.0, 4.0, -2.0, 0.1)]
    }
    /// GPUI's `shadow_lg`.
    pub fn kit_lg() -> [Shadow; 2] {
        [gpui(10.0, 15.0, -3.0, 0.1), gpui(4.0, 6.0, -4.0, 0.1)]
    }
    /// GPUI's `shadow_xl`.
    pub fn kit_xl() -> [Shadow; 2] {
        [gpui(20.0, 25.0, -5.0, 0.1), gpui(8.0, 10.0, -6.0, 0.1)]
    }
    /// GPUI's `shadow_2xl`.
    pub fn kit_2xl() -> Shadow {
        gpui(25.0, 50.0, -12.0, 0.25)
    }
    /// gpui-kit's popup surface (`popover_style`: Popover, PopupMenu, Select, Combobox,
    /// DatePicker): no border, a 1px hairline ring of the foreground at 10% outside the
    /// edge, and shadcn's `shadow-md` at half radii (gpui-kit already halved them for GPUI's σ,
    /// so these blurs are the CSS values again).
    pub fn kit_popover(t: &Theme) -> [Shadow; 3] {
        [
            Shadow {
                spread: 1.0,
                color: t.kit.foreground.alpha(0.1),
                ..Shadow::default()
            },
            gpui(4.0, 3.0, -1.0, 0.1),
            gpui(2.0, 2.0, -2.0, 0.1),
        ]
    }
}

/// The family helpers for text in a named bundled face.
pub fn family(name: &'static str) -> Family {
    Family::Named(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::to_hex;

    #[test]
    fn dark_is_todays_palette() {
        let t = Theme::dark();
        assert!(t.dark);
        assert_eq!(to_hex(t.bg), "#1E1E20");
        assert_eq!(to_hex(t.panel), "#2B2B2E");
        assert_eq!(to_hex(t.accent), "#0A84FF");
        assert!((t.hl_med.a - 13.0 / 255.0).abs() < 1e-6);
        assert_eq!(to_hex(t.kit.popover), "#2B2B2E");
        assert_eq!(to_hex(t.kit.primary_hover), "#2B95FF");
        assert_eq!(to_hex(t.kit.muted_foreground), "#98989D");
    }

    #[test]
    fn prefs_pick_palettes() {
        assert!(Theme::for_pref(ThemePref::Dark, false).dark);
        assert!(!Theme::for_pref(ThemePref::Light, true).dark);
        assert!(Theme::for_pref(ThemePref::System, true).dark);
        assert!(!Theme::for_pref(ThemePref::System, false).dark);
        for p in [ThemePref::Dark, ThemePref::Light, ThemePref::System] {
            assert_eq!(ThemePref::from_name(p.name()), p);
        }
    }
}

//! Roostr's design tokens (`src/routes/app/+layout.svelte:1669-1686`; the app
//! is dark only) and the matching gpui-kit theme, so kit components (inputs,
//! buttons, menus, popovers, scrollbars) sit in the same palette.
//!
//! Sizes are CSS px = GPUI logical px. See `.specs/roostr/style.md` for the
//! full type scale and component specs.

use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeConfig, ThemeMode};
use gpui_kit::{App, Hsla, Pixels, px, rgb, rgba};

/// The page header and the Chat / Properties column headers: one height, so
/// the dividers under them line up (18 top + a 30px row + 8 bottom + the 1px
/// divider).
pub const HEADER_H: f32 = 57.;

/// Pane/card background, body, sticky headers.
pub fn bg() -> Hsla {
    rgb(0x1e1e20).into()
}
/// Window background behind panes (the gaps read as separators).
pub fn gutter() -> Hsla {
    rgb(0x141416).into()
}
/// Raised surfaces: menus, popovers, modals, widget cards, tab strip, callouts, code.
pub fn panel() -> Hsla {
    rgb(0x2b2b2e).into()
}
/// Hover fill on panel items, table row hover.
pub fn hover() -> Hsla {
    rgb(0x3a3a3e).into()
}
/// 1px borders, separators, table rules.
pub fn border() -> Hsla {
    rgb(0x45454a).into()
}
/// Primary text.
pub fn fg() -> Hsla {
    rgb(0xf5f5f7).into()
}
/// Secondary text, icons, placeholders, table headers.
pub fn muted() -> Hsla {
    rgb(0x98989d).into()
}
/// Links, focus, selection rows, primary button, badges, checked checkbox.
pub fn accent() -> Hsla {
    rgb(0x0a84ff).into()
}
pub fn green() -> Hsla {
    rgb(0x30d158).into()
}
pub fn orange() -> Hsla {
    rgb(0xff9f0a).into()
}
pub fn red() -> Hsla {
    rgb(0xff453a).into()
}
pub fn indigo() -> Hsla {
    rgb(0x7d7aff).into()
}
pub fn white() -> Hsla {
    rgb(0xffffff).into()
}
/// Search-match highlight background.
pub fn mark() -> Hsla {
    rgba(0x0a84ff66).into()
}
/// Faint hover (inactive tab hover, editable cell hover).
pub fn hl_light() -> Hsla {
    rgba(0xffffff08).into()
}
/// Standard sidebar hover/current fill, active tab, create pill.
pub fn hl_med() -> Hsla {
    rgba(0xffffff0d).into()
}
/// Editor block selection overlay.
pub fn block_selection() -> Hsla {
    rgba(0x377aff40).into()
}
/// Modal/search overlay.
pub fn overlay() -> Hsla {
    rgba(0x00000080).into()
}
/// Resize handle while dragging.
pub fn resize_active() -> Hsla {
    rgb(0x47474d).into()
}
pub fn lint_warning() -> Hsla {
    rgb(0xffd60a).into()
}

/// Radius scale.
pub mod radius {
    use super::*;
    pub fn xs() -> Pixels {
        px(4.)
    }
    pub fn sm() -> Pixels {
        px(6.)
    }
    pub fn tab() -> Pixels {
        px(7.)
    }
    pub fn md() -> Pixels {
        px(8.)
    }
    pub fn pill() -> Pixels {
        px(9.)
    }
    /// Menus, popovers, widget cards, tag pills.
    pub fn menu() -> Pixels {
        px(10.)
    }
    /// Rail/sidebar panes, small modals.
    pub fn pane() -> Pixels {
        px(12.)
    }
    pub fn modal() -> Pixels {
        px(14.)
    }
    /// Main page card.
    pub fn page() -> Pixels {
        px(16.)
    }
}

/// CSS `line-height: normal` (≈1.2) as a relative line height; GPUI's default is 1.618.
/// Apply at a window root: `.line_height(relative(theme::LINE_HEIGHT_NORMAL))`.
pub const LINE_HEIGHT_NORMAL: f32 = 1.2;

/// Font families. The app UI uses the system font; code uses the system mono.
/// A browser has no system fonts to offer: there the UI is the bundled Inter
/// and code JetBrains Mono. GPUI's iOS text system reads only the system UI
/// font, so code is the bundled JetBrains Mono there too.
pub mod fonts {
    #[cfg(not(target_family = "wasm"))]
    pub const UI: &str = ".SystemUIFont";
    #[cfg(target_family = "wasm")]
    pub const UI: &str = INTER;
    #[cfg(not(any(target_family = "wasm", target_os = "ios")))]
    pub const MONO: &str = "Menlo";
    #[cfg(any(target_family = "wasm", target_os = "ios"))]
    pub const MONO: &str = "JetBrains Mono";
    /// Landing-page headings (bundled).
    pub const ARCHIVO_BLACK: &str = "Archivo Black";
    /// Landing-page body (bundled, variable weight).
    pub const INTER: &str = "Inter";
    /// The browser build's color emoji (Twemoji), once [`super::add_emoji_font`] ran.
    #[cfg(target_family = "wasm")]
    pub const EMOJI: &str = "Twemoji Mozilla";
}

static INTER_TTF: &[u8] = include_bytes!("../../../../static/fonts/Inter.ttf");
static ARCHIVO_BLACK_TTF: &[u8] = include_bytes!("../../../../static/fonts/ArchivoBlack.ttf");
/// The color emoji on iOS: the browser build's Twemoji, embedded. GPUI's
/// iOS text system falls back to it (vendor/gpui-pre-apple) instead of the
/// system's Apple Color Emoji, which it cannot always load there.
#[cfg(target_os = "ios")]
static EMOJI_TTF_IOS: &[u8] = include_bytes!("../fonts/TwemojiMozilla.ttf");

/// The iOS color emoji font's bytes, for the host to register with
/// CoreText before GPUI loads any font.
#[cfg(target_os = "ios")]
pub fn emoji_font_data_ios() -> &'static [u8] {
    EMOJI_TTF_IOS
}
#[cfg(any(target_family = "wasm", target_os = "ios"))]
static MONO_TTF: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
/// GPUI's browser text system resolves `.SystemUIFont` (the default text
/// style, measured before any Roostr view sets a family) to IBM Plex Sans, so
/// that family has to exist before the first window opens.
#[cfg(target_family = "wasm")]
static SYSTEM_UI_TTF: &[u8] = include_bytes!("../fonts/IBMPlexSans-Regular.ttf");
/// The symbols the UI draws that Inter, IBM Plex Sans and JetBrains Mono lack
/// (`▨` `◍` `⌕` `☐` `＋` …): a 4 KB Noto subset the text system falls back to.
/// Desktop and iOS fall back to the system's fonts instead.
#[cfg(target_family = "wasm")]
static SYMBOLS_TTF: &[u8] = include_bytes!("../fonts/RoostrSymbols.ttf");

/// Marks the bundled fonts as registered: [`init`] runs before the browser's
/// pairing gate and again with the app.
struct FontsLoaded;

impl gpui_kit::Global for FontsLoaded {}

/// The fonts the browser build ships, for layout outside GPUI's text
/// system (the graph's SVG labels).
#[cfg(target_family = "wasm")]
pub fn bundled_font_data() -> [&'static [u8]; 5] {
    [
        INTER_TTF,
        ARCHIVO_BLACK_TTF,
        MONO_TTF,
        SYSTEM_UI_TTF,
        SYMBOLS_TTF,
    ]
}

fn bundled_fonts() -> Vec<std::borrow::Cow<'static, [u8]>> {
    #[cfg(not(any(target_family = "wasm", target_os = "ios")))]
    {
        vec![INTER_TTF.into(), ARCHIVO_BLACK_TTF.into()]
    }
    #[cfg(target_os = "ios")]
    {
        vec![
            INTER_TTF.into(),
            ARCHIVO_BLACK_TTF.into(),
            MONO_TTF.into(),
            EMOJI_TTF_IOS.into(),
        ]
    }
    #[cfg(target_family = "wasm")]
    {
        bundled_font_data().map(Into::into).to_vec()
    }
}

/// Where the browser build fetches its color emoji font, under the page's
/// base path: `fonts/TwemojiMozilla.ttf` (a COLR build of Twemoji, see
/// `fonts/TwemojiMozilla-LICENSE.md`), which `scripts/build-web.sh` copies
/// beside the module. Fetched after the app starts rather than embedded, so
/// its 1.2 MB do not delay the first frame; until it arrives emoji come from
/// the browser's own fonts. The desktop uses the system's Apple Color Emoji.
#[cfg(target_family = "wasm")]
pub const EMOJI_FONT_PATH: &str = "fonts/TwemojiMozilla.ttf";

/// The fetched emoji font; the static owns it, so both text engines borrow it.
#[cfg(target_family = "wasm")]
static EMOJI_TTF: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

/// Set once the emoji font is in the text system: observe it to lay out
/// text drawn outside GPUI's text system again (the graph's labels).
#[cfg(target_family = "wasm")]
pub struct EmojiFontLoaded;

#[cfg(target_family = "wasm")]
impl gpui_kit::Global for EmojiFontLoaded {}

/// The emoji font, once loaded, for layout outside GPUI's text system.
#[cfg(target_family = "wasm")]
pub fn emoji_font_data() -> Option<&'static [u8]> {
    EMOJI_TTF.get().map(Vec::as_slice)
}

/// Add the fetched emoji font and redraw.
#[cfg(target_family = "wasm")]
pub fn add_emoji_font(bytes: Vec<u8>, cx: &mut App) {
    if EMOJI_TTF.set(bytes).is_err() {
        return;
    }
    let Some(data) = emoji_font_data() else {
        return;
    };
    if let Err(error) = cx.text_system().add_fonts(vec![data.into()]) {
        log::warn!("could not load the emoji font: {error:#}");
        return;
    }
    cx.set_global(EmojiFontLoaded);
    cx.refresh_windows();
}

/// The gpui-kit theme in Roostr's palette.
const KIT_THEME: &str = r##"{
  "name": "Roostr Dark",
  "mode": "dark",
  "font.size": 14,
  "radius": 6,
  "radius.lg": 10,
  "shadow": true,
  "colors": {
    "background": "#1e1e20",
    "foreground": "#f5f5f7",
    "border": "#45454a",
    "accent.background": "#3a3a3e",
    "accent.foreground": "#f5f5f7",
    "accordion.background": "#1e1e20",
    "caret": "#f5f5f7",
    "danger.background": "#ff453a",
    "danger.foreground": "#ffffff",
    "drag_border": "#0a84ff",
    "drop_target.background": "#0a84ff22",
    "group_box.background": "#2b2b2e",
    "group_box.foreground": "#f5f5f7",
    "info.background": "#0a84ff",
    "info.foreground": "#ffffff",
    "input.border": "#45454a",
    "link.foreground": "#0a84ff",
    "link.hover.foreground": "#3a9bff",
    "link.active.foreground": "#0a76e6",
    "list.background": "#1e1e20",
    "list.active.background": "#0a84ff33",
    "list.active.border": "#0a84ff",
    "list.even.background": "#00000000",
    "list.head.background": "#2b2b2e",
    "muted.background": "#2b2b2e",
    "muted.foreground": "#98989d",
    "overlay": "#00000080",
    "popover.background": "#2b2b2e",
    "popover.foreground": "#f5f5f7",
    "primary.background": "#0a84ff",
    "primary.hover.background": "#2b95ff",
    "primary.active.background": "#0a76e6",
    "primary.foreground": "#ffffff",
    "ring": "#0a84ff",
    "scrollbar.background": "#00000000",
    "scrollbar.thumb.background": "#ffffff29",
    "scrollbar.thumb.hover.background": "#ffffff52",
    "secondary.background": "#2b2b2e",
    "secondary.hover.background": "#3a3a3e",
    "secondary.active.background": "#45454a",
    "secondary.foreground": "#f5f5f7",
    "selection.background": "#0a84ff66",
    "sidebar.background": "#1e1e20",
    "sidebar.foreground": "#f5f5f7",
    "sidebar.border": "#45454a",
    "sidebar.accent.background": "#ffffff0d",
    "sidebar.accent.foreground": "#f5f5f7",
    "sidebar.primary.background": "#0a84ff",
    "sidebar.primary.foreground": "#ffffff",
    "skeleton.background": "#2b2b2e",
    "success.background": "#30d158",
    "success.foreground": "#ffffff",
    "switch.background": "#45454a",
    "switch.thumb.background": "#ffffff",
    "tab.background": "#00000000",
    "tab.active.background": "#ffffff0d",
    "tab.active.foreground": "#f5f5f7",
    "tab.foreground": "#98989d",
    "tab_bar.background": "#2b2b2e",
    "table.background": "#1e1e20",
    "table.head.foreground": "#98989d",
    "table.row.border": "#45454a",
    "title_bar.background": "#141416",
    "title_bar.border": "#141416",
    "warning.background": "#ff9f0a",
    "warning.foreground": "#1b1b1b",
    "window.border": "#45454a"
  }
}"##;

/// Register the bundled fonts and switch gpui-kit to the Roostr palette.
/// Call after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    if !cx.has_global::<FontsLoaded>() {
        if let Err(error) = cx.text_system().add_fonts(bundled_fonts()) {
            log::warn!("could not load bundled fonts: {error:#}");
        }
        cx.set_global(FontsLoaded);
    }
    Theme::change(ThemeMode::Dark, None, cx);
    let config: ThemeConfig = serde_json::from_str(KIT_THEME).expect("valid Roostr kit theme");
    let theme = Theme::global_mut(cx);
    theme.apply_config(&Rc::new(config));
    if cfg!(any(target_family = "wasm", target_os = "ios")) {
        // The kit's defaults name desktop families the browser (and GPUI's
        // iOS text system) do not have.
        theme.font_family = fonts::UI.into();
        theme.mono_font_family = fonts::MONO.into();
    }
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kit_theme_parses() {
        let config: ThemeConfig = serde_json::from_str(KIT_THEME).unwrap();
        assert!(config.mode.is_dark());
    }
}

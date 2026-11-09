//! Fonts for an app's `AppSpec`: `AppSpec { fonts: fonts::FONTS, families: fonts::FAMILIES, .. }`.
//!
//! The UI is Intern everywhere (a fully rounded Inter, `fonts/Intern.ttf`, variable weight and
//! optical size, OFL: `fonts/Intern-OFL.txt`). It covers Latin only, so Inter follows it for
//! Greek and Cyrillic. Code is Menlo on macOS; elsewhere the bundled JetBrains Mono, with the
//! symbols those lack (`▨` `◍` `⌕` `☐` `＋` …) from the 4 KB Roostr Symbols subset. Archivo Black
//! (landing-page headings) and Inter (landing-page body) are bundled everywhere, named by
//! [`ARCHIVO_BLACK`] and [`INTER`].
//!
//! Emoji: on the desktop Rune falls back to the system's colour emoji (Apple Color Emoji on a
//! Mac). An iPhone app can't read the system's font files, so iOS embeds Twemoji, as the GPUI
//! iPhone app did. The browser build draws Twemoji too, fetched beside the module rather than
//! embedded so its 1.2 MB don't hold up the first frame: on wasm call
//! `app.font_url(EMOJI_FONT_PATH)` on the `rune::App` (Rune fetches it before the first frame
//! and adds it to the fonts); the [`FAMILIES`] lists already name it. `scripts/build-web.sh`
//! copies `crates/roostr-ui/fonts/TwemojiMozilla.ttf` there.

static INTER_TTF: &[u8] = include_bytes!("../../../../static/fonts/Inter.ttf");
static INTERN_TTF: &[u8] = include_bytes!("../fonts/Intern.ttf");
static ARCHIVO_BLACK_TTF: &[u8] = include_bytes!("../../../../static/fonts/ArchivoBlack.ttf");
#[cfg(not(target_os = "macos"))]
static MONO_TTF: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
#[cfg(not(target_os = "macos"))]
static SYMBOLS_TTF: &[u8] = include_bytes!("../fonts/RoostrSymbols.ttf");
#[cfg(target_os = "ios")]
static EMOJI_TTF: &[u8] = include_bytes!("../fonts/TwemojiMozilla.ttf");

/// The UI family (CSS family list).
#[cfg(target_os = "macos")]
pub const UI: &str = "Intern, Inter";
#[cfg(not(target_os = "macos"))]
pub const UI: &str = "Intern, Inter, Roostr Symbols, Twemoji Mozilla";
/// Code.
#[cfg(target_os = "macos")]
pub const MONO: &str = "Menlo";
#[cfg(not(target_os = "macos"))]
pub const MONO: &str = "JetBrains Mono, Roostr Symbols, Twemoji Mozilla";
/// Landing-page headings (bundled).
pub const ARCHIVO_BLACK: &str = "Archivo Black";
/// Landing-page body (bundled, variable weight).
pub const INTER: &str = "Inter";
/// The browser build's colour emoji, once [`EMOJI_FONT_PATH`] loaded.
pub const EMOJI: &str = "Twemoji Mozilla";

/// `rune::Family`s for text styles that name a bundled face.
pub mod family {
    use rune::Family;

    pub const ARCHIVO_BLACK: Family = Family::Named(super::ARCHIVO_BLACK);
    pub const INTER: Family = Family::Named(super::INTER);
}

/// The faces every build registers (`AppSpec::fonts`). They replace Rune's built-in Inter.
#[cfg(target_os = "macos")]
pub static FONTS: &[&[u8]] = &[INTERN_TTF, INTER_TTF, ARCHIVO_BLACK_TTF];
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub static FONTS: &[&[u8]] = &[INTERN_TTF, INTER_TTF, ARCHIVO_BLACK_TTF, MONO_TTF, SYMBOLS_TTF];
#[cfg(target_os = "ios")]
pub static FONTS: &[&[u8]] =
    &[INTERN_TTF, INTER_TTF, ARCHIVO_BLACK_TTF, MONO_TTF, SYMBOLS_TTF, EMOJI_TTF];

/// The sans and mono slots (`AppSpec::families`).
pub const FAMILIES: (&str, &str) = (UI, MONO);

/// Where the browser build fetches Twemoji (a COLR build, see
/// `fonts/TwemojiMozilla-LICENSE.md`), relative to the page: `App::font_url(EMOJI_FONT_PATH)`.
pub const EMOJI_FONT_PATH: &str = "fonts/TwemojiMozilla.ttf";

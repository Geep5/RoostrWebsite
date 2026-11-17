//! Fonts for an app's `AppSpec`: `AppSpec { fonts: fonts::FONTS, families: fonts::FAMILIES, .. }`.
//!
//! The UI font is the person's pick ([`UiFont`], Settings › Appearance): IBM Plex Sans by
//! default (`fonts/IBMPlexSans-*.ttf`, OFL: `fonts/IBMPlexSans-OFL.txt`), or Intern (a fully
//! rounded Inter, `fonts/Intern.ttf`, OFL: `fonts/Intern-OFL.txt`), Geist (Vercel's,
//! `fonts/Geist*.ttf`, OFL: `fonts/Geist-OFL.txt`), Allo (Josh Puckett's, `fonts/Allo.ttf`,
//! OFL: `fonts/Allo-OFL.txt`), Inter, or Helvetica (the system's, Arial where it has none).
//! Inter follows each for scripts it lacks.
//! Code is Menlo on macOS; elsewhere the bundled JetBrains Mono, with the
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
static PLEX_REGULAR_TTF: &[u8] = include_bytes!("../fonts/IBMPlexSans-Regular.ttf");
static PLEX_ITALIC_TTF: &[u8] = include_bytes!("../fonts/IBMPlexSans-Italic.ttf");
static PLEX_MEDIUM_TTF: &[u8] = include_bytes!("../fonts/IBMPlexSans-Medium.ttf");
static PLEX_SEMIBOLD_TTF: &[u8] = include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf");
static PLEX_BOLD_TTF: &[u8] = include_bytes!("../fonts/IBMPlexSans-Bold.ttf");
static GEIST_TTF: &[u8] = include_bytes!("../fonts/Geist.ttf");
static GEIST_ITALIC_TTF: &[u8] = include_bytes!("../fonts/Geist-Italic.ttf");
static ALLO_TTF: &[u8] = include_bytes!("../fonts/Allo.ttf");
static ARCHIVO_BLACK_TTF: &[u8] = include_bytes!("../../../../static/fonts/ArchivoBlack.ttf");
#[cfg(not(target_os = "macos"))]
static MONO_TTF: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
#[cfg(not(target_os = "macos"))]
static SYMBOLS_TTF: &[u8] = include_bytes!("../fonts/RoostrSymbols.ttf");
#[cfg(target_os = "ios")]
static EMOJI_TTF: &[u8] = include_bytes!("../fonts/TwemojiMozilla.ttf");

/// What follows the picked UI font: Inter for scripts it lacks, and off macOS the bundled
/// symbols and emoji.
#[cfg(target_os = "macos")]
const UI_FALLBACK: &str = "Inter";
#[cfg(not(target_os = "macos"))]
const UI_FALLBACK: &str = "Inter, Roostr Symbols, Twemoji Mozilla";

/// The UI fonts a person can pick (Settings › Appearance), by their stored name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiFont {
    #[default]
    Plex,
    Intern,
    Geist,
    Allo,
    Inter,
    Helvetica,
}

impl UiFont {
    pub const ALL: [UiFont; 6] = [
        UiFont::Plex,
        UiFont::Intern,
        UiFont::Geist,
        UiFont::Allo,
        UiFont::Inter,
        UiFont::Helvetica,
    ];

    /// The stored name (`UiState` key `"font"`); unknown names read as the default.
    pub fn from_name(name: &str) -> UiFont {
        UiFont::ALL
            .into_iter()
            .find(|f| f.name() == name)
            .unwrap_or_default()
    }
    pub fn name(self) -> &'static str {
        match self {
            UiFont::Plex => "plex",
            UiFont::Intern => "intern",
            UiFont::Geist => "geist",
            UiFont::Allo => "allo",
            UiFont::Inter => "inter",
            UiFont::Helvetica => "helvetica",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            UiFont::Plex => "IBM Plex Sans",
            UiFont::Intern => "Intern",
            UiFont::Geist => "Geist",
            UiFont::Allo => "Allo",
            UiFont::Inter => "Inter",
            UiFont::Helvetica => "Helvetica",
        }
    }
    /// The sans slot's family list for this font.
    pub fn families(self) -> String {
        let first = match self {
            UiFont::Plex => "IBM Plex Sans",
            UiFont::Intern => "Intern",
            UiFont::Geist => "Geist",
            UiFont::Allo => "Allo Variable",
            UiFont::Inter => "Inter",
            UiFont::Helvetica => "Helvetica Neue, Helvetica, Arial",
        };
        format!("{first}, {UI_FALLBACK}")
    }
}

/// The UI family (CSS family list) at launch: the default [`UiFont`].
#[cfg(target_os = "macos")]
pub const UI: &str = "IBM Plex Sans, Inter";
#[cfg(not(target_os = "macos"))]
pub const UI: &str = "IBM Plex Sans, Inter, Roostr Symbols, Twemoji Mozilla";
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
pub static FONTS: &[&[u8]] = &[
    PLEX_REGULAR_TTF,
    PLEX_ITALIC_TTF,
    PLEX_MEDIUM_TTF,
    PLEX_SEMIBOLD_TTF,
    PLEX_BOLD_TTF,
    GEIST_TTF,
    GEIST_ITALIC_TTF,
    ALLO_TTF,
    INTERN_TTF,
    INTER_TTF,
    ARCHIVO_BLACK_TTF,
];
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub static FONTS: &[&[u8]] = &[
    PLEX_REGULAR_TTF,
    PLEX_ITALIC_TTF,
    PLEX_MEDIUM_TTF,
    PLEX_SEMIBOLD_TTF,
    PLEX_BOLD_TTF,
    GEIST_TTF,
    GEIST_ITALIC_TTF,
    ALLO_TTF,
    INTERN_TTF,
    INTER_TTF,
    ARCHIVO_BLACK_TTF,
    MONO_TTF,
    SYMBOLS_TTF,
];
#[cfg(target_os = "ios")]
pub static FONTS: &[&[u8]] = &[
    PLEX_REGULAR_TTF,
    PLEX_ITALIC_TTF,
    PLEX_MEDIUM_TTF,
    PLEX_SEMIBOLD_TTF,
    PLEX_BOLD_TTF,
    GEIST_TTF,
    GEIST_ITALIC_TTF,
    ALLO_TTF,
    INTERN_TTF,
    INTER_TTF,
    ARCHIVO_BLACK_TTF,
    MONO_TTF,
    SYMBOLS_TTF,
    EMOJI_TTF,
];

/// The sans and mono slots (`AppSpec::families`).
pub const FAMILIES: (&str, &str) = (UI, MONO);

/// Where the browser build fetches Twemoji (a COLR build, see
/// `fonts/TwemojiMozilla-LICENSE.md`), relative to the page: `App::font_url(EMOJI_FONT_PATH)`.
pub const EMOJI_FONT_PATH: &str = "fonts/TwemojiMozilla.ttf";

#[cfg(test)]
mod tests {
    use super::*;
    use rune::text::TextSystem;
    use rune::{Family, TextStyle};

    fn width(sans: &str, weight: f32) -> f32 {
        let mut text = TextSystem::new(FONTS, (sans, MONO));
        let style = TextStyle {
            family: Family::Sans,
            ..TextStyle::new(16.0, weight)
        };
        text.measure("Hamburgefonstiv 0123456789", style, 1.0).0
    }

    /// IBM Plex Sans draws in its own face: a family name that doesn't match its files would
    /// quietly fall back to Inter. (Intern is Inter's metrics with rounded shapes, so width
    /// can't tell those two apart.)
    #[test]
    fn ibm_plex_sans_draws_its_own_face() {
        let plex = UiFont::Plex.families();
        assert_ne!(
            width(&plex, 400.0),
            width("Inter", 400.0),
            "IBM Plex Sans fell back to Inter"
        );
        for font in [UiFont::Geist, UiFont::Allo] {
            assert_ne!(
                width(&font.families(), 400.0),
                width("Inter", 400.0),
                "{} fell back to Inter",
                font.label()
            );
        }
        // Plex's bold is its own face, not a synthesized regular.
        assert_ne!(width(&plex, 700.0), width(&plex, 400.0));
    }

    #[test]
    fn the_default_and_unknown_picks_are_plex() {
        assert_eq!(UiFont::from_name(""), UiFont::Plex);
        assert_eq!(UiFont::from_name("comic sans"), UiFont::Plex);
        for f in UiFont::ALL {
            assert_eq!(UiFont::from_name(f.name()), f);
        }
        assert!(
            UI.starts_with("IBM Plex Sans"),
            "the launch font is the default pick"
        );
    }
}

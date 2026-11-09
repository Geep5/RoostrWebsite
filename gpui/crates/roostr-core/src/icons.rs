//! Roostr's icon set (`Icon`, `PropIcon`, `LayoutIcon`, `PaneIcon`,
//! `GraphIcon`, `CheckboxIcon` from the web app) as SVG assets embedded in
//! the binary. A [`Glyph`] names one; [`Glyph::asset_path`] is its SVG in
//! [`Assets`]. SVGs render tinted by the text color.

use std::borrow::Cow;

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Embedded;

/// Roostr's own assets (`icons/<name>.svg`), compiled into the binary.
pub struct Assets;

impl Assets {
    /// The bytes at `path` (`icons/plus.svg`), if embedded.
    pub fn get(path: &str) -> Option<Cow<'static, [u8]>> {
        Embedded::get(path).map(|f| f.data)
    }

    /// Every embedded path under `prefix`.
    pub fn list(prefix: &str) -> Vec<String> {
        Embedded::iter()
            .filter(|p| p.starts_with(prefix))
            .map(|p| p.into_owned())
            .collect()
    }
}

macro_rules! glyphs {
    ($($variant:ident => $file:literal,)*) => {
        /// One Roostr icon. `Prop*` are property-format line icons (16px),
        /// `Badge*` the filled property badge glyphs, `Layout*` the view layouts.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Glyph {
            $($variant,)*
        }

        impl Glyph {
            pub const ALL: &[Glyph] = &[$(Glyph::$variant,)*];

            /// The file name without extension, as in the web app's icon name.
            pub fn name(self) -> &'static str {
                match self {
                    $(Glyph::$variant => $file,)*
                }
            }

            /// Look an icon up by its web name (`"chevron-right"`, `"prop-tag"`).
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($file => Some(Glyph::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

glyphs! {
    AlignLeft => "align-left",
    ArrowLeftRight => "arrow-left-right",
    ArrowUpRight => "arrow-up-right",
    BadgeAt => "badge-at",
    BadgeCalendar => "badge-calendar",
    BadgeCheck => "badge-check",
    BadgeClock => "badge-clock",
    BadgeDashed => "badge-dashed",
    BadgeDot => "badge-dot",
    BadgeHalf => "badge-half",
    BadgeLink => "badge-link",
    BadgePhone => "badge-phone",
    BadgeX => "badge-x",
    Box => "box",
    Check => "check",
    CheckboxChecked => "checkbox-checked",
    CheckboxUnchecked => "checkbox-unchecked",
    ChevronDown => "chevron-down",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    ChevronsRight => "chevrons-right",
    Compose => "compose",
    Copy => "copy",
    Disclosure => "disclosure",
    Download => "download",
    DvFilter => "dv-filter",
    DvSearch => "dv-search",
    DvSettings => "dv-settings",
    DvSort => "dv-sort",
    Eraser => "eraser",
    ExternalLink => "external-link",
    FolderMinus => "folder-minus",
    FolderPlus => "folder-plus",
    Graph => "graph",
    Hash => "hash",
    LayoutCalendar => "layout-calendar",
    LayoutGallery => "layout-gallery",
    LayoutKanban => "layout-kanban",
    LayoutTable => "layout-table",
    Lock => "lock",
    MobileBack => "m-back",
    NavBack => "nav-back",
    NavForward => "nav-forward",
    PaintBucket => "paint-bucket",
    Palette => "palette",
    Pane => "pane",
    Pencil => "pencil",
    Person => "person",
    Plus => "plus",
    PropBox => "prop-box",
    PropBrain => "prop-brain",
    PropCheckbox => "prop-checkbox",
    PropClock => "prop-clock",
    PropColor => "prop-color",
    PropComputer => "prop-computer",
    PropCpu => "prop-cpu",
    PropDate => "prop-date",
    PropEmail => "prop-email",
    PropFile => "prop-file",
    PropFolder => "prop-folder",
    PropGlobe => "prop-globe",
    PropKey => "prop-key",
    PropNumber => "prop-number",
    PropObject => "prop-object",
    PropPerson => "prop-person",
    PropPhone => "prop-phone",
    PropRepeat => "prop-repeat",
    PropSearch => "prop-search",
    PropSkill => "prop-skill",
    PropStatus => "prop-status",
    PropTag => "prop-tag",
    PropText => "prop-text",
    PropTool => "prop-tool",
    PropUrl => "prop-url",
    PropWarning => "prop-warning",
    Search => "search",
    Settings => "settings",
    SlidersHorizontal => "sliders-horizontal",
    SortArrow => "sort-arrow",
    SortArrowDown => "sort-arrow-down",
    StarOff => "star-off",
    Star => "star",
    Strikethrough => "strikethrough",
    Trash => "trash",
    Type => "type",
}

impl Glyph {
    /// The icon's SVG in [`Assets`].
    pub fn asset_path(self) -> String {
        format!("icons/{}.svg", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_has_an_embedded_svg() {
        for glyph in Glyph::ALL {
            let path = glyph.asset_path();
            let bytes = Assets::get(&path).unwrap_or_else(|| panic!("missing {path}"));
            assert!(bytes.starts_with(b"<svg"), "{path} is not an svg");
            // Compiled into the binary (borrowed), not read from the source
            // tree at run time (owned): a debug build without rust-embed's
            // debug-embed reads `assets/` from disk, which a browser tab or a
            // phone does not have, and drew every icon blank.
            assert!(
                matches!(bytes, Cow::Borrowed(_)),
                "{path} is read from disk, not embedded"
            );
        }
    }
}

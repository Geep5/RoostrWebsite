//! Roostr's icon set as Rune icons: the web app's `Icon`, `PropIcon`, `LayoutIcon`,
//! `PaneIcon`, `GraphIcon` and `CheckboxIcon` SVGs, plus the gpui-kit (Lucide) icons the
//! screens and kit components drew (`Kit*`, `Close`, `Loader`, …), all in `assets/icons`.
//!
//! `glyph(Glyph::Plus, 16.0).color(t.muted)` draws one at any size in any colour; an icon
//! takes the colour of its element, as `currentColor`.

use rune::{El, Icon};

mod svg {
    include!(concat!(env!("OUT_DIR"), "/icons.rs"));
}

macro_rules! glyphs {
    ($($variant:ident => $file:literal => $icon:ident,)*) => {
        /// One Roostr icon. `Prop*` are property-format line icons (16px), `Badge*` the filled
        /// property badge glyphs, `Layout*` the view layouts, `Kit*` gpui-kit's Lucide versions
        /// of icons Roostr also draws its own way.
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

            /// The Rune icon (`rune::icon(g.icon(), size)`, `InlineIcon::glyph(g.icon())`).
            pub fn icon(self) -> &'static Icon {
                match self {
                    $(Glyph::$variant => &svg::$icon,)*
                }
            }
        }
    };
}

glyphs! {
    AlignLeft => "align-left" => ALIGN_LEFT,
    ArrowLeftRight => "arrow-left-right" => ARROW_LEFT_RIGHT,
    ArrowUpRight => "arrow-up-right" => ARROW_UP_RIGHT,
    BadgeAt => "badge-at" => BADGE_AT,
    BadgeCalendar => "badge-calendar" => BADGE_CALENDAR,
    BadgeCheck => "badge-check" => BADGE_CHECK,
    BadgeClock => "badge-clock" => BADGE_CLOCK,
    BadgeDashed => "badge-dashed" => BADGE_DASHED,
    BadgeDot => "badge-dot" => BADGE_DOT,
    BadgeHalf => "badge-half" => BADGE_HALF,
    BadgeLink => "badge-link" => BADGE_LINK,
    BadgePhone => "badge-phone" => BADGE_PHONE,
    BadgeX => "badge-x" => BADGE_X,
    Box => "box" => BOX,
    Check => "check" => CHECK,
    CheckboxChecked => "checkbox-checked" => CHECKBOX_CHECKED,
    CheckboxUnchecked => "checkbox-unchecked" => CHECKBOX_UNCHECKED,
    ChevronDown => "chevron-down" => CHEVRON_DOWN,
    ChevronLeft => "chevron-left" => CHEVRON_LEFT,
    ChevronRight => "chevron-right" => CHEVRON_RIGHT,
    ChevronsRight => "chevrons-right" => CHEVRONS_RIGHT,
    Compose => "compose" => COMPOSE,
    Copy => "copy" => COPY,
    Disclosure => "disclosure" => DISCLOSURE,
    Download => "download" => DOWNLOAD,
    DvFilter => "dv-filter" => DV_FILTER,
    DvSearch => "dv-search" => DV_SEARCH,
    DvSettings => "dv-settings" => DV_SETTINGS,
    DvSort => "dv-sort" => DV_SORT,
    Eraser => "eraser" => ERASER,
    ExternalLink => "external-link" => EXTERNAL_LINK,
    FolderMinus => "folder-minus" => FOLDER_MINUS,
    FolderPlus => "folder-plus" => FOLDER_PLUS,
    Graph => "graph" => GRAPH,
    Hash => "hash" => HASH,
    LayoutCalendar => "layout-calendar" => LAYOUT_CALENDAR,
    LayoutGallery => "layout-gallery" => LAYOUT_GALLERY,
    LayoutKanban => "layout-kanban" => LAYOUT_KANBAN,
    LayoutTable => "layout-table" => LAYOUT_TABLE,
    Lock => "lock" => LOCK,
    MobileBack => "m-back" => M_BACK,
    NavBack => "nav-back" => NAV_BACK,
    NavForward => "nav-forward" => NAV_FORWARD,
    PaintBucket => "paint-bucket" => PAINT_BUCKET,
    Palette => "palette" => PALETTE,
    Pane => "pane" => PANE,
    Pencil => "pencil" => PENCIL,
    Person => "person" => PERSON,
    Plus => "plus" => PLUS,
    PropBox => "prop-box" => PROP_BOX,
    PropBrain => "prop-brain" => PROP_BRAIN,
    PropCheckbox => "prop-checkbox" => PROP_CHECKBOX,
    PropClock => "prop-clock" => PROP_CLOCK,
    PropColor => "prop-color" => PROP_COLOR,
    PropComputer => "prop-computer" => PROP_COMPUTER,
    PropCpu => "prop-cpu" => PROP_CPU,
    PropDate => "prop-date" => PROP_DATE,
    PropEmail => "prop-email" => PROP_EMAIL,
    PropFile => "prop-file" => PROP_FILE,
    PropFolder => "prop-folder" => PROP_FOLDER,
    PropGlobe => "prop-globe" => PROP_GLOBE,
    PropKey => "prop-key" => PROP_KEY,
    PropNumber => "prop-number" => PROP_NUMBER,
    PropObject => "prop-object" => PROP_OBJECT,
    PropPerson => "prop-person" => PROP_PERSON,
    PropPhone => "prop-phone" => PROP_PHONE,
    PropRepeat => "prop-repeat" => PROP_REPEAT,
    PropSearch => "prop-search" => PROP_SEARCH,
    PropSkill => "prop-skill" => PROP_SKILL,
    PropStatus => "prop-status" => PROP_STATUS,
    PropTag => "prop-tag" => PROP_TAG,
    PropText => "prop-text" => PROP_TEXT,
    PropTool => "prop-tool" => PROP_TOOL,
    PropUrl => "prop-url" => PROP_URL,
    PropWarning => "prop-warning" => PROP_WARNING,
    Search => "search" => SEARCH,
    Settings => "settings" => SETTINGS,
    SlidersHorizontal => "sliders-horizontal" => SLIDERS_HORIZONTAL,
    SortArrow => "sort-arrow" => SORT_ARROW,
    SortArrowDown => "sort-arrow-down" => SORT_ARROW_DOWN,
    StarOff => "star-off" => STAR_OFF,
    Star => "star" => STAR,
    Strikethrough => "strikethrough" => STRIKETHROUGH,
    Trash => "trash" => TRASH,
    Type => "type" => TYPE,
    // gpui-kit's (Lucide) icons, as `IconName::*` drew them.
    Asterisk => "asterisk" => ASTERISK,
    BookOpen => "book-open" => BOOK_OPEN,
    Calendar => "calendar" => CALENDAR,
    CaseSensitive => "case-sensitive" => CASE_SENSITIVE,
    CircleCheck => "circle-check" => CIRCLE_CHECK,
    CircleX => "circle-x" => CIRCLE_X,
    Close => "close" => CLOSE,
    Ellipsis => "ellipsis" => ELLIPSIS,
    Eye => "eye" => EYE,
    EyeOff => "eye-off" => EYE_OFF,
    FileText => "file-text" => FILE_TEXT,
    Inbox => "inbox" => INBOX,
    Info => "info" => INFO,
    KitArrowUp => "kit-arrow-up" => KIT_ARROW_UP,
    KitChevronDown => "kit-chevron-down" => KIT_CHEVRON_DOWN,
    KitCopy => "kit-copy" => KIT_COPY,
    KitExternalLink => "kit-external-link" => KIT_EXTERNAL_LINK,
    KitSearch => "kit-search" => KIT_SEARCH,
    Loader => "loader" => LOADER,
    Minus => "minus" => MINUS,
    Play => "play" => PLAY,
    Replace => "replace" => REPLACE,
    TriangleAlert => "triangle-alert" => TRIANGLE_ALERT,
    User => "user" => USER,
}

/// `g` as a `size` × `size` icon element; set its colour with `.color(..)`.
pub fn glyph(g: Glyph, size: f32) -> El {
    rune::icon(g.icon(), size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_has_a_body_and_names_round_trip() {
        for &g in Glyph::ALL {
            assert_eq!(Glyph::from_name(g.name()), Some(g));
            let icon = g.icon();
            assert_eq!(icon.name, g.name());
            assert!(icon.view > 0.0 && icon.body.len() > 10, "{}", g.name());
        }
    }

    #[test]
    fn every_svg_file_is_a_glyph() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|x| x == "svg") {
                let name = path.file_stem().unwrap().to_str().unwrap();
                assert!(Glyph::from_name(name).is_some(), "{name}.svg has no Glyph");
            }
        }
    }

    #[test]
    fn masked_badges_draw_through_the_full_svg_path() {
        // A mask cuts the badge's mark out; Rune applies masks only to bodies naming colours.
        for g in [Glyph::BadgeCheck, Glyph::BadgeClock, Glyph::BadgeX] {
            let body = g.icon().body;
            assert!(
                body.contains("fill=\"#fff\"") && body.contains("currentColor"),
                "{body}"
            );
        }
    }
}

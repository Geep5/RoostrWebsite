//! Roostr's icon set (`Icon`, `PropIcon`, `LayoutIcon`, `PaneIcon`,
//! `GraphIcon`, `CheckboxIcon` from the web app) as SVG assets, and the app's
//! [`AssetSource`] that serves them alongside gpui-kit's own icons.
//!
//! Use a [`Glyph`] anywhere gpui-kit takes an icon: `Icon::new(Glyph::Plus)`,
//! `Button::new("x").icon(Glyph::Trash)`. SVGs render tinted by the text color.

use std::borrow::Cow;

use anyhow::Result;
use gpui_kit::component::IconNamed;
use gpui_kit::{AssetSource, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Embedded;

/// Path prefix of Roostr's own assets.
const PREFIX: &str = "roostr/";

/// The app's asset source: `roostr/...` from this crate, everything else from gpui-kit.
pub struct Assets;

/// gpui-kit's own icons. The desktop embeds its default set; in the browser
/// gpui-kit would fetch each SVG from a web endpoint, so the same default set
/// (`default-icons.txt` of gpui-kit-assets 0.7.3) is embedded here instead.
#[cfg(not(target_family = "wasm"))]
fn kit_assets() -> gpui_kit::assets::Assets {
    gpui_kit::assets::Assets
}

#[cfg(target_family = "wasm")]
gpui_kit::assets::icon_assets!(
    KitIcons,
    [
        ALargeSmall,
        ArrowDown,
        ArrowLeft,
        ArrowRight,
        ArrowUp,
        Asterisk,
        Ban,
        BatteryCharging,
        BatteryFull,
        BatteryLow,
        BatteryMedium,
        BatteryWarning,
        Battery,
        Bell,
        BookOpen,
        Bot,
        Building2,
        Calendar,
        CaseSensitive,
        ChartPie,
        Check,
        ChevronDown,
        ChevronLeft,
        ChevronRight,
        ChevronUp,
        ChevronsUpDown,
        CircleAlert,
        CircleCheck,
        CircleUser,
        CircleX,
        Close,
        Copy,
        Cpu,
        Dash,
        Delete,
        EllipsisVertical,
        Ellipsis,
        ExternalLink,
        EyeOff,
        Eye,
        FileText,
        File,
        FolderClosed,
        FolderOpen,
        Folder,
        Frame,
        GalleryVerticalEnd,
        Github,
        Globe,
        HardDrive,
        HeartOff,
        Heart,
        Inbox,
        Info,
        Inspector,
        LayoutDashboard,
        LoaderCircle,
        Loader,
        Map,
        Maximize,
        MemoryStick,
        Menu,
        Mic,
        Minimize,
        Minus,
        Moon,
        Network,
        Palette,
        PanelBottomOpen,
        PanelBottom,
        PanelLeftClose,
        PanelLeftOpen,
        PanelLeft,
        PanelRightClose,
        PanelRightOpen,
        PanelRight,
        Pause,
        Play,
        Plus,
        Redo2,
        Redo,
        RefreshCw,
        Replace,
        ResizeCorner,
        RotateCw,
        Search,
        Settings2,
        Settings,
        SortAscending,
        SortDescending,
        SquareTerminal,
        Square,
        StarFill,
        StarOff,
        Star,
        Sun,
        ThumbsDown,
        ThumbsUp,
        TriangleAlert,
        Undo2,
        Undo,
        User,
        WindowClose,
        WindowMaximize,
        WindowMinimize,
        WindowRestore
    ]
);

#[cfg(target_family = "wasm")]
fn kit_assets() -> KitIcons {
    KitIcons
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(own) = path.strip_prefix(PREFIX) {
            return Ok(Embedded::get(own).map(|f| f.data));
        }
        kit_assets().load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if let Some(own) = path.strip_prefix(PREFIX) {
            return Ok(Embedded::iter()
                .filter(|p| p.starts_with(own))
                .map(|p| SharedString::from(format!("{PREFIX}{p}")))
                .collect());
        }
        kit_assets().list(path)
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
    Trash => "trash",
    Type => "type",
}

impl IconNamed for Glyph {
    fn path(self) -> SharedString {
        format!("{PREFIX}icons/{}.svg", self.name()).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_has_an_embedded_svg() {
        for glyph in Glyph::ALL {
            let path = glyph.path();
            let bytes = Assets
                .load(&path)
                .unwrap()
                .unwrap_or_else(|| panic!("missing {path}"));
            assert!(bytes.starts_with(b"<svg"), "{path} is not an svg");
        }
    }
}

//! The block action menu (`BlockMenu.svelte`, Anytype's blockAction): a
//! filter box, root rows with submenu flyouts (Style, Align, Color,
//! Background, Turn into object), the File block's own actions, Duplicate
//! and Delete. Typing in the filter flattens everything into one list.
//!
//! The open menu lives in `Editor::block_menu` ([`OpenBlockMenu`]); its
//! filter field keeps the keyboard (↑/↓ move the highlight, Enter acts,
//! Escape closes the flyout, then the menu), and a press outside closes it.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use anyhow::Result;
use roostr_core::create::CreatableType;
use roostr_core::types::{
    Block, BlockContent, CustomContent, Fields, TextContent, Value, layout, mark, pos, style,
};
use roostr_core::{JsonMap, Route, State, api};
use roostr_rt::Task;
use roostr_ui::{Glyph, Theme, glyph, radius, rems, shadow, ty};
use rune::{Color, Cursor, El, Key, KeyEvent, TextEdit, Tx, col, el, row, text};

use crate::editor::{
    Editor, Editors, OpenBlockMenu, custom_content, new_id, text_content, update_editor,
    with_editor,
};
use crate::files;
use crate::marks::{self, Span};
use crate::slash_menu::{
    Preview, cmd_field, cmd_heading, cmd_panel, cmd_scroll, cmd_search, cmd_separator,
    preview_tile, window_size,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MenuAction {
    Style(u32),
    Align(u32),
    Color(String),
    Background(String),
    Duplicate,
    Delete,
    LinkStyle(String),
    ClearStyle,
    /// Strike every selected text block through end to end, or, when all
    /// of them already are, take it off.
    Strikethrough,
    TurnIntoObject(String),
}

/// Turn-into entries with a rendered preview + description, Anytype-style.
pub(crate) const STYLES: &[(&str, u32, &str, &str, Preview)] = &[
    (
        "Text",
        style::PARAGRAPH,
        "Plain paragraph",
        "Ag",
        Preview::P,
    ),
    (
        "Heading 1",
        style::HEADER1,
        "Big section heading",
        "Ag",
        Preview::H1,
    ),
    (
        "Heading 2",
        style::HEADER2,
        "Medium section heading",
        "Ag",
        Preview::H2,
    ),
    (
        "Heading 3",
        style::HEADER3,
        "Small section heading",
        "Ag",
        Preview::H3,
    ),
    (
        "Bulleted list",
        style::BULLET,
        "Simple list of items",
        "•—",
        Preview::List,
    ),
    (
        "Numbered list",
        style::NUMBERED,
        "Ordered list of items",
        "1—",
        Preview::List,
    ),
    (
        "Toggle",
        style::TOGGLE,
        "Collapsible content",
        "▸",
        Preview::List,
    ),
    (
        "Checkbox",
        style::CHECKBOX,
        "Task with a to-do state",
        "☑",
        Preview::Check,
    ),
    (
        "Quote",
        style::QUOTE,
        "Highlight a passage",
        "Ag",
        Preview::Quote,
    ),
    (
        "Code",
        style::CODE,
        "Monospaced snippet",
        "</>",
        Preview::Code,
    ),
    (
        "Callout",
        style::CALLOUT,
        "Bordered emphasis box",
        "Ag",
        Preview::Callout,
    ),
];

const ALIGNS: &[(&str, u32)] = &[("Left", 0), ("Center", 1), ("Right", 2)];

/// Anytype's palette names, dark-theme values (stored as the hex itself).
pub(crate) const COLORS: &[(&str, &str, &str)] = &[
    ("default", "", ""),
    ("grey", "#8b909b", "#3d3f45"),
    ("yellow", "#ecd91b", "#4c4523"),
    ("orange", "#ffb522", "#4f3d20"),
    ("red", "#f55522", "#4e2c21"),
    ("pink", "#e51ca0", "#4a2242"),
    ("purple", "#ab50cc", "#3e2a4a"),
    ("blue", "#628df2", "#26304c"),
    ("teal", "#0fc8ba", "#1a4341"),
    ("lime", "#5dd400", "#2f4218"),
];

const MENU_W: f32 = 260.;
const SUB_W: f32 = 220.;
/// Danger red of the Delete rows.
const DANGER: u32 = 0xf55522;
/// How long crossing rows waits before it opens or closes a flyout.
const HOVER_INTENT: Duration = Duration::from_millis(220);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SubKind {
    Style,
    Align,
    Color,
    Background,
    Turn,
}

impl SubKind {
    fn row_key(self) -> &'static str {
        match self {
            SubKind::Style => "bm-style",
            SubKind::Align => "bm-align",
            SubKind::Color => "bm-color",
            SubKind::Background => "bm-background",
            SubKind::Turn => "bm-turn-into-object",
        }
    }
}

/// What a list row does when pressed or confirmed with Enter.
#[derive(Clone, Debug)]
enum Fired {
    Action(MenuAction),
    OpenFile,
    CopyImage,
    Download,
}

#[derive(Clone, Debug)]
enum RowAct {
    Sub(SubKind),
    Fire(Fired),
}

/// The open menu's own state (GPUI's `BlockMenu` entity).
pub(crate) struct BlockMenuState {
    /// The filter box; it has the keyboard while the menu is open.
    pub filter: TextEdit,
    /// The open flyout.
    sub: Option<SubKind>,
    /// The flyout was opened by hovering its row (the click that follows keeps it open).
    hover_opened: bool,
    /// The one pending hover intent (open or close a flyout after 220ms).
    pending: Option<Task<()>>,
    /// The lit row: an index into the menu's actionable rows. None until
    /// the pointer or a key picks one.
    highlight: Option<usize>,
}

impl BlockMenuState {
    pub(crate) fn new() -> Self {
        let mut filter = TextEdit::new("");
        filter.focused = true;
        Self {
            filter,
            sub: None,
            hover_opened: false,
            pending: None,
            highlight: None,
        }
    }

    fn close_sub(&mut self) {
        self.pending = None;
        self.sub = None;
        self.hover_opened = false;
    }

    fn open_sub(&mut self, kind: SubKind) {
        self.pending = None;
        self.sub = Some(kind);
        self.hover_opened = true;
    }

    /// The click that follows a hover leaves the flyout open; a second click closes it.
    fn toggle_sub(&mut self, kind: SubKind) {
        if self.sub == Some(kind) {
            if self.hover_opened {
                self.hover_opened = false;
                return;
            }
            self.close_sub();
            return;
        }
        self.pending = None;
        self.hover_opened = false;
        self.sub = Some(kind);
    }
}

/// The `TextEdit` of `object_id`'s open block menu filter (the scratch edit
/// once the menu is gone).
pub(crate) fn block_menu_filter<'a>(s: &'a mut State, object_id: &str) -> &'a mut TextEdit {
    let editors = s.slot::<Editors>();
    let present = editors
        .map
        .get(object_id)
        .is_some_and(|e| e.block_menu.is_some());
    if present {
        &mut editors
            .map
            .get_mut(object_id)
            .and_then(|e| e.block_menu.as_mut())
            .expect("checked above")
            .menu
            .filter
    } else {
        editors.scratch = TextEdit::new("");
        &mut editors.scratch
    }
}

// ───────────────────────────────────────────── rows

/// A colour swatch: the stored hex, or (for "default") the text colour or nothing.
#[derive(Clone, Copy, Debug)]
struct Swatch {
    hex: &'static str,
    text: bool,
}

impl Swatch {
    fn color(self, t: &Theme) -> Color {
        roostr_ui::color::hex(self.hex).unwrap_or(if self.text { t.fg } else { Color::TRANSPARENT })
    }
}

#[derive(Clone, Debug)]
enum Trail {
    Text(&'static str),
    Dot(Swatch),
    None,
}

#[derive(Clone, Debug)]
enum Look {
    /// Opens a flyout.
    Sub { glyph: Glyph, trail: Trail },
    /// Acts at once.
    Plain { glyph: Glyph, danger: bool },
    /// A link block's appearance choice.
    Link { active: bool },
    /// A row of the flattened (filtering) list.
    Flat {
        active: bool,
        danger: bool,
        swatch: Option<Swatch>,
        desc: Option<&'static str>,
        preview: Option<(&'static str, Preview)>,
    },
}

#[derive(Clone, Debug)]
struct Item {
    key: String,
    label: String,
    act: RowAct,
    look: Look,
}

#[derive(Clone, Debug)]
enum Entry {
    Heading(String),
    Separator,
    /// An inert note row (accent: the group note; else "No matching actions").
    Note {
        text: String,
        accent: bool,
    },
    Item(Item),
}

/// `label` as a key part: lowercase words joined by "-".
fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn is_link(block: &Block) -> bool {
    block
        .content
        .custom
        .as_ref()
        .is_some_and(|c| c.content_type == "link")
}

fn file_meta(block: &Block) -> Option<BTreeMap<String, String>> {
    block
        .content
        .custom
        .as_ref()
        .filter(|c| c.content_type == "file")
        .map(|c| c.meta.clone().unwrap_or_default())
}

fn link_style(block: &Block) -> String {
    block
        .content
        .custom
        .as_ref()
        .and_then(|c| c.meta.as_ref())
        .and_then(|m| m.get("style").cloned())
        .unwrap_or_else(|| "text".into())
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

fn item(key: String, label: impl Into<String>, act: RowAct, look: Look) -> Entry {
    Entry::Item(Item {
        key,
        label: label.into(),
        act,
        look,
    })
}

fn flat(
    section: &'static str,
    label: &str,
    fired: Fired,
    active: bool,
    swatch: Option<Swatch>,
    desc: Option<&'static str>,
    preview: Option<(&'static str, Preview)>,
) -> (&'static str, Item) {
    (
        section,
        Item {
            key: format!("bm-{}-{}", slug(section), slug(label)),
            label: label.to_string(),
            act: RowAct::Fire(fired),
            look: Look::Flat {
                active,
                danger: label == "Delete",
                swatch,
                desc,
                preview,
            },
        },
    )
}

/// Every row of the flat (filtering) list, by section, then filtered.
fn flat_items(block: &Block, types: &[CreatableType], filter: &str) -> Vec<(&'static str, Item)> {
    let mut out = Vec::new();
    let t = block.content.text.as_ref();
    if is_link(block) {
        let cur = link_style(block);
        out.push(flat(
            "Appearance",
            "Text",
            Fired::Action(MenuAction::LinkStyle("text".into())),
            cur == "text",
            None,
            Some("Inline link"),
            Some(("Ag", Preview::P)),
        ));
        out.push(flat(
            "Appearance",
            "Card",
            Fired::Action(MenuAction::LinkStyle("card".into())),
            cur == "card",
            None,
            Some("Bordered preview card"),
            Some(("▭", Preview::P)),
        ));
    }
    if let Some(t) = t {
        for (label, value, desc, preview, cls) in STYLES {
            out.push(flat(
                "Turn into",
                label,
                Fired::Action(MenuAction::Style(*value)),
                t.style == *value,
                None,
                Some(*desc),
                Some((*preview, *cls)),
            ));
        }
    }
    let align = block.align.unwrap_or(0);
    for (label, value) in ALIGNS {
        out.push(flat(
            "Align",
            label,
            Fired::Action(MenuAction::Align(*value)),
            align == *value,
            None,
            None,
            None,
        ));
    }
    if let Some(t) = t {
        let color = t.color.clone().unwrap_or_default();
        for (name, text, _) in COLORS {
            out.push(flat(
                "Color",
                name,
                Fired::Action(MenuAction::Color(text.to_string())),
                color == *text,
                Some(Swatch {
                    hex: *text,
                    text: true,
                }),
                None,
                None,
            ));
        }
    }
    let bg = block.background_color.clone().unwrap_or_default();
    for (name, _, b) in COLORS {
        out.push(flat(
            "Background",
            name,
            Fired::Action(MenuAction::Background(b.to_string())),
            bg == *b,
            Some(Swatch {
                hex: *b,
                text: false,
            }),
            None,
            None,
        ));
    }
    for ty in types {
        let mut row = flat(
            "Turn into object",
            &ty.name,
            Fired::Action(MenuAction::TurnIntoObject(ty.key.clone())),
            false,
            None,
            None,
            None,
        );
        row.1.key = format!("bm-turn-into-object-{}", slug(&ty.key));
        out.push(row);
    }
    if let Some(meta) = file_meta(block) {
        if meta.get("fileId").is_some_and(|f| !f.is_empty()) {
            out.push(flat(
                "File",
                "Open file",
                Fired::OpenFile,
                false,
                None,
                None,
                None,
            ));
        }
        if meta.get("mime").is_some_and(|m| m.starts_with("image/")) {
            out.push(flat(
                "File",
                "Copy image",
                Fired::CopyImage,
                false,
                None,
                None,
                None,
            ));
        }
        out.push(flat(
            "File",
            "Download",
            Fired::Download,
            false,
            None,
            None,
            None,
        ));
    }
    out.push(flat(
        "Actions",
        "Duplicate",
        Fired::Action(MenuAction::Duplicate),
        false,
        None,
        None,
        None,
    ));
    out.push(flat(
        "Actions",
        "Delete",
        Fired::Action(MenuAction::Delete),
        false,
        None,
        None,
        None,
    ));
    let f = filter.trim().to_lowercase();
    if f.is_empty() {
        return out;
    }
    out.into_iter()
        .filter(|(section, i)| {
            i.label.to_lowercase().contains(&f) || section.to_lowercase().contains(&f)
        })
        .collect()
}

/// The menu's rows: the group note, then the root rows, or the flattened
/// list while the filter has text.
fn entries(block: &Block, group_count: usize, filter: &str, types: &[CreatableType]) -> Vec<Entry> {
    let mut out = Vec::new();
    if group_count > 1 {
        out.push(Entry::Note {
            text: format!("Applies to {group_count} selected blocks"),
            accent: true,
        });
    }
    if !filter.trim().is_empty() {
        let items = flat_items(block, types, filter);
        if items.is_empty() {
            out.push(Entry::Note {
                text: "No matching actions".into(),
                accent: false,
            });
        }
        let mut section = "";
        for (sec, it) in items {
            if sec != section {
                section = sec;
                out.push(Entry::Heading(sec.to_uppercase()));
            }
            out.push(Entry::Item(it));
        }
        return out;
    }

    let t = block.content.text.as_ref();
    if is_link(block) {
        let cur = link_style(block);
        out.push(Entry::Heading("APPEARANCE".into()));
        for (label, value) in [("Text", "text"), ("Card", "card")] {
            out.push(item(
                format!("bm-link-{value}"),
                label,
                RowAct::Fire(Fired::Action(MenuAction::LinkStyle(value.into()))),
                Look::Link {
                    active: cur == value,
                },
            ));
        }
    }
    out.push(Entry::Heading(
        if t.is_some() { "TEXT" } else { "BLOCK" }.into(),
    ));
    if let Some(t) = t {
        let cur = STYLES
            .iter()
            .find(|s| s.1 == t.style)
            .map_or("Text", |s| s.0);
        out.push(sub_item(
            SubKind::Style,
            Glyph::Type,
            "Style",
            Trail::Text(cur),
        ));
    }
    let cur_align = ALIGNS
        .iter()
        .find(|a| a.1 == block.align.unwrap_or(0))
        .map_or("Left", |a| a.0);
    out.push(sub_item(
        SubKind::Align,
        Glyph::AlignLeft,
        "Align",
        Trail::Text(cur_align),
    ));
    if let Some(t) = t {
        let c = t.color.clone().unwrap_or_default();
        let hex = COLORS.iter().find(|x| x.1 == c).map_or("", |x| x.1);
        out.push(sub_item(
            SubKind::Color,
            Glyph::Palette,
            "Color",
            Trail::Dot(Swatch { hex, text: true }),
        ));
    }
    let b = block.background_color.clone().unwrap_or_default();
    let hex = COLORS.iter().find(|x| x.2 == b).map_or("", |x| x.2);
    out.push(sub_item(
        SubKind::Background,
        Glyph::PaintBucket,
        "Background",
        Trail::Dot(Swatch { hex, text: false }),
    ));
    if t.is_some() {
        out.push(plain_item(
            Glyph::Eraser,
            "Clear style",
            Fired::Action(MenuAction::ClearStyle),
            false,
        ));
    }
    out.push(Entry::Separator);
    out.push(sub_item(
        SubKind::Turn,
        Glyph::Box,
        "Turn into object",
        Trail::None,
    ));
    if let Some(meta) = file_meta(block) {
        if meta.get("fileId").is_some_and(|f| !f.is_empty()) {
            out.push(plain_item(
                Glyph::ExternalLink,
                "Open file",
                Fired::OpenFile,
                false,
            ));
        }
        if meta.get("mime").is_some_and(|m| m.starts_with("image/")) {
            out.push(plain_item(
                Glyph::Copy,
                "Copy image",
                Fired::CopyImage,
                false,
            ));
        }
        out.push(plain_item(
            Glyph::Download,
            "Download",
            Fired::Download,
            false,
        ));
    }
    out.push(plain_item(
        Glyph::Copy,
        "Duplicate",
        Fired::Action(MenuAction::Duplicate),
        false,
    ));
    out.push(plain_item(
        Glyph::Trash,
        "Delete block",
        Fired::Action(MenuAction::Delete),
        true,
    ));
    out
}

fn sub_item(kind: SubKind, glyph: Glyph, label: &'static str, trail: Trail) -> Entry {
    item(
        kind.row_key().to_string(),
        label,
        RowAct::Sub(kind),
        Look::Sub { glyph, trail },
    )
}

fn plain_item(glyph: Glyph, label: &'static str, fired: Fired, danger: bool) -> Entry {
    item(
        format!("bm-{}", slug(label)),
        label,
        RowAct::Fire(fired),
        Look::Plain { glyph, danger },
    )
}

/// The actionable rows in order (what the highlight indexes).
fn items_of(entries: Vec<Entry>) -> Vec<Item> {
    entries
        .into_iter()
        .filter_map(|e| match e {
            Entry::Item(i) => Some(i),
            _ => None,
        })
        .collect()
}

// ───────────────────────────────────────────── editor side

impl Editor {
    /// Open the block action menu on `id` at window point `at`.
    pub(crate) fn open_block_menu(&mut self, id: &str, at: (f32, f32), s: &mut State) {
        if self.read_only {
            return;
        }
        self.toolbar = None;
        self.slash = None;
        // Opened on a block inside a multi-selection: styling applies to the whole selection.
        let group = (self.selected.len() > 1 && self.selected.iter().any(|s| s == id))
            .then(|| self.selected.clone());
        // Opened outside the selection: retarget it to the clicked block.
        if group.is_none() && !self.selected.is_empty() && !self.selected.iter().any(|s| s == id) {
            self.selected = vec![id.to_string()];
        }
        if self.block(id).is_none() {
            return;
        }
        if let Some(a) = self.active.clone() {
            self.flush_save(&a, s);
        }
        self.block_menu = Some(OpenBlockMenu {
            block: id.to_string(),
            group,
            at,
            menu: BlockMenuState::new(),
        });
    }

    /// The open menu's actionable rows as they stand.
    fn block_menu_items(&self, s: &State) -> Vec<Item> {
        let Some(open) = &self.block_menu else {
            return Vec::new();
        };
        let Some(block) = self.block(&open.block) else {
            return Vec::new();
        };
        let types = roostr_core::create::creatable_types(s);
        items_of(entries(
            block,
            open.group.as_ref().map_or(1, Vec::len),
            &open.menu.filter.text,
            &types,
        ))
    }

    /// The filter changed: the flyout closes, the first row lights.
    fn block_menu_query_changed(&mut self) {
        if let Some(open) = self.block_menu.as_mut() {
            open.menu.close_sub();
            open.menu.highlight = Some(0);
        }
    }

    /// Keys in the filter field.
    fn block_menu_key(&mut self, ev: &KeyEvent, s: &mut State, tx: &mut Tx) -> bool {
        let Some(open) = self.block_menu.as_mut() else {
            return false;
        };
        match ev.key {
            Key::Escape => {
                if open.menu.sub.is_some() {
                    open.menu.close_sub();
                } else {
                    self.block_menu = None;
                }
                true
            }
            Key::Up | Key::Down => {
                let items = self.block_menu_items(s);
                if items.is_empty() {
                    return true;
                }
                let len = items.len() as isize;
                let step: isize = if ev.key == Key::Down { 1 } else { -1 };
                let Some(open) = self.block_menu.as_mut() else {
                    return true;
                };
                let at = open
                    .menu
                    .highlight
                    .filter(|h| *h < items.len())
                    .unwrap_or(if step > 0 { items.len() - 1 } else { 0 });
                let next = (at as isize + step).rem_euclid(len) as usize;
                open.menu.highlight = Some(next);
                tx.scroll_into_view(items[next].key.clone());
                true
            }
            Key::Enter => {
                let items = self.block_menu_items(s);
                let highlighted = self
                    .block_menu
                    .as_ref()
                    .and_then(|o| o.menu.highlight)
                    .and_then(|h| items.into_iter().nth(h));
                if let Some(it) = highlighted {
                    self.block_menu_activate(it.act, s, tx);
                }
                true
            }
            _ => false,
        }
    }

    fn block_menu_activate(&mut self, act: RowAct, s: &mut State, tx: &mut Tx) {
        match act {
            RowAct::Sub(kind) => {
                if let Some(open) = self.block_menu.as_mut() {
                    open.menu.toggle_sub(kind);
                }
            }
            RowAct::Fire(fired) => self.block_menu_fire(fired, s, tx),
        }
    }

    /// Hover on a flyout row: one pending action at a time; crossing rows
    /// gets 220ms to prove itself. A mouse's only: a tap opens a flyout by
    /// its click.
    fn block_menu_hover_sub(&mut self, kind: SubKind, s: &mut State) {
        if roostr_core::layout::is_touch() {
            return;
        }
        let oid = self.object_id.clone();
        let Some(open) = self.block_menu.as_mut() else {
            return;
        };
        let m = &mut open.menu;
        m.pending = None;
        if m.sub == Some(kind) {
            return;
        }
        if m.sub.is_none() {
            m.open_sub(kind);
            return;
        }
        m.pending = Some(s.rt.spawn(async move |cx| {
            cx.timer(HOVER_INTENT).await;
            update_editor(&cx, &oid, |e, _| {
                if let Some(open) = e.block_menu.as_mut() {
                    open.menu.open_sub(kind);
                }
            });
        }));
    }

    /// Hover on a row that acts at once: an open flyout closes after 220ms.
    fn block_menu_hover_plain(&mut self, s: &mut State) {
        if roostr_core::layout::is_touch() {
            return;
        }
        let oid = self.object_id.clone();
        let Some(open) = self.block_menu.as_mut() else {
            return;
        };
        open.menu.pending = Some(s.rt.spawn(async move |cx| {
            cx.timer(HOVER_INTENT).await;
            update_editor(&cx, &oid, |e, _| {
                if let Some(open) = e.block_menu.as_mut() {
                    open.menu.close_sub();
                }
            });
        }));
    }

    /// Run a row: the File block's own actions here (nothing in the page
    /// changes), the rest through [`Self::on_menu_action`]; the menu closes.
    fn block_menu_fire(&mut self, fired: Fired, s: &mut State, tx: &mut Tx) {
        let Some(open) = &self.block_menu else {
            return;
        };
        let block_id = open.block.clone();
        if let Some(meta) = self.block(&block_id).and_then(file_meta) {
            let file_id = meta.get("fileId").cloned().unwrap_or_default();
            let name = s
                .store
                .summary(&file_id)
                .map(|s| s.name.clone())
                .filter(|n| !n.is_empty())
                .or_else(|| meta.get("name").cloned().filter(|n| !n.is_empty()))
                .unwrap_or_else(|| "file".into());
            match fired {
                Fired::OpenFile => {
                    self.block_menu = None;
                    roostr_core::nav::navigate(Route::Object(file_id), s);
                    return;
                }
                Fired::CopyImage => {
                    self.block_menu = None;
                    if let Err(err) = self.copy_image_block(&block_id, tx) {
                        self.file_notice = format!(
                            "Couldn't copy the image: {}",
                            files::view_failure_text(&err)
                        );
                    }
                    return;
                }
                Fired::Download => {
                    self.block_menu = None;
                    let oid = self.object_id.clone();
                    let what = format!("Couldn't download {name}");
                    files::download_file_block(&meta, name, s, tx, move |s, err| {
                        let text = format!("{what}: {}", files::view_failure_text(&err));
                        with_editor(s, &oid, |e, _| e.file_notice = text);
                    });
                    return;
                }
                Fired::Action(_) => {}
            }
        }
        if let Fired::Action(action) = fired {
            self.on_menu_action(action, s);
        }
        self.block_menu = None;
    }

    /// Duplicate a block's subtree below it (fresh ids, preserved order).
    fn duplicate_block(&mut self, id: &str, s: &mut State) {
        let Some(src) = self.block(id).cloned() else {
            return;
        };
        self.cancel_pending(id);
        if src.content.table.is_some() {
            // Cells are addressed "<rowId>-<colId>": create a same-size table, then copy cells.
            let (cols, rows) = self.table_shape(id);
            let create =
                self.w_table_create(id, pos::BOTTOM, rows.len() as u32, cols.len() as u32, s);
            let cells: Vec<Vec<Option<TextContent>>> = rows
                .iter()
                .map(|r| {
                    cols.iter()
                        .map(|c| self.text_of(&format!("{r}-{c}")).cloned())
                        .collect()
                })
                .collect();
            let oid = self.object_id.clone();
            s.rt.spawn(async move |cx| {
                let Ok(result) = create.await else { return };
                let new_table = result
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let Some(fetch) =
                    update_editor(&cx, &oid, |e, s| s.store.fetch_object(&e.object_id))
                else {
                    return;
                };
                let Ok(fresh) = fetch.await else { return };
                let Some(tasks) = update_editor(&cx, &oid, |e, s| {
                    let kids: Vec<&Block> = fresh
                        .block(&new_table)
                        .map(|t| {
                            t.children_ids
                                .iter()
                                .filter_map(|i| fresh.block(i))
                                .collect()
                        })
                        .unwrap_or_default();
                    let find = |st: u32| {
                        kids.iter()
                            .find(|b| b.content.layout.as_ref().map(|l| l.style) == Some(st))
                            .map(|b| b.children_ids.clone())
                            .unwrap_or_default()
                    };
                    let (ncols, nrows) = (find(layout::TABLE_COLUMNS), find(layout::TABLE_ROWS));
                    let mut tasks = Vec::new();
                    for (r, row) in cells.iter().enumerate() {
                        for (c, cell) in row.iter().enumerate() {
                            let Some(cell) = cell else { continue };
                            if cell.text.is_empty() && cell.marks.as_ref().is_none_or(Vec::is_empty)
                            {
                                continue;
                            }
                            let (Some(nr), Some(nc)) = (nrows.get(r), ncols.get(c)) else {
                                continue;
                            };
                            tasks.push(e.w_block_update(
                                &format!("{nr}-{nc}"),
                                BlockContent {
                                    text: Some(cell.clone()),
                                    ..Default::default()
                                },
                                s,
                            ));
                        }
                    }
                    tasks
                }) else {
                    return;
                };
                update_editor(&cx, &oid, |e, s| e.await_then_refresh(tasks, s));
            })
            .detach();
            return;
        }
        let mut tasks = Vec::new();
        fn clone_into(
            this: &Editor,
            src: &str,
            target: &str,
            position: u32,
            tasks: &mut Vec<Task<Result<JsonMap>>>,
            s: &State,
        ) {
            let Some(b) = this.block(src).cloned() else {
                return;
            };
            let nid = new_id();
            tasks.push(this.w_block_add(
                Block {
                    id: nid.clone(),
                    children_ids: vec![],
                    ..b.clone()
                },
                target,
                position,
                s,
            ));
            for c in &b.children_ids {
                clone_into(this, c, &nid, pos::INNER, tasks, s);
            }
        }
        clone_into(self, id, id, pos::BOTTOM, &mut tasks, s);
        self.await_then_refresh(tasks, s);
    }

    /// New text contents for `ids` from `make(current, live text, live
    /// spans)`, written at once: group styling snapshots reads before any
    /// write and refreshes once.
    fn group_text(
        &mut self,
        ids: &[String],
        make: &dyn Fn(&TextContent, &str, &[Span]) -> TextContent,
        s: &State,
    ) -> Vec<Task<Result<JsonMap>>> {
        let payloads: Vec<(String, BlockContent)> = ids
            .iter()
            .filter_map(|b| {
                let cur = self.text_of(b)?.clone();
                let (text, spans) = self.live(b);
                Some((
                    b.clone(),
                    BlockContent {
                        text: Some(make(&cur, &text, &spans)),
                        ..Default::default()
                    },
                ))
            })
            .collect();
        let mut tasks = Vec::new();
        for (b, content) in payloads {
            self.cancel_pending(&b);
            tasks.push(self.w_block_update(&b, content, s));
        }
        tasks
    }

    /// A block menu choice, on the menu's block or the selection it opened in.
    pub(crate) fn on_menu_action(&mut self, action: MenuAction, s: &mut State) {
        let Some(open) = self.block_menu.as_ref() else {
            return;
        };
        let id = open.block.clone();
        // Styling from a multi-selection hits every selected block.
        let ids = open.group.clone().unwrap_or_else(|| vec![id.clone()]);
        let text_ids: Vec<String> = ids
            .iter()
            .filter(|b| self.text_of(b).is_some())
            .cloned()
            .collect();
        match action {
            MenuAction::Style(value) => {
                if ids.len() == 1 {
                    self.apply_style(&id, value, s);
                    return;
                }
                let tasks = self.group_text(
                    &text_ids,
                    &|cur, text, spans| TextContent {
                        text: text.to_string(),
                        marks: Some(marks::marks_from_spans(text, spans)),
                        style: value,
                        checked: Some(cur.checked.unwrap_or(false)),
                        color: Some(cur.color.clone().unwrap_or_default()),
                    },
                    s,
                );
                self.await_then_refresh(tasks, s);
            }
            MenuAction::Align(value) => {
                let tasks = ids
                    .iter()
                    .map(|b| self.w_set_attrs(b, Some(value), None, s))
                    .collect();
                self.await_then_refresh(tasks, s);
            }
            MenuAction::Color(value) => {
                let tasks = self.group_text(
                    &text_ids,
                    &|cur, text, spans| TextContent {
                        text: text.to_string(),
                        marks: Some(marks::marks_from_spans(text, spans)),
                        color: Some(value.clone()),
                        ..cur.clone()
                    },
                    s,
                );
                self.await_then_refresh(tasks, s);
            }
            MenuAction::Background(value) => {
                let tasks = ids
                    .iter()
                    .map(|b| self.w_set_attrs(b, None, Some(value.clone()), s))
                    .collect();
                self.await_then_refresh(tasks, s);
            }
            MenuAction::Duplicate => self.duplicate_block(&id, s),
            MenuAction::LinkStyle(value) => {
                if let Some(custom) = self.block(&id).and_then(|b| b.content.custom.clone())
                    && custom.content_type == "link"
                {
                    let mut meta = custom.meta.clone().unwrap_or_default();
                    meta.insert("style".into(), value);
                    let content = BlockContent {
                        custom: Some(CustomContent {
                            content_type: "link".into(),
                            meta: Some(meta),
                            data: None,
                        }),
                        ..Default::default()
                    };
                    let task = self.w_block_update(&id, content, s);
                    self.await_then_refresh(vec![task], s);
                }
            }
            MenuAction::TurnIntoObject(type_key) => self.turn_into_object(&id, type_key, s),
            MenuAction::ClearStyle => {
                let mut tasks = self.group_text(
                    &text_ids,
                    &|cur, text, spans| TextContent {
                        text: text.to_string(),
                        marks: Some(marks::marks_from_spans(text, spans)),
                        color: Some(String::new()),
                        ..cur.clone()
                    },
                    s,
                );
                for b in &ids {
                    tasks.push(self.w_set_attrs(b, Some(0), Some(String::new()), s));
                }
                self.await_then_refresh(tasks, s);
            }
            MenuAction::Strikethrough => {
                // One answer for the whole group: off only when every block
                // with text is already struck through end to end.
                let strike = mark::STRIKETHROUGH;
                let all_struck = text_ids.iter().all(|b| {
                    let (text, spans) = self.live(b);
                    text.is_empty() || marks::covers(&spans, 0, text.len(), strike)
                });
                let tasks = self.group_text(
                    &text_ids,
                    &|cur, text, spans| {
                        let next = if all_struck != marks::covers(spans, 0, text.len(), strike) {
                            spans.to_vec()
                        } else {
                            marks::toggle_mark(spans, 0, text.len(), strike, None)
                        };
                        TextContent {
                            text: text.to_string(),
                            marks: Some(marks::marks_from_spans(text, &next)),
                            ..cur.clone()
                        }
                    },
                    s,
                );
                self.await_then_refresh(tasks, s);
            }
            MenuAction::Delete => {
                if ids.len() > 1 {
                    let set: HashSet<String> = ids.iter().cloned().collect();
                    let tops = self.topmost_of(&set);
                    self.selected.clear();
                    let mut tasks = Vec::new();
                    for b in tops {
                        self.cancel_pending(&b);
                        tasks.push(self.w_block_remove(&b, s));
                    }
                    self.await_then_refresh(tasks, s);
                } else {
                    self.remove_block_by_id(&id, s).detach();
                }
            }
        }
    }

    /// Turn-into from the block action menu.
    fn apply_style(&mut self, id: &str, value: u32, s: &mut State) {
        let cur = self.text_of(id).cloned();
        let (text, spans) = self.live(id);
        self.cancel_pending(id);
        let content = text_content(
            &text,
            &spans,
            value,
            cur.as_ref().and_then(|t| t.checked).unwrap_or(false),
            cur.as_ref()
                .and_then(|t| t.color.clone())
                .unwrap_or_default(),
        );
        let task = self.w_block_update(id, content, s);
        self.focus_request = Some((id.to_string(), text.len()));
        self.await_then_refresh(vec![task], s);
    }

    /// Anytype's Turn into object: the block's text becomes a new object's
    /// name, its children that object's content, and a link block takes its place.
    fn turn_into_object(&mut self, id: &str, type_key: String, s: &mut State) {
        self.flush_all(s);
        let typed = self.live(id).0;
        let id = id.to_string();
        let refresh = self.refresh(s);
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            refresh.await;
            let Some(Some((src, ch))) = update_editor(&cx, &oid, |e, _| {
                let src = e.block(&id).cloned()?;
                let ch = e
                    .object_fields()
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Some((src, ch))
            }) else {
                return;
            };
            let stored = src
                .content
                .text
                .as_ref()
                .map(|t| t.text.clone())
                .unwrap_or_default();
            let base = if stored.is_empty() { typed } else { stored };
            let name: String = base.trim().chars().take(120).collect();
            let name = if name.is_empty() {
                "Untitled".to_string()
            } else {
                name
            };
            let mut fields = Fields::new();
            if !ch.is_empty() {
                fields.insert("channel".into(), Value::string(ch));
            }
            let Some(create) = update_editor(&cx, &oid, |_, s| {
                roostr_core::create::from_type(&name, &type_key, fields, s)
            }) else {
                return;
            };
            let new_obj = match create.await {
                Ok(id) => id,
                Err(error) => {
                    log::error!("[editor] turn into object failed: {error:#}");
                    return;
                }
            };
            let Some(tasks) = update_editor(&cx, &oid, |e, s| {
                let mut tasks = Vec::new();
                fn move_into(
                    this: &Editor,
                    src: &str,
                    new_obj: &str,
                    target: &str,
                    position: u32,
                    tasks: &mut Vec<Task<Result<JsonMap>>>,
                    s: &State,
                ) {
                    let Some(b) = this.block(src).cloned() else {
                        return;
                    };
                    let nid = new_id();
                    let block = Block {
                        id: nid.clone(),
                        children_ids: vec![],
                        ..b.clone()
                    };
                    let (obj, target) = (new_obj.to_string(), target.to_string());
                    tasks.push(this.writes.run(s, move |s| {
                        api::note::block_add(&obj, &block, &target, position, s)
                    }));
                    for c in &b.children_ids {
                        move_into(this, c, new_obj, &nid, pos::INNER, tasks, s);
                    }
                }
                for c in &src.children_ids {
                    move_into(e, c, &new_obj, "", pos::NONE, &mut tasks, s);
                }
                let link = Block {
                    id: new_id(),
                    content: custom_content("link", &[("target", &new_obj)]),
                    ..Default::default()
                };
                tasks.push(e.w_block_add(link, &id, pos::BOTTOM, s));
                tasks
            }) else {
                return;
            };
            for t in tasks {
                let _ = t.await;
            }
            let Some(remove) = update_editor(&cx, &oid, |e, s| e.remove_block_by_id(&id, s)) else {
                return;
            };
            remove.await;
            update_editor(&cx, &oid, |_, s| s.store.refresh_all());
        })
        .detach();
    }
}

// ───────────────────────────────────────────── view

/// The open block menu (260pt at its point, kept in the window) and its
/// flyout, pushed into `layer`.
pub(crate) fn block_menu_view(
    s: &State,
    cx: &mut rune::Cx<State>,
    t: &Theme,
    e: &Editor,
    layer: &mut Vec<El>,
) {
    let Some(open) = &e.block_menu else { return };
    let Some(block) = e.block(&open.block) else {
        return;
    };
    let menu = &open.menu;
    let oid = e.object_id.clone();
    let types = roostr_core::create::creatable_types(s);
    let list_entries = entries(
        block,
        open.group.as_ref().map_or(1, Vec::len),
        &menu.filter.text,
        &types,
    );

    let [vw, vh] = window_size(cx);
    let h = 440f32.min(vh - 40.);
    let left = 8f32.max(open.at.0.min(vw - MENU_W - 12.));
    let top = open.at.1.min(vh - h - 12.);

    let mut list = col().padding(rems(0.25)).items_stretch();
    let mut index = 0;
    for entry in list_entries {
        list = list.child(match entry {
            Entry::Heading(label) => cmd_heading(t, &label),
            Entry::Separator => cmd_separator(t),
            Entry::Note { text: note, accent } => note_row(t, note, accent),
            Entry::Item(it) => {
                let lit = menu.highlight == Some(index);
                let r = menu_row(t, &oid, index, it, lit, menu.sub);
                index += 1;
                r
            }
        });
    }

    let field_oid = oid.clone();
    let change_oid = oid.clone();
    let key_oid = oid.clone();
    let field = cmd_field(t, &menu.filter, "bm-filter", "Filter actions…")
        .on_edit(rune::edit_access(move |s: &mut State| {
            block_menu_filter(s, &field_oid)
        }))
        .on_input_change(move |s: &mut State, _: &mut Tx| {
            with_editor(s, &change_oid, |e, _| e.block_menu_query_changed());
        })
        .on_input_key(move |s: &mut State, ev: &KeyEvent, tx: &mut Tx| {
            with_editor(s, &key_oid, |e, s| e.block_menu_key(ev, s, tx)).unwrap_or(false)
        });
    let search = cmd_search(t, field).key("bm-filter-bar");
    let panel = cmd_panel(t)
        .box_shadow(shadow::menu())
        .child(search)
        // Rows leave the keyboard in the filter field.
        .child(cmd_scroll(list, 390.).preserve_focus_on_press(true));

    let mut surface = col()
        .key("block-menu")
        .role("menu")
        .w(MENU_W)
        .max_h(440.)
        .items_stretch()
        .child(panel);

    if let Some(kind) = menu.sub {
        let sub_top = cx.window_layout(kind.row_key()).map_or(open.at.1, |r| r[1]);
        let fh = 420f32.min(vh - 24.);
        let right_x = left + MENU_W - 6.;
        let fl = if right_x + SUB_W + 12. <= vw {
            right_x
        } else {
            8f32.max(left - SUB_W + 6.)
        };
        let ft = 8f32.max(sub_top.min(vh - fh - 12.));
        let hover_oid = oid.clone();
        let fly = flyout(t, &oid, kind, block, &types, fh)
            .on_hover_change(move |s: &mut State, inside: bool, _: &mut Tx| {
                // Reaching the flyout cancels the pending close from rows crossed on the way.
                if inside {
                    with_editor(s, &hover_oid, |e, _| {
                        if let Some(open) = e.block_menu.as_mut() {
                            open.menu.pending = None;
                        }
                    });
                }
            })
            .absolute(fl - left, ft - top)
            .transition(roostr_ui::motion::appear(-4.));
        surface = surface.child(fly);
    }

    let close_oid = oid.clone();
    let surface = roostr_ui::overlay::dismissable(
        surface,
        "block-menu",
        move |s: &mut State| {
            with_editor(s, &close_oid, |e, _| e.block_menu = None);
        },
        layer,
    );
    layer.push(
        surface
            .absolute(left, top)
            .transition(roostr_ui::motion::appear(-4.))
            .instant_changes(),
    );
}

/// An inert note row in the kit row padding.
fn note_row(t: &Theme, note: String, accent: bool) -> El {
    let body = if accent {
        el().ml(4.)
            .child(text(note).font(ty::ui(11.)).color(t.accent))
    } else {
        el().padding(4.)
            .child(text(note).font(ty::ui(13.)).color(t.muted))
    };
    row()
        .w_full()
        .px(rems(0.5))
        .py(rems(0.375))
        .child(body)
        .disabled(true)
}

fn dot(t: &Theme, color: Color, ring: bool) -> El {
    let d = el().size(14., 14.).radius(7.).fill(color);
    if ring { d.border(1., t.border) } else { d }
}

fn check(color: Color) -> El {
    glyph(Glyph::Check, 14.).color(color)
}

/// A root or flat row: Roostr's 10/7 spacing over the whole kit row, the
/// accent fill while lit, `bm-<label>` keys.
fn menu_row(t: &Theme, oid: &str, index: usize, it: Item, lit: bool, sub: Option<SubKind>) -> El {
    let open = matches!(it.act, RowAct::Sub(k) if sub == Some(k));
    let danger = roostr_ui::color::rgb(DANGER);
    let mut r = row()
        .key(it.key.clone())
        .role("menuitem")
        .label(it.label.clone())
        .w_full()
        .center_y()
        .gap(8.)
        .px(10.)
        .py(7.)
        .radius(6.)
        .cursor(Cursor::Pointer);
    r = if open {
        r.fill(t.hover).expanded(true)
    } else if lit {
        r.fill(t.kit.accent)
    } else {
        r
    };
    let label = |c: Color| {
        text(it.label.clone())
            .font(ty::ui(13.))
            .color(c)
            .truncate()
            .shrink()
            .a11y_hidden()
    };
    let spacer = || el().grow(1.);
    let mut hover_sub = None;
    r = match &it.look {
        Look::Sub { glyph: g, trail } => {
            if let RowAct::Sub(k) = it.act {
                hover_sub = Some(k);
            }
            let mut tr = row().gap(6.).center_y();
            tr = match trail {
                Trail::Text(v) => tr.child(text(*v).font(ty::ui(12.)).color(t.muted)),
                Trail::Dot(sw) => tr.child(dot(t, sw.color(t), true)),
                Trail::None => tr,
            };
            tr = tr.child(glyph(Glyph::ChevronRight, 14.).color(t.muted));
            r.child(glyph(*g, 16.).color(t.fg))
                .child(label(t.fg))
                .child(spacer())
                .child(tr)
        }
        Look::Plain {
            glyph: g,
            danger: d,
        } => {
            let ink = if *d { danger } else { t.fg };
            r.child(glyph(*g, 16.).color(ink)).child(label(ink))
        }
        Look::Link { active } => {
            let ink = if *active { t.accent } else { t.fg };
            let r = r.child(label(ink));
            if *active {
                r.child(spacer()).child(check(ink))
            } else {
                r
            }
        }
        Look::Flat {
            active,
            danger: d,
            swatch,
            desc,
            preview,
        } => {
            let ink = if *d {
                danger
            } else if *active {
                t.accent
            } else {
                t.fg
            };
            let mut r = r;
            if let Some(sw) = swatch {
                r = r.child(
                    el().size(14., 14.)
                        .radius(4.)
                        .border(1., t.border)
                        .fill(sw.color(t)),
                );
            }
            r = match preview {
                Some((p, cls)) => r
                    .child(preview_tile(t, p, *cls, [34., 30.], t.bg, None, false))
                    .child(
                        col().gap(1.).shrink().min_w(0.).child(label(ink)).child(
                            text(desc.unwrap_or(""))
                                .font(ty::ui(11.))
                                .color(t.muted)
                                .a11y_hidden(),
                        ),
                    ),
                // Filter mode shows labels as stored (palette names stay lowercase).
                None => r.child(label(ink)),
            };
            if *active {
                r.child(spacer()).child(check(ink))
            } else {
                r
            }
        }
    };

    let act = it.act.clone();
    let (press_oid, hover_oid) = (oid.to_string(), oid.to_string());
    r.on_press_tx(move |s: &mut State, tx: &mut Tx| {
        let act = act.clone();
        with_editor(s, &press_oid, |e, s| e.block_menu_activate(act, s, tx));
    })
    .on_hover_change(move |s: &mut State, inside: bool, _: &mut Tx| {
        if !inside {
            return;
        }
        with_editor(s, &hover_oid, |e, s| {
            if let Some(open) = e.block_menu.as_mut() {
                open.menu.highlight = Some(index);
            }
            match hover_sub {
                Some(kind) => e.block_menu_hover_sub(kind, s),
                None => {
                    // Rows of the flat list don't touch flyouts (there are none).
                    if e.block_menu
                        .as_ref()
                        .is_some_and(|o| o.menu.filter.text.trim().is_empty())
                    {
                        e.block_menu_hover_plain(s);
                    }
                }
            }
        });
    })
}

/// One flyout choice: the kit menu row around Roostr's 13px row, the check
/// at the right while it is the current one.
fn fly_item(
    t: &Theme,
    oid: &str,
    key: String,
    label: &str,
    body: El,
    active: bool,
    action: MenuAction,
) -> El {
    let mut r = row()
        .key(key)
        .role("menuitemcheckbox")
        .label(label.to_string())
        .checked(active)
        .px(rems(0.5))
        .gap(rems(0.25))
        .radius(radius::KIT)
        .min_h(rems(1.625))
        .center_y()
        .hover(t.kit.accent)
        .cursor(Cursor::Pointer)
        .child(body.grow(1.).shrink());
    if active {
        r = r.child(glyph(Glyph::Check, rems(0.75)).color(t.kit.foreground));
    }
    let oid = oid.to_string();
    r.on_press_tx(move |s: &mut State, tx: &mut Tx| {
        let fired = Fired::Action(action.clone());
        with_editor(s, &oid, |e, s| e.block_menu_fire(fired, s, tx));
    })
}

/// The 13px row body inside a flyout item.
fn fly_row(active: bool, t: &Theme) -> (El, Color) {
    let ink = if active { t.accent } else { t.fg };
    (row().center_y().gap(8.).ml(2.).py(7.), ink)
}

fn fly_text(label: &str, ink: Color) -> El {
    text(label.to_string())
        .font(ty::ui(13.))
        .color(ink)
        .truncate()
        .a11y_hidden()
}

/// A preview, the name and a one-line description (Style rows).
fn fly_styled(t: &Theme, tile: El, label: &str, desc: &str, ink: Color, active: bool) -> El {
    let (r, _) = fly_row(active, t);
    r.child(tile).child(
        col()
            .gap(1.)
            .shrink()
            .min_w(0.)
            .child(fly_text(label, ink))
            .child(
                text(desc.to_string())
                    .font(ty::ui(11.))
                    .color(t.muted)
                    .a11y_hidden(),
            ),
    )
}

/// The flyout for `kind`: a kit popup menu of the choices, the current one checked.
fn flyout(
    t: &Theme,
    oid: &str,
    kind: SubKind,
    block: &Block,
    types: &[CreatableType],
    max_h: f32,
) -> El {
    let text_c = block.content.text.as_ref();
    let mut rows: Vec<El> = Vec::new();
    match kind {
        SubKind::Style => {
            if let Some(tc) = text_c {
                for (label, value, desc, preview, cls) in STYLES {
                    let active = tc.style == *value;
                    let (_, ink) = fly_row(active, t);
                    let tile_ink = if *cls == Preview::P { t.muted } else { t.fg };
                    let tile =
                        preview_tile(t, preview, *cls, [34., 30.], t.bg, Some(tile_ink), false);
                    rows.push(fly_item(
                        t,
                        oid,
                        format!("bm-style-{}", slug(label)),
                        label,
                        fly_styled(t, tile, label, desc, ink, active),
                        active,
                        MenuAction::Style(*value),
                    ));
                }
                rows.push(
                    el().h(2.)
                        .my(rems(0.125))
                        .mx(-rems(0.25))
                        .fill(t.kit.border)
                        .a11y_hidden(),
                );
                // Last, under the block styles: a text mark (the whole block
                // struck through), on or off over any style.
                let spans = marks::spans_from_marks(&tc.text, tc.marks.as_deref().unwrap_or(&[]));
                let struck = !tc.text.is_empty()
                    && marks::covers(&spans, 0, tc.text.len(), mark::STRIKETHROUGH);
                let (_, ink) = fly_row(struck, t);
                let tile = preview_tile(t, "S", Preview::P, [34., 30.], t.bg, Some(t.fg), true);
                rows.push(fly_item(
                    t,
                    oid,
                    "bm-strikethrough".into(),
                    "Strikethrough",
                    fly_styled(
                        t,
                        tile,
                        "Strikethrough",
                        "Cross out the whole block",
                        ink,
                        struck,
                    ),
                    struck,
                    MenuAction::Strikethrough,
                ));
            }
        }
        SubKind::Align => {
            let align = block.align.unwrap_or(0);
            for (label, value) in ALIGNS {
                let active = align == *value;
                let (r, ink) = fly_row(active, t);
                rows.push(fly_item(
                    t,
                    oid,
                    format!("bm-align-{}", slug(label)),
                    label,
                    r.child(fly_text(label, ink)),
                    active,
                    MenuAction::Align(*value),
                ));
            }
        }
        SubKind::Color => {
            if let Some(tc) = text_c {
                let cur = tc.color.clone().unwrap_or_default();
                for (name, hex, _) in COLORS {
                    let active = cur == *hex;
                    let (r, ink) = fly_row(active, t);
                    let label = capitalize(name);
                    rows.push(fly_item(
                        t,
                        oid,
                        format!("bm-color-{name}"),
                        &label,
                        r.child(dot(
                            t,
                            Swatch {
                                hex: *hex,
                                text: true,
                            }
                            .color(t),
                            false,
                        ))
                        .child(fly_text(&label, ink)),
                        active,
                        MenuAction::Color(hex.to_string()),
                    ));
                }
            }
        }
        SubKind::Background => {
            let cur = block.background_color.clone().unwrap_or_default();
            for (name, _, hex) in COLORS {
                let active = cur == *hex;
                let (r, ink) = fly_row(active, t);
                let label = capitalize(name);
                rows.push(fly_item(
                    t,
                    oid,
                    format!("bm-background-{name}"),
                    &label,
                    r.child(dot(
                        t,
                        Swatch {
                            hex: *hex,
                            text: false,
                        }
                        .color(t),
                        true,
                    ))
                    .child(fly_text(&label, ink)),
                    active,
                    MenuAction::Background(hex.to_string()),
                ));
            }
        }
        SubKind::Turn => {
            for ty in types {
                let (r, ink) = fly_row(false, t);
                rows.push(fly_item(
                    t,
                    oid,
                    format!("bm-turn-{}", slug(&ty.key)),
                    &ty.name,
                    r.child(
                        el().w(18.)
                            .center()
                            .child(text(ty.icon.clone()).font(ty::ui(13.)).color(t.fg)),
                    )
                    .child(fly_text(&ty.name, ink)),
                    false,
                    MenuAction::TurnIntoObject(ty.key.clone()),
                ));
            }
        }
    }
    let items = col()
        .items_stretch()
        .padding(rems(0.25))
        .gap(rems(0.125))
        .children(rows);
    let mut surface = col()
        .key("block-menu-flyout")
        .role("menu")
        .w(SUB_W)
        .items_stretch()
        .fill(t.kit.popover)
        .radius(radius::KIT)
        // Focus stays in the filter field.
        .preserve_focus_on_press(true);
    for sh in shadow::kit_popover(t) {
        surface = surface.box_shadow(sh);
    }
    surface.child(
        col()
            .max_h(max_h.min(418.))
            .min_h(0.)
            .shrink()
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .child(items),
    )
}

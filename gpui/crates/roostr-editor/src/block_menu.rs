//! The block action menu (`BlockMenu.svelte`, Anytype's blockAction): a
//! filter box, root rows with submenu flyouts (Style, Align, Color,
//! Background, Turn into object), the File block's own actions, Duplicate
//! and Delete. Typing in the filter flattens everything into one list.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::types::{Block, style};
use roostr_core::{Glyph, Store, theme};

use crate::slash_menu::{Preview, preview_tile, section_label};

#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Style(u32),
    Align(u32),
    Color(String),
    Background(String),
    Duplicate,
    Delete,
    LinkStyle(String),
    ClearStyle,
    TurnIntoObject(String),
}

pub enum MenuEvent {
    Action(MenuAction),
    Close,
}

impl EventEmitter<MenuEvent> for BlockMenu {}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SubKind {
    Style,
    Align,
    Color,
    Background,
    Turn,
}

/// One row of the flat (filtering) list.
struct Item {
    section: &'static str,
    label: SharedString,
    action: Fired,
    active: bool,
    swatch: Option<Hsla>,
    desc: Option<&'static str>,
    preview: Option<(&'static str, Preview)>,
}

#[derive(Clone)]
enum Fired {
    Action(MenuAction),
    OpenFile,
    CopyImage,
    Download,
}

pub struct BlockMenu {
    block: Block,
    at: Point<Pixels>,
    group_count: usize,
    filter: Entity<InputState>,
    sub: Option<(SubKind, Pixels)>,
    hover_opened: bool,
    pending: Option<Task<()>>,
    /// Row tops (window y) of the submenu rows, for the flyout.
    row_tops: Rc<RefCell<HashMap<SubKind, Pixels>>>,
    /// Menu + flyout bounds, for click-outside.
    bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    _subs: Vec<Subscription>,
}

fn swatch_color(hex: &str, fallback: Hsla) -> Hsla {
    roostr_core::color::hex(hex).unwrap_or(fallback)
}

impl BlockMenu {
    pub fn new(
        block: Block,
        at: Point<Pixels>,
        group_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter actions…"));
        let subs = vec![
            cx.subscribe(&filter, |this: &mut Self, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.close_sub();
                    cx.notify();
                }
            }),
        ];
        filter.update(cx, |f, cx| f.focus(window, cx));
        Self {
            block,
            at,
            group_count,
            filter,
            sub: None,
            hover_opened: false,
            pending: None,
            row_tops: Rc::default(),
            bounds: Rc::default(),
            _subs: subs,
        }
    }

    fn text(&self) -> Option<&roostr_core::types::TextContent> {
        self.block.content.text.as_ref()
    }

    fn is_link(&self) -> bool {
        self.block
            .content
            .custom
            .as_ref()
            .is_some_and(|c| c.content_type == "link")
    }

    fn file_meta(&self) -> Option<std::collections::BTreeMap<String, String>> {
        self.block
            .content
            .custom
            .as_ref()
            .filter(|c| c.content_type == "file")
            .map(|c| c.meta.clone().unwrap_or_default())
    }

    fn close_sub(&mut self) {
        self.pending = None;
        self.sub = None;
        self.hover_opened = false;
    }

    fn open_sub(&mut self, kind: SubKind) {
        self.pending = None;
        let top = self
            .row_tops
            .borrow()
            .get(&kind)
            .copied()
            .unwrap_or(self.at.y);
        self.sub = Some((kind, top));
        self.hover_opened = true;
    }

    /// The click that follows a hover leaves the flyout open; a second click closes it.
    fn toggle_sub(&mut self, kind: SubKind, cx: &mut Context<Self>) {
        if self.sub.map(|s| s.0) == Some(kind) {
            if self.hover_opened {
                self.hover_opened = false;
                return;
            }
            self.close_sub();
            cx.notify();
            return;
        }
        self.pending = None;
        self.hover_opened = false;
        let top = self
            .row_tops
            .borrow()
            .get(&kind)
            .copied()
            .unwrap_or(self.at.y);
        self.sub = Some((kind, top));
        cx.notify();
    }

    /// Hover: one pending action at a time; crossing rows gets 220ms to prove
    /// itself. A mouse's only (`pointerType !== "mouse"` returns): a tap opens
    /// a flyout by its click.
    fn hover_sub(&mut self, kind: SubKind, cx: &mut Context<Self>) {
        if roostr_core::layout::is_touch() {
            return;
        }
        self.pending = None;
        if self.sub.map(|s| s.0) == Some(kind) {
            return;
        }
        if self.sub.is_none() {
            self.open_sub(kind);
            cx.notify();
            return;
        }
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(220))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.open_sub(kind);
                cx.notify();
            });
        }));
    }

    fn hover_plain(&mut self, cx: &mut Context<Self>) {
        if roostr_core::layout::is_touch() {
            return;
        }
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(220))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.close_sub();
                cx.notify();
            });
        }));
    }

    fn fire(&mut self, fired: Fired, cx: &mut Context<Self>) {
        // The file's own actions run here: nothing in the page changes.
        if let Some(meta) = self.file_meta() {
            let name = {
                let store = Store::global(cx).read(cx);
                let id = meta.get("fileId").cloned().unwrap_or_default();
                store
                    .summary(&id)
                    .map(|s| s.name.clone())
                    .filter(|n| !n.is_empty())
                    .or_else(|| meta.get("name").cloned().filter(|n| !n.is_empty()))
                    .unwrap_or_else(|| "file".into())
            };
            match fired {
                Fired::OpenFile => {
                    let id = meta.get("fileId").cloned().unwrap_or_default();
                    roostr_core::nav::navigate(roostr_core::Route::Object(id), cx);
                    cx.emit(MenuEvent::Close);
                    return;
                }
                Fired::CopyImage | Fired::Download => {
                    let copy = matches!(fired, Fired::CopyImage);
                    let task = if copy {
                        crate::files::copy_image_block(&meta, cx)
                    } else {
                        crate::files::download_file_block(&meta, name.clone(), cx)
                    };
                    cx.emit(MenuEvent::Close);
                    cx.spawn(async move |_, cx| {
                        if let Err(err) = task.await {
                            let what = if copy {
                                "Couldn't copy the image".to_string()
                            } else {
                                format!("Couldn't download {name}")
                            };
                            let text = format!("{what}: {}", crate::files::view_failure_text(&err));
                            cx.update(|cx| crate::alert(text, cx));
                        }
                    })
                    .detach();
                    return;
                }
                Fired::Action(_) => {}
            }
        }
        if let Fired::Action(a) = fired {
            cx.emit(MenuEvent::Action(a));
        }
        cx.emit(MenuEvent::Close);
    }

    fn items(&self, cx: &App) -> Vec<Item> {
        let mut out = Vec::new();
        let t = self.text();
        if self.is_link() {
            let cur = self
                .block
                .content
                .custom
                .as_ref()
                .and_then(|c| c.meta.as_ref())
                .and_then(|m| m.get("style").cloned())
                .unwrap_or_else(|| "text".into());
            out.push(Item {
                section: "Appearance",
                label: "Text".into(),
                action: Fired::Action(MenuAction::LinkStyle("text".into())),
                active: cur == "text",
                swatch: None,
                desc: Some("Inline link"),
                preview: Some(("Ag", Preview::P)),
            });
            out.push(Item {
                section: "Appearance",
                label: "Card".into(),
                action: Fired::Action(MenuAction::LinkStyle("card".into())),
                active: cur == "card",
                swatch: None,
                desc: Some("Bordered preview card"),
                preview: Some(("▭", Preview::P)),
            });
        }
        if let Some(t) = t {
            for (label, value, desc, preview, cls) in STYLES {
                out.push(Item {
                    section: "Turn into",
                    label: (*label).into(),
                    action: Fired::Action(MenuAction::Style(*value)),
                    active: t.style == *value,
                    swatch: None,
                    desc: Some(desc),
                    preview: Some((preview, *cls)),
                });
            }
        }
        let align = self.block.align.unwrap_or(0);
        for (label, value) in ALIGNS {
            out.push(Item {
                section: "Align",
                label: (*label).into(),
                action: Fired::Action(MenuAction::Align(*value)),
                active: align == *value,
                swatch: None,
                desc: None,
                preview: None,
            });
        }
        if let Some(t) = t {
            let color = t.color.clone().unwrap_or_default();
            for (name, text, _) in COLORS {
                out.push(Item {
                    section: "Color",
                    label: (*name).into(),
                    action: Fired::Action(MenuAction::Color(text.to_string())),
                    active: color == *text,
                    swatch: Some(swatch_color(text, theme::fg())),
                    desc: None,
                    preview: None,
                });
            }
        }
        let bg = self.block.background_color.clone().unwrap_or_default();
        for (name, _, b) in COLORS {
            out.push(Item {
                section: "Background",
                label: (*name).into(),
                action: Fired::Action(MenuAction::Background(b.to_string())),
                active: bg == *b,
                swatch: Some(swatch_color(b, gpui_kit::transparent_black())),
                desc: None,
                preview: None,
            });
        }
        for ty in roostr_core::create::creatable_types(cx) {
            out.push(Item {
                section: "Turn into object",
                label: ty.name.into(),
                action: Fired::Action(MenuAction::TurnIntoObject(ty.key)),
                active: false,
                swatch: None,
                desc: None,
                preview: None,
            });
        }
        if let Some(meta) = self.file_meta() {
            if meta.get("fileId").is_some_and(|f| !f.is_empty()) {
                out.push(Item {
                    section: "File",
                    label: "Open file".into(),
                    action: Fired::OpenFile,
                    active: false,
                    swatch: None,
                    desc: None,
                    preview: None,
                });
            }
            if meta.get("mime").is_some_and(|m| m.starts_with("image/")) {
                out.push(Item {
                    section: "File",
                    label: "Copy image".into(),
                    action: Fired::CopyImage,
                    active: false,
                    swatch: None,
                    desc: None,
                    preview: None,
                });
            }
            out.push(Item {
                section: "File",
                label: "Download".into(),
                action: Fired::Download,
                active: false,
                swatch: None,
                desc: None,
                preview: None,
            });
        }
        out.push(Item {
            section: "Actions",
            label: "Duplicate".into(),
            action: Fired::Action(MenuAction::Duplicate),
            active: false,
            swatch: None,
            desc: None,
            preview: None,
        });
        out.push(Item {
            section: "Actions",
            label: "Delete".into(),
            action: Fired::Action(MenuAction::Delete),
            active: false,
            swatch: None,
            desc: None,
            preview: None,
        });
        let f = self.filter.read(cx).value().trim().to_lowercase();
        if f.is_empty() {
            return out;
        }
        out.into_iter()
            .filter(|i| {
                i.label.to_lowercase().contains(&f) || i.section.to_lowercase().contains(&f)
            })
            .collect()
    }

    fn row_base(id: impl Into<ElementId>) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(8.))
            .w_full()
            .px(px(10.))
            .py(px(7.))
            .rounded(px(6.))
            .text_size(px(13.))
            .text_color(theme::fg())
            .cursor_pointer()
            .hover(|s| s.bg(theme::hover()))
    }

    fn check() -> Div {
        div().ml_auto().child("✓")
    }

    fn sub_row(
        &self,
        kind: SubKind,
        glyph: Glyph,
        label: &'static str,
        trail: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.sub.map(|s| s.0) == Some(kind);
        let tops = self.row_tops.clone();
        Self::row_base(SharedString::from(format!("sub-{label}")))
            .relative()
            .when(open, |d| d.bg(theme::hover()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hover_sub(kind, cx);
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_sub(kind, cx)))
            .child(
                canvas(
                    move |b, _, _| {
                        tops.borrow_mut().insert(kind, b.top());
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size(px(1.)),
            )
            .child(Icon::new(glyph).size(px(16.)))
            .child(label)
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child(trail)
                    .child(Icon::new(Glyph::ChevronRight).size(px(14.))),
            )
            .into_any_element()
    }

    fn plain_row(
        &self,
        id: &'static str,
        glyph: Glyph,
        label: &'static str,
        fired: Fired,
        danger: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Self::row_base(id)
            .when(danger, |d| d.text_color(rgb(0xf55522)))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hover_plain(cx);
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.fire(fired.clone(), cx)))
            .child(Icon::new(glyph).size(px(16.)))
            .child(label)
            .into_any_element()
    }

    fn dot(color: Hsla, ring: bool) -> Div {
        div()
            .size(px(14.))
            .rounded_full()
            .flex_none()
            .bg(color)
            .when(ring, |d| d.border_1().border_color(theme::border()))
    }

    fn flyout(&self, kind: SubKind, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        let t = self.text().cloned();
        match kind {
            SubKind::Style => {
                let Some(t) = t else { return rows };
                for (i, (label, value, desc, preview, cls)) in STYLES.iter().enumerate() {
                    let active = t.style == *value;
                    let fired = Fired::Action(MenuAction::Style(*value));
                    rows.push(
                        Self::row_base(("fly-style", i))
                            .when(active, |d| d.text_color(theme::accent()))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.fire(fired.clone(), cx)),
                            )
                            .child(
                                preview_tile(preview, *cls, px(34.), px(30.), theme::bg())
                                    .text_color(if *cls == Preview::P {
                                        theme::muted()
                                    } else {
                                        theme::fg()
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.))
                                    .min_w_0()
                                    .child(div().text_size(px(13.)).child(*label))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme::muted())
                                            .child(*desc),
                                    ),
                            )
                            .when(active, |d| d.child(Self::check()))
                            .into_any_element(),
                    );
                }
            }
            SubKind::Align => {
                let align = self.block.align.unwrap_or(0);
                for (i, (label, value)) in ALIGNS.iter().enumerate() {
                    let active = align == *value;
                    let fired = Fired::Action(MenuAction::Align(*value));
                    rows.push(
                        Self::row_base(("fly-align", i))
                            .when(active, |d| d.text_color(theme::accent()))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.fire(fired.clone(), cx)),
                            )
                            .child(*label)
                            .when(active, |d| d.child(Self::check()))
                            .into_any_element(),
                    );
                }
            }
            SubKind::Color => {
                let Some(t) = t else { return rows };
                let cur = t.color.unwrap_or_default();
                for (i, (name, text, _)) in COLORS.iter().enumerate() {
                    let active = cur == *text;
                    let fired = Fired::Action(MenuAction::Color(text.to_string()));
                    rows.push(
                        Self::row_base(("fly-color", i))
                            .when(active, |d| d.text_color(theme::accent()))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.fire(fired.clone(), cx)),
                            )
                            .child(Self::dot(swatch_color(text, theme::fg()), false))
                            .child(capitalize(name))
                            .when(active, |d| d.child(Self::check()))
                            .into_any_element(),
                    );
                }
            }
            SubKind::Background => {
                let cur = self.block.background_color.clone().unwrap_or_default();
                for (i, (name, _, bg)) in COLORS.iter().enumerate() {
                    let active = cur == *bg;
                    let fired = Fired::Action(MenuAction::Background(bg.to_string()));
                    rows.push(
                        Self::row_base(("fly-bg", i))
                            .when(active, |d| d.text_color(theme::accent()))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.fire(fired.clone(), cx)),
                            )
                            .child(Self::dot(
                                swatch_color(bg, gpui_kit::transparent_black()),
                                true,
                            ))
                            .child(capitalize(name))
                            .when(active, |d| d.child(Self::check()))
                            .into_any_element(),
                    );
                }
            }
            SubKind::Turn => {
                for (i, ty) in roostr_core::create::creatable_types(cx)
                    .into_iter()
                    .enumerate()
                {
                    let fired = Fired::Action(MenuAction::TurnIntoObject(ty.key.clone()));
                    rows.push(
                        Self::row_base(("fly-turn", i))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.fire(fired.clone(), cx)),
                            )
                            .child(div().w(px(18.)).flex_none().text_center().child(ty.icon))
                            .child(ty.name)
                            .into_any_element(),
                    );
                }
            }
        }
        rows
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

impl Render for BlockMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let h = px(440.).min(viewport.height - px(40.));
        let left = px(8.).max(self.at.x.min(viewport.width - px(MENU_W) - px(12.)));
        let top = self.at.y.min(viewport.height - h - px(12.));
        let filtering = !self.filter.read(cx).value().trim().is_empty();
        let t = self.text().cloned();

        let mut scroll = div()
            .id("block-menu-scroll")
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .p(px(4.));
        if filtering {
            let items = self.items(cx);
            if items.is_empty() {
                scroll = scroll.child(
                    div()
                        .p(px(12.))
                        .text_size(px(13.))
                        .text_color(theme::muted())
                        .child("No matching actions"),
                );
            }
            let mut last = "";
            for (i, item) in items.into_iter().enumerate() {
                if item.section != last {
                    last = item.section;
                    scroll = scroll.child(section_label(item.section).px(px(10.)));
                }
                let fired = item.action.clone();
                let danger = item.label.as_ref() == "Delete";
                let mut row = Self::row_base(("flat", i))
                    .when(item.active, |d| d.text_color(theme::accent()))
                    .when(danger, |d| d.text_color(rgb(0xf55522)))
                    .on_click(cx.listener(move |this, _, _, cx| this.fire(fired.clone(), cx)));
                if let Some(sw) = item.swatch {
                    row = row.child(
                        div()
                            .size(px(14.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme::border())
                            .flex_none()
                            .bg(sw),
                    );
                }
                if let Some((preview, cls)) = item.preview {
                    row = row
                        .child(preview_tile(preview, cls, px(34.), px(30.), theme::bg()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .min_w_0()
                                .child(div().text_size(px(13.)).child(item.label.clone()))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme::muted())
                                        .child(item.desc.unwrap_or("")),
                                ),
                        );
                } else {
                    // Filter mode shows labels as stored (palette names stay lowercase).
                    row = row.child(item.label.clone());
                }
                if item.active {
                    row = row.child(Self::check());
                }
                scroll = scroll.child(row);
            }
        } else {
            if self.is_link() {
                scroll = scroll.child(section_label("Appearance").px(px(10.)));
                let cur = self
                    .block
                    .content
                    .custom
                    .as_ref()
                    .and_then(|c| c.meta.as_ref())
                    .and_then(|m| m.get("style").cloned())
                    .unwrap_or_else(|| "text".into());
                for (label, value) in [("Text", "text"), ("Card", "card")] {
                    let active = cur == value;
                    let fired = Fired::Action(MenuAction::LinkStyle(value.into()));
                    scroll = scroll.child(
                        Self::row_base(SharedString::from(format!("link-{value}")))
                            .when(active, |d| d.text_color(theme::accent()))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.fire(fired.clone(), cx)),
                            )
                            .child(label)
                            .when(active, |d| d.child(Self::check())),
                    );
                }
            }
            scroll =
                scroll.child(section_label(if t.is_some() { "Text" } else { "Block" }).px(px(10.)));
            if let Some(t) = &t {
                let cur = STYLES
                    .iter()
                    .find(|s| s.1 == t.style)
                    .map_or("Text", |s| s.0);
                scroll = scroll.child(self.sub_row(
                    SubKind::Style,
                    Glyph::Type,
                    "Style",
                    div().child(cur).into_any_element(),
                    cx,
                ));
            }
            let cur_align = ALIGNS
                .iter()
                .find(|a| a.1 == self.block.align.unwrap_or(0))
                .map_or("Left", |a| a.0);
            scroll = scroll.child(self.sub_row(
                SubKind::Align,
                Glyph::AlignLeft,
                "Align",
                div().child(cur_align).into_any_element(),
                cx,
            ));
            if let Some(t) = &t {
                let c = t.color.clone().unwrap_or_default();
                let color = COLORS.iter().find(|x| x.1 == c).map_or("", |x| x.1);
                scroll = scroll.child(self.sub_row(
                    SubKind::Color,
                    Glyph::Palette,
                    "Color",
                    Self::dot(swatch_color(color, theme::fg()), true).into_any_element(),
                    cx,
                ));
            }
            let b = self.block.background_color.clone().unwrap_or_default();
            let bg = COLORS.iter().find(|x| x.2 == b).map_or("", |x| x.2);
            scroll = scroll.child(self.sub_row(
                SubKind::Background,
                Glyph::PaintBucket,
                "Background",
                Self::dot(swatch_color(bg, gpui_kit::transparent_black()), true).into_any_element(),
                cx,
            ));
            if t.is_some() {
                scroll = scroll.child(self.plain_row(
                    "clear",
                    Glyph::Eraser,
                    "Clear style",
                    Fired::Action(MenuAction::ClearStyle),
                    false,
                    cx,
                ));
            }
            scroll = scroll.child(div().h(px(1.)).bg(theme::border()).mx(px(6.)).my(px(4.)));
            scroll = scroll.child(self.sub_row(
                SubKind::Turn,
                Glyph::Box,
                "Turn into object",
                div().into_any_element(),
                cx,
            ));
            if let Some(meta) = self.file_meta() {
                if meta.get("fileId").is_some_and(|f| !f.is_empty()) {
                    scroll = scroll.child(self.plain_row(
                        "open-file",
                        Glyph::ExternalLink,
                        "Open file",
                        Fired::OpenFile,
                        false,
                        cx,
                    ));
                }
                if meta.get("mime").is_some_and(|m| m.starts_with("image/")) {
                    scroll = scroll.child(self.plain_row(
                        "copy-image",
                        Glyph::Copy,
                        "Copy image",
                        Fired::CopyImage,
                        false,
                        cx,
                    ));
                }
                scroll = scroll.child(self.plain_row(
                    "download",
                    Glyph::Download,
                    "Download",
                    Fired::Download,
                    false,
                    cx,
                ));
            }
            scroll = scroll.child(self.plain_row(
                "duplicate",
                Glyph::Copy,
                "Duplicate",
                Fired::Action(MenuAction::Duplicate),
                false,
                cx,
            ));
            scroll = scroll.child(self.plain_row(
                "delete",
                Glyph::Trash,
                "Delete block",
                Fired::Action(MenuAction::Delete),
                true,
                cx,
            ));
        }

        let bounds = self.bounds.clone();
        bounds.borrow_mut().clear();
        let record = |b: Rc<RefCell<Vec<Bounds<Pixels>>>>| {
            canvas(
                move |bounds, _, _| b.borrow_mut().push(bounds),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };
        let appear = roostr_core::motion::appear_frame("block-menu-appear", -4., window, cx);
        let menu = div()
            .id("block-menu")
            .occlude()
            .relative()
            .w(px(MENU_W))
            .max_h(px(440.))
            .flex()
            .flex_col()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(px(10.))
            .shadow(roostr_core::ui::menu_shadow())
            .overflow_hidden()
            .child(record(bounds.clone()))
            .child(
                div()
                    .border_b_1()
                    .border_color(theme::border())
                    .px(px(4.))
                    .py(px(2.))
                    .child(
                        Input::new(&self.filter)
                            .appearance(false)
                            .text_size(px(13.)),
                    ),
            )
            .when(self.group_count > 1, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme::accent())
                        .px(px(12.))
                        .pt(px(6.))
                        .child(format!("Applies to {} selected blocks", self.group_count)),
                )
            })
            .child(scroll);

        let flyout = self.sub.map(|(kind, sub_top)| {
            let fh = px(420.).min(viewport.height - px(24.));
            let right_x = left + px(MENU_W) - px(6.);
            let fl = if right_x + px(SUB_W) + px(12.) <= viewport.width {
                right_x
            } else {
                px(8.).max(left - px(SUB_W) + px(6.))
            };
            let ft = px(8.).max(sub_top.min(viewport.height - fh - px(12.)));
            let rows = self.flyout(kind, cx);
            let appear = roostr_core::motion::appear_frame(
                ElementId::Name(format!("block-flyout-appear-{kind:?}").into()),
                -4.,
                window,
                cx,
            );
            deferred(
                anchored().position(point(fl, ft)).child(
                    appear.apply(
                        div()
                            .id("block-menu-flyout")
                            .occlude()
                            .relative()
                            .w(px(SUB_W))
                            .max_h(px(420.))
                            .overflow_y_scroll()
                            .bg(theme::panel())
                            .border_1()
                            .border_color(theme::border())
                            .rounded(px(10.))
                            .shadow(roostr_core::ui::menu_shadow())
                            .p(px(4.))
                            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                                // Reaching the flyout cancels the pending close from rows crossed on the way.
                                if *hovered {
                                    this.pending = None;
                                }
                            }))
                            .child(record(bounds.clone()))
                            .children(rows),
                    ),
                ),
            )
            .with_priority(4)
        });

        let entity = cx.entity();
        div()
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    if this.sub.is_some() {
                        this.close_sub();
                        cx.notify();
                    } else {
                        cx.emit(MenuEvent::Close);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let bounds = bounds.clone();
                        let entity = entity.clone();
                        window.on_mouse_event(move |ev: &MouseDownEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            if !bounds.borrow().iter().any(|b| b.contains(&ev.position)) {
                                entity.update(cx, |_, cx| cx.emit(MenuEvent::Close));
                            }
                        });
                    },
                )
                .size_0(),
            )
            // Not deferred: the filter input must stay on the focus path (keys/actions).
            .child(
                anchored()
                    .position(point(left, top))
                    .child(appear.apply(menu)),
            )
            .children(flyout)
    }
}

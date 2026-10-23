//! The block action menu (`BlockMenu.svelte`, Anytype's blockAction): a
//! filter box, root rows with submenu flyouts (Style, Align, Color,
//! Background, Turn into object), the File block's own actions, Duplicate
//! and Delete. Typing in the filter flattens everything into one list.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{Disableable as _, Icon, Side};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::types::{Block, style};
use roostr_core::{Glyph, Store, theme};

use crate::slash_menu::{Preview, preview_tile};

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
    /// The kit command list: its query field is the filter box.
    cmd: Entity<CommandState>,
    sub: Option<(SubKind, Pixels)>,
    /// The open flyout's kit menu, rebuilt when another flyout opens.
    flyout: Option<(SubKind, Entity<PopupMenu>)>,
    hover_opened: bool,
    pending: Option<Task<()>>,
    /// Row tops (window y) of the submenu rows, for the flyout.
    row_tops: Rc<RefCell<HashMap<SubKind, Pixels>>>,
    /// Menu + flyout bounds, for click-outside.
    bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

/// What a list row does when clicked or confirmed with Enter.
#[derive(Clone)]
enum RowAct {
    Sub(SubKind),
    Fire(Fired),
    None,
}

/// The value shown at the end of a submenu row.
#[derive(Clone)]
enum Trail {
    Text(&'static str),
    Dot(Hsla),
    None,
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
        let cmd = cx.new(|cx| CommandState::new(window, cx));
        cmd.update(cx, |cmd, cx| {
            // No row lit until the pointer or a key picks one.
            cmd.set_selected_index(None, window, cx);
            cmd.focus(window, cx);
        });
        Self {
            block,
            at,
            group_count,
            cmd,
            sub: None,
            flyout: None,
            hover_opened: false,
            pending: None,
            row_tops: Rc::default(),
            bounds: Rc::default(),
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
        let f = self.cmd.read(cx).query(cx).trim().to_lowercase();
        if f.is_empty() {
            return out;
        }
        out.into_iter()
            .filter(|i| {
                i.label.to_lowercase().contains(&f) || i.section.to_lowercase().contains(&f)
            })
            .collect()
    }

    /// A list row's content. It spans the whole kit row (over the row's own
    /// 8/6 padding) so hover and Roostr's 10/7 spacing cover all of it, and a
    /// click on it leaves focus in the filter field.
    fn row(id: impl Into<ElementId>) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .flex_1()
            .items_center()
            .gap(px(8.))
            .mx(px(-8.))
            .my(px(-6.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(6.))
            .text_size(px(13.))
            .text_color(theme::fg())
            .cursor_pointer()
            .capture_any_mouse_down(|_, window, _| window.prevent_default())
    }

    /// A flyout row's content inside its kit menu item.
    fn fly_row(id: impl Into<ElementId>, active: bool) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .flex_1()
            .items_center()
            .gap(px(8.))
            .ml(px(2.))
            .py(px(7.))
            .text_size(px(13.))
            .cursor_pointer()
            .text_color(if active { theme::accent() } else { theme::fg() })
    }

    fn check() -> Div {
        div().ml_auto().child("✓")
    }

    fn dot(color: Hsla, ring: bool) -> Div {
        div()
            .size(px(14.))
            .rounded_full()
            .flex_none()
            .bg(color)
            .when(ring, |d| d.border_1().border_color(theme::border()))
    }

    /// A row that opens a flyout: on hover (with the 220ms intent delay)
    /// or on click.
    fn sub_item(
        &self,
        kind: SubKind,
        glyph: Glyph,
        label: &'static str,
        trail: Trail,
        cx: &Context<Self>,
    ) -> CommandItem {
        let open = self.sub.map(|s| s.0) == Some(kind);
        let tops = self.row_tops.clone();
        let this = cx.entity().downgrade();
        CommandItem::new().label(label).child(move |_, _| {
            let tops = tops.clone();
            let this = this.clone();
            Self::row(SharedString::from(format!("sub-{label}")))
                .relative()
                .when(open, |d| d.bg(theme::hover()))
                .on_hover(move |hovered, _, cx| {
                    if *hovered {
                        let _ = this.update(cx, |this, cx| this.hover_sub(kind, cx));
                    }
                })
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
                        .map(|d| match trail {
                            Trail::Text(text) => d.child(text),
                            Trail::Dot(color) => d.child(Self::dot(color, true)),
                            Trail::None => d,
                        })
                        .child(Icon::new(Glyph::ChevronRight).size(px(14.))),
                )
        })
    }

    /// A row that acts at once; hovering it closes an open flyout after 220ms.
    fn plain_item(
        &self,
        id: &'static str,
        glyph: Glyph,
        label: &'static str,
        danger: bool,
        cx: &Context<Self>,
    ) -> CommandItem {
        let this = cx.entity().downgrade();
        CommandItem::new().label(label).child(move |_, _| {
            let this = this.clone();
            Self::row(id)
                .when(danger, |d| d.text_color(rgb(0xf55522)))
                .on_hover(move |hovered, _, cx| {
                    if *hovered {
                        let _ = this.update(cx, |this, cx| this.hover_plain(cx));
                    }
                })
                .child(Icon::new(glyph).size(px(16.)))
                .child(label)
        })
    }

    /// The flyout for `kind`: a kit menu of the choices, the current one checked.
    fn build_flyout(
        &self,
        kind: SubKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PopupMenu> {
        let this = cx.entity().downgrade();
        let t = self.text().cloned();
        let align = self.block.align.unwrap_or(0);
        let bg_cur = self.block.background_color.clone().unwrap_or_default();
        let types = if kind == SubKind::Turn {
            roostr_core::create::creatable_types(cx)
        } else {
            Vec::new()
        };
        PopupMenu::build(window, cx, move |mut m, _, _| {
            m = m
                .min_w(px(SUB_W - 2.))
                .max_w(px(SUB_W - 2.))
                .max_h(px(418.))
                .scrollable(true)
                .check_side(Side::Right);
            let on = |fired: Fired| fire_on(this.clone(), fired);
            match kind {
                SubKind::Style => {
                    let Some(t) = &t else { return m };
                    for (i, (label, value, desc, preview, cls)) in STYLES.iter().enumerate() {
                        let active = t.style == *value;
                        m = m.item(
                            PopupMenuItem::element(move |_, _| {
                                Self::fly_row(("fly-style", i), active)
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
                            })
                            .checked(active)
                            .on_click(on(Fired::Action(MenuAction::Style(*value)))),
                        );
                    }
                }
                SubKind::Align => {
                    for (i, (label, value)) in ALIGNS.iter().enumerate() {
                        let active = align == *value;
                        m = m.item(
                            PopupMenuItem::element(move |_, _| {
                                Self::fly_row(("fly-align", i), active).child(*label)
                            })
                            .checked(active)
                            .on_click(on(Fired::Action(MenuAction::Align(*value)))),
                        );
                    }
                }
                SubKind::Color => {
                    let Some(t) = &t else { return m };
                    let cur = t.color.clone().unwrap_or_default();
                    for (i, (name, text, _)) in COLORS.iter().enumerate() {
                        let active = cur == *text;
                        m = m.item(
                            PopupMenuItem::element(move |_, _| {
                                Self::fly_row(("fly-color", i), active)
                                    .child(Self::dot(swatch_color(text, theme::fg()), false))
                                    .child(capitalize(name))
                            })
                            .checked(active)
                            .on_click(on(Fired::Action(MenuAction::Color(text.to_string())))),
                        );
                    }
                }
                SubKind::Background => {
                    for (i, (name, _, bg)) in COLORS.iter().enumerate() {
                        let active = bg_cur == *bg;
                        m = m.item(
                            PopupMenuItem::element(move |_, _| {
                                Self::fly_row(("fly-bg", i), active)
                                    .child(Self::dot(
                                        swatch_color(bg, gpui_kit::transparent_black()),
                                        true,
                                    ))
                                    .child(capitalize(name))
                            })
                            .checked(active)
                            .on_click(on(Fired::Action(MenuAction::Background(bg.to_string())))),
                        );
                    }
                }
                SubKind::Turn => {
                    for (i, ty) in types.iter().enumerate() {
                        let (icon, name) = (ty.icon.clone(), ty.name.clone());
                        m = m.item(
                            PopupMenuItem::element(move |_, _| {
                                Self::fly_row(("fly-turn", i), false)
                                    .child(
                                        div()
                                            .w(px(18.))
                                            .flex_none()
                                            .text_center()
                                            .child(icon.clone()),
                                    )
                                    .child(name.clone())
                            })
                            .on_click(on(Fired::Action(
                                MenuAction::TurnIntoObject(ty.key.clone()),
                            ))),
                        );
                    }
                }
            }
            m
        })
    }

    /// The list's groups (`None` is a separator) and what each row does.
    fn groups(&self, cx: &Context<Self>) -> Vec<Option<(CommandGroup, Vec<RowAct>)>> {
        let mut groups = Vec::new();
        if self.group_count > 1 {
            let n = self.group_count;
            groups.push(Some((
                CommandGroup::new().item(CommandItem::new().disabled(true).child(move |_, _| {
                    div()
                        .ml(px(4.))
                        .text_size(px(11.))
                        .text_color(theme::accent())
                        .child(format!("Applies to {n} selected blocks"))
                })),
                vec![RowAct::None],
            )));
        }
        let filtering = !self.cmd.read(cx).query(cx).trim().is_empty();
        if filtering {
            let items = self.items(cx);
            if items.is_empty() {
                groups.push(Some((
                    CommandGroup::new().item(CommandItem::new().disabled(true).child(|_, _| {
                        div()
                            .p(px(4.))
                            .text_size(px(13.))
                            .text_color(theme::muted())
                            .child("No matching actions")
                    })),
                    vec![RowAct::None],
                )));
            }
            let mut current: Option<(&'static str, Vec<CommandItem>, Vec<RowAct>)> = None;
            let close = |(section, rows, acts): (&'static str, Vec<CommandItem>, Vec<RowAct>)| {
                Some((
                    CommandGroup::new()
                        .label(section.to_uppercase())
                        .items(rows),
                    acts,
                ))
            };
            for (i, item) in items.into_iter().enumerate() {
                if current.as_ref().map(|c| c.0) != Some(item.section) {
                    if let Some(done) = current.take() {
                        groups.push(close(done));
                    }
                    current = Some((item.section, Vec::new(), Vec::new()));
                }
                let Item {
                    label,
                    action,
                    active,
                    swatch,
                    desc,
                    preview,
                    ..
                } = item;
                let danger = label.as_ref() == "Delete";
                let row = CommandItem::new().label(label.clone()).child(move |_, _| {
                    let row = Self::row(("flat", i))
                        .when(active, |d| d.text_color(theme::accent()))
                        .when(danger, |d| d.text_color(rgb(0xf55522)))
                        .when_some(swatch, |d, sw| {
                            d.child(
                                div()
                                    .size(px(14.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .flex_none()
                                    .bg(sw),
                            )
                        });
                    let row = match preview {
                        Some((preview, cls)) => row
                            .child(preview_tile(preview, cls, px(34.), px(30.), theme::bg()))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.))
                                    .min_w_0()
                                    .child(div().text_size(px(13.)).child(label.clone()))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme::muted())
                                            .child(desc.unwrap_or("")),
                                    ),
                            ),
                        // Filter mode shows labels as stored (palette names stay lowercase).
                        None => row.child(label.clone()),
                    };
                    row.when(active, |d| d.child(Self::check()))
                });
                if let Some((_, rows, acts)) = current.as_mut() {
                    rows.push(row);
                    acts.push(RowAct::Fire(action));
                }
            }
            if let Some(done) = current {
                groups.push(close(done));
            }
            return groups;
        }

        let t = self.text().cloned();
        if self.is_link() {
            let cur = self
                .block
                .content
                .custom
                .as_ref()
                .and_then(|c| c.meta.as_ref())
                .and_then(|m| m.get("style").cloned())
                .unwrap_or_else(|| "text".into());
            let mut g = CommandGroup::new().label("APPEARANCE");
            let mut acts = Vec::new();
            for (label, value) in [("Text", "text"), ("Card", "card")] {
                let active = cur == value;
                g = g.item(CommandItem::new().label(label).child(move |_, _| {
                    Self::row(SharedString::from(format!("link-{value}")))
                        .when(active, |d| d.text_color(theme::accent()))
                        .child(label)
                        .when(active, |d| d.child(Self::check()))
                }));
                acts.push(RowAct::Fire(Fired::Action(MenuAction::LinkStyle(
                    value.into(),
                ))));
            }
            groups.push(Some((g, acts)));
        }
        let mut g = CommandGroup::new().label(if t.is_some() { "TEXT" } else { "BLOCK" });
        let mut acts = Vec::new();
        if let Some(t) = &t {
            let cur = STYLES
                .iter()
                .find(|s| s.1 == t.style)
                .map_or("Text", |s| s.0);
            g = g.item(self.sub_item(SubKind::Style, Glyph::Type, "Style", Trail::Text(cur), cx));
            acts.push(RowAct::Sub(SubKind::Style));
        }
        let cur_align = ALIGNS
            .iter()
            .find(|a| a.1 == self.block.align.unwrap_or(0))
            .map_or("Left", |a| a.0);
        g = g.item(self.sub_item(
            SubKind::Align,
            Glyph::AlignLeft,
            "Align",
            Trail::Text(cur_align),
            cx,
        ));
        acts.push(RowAct::Sub(SubKind::Align));
        if let Some(t) = &t {
            let c = t.color.clone().unwrap_or_default();
            let color = COLORS.iter().find(|x| x.1 == c).map_or("", |x| x.1);
            g = g.item(self.sub_item(
                SubKind::Color,
                Glyph::Palette,
                "Color",
                Trail::Dot(swatch_color(color, theme::fg())),
                cx,
            ));
            acts.push(RowAct::Sub(SubKind::Color));
        }
        let b = self.block.background_color.clone().unwrap_or_default();
        let bg = COLORS.iter().find(|x| x.2 == b).map_or("", |x| x.2);
        g = g.item(self.sub_item(
            SubKind::Background,
            Glyph::PaintBucket,
            "Background",
            Trail::Dot(swatch_color(bg, gpui_kit::transparent_black())),
            cx,
        ));
        acts.push(RowAct::Sub(SubKind::Background));
        if t.is_some() {
            g = g.item(self.plain_item("clear", Glyph::Eraser, "Clear style", false, cx));
            acts.push(RowAct::Fire(Fired::Action(MenuAction::ClearStyle)));
        }
        groups.push(Some((g, acts)));
        groups.push(None);

        let mut g = CommandGroup::new().item(self.sub_item(
            SubKind::Turn,
            Glyph::Box,
            "Turn into object",
            Trail::None,
            cx,
        ));
        let mut acts = vec![RowAct::Sub(SubKind::Turn)];
        if let Some(meta) = self.file_meta() {
            if meta.get("fileId").is_some_and(|f| !f.is_empty()) {
                g = g.item(self.plain_item(
                    "open-file",
                    Glyph::ExternalLink,
                    "Open file",
                    false,
                    cx,
                ));
                acts.push(RowAct::Fire(Fired::OpenFile));
            }
            if meta.get("mime").is_some_and(|m| m.starts_with("image/")) {
                g = g.item(self.plain_item("copy-image", Glyph::Copy, "Copy image", false, cx));
                acts.push(RowAct::Fire(Fired::CopyImage));
            }
            g = g.item(self.plain_item("download", Glyph::Download, "Download", false, cx));
            acts.push(RowAct::Fire(Fired::Download));
        }
        g = g.item(self.plain_item("duplicate", Glyph::Copy, "Duplicate", false, cx));
        acts.push(RowAct::Fire(Fired::Action(MenuAction::Duplicate)));
        g = g.item(self.plain_item("delete", Glyph::Trash, "Delete block", true, cx));
        acts.push(RowAct::Fire(Fired::Action(MenuAction::Delete)));
        groups.push(Some((g, acts)));
        groups
    }
}

/// A flyout pick: run `fired` on the menu.
fn fire_on(
    this: WeakEntity<BlockMenu>,
    fired: Fired,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| {
        let _ = this.update(cx, |this, cx| this.fire(fired.clone(), cx));
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

        let groups = self.groups(cx);
        let acts: Rc<Vec<Vec<RowAct>>> =
            Rc::new(groups.iter().flatten().map(|(_, a)| a.clone()).collect());
        let this = cx.entity().downgrade();
        let on_query = this.clone();
        let mut list = Command::new(&self.cmd)
            // The menu answers the filter itself: typing flattens the menu.
            .filterable(false)
            .bordered(false)
            .placeholder("Filter actions…")
            .max_h(px(390.))
            .on_query(move |_, _, cx| {
                let _ = on_query.update(cx, |this, cx| {
                    this.close_sub();
                    cx.notify();
                });
            })
            .on_confirm(move |ix, _, cx| {
                let Some(act) = acts.get(ix.section).and_then(|s| s.get(ix.row)).cloned() else {
                    return;
                };
                let _ = this.update(cx, |this, cx| match act {
                    RowAct::Sub(kind) => this.toggle_sub(kind, cx),
                    RowAct::Fire(fired) => this.fire(fired, cx),
                    RowAct::None => {}
                });
            });
        for group in groups {
            list = match group {
                Some((group, _)) => list.group(group),
                None => list.separator(),
            };
        }

        match self.sub {
            Some((kind, _)) if self.flyout.as_ref().map(|f| f.0) != Some(kind) => {
                let menu = self.build_flyout(kind, window, cx);
                self.flyout = Some((kind, menu));
            }
            None => self.flyout = None,
            _ => {}
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
            .child(list);

        let flyout = self
            .sub
            .zip(self.flyout.clone())
            .map(|((kind, sub_top), (_, popup))| {
                let fh = px(420.).min(viewport.height - px(24.));
                let right_x = left + px(MENU_W) - px(6.);
                let fl = if right_x + px(SUB_W) + px(12.) <= viewport.width {
                    right_x
                } else {
                    px(8.).max(left - px(SUB_W) + px(6.))
                };
                let ft = px(8.).max(sub_top.min(viewport.height - fh - px(12.)));
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
                                .bg(theme::panel())
                                .border_1()
                                .border_color(theme::border())
                                .rounded(px(10.))
                                .shadow(roostr_core::ui::menu_shadow())
                                .overflow_hidden()
                                // Focus stays in the filter field.
                                .capture_any_mouse_down(|_, window, _| window.prevent_default())
                                .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                                    // Reaching the flyout cancels the pending close from rows crossed on the way.
                                    if *hovered {
                                        this.pending = None;
                                    }
                                }))
                                .child(record(bounds.clone()))
                                .child(popup),
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

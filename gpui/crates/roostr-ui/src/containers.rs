//! gpui-kit's containers as Roostr's screens use them: `GroupBox` (sidebar sections and widget
//! cards, Settings cards), `Collapsible` (editor toggle blocks), `DescriptionList` (a file's
//! facts) and `Breadcrumb` (the header's path pill).

use std::rc::Rc;

use rune::{Color, Cursor, El, TextStyle, Transition, col, el, row, text};

use crate::icons::{Glyph, glyph};
use crate::motion;
use crate::theme::{Theme, radius, rems, ty};

/// gpui-kit's `GroupBoxVariant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupBoxVariant {
    /// No surface: title and content only.
    #[default]
    Normal,
    /// The `group_box` fill, 1 rem padding.
    Fill,
    /// A `border` outline, 1 rem padding.
    Outline,
}

/// gpui-kit's `GroupBox`: an optional muted title over a surface holding the children, with an
/// optional footer under it. The surface's look can be overridden as Roostr does with
/// `content_style` (fill, border, radius, padding, gap).
pub struct GroupBox {
    key: String,
    variant: GroupBoxVariant,
    title: Option<El>,
    footer: Option<El>,
    children: Vec<El>,
    fill: Option<Color>,
    border: Option<(f32, Color)>,
    radius: Option<f32>,
    padding: Option<[f32; 4]>,
    gap: Option<f32>,
    clip: bool,
}

impl GroupBox {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            variant: GroupBoxVariant::Normal,
            title: None,
            footer: None,
            children: Vec::new(),
            fill: None,
            border: None,
            radius: None,
            padding: None,
            gap: None,
            clip: false,
        }
    }
    pub fn variant(mut self, v: GroupBoxVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn fill(self) -> Self {
        self.variant(GroupBoxVariant::Fill)
    }
    pub fn outline(self) -> Self {
        self.variant(GroupBoxVariant::Outline)
    }
    /// A plain-text title (muted, line height 1.25).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(text(title.into()));
        self
    }
    /// A title element of the caller's.
    pub fn title_el(mut self, title: El) -> Self {
        self.title = Some(title);
        self
    }
    /// Text under the surface (`text_sm`, muted), 8 pt below it.
    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(text(footer.into()));
        self
    }
    pub fn child(mut self, c: El) -> Self {
        self.children.push(c);
        self
    }
    pub fn children(mut self, cs: impl IntoIterator<Item = El>) -> Self {
        self.children.extend(cs);
        self
    }
    /// The surface's fill instead of the variant's.
    pub fn content_fill(mut self, c: Color) -> Self {
        self.fill = Some(c);
        self
    }
    pub fn content_border(mut self, w: f32, c: Color) -> Self {
        self.border = Some((w, c));
        self
    }
    pub fn content_radius(mut self, r: f32) -> Self {
        self.radius = Some(r);
        self
    }
    /// The surface's padding, `[top, right, bottom, left]`.
    pub fn content_padding(mut self, p: [f32; 4]) -> Self {
        self.padding = Some(p);
        self
    }
    /// The gap between the children (gpui-kit: 1 rem).
    pub fn content_gap(mut self, g: f32) -> Self {
        self.gap = Some(g);
        self
    }
    /// Clip the children to the surface's rounded corners (`overflow_hidden`).
    pub fn content_clip(mut self) -> Self {
        self.clip = true;
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let (bg, border, paddings) = match self.variant {
            GroupBoxVariant::Normal => (None, None, false),
            GroupBoxVariant::Fill => (Some(t.kit.group_box), None, true),
            GroupBoxVariant::Outline => (None, Some((1.0, t.kit.border)), true),
        };
        let mut root =
            col()
                .key(self.key.clone())
                .w_full()
                .gap(if paddings { rems(0.75) } else { rems(1.0) });
        if let Some(title) = self.title {
            let font = TextStyle {
                line_height: rems(1.0) * 1.25,
                ..ty::ui(rems(1.0))
            };
            root = root.child(title.font(font).color(t.kit.muted_foreground));
        }
        let r = self.radius.unwrap_or(radius::KIT);
        let p = self
            .padding
            .unwrap_or(if paddings { [rems(1.0); 4] } else { [0.0; 4] });
        let mut surface = col()
            .key(format!("{}-content", self.key))
            .gap(self.gap.unwrap_or(rems(1.0)))
            .radius(r)
            .pt(p[0])
            .pr(p[1])
            .pb(p[2])
            .pl(p[3])
            .children(self.children);
        if let Some(c) = self.fill.or(bg) {
            surface = surface.fill(c);
        }
        if let Some((w, c)) = self.border.or(border) {
            surface = surface.border(w, c).border_layout();
        }
        if self.clip {
            surface = surface.clip_rounded(r);
        }
        let mut slot = col().gap(rems(0.5)).child(surface);
        if let Some(f) = self.footer {
            slot = slot.child(f.font(ty::ui(rems(0.875))).color(t.kit.muted_foreground));
        }
        root.child(slot)
    }
}

/// gpui-kit's `Collapsible`: a trigger row and content shown only while open, revealed on
/// [`motion::DISCLOSURE`] (the surrounding layout eases to the new height, the content fades).
pub struct Collapsible<S> {
    key: String,
    open: bool,
    trigger: Vec<El>,
    content: Vec<El>,
    fill: Option<Color>,
    radius: f32,
    on_toggle: Option<Rc<dyn Fn(&mut S)>>,
}

impl<S: 'static> Collapsible<S> {
    /// The trigger row is keyed `{key}-trigger`, the content `{key}-content`.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            open: false,
            trigger: Vec::new(),
            content: Vec::new(),
            fill: None,
            radius: 0.0,
            on_toggle: None,
        }
    }
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
    /// Part of the always-shown trigger.
    pub fn child(mut self, c: El) -> Self {
        self.trigger.push(c);
        self
    }
    /// Part of the content shown while open.
    pub fn content(mut self, c: El) -> Self {
        self.content.push(c);
        self
    }
    pub fn fill(mut self, c: Color) -> Self {
        self.fill = Some(c);
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = r;
        self
    }
    /// Pressing the trigger runs `f` (flip the open flag); the trigger is then a button that
    /// reports its expanded state.
    pub fn on_toggle(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }

    pub fn build(self) -> El {
        let mut trigger = col()
            .key(format!("{}-trigger", self.key))
            .children(self.trigger);
        if let Some(f) = self.on_toggle {
            trigger = trigger
                .role("button")
                .expanded(self.open)
                .focusable(true)
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut S| f(s));
        }
        let mut root = col()
            .key(self.key.clone())
            .role("group")
            .radius(self.radius)
            .clip_rounded(self.radius)
            .motion(motion::DISCLOSURE.spring())
            .child(trigger);
        if let Some(c) = self.fill {
            root = root.fill(c);
        }
        if self.open {
            root = root.child(
                col()
                    .key(format!("{}-content", self.key))
                    .role("region")
                    .transition(Transition {
                        springs: rune::SpringPair::new(
                            motion::DISCLOSURE.spring(),
                            motion::DISCLOSURE.spring(),
                        ),
                        ..Transition::opacity()
                    })
                    .children(self.content),
            );
        }
        root
    }
}

/// A [`DescriptionList`] label or value: text, or an element of the caller's.
pub enum DescriptionText {
    Text(String),
    El(El),
}

impl From<&str> for DescriptionText {
    fn from(s: &str) -> Self {
        DescriptionText::Text(s.to_owned())
    }
}
impl From<String> for DescriptionText {
    fn from(s: String) -> Self {
        DescriptionText::Text(s)
    }
}
impl From<El> for DescriptionText {
    fn from(e: El) -> Self {
        DescriptionText::El(e)
    }
}

enum DescriptionItem {
    Item {
        label: DescriptionText,
        value: DescriptionText,
        span: usize,
    },
    Separator,
}

/// gpui-kit's `DescriptionList`: label / value pairs in `columns` per row (items span one or
/// more), labels beside the values (horizontal, `label_width` wide) or above them, bordered
/// (label cells on `description_list_label`) or not.
pub struct DescriptionList {
    items: Vec<DescriptionItem>,
    size: crate::KitSize,
    vertical: bool,
    label_width: f32,
    bordered: bool,
    columns: usize,
}

impl DescriptionList {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            size: crate::KitSize::Medium,
            vertical: false,
            label_width: 120.0,
            bordered: true,
            columns: 3,
        }
    }
    pub fn horizontal() -> Self {
        Self::new()
    }
    pub fn vertical() -> Self {
        Self {
            vertical: true,
            ..Self::new()
        }
    }
    pub fn size(mut self, s: crate::KitSize) -> Self {
        self.size = s;
        self
    }
    pub fn label_width(mut self, w: f32) -> Self {
        self.label_width = w;
        self
    }
    pub fn bordered(mut self, b: bool) -> Self {
        self.bordered = b;
        self
    }
    pub fn columns(mut self, n: usize) -> Self {
        self.columns = n.clamp(1, 10);
        self
    }
    pub fn item(
        mut self,
        label: impl Into<DescriptionText>,
        value: impl Into<DescriptionText>,
        span: usize,
    ) -> Self {
        self.items.push(DescriptionItem::Item {
            label: label.into(),
            value: value.into(),
            span,
        });
        self
    }
    pub fn separator(mut self) -> Self {
        self.items.push(DescriptionItem::Separator);
        self
    }

    pub fn build(self, t: &Theme) -> El {
        use crate::KitSize::*;
        let base_gap = match self.size {
            XSmall | Small => 2.0,
            Large => 8.0,
            _ => 4.0,
        };
        let (mut px, mut py) = match self.size {
            XSmall | Small => (4.0, 2.0),
            Large => (12.0, 6.0),
            _ => (8.0, 4.0),
        };
        if !self.bordered {
            (px, py) = (0.0, 0.0);
        }
        let cols = self.columns;
        // Items into rows of at most `columns` spans.
        let mut rows: Vec<Vec<DescriptionItem>> = Vec::new();
        let mut used = 0;
        for item in self.items {
            let span = match &item {
                DescriptionItem::Item { span, .. } => *span,
                DescriptionItem::Separator => cols,
            };
            if rows.is_empty() || used + span > cols {
                rows.push(Vec::new());
                used = 0;
            }
            rows.last_mut().expect("a row").push(item);
            used += span;
        }
        let n = rows.len();
        let mut list = col().gap(if self.bordered { 0.0 } else { base_gap });
        if self.bordered {
            list = list
                .radius(radius::KIT)
                .border(1.0, t.kit.border)
                .border_layout()
                .clip_rounded(radius::KIT);
        }
        let label_font = ty::ui(rems(0.875));
        let value_font = ty::ui(rems(1.0));
        for (ix, items) in rows.into_iter().enumerate() {
            let mut r = row().w_full().items_stretch();
            if self.bordered && ix + 1 < n {
                r = r
                    .border_sides([0.0, 0.0, 1.0, 0.0], t.kit.border)
                    .border_layout();
            }
            for (col_ix, item) in items.into_iter().enumerate() {
                let DescriptionItem::Item { label, value, span } = item else {
                    let mut sep = el().h(rems(0.5)).w_full();
                    if self.bordered {
                        sep = sep.fill(t.kit.description_list_label);
                    }
                    r = r.child(sep);
                    continue;
                };
                let mut lab = col().px(px).py(py).child(match label {
                    DescriptionText::Text(s) => text(s)
                        .font(label_font)
                        .color(t.kit.description_list_label_foreground),
                    DescriptionText::El(e) => e,
                });
                if self.bordered {
                    let sides = if self.vertical {
                        [0.0, 0.0, 1.0, 0.0]
                    } else {
                        [0.0, 1.0, 0.0, if col_ix == 0 { 0.0 } else { 1.0 }]
                    };
                    lab = lab
                        .fill(t.kit.description_list_label)
                        .border_sides(sides, t.kit.border)
                        .border_layout();
                }
                if !self.vertical {
                    lab = lab.w(self.label_width);
                }
                let val = col()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .px(px)
                    .py(py)
                    .clip_rounded(0.0)
                    .child(match value {
                        DescriptionText::Text(s) => {
                            text(s).font(value_font).color(t.kit.foreground)
                        }
                        DescriptionText::El(e) => e,
                    });
                let cell = if self.vertical {
                    col()
                } else {
                    row().items_stretch()
                };
                r = r.child(
                    cell.w_frac(span as f32 / cols as f32)
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .child(lab)
                        .child(val),
                );
            }
            list = list.child(r);
        }
        list
    }
}

impl Default for DescriptionList {
    fn default() -> Self {
        Self::new()
    }
}

/// One crumb of a [`Breadcrumb`].
pub struct BreadcrumbItem<S> {
    label: String,
    color: Option<Color>,
    truncate: bool,
    disabled: bool,
    on_click: Option<Rc<dyn Fn(&mut S)>>,
}

impl<S: 'static> BreadcrumbItem<S> {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            color: None,
            truncate: false,
            disabled: false,
            on_click: None,
        }
    }
    /// The crumb's colour instead of muted (last: foreground).
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// Ellipsise the label when the row is short of room.
    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    pub fn on_click(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

/// gpui-kit's `Breadcrumb`: crumbs (muted, the last in the foreground) between chevrons.
pub struct Breadcrumb<S> {
    key: String,
    items: Vec<BreadcrumbItem<S>>,
    size: f32,
}

impl<S: 'static> Breadcrumb<S> {
    /// Crumbs are keyed `{key}-{ix}`.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            items: Vec::new(),
            size: rems(0.875),
        }
    }
    pub fn child(mut self, item: BreadcrumbItem<S>) -> Self {
        self.items.push(item);
        self
    }
    /// The crumbs' text size instead of `text_sm` (Roostr's header: 14).
    pub fn text_size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let n = self.items.len();
        let mut r = row()
            .key(self.key.clone())
            .role("list")
            .center_y()
            .gap(rems(0.375))
            .min_w(0.0);
        for (ix, item) in self.items.into_iter().enumerate() {
            let last = ix + 1 == n;
            let base = if last && !item.disabled {
                t.kit.foreground
            } else {
                t.kit.muted_foreground
            };
            let mut label = text(item.label.clone())
                .font(ty::ui(self.size))
                .color(item.color.unwrap_or(base));
            if item.truncate {
                label = label.truncate().shrink();
            }
            let mut crumb = row()
                .key(format!("{}-{ix}", self.key))
                .min_w(0.0)
                .child(label);
            crumb = match item.on_click.filter(|_| !item.disabled) {
                Some(f) => crumb
                    .role("link")
                    .label(item.label)
                    .focusable(true)
                    .cursor(Cursor::Pointer)
                    .on_press(move |s: &mut S| f(s)),
                None => crumb.role("listitem"),
            };
            if item.truncate {
                crumb = crumb.shrink();
            }
            r = r.child(crumb);
            if !last {
                r = r.child(glyph(Glyph::ChevronRight, rems(0.875)).color(t.kit.muted_foreground));
            }
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime};

    struct App {
        open: bool,
        crumbs: usize,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let t = Theme::dark();
        col()
            .padding(20.0)
            .child(
                Collapsible::new("toggle")
                    .open(s.open)
                    .on_toggle(|s: &mut App| s.open = !s.open)
                    .child(text("Details").font(ty::ui(13.0)).color(t.fg))
                    .content(text("Inside").key("inside").font(ty::ui(13.0)).color(t.fg))
                    .build(),
            )
            .child(
                Breadcrumb::new("crumbs")
                    .child(BreadcrumbItem::new("Home").on_click(|s: &mut App| s.crumbs += 1))
                    .child(BreadcrumbItem::new("Page"))
                    .build(t),
            )
    }

    fn click(rt: &mut Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn collapsible_toggles_its_content() {
        let mut rt = Runtime::new(
            App {
                open: false,
                crumbs: 0,
            },
            view,
            (400.0, 300.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        assert!(rt.find("toggle-content").is_none());
        click(&mut rt, "toggle-trigger");
        assert!(rt.state.open);
        rt.frame(1.0);
        assert!(rt.find("inside").is_some());
        click(&mut rt, "toggle-trigger");
        assert!(!rt.state.open);
        rt.frame(1.0);
        assert!(rt.find("toggle-content").is_none());
    }

    #[test]
    fn a_crumb_with_a_click_presses() {
        let mut rt = Runtime::new(
            App {
                open: false,
                crumbs: 0,
            },
            view,
            (400.0, 300.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        click(&mut rt, "crumbs-0");
        click(&mut rt, "crumbs-1");
        assert_eq!(rt.state.crumbs, 1);
    }
}

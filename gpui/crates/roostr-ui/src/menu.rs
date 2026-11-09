//! Menus: Roostr's own menu surface and rows (`roostr_core::ui::{menu_panel, menu_separator,
//! menu_label, MenuItem}`), gpui-kit's `PopupMenu` / `PopupMenuItem` drawn as the kit drew them
//! in Roostr's theme, and [`DropdownMenu`], the overlay that opens either kind next to a trigger
//! (gpui-kit `DropdownMenu`) or at a point (a context menu from `El::on_secondary_press`).
//!
//! An open menu is modal (`crate::overlay::dismissable`): it holds keyboard focus, Up and Down
//! move one highlight shared with the pointer (`focus_highlight`) and wrap, typing selects by
//! label, Return presses the highlighted row, Escape or a press outside closes it, and focus
//! returns to the trigger.

use std::any::Any;
use std::rc::Rc;

use rune::{El, Key, PointerPress, Tx, col, el, row, text};

use crate::icons::{Glyph, glyph};
use crate::overlay::{self, Acts, Placement};
use crate::theme::{Theme, radius, rems, shadow, ty};

// ---------------------------------------------------------------- Roostr's own menus

/// The popover/menu surface (`roostr_core::ui::menu_panel`): panel fill, 1px border, radius 10,
/// padding 6, the menu shadow, a column of 13px rows in the text colour. Callers set `min_w`
/// (170 create menus, 220 more menus).
pub fn menu_panel(t: &Theme) -> El {
    col()
        .items_stretch()
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(radius::MENU)
        .padding(6.0)
        .box_shadow(shadow::menu())
}

/// A 1px separator between menu groups, 4px above and below and 2px in from the sides
/// (`roostr_core::ui::menu_separator`).
pub fn menu_separator(t: &Theme) -> El {
    el().h(1.0).mx(2.0).my(4.0).fill(t.border).a11y_hidden()
}

/// A muted note or empty row inside a menu: 12px, padding 6 × 8 (`roostr_core::ui::menu_label`).
pub fn menu_label(t: &Theme, label: impl Into<String>) -> El {
    row()
        .px(8.0)
        .py(6.0)
        .child(text(label.into()).font(ty::ui(12.0)).color(t.muted))
}

/// How a [`MenuItem`] lights up (`roostr_core::ui::MenuItemStyle`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuItemStyle {
    /// Create menus: hover fill `--hover`, text stays.
    #[default]
    Plain,
    /// "More" menus: hover solid accent with white text.
    Accent,
    /// Red text; hover red fill with white text.
    Danger,
}

/// One Roostr menu row (`roostr_core::ui::MenuItem`): gap 8, padding 6 × 8, radius 6, 13px, an
/// optional leading 16px glyph or any icon, the label truncating, an optional trailing element.
/// Pointer hover and the keyboard highlight paint the style's fill and text colour; `selected`
/// paints them at rest. Disabled rows show at 50% and do nothing.
pub struct MenuItem {
    key: String,
    label: String,
    lead: Option<Lead>,
    trailing: Option<El>,
    style: MenuItemStyle,
    selected: bool,
    disabled: bool,
    acts: Acts,
}

enum Lead {
    Glyph(Glyph),
    El(El),
}

impl MenuItem {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> MenuItem {
        MenuItem {
            key: key.into(),
            label: label.into(),
            lead: None,
            trailing: None,
            style: MenuItemStyle::Plain,
            selected: false,
            disabled: false,
            acts: Acts::default(),
        }
    }
    /// A 16px line glyph in the row's text colour.
    pub fn glyph(mut self, g: Glyph) -> Self {
        self.lead = Some(Lead::Glyph(g));
        self
    }
    /// Any leading element (an emoji or image icon).
    pub fn icon(mut self, icon: El) -> Self {
        self.lead = Some(Lead::El(icon));
        self
    }
    pub fn trailing(mut self, e: El) -> Self {
        self.trailing = Some(e);
        self
    }
    pub fn style(mut self, style: MenuItemStyle) -> Self {
        self.style = style;
        self
    }
    pub fn danger(self) -> Self {
        self.style(MenuItemStyle::Danger)
    }
    pub fn accent_hover(self) -> Self {
        self.style(MenuItemStyle::Accent)
    }
    /// A row the app highlights itself: drawn as hovered.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.press(f);
        self
    }
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.press_tx(f);
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let (fg, hover_bg, hover_fg) = match self.style {
            MenuItemStyle::Plain => (t.fg, t.hover, t.fg),
            MenuItemStyle::Accent => (t.fg, t.accent, t.white),
            MenuItemStyle::Danger => (t.red, t.red, t.white),
        };
        let lit = self.selected && !self.disabled;
        let ink = if lit { hover_fg } else { fg };
        let mut e = row()
            .key(self.key.clone())
            .role("menuitem")
            .label(self.label.clone())
            .center_y()
            .gap(8.0)
            .px(8.0)
            .py(6.0)
            .radius(radius::SM);
        match self.lead {
            Some(Lead::Glyph(g)) => e = e.child(glyph(g, 16.0).color(ink).group_color(hover_fg)),
            Some(Lead::El(i)) => e = e.child(i),
            None => {}
        }
        e = e.child(
            text(self.label)
                .font(ty::ui(13.0))
                .color(ink)
                .group_color(hover_fg)
                .truncate()
                .grow(1.0)
                .shrink()
                .a11y_hidden(),
        );
        if let Some(tr) = self.trailing {
            e = e.child(tr);
        }
        if self.disabled {
            return e.disabled(true).layer_opacity(0.5);
        }
        e = e
            .focusable(true)
            .no_focus_ring()
            .hover_group_focus()
            .cursor(rune::Cursor::Pointer);
        e = if lit {
            e.fill(hover_bg)
        } else {
            e.focus_highlight(hover_bg)
        };
        self.acts.apply(e, true)
    }
}

// ---------------------------------------------------------------- gpui-kit PopupMenu

/// A handler waiting for the menu's close request so a pick can close the menu as gpui-kit's
/// `PopupMenu` dismisses itself after an item's `on_click`.
type Pick = Box<dyn FnOnce(El, Option<&dyn Any>) -> El>;

fn pick<S: 'static>(f: impl Fn(&mut S, &mut Tx) + 'static) -> Pick {
    Box::new(move |e: El, close: Option<&dyn Any>| {
        let close = close
            .and_then(|c| c.downcast_ref::<Rc<dyn Fn(&mut S)>>())
            .cloned();
        e.on_press_tx(move |s: &mut S, tx: &mut Tx| {
            f(s, tx);
            if let Some(close) = &close {
                close(s);
            }
        })
    })
}

enum Body {
    Item {
        label: String,
        glyph: Option<Glyph>,
        shortcut: Option<String>,
    },
    Element(El),
    Submenu {
        label: String,
        glyph: Option<Glyph>,
        menu: Box<PopupMenu>,
        open: bool,
    },
    Separator,
    Label(String),
}

/// One entry of a [`PopupMenu`] (gpui-kit `PopupMenuItem`): an item (label, optional icon,
/// check, shortcut hint), a custom element row, a submenu, a separator or a label.
pub struct PopupMenuItem {
    body: Body,
    checked: bool,
    disabled: bool,
    picks: Vec<Pick>,
    toggle_el: Option<Box<dyn FnOnce(El, bool) -> El>>,
    sub_key: Option<Box<dyn FnOnce(El) -> El>>,
}

impl PopupMenuItem {
    fn of(body: Body) -> PopupMenuItem {
        PopupMenuItem {
            body,
            checked: false,
            disabled: false,
            picks: Vec::new(),
            toggle_el: None,
            sub_key: None,
        }
    }
    /// A plain item (`PopupMenuItem::new`).
    pub fn new(label: impl Into<String>) -> PopupMenuItem {
        Self::of(Body::Item {
            label: label.into(),
            glyph: None,
            shortcut: None,
        })
    }
    /// A row drawing `content` (`PopupMenuItem::element`), at least an item tall.
    pub fn element(content: El) -> PopupMenuItem {
        Self::of(Body::Element(content))
    }
    /// A submenu row with a chevron (`PopupMenuItem::submenu`): `open` shows `menu` beside it.
    /// Opening and closing go through [`Self::on_toggle`].
    pub fn submenu(label: impl Into<String>, menu: PopupMenu, open: bool) -> PopupMenuItem {
        Self::of(Body::Submenu {
            label: label.into(),
            glyph: None,
            menu: Box::new(menu),
            open,
        })
    }
    /// A 2px rule between groups (`PopupMenuItem::separator`).
    pub fn separator() -> PopupMenuItem {
        Self::of(Body::Separator)
    }
    /// A muted, inert row (`PopupMenuItem::label`).
    pub fn label(text: impl Into<String>) -> PopupMenuItem {
        Self::of(Body::Label(text.into()))
    }
    /// A leading icon (drawn xsmall, 0.75 rem).
    pub fn icon(mut self, g: Glyph) -> Self {
        match &mut self.body {
            Body::Item { glyph, .. } | Body::Submenu { glyph, .. } => *glyph = Some(g),
            _ => {}
        }
        self
    }
    /// A check in the leading column (gpui-kit's default left check side).
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
    /// The shortcut hint after the label, as the kit drew the action's `Kbd` in a menu.
    pub fn shortcut(mut self, keys: impl Into<String>) -> Self {
        if let Body::Item { shortcut, .. } = &mut self.body {
            *shortcut = Some(keys.into());
        }
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Runs on a pick (press or Return); the menu then closes.
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.picks.push(pick(move |s: &mut S, _: &mut Tx| f(s)));
        self
    }
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.picks.push(pick(f));
        self
    }
    /// A submenu's open requests: pressing or hovering it, or Right, asks `true`; pressing it
    /// open or Left inside it asks `false`.
    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S, bool) + 'static) -> Self {
        let f = Rc::new(f);
        let (press, hover, key) = (f.clone(), f.clone(), f.clone());
        self.toggle_el = Some(Box::new(move |e: El, open: bool| {
            e.on_press(move |s: &mut S| press(s, !open))
                .on_hover_change(move |s: &mut S, inside: bool, _: &mut Tx| {
                    if inside && !open {
                        hover(s, true)
                    }
                })
                .on_key(move |s: &mut S, k: &rune::KeyEvent, _: &mut Tx| {
                    if k.key == Key::Right {
                        key(s, true);
                        true
                    } else {
                        false
                    }
                })
        }));
        self.sub_key = Some(Box::new(move |e: El| {
            e.on_key(move |s: &mut S, k: &rune::KeyEvent, _: &mut Tx| {
                if k.key == Key::Left {
                    f(s, false);
                    true
                } else {
                    false
                }
            })
        }));
        self
    }

    fn left_icon(&self) -> bool {
        match &self.body {
            Body::Item { glyph, .. } => glyph.is_some() || self.checked,
            Body::Element(_) => self.checked,
            Body::Submenu { glyph, .. } => glyph.is_some(),
            _ => false,
        }
    }

    fn build(
        self,
        t: &Theme,
        key: String,
        left: bool,
        close: Option<&dyn Any>,
        layer: &mut Vec<El>,
    ) -> El {
        let item_h = rems(1.625);
        let icon_size = rems(0.75);
        let lead = |g: Option<Glyph>, checked: bool, ink| -> Option<El> {
            if !left {
                return None;
            }
            Some(match (g, checked) {
                (Some(g), _) => glyph(g, icon_size)
                    .color(ink)
                    .group_color(t.kit.accent_foreground),
                (None, true) => glyph(Glyph::Check, icon_size)
                    .color(ink)
                    .group_color(t.kit.accent_foreground),
                (None, false) => el().size(icon_size, icon_size),
            })
        };
        let ink = if self.disabled {
            t.kit.muted_foreground
        } else {
            t.kit.foreground
        };
        let font = ty::ui(rems(0.875));
        let base = row()
            .key(key.clone())
            .px(rems(0.5))
            .gap(rems(0.25))
            .radius(radius::KIT.min(8.0))
            .center_y();
        match self.body {
            Body::Separator => el()
                .key(key)
                .h(2.0)
                .my(rems(0.125))
                .mx(-rems(0.25))
                .fill(t.kit.border)
                .a11y_hidden(),
            Body::Label(l) => base
                .children(lead(None, false, ink))
                .child(text(l).font(font).color(t.kit.muted_foreground).grow(1.0)),
            Body::Item {
                label,
                glyph: g,
                shortcut,
            } => {
                let mut inner = row()
                    .grow(1.0)
                    .shrink()
                    .gap(rems(0.75))
                    .center_y()
                    .justify_between()
                    .child(
                        text(label.clone())
                            .font(font)
                            .color(ink)
                            .group_color(t.kit.accent_foreground)
                            .a11y_hidden(),
                    );
                if let Some(s) = &shortcut {
                    inner = inner.child(
                        text(s.clone())
                            .font(ty::ui(rems(0.75)).lh(rems(0.75)))
                            .color(t.kit.muted_foreground)
                            .a11y_hidden(),
                    );
                }
                let mut e = base
                    .role(if self.checked {
                        "menuitemcheckbox"
                    } else {
                        "menuitem"
                    })
                    .label(label)
                    .h(item_h)
                    .children(lead(g, self.checked, ink))
                    .child(inner);
                if self.checked {
                    e = e.checked(true);
                }
                if let Some(s) = shortcut {
                    e = e.shortcut_hint(s);
                }
                finish(t, e, self.disabled, self.picks, close)
            }
            Body::Element(content) => {
                let e = base.role("menuitem").child(
                    row()
                        .grow(1.0)
                        .shrink()
                        .min_h(item_h)
                        .center_y()
                        .gap(rems(0.25))
                        .children(lead(None, self.checked, ink))
                        .child(content.grow(1.0).shrink()),
                );
                finish(t, e, self.disabled, self.picks, close)
            }
            Body::Submenu {
                label,
                glyph: g,
                menu,
                open,
            } => {
                let mut e = base
                    .role("menuitem")
                    .label(label.clone())
                    .expanded(open)
                    .min_h(item_h)
                    .children(lead(g, false, ink))
                    .child(
                        row()
                            .grow(1.0)
                            .shrink()
                            .gap(rems(0.5))
                            .center_y()
                            .justify_between()
                            .child(
                                text(label)
                                    .font(font)
                                    .color(ink)
                                    .group_color(t.kit.accent_foreground)
                                    .a11y_hidden(),
                            )
                            .child(
                                glyph(Glyph::ChevronRight, icon_size).color(t.kit.muted_foreground),
                            ),
                    );
                if open && !self.disabled {
                    // Lit while its flyout shows, as the kit keeps a submenu's row selected.
                    e = e.fill(t.kit.accent);
                    let mut deeper = Vec::new();
                    let mut surface = menu.surface(t, &format!("{key}-sub"), close, &mut deeper);
                    if let Some(k) = self.sub_key {
                        surface = k(surface);
                    }
                    layer.push(overlay::anchored_still(
                        surface,
                        &key,
                        Placement::RightStart,
                        -rems(0.5),
                    ));
                    layer.extend(deeper);
                }
                let e = finish(t, e, self.disabled, Vec::new(), None);
                match (self.toggle_el, self.disabled) {
                    (Some(f), false) => f(e, open),
                    _ => e,
                }
            }
        }
    }
}

/// The kit menu row's common end: disabled rows are inert and muted; live ones share the
/// accent highlight with the pointer and run their picks.
fn finish(t: &Theme, e: El, disabled: bool, picks: Vec<Pick>, close: Option<&dyn Any>) -> El {
    if disabled {
        return e.disabled(true);
    }
    let mut e = e
        .focusable(true)
        .no_focus_ring()
        .hover_group_focus()
        .focus_highlight(t.kit.accent)
        .cursor(rune::Cursor::Pointer);
    for p in picks {
        e = p(e, close);
    }
    e
}

/// gpui-kit's `PopupMenu` as Roostr's screens fill it: the popover surface (popover fill, no
/// border, radius 6, the kit popup ring and shadow), padding 0.25 rem, rows 1.625 rem tall with
/// 0.125 rem between, at least 8 rem wide and at most 31.25 rem unless set. Open it with
/// [`DropdownMenu`]; [`PopupMenu::panel`] draws it in place.
pub struct PopupMenu {
    items: Vec<PopupMenuItem>,
    min_w: Option<f32>,
    max_w: Option<f32>,
    max_h: Option<f32>,
}

impl Default for PopupMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl PopupMenu {
    pub fn new() -> PopupMenu {
        PopupMenu {
            items: Vec::new(),
            min_w: None,
            max_w: None,
            max_h: None,
        }
    }
    pub fn item(mut self, item: PopupMenuItem) -> Self {
        self.items.push(item);
        self
    }
    pub fn separator(self) -> Self {
        self.item(PopupMenuItem::separator())
    }
    pub fn label(self, text: impl Into<String>) -> Self {
        self.item(PopupMenuItem::label(text))
    }
    pub fn min_w(mut self, w: f32) -> Self {
        self.min_w = Some(w);
        self
    }
    pub fn max_w(mut self, w: f32) -> Self {
        self.max_w = Some(w);
        self
    }
    /// A scrollable menu at most `h` tall (`.max_h(..).scrollable(true)`).
    pub fn max_h(mut self, h: f32) -> Self {
        self.max_h = Some(h);
        self
    }

    /// The menu drawn in place under `key` (open submenus go into `layer`), for galleries and
    /// screens that embed it.
    pub fn panel(self, t: &Theme, key: &str, layer: &mut Vec<El>) -> El {
        self.surface(t, key, None, layer)
    }

    fn surface(self, t: &Theme, key: &str, close: Option<&dyn Any>, layer: &mut Vec<El>) -> El {
        let left = self.items.iter().any(PopupMenuItem::left_icon);
        let n = self.items.len();
        let mut rows = col()
            .key("items")
            .items_stretch()
            .padding(rems(0.25))
            .gap(rems(0.125))
            .min_w(self.min_w.unwrap_or(rems(8.0)))
            .max_w(self.max_w.unwrap_or(rems(31.25)));
        for (i, item) in self.items.into_iter().enumerate() {
            // The kit drops a trailing separator.
            if i + 1 == n && matches!(item.body, Body::Separator) {
                continue;
            }
            rows = rows.child(item.build(t, format!("{key}-item{i}"), left, close, layer));
        }
        let mut surface = col()
            .key(format!("{key}-menu"))
            .role("menu")
            .focus_group()
            .items_stretch()
            .fill(t.kit.popover)
            .radius(radius::KIT);
        for s in shadow::kit_popover(t) {
            surface = surface.box_shadow(s);
        }
        match self.max_h {
            Some(h) => surface.child(
                col()
                    .key("scroll")
                    .max_h(h)
                    .min_h(0.0)
                    .shrink()
                    .scroll(rune::ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(rows),
            ),
            None => surface.child(rows),
        }
    }
}

// ---------------------------------------------------------------- the overlay

enum Content {
    None,
    Popup(PopupMenu),
    Panel(El),
}

/// A menu opened from a trigger or at a point (gpui-kit `DropdownMenu` / a context menu):
/// `DropdownMenu::new("more").trigger(button).menu(popup).open(s.more).on_close(|s: &mut S| s.more = false).build(t, &mut layer)`.
///
/// Whether it is open is the app's. `on_close` runs on Escape, a press outside and, for a
/// [`PopupMenu`], after every pick. A [`Self::panel`] body (a [`menu_panel`] of [`MenuItem`]s)
/// gets the same keyboard and dismissal; its rows close it themselves.
pub struct DropdownMenu {
    key: String,
    trigger: Option<El>,
    content: Content,
    open: bool,
    at: Option<(f32, f32)>,
    placement: Placement,
    gap: f32,
    close: Option<Box<dyn Any>>,
    dismiss: Option<Box<dyn FnOnce(El, &str, &mut Vec<El>) -> El>>,
}

impl DropdownMenu {
    pub fn new(key: impl Into<String>) -> DropdownMenu {
        DropdownMenu {
            key: key.into(),
            trigger: None,
            content: Content::None,
            open: false,
            at: None,
            placement: Placement::BottomStart,
            // The kit Popover's offset (0.25 rem) plus its content's `top_1`.
            gap: rems(0.5),
            close: None,
            dismiss: None,
        }
    }
    /// A context menu over `area`: open while `point` (the secondary press stored from
    /// `El::on_secondary_press`) is set, with its top-left corner at that point.
    pub fn context(key: impl Into<String>, area: El, point: Option<PointerPress>) -> DropdownMenu {
        let mut m = DropdownMenu::new(key).trigger(area).open(point.is_some());
        m.at = point.map(|p| (p.x, p.y));
        m.gap = 0.0;
        m
    }
    pub fn trigger(mut self, t: El) -> Self {
        self.trigger = Some(t);
        self
    }
    pub fn menu(mut self, m: PopupMenu) -> Self {
        self.content = Content::Popup(m);
        self
    }
    /// Any menu surface, such as a [`menu_panel`] of [`MenuItem`]s.
    pub fn panel(mut self, e: El) -> Self {
        self.content = Content::Panel(e);
        self
    }
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
    /// Where it opens beside the trigger (below, start-aligned by default).
    pub fn placement(mut self, p: Placement) -> Self {
        self.placement = p;
        self
    }
    pub fn gap(mut self, g: f32) -> Self {
        self.gap = g;
        self
    }
    /// Open the menu with its top-left corner at a window point.
    pub fn at(mut self, x: f32, y: f32) -> Self {
        self.at = Some((x, y));
        self.gap = 0.0;
        self
    }
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        let f: Rc<dyn Fn(&mut S)> = Rc::new(f);
        self.close = Some(Box::new(f.clone()));
        self.dismiss = Some(Box::new(
            move |surface: El, key: &str, layer: &mut Vec<El>| {
                overlay::dismissable(surface, key, move |s: &mut S| f(s), layer)
            },
        ));
        self
    }

    pub fn build(self, t: &Theme, layer: &mut Vec<El>) -> El {
        let anchor = format!("{}-trigger", self.key);
        let pointer = self.at.is_some();
        let trigger = row().key(anchor.clone()).children(self.trigger.map(|tr| {
            if pointer || self.at.is_some() {
                tr
            } else {
                tr.expanded(self.open)
            }
        }));
        if !self.open {
            return trigger;
        }
        // Submenus anchor to rows inside the surface, so they mount after it.
        let mut subs = Vec::new();
        let close = self.close.as_deref();
        let surface = match self.content {
            Content::Popup(m) => m.surface(t, &self.key, close, &mut subs),
            Content::Panel(p) => p
                .key(format!("{}-menu", self.key))
                .role("menu")
                .focus_group(),
            Content::None => col().key(format!("{}-menu", self.key)).role("menu"),
        };
        let mut surface = surface.focus_return(anchor.clone());
        if let Some(d) = self.dismiss {
            surface = d(surface, &self.key, layer);
        }
        let placed = match self.at {
            Some((x, y)) => {
                let point = format!("{}-point", self.key);
                layer.push(el().key(point.clone()).absolute(x, y).size(0.0, 0.0));
                overlay::anchored_still(surface, &point, Placement::BottomStart, 0.0)
            }
            None => overlay::anchored(surface, &anchor, self.placement, self.gap),
        };
        layer.push(placed);
        layer.extend(subs);
        trigger
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, KeyEvent, Mods, Runtime, col};

    #[derive(Default)]
    struct S {
        open: bool,
        point: Option<PointerPress>,
        picked: Vec<&'static str>,
        sub: bool,
    }

    fn popup() -> PopupMenu {
        PopupMenu::new()
            .item(PopupMenuItem::new("Open").on_press(|s: &mut S| s.picked.push("open")))
            .item(
                PopupMenuItem::new("Copy ID")
                    .shortcut("⌘C")
                    .on_press(|s: &mut S| s.picked.push("copy")),
            )
            .separator()
            .item(
                PopupMenuItem::new("Locked")
                    .disabled(true)
                    .on_press(|s: &mut S| s.picked.push("locked")),
            )
            .item(
                PopupMenuItem::new("Pin")
                    .checked(true)
                    .on_press(|s: &mut S| s.picked.push("pin")),
            )
    }

    fn view(s: &S, _: &mut rune::Cx<S>) -> El {
        let t = Theme::dark();
        let mut layer = Vec::new();
        let button = row()
            .key("btn")
            .w(80.0)
            .h(28.0)
            .focusable(true)
            .on_press(|s: &mut S| s.open = !s.open);
        let menu = DropdownMenu::new("m")
            .trigger(button)
            .menu(popup())
            .open(s.open)
            .on_close(|s: &mut S| s.open = false)
            .build(t, &mut layer);
        let area = el()
            .key("area")
            .w(300.0)
            .h(200.0)
            .on_secondary_press(|s: &mut S, p: PointerPress, _: &mut Tx| s.point = Some(p));
        let sub = PopupMenu::new()
            .item(PopupMenuItem::new("Note").on_press(|s: &mut S| s.picked.push("note")));
        let ctx = DropdownMenu::context("ctx", area, s.point)
            .menu(
                PopupMenu::new()
                    .item(
                        PopupMenuItem::new("Rename").on_press(|s: &mut S| s.picked.push("rename")),
                    )
                    .item(
                        PopupMenuItem::submenu("Change type", sub, s.sub)
                            .on_toggle(|s: &mut S, o| s.sub = o),
                    ),
            )
            .on_close(|s: &mut S| {
                s.point = None;
                s.sub = false;
            })
            .build(t, &mut layer);
        crate::root(
            col()
                .w_full()
                .h_full()
                .padding(20.0)
                .gap(20.0)
                .child(menu)
                .child(ctx),
            layer,
        )
    }

    fn rt() -> Runtime<S> {
        let mut rt = Runtime::new(
            S::default(),
            view,
            (800.0, 600.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        rt
    }

    fn press(rt: &mut Runtime<S>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn key(rt: &mut Runtime<S>, key: Key) {
        rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
    }

    fn focused(rt: &Runtime<S>) -> Option<String> {
        rt.elements()
            .into_iter()
            .find(|e| e.focused)
            .and_then(|e| e.key)
    }

    #[test]
    fn keyboard_moves_wraps_presses_and_escapes() {
        let mut rt = rt();
        press(&mut rt, "btn");
        assert!(rt.state.open);
        key(&mut rt, Key::Down);
        let first = focused(&rt);
        key(&mut rt, Key::Down);
        let second = focused(&rt);
        assert_ne!(first, second);
        key(&mut rt, Key::Up);
        assert_eq!(focused(&rt), first);
        // Up from the first row wraps to the last live one (the disabled row is skipped).
        let mut seen = Vec::new();
        for _ in 0..4 {
            key(&mut rt, Key::Down);
            seen.push(focused(&rt));
        }
        assert!(seen.contains(&first), "Down wraps: {seen:?}");
        assert!(
            !seen.contains(&Some("m-item3".into())),
            "disabled row skipped: {seen:?}"
        );
        // Back to "Copy ID" and press it.
        while focused(&rt).as_deref() != Some("m-item1") {
            key(&mut rt, Key::Down);
        }
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.picked, vec!["copy"]);
        assert!(!rt.state.open, "a pick closes the menu");

        press(&mut rt, "btn");
        assert!(rt.state.open);
        key(&mut rt, Key::Down);
        key(&mut rt, Key::Escape);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.open);
        assert_eq!(
            focused(&rt).as_deref(),
            Some("btn"),
            "focus returns to the trigger"
        );
    }

    #[test]
    fn disabled_item_does_not_fire() {
        let mut rt = rt();
        press(&mut rt, "btn");
        press(&mut rt, "m-item3");
        assert!(rt.state.picked.is_empty());
    }

    #[test]
    fn context_menu_opens_at_the_press_point() {
        let mut rt = rt();
        let (ax, ay) = rt.find("area").unwrap();
        let (x, y) = (ax - 40.0, ay + 10.0);
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0);
        assert_eq!(rt.state.point.map(|p| (p.x, p.y)), Some((x, y)));
        let menu = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("ctx-menu"))
            .expect("open");
        assert!(
            (menu.rect[0] - x).abs() < 1.0 && (menu.rect[1] - y).abs() < 1.0,
            "{:?} vs {x},{y}",
            menu.rect
        );
        // Right opens the submenu, its row is pickable, and the pick closes everything.
        while focused(&rt).as_deref() != Some("ctx-item1") {
            key(&mut rt, Key::Down);
        }
        key(&mut rt, Key::Right);
        assert!(rt.state.sub);
        press(&mut rt, "ctx-item1-sub-item0");
        assert_eq!(rt.state.picked, vec!["note"]);
        assert!(rt.state.point.is_none());
    }
}

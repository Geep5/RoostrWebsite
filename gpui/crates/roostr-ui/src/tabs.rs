//! gpui-kit's `TabBar` and `Tab` (`tab/tab_bar.rs`, `tab/tab.rs`): the default tab strip,
//! outline, pill, segmented and underline variants in every size, with prefix / suffix slots,
//! disabled tabs, a label cap (`max_width`), the clicked tab scrolled into view, and the sliding
//! indicator the pill, segmented and underline bars move between tabs.

use std::rc::Rc;

use rune::{Color, Cursor, El, Shadow, Spring, Tx, el, row, text};

use crate::KitSize;
use crate::icons::{Glyph, glyph};
use crate::theme::{Theme, radius, rems, shadow, ty};

/// gpui-kit's `spring_move` (280 ms, damping 0.85): the indicator's slide.
const SPRING_MOVE: Spring = Spring::new(0.28, 0.15);

/// gpui-kit's `TabVariant` (its `Folder` look is not used by Roostr and not ported).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabVariant {
    #[default]
    Tab,
    Outline,
    Pill,
    Segmented,
    Underline,
}

/// One state's look (gpui-kit's `TabStyle`): borders are `[top, right, bottom, left]`.
#[derive(Clone, Copy)]
struct TabStyle {
    fg: Color,
    bg: Color,
    borders: [f32; 4],
    border: Color,
    inner_bg: Color,
    shadow: bool,
}

impl TabStyle {
    fn new(fg: Color) -> Self {
        Self {
            fg,
            bg: Color::TRANSPARENT,
            borders: [0.0; 4],
            border: Color::TRANSPARENT,
            inner_bg: Color::TRANSPARENT,
            shadow: false,
        }
    }
}

fn size_ix(size: KitSize) -> usize {
    match size {
        KitSize::XSmall => 0,
        KitSize::Small => 1,
        KitSize::Large => 3,
        _ => 2,
    }
}

impl TabVariant {
    fn height(self, size: KitSize) -> f32 {
        let under = self == TabVariant::Underline;
        [
            if under { 26.0 } else { 20.0 },
            if under { 30.0 } else { 24.0 },
            if under { 36.0 } else { 32.0 },
            if under { 44.0 } else { 36.0 },
        ][size_ix(size)]
    }

    fn inner_height(self, size: KitSize) -> f32 {
        use TabVariant::*;
        match (size_ix(size), self) {
            (0, Tab | Outline | Pill) => 18.0,
            (0, Segmented) => 16.0,
            (0, Underline) => 20.0,
            (1, Tab | Outline | Pill) => 22.0,
            (1, Segmented) => 18.0,
            (1, Underline) => 22.0,
            (3, Tab | Outline | Pill) => 36.0,
            (3, Segmented) => 28.0,
            (3, Underline) => 32.0,
            (_, Tab) => 30.0,
            (_, Outline | Pill) => 26.0,
            (_, Segmented) => 24.0,
            (_, Underline) => 26.0,
        }
    }

    fn padding_x(self, size: KitSize) -> f32 {
        if self == TabVariant::Underline {
            0.0
        } else {
            [8.0, 10.0, 12.0, 16.0][size_ix(size)]
        }
    }

    /// The inner box's top and bottom margins.
    fn inner_margins(self, size: KitSize) -> (f32, f32) {
        if self == TabVariant::Underline {
            [(1.0, 2.0), (2.0, 3.0), (3.0, 4.0), (5.0, 6.0)][size_ix(size)]
        } else {
            (0.0, 0.0)
        }
    }

    fn normal(self, t: &Theme) -> TabStyle {
        let k = &t.kit;
        match self {
            TabVariant::Tab => TabStyle {
                borders: [0.0, 1.0, 0.0, 1.0],
                ..TabStyle::new(k.tab_foreground)
            },
            TabVariant::Outline => TabStyle {
                borders: [1.0; 4],
                border: k.border,
                ..TabStyle::new(k.tab_foreground)
            },
            TabVariant::Pill => TabStyle::new(k.foreground),
            TabVariant::Segmented => TabStyle::new(k.tab_foreground),
            TabVariant::Underline => TabStyle {
                borders: [0.0, 0.0, 2.0, 0.0],
                ..TabStyle::new(k.tab_foreground)
            },
        }
    }

    fn hovered(self, t: &Theme) -> TabStyle {
        let k = &t.kit;
        match self {
            TabVariant::Tab => TabStyle {
                borders: [0.0, 1.0, 0.0, 1.0],
                ..TabStyle::new(k.tab_active_foreground)
            },
            TabVariant::Outline => TabStyle {
                bg: k.secondary_hover,
                borders: [1.0; 4],
                border: k.border,
                ..TabStyle::new(k.secondary_foreground)
            },
            TabVariant::Pill => TabStyle {
                bg: k.secondary,
                ..TabStyle::new(k.secondary_foreground)
            },
            TabVariant::Segmented => TabStyle::new(k.tab_active_foreground),
            TabVariant::Underline => TabStyle {
                borders: [0.0, 0.0, 2.0, 0.0],
                ..TabStyle::new(k.tab_active_foreground)
            },
        }
    }

    fn selected(self, t: &Theme) -> TabStyle {
        let k = &t.kit;
        match self {
            TabVariant::Tab => TabStyle {
                bg: k.tab_active,
                borders: [0.0, 1.0, 0.0, 1.0],
                border: k.border,
                ..TabStyle::new(k.tab_active_foreground)
            },
            TabVariant::Outline => TabStyle {
                borders: [1.0; 4],
                border: k.primary,
                ..TabStyle::new(k.primary)
            },
            TabVariant::Pill => TabStyle {
                bg: k.primary,
                ..TabStyle::new(k.primary_foreground)
            },
            TabVariant::Segmented => TabStyle {
                inner_bg: k.background,
                shadow: true,
                ..TabStyle::new(k.tab_active_foreground)
            },
            TabVariant::Underline => TabStyle {
                borders: [0.0, 0.0, 2.0, 0.0],
                border: k.primary,
                ..TabStyle::new(k.tab_active_foreground)
            },
        }
    }

    fn disabled(self, selected: bool, t: &Theme) -> TabStyle {
        let k = &t.kit;
        let muted = TabStyle::new(k.muted_foreground);
        match self {
            TabVariant::Tab => TabStyle {
                borders: [0.0, 1.0, 0.0, 1.0],
                border: if selected {
                    k.border
                } else {
                    Color::TRANSPARENT
                },
                ..muted
            },
            TabVariant::Outline => TabStyle {
                borders: [1.0; 4],
                border: if selected { k.primary } else { k.border },
                ..muted
            },
            TabVariant::Pill if selected => TabStyle {
                bg: k.primary.alpha(k.primary.a * 0.5),
                ..TabStyle::new(k.primary_foreground.alpha(k.primary_foreground.a * 0.5))
            },
            TabVariant::Pill => muted,
            TabVariant::Segmented => TabStyle {
                bg: k.tab_bar,
                inner_bg: if selected {
                    k.background
                } else {
                    Color::TRANSPARENT
                },
                ..muted
            },
            TabVariant::Underline => TabStyle {
                borders: [0.0, 0.0, 2.0, 0.0],
                border: if selected {
                    k.border
                } else {
                    Color::TRANSPARENT
                },
                ..muted
            },
        }
    }

    fn bar_radius(self, size: KitSize) -> f32 {
        match (self, size_ix(size)) {
            (TabVariant::Segmented, 0 | 1) => radius::KIT,
            (TabVariant::Segmented, _) => radius::KIT_LG,
            _ => 0.0,
        }
    }

    fn radius(self, size: KitSize) -> f32 {
        match self {
            TabVariant::Outline | TabVariant::Pill => 9999.0,
            TabVariant::Segmented => self.bar_radius(size),
            _ => 0.0,
        }
    }

    fn inner_radius(self, size: KitSize) -> f32 {
        match (self, size_ix(size)) {
            (TabVariant::Segmented, 3) => (self.bar_radius(size) - 3.0).max(0.0),
            (TabVariant::Segmented, _) => (self.bar_radius(size) - 2.0).max(0.0),
            _ => 0.0,
        }
    }

    fn has_indicator(self) -> bool {
        matches!(
            self,
            TabVariant::Segmented | TabVariant::Pill | TabVariant::Underline
        )
    }
}

/// gpui-kit's `raised_shadow` (the segmented indicator), black at 10%.
fn raised_shadow() -> [Shadow; 2] {
    [
        shadow::gpui(1.0, 1.5, 0.0, 0.1),
        shadow::gpui(1.0, 1.0, -1.0, 0.1),
    ]
}

/// One tab of a [`TabBar`] (gpui-kit `Tab`): a label or an icon, optional children after the
/// label, prefix and suffix slots.
pub struct Tab<S> {
    label: Option<String>,
    icon: Option<Glyph>,
    children: Vec<El>,
    prefix: Option<El>,
    suffix: Option<El>,
    disabled: bool,
    grow: bool,
    on_click: Option<Rc<dyn Fn(&mut S)>>,
    /// The tab's own key in place of `{bar}-tab{ix}`, so it keeps its identity (and slides)
    /// when the tabs reorder.
    key: Option<String>,
    /// Applied to the tab's finished element (a drag's offset, a lift).
    wrap: Option<Box<dyn FnOnce(El) -> El>>,
}

impl<S: 'static> Default for Tab<S> {
    fn default() -> Self {
        Self {
            label: None,
            icon: None,
            children: Vec::new(),
            prefix: None,
            suffix: None,
            disabled: false,
            grow: false,
            on_click: None,
            key: None,
            wrap: None,
        }
    }
}

impl<S: 'static> Tab<S> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// An icon-only tab (fixed width, exempt from the bar's `max_width`).
    pub fn icon(mut self, g: Glyph) -> Self {
        self.icon = Some(g);
        self
    }
    /// Content after the label, inside the tab's padded box.
    pub fn child(mut self, c: El) -> Self {
        self.children.push(c);
        self
    }
    pub fn prefix(mut self, e: El) -> Self {
        self.prefix = Some(e);
        self
    }
    pub fn suffix(mut self, e: El) -> Self {
        self.suffix = Some(e);
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    /// `flex_1()`: the tab takes an equal share of the bar's width.
    pub fn flex_1(mut self) -> Self {
        self.grow = true;
        self
    }
    /// Key the tab `key` instead of by its place, so a reorder moves it rather than its content.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }
    /// Change the tab's finished element (`f` gets it last): an offset, a lift, handlers.
    pub fn wrap(mut self, f: impl FnOnce(El) -> El + 'static) -> Self {
        self.wrap = Some(Box::new(f));
        self
    }
    /// The tab's own click, used when the bar has no `on_click`.
    pub fn on_click(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

/// gpui-kit's `TabBar`: tabs in a row. A [`TabBar::scrollable`] bar scrolls sideways when the
/// tabs overflow its width (gpui-kit always does; in Rune a scroll viewport has no width of its
/// own, so a bar sized by its tabs leaves it off).
pub struct TabBar<S> {
    key: String,
    variant: TabVariant,
    size: KitSize,
    selected: Option<usize>,
    max_width: Option<f32>,
    tabs: Vec<Tab<S>>,
    prefix: Option<El>,
    suffix: Option<El>,
    on_click: Option<Rc<dyn Fn(&mut S, usize)>>,
    fill: Option<Color>,
    border: Option<(f32, Color)>,
    radius: Option<f32>,
    padding: Option<f32>,
    px: Option<f32>,
    h: Option<f32>,
    scroll: bool,
}

impl<S: 'static> TabBar<S> {
    /// Tabs are keyed `{key}-tab{ix}`.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            variant: TabVariant::Tab,
            size: KitSize::Medium,
            selected: None,
            max_width: None,
            tabs: Vec::new(),
            prefix: None,
            suffix: None,
            on_click: None,
            fill: None,
            border: None,
            radius: None,
            padding: None,
            px: None,
            h: None,
            scroll: false,
        }
    }
    pub fn variant(mut self, v: TabVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn outline(self) -> Self {
        self.variant(TabVariant::Outline)
    }
    pub fn pill(self) -> Self {
        self.variant(TabVariant::Pill)
    }
    pub fn segmented(self) -> Self {
        self.variant(TabVariant::Segmented)
    }
    pub fn underline(self) -> Self {
        self.variant(TabVariant::Underline)
    }
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = s;
        self
    }
    pub fn xsmall(self) -> Self {
        self.size(KitSize::XSmall)
    }
    pub fn small(self) -> Self {
        self.size(KitSize::Small)
    }
    pub fn large(self) -> Self {
        self.size(KitSize::Large)
    }
    pub fn selected_index(mut self, ix: usize) -> Self {
        self.selected = Some(ix);
        self
    }
    /// Caps each labelled tab's width; its label ellipsises.
    pub fn max_width(mut self, w: f32) -> Self {
        self.max_width = Some(w);
        self
    }
    pub fn child(mut self, tab: Tab<S>) -> Self {
        self.tabs.push(tab);
        self
    }
    pub fn children(mut self, tabs: impl IntoIterator<Item = Tab<S>>) -> Self {
        self.tabs.extend(tabs);
        self
    }
    pub fn prefix(mut self, e: El) -> Self {
        self.prefix = Some(e);
        self
    }
    pub fn suffix(mut self, e: El) -> Self {
        self.suffix = Some(e);
        self
    }
    /// Runs with the clicked tab's index; replaces the tabs' own `on_click`.
    pub fn on_click(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
    /// The bar's fill instead of the variant's (`.bg(..)`).
    pub fn fill(mut self, c: Color) -> Self {
        self.fill = Some(c);
        self
    }
    pub fn border(mut self, w: f32, c: Color) -> Self {
        self.border = Some((w, c));
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = Some(r);
        self
    }
    /// Padding all round instead of the variant's.
    pub fn padding(mut self, p: f32) -> Self {
        self.padding = Some(p);
        self
    }
    pub fn px(mut self, p: f32) -> Self {
        self.px = Some(p);
        self
    }
    pub fn h(mut self, h: f32) -> Self {
        self.h = Some(h);
        self
    }
    /// The tabs scroll sideways inside the bar's width (a tab strip); give the bar a width.
    pub fn scrollable(mut self) -> Self {
        self.scroll = true;
        self
    }

    pub fn build(mut self, t: &Theme) -> El {
        let v = self.variant;
        let size = self.size;
        let k = &self.key;
        let default_gap = match size {
            KitSize::XSmall | KitSize::Small => 8.0,
            KitSize::Large => 16.0,
            _ => 12.0,
        };
        let (bg, pad_x, gap) = match v {
            TabVariant::Tab => (t.kit.tab_bar, 0.0, 0.0),
            TabVariant::Outline => (Color::TRANSPARENT, 0.0, default_gap),
            TabVariant::Pill => (Color::TRANSPARENT, 0.0, 4.0),
            TabVariant::Segmented => (
                t.kit.tab_bar_segmented,
                [2.0, 3.0, 4.0, 4.0][size_ix(size)],
                2.0,
            ),
            TabVariant::Underline => (
                Color::TRANSPARENT,
                0.0,
                [10.0, 12.0, 16.0, 20.0][size_ix(size)],
            ),
        };
        let row_key = format!("{k}-tabs");
        let sel = self.selected.filter(|&s| s < self.tabs.len());
        // The indicator is placed from where the last layout put the selected tab; until a
        // layout exists the selected tab paints its own active look, as gpui-kit does.
        let selected_key = sel.map(|s| {
            self.tabs[s]
                .key
                .clone()
                .unwrap_or_else(|| format!("{k}-tab{s}"))
        });
        let placed = sel.filter(|_| v.has_indicator()).and_then(|_| {
            let tabs = rune::memo::layout(&row_key)?;
            let tab = rune::memo::layout(&selected_key?)?;
            Some((tab[0] - tabs[0], tab[2]))
        });
        let mut tabs = row().key(row_key).center_y().gap(gap);
        if let Some((left, width)) = placed {
            let h = v.height(size);
            let mut ind = el()
                .key(format!("{k}-indicator"))
                .absolute(left, 0.0)
                .w(width)
                .h(h)
                .motion(SPRING_MOVE);
            ind = match v {
                TabVariant::Segmented => {
                    let ir = v.inner_radius(size);
                    let mut inner = el()
                        .w_full()
                        .h(v.inner_height(size))
                        .fill(t.kit.background)
                        .radius(ir);
                    for s in raised_shadow() {
                        inner = inner.box_shadow(s);
                    }
                    ind.center_y().child(inner)
                }
                TabVariant::Pill => ind.fill(t.kit.primary).radius(9999.0),
                _ => ind.child(
                    el().absolute(0.0, h - 2.0)
                        .w_full()
                        .h(2.0)
                        .fill(t.kit.primary),
                ),
            };
            tabs = tabs.child(ind);
        }
        let on_click = self.on_click.clone();
        for (ix, tab) in std::mem::take(&mut self.tabs).into_iter().enumerate() {
            let selected = sel == Some(ix);
            let handler: Option<Rc<dyn Fn(&mut S)>> = match &on_click {
                Some(f) => {
                    let f = f.clone();
                    Some(Rc::new(move |s: &mut S| f(s, ix)))
                }
                None => tab.on_click.clone(),
            };
            tabs = tabs.child(self.tab(t, ix, tab, selected, placed.is_some(), handler));
        }
        let pad = self.padding;
        let mut bar = row()
            .key(k.clone())
            .role("tablist")
            .focus_group()
            .center_y()
            .fill(self.fill.unwrap_or(bg))
            .radius(self.radius.unwrap_or(v.bar_radius(size)));
        bar = match pad {
            Some(p) => bar.padding(p),
            None => bar.px(self.px.unwrap_or(pad_x)),
        };
        if let Some(h) = self.h {
            bar = bar.h(h);
        }
        if let Some((w, c)) = self.border {
            bar = bar.border(w, c).border_layout();
        }
        if matches!(v, TabVariant::Underline | TabVariant::Tab) {
            bar = bar.child(
                el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .border_sides([0.0, 0.0, 1.0, 0.0], t.kit.border),
            );
        }
        if let Some(p) = self.prefix {
            bar = bar.child(p);
        }
        bar = if self.scroll {
            let scroll_key = format!("{k}-scroll");
            // Where the tabs are scrolled to and how far they can go, kept current by the
            // viewport (scrolling draws without re-running the view).
            let pos = Rc::new(std::cell::Cell::new((0.0_f32, 0.0_f32)));
            let (seen, wheel_pos) = (pos.clone(), pos);
            let wheel_key = scroll_key.clone();
            bar.child(
                row()
                    .key(scroll_key)
                    .grow(1.0)
                    .shrink()
                    .scroll_x()
                    .on_scroll_offset(move |at: rune::ScrollPos| {
                        seen.set((at.offset[0], at.max[0]))
                    })
                    // A mouse wheel turns up and down, which no sideways viewport hears: over
                    // the tabs it scrolls them, as a browser's tab strip does. Sideways
                    // trackpad swipes keep the viewport's own scrolling and momentum.
                    .on_wheel(move |_: &mut S, e: rune::WheelEvent, tx: &mut Tx| {
                        let (at, max) = wheel_pos.get();
                        if max <= 0.0 || e.dy.abs() <= e.dx.abs() {
                            return false;
                        }
                        let to = (at - e.dy).clamp(0.0, max);
                        // Several ticks can land before the next frame reports the move.
                        wheel_pos.set((to, max));
                        tx.scroll_to_x(wheel_key.clone(), to);
                        true
                    })
                    .child(tabs),
            )
        } else {
            bar.child(tabs.grow(1.0))
        };
        if let Some(s) = self.suffix {
            bar = bar.child(s);
        }
        bar
    }

    fn tab(
        &self,
        t: &Theme,
        ix: usize,
        tab: Tab<S>,
        selected: bool,
        indicator: bool,
        click: Option<Rc<dyn Fn(&mut S)>>,
    ) -> El {
        let v = self.variant;
        let size = self.size;
        let style = if tab.disabled {
            v.disabled(selected, t)
        } else if selected {
            let mut s = v.selected(t);
            // A placed indicator alone shows the selection.
            if indicator {
                match v {
                    TabVariant::Pill => s.bg = Color::TRANSPARENT,
                    TabVariant::Underline => s.border = Color::TRANSPARENT,
                    TabVariant::Segmented => s.inner_bg = Color::TRANSPARENT,
                    _ => {}
                }
            }
            s
        } else {
            v.normal(t)
        };
        let hover = v.hovered(t);
        let interactive = !tab.disabled && !selected;
        let font = match size {
            KitSize::XSmall => ty::ui(rems(0.75)),
            KitSize::Large => ty::ui(rems(1.0)),
            _ => ty::ui(rems(0.875)),
        };
        let font = rune::TextStyle {
            line_height: font.size * 1.25,
            ..font
        };
        let ih = v.inner_height(size);
        let (mt, mb) = v.inner_margins(size);
        let max_w = self.max_width.filter(|_| tab.icon.is_none());
        let ink = |e: El| {
            if interactive {
                e.group_color(hover.fg)
            } else {
                e
            }
        };
        let mut inner = row()
            .h(ih)
            .mt(mt)
            .mb(mb)
            .center()
            .radius(v.inner_radius(size))
            .fill(style.inner_bg);
        if style.shadow && !indicator {
            inner = inner.box_shadow(shadow::kit_xs());
        }
        inner = match tab.icon {
            Some(g) => {
                let px = [rems(0.625), rems(0.875), rems(1.0), rems(1.0)][size_ix(size)];
                inner.w(ih * 1.25).child(ink(glyph(g, px).color(style.fg)))
            }
            None => {
                let mut i = inner.px(v.padding_x(size)).gap(0.0);
                if let Some(l) = tab.label.clone() {
                    let mut l = text(l).font(font).color(style.fg).single_line();
                    if max_w.is_some() {
                        l = l.truncate().shrink();
                    }
                    i = i.child(ink(l));
                }
                i.children(tab.children)
            }
        };
        if max_w.is_some() {
            inner = inner.grow(1.0).shrink().min_w(0.0);
        }
        let key = tab
            .key
            .clone()
            .unwrap_or_else(|| format!("{}-tab{ix}", self.key));
        let mut e = row()
            .key(key.clone())
            .role("tab")
            .checked(selected)
            .center_y()
            .gap(rems(0.25))
            .h(v.height(size))
            .radius(v.radius(size))
            .clip_rounded(v.radius(size).min(v.height(size) / 2.0))
            .fill(style.bg)
            .border_sides(style.borders, style.border)
            .border_layout();
        if let Some(label) = &tab.label {
            e = e.label(label.clone());
        }
        if let Some(w) = max_w {
            e = e.max_w(w);
        }
        if tab.grow {
            e = e.grow(1.0);
        }
        if let Some(p) = tab.prefix {
            e = e.child(p);
        }
        e = e.child(inner);
        if let Some(s) = tab.suffix {
            e = e.child(s);
        }
        if interactive {
            e = e.hover_group();
            if hover.bg != Color::TRANSPARENT {
                e = e.hover(hover.bg);
            }
        }
        if tab.disabled {
            e = e.disabled(true);
        } else {
            e = e
                .focusable(true)
                .focus_selected(selected)
                .cursor(Cursor::Pointer);
            if let Some(f) = click {
                e = e.on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.scroll_into_view(&key);
                    f(s)
                });
            }
        }
        match tab.wrap {
            Some(wrap) => wrap(e),
            None => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime, col};

    struct App {
        picked: usize,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let t = Theme::dark();
        let mut bars = col().padding(20.0).gap(12.0);
        for (i, v) in [
            TabVariant::Tab,
            TabVariant::Outline,
            TabVariant::Pill,
            TabVariant::Segmented,
            TabVariant::Underline,
        ]
        .into_iter()
        .enumerate()
        {
            bars = bars.child(
                TabBar::new(format!("bar{i}"))
                    .variant(v)
                    .selected_index(s.picked)
                    .on_click(|s: &mut App, ix| s.picked = ix)
                    .child(Tab::new().label("Account"))
                    .child(Tab::new().label("Profile"))
                    .child(Tab::new().label("Off").disabled(true))
                    .build(t),
            );
        }
        bars
    }

    fn click(rt: &mut Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn a_click_selects_the_tab_and_disabled_tabs_ignore_it() {
        let mut rt = Runtime::new(
            App { picked: 0 },
            view,
            (600.0, 400.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        click(&mut rt, "bar3-tab1");
        assert_eq!(rt.state.picked, 1);
        click(&mut rt, "bar1-tab0");
        assert_eq!(rt.state.picked, 0);
        click(&mut rt, "bar4-tab2");
        assert_eq!(rt.state.picked, 0);
    }

    #[test]
    fn indicator_lands_under_the_selected_tab() {
        let mut rt = Runtime::new(
            App { picked: 1 },
            view,
            (600.0, 400.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0);
        let tab = rt.find("bar2-tab1").expect("tab");
        let ind = rt
            .find("bar2-indicator")
            .expect("the pill indicator is placed");
        assert!(
            (tab.0 - ind.0).abs() < 1.0 && (tab.1 - ind.1).abs() < 1.0,
            "{tab:?} {ind:?}"
        );
    }
}

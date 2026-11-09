//! gpui-kit's `ColorPicker` and `ColorPickerState` (`color_picker.rs` over
//! `gpui_base::ColorPickerState`), as Settings (chat colour) and the property editor
//! (`chat_color`) use them: a colour square that opens a popover with a Palette / HSLA
//! segmented switch, the featured colours and gpui-kit's nine palette rows (or the hue,
//! saturation, lightness and alpha sliders), and the previewed colour with its hex field.
//! Hovering a swatch previews it; pressing one commits it and closes the picker; Enter in the hex
//! field commits a valid hex; dragging a slider commits as it moves.
//!
//! The app keeps a [`ColorPickerState`] in its state and hands the builder a lens to it.

use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use rune::{Color, Cursor, Drag, El, TextEdit, Tx, col, el, row, text, text_input};

use crate::KitSize;
use crate::calendar::Lens;
use crate::color::{hsl, hsla, to_hsla};
use crate::overlay::{self, Placement};
use crate::tabs::{Tab, TabBar};
use crate::theme::{Theme, radius, rems, shadow, ty};

/// Tailwind's stone (gpui-kit `DEFAULT_COLORS.stone`), 50 … 950; the other palette rows are
/// [`crate::tags::KitColor`] families.
const STONE: [(f32, f32, f32); 11] = [
    (60.0, 9.1, 97.8),
    (60.0, 4.8, 95.9),
    (20.0, 5.9, 90.0),
    (24.0, 5.7, 82.9),
    (24.0, 5.4, 63.9),
    (25.0, 5.3, 44.7),
    (33.3, 5.5, 32.4),
    (30.0, 6.3, 25.1),
    (12.0, 6.5, 15.1),
    (24.0, 9.8, 10.0),
    (20.0, 14.3, 4.1),
];
const SCALES: [u16; 11] = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950];

/// gpui-kit's palette rows (stone, red, orange, yellow, green, cyan, blue, purple, pink), each
/// from its darkest (950) to its lightest (50), as the kit lists them.
fn palettes() -> Vec<Vec<Color>> {
    use crate::tags::KitColor::*;
    let mut rows = vec![
        STONE
            .iter()
            .rev()
            .map(|&(h, s, l)| hsl(h, s, l))
            .collect::<Vec<_>>(),
    ];
    for c in [Red, Orange, Yellow, Green, Cyan, Blue, Purple, Pink] {
        rows.push(SCALES.iter().rev().map(|&n| c.scale(n)).collect());
    }
    rows
}

/// gpui-kit's `Colorize::darken`: lightness × (1 − f).
fn darken(c: Color, f: f32) -> Color {
    let [h, s, l, a] = to_hsla(c);
    hsla(h, s, l * (1.0 - f.clamp(0.0, 1.0)), a)
}

/// gpui-kit's `Colorize::lighten`: lightness × (1 + f).
fn lighten(c: Color, f: f32) -> Color {
    let [h, s, l, a] = to_hsla(c);
    hsla(h, s, (l * (1.0 + f.clamp(0.0, 1.0))).min(1.0), a)
}

/// `#RRGGBB`, or `#RRGGBBAA` when translucent (gpui-base `hex_string`).
pub fn color_hex(c: Color) -> String {
    let [r, g, b] = crate::color::to_srgb(c);
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    if c.a < 1.0 {
        format!("#{:02X}{:02X}{:02X}{:02X}", ch(r), ch(g), ch(b), ch(c.a))
    } else {
        format!("#{:02X}{:02X}{:02X}", ch(r), ch(g), ch(b))
    }
}

/// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`, with or without the `#` (gpui-base `parse_hex`).
pub fn parse_color_hex(value: &str) -> Option<Color> {
    let v = value.strip_prefix('#').unwrap_or(value);
    if !v.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (w, alpha) = match v.len() {
        3 => (1, false),
        4 => (1, true),
        6 => (2, false),
        8 => (2, true),
        _ => return None,
    };
    let part = |i: usize| {
        let s = &v[i * w..(i + 1) * w];
        let n = u8::from_str_radix(s, 16).ok()?;
        Some(if w == 1 { n * 17 } else { n } as f32 / 255.0)
    };
    let a = if alpha { part(3)? } else { 1.0 };
    Some(crate::color::from_srgb([part(0)?, part(1)?, part(2)?], a))
}

/// gpui-kit's `ColorPickerState`: the committed colour, the previewed one, open, the panel
/// (0 Palette, 1 HSLA), the hex field and the four slider values (0..=1).
pub struct ColorPickerState {
    value: Option<Color>,
    preview: Option<Color>,
    open: bool,
    active_tab: usize,
    hex: TextEdit,
    hsla: [f32; 4],
}

impl Default for ColorPickerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorPickerState {
    /// Empty and closed.
    pub fn new() -> Self {
        Self {
            value: None,
            preview: None,
            open: false,
            active_tab: 0,
            hex: TextEdit::new(""),
            hsla: [0.0; 4],
        }
    }
    /// The initial committed (and previewed) colour.
    pub fn default_value(mut self, c: Color) -> Self {
        self.set_value(c);
        self
    }
    pub fn value(&self) -> Option<Color> {
        self.value
    }
    pub fn preview(&self) -> Option<Color> {
        self.preview
    }
    /// The previewed colour, else the committed one.
    pub fn displayed_color(&self) -> Option<Color> {
        self.preview.or(self.value)
    }
    /// The hex field's text.
    pub fn hex_text(&self) -> &str {
        &self.hex.text
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }
    pub fn toggle_open(&mut self) {
        self.open = !self.open;
    }
    pub fn active_tab(&self) -> usize {
        self.active_tab
    }
    pub fn set_active_tab(&mut self, tab: usize) {
        self.active_tab = tab;
    }
    /// Replaces the committed colour (no change event).
    pub fn set_value(&mut self, c: Color) {
        self.update(Some(c));
    }
    /// Clears the committed colour (no change event).
    pub fn clear_value(&mut self) {
        self.update(None);
    }
    /// Shows `c` as the preview and in the hex field.
    pub fn preview_color(&mut self, c: Color) {
        self.preview = Some(c);
        self.hex.replace_text(color_hex(c));
    }
    /// Back to the committed colour.
    pub fn clear_preview(&mut self) {
        if self.preview != self.value {
            self.preview = self.value;
            self.hex
                .replace_text(self.value.map(color_hex).unwrap_or_default());
        }
    }
    /// Previews a typed hex (and moves the sliders to it); false when it doesn't parse.
    pub fn preview_hex(&mut self, value: &str) -> bool {
        let Some(c) = parse_color_hex(value) else {
            return false;
        };
        self.preview = Some(c);
        self.hsla = to_hsla(c);
        true
    }
    /// Commits a palette pick and closes, as a swatch press does.
    pub fn select_color(&mut self, c: Color) {
        self.open = false;
        self.update(Some(c));
    }
    /// Parses and commits a hex, closing on success.
    pub fn commit_hex(&mut self, value: &str) -> Option<Color> {
        let c = parse_color_hex(value)?;
        self.select_color(c);
        Some(c)
    }
    /// Sets one slider (0 hue, 1 saturation, 2 lightness, 3 alpha) and commits the colour they
    /// make, leaving the picker open.
    pub fn set_component(&mut self, i: usize, v: f32) -> Color {
        self.hsla[i] = (v.clamp(0.0, 1.0) * 100.0).round() / 100.0;
        let [h, s, l, a] = self.hsla;
        let c = hsla(h, s, l, a);
        self.value = Some(c);
        self.preview = Some(c);
        self.hex.replace_text(color_hex(c));
        c
    }
    fn update(&mut self, c: Option<Color>) {
        self.value = c;
        self.preview = c;
        self.hex.replace_text(c.map(color_hex).unwrap_or_default());
        if let Some(c) = c {
            self.hsla = to_hsla(c);
        }
    }
}

type Change<S> = Rc<dyn Fn(&mut S, Option<Color>)>;

/// gpui-kit's `ColorPicker`: the colour square trigger and, while open, the picker popover.
pub struct ColorPicker<S> {
    key: String,
    lens: Lens<S, ColorPickerState>,
    value: Option<Color>,
    preview: Option<Color>,
    open: bool,
    tab: usize,
    hex: TextEdit,
    hsla: [f32; 4],
    size: KitSize,
    label: Option<String>,
    a11y: Option<String>,
    featured: Option<Vec<Color>>,
    on_change: Option<Change<S>>,
}

impl<S: 'static> ColorPicker<S> {
    /// The trigger is keyed `{key}`, the swatches `{key}-featured{i}` and `{key}-#RRGGBB`.
    pub fn new(
        key: impl Into<String>,
        state: &ColorPickerState,
        lens: impl for<'a> Fn(&'a mut S) -> &'a mut ColorPickerState + 'static,
    ) -> Self {
        Self {
            key: key.into(),
            lens: Rc::new(lens),
            value: state.value,
            preview: state.preview,
            open: state.open,
            tab: state.active_tab,
            hex: state.hex.clone(),
            hsla: state.hsla,
            size: KitSize::Medium,
            label: None,
            a11y: None,
            featured: None,
            on_change: None,
        }
    }
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = s;
        self
    }
    /// Text beside the square.
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    /// The announced name instead of the label.
    pub fn accessibility_label(mut self, l: impl Into<String>) -> Self {
        self.a11y = Some(l.into());
        self
    }
    /// The quick-pick row (default: gpui-kit's red, blue, green, yellow, cyan, magenta and
    /// their light shades).
    pub fn featured_colors(mut self, colors: Vec<Color>) -> Self {
        self.featured = Some(colors);
        self
    }
    /// gpui-kit's `ColorPickerEvent::Change`: a committed colour (swatch, hex Enter, slider).
    pub fn on_change(mut self, f: impl Fn(&mut S, Option<Color>) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }

    /// `f` on the state, then `on_change` with the colour it returns (if any).
    fn commit(
        &self,
        f: impl Fn(&mut ColorPickerState) -> Option<Color> + 'static,
    ) -> impl Fn(&mut S) + 'static {
        let lens = self.lens.clone();
        let change = self.on_change.clone();
        move |s: &mut S| {
            if let Some(c) = f(lens(s))
                && let Some(g) = &change
            {
                g(s, Some(c));
            }
        }
    }

    /// The colour square (`ColorPickerButton`).
    pub fn trigger(&self, t: &Theme) -> El {
        let side = match self.size {
            KitSize::Large => rems(2.75),
            KitSize::Small => rems(1.25),
            KitSize::XSmall => rems(1.0),
            KitSize::Px(p) => p,
            _ => rems(2.0),
        };
        let mut square = el()
            .key(format!("{}-square", self.key))
            .size(side, side)
            .radius(radius::KIT)
            .clip_rounded(radius::KIT)
            .fill(t.kit.background)
            .border(1.0, t.kit.input)
            .border_layout();
        if let Some(v) = self.value {
            square = square.fill(v).border(1.0, darken(v, 0.3)).border_layout();
            square = crate::tooltip::tooltip(square, color_hex(v));
        }
        let lens = self.lens.clone();
        let mut e = row()
            .key(self.key.clone())
            .role("button")
            .expanded(self.open)
            .center_y()
            .gap(rems(0.5))
            .focusable(true)
            .cursor(Cursor::Pointer)
            .on_press(move |s: &mut S| lens(s).toggle_open())
            .child(square);
        if let Some(l) = self.a11y.clone().or_else(|| self.label.clone()) {
            e = e.label(l);
        }
        if let Some(l) = &self.label {
            e = e.child(
                text(l.clone())
                    .font(ty::ui(rems(1.0)))
                    .color(t.kit.foreground),
            );
        }
        e
    }

    fn swatch(&self, t: &Theme, key: String, c: Color) -> El {
        let _ = t;
        let preview = {
            let lens = self.lens.clone();
            move |s: &mut S| lens(s).preview_color(c)
        };
        el().key(key)
            .role("radio")
            .label(color_hex(c))
            .checked(self.value == Some(c))
            .size(rems(1.25), rems(1.25))
            .fill(c)
            .border(1.0, darken(c, 0.1))
            .border_layout()
            .hover(lighten(c, 0.1))
            .press(darken(c, 0.2))
            .cursor(Cursor::Pointer)
            .focusable(true)
            .on_hover(preview)
            .on_press(self.commit(move |st| {
                st.select_color(c);
                Some(c)
            }))
    }

    fn separator(t: &Theme) -> El {
        el().w_full().h(1.0).fill(t.kit.border)
    }

    fn palette_panel(&self, t: &Theme) -> El {
        let k = &t.kit;
        let featured = self.featured.clone().unwrap_or_else(|| {
            vec![
                k.red,
                k.red_light,
                k.blue,
                k.blue_light,
                k.green,
                k.green_light,
                k.yellow,
                k.yellow_light,
                k.cyan,
                k.cyan_light,
                k.magenta,
                k.magenta_light,
            ]
        });
        let mut feat = row().gap(rems(0.25));
        for (i, c) in featured.into_iter().enumerate() {
            feat = feat.child(self.swatch(t, format!("{}-featured{i}", self.key), c));
        }
        let mut rows = col().gap(rems(0.25));
        for colors in palettes() {
            let mut r = row().gap(rems(0.25));
            for c in colors {
                r = r.child(self.swatch(t, format!("{}-{}", self.key, color_hex(c)), c));
            }
            rows = rows.child(r);
        }
        col()
            .gap(rems(0.75))
            .child(feat)
            .child(Self::separator(t))
            .child(rows)
    }

    /// One HSLA row: label, gradient track with the kit slider thumb, value.
    fn slider(&self, t: &Theme, i: usize, name: &str, track: El, shown: String) -> El {
        let label_color = t.kit.foreground.alpha(t.kit.foreground.a * 0.7);
        let small = ty::ui(rems(0.75));
        let v = self.hsla[i];
        let lens = self.lens.clone();
        let change = self.on_change.clone();
        let thumb = row()
            .key(format!("{}-thumb{i}", self.key))
            .size(rems(1.0), rems(1.0))
            .padding(1.0)
            .ml(-8.0)
            .radius(9999.0)
            .child(
                el().size(rems(1.0) - 2.0, rems(1.0) - 2.0)
                    .radius(9999.0)
                    .fill(t.kit.slider_thumb),
            );
        let rail = row()
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .center_y()
            .child(el().w_frac(v).h(1.0))
            .child(thumb);
        let area = el()
            .key(format!("{}-slider{i}", self.key))
            .role("slider")
            .label(name.to_owned())
            .grow(1.0)
            .h(rems(2.0))
            .cursor(Cursor::Pointer)
            .child(
                track
                    .pin(
                        Some(0.0),
                        Some((rems(2.0) - rems(0.625)) / 2.0),
                        Some(0.0),
                        None,
                    )
                    .h(rems(0.625)),
            )
            .child(rail)
            .on_drag(move |s: &mut S, d: &mut Drag, _: &mut Tx| {
                let x = if d.w > 0.0 { d.x / d.w } else { 0.0 };
                let c = lens(s).set_component(i, x);
                if let Some(f) = &change {
                    f(s, Some(c));
                }
            });
        row()
            .gap(rems(0.5))
            .center_y()
            .child(
                el().min_w(rems(4.0))
                    .child(text(name.to_owned()).font(small).color(label_color)),
            )
            .child(area)
            .child(
                row()
                    .w(rems(2.5))
                    .justify_end()
                    .child(text(shown).font(small).color(label_color)),
            )
    }

    fn sliders_panel(&self, t: &Theme) -> El {
        let [h, s, l, a] = self.displayed_hsla();
        let hue: Vec<(f32, Color)> = (0..=6)
            .map(|i| (i as f32 / 6.0, hsla(i as f32 / 6.0, 1.0, 0.5, 1.0)))
            .collect();
        let light = [
            (0.0, hsla(h, 1.0, 0.0, 1.0)),
            (0.5, hsla(h, 1.0, 0.5, 1.0)),
            (1.0, hsla(h, 1.0, 1.0, 1.0)),
        ];
        let sat = [(0.0, hsla(h, 0.0, l, 1.0)), (1.0, hsla(h, 1.0, l, 1.0))];
        let alpha = [(0.0, hsla(h, s, l, 0.0)), (1.0, hsla(h, s, l, 1.0))];
        let track =
            |stops: &[(f32, Color)]| el().clip_rounded(0.0).linear_gradient(FRAC_PI_2, stops);
        col()
            .gap(rems(0.5))
            .child(self.slider(t, 0, "Hue", track(&hue), format!("{:.0}", h * 360.0)))
            .child(self.slider(t, 1, "Saturation", track(&sat), format!("{:.0}", s * 100.0)))
            .child(self.slider(
                t,
                2,
                "Lightness",
                track(&light),
                format!("{:.0}", l * 100.0),
            ))
            .child(self.slider(t, 3, "Alpha", track(&alpha), format!("{:.0}", a * 100.0)))
    }

    /// The colour the sliders show: the preview's, else black (gpui-kit).
    fn displayed_hsla(&self) -> [f32; 4] {
        self.preview
            .or(self.value)
            .map(to_hsla)
            .unwrap_or([0.0, 0.0, 0.0, 1.0])
    }

    /// The popover body (`render_colors`): 18 rem wide, popover look, 0.75 rem padding.
    pub fn panel(&self, t: &Theme) -> El {
        let tabs_lens = self.lens.clone();
        let bar = TabBar::<S>::new(format!("{}-mode", self.key))
            .segmented()
            .selected_index(self.tab)
            .on_click(move |s: &mut S, ix| tabs_lens(s).set_active_tab(ix))
            .child(Tab::new().label("Palette").flex_1())
            .child(Tab::new().label("HSLA").flex_1())
            .build(t);
        let mut body =
            col()
                .padding(rems(0.125))
                .gap(rems(0.75))
                .child(bar)
                .child(if self.tab == 0 {
                    self.palette_panel(t)
                } else {
                    self.sliders_panel(t)
                });
        if let Some(p) = self.preview {
            let lens = self.lens.clone();
            let submit_lens = self.lens.clone();
            let change = self.on_change.clone();
            let bg = t
                .kit
                .input
                .alpha(t.kit.input.a * if t.dark { 0.3 } else { 0.0 });
            let bg = if t.dark { bg } else { t.kit.background };
            let field = text_input(&self.hex, ty::ui(rems(0.875)))
                .key(format!("{}-hex", self.key))
                .single_line()
                .color(t.kit.foreground)
                .on_edit(move |s: &mut S| &mut lens(s).hex)
                .on_input_change({
                    let lens = self.lens.clone();
                    move |s: &mut S, _: &mut Tx| {
                        let st = lens(s);
                        let text = st.hex.text.clone();
                        st.preview_hex(&text);
                    }
                })
                .on_submit(move |s: &mut S, _: &mut Tx| {
                    let st = submit_lens(s);
                    let text = st.hex.text.clone();
                    if let Some(c) = st.commit_hex(&text)
                        && let Some(f) = &change
                    {
                        f(s, Some(c));
                    }
                });
            body = body.child(Self::separator(t)).child(
                row()
                    .gap(rems(0.5))
                    .center_y()
                    .child(
                        el().size(rems(1.25), rems(1.25))
                            .radius(radius::KIT)
                            .fill(p)
                            .border(1.0, darken(p, 0.2))
                            .border_layout(),
                    )
                    .child(
                        row()
                            .grow(1.0)
                            .h(rems(1.5))
                            .px(rems(0.625))
                            .py(2.0)
                            .center_y()
                            .fill(bg)
                            .border(1.0, t.kit.input)
                            .border_layout()
                            .radius(radius::KIT)
                            .child(field.grow(1.0)),
                    ),
            );
        }
        let mut surface = col()
            .key(format!("{}-popover", self.key))
            .w(rems(18.0))
            .padding(rems(0.75))
            .fill(t.kit.popover)
            .radius(radius::KIT)
            .child(body);
        for s in shadow::kit_popover(t) {
            surface = surface.box_shadow(s);
        }
        surface
    }

    /// The trigger in place; while open, the popover below it in `layer` (closed by an
    /// outside press or Escape).
    pub fn build(self, t: &Theme, layer: &mut Vec<El>) -> El {
        let trigger = self.trigger(t);
        if self.open {
            let lens = self.lens.clone();
            let surface =
                overlay::anchored(self.panel(t), &self.key, Placement::BottomStart, rems(0.25));
            let surface = overlay::dismissable(
                surface,
                &self.key,
                move |s: &mut S| lens(s).set_open(false),
                layer,
            );
            layer.push(surface);
        }
        trigger
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime};

    struct App {
        picker: ColorPickerState,
        changes: Vec<String>,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let t = Theme::dark();
        let mut layer = Vec::new();
        let base = col().padding(20.0).child(
            ColorPicker::new("chat", &s.picker, |s: &mut App| &mut s.picker)
                .on_change(|s: &mut App, c| s.changes.push(c.map(color_hex).unwrap_or_default()))
                .build(t, &mut layer),
        );
        crate::root(base, layer)
    }

    fn click(rt: &mut Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn choosing_a_swatch_fires_on_change_with_its_hex_and_closes() {
        let state = ColorPickerState::new().default_value(crate::color::rgb(0xEDCF7F));
        let mut rt = Runtime::new(
            App {
                picker: state,
                changes: Vec::new(),
            },
            view,
            (500.0, 600.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        click(&mut rt, "chat");
        assert!(rt.state.picker.is_open());
        rt.frame(0.5);
        let red = color_hex(Theme::dark().kit.red);
        click(&mut rt, "chat-featured0");
        assert_eq!(rt.state.changes, [red.clone()]);
        assert_eq!(rt.state.picker.value().map(color_hex), Some(red));
        assert!(!rt.state.picker.is_open());
    }

    #[test]
    fn hex_round_trips_and_partial_input_does_not_preview() {
        assert_eq!(
            parse_color_hex("#EDCF7F").map(color_hex).as_deref(),
            Some("#EDCF7F")
        );
        assert_eq!(
            parse_color_hex("fff").map(color_hex).as_deref(),
            Some("#FFFFFF")
        );
        assert_eq!(
            parse_color_hex("#ff000080").map(color_hex).as_deref(),
            Some("#FF000080")
        );
        let mut st = ColorPickerState::new().default_value(crate::color::rgb(0x112233));
        assert!(!st.preview_hex("#12"));
        assert_eq!(st.preview().map(color_hex).as_deref(), Some("#112233"));
    }
}

//! Dialogs: gpui-kit's `Dialog` (what `window.open_dialog` showed: the dimmed layer, a centred
//! surface with title, description, body, footer and close button, its entrance per
//! [`DialogEntrance`]) and Roostr's [`modal_frame`] (`roostr_core::ui::modal_frame`), the bare
//! panel the search modal draws in.
//!
//! An open dialog is modal: focus is trapped inside, Escape and (unless `overlay_closable(false)`)
//! a press on the dim close it through [`Dialog::on_close`], and Return presses the footer button
//! marked `El::default_button()` (or runs [`Dialog::on_ok`]).

use std::rc::Rc;

use rune::{El, Key, Spring, SpringPair, Transition, Tx, col, el, row, text};

use crate::icons::{Glyph, glyph};
use crate::theme::ty::Weights as _;
use crate::theme::{Theme, radius, rems, shadow, ty};
use crate::{motion, overlay};

/// A modal panel (`roostr_core::ui::modal_frame`): panel fill, 1px border, radius 14, the modal
/// shadow, `width` wide, clipping its content. Callers add padding and the overlay.
pub fn modal_frame(t: &Theme, width: f32) -> El {
    col()
        .w(width)
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(radius::MODAL)
        .box_shadow(shadow::modal())
        .clip_rounded(radius::MODAL)
}

/// How a [`Dialog`] enters (gpui-kit `DialogEntrance`); the dim fades in with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DialogEntrance {
    /// Slides down from the window top while fading in, over 250 ms.
    #[default]
    SlideDown,
    /// Fades in in place over 180 ms.
    Fade,
    /// Fades in while settling 0.25 rem from above, over 180 ms.
    FadeSlide,
    /// Shows at once.
    None,
}

type Close = Box<dyn FnOnce(El, &str, bool, bool, &mut Vec<El>) -> El>;

/// gpui-kit's `Dialog`: `Dialog::new("confirm").title("Delete?").child(body).footer(buttons)
/// .open(s.open).on_close(|s: &mut S| s.open = false).build(t, &mut layer)`.
///
/// Defaults are the kit's: 448 wide, padding 16, radius 10, `background` fill, 1px border,
/// `shadow-xl`, a tenth of the window down, the close button at the top right. Roostr's small
/// cards override fill, radius, padding and width ([`Dialog::fill`], [`Dialog::radius`],
/// [`Dialog::padding`], [`Dialog::width`]) and are `centered`.
pub struct Dialog {
    key: String,
    title: Option<String>,
    description: Option<String>,
    body: Vec<El>,
    footer: Vec<El>,
    width: f32,
    padding: f32,
    radius: f32,
    fill: Option<rune::Color>,
    border: Option<rune::Color>,
    shadow: Option<rune::Shadow>,
    margin_top: Option<f32>,
    centered: bool,
    close_button: bool,
    overlay: bool,
    overlay_closable: bool,
    keyboard: bool,
    entrance: DialogEntrance,
    open: bool,
    close: Option<Close>,
    close_press: Option<Box<dyn FnOnce(El) -> El>>,
    ok: Option<Box<dyn FnOnce(El) -> El>>,
}

impl Dialog {
    pub fn new(key: impl Into<String>) -> Dialog {
        Dialog {
            key: key.into(),
            title: None,
            description: None,
            body: Vec::new(),
            footer: Vec::new(),
            width: 448.0,
            padding: 16.0,
            radius: radius::KIT_LG,
            fill: None,
            border: None,
            shadow: None,
            margin_top: None,
            centered: false,
            close_button: true,
            overlay: true,
            overlay_closable: true,
            keyboard: true,
            entrance: DialogEntrance::SlideDown,
            open: false,
            close: None,
            close_press: None,
            ok: None,
        }
    }
    /// The title: 1 rem semibold, line height 1.25.
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }
    /// The description under the title: 0.875 rem, muted.
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }
    pub fn child(mut self, c: El) -> Self {
        self.body.push(c);
        self
    }
    /// A footer button (right-aligned, 0.5 rem apart). Mark the one Return presses with
    /// `El::default_button()`.
    pub fn footer(mut self, button: El) -> Self {
        self.footer.push(button);
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    /// The surface's inset on every side (the kit's 16 by default).
    pub fn padding(mut self, p: f32) -> Self {
        self.padding = p;
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = r;
        self
    }
    pub fn fill(mut self, c: rune::Color) -> Self {
        self.fill = Some(c);
        self
    }
    pub fn border_color(mut self, c: rune::Color) -> Self {
        self.border = Some(c);
        self
    }
    /// One shadow in place of the kit's `shadow-xl` (Roostr's settings use the modal shadow).
    pub fn shadow(mut self, s: rune::Shadow) -> Self {
        self.shadow = Some(s);
        self
    }
    /// The distance from the window top (a tenth of the window by default).
    pub fn margin_top(mut self, m: f32) -> Self {
        self.margin_top = Some(m);
        self
    }
    /// Centre it vertically.
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }
    pub fn close_button(mut self, on: bool) -> Self {
        self.close_button = on;
        self
    }
    /// Whether to dim the window behind it (`t.kit.overlay`).
    pub fn overlay(mut self, on: bool) -> Self {
        self.overlay = on;
        self
    }
    pub fn overlay_closable(mut self, on: bool) -> Self {
        self.overlay_closable = on;
        self
    }
    /// Whether Escape closes it.
    pub fn keyboard(mut self, on: bool) -> Self {
        self.keyboard = on;
        self
    }
    pub fn entrance(mut self, e: DialogEntrance) -> Self {
        self.entrance = e;
        self
    }
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
    /// Close requests: Escape, the dim, the close button.
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        let f = Rc::new(f);
        let button = f.clone();
        self.close_press = Some(Box::new(move |e: El| {
            e.on_press(move |s: &mut S| button(s))
        }));
        self.close = Some(Box::new(
            move |surface: El, key: &str, outside: bool, keyboard: bool, layer: &mut Vec<El>| {
                if outside && keyboard {
                    overlay::dismissable(surface, key, move |s: &mut S| f(s), layer)
                } else if outside {
                    let g = f.clone();
                    layer.push(overlay::scrim(format!("{key}-scrim"), move |s: &mut S| {
                        g(s)
                    }));
                    surface.modal(true, |_: &mut S| {})
                } else if keyboard {
                    surface.modal(false, move |s: &mut S| f(s))
                } else {
                    surface.modal(false, |_: &mut S| {})
                }
            },
        ));
        self
    }
    /// Return with no default button in the footer (gpui-kit's `on_ok`).
    pub fn on_ok<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.ok = Some(Box::new(move |e: El| {
            e.on_key(move |s: &mut S, k: &rune::KeyEvent, _: &mut Tx| {
                if k.key == Key::Enter && k.mods == rune::Mods::default() {
                    f(s);
                    true
                } else {
                    false
                }
            })
        }));
        self
    }

    /// The surface alone, laid out in place (for galleries, or a host that positions it).
    pub fn panel(self, t: &Theme) -> El {
        self.surface(t)
    }

    fn surface(mut self, t: &Theme) -> El {
        let p = self.padding;
        let mut inner = col()
            .grow(1.0)
            .shrink()
            .min_h(0.0)
            .gap(rems(0.5))
            .items_stretch();
        if let Some(title) = self.title.take() {
            inner = inner.child(
                row().px(p).child(
                    text(title)
                        .key("title")
                        .font(ty::ui(rems(1.0)).semibold().lh(rems(1.0) * 1.25))
                        .color(t.kit.foreground)
                        .wrap(),
                ),
            );
        }
        if let Some(d) = self.description.take() {
            inner = inner.child(
                row().px(p).child(
                    text(d)
                        .key("description")
                        .font(ty::ui(rems(0.875)))
                        .color(t.kit.muted_foreground)
                        .wrap(),
                ),
            );
        }
        if !self.body.is_empty() {
            inner = inner.child(
                col()
                    .key("body")
                    .px(p)
                    .shrink()
                    .min_h(0.0)
                    .items_stretch()
                    .scroll(rune::ScrollSpec {
                        stick_bottom: false,
                    })
                    .children(std::mem::take(&mut self.body)),
            );
        }
        let mut s = col()
            .key(format!("{}-dialog", self.key))
            .role("dialog")
            .w(self.width)
            .min_h(rems(6.0))
            .pt(p)
            .pb(p)
            .gap(p.max(8.0))
            .items_stretch()
            .fill(self.fill.unwrap_or(t.kit.background))
            .border(1.0, self.border.unwrap_or(t.kit.border))
            .radius(self.radius)
            .child(inner);
        match self.shadow {
            Some(sh) => s = s.box_shadow(sh),
            None => {
                for sh in shadow::kit_xl() {
                    s = s.box_shadow(sh);
                }
            }
        }
        if !self.footer.is_empty() {
            s = s.child(
                row()
                    .key("footer")
                    .px(p)
                    .gap(rems(0.5))
                    .justify_end()
                    .center_y()
                    .children(std::mem::take(&mut self.footer)),
            );
        }
        if self.close_button {
            let inset = (p - 10.0).max(8.0);
            let size = rems(1.5);
            let hover = if t.dark {
                t.kit.accent.alpha(t.kit.accent.a * 0.5)
            } else {
                t.kit.accent
            };
            let mut close = row()
                .key(format!("{}-close", self.key))
                .role("button")
                .label("Close")
                .absolute(self.width - inset - size, inset)
                .size(size, size)
                .center()
                .radius(radius::KIT)
                .hover(hover)
                .hover_group()
                .cursor(rune::Cursor::Pointer)
                .child(
                    glyph(Glyph::Close, rems(0.875))
                        .color(t.kit.secondary_foreground)
                        .group_color(t.kit.accent_foreground),
                );
            if let Some(f) = self.close_press.take() {
                close = f(close);
            }
            s = s.child(close);
        }
        if let Some(ok) = self.ok.take() {
            s = ok(s);
        }
        s
    }

    /// The panel's entrance: it springs into place on [`motion::POP`], a slight bounce (a
    /// fading one also grows in a touch so the bounce shows); it leaves at once.
    fn transition(&self, top: f32) -> Transition {
        let from_y = match self.entrance {
            DialogEntrance::SlideDown => -top,
            DialogEntrance::Fade => 0.0,
            DialogEntrance::FadeSlide => -rems(0.25),
            DialogEntrance::None => return Transition::none(),
        };
        let grow = if matches!(self.entrance, DialogEntrance::SlideDown) {
            1.0
        } else {
            0.96
        };
        motion::pop(rune::Effect {
            offset: rune::Vec2::new(0.0, from_y),
            ..rune::Effect::scale(grow).combined(rune::Effect::opacity(0.0))
        })
    }

    /// The scrim fading in under the panel (no bounce: it only fades), gone at once on close.
    fn dim_transition(&self) -> Transition {
        let secs = match self.entrance {
            DialogEntrance::SlideDown => 0.25,
            DialogEntrance::Fade | DialogEntrance::FadeSlide => 0.18,
            DialogEntrance::None => return Transition::none(),
        };
        Transition {
            insertion: rune::Effect::opacity(0.0),
            removal: rune::Effect::IDENTITY,
            springs: SpringPair::new(Spring::new(secs, 0.0), Spring::NONE),
            ..Transition::identity()
        }
    }

    pub fn build(mut self, t: &Theme, layer: &mut Vec<El>) {
        if !self.open {
            return;
        }
        let key = self.key.clone();
        let margin = 16.0;
        let top = self.margin_top.unwrap_or(0.0);
        let enter = self.transition(top.max(48.0));
        let dim_enter = self.dim_transition();
        if self.overlay {
            layer.push(
                el().key(format!("{key}-overlay"))
                    .absolute(0.0, 0.0)
                    .w_full()
                    .h_full()
                    .fill(t.kit.overlay)
                    .transition(dim_enter),
            );
        }
        let (outside, keyboard, centered, margin_top) = (
            self.overlay_closable,
            self.keyboard,
            self.centered,
            self.margin_top,
        );
        let close = self.close.take();
        let mut surface = self.surface(t).transition(enter).instant_changes();
        if let Some(c) = close {
            surface = c(surface, &key, outside, keyboard, layer);
        }
        let host = col()
            .key(format!("{key}-host"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .px(margin)
            .items_center();
        let host = if centered {
            host.py(margin).justify_center().child(surface)
        } else if let Some(m) = margin_top {
            host.pt(m.max(margin)).pb(margin).child(surface)
        } else {
            // A tenth of the window down: one part above, nine below.
            host.child(el().grow(1.0).h(0.0))
                .child(col().grow(9.0).h(0.0).items_center().child(surface))
        };
        layer.push(host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, KeyEvent, Mods, Runtime};

    #[derive(Default)]
    struct S {
        open: bool,
        deleted: bool,
    }

    fn view(s: &S, _: &mut rune::Cx<S>) -> El {
        let t = Theme::dark();
        let mut layer = Vec::new();
        Dialog::new("d")
            .title("Delete \"Status\"?")
            .description("This cannot be undone.")
            .footer(
                row()
                    .key("cancel")
                    .w(60.0)
                    .h(28.0)
                    .role("button")
                    .on_press(|s: &mut S| s.open = false),
            )
            .footer(
                row()
                    .key("delete")
                    .w(60.0)
                    .h(28.0)
                    .role("button")
                    .default_button()
                    .on_press(|s: &mut S| {
                        s.deleted = true;
                        s.open = false
                    }),
            )
            .open(s.open)
            .on_close(|s: &mut S| s.open = false)
            .build(t, &mut layer);
        let opener = row()
            .key("open")
            .w(80.0)
            .h(28.0)
            .role("button")
            .on_press(|s: &mut S| s.open = true);
        crate::root(col().w_full().h_full().padding(20.0).child(opener), layer)
    }

    fn opened() -> Runtime<S> {
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
        let (x, y) = rt.find("open").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0);
        assert!(rt.state.open);
        rt
    }

    #[test]
    fn sits_a_tenth_down_centred() {
        let rt = opened();
        let d = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("d-dialog"))
            .expect("open");
        assert!(
            (d.rect[0] - (800.0 - 448.0) / 2.0).abs() < 1.0,
            "{:?}",
            d.rect
        );
        assert!((d.rect[1] - 60.0).abs() < 1.0, "{:?}", d.rect);
        assert!((d.rect[2] - 448.0).abs() < 1.0, "{:?}", d.rect);
    }

    #[test]
    fn escape_closes() {
        let mut rt = opened();
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.open);
        assert!(!rt.state.deleted);
    }

    #[test]
    fn return_presses_the_default_button() {
        let mut rt = opened();
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert!(rt.state.deleted);
        assert!(!rt.state.open);
    }

    #[test]
    fn outside_press_closes_and_close_button_closes() {
        let mut rt = opened();
        rt.pointer_down(10.0, 590.0);
        rt.pointer_up(10.0, 590.0);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.open);
        let mut rt = opened();
        let (x, y) = rt.find("d-close").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.open);
    }
}

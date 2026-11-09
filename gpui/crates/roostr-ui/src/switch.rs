//! gpui-kit's `Switch`: a pill track (`switch` off, `primary` or a custom colour on) with a
//! `switch_thumb` knob that slides on the control motion, and an optional label.

use std::rc::Rc;

use rune::{Color, Cursor, El, Tx, el, row, text};

use crate::KitSize;
use crate::input::{fade, input_text};
use crate::motion;
use crate::overlay::Acts;
use crate::theme::{Theme, rems, ty};

/// gpui-kit's `Switch`. Sizes: XSmall/Small 28 × 16 (12pt knob), Medium 36 × 20 (16pt),
/// Large 44 × 24 (20pt); the knob sits 2pt inside the track. Controlled: [`Self::on_change`]
/// gets the requested value.
pub struct Switch {
    key: String,
    label: Option<String>,
    checked: bool,
    disabled: bool,
    size: KitSize,
    color: Option<Color>,
    label_left: bool,
    tab_stop: bool,
    tooltip: Option<String>,
    acts: Acts,
}

impl Switch {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: None,
            checked: false,
            disabled: false,
            size: KitSize::Medium,
            color: None,
            label_left: false,
            tab_stop: true,
            tooltip: None,
            acts: Acts::default(),
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// `label_side(Side::Left)`: the label before the track.
    pub fn label_left(mut self) -> Self {
        self.label_left = true;
        self
    }

    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }

    pub fn size(mut self, size: KitSize) -> Self {
        self.size = size;
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

    /// The checked track's colour, in place of `primary` (`.color(..)`).
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }

    pub fn tab_stop(mut self, on: bool) -> Self {
        self.tab_stop = on;
        self
    }

    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// A press asks for the other value: `f(state, !checked)`.
    pub fn on_change<S: 'static>(self, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.on_change_tx(move |s: &mut S, v, _: &mut Tx| f(s, v))
    }

    pub fn on_change_tx<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        let next = !self.checked;
        let f = Rc::new(f);
        self.acts
            .press_tx(move |s: &mut S, tx: &mut Tx| f(s, next, tx));
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let key = self.key;
        let (checked, disabled) = (self.checked, self.disabled);
        let (w, h) = match self.size {
            KitSize::XSmall | KitSize::Small => (28.0, 16.0),
            KitSize::Large => (44.0, 24.0),
            _ => (36.0, 20.0),
        };
        let bar = match self.size {
            KitSize::XSmall | KitSize::Small => 12.0,
            KitSize::Large => 20.0,
            _ => 16.0,
        };
        let inset = 2.0;
        let on = self.color.unwrap_or(t.kit.primary);
        let fill = if checked { on } else { t.kit.switch };
        let fill = if disabled { fade(fill, 0.5) } else { fill };
        let x = if checked { w - bar - inset * 2.0 } else { 0.0 };
        let spring = motion::CONTROL.spring();
        let thumb = el()
            .key(format!("{key}-thumb"))
            .size(bar, bar)
            .absolute(inset + x, (h - bar) / 2.0)
            .radius(h)
            .fill(t.kit.switch_thumb)
            .motion(spring);
        let mut track = el()
            .key(format!("{key}-track"))
            .size(w, h)
            .radius(h)
            .fill(fill)
            .motion(spring)
            .focus_ring(3.0, 0.0, fade(t.kit.ring, 0.5))
            .child(thumb)
            .a11y_hidden();
        if let Some(tip) = self.tooltip {
            track = crate::tooltip::tooltip(track, tip);
        }
        let mut root = row()
            .key(key.clone())
            .role("switch")
            .checked(checked)
            .gap(rems(0.5))
            .items_start();
        if let Some(l) = &self.label {
            root = root.label(l.clone());
        }
        let label = self.label.map(|l| {
            let size = match self.size {
                KitSize::Px(_) => input_text(KitSize::Medium),
                s => input_text(s),
            };
            let color = if disabled {
                t.kit.muted_foreground
            } else {
                t.kit.foreground
            };
            text(l)
                .key(format!("{key}-label"))
                .font(ty::ui(size).lh(h))
                .color(color)
                .min_w(0.0)
                .a11y_hidden()
        });
        root = match (label, self.label_left) {
            (Some(l), true) => root.child(l).child(track),
            (Some(l), false) => root.child(track).child(l),
            (None, _) => root.child(track),
        };
        root = root
            .focusable(self.tab_stop && !disabled)
            .disabled(disabled);
        if disabled {
            root = root.cursor(Cursor::NotAllowed);
        }
        let interactive = !disabled && !self.acts.is_empty();
        self.acts.apply(root, interactive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime, col};

    #[derive(Default)]
    struct S {
        on: bool,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let t = Theme::light();
        col()
            .padding(20.0)
            .gap(20.0)
            .child(
                Switch::new("wifi")
                    .label("Wi-Fi")
                    .checked(s.on)
                    .on_change(|s: &mut S, v| s.on = v)
                    .build(t),
            )
            .child(
                Switch::new("off")
                    .label("Locked")
                    .disabled(true)
                    .on_change(|s: &mut S, v| s.on = v)
                    .build(t),
            )
    }

    #[test]
    fn switch_toggles_and_its_knob_slides() {
        let mut rt = Runtime::new(
            S::default(),
            view,
            (300.0, 200.0),
            Env {
                system_dark: false,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        let (x0, _) = rt.find("wifi-thumb").unwrap();
        let (x, y) = rt.find("wifi-track").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.on);
        let (x1, _) = rt.find("wifi-thumb").unwrap();
        assert!((x1 - x0 - 16.0).abs() < 0.5, "{x0} → {x1}");
        let (x, y) = rt.find("wifi-label").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.on);
        let (x, y) = rt.find("off").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.on);
    }
}

//! Checkboxes: gpui-kit's `Checkbox` ([`Checkbox`]) and Roostr's `ui::checkbox` /
//! `ui::checkbox_display` ([`checkbox`], [`checkbox_display`]).

use std::rc::Rc;

use rune::{Cursor, El, Tx, col, el, row, text};

use crate::KitSize;
use crate::icons::{Glyph, glyph};
use crate::input::{fade, input_background, input_text};
use crate::motion;
use crate::overlay::Acts;
use crate::theme::{Theme, radius, rems, ty};

/// gpui-kit's `Checkbox`: a 1pt-bordered box (radius 4; 10.5 / 12.25 / 14 / 15.75 square from
/// XSmall to Large) filled `primary` with a `primary_foreground` check when checked, and an
/// optional label 7 after it. Controlled: [`Self::on_change`] gets the requested value.
pub struct Checkbox {
    key: String,
    label: Option<String>,
    children: Vec<El>,
    checked: bool,
    disabled: bool,
    size: KitSize,
    tab_stop: bool,
    pointer: bool,
    tooltip: Option<String>,
    acts: Acts,
}

impl Checkbox {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: None,
            children: Vec::new(),
            checked: false,
            disabled: false,
            size: KitSize::Medium,
            tab_stop: true,
            pointer: false,
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

    /// Content under the label (gpui-kit's `ParentElement` children).
    pub fn child(mut self, e: El) -> Self {
        self.children.push(e);
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

    /// `tab_stop(false)`: out of the Tab order.
    pub fn tab_stop(mut self, on: bool) -> Self {
        self.tab_stop = on;
        self
    }

    /// The pointing-hand cursor (`cursor_pointer()`).
    pub fn pointer(mut self) -> Self {
        self.pointer = true;
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
        let side = rems(match self.size {
            KitSize::XSmall => 0.75,
            KitSize::Small => 0.875,
            KitSize::Large => 1.125,
            _ => 1.0,
        });
        let mark = rems(match self.size {
            KitSize::XSmall => 0.5,
            KitSize::Small => 0.625,
            KitSize::Large => 0.875,
            _ => 0.75,
        });
        let has_content = self.label.is_some() || !self.children.is_empty();
        let (fill, border) = match (checked, disabled) {
            (true, false) => (t.kit.primary, t.kit.primary),
            (true, true) => (fade(t.kit.primary, 0.5), fade(t.kit.primary, 0.5)),
            (false, false) => (input_background(t), t.kit.input),
            (false, true) => (input_background(t), fade(t.kit.input, 0.5)),
        };
        let ink = if disabled {
            fade(t.kit.primary_foreground, 0.5)
        } else {
            t.kit.primary_foreground
        };
        // The mark sits 1pt inside the 1pt border (`absolute().top_px().left_px()`), fading on
        // the control motion as gpui-kit springs its opacity.
        let indicator = el()
            .key(format!("{key}-box"))
            .size(side, side)
            .mt(if has_content { side * 0.125 } else { 0.0 })
            .radius(radius::KIT.min(4.0))
            .fill(fill)
            .border(1.0, border)
            .motion(motion::CONTROL.spring())
            .child(
                glyph(Glyph::Check, mark)
                    .key(format!("{key}-mark"))
                    .absolute(2.0, 2.0)
                    .color(ink)
                    .opacity(if checked { 1.0 } else { 0.0 })
                    .motion(motion::CONTROL.spring()),
            )
            .a11y_hidden();
        let size = input_text(self.size);
        let mut root = row()
            .key(key.clone())
            .role("checkbox")
            .checked(checked)
            .gap(rems(0.5))
            .items_start()
            .radius(radius::KIT * 0.5)
            .child(indicator);
        if let Some(l) = &self.label {
            root = root.label(l.clone());
        }
        if has_content {
            let mut content = col()
                .key(format!("{key}-content"))
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(rems(0.25));
            if let Some(l) = self.label {
                let color = if disabled {
                    t.kit.muted_foreground
                } else {
                    t.kit.foreground
                };
                content = content.child(
                    text(l)
                        .font(ty::ui(size).lh(size * 1.25))
                        .color(color)
                        .a11y_hidden(),
                );
            }
            for c in self.children {
                content = content.child(c);
            }
            root = root.child(content);
        }
        let interactive = !disabled && !self.acts.is_empty();
        root = root
            .focusable(self.tab_stop && !disabled)
            .disabled(disabled);
        if self.pointer {
            root = root.cursor(Cursor::Pointer);
        }
        if disabled {
            root = root.cursor(Cursor::Default);
        }
        if let Some(tip) = self.tooltip {
            root = crate::tooltip::tooltip(root, tip);
        }
        self.acts.apply(root, interactive)
    }
}

/// Roostr's `ui::checkbox`: the kit checkbox at its largest size, primary (accent) fill when
/// checked, pointing-hand cursor. Chain `.on_change(..)`, `.tooltip(..)`, `.disabled(..)`, or a
/// smaller size for denser rows.
pub fn checkbox(key: impl Into<String>, checked: bool) -> Checkbox {
    Checkbox::new(key).checked(checked).large().pointer()
}

/// Roostr's `ui::checkbox_display`: a [`checkbox`] that only shows a value: full colour, no
/// handler, out of the Tab order; presses fall through to the row around it.
pub fn checkbox_display(key: impl Into<String>, checked: bool) -> Checkbox {
    checkbox(key, checked).tab_stop(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime};

    #[derive(Default)]
    struct S {
        done: bool,
        row_presses: usize,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let t = Theme::dark();
        col()
            .padding(20.0)
            .gap(20.0)
            .child(
                checkbox("done", s.done)
                    .label("Done")
                    .on_change(|s: &mut S, v| s.done = v)
                    .build(t),
            )
            .child(
                Checkbox::new("off")
                    .label("Disabled")
                    .disabled(true)
                    .on_change(|s: &mut S, v| s.done = v)
                    .build(t),
            )
            .child(
                row()
                    .key("row")
                    .padding(8.0)
                    .child(checkbox_display("shown", s.done).build(t))
                    .on_press(|s: &mut S| s.row_presses += 1),
            )
    }

    fn click(rt: &mut Runtime<S>, k: &str) {
        let (x, y) = rt.find(k).unwrap_or_else(|| panic!("{k}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn checkbox_toggles_and_display_passes_presses_through() {
        let mut rt = Runtime::new(
            S::default(),
            view,
            (400.0, 300.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        click(&mut rt, "done");
        assert!(rt.state.done);
        click(&mut rt, "done-box");
        assert!(!rt.state.done);
        click(&mut rt, "off");
        assert!(!rt.state.done);
        click(&mut rt, "shown");
        assert_eq!(rt.state.row_presses, 1);
        assert!(!rt.state.done);
    }
}

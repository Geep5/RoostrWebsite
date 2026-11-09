//! gpui-kit's `Popover`: a surface anchored to its trigger, in the kit's popup look (popover
//! fill, no border, radius 6, the hairline ring and `shadow-md`, padding 0.75 rem), placed
//! 0.5 rem from the trigger (the kit's 0.25 rem offset plus its content's `top_1`).
//!
//! Whether it is open is the app's; [`Popover::on_close`] runs on Escape and, unless
//! `overlay_closable(false)`, on a press outside. Focus stays inside while it is open and
//! returns to the trigger when it closes.

use std::rc::Rc;

use rune::{El, col, row};

use crate::overlay::{self, Placement};
use crate::theme::{Theme, radius, rems, shadow};

/// `Popover::new("share").trigger(button).child(body).open(s.open).on_close(|s: &mut S| s.open = false).build(t, &mut layer)`.
pub struct Popover {
    key: String,
    trigger: Option<El>,
    body: Vec<El>,
    open: bool,
    placement: Placement,
    gap: f32,
    appearance: bool,
    overlay_closable: bool,
    close: Option<Box<dyn FnOnce(El, &str, bool, &mut Vec<El>) -> El>>,
}

impl Popover {
    pub fn new(key: impl Into<String>) -> Popover {
        Popover {
            key: key.into(),
            trigger: None,
            body: Vec::new(),
            open: false,
            // The kit's default anchor (`TopLeft`): the popover's top-left under the trigger's.
            placement: Placement::BottomStart,
            gap: rems(0.5),
            appearance: true,
            overlay_closable: true,
            close: None,
        }
    }
    pub fn trigger(mut self, t: El) -> Self {
        self.trigger = Some(t);
        self
    }
    pub fn child(mut self, c: El) -> Self {
        self.body.push(c);
        self
    }
    pub fn children(mut self, c: impl IntoIterator<Item = El>) -> Self {
        self.body.extend(c);
        self
    }
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
    /// Which side of the trigger it opens on (gpui-kit's `anchor`).
    pub fn placement(mut self, p: Placement) -> Self {
        self.placement = p;
        self
    }
    /// The distance from the trigger (gpui-kit's `offset`, plus the content's 0.25 rem).
    pub fn offset(mut self, offset: f32) -> Self {
        self.gap = offset + rems(0.25);
        self
    }
    /// `false`: no fill, ring, shadow or padding; the content draws itself.
    pub fn appearance(mut self, on: bool) -> Self {
        self.appearance = on;
        self
    }
    /// Whether a press outside closes it (Escape always does).
    pub fn overlay_closable(mut self, on: bool) -> Self {
        self.overlay_closable = on;
        self
    }
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        let f = Rc::new(f);
        self.close = Some(Box::new(
            move |surface: El, key: &str, outside: bool, layer: &mut Vec<El>| {
                if outside {
                    overlay::dismissable(surface, key, move |s: &mut S| f(s), layer)
                } else {
                    surface.modal(false, move |s: &mut S| f(s))
                }
            },
        ));
        self
    }

    pub fn build(self, t: &Theme, layer: &mut Vec<El>) -> El {
        let anchor = format!("{}-trigger", self.key);
        let trigger = row()
            .key(anchor.clone())
            .children(self.trigger.map(|tr| tr.expanded(self.open)));
        if !self.open {
            return trigger;
        }
        let mut surface = col()
            .key(format!("{}-content", self.key))
            .role("dialog")
            .children(self.body);
        if self.appearance {
            surface = popover_surface(t, surface);
        }
        surface = surface.focus_return(anchor.clone());
        if let Some(close) = self.close {
            surface = close(surface, &self.key, self.overlay_closable, layer);
        }
        layer.push(overlay::anchored(
            surface,
            &anchor,
            self.placement,
            self.gap,
        ));
        trigger
    }
}

/// gpui-kit's `popover_style` plus the Popover's `p_3` on `e`: popover fill, radius 6, the kit
/// popup ring and shadow, padding 0.75 rem. Also the inline (static) look of an open popover.
pub fn popover_surface(t: &Theme, e: El) -> El {
    let mut e = e
        .fill(t.kit.popover)
        .radius(radius::KIT)
        .padding(rems(0.75));
    for s in shadow::kit_popover(t) {
        e = e.box_shadow(s);
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime, el, text};

    #[derive(Default)]
    struct S {
        open: bool,
    }

    fn view(s: &S, _: &mut rune::Cx<S>) -> El {
        let t = Theme::dark();
        let mut layer = Vec::new();
        let p = Popover::new("p")
            .trigger(
                el().key("btn")
                    .w(80.0)
                    .h(28.0)
                    .on_press(|s: &mut S| s.open = true),
            )
            .child(text("Hello").key("hello"))
            .open(s.open)
            .on_close(|s: &mut S| s.open = false)
            .build(t, &mut layer);
        crate::root(col().w_full().h_full().padding(20.0).child(p), layer)
    }

    #[test]
    fn outside_press_closes() {
        let mut rt = Runtime::new(
            S::default(),
            view,
            (600.0, 400.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        let (x, y) = rt.find("btn").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.open);
        // A press inside keeps it.
        rt.frame(1.0);
        let (x, y) = rt.find("hello").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.open);
        rt.pointer_down(500.0, 350.0);
        rt.pointer_up(500.0, 350.0);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.open);
    }
}

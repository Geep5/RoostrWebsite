//! A property chooser for filter and sort rules (`KeyPicker.svelte`): a
//! button showing the current property; opening it gives a search box over
//! the list.

use std::rc::Rc;

use gpui_kit::component::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::theme;

use crate::widgets::{FieldSkin, below, field, menu_shadow};

type LabelFn = Rc<dyn Fn(&str, &App) -> SharedString>;
type PickFn = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// Button + searchable property menu (web KeyPicker.svelte).
#[derive(IntoElement)]
pub struct KeyPicker {
    id: ElementId,
    value: String,
    keys: Vec<String>,
    label: LabelFn,
    on_pick: Option<PickFn>,
}

impl KeyPicker {
    /// `label` says how a key reads in the list and on the button (emoji + name).
    pub fn new(id: impl Into<ElementId>, value: String, keys: Vec<String>, label: LabelFn) -> Self {
        Self {
            id: id.into(),
            value,
            keys,
            label,
            on_pick: None,
        }
    }

    /// Called with a key other than the current one.
    pub fn on_pick(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }
}

struct KeyPickerState {
    open: bool,
    query: Entity<InputState>,
    active: usize,
    value: String,
    keys: Vec<String>,
    label: Option<LabelFn>,
    on_pick: Option<PickFn>,
    _sub: Subscription,
}

impl KeyPickerState {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search properties"));
        let sub = cx.subscribe_in(
            &query,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    this.active = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    if let Some(k) = this.matches(cx).get(this.active).cloned() {
                        this.pick(&k, window, cx);
                    }
                }
                _ => {}
            },
        );
        Self {
            open: false,
            query,
            active: 0,
            value: String::new(),
            keys: vec![],
            label: None,
            on_pick: None,
            _sub: sub,
        }
    }

    fn label(&self, key: &str, cx: &App) -> SharedString {
        self.label
            .as_ref()
            .map_or_else(|| key.to_string().into(), |f| f(key, cx))
    }

    fn matches(&self, cx: &App) -> Vec<String> {
        let q = self.query.read(cx).value().trim().to_lowercase();
        if q.is_empty() {
            return self.keys.clone();
        }
        self.keys
            .iter()
            .filter(|k| {
                self.label(k, cx).to_lowercase().contains(&q) || k.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        self.query.update(cx, |s, cx| s.set_value("", window, cx));
        self.active = self.keys.iter().position(|k| *k == self.value).unwrap_or(0);
        if self.open {
            self.query.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    fn pick(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
        if key != self.value
            && let Some(f) = self.on_pick.clone()
        {
            f(key, window, cx);
        }
    }
}

impl Render for KeyPickerState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.label(&self.value, cx);
        let mut root = div().relative().flex().child(
            div()
                .id("current")
                .flex()
                .items_center()
                .gap(px(4.))
                .bg(theme::panel())
                .border_1()
                .border_color(if self.open {
                    theme::accent()
                } else {
                    theme::border()
                })
                .rounded(theme::radius::sm())
                .px(px(8.))
                .py(px(4.))
                .text_size(px(13.))
                .text_color(theme::fg())
                .whitespace_nowrap()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx)))
                .child(current)
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(theme::muted())
                        .child("▾"),
                ),
        );
        if self.open {
            let matches = self.matches(cx);
            let q = self.query.read(cx).value().to_string();
            let mut list = div()
                .id("keys")
                .flex()
                .flex_col()
                .min_h_0()
                .max_h(px(300.))
                .overflow_y_scroll();
            for (i, k) in matches.iter().enumerate() {
                let key = k.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("key-{k}")))
                        .flex_none()
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(theme::radius::sm())
                        .text_size(px(13.))
                        .text_color(if *k == self.value {
                            theme::accent()
                        } else {
                            theme::fg()
                        })
                        .cursor_pointer()
                        .when(i == self.active, |d| d.bg(theme::hover()))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.active != i {
                                this.active = i;
                                cx.notify();
                            }
                        }))
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.pick(&key, window, cx)),
                        )
                        .child(self.label(k, cx)),
                );
            }
            if matches.is_empty() {
                list = list.child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(format!("No property matches “{q}”.")),
                );
            }
            let n = matches.len();
            let menu = div()
                .w(px(240.))
                .flex()
                .flex_col()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::border())
                .rounded(theme::radius::menu())
                .p(px(6.))
                .shadow(menu_shadow())
                .text_color(theme::fg())
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.open = false;
                    cx.notify();
                }))
                .capture_action(cx.listener(move |this, _: &MoveDown, _, cx| {
                    cx.stop_propagation();
                    this.active = (this.active + 1).min(n.saturating_sub(1));
                    cx.notify();
                }))
                .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                    cx.stop_propagation();
                    this.active = this.active.saturating_sub(1);
                    cx.notify();
                }))
                .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                    cx.stop_propagation();
                    this.open = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .mb(px(4.))
                        .child(field(&self.query, FieldSkin::Search, window, cx)),
                )
                .child(list);
            root = root.child(below("key-menu", 4., menu));
        }
        root
    }
}

impl RenderOnce for KeyPicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |window, cx| {
            KeyPickerState::new(window, cx)
        });
        state.update(cx, |s, _| {
            s.value = self.value;
            s.keys = self.keys;
            s.label = Some(self.label);
            s.on_pick = self.on_pick;
        });
        state
    }
}

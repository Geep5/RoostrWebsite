//! Anytype's searchObject menu for "Link to existing page"
//! (`LinkPicker.svelte`), and the "Link URL:" prompt the marks toolbar asks.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::types::is_system_type;
use roostr_core::{Store, theme};

pub enum LinkPickerEvent {
    Picked(String),
    Close,
}

/// A search box over the space's objects; picking one creates the link block.
pub struct LinkPicker {
    exclude: String,
    query: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<LinkPickerEvent> for LinkPicker {}

impl LinkPicker {
    pub fn new(exclude: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search objects…"));
        let subs = vec![
            cx.subscribe(&query, |this: &mut Self, _, ev: &InputEvent, cx| match ev {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => {
                    if let Some(first) = this.candidates(cx).first() {
                        cx.emit(LinkPickerEvent::Picked(first.0.clone()));
                    }
                }
                _ => {}
            }),
        ];
        query.update(cx, |q, cx| q.focus(window, cx));
        Self {
            exclude,
            query,
            _subs: subs,
        }
    }

    /// (id, icon, name, type key), the first 8 matches.
    fn candidates(&self, cx: &App) -> Vec<(String, String, String, String)> {
        let q = self.query.read(cx).value().trim().to_lowercase();
        let store = Store::global(cx).read(cx);
        store
            .summaries
            .iter()
            .filter(|s| s.id != self.exclude && !is_system_type(&s.type_key))
            .filter(|s| {
                q.is_empty()
                    || (if s.name.is_empty() {
                        "untitled"
                    } else {
                        &s.name
                    })
                    .to_lowercase()
                    .contains(&q)
            })
            .take(8)
            .map(|s| {
                let icon =
                    roostr_core::objects::object_icon(store, &s.icon, &s.type_key, &s.channel_id);
                (
                    s.id.clone(),
                    icon,
                    if s.name.is_empty() {
                        "Untitled".into()
                    } else {
                        s.name.clone()
                    },
                    s.type_key.clone(),
                )
            })
            .collect()
    }
}

impl Render for LinkPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.candidates(cx);
        let empty = rows.is_empty();
        let entity = cx.entity();
        div()
            .id("link-picker")
            .occlude()
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.))
            .min_w(px(280.))
            .max_w(px(340.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(px(10.))
            .p(px(8.))
            .shadow(roostr_core::ui::menu_shadow())
            .capture_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.emit(LinkPickerEvent::Close);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .mb(px(4.))
                    .rounded(px(10.))
                    .bg(theme::hl_med())
                    .px(px(8.))
                    .child(Input::new(&self.query).appearance(false).text_size(px(13.))),
            )
            .children(
                rows.into_iter()
                    .enumerate()
                    .map(|(i, (id, icon, name, tk))| {
                        div()
                            .id(("link-row", i))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .rounded(px(6.))
                            .px(px(6.))
                            .py(px(5.))
                            .cursor_pointer()
                            .text_size(px(13.))
                            .text_color(theme::fg())
                            .hover(|s| s.bg(theme::hl_med()))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(LinkPickerEvent::Picked(id.clone()))
                            }))
                            .child(roostr_core::ui::emoji_or_image_icon(&icon, px(16.)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::muted())
                                    .child(tk),
                            )
                    }),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .px(px(6.))
                        .py(px(4.))
                        .child("No matching objects."),
                )
            })
            .child(outside_click(entity, |_, cx| {
                cx.emit(LinkPickerEvent::Close)
            }))
    }
}

pub enum LinkPromptEvent {
    Submit(String),
    Cancel,
}

/// The browser's `prompt("Link URL:")`, as a small popover with one field.
pub struct LinkPrompt {
    input: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<LinkPromptEvent> for LinkPrompt {}

impl LinkPrompt {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let subs = vec![
            cx.subscribe(&input, |this: &mut Self, _, ev: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let url = this.input.read(cx).value().trim().to_string();
                    cx.emit(LinkPromptEvent::Submit(url));
                }
            }),
        ];
        input.update(cx, |q, cx| q.focus(window, cx));
        Self { input, _subs: subs }
    }
}

impl Render for LinkPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .id("link-prompt")
            .occlude()
            .relative()
            .w(px(320.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(px(10.))
            .p(px(10.))
            .shadow(roostr_core::ui::menu_shadow())
            .text_size(px(13.))
            .text_color(theme::fg())
            .capture_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.emit(LinkPromptEvent::Cancel);
                    cx.stop_propagation();
                }
            }))
            .child("Link URL:")
            .child(
                div()
                    .rounded(px(6.))
                    .bg(theme::hl_med())
                    .px(px(8.))
                    .child(Input::new(&self.input).appearance(false).text_size(px(13.))),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        roostr_core::ui::outline_button("link-cancel", "Cancel")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(LinkPromptEvent::Cancel))),
                    )
                    .child(
                        roostr_core::ui::primary_button("link-ok", "OK").on_click(cx.listener(
                            |this, _, _, cx| {
                                let url = this.input.read(cx).value().trim().to_string();
                                cx.emit(LinkPromptEvent::Submit(url));
                            },
                        )),
                    ),
            )
            .child(outside_click(entity, |_, cx| {
                cx.emit(LinkPromptEvent::Cancel)
            }))
    }
}

/// A zero-size element that runs `close` when the mouse goes down outside
/// its parent's bounds (the web's window mousedown check).
pub(crate) fn outside_click<T: 'static>(
    entity: Entity<T>,
    close: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> impl IntoElement {
    let close = std::rc::Rc::new(close);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let parent = bounds;
            let entity = entity.clone();
            let close = close.clone();
            window.on_mouse_event(move |ev: &MouseDownEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture && !parent.contains(&ev.position) {
                    let close = close.clone();
                    entity.update(cx, |this, cx| close(this, cx));
                }
            });
        },
    )
    .absolute()
    .inset_0()
}

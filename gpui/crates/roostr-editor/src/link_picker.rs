//! Anytype's searchObject menu for "Link to existing page"
//! (`LinkPicker.svelte`), and the "Link URL:" prompt the marks toolbar asks.
//! Both are kit popovers held open at a point: an outside click or Escape
//! dismisses them.

use std::rc::Rc;

use gpui_kit::component::Selectable;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::popover::Popover;
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
    query: Entity<CommandState>,
}

impl EventEmitter<LinkPickerEvent> for LinkPicker {}

impl LinkPicker {
    pub fn new(exclude: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| CommandState::new(window, cx));
        query.update(cx, |q, cx| q.focus(window, cx));
        Self { exclude, query }
    }

    /// (id, icon, name, type key), the first 8 matches.
    fn candidates(&self, cx: &App) -> Vec<(String, String, String, String)> {
        let q = self.query.read(cx).query(cx).trim().to_lowercase();
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
        let ids: Rc<Vec<String>> = Rc::new(rows.iter().map(|r| r.0.clone()).collect());
        let this = cx.entity().downgrade();
        let notify = this.clone();
        let pick = this.clone();
        let mut list = Command::new(&self.query)
            // The store answers the query (its first 8 matches), not the list.
            .filterable(false)
            .bordered(false)
            .placeholder("Search objects…")
            .on_query(move |_, _, cx| {
                let _ = notify.update(cx, |_, cx| cx.notify());
            })
            // Enter takes the highlighted row: the first, unless ↑/↓ moved it.
            .on_confirm(move |ix, _, cx| {
                if let Some(id) = ids.get(ix.row).cloned() {
                    let _ = pick.update(cx, |_, cx| cx.emit(LinkPickerEvent::Picked(id)));
                }
            })
            .empty(|_, _, _| {
                div()
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .px(px(6.))
                    .py(px(4.))
                    .child("No matching objects.")
            });
        for (_, icon, name, tk) in rows {
            list = list.item(CommandItem::new().label(name.clone()).child(move |_, _| {
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .text_size(px(13.))
                    .text_color(theme::fg())
                    .child(roostr_core::ui::emoji_or_image_icon(&icon, px(16.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(tk.clone()),
                    )
            }));
        }
        let focus = self.query.focus_handle(cx);
        held_popover("link-picker-popover", &focus, move |_, cx| {
            let _ = this.update(cx, |_, cx| cx.emit(LinkPickerEvent::Close));
        })
        .min_w(px(280.))
        .max_w(px(340.))
        .p(px(4.))
        .child(
            div()
                .id("link-picker")
                .capture_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                    if ev.keystroke.key == "escape" {
                        cx.emit(LinkPickerEvent::Close);
                        cx.stop_propagation();
                    }
                }))
                .child(list),
        )
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
        let this = cx.entity().downgrade();
        let focus = self.input.focus_handle(cx);
        held_popover("link-prompt-popover", &focus, move |_, cx| {
            let _ = this.update(cx, |_, cx| cx.emit(LinkPromptEvent::Cancel));
        })
        .w(px(320.))
        .p(px(10.))
        .child(
            div()
                .id("link-prompt")
                .flex()
                .flex_col()
                .gap(px(8.))
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
                    Input::new(&self.input)
                        .bordered(false)
                        .bg(theme::hl_med())
                        .rounded(px(6.))
                        .px(px(8.))
                        .text_size(px(13.)),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            roostr_core::ui::outline_button("link-cancel", "Cancel").on_click(
                                cx.listener(|_, _, _, cx| cx.emit(LinkPromptEvent::Cancel)),
                            ),
                        )
                        .child(roostr_core::ui::primary_button("link-ok", "OK").on_click(
                            cx.listener(|this, _, _, cx| {
                                let url = this.input.read(cx).value().trim().to_string();
                                cx.emit(LinkPromptEvent::Submit(url));
                            }),
                        )),
                ),
        )
    }
}

/// A kit popover held open (it opens with its owner and closes with it) at
/// the point a zero-size trigger marks: the caller places that point.
/// `on_close` runs when the popover dismisses itself on an outside click.
fn held_popover(
    id: &'static str,
    focus: &FocusHandle,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
) -> Popover {
    Popover::new(id)
        .trigger(HeldAt(true))
        .open(true)
        .anchor(Anchor::TopLeft)
        .offset(px(0.))
        // Opening focuses the field, not the surface.
        .track_focus(focus)
        .on_open_change(move |open, window, cx| {
            if !*open {
                on_close(window, cx);
            }
        })
}

/// The point a held popover hangs from.
#[derive(IntoElement)]
struct HeldAt(bool);

impl Selectable for HeldAt {
    fn selected(self, selected: bool) -> Self {
        Self(selected)
    }

    fn is_selected(&self) -> bool {
        self.0
    }
}

impl RenderOnce for HeldAt {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().size_0()
    }
}

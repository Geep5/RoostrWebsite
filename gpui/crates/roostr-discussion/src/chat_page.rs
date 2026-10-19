//! `/app/chat/:id` (`routes/app/chat/[id]/+page.svelte`): the full-screen
//! discussion - header with back, object icon + name and comment count,
//! messages filling the window, composer pinned to the bottom.

use gpui_kit::component::IconNamed as _;
use gpui_kit::*;
use roostr_core::nav::{self, Route};
use roostr_core::objects::object_icon;
use roostr_core::{Glyph, Navigator, Object, Store, theme};

use crate::chat::{DISCUSSION, chat_blocks};
use crate::discussion::{Discussion, DiscussionOptions};

pub struct ChatPage {
    object_id: String,
    discussion: Entity<Discussion>,
    _store: Subscription,
}

impl ChatPage {
    /// The full-screen chat of `object_id`.
    pub fn new(object_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        store.update(cx, |s, cx| s.ensure_object(&object_id, cx));
        let id = object_id.clone();
        let discussion = cx.new(|cx| {
            Discussion::new(
                id,
                DiscussionOptions {
                    full: true,
                    page_mode: true,
                    thread_id: DISCUSSION.into(),
                    embedded: false,
                },
                window,
                cx,
            )
        });
        let subscription = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            object_id,
            discussion,
            _store: subscription,
        }
    }
}

/// Chat lines directly under the discussion root (the header's count).
fn comment_count(object: &Object) -> usize {
    chat_blocks(object, DISCUSSION).len()
}

impl Render for ChatPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx);
        let object = store.read(cx).object(&self.object_id);
        let mut shell = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .text_color(theme::fg())
            .overflow_hidden();
        let Some(object) = object else { return shell };
        let space = roostr_core::nav::Navigator::global(cx)
            .read(cx)
            .space()
            .to_string();
        let icon = object_icon(
            store.read(cx),
            object.field_str("iconEmoji"),
            &object.type_key,
            &space,
        );
        let name = if object.name().is_empty() {
            "Untitled".to_string()
        } else {
            object.name().to_string()
        };
        let comments = comment_count(&object);
        let back_target = object.id.clone();
        // A chat object opens here in its own page's place on a phone, so
        // that page would only send it straight back: go back past it.
        let back_in_history = object.type_key == "chat" && roostr_core::layout::is_mobile(window);
        let icon_el = if roostr_core::objects::is_image_icon(&icon) {
            div().child(roostr_core::ui::emoji_or_image_icon(&icon, px(15.)))
        } else {
            div().text_size(px(15.)).child(icon)
        };
        shell = shell
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pt(px(10.))
                    .px(px(12.))
                    .pb(px(8.))
                    .child(
                        div()
                            .id("back")
                            .flex_none()
                            .size(px(40.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(theme::panel())
                            .text_color(theme::fg())
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                if back_in_history {
                                    Navigator::global(cx).update(cx, |nav, cx| {
                                        if nav.can_go_back() {
                                            nav.back(cx);
                                        } else {
                                            nav.navigate(Route::Home, cx);
                                        }
                                    });
                                } else {
                                    nav::navigate(Route::Object(back_target.clone()), cx)
                                }
                            })
                            .child(
                                svg()
                                    .path(Glyph::MobileBack.path())
                                    .size(px(20.))
                                    .text_color(theme::fg()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .max_w_full()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(icon_el)
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(name),
                                    ),
                            )
                            .child(div().text_size(px(12.)).text_color(theme::muted()).child(
                                format!(
                                    "{comments} comment{}",
                                    if comments == 1 { "" } else { "s" }
                                ),
                            )),
                    )
                    .child(div().flex_none().w(px(40.))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .px(px(12.))
                    .pb(px(8.))
                    .child(self.discussion.clone()),
            );
        shell
    }
}

//! `cargo run -p roostr-discussion --example discussion -- [object-id] [drawer|page|opener] [thread-id]`
//!
//! - `drawer` (default): the object's discussion as a chat object's page shows
//!   it (full, natural height in a scrolling page) beside the Chat and
//!   Properties columns at their widths.
//! - `page`: the full-screen `/app/chat/:id` page.
//! - `opener`: the collapsed "N comments" opener in a page, beside the columns.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::*;
use roostr_core::{PaneState, PaneTab, theme};
use roostr_discussion::{ChatPage, ConversationDrawer, Discussion, DiscussionOptions, pane_counts};

const DEFAULT_OBJECT: &str = "seed-page-launch-plan";

struct Frame {
    object_id: String,
    page: AnyView,
    drawer: Entity<ConversationDrawer>,
}

impl Render for Frame {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = PaneState::global(cx);
        let store = roostr_core::Store::global(cx);
        if let Some(object) = store.read(cx).object(&self.object_id) {
            let (count, conversations) = pane_counts(&object);
            pane.update(cx, |p, cx| p.publish(true, true, count, conversations, cx));
        }
        let views = ConversationDrawer::views(&self.drawer, cx);
        let (count, conversations) = {
            let p = pane.read(cx);
            (p.count, p.conv_count)
        };
        let columns = [PaneTab::Chat, PaneTab::Properties].map(|tab| {
            let (open, width) = {
                let p = pane.read(cx);
                (p.is_open(tab), p.width(tab))
            };
            match views.column(tab) {
                Some(view) if open => div()
                    .flex_none()
                    .w(px(width))
                    .h_full()
                    .ml(px(6.))
                    .rounded(theme::radius::page())
                    .bg(theme::bg())
                    .child(view),
                _ => div().flex_none().p(px(8.)).child(
                    Button::new(tab.name())
                        .outline()
                        .small()
                        .rounded(px(7.))
                        .text_color(theme::muted())
                        .label(format!("‹ {}", tab.name()))
                        .on_click(move |_, _, cx| {
                            PaneState::global(cx).update(cx, |p, cx| p.set_open(tab, true, cx))
                        }),
                ),
            }
        });
        div()
            .size_full()
            .flex()
            .bg(theme::gutter())
            .child(
                div()
                    .id("page")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(theme::bg())
                    .overflow_y_scroll()
                    .pt(px(24.))
                    .pb(px(80.))
                    .px(px(32.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(format!(
                                "pane_counts: {count} message(s), {conversations} conversation(s)"
                            )),
                    )
                    .child(self.page.clone()),
            )
            .children(columns)
    }
}

fn main() {
    roostr_core::dev::run_example("Discussion", |args, window, cx| {
        roostr_properties::init(cx);
        roostr_discussion::init(cx);
        let object_id = args
            .first()
            .cloned()
            .unwrap_or_else(|| DEFAULT_OBJECT.into());
        let mode = args.get(1).cloned().unwrap_or_else(|| "drawer".into());
        if mode == "page" {
            let id = object_id.clone();
            return cx.new(|cx| ChatPage::new(id, window, cx)).into();
        }
        let full = mode != "opener";
        let id = object_id.clone();
        // An optional third argument opens that conversation thread instead of the discussion.
        let thread_id = args
            .get(2)
            .cloned()
            .unwrap_or_else(|| DiscussionOptions::default().thread_id);
        let page: AnyView = cx
            .new(|cx| {
                Discussion::new(
                    id,
                    DiscussionOptions {
                        full,
                        page_mode: false,
                        thread_id,
                        embedded: false,
                    },
                    window,
                    cx,
                )
            })
            .into();
        let id = object_id.clone();
        let drawer = cx.new(|cx| ConversationDrawer::new(id, true, window, cx));
        cx.new(|cx| {
            cx.observe(&PaneState::global(cx), |_, _, cx| cx.notify())
                .detach();
            cx.observe(&roostr_core::Store::global(cx), |_, _, cx| cx.notify())
                .detach();
            Frame {
                object_id,
                page,
                drawer,
            }
        })
        .into()
    });
}

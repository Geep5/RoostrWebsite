//! `cargo run -p roostr-discussion --example discussion -- [object-id] [drawer|page|opener] [thread-id]`
//!
//! - `drawer` (default): the object's discussion as a chat object's page shows it (full,
//!   natural height in a scrolling page) beside the Chat and Properties columns at their
//!   widths.
//! - `page`: the full-screen `/app/chat/:id` page.
//! - `opener`: the collapsed "N comments" opener in a page, beside the columns.
//!
//! Opens on the daemon named by the environment; `--snap out.png --size WxH` draws it
//! headless once the store has loaded.

use roostr_core::nav::Route;
use roostr_core::{PaneTab, State};
use roostr_discussion::{DiscussionOptions, chat_page, discussion, pane_column, pane_counts};
use roostr_ui::{Button, KitSize, ThemePref, radius, ty};
use rune::{Cx, El, ScrollSpec, col, row, text};

const DEFAULT_OBJECT: &str = "seed-page-launch-plan";

fn args() -> (String, String, Option<String>) {
    let args = rune::extra_args();
    (
        args.first()
            .cloned()
            .unwrap_or_else(|| DEFAULT_OBJECT.into()),
        args.get(1).cloned().unwrap_or_else(|| "drawer".into()),
        args.get(2).cloned(),
    )
}

/// The object page's part: publish the pane counts as the page does.
fn publish(s: &mut State) -> bool {
    let (object_id, _, _) = args();
    let Some(object) = s.store.object(&object_id) else {
        return false;
    };
    let (count, conversations) = pane_counts(&object);
    let p = &s.pane;
    if p.available && p.chat && p.count == count && p.conv_count == conversations {
        return false;
    }
    s.pane.publish(true, true, count, conversations);
    true
}

fn register(s: &mut State) {
    roostr_discussion::register(s);
    s.on_poll(publish);
    let (object_id, mode, thread) = args();
    // The object's page is the one showing, as when the app opens it, with both columns.
    let route = if mode == "page" {
        Route::Chat(object_id.clone())
    } else {
        Route::Object(object_id.clone())
    };
    s.nav.navigate(route, &mut s.ui);
    for tab in [PaneTab::Chat, PaneTab::Properties] {
        let pane = &mut s.pane;
        pane.set_open(tab, true, &mut s.ui);
    }
    // Start the loads now, so a `--snap` shows them.
    if mode == "page" {
        roostr_discussion::open_discussion(
            s,
            &object_id,
            &DiscussionOptions {
                full: true,
                page_mode: true,
                ..DiscussionOptions::default()
            },
        );
    } else {
        roostr_discussion::open_discussion(s, &object_id, &page_options(&mode, thread));
        roostr_discussion::open_pane(s, &object_id, true);
    }
}

fn page_options(mode: &str, thread: Option<String>) -> DiscussionOptions {
    DiscussionOptions {
        full: mode != "opener",
        thread_id: thread.unwrap_or_else(|| DiscussionOptions::default().thread_id),
        ..DiscussionOptions::default()
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut layer = Vec::new();
    let window = cx.layout_size("kit-root").unwrap_or([1280.0, 860.0]);
    let (object_id, mode, thread) = args();
    if mode == "page" {
        let page = chat_page(s, cx, t, &mut layer, &object_id, window);
        return roostr_ui::root(page, layer);
    }
    let options = page_options(&mode, thread);
    let p = &s.pane;
    let (count, conversations) = (p.count, p.conv_count);
    let page = col()
        .key("page")
        .grow(1.)
        .min_w(0.)
        .h_full()
        .fill(t.bg)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(
            col()
                .w_full()
                .pt(24.)
                .pb(80.)
                .px(32.)
                .child(
                    text(format!(
                        "pane_counts: {count} message(s), {conversations} conversation(s)"
                    ))
                    .font(ty::ui(11.))
                    .color(t.muted),
                )
                .child(discussion(
                    s, cx, t, &mut layer, &object_id, &options, window,
                )),
        );
    let mut frame = row().w_full().h_full().fill(t.gutter).child(page);
    for tab in [PaneTab::Chat, PaneTab::Properties] {
        frame = frame.child(if p.is_open(tab) {
            col()
                .w(p.width(tab))
                .h_full()
                .ml(6.)
                .radius(radius::PAGE)
                .fill(t.bg)
                .clip_rounded(radius::PAGE)
                .child(pane_column(
                    s, cx, t, &mut layer, &object_id, true, tab, window,
                ))
        } else {
            col().padding(8.).child(
                Button::new(tab.name())
                    .outline()
                    .size(KitSize::Small)
                    .radius(7.)
                    .text_color(t.muted)
                    .label(format!("‹ {}", tab.name()))
                    .on_press(move |s: &mut State| {
                        let pane = &mut s.pane;
                        pane.set_open(tab, true, &mut s.ui);
                    })
                    .build(t),
            )
        });
    }
    roostr_ui::root(frame, layer)
}

fn main() {
    roostr_host::run_example("Discussion", register, view);
}

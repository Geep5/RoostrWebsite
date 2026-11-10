//! Object discussions, chat page, agent exchanges and the chat & properties drawer.
//!
//! Ports `Discussion.svelte`, `ConversationDrawer.svelte`, the `/app/chat/:id` page and
//! `lib/{chat,threads,conversations,drafts}.ts`. Views are functions over the app
//! [`State`]; each shown discussion and pane keeps its state in this crate's slots, made by
//! the view that first needs it (see `local://roostr-discussion-api.md`).

mod attachments;
mod chat;
mod chat_page;
mod discussion;
mod discussion_view;
mod drafts;
mod drawer;
mod live;
mod md;
#[cfg(test)]
mod tests;
mod threads;

use std::collections::HashSet;

use roostr_core::{NavEvent, Object, State};

pub use chat_page::chat_page;
pub use discussion::DiscussionOptions;
pub use discussion_view::discussion;
pub use drawer::{on_escape, pane_column, pane_sheet};

/// Register the crate (replaces GPUI's `init`): discussions follow the store, the panes
/// follow the page and its columns, and instances go with their object's last tab. The
/// discussion's keys (Enter sends, Shift+Enter breaks the line, ↑/↓/Enter/Tab/Escape drive
/// the @ and ! menus, Escape cancels a reply) live on the composer itself.
pub fn register(s: &mut State) {
    s.on_poll(poll);
    s.on_store(discussion::on_store);
    s.on_nav(on_nav);
}

fn poll(s: &mut State) -> bool {
    let panes = drawer::poll(s);
    discussion::poll_all(s) | panes
}

/// A discussion stays while its object is in some tab.
fn on_nav(s: &mut State, event: &NavEvent) {
    if *event != NavEvent::Navigated {
        return;
    }
    let shown: HashSet<String> = s
        .nav
        .tabs()
        .iter()
        .filter_map(|t| t.route.object_id().map(str::to_string))
        .chain(s.nav.route().object_id().map(str::to_string))
        .collect();
    discussion::retain_objects(s, &shown);
}

/// Makes `object_id`'s discussion shown with `options` now, rather than on its view's first
/// frame (a host or test that wants its loads started early). Nothing when it exists.
pub fn open_discussion(s: &mut State, object_id: &str, options: &DiscussionOptions) {
    let key = discussion::instance_key(object_id, discussion::Place::of(options));
    if discussion::get(s, &key).is_none() {
        discussion::create(s, key, object_id.to_string(), options.clone());
    }
}

/// Makes `object_id`'s pane (and, with `chat`, its chat) now rather than on first draw.
pub fn open_pane(s: &mut State, object_id: &str, chat: bool) {
    drawer::open(s, object_id, chat);
    if chat {
        open_discussion(
            s,
            object_id,
            &discussion_view::place_options(discussion::Place::Pane),
        );
    }
}

/// Drops every discussion instance and the pane of `object_id` now (navigation does it for
/// objects in no tab). Drafts are kept.
pub fn forget(s: &mut State, object_id: &str) {
    discussion::forget_object(s, object_id);
    drawer::forget(s, object_id);
}

/// (discussion message count, conversation count) for [`roostr_core::PaneState::publish`],
/// by the object page's rules: the count is the chat lines under `__discussion__`; the
/// conversation count is the exchange threads plus the discussion itself, or 0 when the
/// object has no discussion.
pub fn pane_counts(object: &Object) -> (usize, usize) {
    let count = chat::chat_blocks(object, chat::DISCUSSION).len();
    let t = object.type_key.as_str();
    // An agent's page is its own transcript; queries and collections get a chat like any page.
    let has_discussion = object.mailbox.as_ref().is_some_and(|m| !m.is_empty())
        || !matches!(t, "chat" | "type" | "template" | "relation");
    let conversations = if has_discussion {
        threads::object_thread_count(object) + 1
    } else {
        0
    };
    (count, conversations)
}

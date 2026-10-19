//! Object discussions, chat page, agent exchanges and the chat & properties drawer.
//!
//! Ports `Discussion.svelte`, `ConversationDrawer.svelte`, the
//! `/app/chat/:id` page and `lib/{chat,threads,conversations,drafts}.ts`.

mod chat;
mod chat_page;
mod discussion;
mod drafts;
mod drawer;
mod md;
mod threads;

use gpui_kit::App;
use roostr_core::Object;

pub use chat_page::ChatPage;
pub use discussion::{Discussion, DiscussionOptions};
pub use drawer::ConversationDrawer;

/// Register the crate. The discussion's keys (Enter sends, Shift+Enter breaks
/// the line, ↑/↓/Enter/Tab/Escape drive the @ and ! menus, Escape cancels a
/// reply) live on the composer itself, so there are no global bindings.
pub fn init(_cx: &mut App) {}

/// (discussion message count, conversation count) for [`roostr_core::PaneState`],
/// by the object page's rules: the count is the chat lines under
/// `__discussion__`; the conversation count is the exchange threads plus the
/// discussion itself, or 0 when the object has no discussion.
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

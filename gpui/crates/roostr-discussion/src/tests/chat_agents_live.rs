//! An agent added to an object's Agent property is in its open chat at once: the composer
//! addresses it and the @-menu offers it, with no reload. The chat used to keep the agents
//! it loaded when it opened, so a newly added agent was untaggable until the page was
//! refreshed.

use std::time::Duration;

use roostr_core::nav::Route;

use super::{Harness, Vault, object, post, with_guests};

#[test]
fn an_agent_added_to_an_object_is_in_its_open_chat_at_once() {
    let vault = Vault::new(
        vec![
            object("agent-1", "agent", "Scout"),
            object("chat-1", "chat", "Standup"),
        ],
        "me",
    );
    let mut h = Harness::new(vault.clone(), (390., 844.));
    // The chat object's full-screen chat; nobody is on it yet.
    h.navigate(Route::Chat("chat-1".into()));

    // Scout is added to the Agent property while the chat is open.
    vault.update("chat-1", |o| {
        *o = with_guests(o.clone(), &["agent-1"]);
    });
    vault.commit("chat-1");
    // The store throttles commits (800 ms) before it refetches.
    h.advance(Duration::from_secs(2));

    // The next line goes to Scout: the composer addresses the new agent.
    h.send("hello");
    assert_eq!(vault.posts(), [post("chat-1", "@Scout hello")]);
}

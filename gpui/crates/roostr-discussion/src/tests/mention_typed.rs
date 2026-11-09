//! `@Agent Name` typed out in full - not picked from the menu - tags the agent like a pick.
//! Field report: "@Grant Email " (a space after the two-word name) showed "No agent matches"
//! and the name stayed plain text.

use roostr_core::nav::Route;

use super::{Harness, Vault, object, post, tagged, with_guests};
use crate::discussion::Place;

#[test]
fn a_typed_out_agent_name_tags_the_agent() {
    let vault = Vault::new(
        vec![
            object("agent-1", "agent", "Grant Email"),
            with_guests(object("chat-1", "chat", "Standup"), &["agent-1"]),
        ],
        "me",
    );
    let mut h = Harness::new(vault.clone(), (390., 844.));
    // The chat page, its composer emptied of the seeded `@Grant Email `.
    h.navigate(Route::Chat("chat-1".into()));
    assert!(
        tagged(&h, Place::Page, "Grant Email"),
        "seeded with the guest"
    );
    h.clear_field();

    h.type_text("@Grant Email");
    assert!(h.has("mention-menu"), "the agent is offered");
    // The space after the full name tags it: no "No agent matches", menu closed.
    h.type_text(" ");
    assert!(!h.has("mention-none"), "no 'No agent matches'");
    assert!(!h.has("mention-menu"), "the menu closes on the tag");
    assert!(
        tagged(&h, Place::Page, "Grant Email"),
        "typed out, it is a tag"
    );
    h.send("hi");
    assert_eq!(vault.posts(), [post("chat-1", "@Grant Email hi")]);

    // In other capitals: tagged too, in the agent's own spelling.
    h.clear_field();
    h.type_text("@grant email ");
    assert!(!h.has("mention-none"), "no 'No agent matches'");
    assert!(!h.has("mention-menu"), "the menu closes on the tag");
    h.send("again");
    assert_eq!(vault.posts()[1..], [post("chat-1", "@Grant Email again")]);
}

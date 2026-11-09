//! An agent's chat opens addressed to it on a phone too: the composer starts as `@Agent `
//! (`seedDefaultAgent`), so a plain message reaches the agent. Field report: the browser and
//! the desktop app did, the iPhone app never did - the composer waited for "the active
//! window" to write the tag. The composer's tag is written by its own state now, whatever
//! window is active.

use roostr_core::nav::Route;

use super::{Harness, Vault, object, post, tagged, with_guests};
use crate::discussion::Place;

#[test]
fn an_agents_chat_on_a_phone_opens_addressed_to_it() {
    let vault = Vault::new(
        vec![
            object("agent-1", "agent", "Scout"),
            with_guests(object("chat-1", "chat", "Standup"), &["agent-1"]),
        ],
        "me",
    );
    // iPhone portrait.
    let mut h = Harness::new(vault.clone(), (390., 844.));

    // The agent's page: its chat opens as the sheet, addressed to the agent.
    h.navigate(Route::Object("agent-1".into()));
    h.update(|s| s.pane.set_sheet(true));
    assert!(h.has("disc-sheet"), "the chat sheet");
    assert!(
        tagged(&h, Place::Pane, "Scout"),
        "the composer starts addressed to Scout"
    );
    h.send("hello");
    assert_eq!(vault.posts(), [post("agent-1", "@Scout hello")]);
    assert!(
        tagged(&h, Place::Pane, "Scout"),
        "the composer is addressed to Scout again, as a tag"
    );
    // Leaving and opening the chat again keeps that draft, still a tag (it came back as
    // plain "@Scout" text), with the caret after it.
    h.navigate(Route::Object("chat-1".into()));
    h.navigate(Route::Object("agent-1".into()));
    h.update(|s| s.pane.set_sheet(true));
    assert!(
        tagged(&h, Place::Pane, "Scout"),
        "the kept draft's @Scout is a tag"
    );
    h.send("and again");
    assert_eq!(vault.posts()[1..], [post("agent-1", "@Scout and again")]);

    // A chat object's full-screen chat page, addressed to its only guest agent.
    h.navigate(Route::Chat("chat-1".into()));
    assert!(h.has("chat-page"));
    assert!(!h.has("agent-typing"), "nobody typing yet");
    h.send("standup");
    assert_eq!(vault.posts()[2..], [post("chat-1", "@Scout standup")]);
    // A line to an agent assumes its reply: it shows typing straight away, from this
    // device alone (no harness here reports anything).
    assert!(h.has("agent-typing"), "Scout shows typing");
}

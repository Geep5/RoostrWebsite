//! The object's chat pane in a desktop-width window: its messages scroll, and their text
//! selects and copies. Field report (every platform): the chat would neither scroll nor
//! select, and touching it froze the whole window - the message list's scroll handler read
//! the list while the list held its own state borrowed, so the first wheel or pan panicked.

use roostr_core::PaneTab;
use roostr_core::nav::Route;

use super::{Harness, Vault, chat_block, object, with_discussion};

const MESSAGES: usize = 40;

fn text(n: usize) -> String {
    format!("Message {n} of the launch thread")
}

/// The message whose row is drawn across window height `y`.
fn message_at(h: &Harness, y: f32) -> Option<usize> {
    (1..=MESSAGES).find(|n| {
        h.rect(&format!("msg-m{n}"))
            .is_some_and(|r| r[1] <= y && y < r[1] + r[3])
    })
}

/// Drags across the text of message `n` and returns what is selected (what ⌘C copies).
fn select_message(h: &mut Harness, n: usize) -> String {
    let r = h
        .rect(&format!("msg-text-m{n}"))
        .unwrap_or_else(|| panic!("message {n}'s text is drawn"));
    let y = r[1] + r[3] * 0.5;
    h.rt.pointer_down(r[0] + 13.0, y);
    h.frame();
    h.rt.pointer_move(r[0] + r[2] - 2.0, y);
    h.frame();
    h.rt.pointer_up(r[0] + r[2] - 2.0, y);
    h.settle();
    h.rt.selected_text().unwrap_or_default()
}

#[test]
fn the_chat_pane_scrolls_and_its_text_selects() {
    let messages = (1..=MESSAGES)
        .map(|n| chat_block(&format!("m{n}"), "peer", 1_000 + n as i64, &text(n)))
        .collect();
    let vault = Vault::new(
        vec![with_discussion(
            object("page-1", "page", "Launch plan"),
            messages,
        )],
        "me",
    );
    let mut h = Harness::new(vault, (1280., 800.));
    h.update(|s| {
        let pane = &mut s.pane;
        pane.set_open(PaneTab::Chat, true, &mut s.ui);
    });
    h.navigate(Route::Object("page-1".into()));

    let list = h.rect("chat-messages").expect("the chat pane's messages");
    // Low in the pane, where a peer's bubbles are.
    let probe = list[1] + list[3] * 0.7;
    let before = message_at(&h, probe).expect("a message low in the pane");
    assert!(
        before > MESSAGES - 8,
        "the chat opens at its newest messages: {before}"
    );
    let copied = select_message(&mut h, before);
    assert!(
        copied.contains(&text(before)),
        "a drag over a message selects its text, which ⌘C copies: {copied:?}"
    );

    // A wheel (or a touch pan, which arrives as one) scrolls back through the thread.
    h.rt.wheel(list[0] + 60.0, probe, 600.0);
    h.settle();
    let after = message_at(&h, probe).expect("a message low in the pane");
    assert!(
        after < before,
        "the pane scrolled to older messages: {before} then {after}"
    );
    let copied = select_message(&mut h, after);
    assert!(copied.contains(&text(after)), "{copied:?}");
}

//! The chat's messages never draw on top of each other: each row sits below the one before
//! it, whatever the row holds (a long wrapped message, a reply, your own message on the
//! right) - when the chat opens, after scrolling back, and after the pane narrows so the text
//! re-wraps. Field report: with the message list on gpui-kit's MessageScroller, some bubbles
//! were drawn over others.

use roostr_core::PaneTab;
use roostr_core::nav::Route;
use serde_json::Value as Json;

use super::{Harness, Vault, chat_block, object, with_discussion};

const MESSAGES: usize = 30;
const ME: &str = "me";

/// Message `n`: every few is long enough to wrap over several lines, every third is yours.
fn message(n: usize) -> Json {
    let long = n % 3 == 1;
    let text = if n % 7 == 2 {
        // An agent-style reply: heading, list, bold, inline code, a code block and a table.
        format!(
            "Message {n}\n\n## Status\n\n- **Done:** the importer\n- **Next:** `check_inbox` retries\n- A third point that is long enough to wrap onto a second line in the pane\n\n```\nlet x = 1;\nlet y = 2;\n```\n\n|Inbox|Checked|\n|---|---|\n|grant@|7:02 AM|\n|support@|7:02 AM|"
        )
    } else if n == MESSAGES - 2 {
        // An agent's report taller than the whole chat, with bold and code spans, then short
        // replies after it.
        format!(
            "Message {n}\n\n{}",
            "**What was wrong:** when a history loads, it first updates any old saved fights to the current `format`. In that account's history, one saved fight was missing its list of players.\n\n".repeat(12)
        )
    } else if long {
        format!(
            "Message {n}: {}",
            "a long update that wraps over several lines in the chat pane, ".repeat(4)
        )
    } else {
        format!("Message {n}")
    };
    let author = if n % 3 == 0 { ME } else { "peer" };
    chat_block(&format!("m{n}"), author, 1_000 + n as i64, &text)
}

/// Every message row sits entirely below the one before it, and its text stays inside it
/// (a row measured shorter than its wrapped text lets the text run over the next message),
/// nor out of its sides.
fn assert_no_overlap(h: &Harness, when: &str) {
    let mut rows = Vec::new();
    for n in 1..=MESSAGES {
        let Some(row) = h.rect(&format!("msg-m{n}")) else {
            continue;
        };
        if let Some(text) = h.rect(&format!("msg-text-m{n}")) {
            assert!(
                text[1] + text[3] <= row[1] + row[3] + 0.5,
                "{when}: message {n}'s text (bottom {}) runs out of its row (bottom {})",
                text[1] + text[3],
                row[1] + row[3]
            );
            assert!(
                text[0] >= row[0] - 0.5 && text[0] + text[2] <= row[0] + row[2] + 0.5,
                "{when}: message {n}'s text ({}..{}) runs out of the side of its row ({}..{})",
                text[0],
                text[0] + text[2],
                row[0],
                row[0] + row[2]
            );
        }
        rows.push((n, row));
    }
    assert!(!rows.is_empty(), "{when}: message rows are drawn");
    for pair in rows.windows(2) {
        let ((a, above), (b, below)) = (pair[0], pair[1]);
        assert!(
            above[1] + above[3] <= below[1] + 0.5,
            "{when}: message {a} (bottom {}) runs into message {b} (top {})",
            above[1] + above[3],
            below[1]
        );
    }
}

#[test]
fn chat_messages_never_overlap() {
    let vault = Vault::new(
        vec![with_discussion(
            object("page-1", "page", "Launch plan"),
            (1..=MESSAGES).map(message).collect(),
        )],
        ME,
    );
    let mut h = Harness::new(vault, (1280., 800.));
    h.update(|s| {
        let pane = &mut s.pane;
        pane.set_open(PaneTab::Chat, true, &mut s.ui);
    });
    h.navigate(Route::Object("page-1".into()));
    assert_no_overlap(&h, "when the chat opens");

    let list = h.rect("chat-messages").expect("the chat pane's messages");
    h.rt.wheel(list[0] + 60.0, list[1] + list[3] * 0.5, 500.0);
    h.settle();
    assert_no_overlap(&h, "after scrolling back");

    // Dragging the chat column to its narrowest: every message re-wraps taller.
    h.update(|s| s.pane.set_width(PaneTab::Chat, 0.));
    assert_no_overlap(&h, "after the chat column narrows");

    // A small window squeezes it further still.
    h.resize(760., 800.);
    assert_no_overlap(&h, "in a small window");

    // And back wide: the messages re-wrap shorter.
    h.resize(1400., 800.);
    h.update(|s| s.pane.set_width(PaneTab::Chat, 600.));
    assert_no_overlap(&h, "after the chat column widens again");

    // A phone-width window: the chat is the bottom sheet, its widest messages a few words a
    // line.
    h.resize(400., 800.);
    h.update(|s| s.pane.set_sheet(true));
    assert!(h.has("disc-sheet"));
    assert_no_overlap(&h, "on a phone-width window");
}

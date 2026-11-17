//! A long thread builds only its newest messages: opening an object with hundreds of chat
//! messages stays quick. "Show earlier messages" brings the older ones in, a page at a time.

use roostr_core::PaneTab;
use roostr_core::nav::Route;

use super::{Harness, Vault, chat_block, object, with_discussion};
use crate::discussion::CHAT_PAGE;

#[test]
fn a_long_thread_shows_its_newest_messages_and_earlier_ones_on_request() {
    let total = CHAT_PAGE * 2 + 5;
    let messages = (1..=total)
        .map(|n| {
            chat_block(
                &format!("m{n}"),
                "peer",
                1_000 + n as i64,
                &format!("Message {n}"),
            )
        })
        .collect();
    let vault = Vault::new(
        vec![with_discussion(
            object("page-1", "page", "Launch plan"),
            messages,
        )],
        "me",
    );
    let mut h = Harness::new(vault, (1280., 800.));
    h.update(|s| s.pane.set_open(PaneTab::Chat, true, &mut s.ui));
    h.navigate(Route::Object("page-1".into()));

    let built = |h: &Harness| (1..=total).filter(|n| h.has(&format!("msg-m{n}"))).count();
    assert_eq!(
        built(&h),
        CHAT_PAGE,
        "only the newest page of messages is built"
    );
    assert!(h.has(&format!("msg-m{total}")), "the newest message shows");
    assert!(!h.has("msg-m1"), "the oldest waits");

    // Scrolled back to the top of what shows, the button is there to press.
    let press = |h: &mut Harness| {
        let list = h.rect("chat-messages").expect("the messages");
        h.rt.wheel(list[0] + 60., list[1] + list[3] / 2., 50_000.);
        h.settle();
        let (x, y) =
            h.rt.find("chat-earlier-button")
                .expect("Show earlier messages");
        h.rt.pointer_move(x, y);
        h.rt.pointer_down(x, y);
        h.rt.pointer_up(x, y);
        h.settle();
    };
    press(&mut h);
    assert_eq!(built(&h), CHAT_PAGE * 2, "a page more");
    press(&mut h);
    assert_eq!(built(&h), total, "then the rest");
    assert!(!h.has("chat-earlier-button"), "nothing earlier is left");
}

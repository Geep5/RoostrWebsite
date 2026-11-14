//! Emoji reactions: reaction lines fold into chips under the message they answer (never rows
//! of their own, the latest line per author and emoji winning), pressing a chip toggles yours,
//! and the reaction bar - opened by a right click, or on a touch screen by holding a message
//! still - posts `chat_post` with the reaction params.

use std::sync::Arc;
use std::time::Duration;

use roostr_core::nav::Route;
use roostr_core::types::Value;
use roostr_core::{JsonMap, PaneTab};
use rune::{Key, Mods, TouchPhase};
use serde_json::Value as Json;

use super::{Harness, Vault, chat_block, object, shown, with_discussion};
use crate::discussion::Place;
use crate::reactions::TouchScreen;

const ME: &str = "me";

/// A reaction line as `chat_post` writes it.
fn reaction(id: &str, author: &str, ts: i64, target: &str, emoji: &str, removed: bool) -> Json {
    let mut line = chat_block(id, author, ts, "");
    let meta = &mut line["content"]["custom"]["meta"];
    meta["kind"] = "reaction".into();
    meta["replyTo"] = target.into();
    meta["emoji"] = emoji.into();
    if removed {
        meta["removed"] = "1".into();
    }
    line
}

/// Earlier chatter (the list scrolls), then a peer's message `m1` and yours `m2`, then
/// `lines`.
fn chat(lines: Vec<Json>) -> Vec<Json> {
    let mut blocks: Vec<Json> = (0..20)
        .map(|n| chat_block(&format!("f{n}"), "peer", 100 + n, &format!("Earlier {n}")))
        .collect();
    blocks.push(chat_block("m1", "peer", 1_000, "Found 1 new message"));
    blocks.push(chat_block("m2", ME, 2_000, "Thanks"));
    blocks.extend(lines);
    blocks
}

/// The space's Reaction "Approve" (🚀).
fn approve() -> roostr_core::Object {
    let mut o = object("rx-approve", "reaction", "Approve");
    o.fields.insert("emoji".into(), Value::string("🚀"));
    o
}

fn open(vault: Arc<Vault>) -> Harness {
    let mut h = Harness::new(vault, (1280., 800.));
    h.update(|s| s.pane.set_open(PaneTab::Chat, true, &mut s.ui));
    h.navigate(Route::Object("page-1".into()));
    h
}

/// Message `id`'s chips as shown: (emoji, who).
fn chips(h: &Harness, id: &str) -> Vec<(String, Vec<String>)> {
    let d = shown(h, Place::Pane).expect("the chat pane's discussion");
    d.message(id)
        .expect("the message")
        .reactions
        .iter()
        .map(|r| (r.emoji.clone(), r.authors.clone()))
        .collect()
}

fn own(chips: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
    chips
        .iter()
        .map(|(e, who)| (e.to_string(), who.iter().map(|w| w.to_string()).collect()))
        .collect()
}

fn center(h: &Harness, key: &str) -> [f32; 2] {
    let r = h.rect(key).unwrap_or_else(|| panic!("{key} is drawn"));
    [r[0] + r[2] / 2., r[1] + r[3] / 2.]
}

fn click(h: &mut Harness, key: &str) {
    let [x, y] = center(h, key);
    h.rt.pointer_move(x, y);
    h.rt.pointer_down(x, y);
    h.frame();
    h.rt.pointer_up(x, y);
    h.settle();
}

fn right_click(h: &mut Harness, key: &str) {
    let [x, y] = center(h, key);
    h.rt.pointer_move(x, y);
    h.rt.secondary_down(x, y);
    h.frame();
    h.rt.secondary_up(x, y);
    h.settle();
}

/// The reactions posted: `chat_post`s carrying `reaction`, without the planner clock the
/// store adds to every mutation.
fn reacted(vault: &Vault) -> Vec<JsonMap> {
    vault
        .chat_posts
        .lock()
        .iter()
        .filter(|p| p.contains_key("reaction"))
        .map(|p| {
            let mut p = p.clone();
            p.remove("now_ms");
            p.remove("tz_offset_min");
            p
        })
        .collect()
}

fn post(object: &str, target: &str, emoji: &str, removed: bool) -> Json {
    serde_json::json!({
        "object_id": object, "thread_id": "", "text": "", "reply_to": target,
        "reaction": emoji, "removed": removed,
    })
}

#[test]
fn reaction_lines_fold_into_chips_under_their_message_and_never_show_as_rows() {
    let vault = Vault::new(
        vec![with_discussion(
            object("page-1", "page", "Inbox"),
            chat(vec![
                reaction("r1", ME, 3_000, "m1", "✅", false),
                reaction("r2", "peer", 3_100, "m1", "✅", false),
                // Taken back: the removal is the latest line.
                reaction("r3", "peer", 3_200, "m1", "👀", false),
                reaction("r4", "peer", 3_300, "m1", "👀", true),
                // Merged out of order: the add is the newer line, so it stays.
                reaction("r5", ME, 3_500, "m1", "🔥", false),
                reaction("r6", ME, 3_400, "m1", "🔥", true),
            ]),
        )],
        ME,
    );
    let h = open(vault.clone());
    assert!(h.has("msg-m1") && h.has("msg-m2"));
    for line in ["r1", "r2", "r3", "r4", "r5", "r6"] {
        assert!(!h.has(&format!("msg-{line}")), "reaction {line} is no row");
    }
    let d = shown(&h, Place::Pane).unwrap();
    assert_eq!(d.messages.len(), 22, "only the messages are messages");
    assert_eq!(
        chips(&h, "m1"),
        own(&[("✅", &[ME, "peer"]), ("🔥", &[ME])])
    );
    assert!(h.has("reaction-m1-0") && h.has("reaction-m1-1"));
    assert!(!h.has("reaction-m1-2"), "the removed 👀 shows no chip");
    let object = vault.objects.lock()[0].clone();
    assert_eq!(
        crate::pane_counts(&object).0,
        22,
        "the pane counts messages only"
    );
}

#[test]
fn pressing_your_chip_takes_your_reaction_back_at_once() {
    let vault = Vault::new(
        vec![with_discussion(
            object("page-1", "page", "Inbox"),
            chat(vec![
                reaction("r1", ME, 3_000, "m1", "✅", false),
                reaction("r2", "peer", 3_100, "m1", "✅", false),
            ]),
        )],
        ME,
    );
    let mut h = open(vault.clone());
    click(&mut h, "reaction-m1-0");
    let posted: Vec<Json> = reacted(&vault).into_iter().map(Json::Object).collect();
    assert_eq!(posted, [post("page-1", "m1", "✅", true)]);
    assert_eq!(
        chips(&h, "m1"),
        own(&[("✅", &["peer"])]),
        "yours goes before the vault has the line"
    );

    // The real line lands: it shows the same, and nothing is pending any more.
    vault.update("page-1", |o| {
        *o = with_discussion(
            object("page-1", "page", "Inbox"),
            chat(vec![
                reaction("r1", ME, 3_000, "m1", "✅", false),
                reaction("r2", "peer", 3_100, "m1", "✅", false),
                reaction("r3", ME, 4_000, "m1", "✅", true),
            ]),
        );
    });
    vault.commit("page-1");
    h.settle();
    assert_eq!(chips(&h, "m1"), own(&[("✅", &["peer"])]));
    assert!(shown(&h, Place::Pane).unwrap().pending_reactions.is_empty());

    // Pressing the peer's chip puts yours on it.
    click(&mut h, "reaction-m1-0");
    let last = reacted(&vault).pop().map(Json::Object);
    assert_eq!(last, Some(post("page-1", "m1", "✅", false)));
    assert_eq!(chips(&h, "m1"), own(&[("✅", &["peer", ME])]));
}

#[test]
fn a_right_click_opens_the_reaction_bar_and_a_pick_posts_the_reaction() {
    let vault = Vault::new(
        vec![
            with_discussion(object("page-1", "page", "Inbox"), chat(Vec::new())),
            approve(),
        ],
        ME,
    );
    let mut h = open(vault.clone());
    assert!(!h.has("react-pick-0"));
    right_click(&mut h, "msg-text-m2");
    assert!(h.has("react-pick-0"), "the bar opens");
    assert!(h.has("react-more") && h.has("react-reply") && h.has("react-copy"));
    let quick = crate::reactions::quick_emojis(&h.rt.state);
    assert_eq!(quick.len(), 6);
    assert_eq!(quick[0], "🚀", "the space's Reaction emoji lead");

    // Your own message takes reactions too.
    click(&mut h, "react-pick-0");
    assert!(!h.has("react-pick-0"), "picking closes the bar");
    let posted: Vec<Json> = reacted(&vault).into_iter().map(Json::Object).collect();
    assert_eq!(posted, [post("page-1", "m2", "🚀", false)]);
    assert_eq!(chips(&h, "m2"), own(&[("🚀", &[ME])]));

    // Escape closes it unpicked.
    right_click(&mut h, "msg-text-m1");
    assert!(h.has("react-pick-0"));
    h.key(Key::Escape, Mods::default());
    h.settle();
    assert!(!h.has("react-pick-0"));
    assert_eq!(reacted(&vault).len(), 1);
}

#[test]
fn on_a_touch_screen_holding_a_message_still_opens_the_bar_and_a_pan_does_not() {
    let vault = Vault::new(
        vec![with_discussion(
            object("page-1", "page", "Inbox"),
            chat(Vec::new()),
        )],
        ME,
    );
    let mut h = open(vault);
    h.update(|s| s.put(TouchScreen));

    // Held still: the bar waits for the hold, opens once it passes, and stays open once the
    // finger lifts.
    let [x, y] = center(&h, "msg-text-m1");
    h.rt.touch(1, TouchPhase::Started, x, y, 0.);
    h.frame();
    h.frame();
    assert!(!h.has("react-pick-0"), "a press is no hold yet");
    h.advance(Duration::from_millis(600));
    assert!(h.has("react-pick-0"), "the hold opens the bar");
    h.rt.touch(1, TouchPhase::Ended, x, y, 0.7);
    h.settle();
    assert!(h.has("react-pick-0"));
    h.key(Key::Escape, Mods::default());
    h.settle();
    assert!(!h.has("react-pick-0"));

    // Lifted early: nothing.
    let [x, y] = center(&h, "msg-text-m1");
    h.rt.touch(2, TouchPhase::Started, x, y, 1.);
    h.frame();
    h.rt.touch(2, TouchPhase::Ended, x, y, 1.1);
    h.frame();
    assert!(
        shown(&h, Place::Pane).unwrap().hold.is_none(),
        "the lift ends the hold"
    );
    h.advance(Duration::from_millis(600));
    assert!(!h.has("react-pick-0"), "a tap opens nothing");

    // Moved first: the list scrolls under the finger and the bar stays shut.
    let [x, y] = center(&h, "msg-text-m1");
    h.rt.touch(3, TouchPhase::Started, x, y, 2.);
    h.frame();
    let before = h.rect("msg-text-m1").unwrap()[1];
    for i in 1..=10 {
        h.rt.touch(
            3,
            TouchPhase::Moved,
            x,
            y + 8. * i as f32,
            2. + 0.02 * i as f64,
        );
        h.frame();
    }
    assert!(
        h.rect("msg-text-m1").unwrap()[1] > before + 20.,
        "the list pans"
    );
    assert!(
        shown(&h, Place::Pane).unwrap().hold.is_some(),
        "the finger is still down"
    );
    h.advance(Duration::from_millis(600));
    assert!(!h.has("react-pick-0"), "a pan is no hold");
    h.rt.touch(3, TouchPhase::Ended, x, y + 80., 2.9);
    h.settle();
    assert!(!h.has("react-pick-0"));
}

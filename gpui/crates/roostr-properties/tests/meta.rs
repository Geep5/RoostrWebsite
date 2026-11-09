//! Headless tests of the emoji picker (the meta badges are tested in `src/meta.rs`).

mod support;

use roostr_core::State;
use roostr_properties::EmojiPicker;
use roostr_ui::ThemePref;
use rune::{Cx, El};
use support::{App, Vault};

#[derive(Default)]
struct Picked(Vec<String>);

fn picker_view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut layer = vec![];
    let body = EmojiPicker::new("emoji")
        .on_pick(|s: &mut State, icon: &str| s.slot::<Picked>().0.push(icon.to_string()))
        .build(s, cx, t, &mut layer);
    roostr_ui::root(body, layer)
}

/// The index of `emoji` in the bundled data (the cell key is `emoji-<index>`).
fn emoji_index(emoji: &str) -> usize {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../src/emoji_data.json")).expect("valid");
    data["emoji"]
        .as_array()
        .expect("emoji list")
        .iter()
        .position(|e| e[0] == emoji)
        .expect("bundled emoji")
}

#[test]
fn emoji_search_finds_by_alias_and_picks() {
    let mut app = App::new(Vault::new(vec![], vec![]), picker_view);
    app.step();
    let pin = format!("emoji-list/emoji-{}", emoji_index("🧋"));
    app.click("emoji/query/query-field");
    // "boba" is only an alias of 🧋 (CLDR: "bubble tea").
    app.type_text("boba");
    assert!(app.shown(&pin), "🧋 found by its alias");
    app.click(&pin);
    assert_eq!(app.s().slot::<Picked>().0, vec!["🧋".to_string()]);
}

#[test]
fn emoji_remove_picks_empty() {
    let mut app = App::new(Vault::new(vec![], vec![]), picker_view);
    app.step();
    app.click("emoji/remove");
    assert_eq!(app.s().slot::<Picked>().0, vec![String::new()]);
}

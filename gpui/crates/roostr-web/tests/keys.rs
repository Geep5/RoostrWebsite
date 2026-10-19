//! In a Mac browser Command is GPUI's `secondary` key, as on the desktop:
//! Command-C copies the chat's selected text, Command-A selects all, and so
//! on (vendor/gpui-pre-web's `keystroke_modifiers`). GPUI parses `secondary`
//! as Control on wasm, so a Mac's Command-C used to match no binding.
//!
//! Run with `wasm-bindgen-test-runner` as the wasm32 test runner (Node is
//! enough): `cargo test --target wasm32-unknown-unknown -p roostr-web`.

#![cfg(target_family = "wasm")]

use gpui_kit::web::keystroke_modifiers;
use gpui_kit::{KeyBinding, Keystroke, Modifiers, actions};
use wasm_bindgen_test::*;

actions!(keys_test, [Shortcut]);

fn command() -> Modifiers {
    Modifiers {
        platform: true,
        ..Modifiers::default()
    }
}

fn control() -> Modifiers {
    Modifiers {
        control: true,
        ..Modifiers::default()
    }
}

/// Whether `key` pressed with the DOM's `held` fires the binding `binding`.
fn fires(binding: &str, held: Modifiers, key: &str, mac: bool) -> bool {
    let keystroke = Keystroke {
        modifiers: keystroke_modifiers(held, key, mac),
        key: key.to_string(),
        key_char: None,
    };
    KeyBinding::new(binding, Shortcut, None).match_keystrokes(&[keystroke]) == Some(false)
}

#[wasm_bindgen_test]
fn command_is_secondary_in_a_mac_browser() {
    for key in ["c", "x", "a", "z", "b"] {
        let binding = format!("secondary-{key}");
        assert!(fires(&binding, command(), key, true), "Command-{key}");
        assert!(!fires(&binding, control(), key, true), "Control-{key}");
    }
    let shift_command = Modifiers {
        shift: true,
        ..command()
    };
    assert!(fires("secondary-shift-z", shift_command, "z", true));
}

#[wasm_bindgen_test]
fn control_is_secondary_elsewhere() {
    assert!(fires("secondary-c", control(), "c", false));
    assert!(!fires("secondary-c", command(), "c", false));
}

/// Command-V matches nothing, so the browser's own paste runs (its `paste`
/// event carries the clipboard to the focused input).
#[wasm_bindgen_test]
fn command_v_stays_the_browsers_paste() {
    assert!(!fires("secondary-v", command(), "v", true));
    let shift_command = Modifiers {
        shift: true,
        ..command()
    };
    assert_eq!(keystroke_modifiers(shift_command, "v", true), shift_command);
}

#[wasm_bindgen_test]
fn parsed_secondary_is_control_on_wasm() {
    // What makes the Mac swap necessary; if GPUI ever resolves `secondary`
    // per browser, `keystroke_modifiers` must go.
    assert_eq!(
        Keystroke::parse("secondary-c").unwrap().modifiers,
        control()
    );
}

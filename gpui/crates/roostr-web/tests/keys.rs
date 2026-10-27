//! In a Mac browser Command is GPUI's `secondary` key, as on the desktop:
//! Command-C copies the chat's selected text, Command-A selects all, and so
//! on (vendor/gpui-pre-web's `keystroke_modifiers`). GPUI Kit binds Control
//! on wasm, and the web platform reports a Mac browser as no Mac, so GPUI
//! resolves `secondary` (`KeyBinding::load`) as Control there too; a Mac's
//! Command-C would otherwise match no binding.
//!
//! Run with `wasm-bindgen-test-runner` as the wasm32 test runner (Node is
//! enough): `cargo test --target wasm32-unknown-unknown -p roostr-web`.

#![cfg(target_family = "wasm")]

use gpui_kit::web::keystroke_modifiers;
use gpui_kit::{Keystroke, Modifiers, OperatingSystem};
use wasm_bindgen_test::*;

/// The OS GPUI is told a browser runs on, for resolving `secondary`: a Mac
/// browser reports `Unknown` (vendor/gpui-pre-web's `platform_info`).
const BROWSER_OS: OperatingSystem = OperatingSystem::Unknown;

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
    let binding = Keystroke::parse_with_os(binding, BROWSER_OS).unwrap();
    binding.modifiers == keystroke.modifiers && binding.key == keystroke.key
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
fn secondary_is_control_for_a_browser() {
    // What makes the Mac swap necessary: the kit binds Control on wasm, and
    // GPUI resolves `secondary` as Command only for a Mac, which the web
    // platform does not report. If either follows the browser's OS,
    // `keystroke_modifiers` must go.
    assert_eq!(
        Keystroke::parse_with_os("secondary-c", BROWSER_OS)
            .unwrap()
            .modifiers,
        control()
    );
}

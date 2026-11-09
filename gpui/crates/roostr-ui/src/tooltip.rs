//! Tooltips: the `[data-tip]` tooltip as gpui-kit's `Tooltip` drew it (popover fill, 1px
//! border, radius 6, `shadow_md`, 12.25px text, padding 2 × 7, the shortcut as muted 10.5px text
//! 10.5px after the label), on Rune's own help tooltips (`El::help`).
//!
//! [`crate::theme`] installs the look each view run (`Cx::help_look`); elements opt in with
//! [`tooltip`]: `tooltip(button, "Search (⌘K)")` shows "Search" with the `⌘K` hint.

use rune::{El, HelpLook};

use crate::theme::{Theme, radius, rems, shadow, ty};

/// Roostr's tooltip look for `t`: gpui-kit's `Tooltip`, after GPUI's 500 ms delay, sliding
/// 4px and fading in over 150 ms (`ENTER_DURATION`).
pub fn help_look(t: &Theme) -> HelpLook {
    let hint = ty::ui(rems(0.75)).lh(rems(0.75));
    HelpLook::system(t.dark)
        .fill(t.kit.popover)
        .ink(t.kit.popover_foreground)
        .ring(1.0, t.kit.border)
        .radius(radius::KIT)
        .padding(rems(0.5), rems(0.125))
        .font(ty::ui(rems(0.875)))
        .max_width(None)
        .shadows(shadow::kit_md())
        .gap(4.0)
        .margin(rems(0.75))
        .delay(0.5)
        .skip_delay(0.0)
        .fade(0.15, 0.0, 1.0)
        .keycaps(
            rune::Color::TRANSPARENT,
            t.kit.muted_foreground,
            hint,
            false,
        )
        .keycap_shape(rems(0.75), 0.0, 0.0, 0.0, rems(0.75))
}

/// `el` with the tooltip `text`; a trailing shortcut hint ("Search (⌘K)") shows after the label
/// as the kit drew a `Kbd` ([`split_shortcut`]).
pub fn tooltip(el: El, text: impl Into<String>) -> El {
    let text = text.into();
    match split_shortcut(&text) {
        Some((label, keys)) => el.help(label).help_keys([keys]),
        None => el.help(text),
    }
}

/// A tooltip text's trailing shortcut hint: "Search (⌘K)" → `("Search", "⌘K")`, formatted as
/// gpui-kit's `Kbd` shows it on this platform (⌘ is Ctrl off macOS: "Ctrl+K"). `None` without a
/// modifier glyph in the parentheses.
pub fn split_shortcut(text: &str) -> Option<(&str, String)> {
    let (label, hint) = text.strip_suffix(')')?.rsplit_once(" (")?;
    let (mut control, mut alt, mut shift, mut platform) = (false, false, false, false);
    let mut key = String::new();
    for c in hint.chars() {
        match c {
            '⌘' if cfg!(target_os = "macos") => platform = true,
            '⌘' | '⌃' => control = true,
            '⌥' => alt = true,
            '⇧' => shift = true,
            c => key.extend(c.to_lowercase()),
        }
    }
    if !(control || alt || shift || platform) || key.is_empty() {
        return None;
    }
    Some((label, format_keys(control, alt, shift, platform, &key)))
}

/// gpui-kit's `Kbd::format`: macOS glyphs run together in ⌃⌥⇧⌘ order; elsewhere names joined
/// by `+`.
pub fn format_keys(control: bool, alt: bool, shift: bool, platform: bool, key: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let mut parts: Vec<String> = Vec::new();
    let mut push = |on: bool, glyph: &str, name: &str| {
        if on {
            parts.push(if mac { glyph } else { name }.to_owned());
        }
    };
    push(control, "⌃", "Ctrl");
    push(alt, "⌥", "Alt");
    push(shift, "⇧", "Shift");
    push(platform, "⌘", "Win");
    let named = |glyph: &str, name: &str| {
        if mac {
            glyph.to_owned()
        } else {
            name.to_owned()
        }
    };
    let key = match key {
        "ctrl" => named("⌃", "Ctrl"),
        "alt" => named("⌥", "Alt"),
        "shift" => named("⇧", "Shift"),
        "cmd" => named("⌘", "Win"),
        "space" if mac => "Space".to_owned(),
        "backspace" => named("⌫", "Backspace"),
        "delete" => named("⌫", "Delete"),
        "escape" => named("⎋", "Esc"),
        "enter" => named("⏎", "Enter"),
        "pagedown" => "Page Down".to_owned(),
        "pageup" => "Page Up".to_owned(),
        "left" => named("←", "Left"),
        "right" => named("→", "Right"),
        "up" => named("↑", "Up"),
        "down" => named("↓", "Down"),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => {
            let mut chars = k.chars();
            chars
                .next()
                .map(|f| f.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    };
    parts.push(key);
    parts.join(if mac { "" } else { "+" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_trailing_shortcut() {
        let (label, keys) = split_shortcut("Search (⌘K)").unwrap();
        assert_eq!(label, "Search");
        assert_eq!(
            keys,
            if cfg!(target_os = "macos") {
                "⌘K"
            } else {
                "Ctrl+K"
            }
        );
        let (label, keys) = split_shortcut("New page (⇧⌘N)").unwrap();
        assert_eq!(label, "New page");
        assert_eq!(
            keys,
            if cfg!(target_os = "macos") {
                "⇧⌘N"
            } else {
                "Ctrl+Shift+N"
            }
        );
        assert_eq!(split_shortcut("Back (soon)"), None);
        assert_eq!(split_shortcut("Back"), None);
    }
}

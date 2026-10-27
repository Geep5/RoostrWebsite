//! Chat bubble colors (`src/lib/chat-color.ts`): `#RRGGBB` hex, text inside a
//! bubble is ink or white, whichever reads better.

use gpui_kit::{Hsla, Rgba, rgb, rgba};

/// Roostr cream: the front page background.
pub const DEFAULT_CHAT_COLOR: &str = "#EDCF7F";
pub const INK: &str = "#1B1B1B";
pub const WHITE: &str = "#FFFFFF";

/// A colour as upper-case `#RRGGBB` (what a colour picker's choice is saved as).
pub fn to_hex(color: Hsla) -> String {
    let rgba = Rgba::from(color);
    let c = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02X}{:02X}{:02X}", c(rgba.r), c(rgba.g), c(rgba.b))
}

/// A #RGB or #RRGGBB hex, normalized to upper-case #RRGGBB; `None` when invalid.
pub fn normalize_hex(raw: &str) -> Option<String> {
    let s = raw.trim();
    let h = s.strip_prefix('#').unwrap_or(s);
    if !(h.len() == 3 || h.len() == 6) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let full: String = if h.len() == 3 {
        h.chars().flat_map(|c| [c, c]).collect()
    } else {
        h.to_string()
    };
    Some(format!("#{}", full.to_uppercase()))
}

fn channels(hex: &str) -> Option<[f32; 3]> {
    let h = normalize_hex(hex)?;
    let byte = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16)
            .ok()
            .map(|b| b as f32 / 255.)
    };
    Some([byte(1)?, byte(3)?, byte(5)?])
}

/// WCAG relative luminance of a hex color (0 when invalid).
pub fn luminance(hex: &str) -> f32 {
    let Some(rgb) = channels(hex) else { return 0. };
    let lin = |c: f32| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

fn contrast(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Ink or white: whichever has the higher contrast on `bg`.
pub fn ink_for(bg: &str) -> &'static str {
    let l = luminance(bg);
    if contrast(l, luminance(INK)) >= contrast(l, 1.) {
        INK
    } else {
        WHITE
    }
}

/// Bubble colors for `bg`: background, text, and a tint for code/links.
pub struct Bubble {
    pub bg: Hsla,
    pub ink: Hsla,
    pub tint: Hsla,
}

pub fn bubble(bg: &str) -> Bubble {
    let ink = ink_for(bg);
    Bubble {
        bg: hex(bg).unwrap_or_else(|| hex(DEFAULT_CHAT_COLOR).expect("valid default")),
        ink: hex(ink).expect("valid ink"),
        tint: if ink == INK {
            rgba(0x0000001f).into()
        } else {
            rgba(0xffffff38).into()
        },
    }
}

/// Parse `#RGB`/`#RRGGBB` into a color.
pub fn hex(raw: &str) -> Option<Hsla> {
    let h = normalize_hex(raw)?;
    let n = u32::from_str_radix(&h[1..], 16).ok()?;
    let color: Rgba = rgb(n);
    Some(color.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_short_and_long_hex() {
        assert_eq!(normalize_hex("#abc").as_deref(), Some("#AABBCC"));
        assert_eq!(normalize_hex(" edcf7f ").as_deref(), Some("#EDCF7F"));
        assert_eq!(normalize_hex("#abcd"), None);
        assert_eq!(normalize_hex("#ggg"), None);
    }

    #[test]
    fn picks_readable_ink() {
        assert_eq!(ink_for(DEFAULT_CHAT_COLOR), INK);
        assert_eq!(ink_for("#1B1B1B"), WHITE);
    }

    #[test]
    fn hex_validation_normalizes_short_and_long_and_refuses_the_rest() {
        assert_eq!(normalize_hex("#edcf7f").as_deref(), Some("#EDCF7F"));
        assert_eq!(normalize_hex(" 0af ").as_deref(), Some("#00AAFF"));
        assert_eq!(normalize_hex("#123456").as_deref(), Some("#123456"));
        for bad in [
            "",
            "#",
            "#12",
            "#1234",
            "#12345g",
            "red",
            "rgb(0,0,0)",
            "#1234567",
        ] {
            assert_eq!(normalize_hex(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn ink_on_light_bubbles_white_on_dark_ones() {
        for light in [DEFAULT_CHAT_COLOR, "#FFFFFF", "#FFEB3B"] {
            assert_eq!(ink_for(light), INK, "{light}");
        }
        for dark in ["#000000", "#1E3A8A", "#7C3AED"] {
            assert_eq!(ink_for(dark), WHITE, "{dark}");
        }
    }

    #[test]
    fn bubble_carries_the_chosen_ink_and_a_matching_tint() {
        let light = bubble("#EDCF7F");
        assert_eq!(light.bg, hex("#EDCF7F").unwrap());
        assert_eq!(light.ink, hex(INK).unwrap());
        // rgba(0, 0, 0, 0.12)
        assert_eq!(light.tint.l, 0.);
        assert!((light.tint.a - 0.12).abs() < 0.005, "{:?}", light.tint);
        assert_eq!(bubble("#000000").ink, hex(WHITE).unwrap());
    }
}

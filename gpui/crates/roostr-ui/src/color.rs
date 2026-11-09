//! Colour helpers in the spaces Roostr's GPUI code worked in: hex literals, HSL (gpui-kit's
//! palette), and per-channel mixing in gamma-encoded sRGB as CSS `color-mix(in srgb, ..)` and
//! GPUI's `Rgba` did. Rune's [`Color`] is linear sRGB with straight alpha, so every helper here
//! decodes and encodes at its edges.
//!
//! The chat bubble colours (`src/lib/chat-color.ts`) live here too: `#RRGGBB` hex, text inside
//! a bubble ink or white, whichever reads better.

use rune::Color;
use rune::color::{srgb_decode, srgb_encode};

/// An opaque `0xRRGGBB` sRGB colour.
pub fn rgb(hex: u32) -> Color {
    rgba((hex << 8) | 0xff)
}

/// A `0xRRGGBBAA` sRGB colour with straight alpha.
pub fn rgba(hex: u32) -> Color {
    let byte = |shift: u32| ((hex >> shift) & 0xff) as f32 / 255.0;
    from_srgb([byte(24), byte(16), byte(8)], byte(0))
}

/// A colour from gamma-encoded sRGB channels (0..=1) and straight alpha.
pub fn from_srgb([r, g, b]: [f32; 3], a: f32) -> Color {
    Color {
        r: srgb_decode(r),
        g: srgb_decode(g),
        b: srgb_decode(b),
        a,
    }
}

/// The gamma-encoded sRGB channels (0..=1) of `c`.
pub fn to_srgb(c: Color) -> [f32; 3] {
    [srgb_encode(c.r), srgb_encode(c.g), srgb_encode(c.b)]
}

/// HSL as gpui-kit writes its palette (`hue` in degrees, `saturation` and `lightness` in percent),
/// converted the way GPUI's `Hsla` → `Rgba` does.
pub fn hsl(hue: f32, saturation: f32, lightness: f32) -> Color {
    hsla(hue / 360.0, saturation / 100.0, lightness / 100.0, 1.0)
}

/// GPUI's `hsla(h, s, l, a)` (every part 0..=1) as a colour.
pub fn hsla(h: f32, s: f32, l: f32, a: f32) -> Color {
    let h = h.clamp(0.0, 1.0);
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let cm = c + m;
    let xm = x + m;
    let (r, g, b) = match (h * 6.0).floor() as i32 {
        0 | 6 => (cm, xm, m),
        1 => (xm, cm, m),
        2 => (m, cm, xm),
        3 => (m, xm, cm),
        4 => (xm, m, cm),
        _ => (cm, m, xm),
    };
    from_srgb([r, g, b], a)
}

/// The HSL parts (0..=1) of `c`, as GPUI's `Rgba` → `Hsla` computes them.
pub fn to_hsla(c: Color) -> [f32; 4] {
    let [r, g, b] = to_srgb(c);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let l = (max + min) / 2.0;
    let s = if l == 0.0 || l == 1.0 {
        0.0
    } else if l < 0.5 {
        delta / (2.0 * l)
    } else {
        delta / (2.0 - 2.0 * l)
    };
    let h = if delta == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    [h, s, l, c.a]
}

/// `color-mix(in srgb, a p, b)`: per-channel interpolation in gamma sRGB, alpha too.
pub fn mix_srgb(a: Color, p: f32, b: Color) -> Color {
    let (x, y) = (to_srgb(a), to_srgb(b));
    let m = |x: f32, y: f32| x * p + y * (1.0 - p);
    from_srgb([m(x[0], y[0]), m(x[1], y[1]), m(x[2], y[2])], m(a.a, b.a))
}

/// `a` → `b` at `t` (0..=1), mixed in gamma sRGB like a CSS colour transition.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    mix_srgb(b, t, a)
}

/// CSS `filter: brightness(f)` on a fill: each gamma sRGB channel scaled, capped at 1.
pub fn brighten(color: Color, f: f32) -> Color {
    let [r, g, b] = to_srgb(color);
    from_srgb(
        [(r * f).min(1.0), (g * f).min(1.0), (b * f).min(1.0)],
        color.a,
    )
}

/// Roostr cream: the front page background and the default chat colour.
pub const DEFAULT_CHAT_COLOR: &str = "#EDCF7F";
pub const INK: &str = "#1B1B1B";
pub const WHITE: &str = "#FFFFFF";

/// A colour as upper-case `#RRGGBB` (what a colour picker's choice is saved as).
pub fn to_hex(color: Color) -> String {
    let [r, g, b] = to_srgb(color);
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", c(r), c(g), c(b))
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

/// Parse `#RGB`/`#RRGGBB` into a colour.
pub fn hex(raw: &str) -> Option<Color> {
    let h = normalize_hex(raw)?;
    u32::from_str_radix(&h[1..], 16).ok().map(rgb)
}

/// WCAG relative luminance of a hex colour (0 when invalid).
pub fn luminance(hex: &str) -> f32 {
    let Some(h) = normalize_hex(hex) else {
        return 0.0;
    };
    let channel = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16)
            .map(|b| b as f32 / 255.0)
            .unwrap_or(0.0)
    };
    let lin = |c: f32| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(channel(1)) + 0.7152 * lin(channel(3)) + 0.0722 * lin(channel(5))
}

fn contrast(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Ink or white: whichever has the higher contrast on `bg`.
pub fn ink_for(bg: &str) -> &'static str {
    let l = luminance(bg);
    if contrast(l, luminance(INK)) >= contrast(l, 1.0) {
        INK
    } else {
        WHITE
    }
}

/// Bubble colours for `bg`: background, text, and a tint for code and links.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bubble {
    pub bg: Color,
    pub ink: Color,
    pub tint: Color,
}

pub fn bubble(bg: &str) -> Bubble {
    let ink = ink_for(bg);
    Bubble {
        bg: hex(bg).unwrap_or_else(|| hex(DEFAULT_CHAT_COLOR).expect("valid default")),
        ink: hex(ink).expect("valid ink"),
        tint: if ink == INK {
            rgba(0x0000001f)
        } else {
            rgba(0xffffff38)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Color, b: Color) -> bool {
        let (x, y) = (to_srgb(a), to_srgb(b));
        x.iter().zip(y).all(|(p, q)| (p - q).abs() < 0.002) && (a.a - b.a).abs() < 0.002
    }

    #[test]
    fn hex_literals_round_trip() {
        assert_eq!(to_hex(rgb(0x1e1e20)), "#1E1E20");
        assert_eq!(to_hex(rgba(0x0a84ff66)), "#0A84FF");
        assert!((rgba(0x0a84ff66).a - 0x66 as f32 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn hsl_matches_the_kit_palette_hex() {
        // gpui-kit's red-500: hsl(0 84.2% 60.2%) = #ef4444.
        assert_eq!(to_hex(hsl(0.0, 84.2, 60.2)), "#EF4444");
        // blue-500: hsl(217.2 91.2% 59.8%) = #3b82f6.
        assert_eq!(to_hex(hsl(217.2, 91.2, 59.8)), "#3B82F6");
        let [h, s, l, _] = to_hsla(hsl(217.2, 91.2, 59.8));
        assert!(
            (h * 360.0 - 217.2).abs() < 0.5 && (s - 0.912).abs() < 0.01 && (l - 0.598).abs() < 0.01
        );
    }

    #[test]
    fn srgb_mix_is_per_channel_in_gamma_space() {
        let m = mix_srgb(rgb(0xffffff), 0.5, rgb(0x000000));
        assert!(to_srgb(m).iter().all(|c| (c - 0.5).abs() < 1e-4), "{m:?}");
        assert!(close(
            mix(rgb(0x000000), rgb(0xffffff), 0.25),
            rgb(0x404040)
        ));
        assert!(close(brighten(rgb(0x808080), 1.5), rgb(0xc0c0c0)));
    }

    #[test]
    fn normalizes_short_and_long_hex_and_refuses_the_rest() {
        assert_eq!(normalize_hex("#abc").as_deref(), Some("#AABBCC"));
        assert_eq!(normalize_hex(" edcf7f ").as_deref(), Some("#EDCF7F"));
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
        let light = bubble("#EDCF7F");
        assert_eq!(light.ink, hex(INK).unwrap());
        assert!((light.tint.a - 0.12).abs() < 0.005);
        assert_eq!(bubble("#000000").ink, hex(WHITE).unwrap());
    }
}

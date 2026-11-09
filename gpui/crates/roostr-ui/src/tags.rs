//! Option and tag colours (`src/lib/options.ts`): the palette names stored on options, and the
//! fill / border / text each paints with. The dark looks are today's, value for value; light
//! uses gpui-kit's light `Tag::color` scales (fill 50, border 200, text 600).

use rune::Color;

use crate::color::{hsl, hsla, mix_srgb, to_hsla, to_srgb};
use crate::theme::Theme;

/// gpui-kit's (Tailwind's) colour families, scales 50 … 950 as gpui-kit's `ColorName::scale`
/// resolves them (its `default-colors.json` HSL channels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KitColor {
    Gray,
    Red,
    Orange,
    Amber,
    Yellow,
    Lime,
    Green,
    Emerald,
    Teal,
    Cyan,
    Sky,
    Blue,
    Indigo,
    Violet,
    Purple,
    Fuchsia,
    Pink,
    Rose,
    Neutral,
}

const SCALES: [u16; 11] = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950];

type Scale = [(f32, f32, f32); 11];

#[rustfmt::skip]
const HSL: [Scale; 19] = [
    [(210.0, 20.0, 98.0), (220.0, 14.3, 95.9), (220.0, 13.0, 91.0), (216.0, 12.2, 83.9), (217.9, 10.6, 64.9), (220.0, 8.9, 46.1), (215.0, 13.8, 34.1), (216.9, 19.1, 26.7), (215.0, 27.9, 16.9), (220.9, 39.3, 11.0), (224.0, 71.4, 4.1)],
    [(0.0, 85.7, 97.3), (0.0, 93.3, 94.1), (0.0, 96.3, 89.4), (0.0, 93.5, 81.8), (0.0, 90.6, 70.8), (0.0, 84.2, 60.2), (0.0, 72.2, 50.6), (0.0, 73.7, 41.8), (0.0, 70.0, 35.3), (0.0, 62.8, 30.6), (0.0, 74.7, 15.5)],
    [(33.3, 100.0, 96.5), (34.3, 100.0, 91.8), (32.1, 97.7, 83.1), (30.7, 97.2, 72.4), (27.0, 96.0, 61.0), (24.6, 95.0, 53.1), (20.5, 90.2, 48.2), (17.5, 88.3, 40.4), (15.0, 79.1, 33.7), (15.3, 74.6, 27.8), (13.0, 81.1, 14.5)],
    [(48.0, 100.0, 96.1), (48.0, 96.5, 88.8), (48.0, 96.6, 76.7), (45.9, 96.7, 64.5), (43.3, 96.4, 56.3), (37.7, 92.1, 50.2), (32.1, 94.6, 43.7), (26.0, 90.5, 37.1), (22.7, 82.5, 31.4), (21.7, 77.8, 26.5), (20.9, 91.7, 14.1)],
    [(54.5, 91.7, 95.3), (54.9, 96.7, 88.0), (52.8, 98.3, 76.9), (50.4, 97.8, 63.5), (47.9, 95.8, 53.1), (45.4, 93.4, 47.5), (40.6, 96.1, 40.4), (35.5, 91.7, 32.9), (31.8, 81.0, 28.8), (28.4, 72.5, 25.7), (26.0, 83.3, 14.1)],
    [(78.3, 92.0, 95.1), (79.6, 89.1, 89.2), (80.9, 88.5, 79.6), (82.0, 84.5, 67.1), (82.7, 78.0, 55.5), (83.7, 80.5, 44.3), (84.8, 85.2, 34.5), (85.9, 78.4, 27.3), (86.3, 69.0, 22.7), (87.6, 61.2, 20.2), (89.3, 80.4, 10.0)],
    [(138.5, 76.5, 96.7), (140.6, 84.2, 92.5), (141.0, 78.9, 85.1), (141.7, 76.6, 73.1), (141.9, 69.2, 58.0), (142.1, 70.6, 45.3), (142.1, 76.2, 36.3), (142.4, 71.8, 29.2), (142.8, 64.2, 24.1), (143.8, 61.2, 20.2), (144.9, 80.4, 10.0)],
    [(151.8, 81.0, 95.9), (149.3, 80.4, 90.0), (152.4, 76.0, 80.4), (156.2, 71.6, 66.9), (158.1, 64.4, 51.6), (160.1, 84.1, 39.4), (161.4, 93.5, 30.4), (162.9, 93.5, 24.3), (163.1, 88.1, 19.8), (164.2, 85.7, 16.5), (165.7, 91.3, 9.0)],
    [(166.2, 76.5, 96.7), (167.2, 85.5, 89.2), (168.4, 83.8, 78.2), (170.6, 76.9, 64.3), (172.5, 66.0, 50.4), (173.4, 80.4, 40.0), (174.7, 83.9, 31.6), (175.3, 77.4, 26.1), (176.1, 69.4, 21.8), (175.9, 60.8, 19.0), (178.6, 84.3, 10.0)],
    [(183.2, 100.0, 96.3), (185.1, 95.9, 90.4), (186.2, 93.5, 81.8), (187.0, 92.4, 69.0), (187.9, 85.7, 53.3), (188.7, 94.5, 42.7), (191.6, 91.4, 36.5), (192.9, 82.3, 31.0), (194.4, 69.6, 27.1), (196.4, 63.6, 23.7), (197.0, 78.9, 14.9)],
    [(204.0, 100.0, 97.1), (204.0, 93.8, 93.7), (200.6, 94.4, 86.1), (199.4, 95.5, 73.9), (198.4, 93.2, 59.6), (198.6, 88.7, 48.4), (200.4, 98.0, 39.4), (201.3, 96.3, 32.2), (201.0, 90.0, 27.5), (202.0, 80.3, 23.9), (204.0, 80.2, 15.9)],
    [(213.8, 100.0, 96.9), (214.3, 94.6, 92.7), (213.3, 96.9, 87.3), (211.7, 96.4, 78.4), (213.1, 93.9, 67.8), (217.2, 91.2, 59.8), (221.2, 83.2, 53.3), (224.3, 76.3, 48.0), (225.9, 70.7, 40.2), (224.4, 64.3, 32.9), (226.2, 57.0, 21.0)],
    [(225.9, 100.0, 96.7), (226.5, 100.0, 93.9), (228.0, 96.5, 88.8), (229.7, 93.5, 81.8), (234.5, 89.5, 73.9), (238.7, 83.5, 66.7), (243.4, 75.4, 58.6), (244.5, 57.9, 50.6), (243.7, 54.5, 41.4), (242.2, 47.4, 34.3), (243.8, 47.1, 20.0)],
    [(250.0, 100.0, 97.6), (251.4, 91.3, 95.5), (250.5, 95.2, 91.8), (252.5, 94.7, 85.1), (255.1, 91.7, 76.3), (258.3, 89.5, 66.3), (262.1, 83.3, 57.8), (263.4, 70.0, 50.4), (263.4, 69.3, 42.2), (263.5, 67.4, 34.9), (261.2, 72.6, 22.9)],
    [(270.0, 100.0, 98.0), (268.7, 100.0, 95.5), (268.6, 100.0, 91.8), (269.2, 97.4, 85.1), (270.0, 95.2, 75.3), (270.7, 91.0, 65.1), (271.5, 81.3, 55.9), (272.1, 71.7, 47.1), (272.9, 67.2, 39.4), (273.6, 65.6, 32.0), (273.5, 86.9, 21.0)],
    [(289.1, 100.0, 97.8), (287.0, 100.0, 95.5), (288.3, 95.8, 90.6), (291.1, 93.1, 82.9), (292.0, 91.4, 72.5), (292.2, 84.1, 60.6), (293.4, 69.5, 48.8), (294.7, 72.4, 39.8), (295.4, 70.2, 32.9), (296.7, 63.6, 28.0), (296.8, 90.2, 16.1)],
    [(327.3, 73.3, 97.1), (325.7, 77.8, 94.7), (325.9, 84.6, 89.8), (327.4, 87.1, 81.8), (328.6, 85.5, 70.2), (330.4, 81.2, 60.4), (333.3, 71.4, 50.6), (335.1, 77.6, 42.0), (335.8, 74.4, 35.3), (335.9, 69.0, 30.4), (336.2, 83.9, 17.1)],
    [(355.7, 100.0, 97.3), (355.6, 100.0, 94.7), (352.7, 96.1, 90.0), (352.6, 95.7, 81.8), (351.3, 94.5, 71.4), (349.7, 89.2, 60.2), (346.8, 77.2, 49.8), (345.3, 82.7, 40.8), (343.4, 79.7, 34.7), (341.5, 75.5, 30.4), (343.1, 87.7, 15.9)],
    [(0.0, 0.0, 98.0), (0.0, 0.0, 96.1), (0.0, 0.0, 89.8), (0.0, 0.0, 83.1), (0.0, 0.0, 63.9), (0.0, 0.0, 45.1), (0.0, 0.0, 32.2), (0.0, 0.0, 25.1), (0.0, 0.0, 14.9), (0.0, 0.0, 9.0), (0.0, 0.0, 3.9)],
];

impl KitColor {
    /// The colour at `scale` (50, 100, 200 … 900, 950); any other scale is 500, as in gpui-kit.
    pub fn scale(self, scale: u16) -> Color {
        let i = SCALES.iter().position(|&s| s == scale).unwrap_or(5);
        let (h, s, l) = HSL[self as usize][i];
        hsl(h, s, l)
    }
}

/// One palette entry: a stored option colour `name` and the kit colour it paints with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagColor {
    pub name: &'static str,
    pub kit: KitColor,
}

impl TagColor {
    /// The vivid 500 tone (swatches, badge icons).
    pub fn hex(&self) -> Color {
        self.kit.scale(500)
    }
    /// The kit `Tag::color` fill: 950 at 50% (dark), 50 (light).
    pub fn bg(&self, t: &Theme) -> Color {
        if t.dark {
            self.kit.scale(950).alpha(0.5)
        } else {
            self.kit.scale(50)
        }
    }
    /// The kit `Tag::color` border: 800 at 50% (dark), 200 (light).
    pub fn border(&self, t: &Theme) -> Color {
        if t.dark {
            self.kit.scale(800).alpha(0.5)
        } else {
            self.kit.scale(200)
        }
    }
    /// The kit `Tag::color` text: 300 (dark), 600 (light).
    pub fn text(&self, t: &Theme) -> Color {
        if t.dark {
            self.kit.scale(300)
        } else {
            self.kit.scale(600)
        }
    }
    /// `#rrggbb` of the vivid tone, as the web stores it.
    pub fn hex_str(&self) -> String {
        let [r, g, b] = to_srgb(self.hex());
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
    }
}

const fn tc(name: &'static str, kit: KitColor) -> TagColor {
    TagColor { name, kit }
}

/// The option palette, in the kit's hue order. The first ten names are Anytype's (`grey`,
/// `ice` = the kit's Gray and Sky), kept so stored options keep their colour; the rest are the
/// kit colours Anytype lacks.
pub const TAG_COLORS: &[TagColor] = &[
    tc("grey", KitColor::Gray),
    tc("red", KitColor::Red),
    tc("orange", KitColor::Orange),
    tc("amber", KitColor::Amber),
    tc("yellow", KitColor::Yellow),
    tc("lime", KitColor::Lime),
    tc("green", KitColor::Green),
    tc("emerald", KitColor::Emerald),
    tc("teal", KitColor::Teal),
    tc("cyan", KitColor::Cyan),
    tc("ice", KitColor::Sky),
    tc("blue", KitColor::Blue),
    tc("indigo", KitColor::Indigo),
    tc("violet", KitColor::Violet),
    tc("purple", KitColor::Purple),
    tc("fuchsia", KitColor::Fuchsia),
    tc("pink", KitColor::Pink),
    tc("rose", KitColor::Rose),
];

pub fn tag_color(name: &str) -> Option<&'static TagColor> {
    TAG_COLORS.iter().find(|c| c.name == name)
}

/// `colorHex`: the palette's vivid hex for `name`, else `name` itself, else grey's.
pub fn color_hex(name: &str) -> String {
    match tag_color(name) {
        Some(c) => c.hex_str(),
        None if !name.is_empty() => name.to_string(),
        None => TAG_COLORS[0].hex_str(),
    }
}

/// A pill's colours: fill, text, 1px border.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TagPaint {
    pub bg: Color,
    pub fg: Color,
    pub border: Color,
}

/// A tag/status option's pill: an outline in the option's colour - linked objects are the
/// filled pills ([`object_pill_paint`]), so the two read apart. `None` for an unknown name
/// (draw [`unknown_tag_paint`]).
pub fn tag_colors(t: &Theme, name: &str) -> Option<TagPaint> {
    tag_color(name).map(|c| TagPaint {
        bg: Color::TRANSPARENT,
        fg: c.text(t),
        border: c.text(t).alpha(0.55),
    })
}

/// An option with no (known) colour: a neutral outline, `fg` text.
pub fn unknown_tag_paint(fg: Color) -> TagPaint {
    TagPaint {
        bg: Color::TRANSPARENT,
        fg,
        border: fg.alpha(0.35),
    }
}

/// The pill background of an option with no (known) colour: `rgba(255,255,255,0.09)` (dark),
/// black at 6% (light).
pub fn unknown_tag_bg(t: &Theme) -> Color {
    if t.dark {
        Color::WHITE.alpha(0.09)
    } else {
        Color::BLACK.alpha(0.06)
    }
}

/// A linked object's pill (its emoji and name): neutral fill, `fg` text.
pub fn object_pill_paint(t: &Theme, fg: Color) -> TagPaint {
    TagPaint {
        bg: unknown_tag_bg(t),
        fg,
        border: unknown_tag_bg(t),
    }
}

/// The kit `Tag::color` look for any colour, e.g. a person's chat colour: its hue and
/// saturation at the kit's 950 (fill, 50%), 800 (border, 50%) and 300 (text) lightness when
/// dark; 95%, 85% and 35% lightness when light.
pub fn tint_paint(t: &Theme, color: Color) -> TagPaint {
    let [h, s, _, _] = to_hsla(color);
    let at = |l: f32, a: f32| hsla(h, s, l, a);
    if t.dark {
        TagPaint {
            bg: at(0.16, 0.5),
            border: at(0.32, 0.5),
            fg: at(0.8, 1.0),
        }
    } else {
        TagPaint {
            bg: at(0.95, 1.0),
            border: at(0.85, 1.0),
            fg: at(0.35, 1.0),
        }
    }
}

/// Property badge colours (`badgeStyle`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BadgeColors {
    pub bg: Color,
    pub fg: Color,
    pub icon: Color,
}

/// Soft tint of the option's vivid tone over the panel (18%), its pale label, and the vivid
/// tone for the icon. Unknown → muted 14% over panel, fg, muted.
pub fn badge_colors(t: &Theme, name: &str) -> BadgeColors {
    match tag_color(name) {
        Some(c) => BadgeColors {
            bg: mix_srgb(c.hex(), 0.18, t.panel),
            fg: c.text(t),
            icon: c.hex(),
        },
        None => BadgeColors {
            bg: mix_srgb(t.muted, 0.14, t.panel),
            fg: t.fg,
            icon: t.muted,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::to_hex;

    #[test]
    fn kit_scales_match_the_kit_hex() {
        assert_eq!(to_hex(KitColor::Red.scale(500)), "#EF4444");
        assert_eq!(to_hex(KitColor::Sky.scale(300)), "#7DD3FC");
        assert_eq!(
            to_hex(KitColor::Gray.scale(123)),
            to_hex(KitColor::Gray.scale(500))
        );
    }

    #[test]
    fn badge_mix_is_srgb_interpolation() {
        let t = Theme::dark();
        let red = to_srgb(tag_color("red").unwrap().hex());
        let panel = to_srgb(t.panel);
        let c = to_srgb(badge_colors(t, "red").bg);
        for i in 0..3 {
            assert!((c[i] - (red[i] * 0.18 + panel[i] * 0.82)).abs() < 1e-3);
        }
    }

    #[test]
    fn color_hex_falls_back() {
        let teal = color_hex("teal");
        assert!(teal.len() == 7 && teal.starts_with('#'), "{teal}");
        assert_eq!(color_hex("#123456"), "#123456");
        assert_eq!(color_hex(""), TAG_COLORS[0].hex_str());
    }

    #[test]
    fn stored_anytype_names_keep_a_colour() {
        for name in [
            "grey", "yellow", "orange", "red", "pink", "purple", "blue", "ice", "teal", "lime",
        ] {
            assert!(tag_color(name).is_some(), "{name}");
        }
    }
}

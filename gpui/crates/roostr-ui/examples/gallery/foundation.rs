//! Gallery section: palettes (Roostr tokens, the gpui-kit colours, the option palette), type
//! and every icon.

use roostr_ui::tags::{KitColor, TAG_COLORS, badge_colors, tag_colors};
use roostr_ui::ty::Weights as _;
use roostr_ui::{Glyph, Theme, emoji_or_image_icon, glyph, icon_text, prop_line_icon, ty};
use rune::{Color, El, col, el, row, text};

use crate::{Gallery, section};

/// A `data:` image icon (a rounded square in the accent blue).
pub const SAMPLE_ICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E%3Crect width='16' height='16' rx='3' fill='%230a84ff'/%3E%3Ccircle cx='8' cy='8' r='4' fill='white'/%3E%3C/svg%3E";

fn swatch(name: &str, c: Color, t: &Theme) -> El {
    col()
        .gap(3.0)
        .w(84.0)
        .child(
            el().h(28.0)
                .w_full()
                .radius(6.0)
                .fill(c)
                .border(1.0, t.border),
        )
        .child(
            text(name.to_owned())
                .font(ty::ui(10.0))
                .color(t.muted)
                .truncate(),
        )
}

pub fn view(s: &Gallery, t: &Theme) -> El {
    let tokens = [
        ("bg", t.bg),
        ("gutter", t.gutter),
        ("panel", t.panel),
        ("hover", t.hover),
        ("border", t.border),
        ("fg", t.fg),
        ("muted", t.muted),
        ("accent", t.accent),
        ("green", t.green),
        ("orange", t.orange),
        ("red", t.red),
        ("indigo", t.indigo),
        ("white", t.white),
        ("mark", t.mark),
        ("hl_light", t.hl_light),
        ("hl_med", t.hl_med),
        ("block_selection", t.block_selection),
        ("overlay", t.overlay),
        ("resize_active", t.resize_active),
        ("lint_warning", t.lint_warning),
    ];
    let k = &t.kit;
    let kit = [
        ("background", k.background),
        ("foreground", k.foreground),
        ("border", k.border),
        ("accent", k.accent),
        ("muted", k.muted),
        ("muted_fg", k.muted_foreground),
        ("popover", k.popover),
        ("primary", k.primary),
        ("primary_hover", k.primary_hover),
        ("secondary", k.secondary),
        ("secondary_hover", k.secondary_hover),
        ("button", k.button),
        ("button_hover", k.button_hover),
        ("danger", k.danger),
        ("success", k.success),
        ("warning", k.warning),
        ("info", k.info),
        ("ring", k.ring),
        ("selection", k.selection),
        ("list_active", k.list_active),
        ("list_hover", k.list_hover),
        ("switch", k.switch),
        ("skeleton", k.skeleton),
        ("tab_bar", k.tab_bar),
        ("scrollbar_thumb", k.scrollbar_thumb),
        ("link", k.link),
    ];
    let swatches = |list: &[(&str, Color)]| {
        row()
            .gap(8.0)
            .flex_wrap()
            .children(list.iter().map(|(n, c)| swatch(n, *c, t)))
    };
    let options = row()
        .gap(6.0)
        .flex_wrap()
        .children(TAG_COLORS.iter().map(|c| {
            let paint = tag_colors(t, c.name).expect("palette name");
            let badge = badge_colors(t, c.name);
            col()
                .gap(4.0)
                .items_center()
                .child(el().size(22.0, 22.0).radius(11.0).fill(c.hex()))
                .child(
                    row()
                        .h(20.0)
                        .px(6.0)
                        .radius(10.0)
                        .center()
                        .fill(paint.bg)
                        .border(1.0, paint.border)
                        .child(text(c.name).font(ty::ui(12.0)).color(paint.fg)),
                )
                .child(
                    el().size(22.0, 14.0)
                        .radius(4.0)
                        .fill(badge.bg)
                        .border(1.0, badge.icon),
                )
        }));
    let scales =
        col().gap(2.0).children(
            [KitColor::Blue, KitColor::Rose, KitColor::Neutral].map(|kc| {
                row().gap(2.0).children(
                    [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950]
                        .map(|n| el().size(24.0, 12.0).fill(kc.scale(n))),
                )
            }),
        );
    let icons = row().gap(10.0).flex_wrap().children(
        Glyph::ALL
            .iter()
            .map(|&g| glyph(g, 16.0).color(t.muted).help(g.name())),
    );
    let big = row().gap(10.0).children(
        [
            Glyph::BadgeCheck,
            Glyph::BadgeClock,
            Glyph::BadgeX,
            Glyph::BadgeHalf,
            Glyph::LayoutKanban,
            Glyph::LayoutTable,
            Glyph::Graph,
            Glyph::CheckboxChecked,
        ]
        .map(|g| glyph(g, 28.0).color(t.accent)),
    );
    let type_ = col()
        .gap(6.0)
        .child(
            text("Roostr — the quick brown fox 🦊 🐓 ✓ ⌘K ▨ ◍ ⌕")
                .font(ty::ui(13.0))
                .color(t.fg),
        )
        .child(
            text("Page title 34 / 750")
                .font(ty::ui(34.0).weight(750.0))
                .color(t.fg),
        )
        .child(
            text("Section 11 / 600 muted")
                .font(ty::ui(11.0).semibold())
                .color(t.muted),
        )
        .child(
            text("Kit body 14 · menu 13 · caption 12")
                .font(ty::ui(14.0))
                .color(t.fg),
        )
        .child(
            text("fn main() { mono 13 }")
                .font(ty::mono(13.0))
                .color(t.fg),
        )
        .child(
            text("ARCHIVO BLACK")
                .font(ty::ui(22.0).family(roostr_ui::fonts::family::ARCHIVO_BLACK))
                .color(t.fg),
        );
    let mut media = row().gap(10.0).items_center();
    if let Some(images) = &s.images {
        media = media
            .child(emoji_or_image_icon("🐓", 24.0, images))
            .child(emoji_or_image_icon("📄", 18.0, images))
            .child(emoji_or_image_icon(SAMPLE_ICON, 24.0, images))
            .child(icon_text("🚀", 16.0, ty::ui(13.0), images))
            .child(text("Launch plan").font(ty::ui(13.0)).color(t.fg));
    }
    for (key, format) in [
        ("", "text"),
        ("", "number"),
        ("", "date"),
        ("", "checkbox"),
        ("", "status"),
        ("", "tag"),
        ("", "url"),
        ("", "email"),
        ("", "phone"),
        ("", "object"),
        ("", "file"),
        ("model", ""),
        ("prompt", ""),
        ("agent", ""),
    ] {
        media = media.child(glyph(prop_line_icon(key, format), 16.0).color(t.muted));
    }
    section(
        t,
        "FOUNDATION",
        col()
            .gap(14.0)
            .child(swatches(&tokens))
            .child(swatches(&kit))
            .child(options)
            .child(scales)
            .child(icons)
            .child(big)
            .child(media)
            .child(type_),
    )
}

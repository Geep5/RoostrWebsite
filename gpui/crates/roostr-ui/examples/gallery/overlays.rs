//! Gallery section: menus, popovers and dialogs, open in place and behind live triggers.

use roostr_ui::{
    Dialog, DialogEntrance, DropdownMenu, Glyph, MenuItem, MenuItemStyle, Placement, Popover,
    PopupMenu, PopupMenuItem, Theme, glyph, menu_label, menu_panel, menu_separator, modal_frame,
    popover_surface, rems, ty,
};
use rune::{El, PointerPress, Tx, col, el, row, text};

use roostr_ui::ty::Weights as _;

use crate::{Gallery, labeled, section};

#[derive(Default)]
pub struct State {
    menu: bool,
    sub: bool,
    context: Option<PointerPress>,
    popover: bool,
    dialog: bool,
    pinned: bool,
    last: String,
}

fn ov(s: &mut Gallery) -> &mut State {
    &mut s.overlays
}

/// A plain trigger in the kit's outline-button shape (this section uses only its own
/// components; the real buttons live in the basics section).
fn trigger(t: &Theme, key: &str, label: &str) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .focusable(true)
        .h(rems(2.0))
        .px(rems(1.0))
        .center()
        .radius(roostr_ui::radius::KIT)
        .border(1.0, t.kit.border)
        .fill(t.kit.background)
        .hover(t.kit.secondary_hover)
        .cursor(rune::Cursor::Pointer)
        .child(
            text(label.to_owned())
                .font(ty::ui(rems(0.875)).weight(ty::MEDIUM))
                .color(t.kit.foreground),
        )
}

fn roostr_menu(t: &Theme) -> El {
    menu_panel(t)
        .w(220.0)
        .child(menu_label(t, "Create"))
        .child(
            MenuItem::new("ov-plain", "Plain row")
                .glyph(Glyph::FileText)
                .build(t),
        )
        .child(
            MenuItem::new("ov-accent", "Accent row")
                .accent_hover()
                .build(t),
        )
        .child(
            MenuItem::new("ov-selected", "Selected (accent)")
                .accent_hover()
                .selected(true)
                .build(t),
        )
        .child(
            MenuItem::new("ov-selected-plain", "Selected (plain)")
                .selected(true)
                .glyph(Glyph::Inbox)
                .build(t),
        )
        .child(
            MenuItem::new("ov-trailing", "With trailing")
                .trailing(text("⌘N").font(ty::ui(12.0)).color(t.muted))
                .build(t),
        )
        .child(
            MenuItem::new("ov-disabled", "Disabled row")
                .disabled(true)
                .build(t),
        )
        .child(menu_separator(t))
        .child(
            MenuItem::new("ov-danger", "Delete")
                .style(MenuItemStyle::Danger)
                .build(t),
        )
}

fn kit_menu(t: &Theme, s: &State, live: bool) -> PopupMenu {
    let types = PopupMenu::new()
        .min_w(160.0)
        .item(PopupMenuItem::new("Note").on_press(|g: &mut Gallery| ov(g).last = "Note".into()))
        .item(
            PopupMenuItem::new("Task")
                .checked(true)
                .on_press(|g: &mut Gallery| ov(g).last = "Task".into()),
        );
    PopupMenu::new()
        .min_w(234.0)
        .label("Object")
        .item(
            PopupMenuItem::new("Open in new tab")
                .icon(Glyph::KitExternalLink)
                .on_press(|g: &mut Gallery| ov(g).last = "Open in new tab".into()),
        )
        .item(
            PopupMenuItem::new("Copy ID")
                .icon(Glyph::KitCopy)
                .shortcut("⌘⇧C")
                .on_press(|g: &mut Gallery| ov(g).last = "Copy ID".into()),
        )
        .item(
            PopupMenuItem::new("Pinned")
                .checked(s.pinned)
                .on_press(|g: &mut Gallery| ov(g).pinned ^= true),
        )
        .item(
            PopupMenuItem::submenu("Change type", types, live && s.sub)
                .on_toggle(|g: &mut Gallery, open| ov(g).sub = open),
        )
        .separator()
        .item(PopupMenuItem::new("Duplicate").disabled(true))
        .item(PopupMenuItem::element(
            text("Delete").font(ty::ui(rems(0.875))).color(t.red),
        ))
}

pub fn view(s: &Gallery, t: &Theme, layer: &mut Vec<El>) -> El {
    let o = &s.overlays;
    let mut inline = Vec::new();
    let statics = row()
        .gap(16.0)
        .items_start()
        .flex_wrap()
        .child(labeled(t, "menu_panel + MenuItem", roostr_menu(t)))
        .child(labeled(
            t,
            "PopupMenu (kit)",
            kit_menu(t, o, false).panel(t, "ov-kit", &mut inline),
        ))
        .child(labeled(
            t,
            "Popover",
            popover_surface(
                t,
                col()
                    .w(200.0)
                    .gap(6.0)
                    .child(
                        text("Share")
                            .font(ty::ui(rems(0.875)).weight(ty::SEMIBOLD))
                            .color(t.kit.popover_foreground),
                    )
                    .child(
                        text("Anyone with the link can view.")
                            .font(ty::ui(rems(0.875)))
                            .color(t.kit.muted_foreground)
                            .wrap(),
                    ),
            ),
        ));
    layer.extend(inline);
    let dialog = Dialog::new("ov-static")
        .title("Delete \"Status\"?")
        .description("This deletes the property from every object.")
        .child(
            text("Body content")
                .font(ty::ui(rems(0.875)))
                .color(t.kit.foreground),
        )
        .footer(trigger(t, "ov-static-cancel", "Cancel"))
        .footer(trigger(t, "ov-static-ok", "Delete"))
        .width(360.0)
        .panel(t);
    let frame = modal_frame(t, 320.0).child(
        col()
            .padding(20.0)
            .gap(8.0)
            .child(text("modal_frame").font(ty::ui(16.0)).color(t.fg))
            .child(
                text("Panel fill, 1px border, radius 14, modal shadow.")
                    .font(ty::ui(13.0))
                    .color(t.muted)
                    .wrap(),
            ),
    );
    let modals = row()
        .gap(24.0)
        .items_start()
        .child(labeled(t, "Dialog (kit)", dialog))
        .child(labeled(t, "modal_frame", frame));

    // Live triggers.
    let menu = DropdownMenu::new("ov-menu")
        .trigger(trigger(t, "ov-menu-btn", "Menu").on_press(|g: &mut Gallery| ov(g).menu ^= true))
        .menu(kit_menu(t, o, true))
        .open(o.menu)
        .on_close(|g: &mut Gallery| {
            ov(g).menu = false;
            ov(g).sub = false;
        })
        .build(t, layer);
    let area = el()
        .key("ov-ctx-area")
        .w(220.0)
        .h(rems(2.0) + 24.0)
        .radius(roostr_ui::radius::KIT)
        .border(1.0, t.border)
        .center()
        .child(text("Right-click here").font(ty::ui(12.0)).color(t.muted))
        .on_secondary_press(|g: &mut Gallery, p: PointerPress, _: &mut Tx| ov(g).context = Some(p));
    let context = DropdownMenu::context("ov-ctx", area, o.context)
        .panel(
            menu_panel(t)
                .min_w(170.0)
                .child(
                    MenuItem::new("ov-ctx-rename", "Rename")
                        .glyph(Glyph::FileText)
                        .on_press(|g: &mut Gallery| {
                            ov(g).last = "Rename".into();
                            ov(g).context = None;
                        })
                        .build(t),
                )
                .child(menu_separator(t))
                .child(
                    MenuItem::new("ov-ctx-delete", "Delete")
                        .danger()
                        .on_press(|g: &mut Gallery| {
                            ov(g).last = "Delete".into();
                            ov(g).context = None;
                        })
                        .build(t),
                ),
        )
        .on_close(|g: &mut Gallery| ov(g).context = None)
        .build(t, layer);
    let popover = Popover::new("ov-pop")
        .trigger(
            trigger(t, "ov-pop-btn", "Popover").on_press(|g: &mut Gallery| ov(g).popover ^= true),
        )
        .placement(Placement::BottomStart)
        .child(
            text("Popover content")
                .font(ty::ui(rems(0.875)))
                .color(t.kit.popover_foreground),
        )
        .open(o.popover)
        .on_close(|g: &mut Gallery| ov(g).popover = false)
        .build(t, layer);
    Dialog::new("ov-dialog")
        .title("Rename object")
        .description("Return presses Rename; Escape or the dim closes.")
        .footer(
            trigger(t, "ov-dialog-cancel", "Cancel")
                .on_press(|g: &mut Gallery| ov(g).dialog = false),
        )
        .footer(
            trigger(t, "ov-dialog-ok", "Rename")
                .default_button()
                .on_press(|g: &mut Gallery| {
                    ov(g).last = "Renamed".into();
                    ov(g).dialog = false;
                }),
        )
        .entrance(DialogEntrance::FadeSlide)
        .open(o.dialog)
        .on_close(|g: &mut Gallery| ov(g).dialog = false)
        .build(t, layer);
    let live = row()
        .gap(12.0)
        .center_y()
        .child(menu)
        .child(context)
        .child(popover)
        .child(
            trigger(t, "ov-dialog-btn", "Dialog").on_press(|g: &mut Gallery| ov(g).dialog = true),
        )
        .child(
            row()
                .gap(4.0)
                .center_y()
                .child(glyph(Glyph::Info, 14.0).color(t.muted))
                .child(
                    text(if o.last.is_empty() {
                        "Last pick: none".to_owned()
                    } else {
                        format!("Last pick: {}", o.last)
                    })
                    .font(ty::ui(12.0))
                    .color(t.muted),
                ),
        );
    section(
        t,
        "OVERLAYS",
        col()
            .gap(20.0)
            .items_start()
            .child(statics)
            .child(modals)
            .child(labeled(t, "Live", live)),
    )
}

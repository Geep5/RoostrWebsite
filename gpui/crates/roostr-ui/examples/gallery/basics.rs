//! Gallery section: basics (buttons, toggles, display components, tags, badges, text).

use std::sync::LazyLock;

use roostr_ui::tags::{KitColor, TAG_COLORS};
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    Alert, Avatar, Badge, Button, ButtonCustomVariant, CountBadge, Empty, EmptyContent,
    EmptyHeader, EmptyMedia, Glyph, Kbd, KitSize, Label, Link, Progress, ProgressCircle, Separator,
    ShimmerText, Skeleton, Spinner, Tag, TagPill, Theme, Toggle, ToggleGroup, danger_button,
    disabled, icon_button, outline_button, primary_button, section_label, spaced_text, ty,
};
use rune::{El, Pixels, col, row, text};

use crate::{Gallery, labeled, section};

#[derive(Default)]
pub struct State {
    presses: u32,
    bold: bool,
    days: Vec<bool>,
    when: Vec<bool>,
    alert_closed: bool,
}

/// A small generated picture for the avatar (Rune decodes real ones through `ImageCache`).
static PICTURE: LazyLock<Pixels> = LazyLock::new(|| {
    let p = Pixels::new();
    let n = 32u32;
    let mut rgba = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            rgba.extend_from_slice(&[(x * 8) as u8, 120, (y * 8) as u8, 255]);
        }
    }
    p.set(n, n, rgba);
    p
});

fn wrap(children: impl IntoIterator<Item = El>) -> El {
    row().gap(8.0).flex_wrap().center_y().children(children)
}

fn buttons(s: &Gallery, t: &Theme) -> El {
    let variants: [(&str, fn(Button) -> Button); 10] = [
        ("default", |b| b),
        ("primary", Button::primary),
        ("secondary", Button::secondary),
        ("danger", Button::danger),
        ("warning", Button::warning),
        ("success", Button::success),
        ("info", Button::info),
        ("ghost", Button::ghost),
        ("link", Button::link),
        ("text", Button::text),
    ];
    let mut rows = col().gap(8.0);
    for (name, v) in variants {
        let mut r = row()
            .gap(6.0)
            .center_y()
            .child(text(name).font(ty::ui(11.0)).color(t.muted).w(64.0));
        for (sz, size) in [
            ("xs", KitSize::XSmall),
            ("s", KitSize::Small),
            ("m", KitSize::Medium),
            ("l", KitSize::Large),
        ] {
            r = r.child(
                v(Button::new(format!("b-{name}-{sz}")).label(name).size(size))
                    .on_press(|g: &mut Gallery| g.basics.presses += 1)
                    .build(t),
            );
        }
        r = r
            .child(
                v(Button::new(format!("b-{name}-sel"))
                    .label("selected")
                    .selected(true))
                .build(t),
            )
            .child(
                v(Button::new(format!("b-{name}-load"))
                    .label("loading")
                    .icon(Glyph::Check)
                    .loading(true))
                .build(t),
            )
            .child(
                v(Button::new(format!("b-{name}-dis"))
                    .label("disabled")
                    .disabled(true))
                .build(t),
            )
            .child(
                v(Button::new(format!("b-{name}-out"))
                    .label("outline")
                    .outline())
                .build(t),
            )
            .child(
                v(Button::new(format!("b-{name}-icon")).icon(Glyph::Plus))
                    .tooltip("Add (⌘N)")
                    .build(t),
            );
        rows = rows.child(r);
    }
    let custom = ButtonCustomVariant::new(t)
        .color(t.indigo)
        .foreground(t.white)
        .hover(t.accent)
        .active(t.accent)
        .shadow(true);
    rows.child(wrap([
        Button::new("b-custom")
            .label("custom")
            .custom(custom)
            .build(t),
        Button::new("b-compact").label("compact").compact().build(t),
        Button::new("b-rounded")
            .label("rounded L")
            .rounded(roostr_ui::ButtonRounded::Large)
            .build(t),
        text(format!("pressed {}×", s.basics.presses))
            .font(ty::ui(12.0))
            .color(t.muted),
    ]))
}

fn roostr_buttons(t: &Theme) -> El {
    wrap([
        icon_button("rb-icon", Glyph::Plus)
            .tooltip("New page (⌘N)")
            .build(t),
        primary_button("rb-primary", "New").build(t),
        outline_button("rb-outline", "Cancel").build(t),
        danger_button("rb-danger", "Delete").build(t),
        disabled(primary_button("rb-primary-dis", "New"), true).build(t),
        disabled(outline_button("rb-outline-dis", "Cancel"), true).build(t),
        disabled(danger_button("rb-danger-dis", "Delete"), true).build(t),
    ])
}

fn toggles(s: &Gallery, t: &Theme) -> El {
    let b = &s.basics;
    let on = |v: &Vec<bool>, i: usize| v.get(i).copied().unwrap_or(false);
    let seg = |key: &str, label: &str, on: bool| {
        Toggle::new(key)
            .checked(on)
            .label(label)
            .h_auto()
            .min_w(0.0)
            .px(9.0)
            .py(3.0)
            .text_size(12.0)
            .pointer()
            .border_color(t.border)
            .text_color(if on { t.fg } else { t.muted })
            .bg(if on {
                roostr_ui::color::rgba(0x7896ff2e)
            } else {
                rune::Color::TRANSPARENT
            })
    };
    let days = ToggleGroup::new("t-days")
        .gap(3.0)
        .children(
            ["M", "T", "W", "T", "F", "S", "S"]
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let o = on(&b.days, i);
                    let t0 = Toggle::new(format!("t-day{i}"))
                        .checked(o)
                        .label(*d)
                        .square(26.0)
                        .min_w(26.0)
                        .p(0.0)
                        .rounded_full()
                        .border_w(1.0)
                        .text_size(11.0)
                        .pointer();
                    if o {
                        t0.bg(t.accent).border_color(t.accent).text_color(t.white)
                    } else {
                        t0.border_color(t.border).text_color(t.muted)
                    }
                }),
        )
        .on_press(|g: &mut Gallery, next: Vec<bool>| g.basics.days = next);
    col()
        .gap(8.0)
        .child(wrap([
            Toggle::new("t-bold")
                .icon(Glyph::CaseSensitive)
                .checked(b.bold)
                .on_press(|g: &mut Gallery, v| g.basics.bold = v)
                .build(t),
            Toggle::new("t-outline")
                .label("outline")
                .outline()
                .checked(b.bold)
                .on_press(|g: &mut Gallery, v| g.basics.bold = v)
                .build(t),
            Toggle::new("t-dis")
                .label("disabled")
                .disabled(true)
                .build(t),
            Toggle::new("t-s")
                .label("small")
                .size(KitSize::Small)
                .build(t),
            Toggle::new("t-l")
                .label("large")
                .size(KitSize::Large)
                .checked(true)
                .build(t),
        ]))
        .child(
            ToggleGroup::new("t-group")
                .segmented()
                .outline()
                .single()
                .child(seg("t-g0", "On a schedule", on(&b.when, 0)))
                .child(seg("t-g1", "After last message", on(&b.when, 1)))
                .on_press(|g: &mut Gallery, next: Vec<bool>| g.basics.when = next)
                .build(t),
        )
        .child(days.build(t))
}

fn display(s: &Gallery, t: &Theme) -> El {
    let alerts = col()
        .gap(8.0)
        .child(
            Alert::new("a-default", "A default alert.")
                .title("Heads up")
                .build(t),
        )
        .child(
            Alert::info("a-info", "Sync runs in the background.")
                .title("Info")
                .build(t),
        )
        .child(Alert::success("a-ok", "Saved.").build(t))
        .child(
            Alert::warning("a-warn", "Your vault is almost full.")
                .banner()
                .build(t),
        )
        .children((!s.basics.alert_closed).then(|| {
            Alert::error("a-err", "Could not reach the relay.")
                .title("Error")
                .on_close(|g: &mut Gallery| g.basics.alert_closed = true)
                .build(t)
        }));
    col()
        .gap(12.0)
        .child(labeled(
            t,
            "Label / Link",
            wrap([
                Label::new("Plain label").build(t),
                Label::new("Name").secondary("(secondary)").build(t),
                Label::new("Highlighted roost match")
                    .highlights("roost")
                    .build(t),
                Label::new("secret").masked(true).build(t),
                Link::new("l-open")
                    .label("Open object")
                    .on_press(|g: &mut Gallery| g.basics.presses += 1)
                    .build(t),
                Link::new("l-fg")
                    .label("Member")
                    .color(t.fg)
                    .underline(t.fg.alpha(0.4))
                    .text_size(12.0)
                    .weight(ty::SEMIBOLD)
                    .build(t),
            ]),
        ))
        .child(labeled(
            t,
            "Separator",
            col()
                .gap(10.0)
                .child(Separator::horizontal().build(t))
                .child(Separator::horizontal().dashed().build(t))
                .child(Separator::horizontal().label("OR").build(t))
                .child(
                    row()
                        .h(24.0)
                        .gap(12.0)
                        .child(text("a").color(t.fg))
                        .child(Separator::vertical().build(t))
                        .child(text("b").color(t.fg))
                        .child(Separator::vertical().dashed().build(t))
                        .child(text("c").color(t.fg)),
                ),
        ))
        .child(labeled(
            t,
            "Kbd",
            wrap([
                Kbd::new("cmd-k").build(t),
                Kbd::new("cmd-shift-n").build(t),
                Kbd::new("escape").outline().build(t),
                Kbd::new("ctrl-enter").appearance(false).build(t),
            ]),
        ))
        .child(labeled(
            t,
            "Avatar",
            wrap([
                Avatar::new().size(KitSize::XSmall).build(t),
                Avatar::new()
                    .size(KitSize::Small)
                    .name("Grant Farwell")
                    .build(t),
                Avatar::new().name("Roostr").build(t),
                Avatar::new()
                    .size(KitSize::Large)
                    .name("Ada Lovelace")
                    .build(t),
                Avatar::new().src(PICTURE.clone()).build(t),
                Avatar::new()
                    .size(KitSize::Px(32.0))
                    .placeholder_size(16.0)
                    .build(t),
            ]),
        ))
        .child(labeled(t, "Alert", alerts))
        .child(labeled(
            t,
            "Empty",
            row()
                .gap(12.0)
                .child(
                    Empty::new()
                        .header(
                            EmptyHeader::new()
                                .media(EmptyMedia::icon(Glyph::Inbox))
                                .title("No messages")
                                .description("New messages show up here."),
                        )
                        .content(EmptyContent::new().child(primary_button("e-new", "New").build(t)))
                        .build(t)
                        .border(1.0, t.border)
                        .border_dash(4.0, 2.0)
                        .w(300.0),
                )
                .child(
                    Empty::new()
                        .text("Nothing pinned", 12.0, Some(t.muted))
                        .build(t)
                        .items_start()
                        .gap(0.0)
                        .padding(0.0)
                        .px(8.0)
                        .w(160.0),
                ),
        ))
        .child(labeled(
            t,
            "Spinner / Progress / Skeleton / Shimmer",
            col()
                .gap(10.0)
                .child(wrap([
                    Spinner::new().size(KitSize::XSmall).build(t),
                    Spinner::new().build(t),
                    Spinner::new().size(KitSize::Large).color(t.muted).build(t),
                    ProgressCircle::new("pc-25")
                        .value(25.0)
                        .size(KitSize::Large)
                        .build(t),
                    ProgressCircle::new("pc-70")
                        .value(70.0)
                        .px(20.0)
                        .color(t.accent)
                        .build(t),
                    ProgressCircle::new("pc-load")
                        .loading(true)
                        .px(20.0)
                        .color(t.orange)
                        .build(t),
                ]))
                .child(
                    Progress::new("p-xs")
                        .value(30.0)
                        .size(KitSize::XSmall)
                        .build(t),
                )
                .child(Progress::new("p-m").value(60.0).build(t))
                .child(
                    Progress::new("p-load")
                        .loading(true)
                        .size(KitSize::Large)
                        .build(t),
                )
                .child(Skeleton::new().build(t).h(10.0).w(240.0).radius(5.0))
                .child(
                    Skeleton::new()
                        .secondary()
                        .build(t)
                        .h(10.0)
                        .w(160.0)
                        .radius(5.0),
                )
                .child(
                    ShimmerText::new("sh", "Loading your space…")
                        .font(ty::ui(12.0))
                        .color(t.muted)
                        .build(t),
                ),
        ))
}

fn tags(t: &Theme) -> El {
    let kinds: [(&str, fn() -> Tag); 6] = [
        ("primary", Tag::primary),
        ("secondary", Tag::secondary),
        ("danger", Tag::danger),
        ("success", Tag::success),
        ("warning", Tag::warning),
        ("info", Tag::info),
    ];
    let mut kit = Vec::new();
    for (name, f) in kinds {
        kit.push(f().label(name).build(t));
        kit.push(f().outline().label(name).build(t));
    }
    kit.push(Tag::color(KitColor::Violet).label("violet").build(t));
    kit.push(
        Tag::secondary()
            .size(KitSize::Small)
            .label("small")
            .build(t),
    );
    kit.push(
        Tag::secondary()
            .size(KitSize::Large)
            .label("large")
            .build(t),
    );
    kit.push(Tag::primary().rounded_full().label("rounded").build(t));
    let pills = TAG_COLORS
        .iter()
        .map(|c| TagPill::new(c.name, c.name).build(t))
        .chain([TagPill::new("unknown", "").build(t)]);
    let badges = [
        Badge::new("bd-done", "Done", "green", Glyph::BadgeCheck)
            .tooltip("Status")
            .on_press(|g: &mut Gallery| g.basics.presses += 1)
            .build(t),
        Badge::new("bd-due", "Tomorrow", "orange", Glyph::BadgeCalendar)
            .on_press(|g: &mut Gallery| g.basics.presses += 1)
            .build(t),
        Badge::new("bd-plain", "Note", "", Glyph::BadgeDot).build(t),
        Badge::new("bd-blue", "Linked", "blue", Glyph::BadgeLink).build(t),
    ];
    col()
        .gap(10.0)
        .child(labeled(t, "Tag", wrap(kit)))
        .child(labeled(t, "TagPill (every palette name)", wrap(pills)))
        .child(labeled(t, "Badge (first two clickable)", wrap(badges)))
        .child(labeled(
            t,
            "CountBadge",
            row()
                .gap(20.0)
                .child(
                    CountBadge::new("cb-1-badge", icon_button("cb-1", Glyph::Inbox).build(t))
                        .count(3)
                        .build(t),
                )
                .child(
                    CountBadge::new("cb-2-badge", icon_button("cb-2", Glyph::Inbox).build(t))
                        .count(120)
                        .build(t),
                )
                .child(
                    CountBadge::new("cb-3-badge", icon_button("cb-3", Glyph::Inbox).build(t))
                        .dot()
                        .build(t),
                ),
        ))
}

fn texts(t: &Theme) -> El {
    col()
        .gap(8.0)
        .child(section_label(t, "Properties"))
        .child(
            spaced_text("SPACED 0.2EM", 0.2)
                .size(13.0)
                .weight(ty::MEDIUM)
                .color(t.fg)
                .build(t),
        )
        .child(
            roostr_ui::SpacedText::new(0.1)
                .run("RED ", Some(t.red))
                .run("GREEN ", Some(t.green))
                .run("plain", None)
                .size(14.0)
                .color(t.muted)
                .build(t),
        )
        .child(
            text("plain 13px for comparison")
                .font(ty::ui(13.0).medium())
                .color(t.fg),
        )
}

pub fn view(s: &Gallery, t: &Theme, _: &mut Vec<El>) -> El {
    section(
        t,
        "BASICS",
        col()
            .gap(14.0)
            .child(labeled(
                t,
                "Button: variant × size, selected, loading, disabled, outline, icon",
                buttons(s, t),
            ))
            .child(labeled(
                t,
                "Roostr buttons (icon, primary, outline, danger; disabled)",
                roostr_buttons(t),
            ))
            .child(labeled(t, "Toggle / ToggleGroup", toggles(s, t)))
            .child(display(s, t))
            .child(tags(t))
            .child(labeled(t, "section_label / spaced_text", texts(t))),
    )
}

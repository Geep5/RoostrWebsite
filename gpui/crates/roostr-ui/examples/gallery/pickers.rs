//! Gallery section: PICKERS & CONTAINERS (calendar, date picker, colour picker, tab bars, scroll
//! areas, group boxes, collapsibles, description lists, breadcrumbs).

use chrono::{Duration, NaiveDate};
use roostr_ui::{
    Breadcrumb, BreadcrumbItem, Calendar, CalendarState, Collapsible, ColorPicker,
    ColorPickerState, DatePicker, DescriptionList, GroupBox, ScrollArea, ScrollbarMode, Tab,
    TabBar, TabVariant, Theme, color, ty,
};
use rune::{El, col, el, row, text};

use crate::{Gallery, labeled, section};

pub struct State {
    cal: CalendarState,
    date: CalendarState,
    date_open: bool,
    picked: Option<NaiveDate>,
    color: ColorPickerState,
    palette: ColorPickerState,
    sliders: ColorPickerState,
    tabs: [usize; 5],
    open: bool,
    closed: bool,
    crumbs: usize,
}

impl Default for State {
    fn default() -> Self {
        let mut cal = CalendarState::new();
        let today = cal.today();
        cal.set_date(today + Duration::days(3));
        let swatch = color::rgb(0xEDCF7F);
        Self {
            cal,
            date: CalendarState::new(),
            date_open: false,
            picked: Some(today),
            color: ColorPickerState::new().default_value(swatch),
            palette: ColorPickerState::new().default_value(swatch),
            sliders: {
                let mut s = ColorPickerState::new().default_value(color::rgb(0x538ACE));
                s.set_active_tab(1);
                s
            },
            tabs: [0, 1, 1, 0, 2],
            open: true,
            closed: false,
            crumbs: 0,
        }
    }
}

fn line(t: &Theme, s: &str) -> El {
    text(s.to_owned()).font(ty::ui(13.0)).color(t.fg)
}

fn tab_bars(s: &Gallery, t: &Theme) -> El {
    let mut c = col().gap(12.0);
    let variants = [
        ("default", TabVariant::Tab),
        ("outline", TabVariant::Outline),
        ("pill", TabVariant::Pill),
        ("segmented", TabVariant::Segmented),
        ("underline", TabVariant::Underline),
    ];
    for (i, (name, v)) in variants.into_iter().enumerate() {
        let bar = TabBar::new(format!("pk-tabs-{name}"))
            .variant(v)
            .selected_index(s.pickers.tabs[i])
            .on_click(move |g: &mut Gallery, ix| g.pickers.tabs[i] = ix)
            .child(Tab::new().label("Account"))
            .child(Tab::new().label("Profile"))
            .child(Tab::new().label("Settings"))
            .child(Tab::new().label("Disabled").disabled(true))
            .build(t);
        c = c.child(labeled(t, name, bar));
    }
    let sizes = row()
        .gap(12.0)
        .items_start()
        .child(
            TabBar::<Gallery>::new("pk-tabs-xs")
                .segmented()
                .xsmall()
                .selected_index(0)
                .child(Tab::new().label("XSmall"))
                .child(Tab::new().label("B"))
                .build(t),
        )
        .child(
            TabBar::<Gallery>::new("pk-tabs-sm")
                .segmented()
                .small()
                .selected_index(0)
                .child(Tab::new().label("Small"))
                .child(Tab::new().label("B"))
                .build(t),
        )
        .child(
            TabBar::<Gallery>::new("pk-tabs-lg")
                .segmented()
                .large()
                .selected_index(0)
                .child(Tab::new().label("Large"))
                .child(Tab::new().label("B"))
                .build(t),
        );
    // The pane's tab switch as the discussion drawer restyles it.
    let pane = TabBar::<Gallery>::new("pk-tabs-pane")
        .segmented()
        .small()
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(9.0)
        .padding(2.0)
        .selected_index(0)
        .child(Tab::new().label("Properties"))
        .child(Tab::new().label("Chat"))
        .build(t);
    c.child(labeled(t, "sizes", sizes))
        .child(labeled(t, "pane (Roostr)", pane))
}

fn scrolls(t: &Theme) -> El {
    let mut rows = col().gap(4.0).padding(8.0);
    for i in 1..=30 {
        rows = rows.child(line(t, &format!("Row {i}")));
    }
    let vertical = ScrollArea::vertical("pk-scroll-v", rows)
        .mode(ScrollbarMode::Always)
        .h(140.0)
        .w(200.0)
        .build(t);
    let mut strip = row().gap(8.0).padding(8.0);
    for i in 1..=20 {
        strip = strip.child(
            el().size(48.0, 48.0)
                .radius(6.0)
                .fill(t.kit.muted)
                .child(line(t, &i.to_string())),
        );
    }
    let horizontal = ScrollArea::horizontal("pk-scroll-h", strip)
        .mode(ScrollbarMode::Always)
        .w(260.0)
        .build(t);
    row()
        .gap(16.0)
        .items_start()
        .child(labeled(
            t,
            "vertical",
            el().border(1.0, t.border).child(vertical),
        ))
        .child(labeled(
            t,
            "horizontal",
            el().border(1.0, t.border).child(horizontal),
        ))
}

fn group_boxes(t: &Theme) -> El {
    let body = || line(t, "Content inside the box");
    row()
        .w_full()
        .equal_columns(4)
        .gap(12.0)
        .items_start()
        .child(
            GroupBox::new("pk-gb-normal")
                .title("Normal")
                .child(body())
                .build(t),
        )
        .child(
            GroupBox::new("pk-gb-fill")
                .fill()
                .title("Fill")
                .child(body())
                .footer("A footer note")
                .build(t),
        )
        .child(
            GroupBox::new("pk-gb-outline")
                .outline()
                .title("Outline")
                .child(body())
                .build(t),
        )
        .child(
            GroupBox::new("pk-gb-roostr")
                .content_fill(t.panel)
                .content_border(1.0, t.border)
                .content_radius(roostr_ui::radius::MENU)
                .content_padding([4.0, 8.0, 4.0, 8.0])
                .content_gap(0.0)
                .child(line(t, "Sidebar section"))
                .child(line(t, "Second row"))
                .build(t),
        )
}

pub fn view(s: &Gallery, t: &Theme, layer: &mut Vec<El>) -> El {
    let p = &s.pickers;
    let calendar = col()
        .w(252.0)
        .padding(10.0)
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(roostr_ui::radius::MENU)
        .child(Calendar::new("pk-cal", &p.cal, |g: &mut Gallery| &mut g.pickers.cal).build(t));
    let date_label = p.picked.map(|d| d.format("%b %-d, %Y").to_string());
    let date_trigger = DatePicker::new("pk-date", &p.date, |g: &mut Gallery| &mut g.pickers.date)
        .open(p.date_open)
        .value_label(date_label.clone())
        .placeholder("Empty")
        .on_toggle(|g: &mut Gallery| g.pickers.date_open = !g.pickers.date_open)
        .on_close(|g: &mut Gallery| g.pickers.date_open = false)
        .on_pick(|g: &mut Gallery, d| g.pickers.picked = d)
        .build(t, layer);
    let date_surface = DatePicker::<Gallery>::new("pk-date-inline", &p.date, |g: &mut Gallery| {
        &mut g.pickers.date
    })
    .on_pick(|g: &mut Gallery, d| g.pickers.picked = d)
    .surface(t);
    let calendars = row()
        .gap(16.0)
        .items_start()
        .child(labeled(t, "calendar (selected + today)", calendar))
        .child(labeled(
            t,
            "date picker",
            col()
                .gap(8.0)
                .items_start()
                .child(date_trigger)
                .child(date_surface),
        ));

    let colors = row()
        .gap(16.0)
        .items_start()
        .child(labeled(
            t,
            "colour picker",
            ColorPicker::new("pk-color", &p.color, |g: &mut Gallery| &mut g.pickers.color)
                .accessibility_label("Chat color picker")
                .build(t, layer),
        ))
        .child(labeled(
            t,
            "palette (open)",
            ColorPicker::new("pk-palette", &p.palette, |g: &mut Gallery| {
                &mut g.pickers.palette
            })
            .panel(t),
        ))
        .child(labeled(
            t,
            "HSLA (open)",
            ColorPicker::new("pk-sliders", &p.sliders, |g: &mut Gallery| {
                &mut g.pickers.sliders
            })
            .panel(t),
        ));

    let collapsibles = row()
        .gap(16.0)
        .items_start()
        .child(labeled(
            t,
            "collapsible (open)",
            Collapsible::new("pk-coll-open")
                .open(p.open)
                .on_toggle(|g: &mut Gallery| g.pickers.open = !g.pickers.open)
                .child(line(t, "▾ Toggle heading"))
                .content(col().pl(20.0).child(line(t, "Nested content")))
                .build(),
        ))
        .child(labeled(
            t,
            "collapsible (closed)",
            Collapsible::new("pk-coll-closed")
                .open(p.closed)
                .on_toggle(|g: &mut Gallery| g.pickers.closed = !g.pickers.closed)
                .child(line(t, "▸ Toggle heading"))
                .content(col().pl(20.0).child(line(t, "Nested content")))
                .build(),
        ));

    let lists = row()
        .gap(16.0)
        .items_start()
        .child(labeled(
            t,
            "description list",
            el().w(420.0).child(
                DescriptionList::horizontal()
                    .columns(2)
                    .item("Name", "roostr.png", 1)
                    .item("Size", "24 KB", 1)
                    .separator()
                    .item("Type", "image/png", 2)
                    .build(t),
            ),
        ))
        .child(labeled(
            t,
            "file facts (Roostr)",
            el().w(240.0).child(
                DescriptionList::horizontal()
                    .bordered(false)
                    .columns(1)
                    .label_width(80.0)
                    .item("SIZE", "24 KB", 1)
                    .item("TYPE", "image/png", 1)
                    .build(t),
            ),
        ));

    let crumbs = Breadcrumb::new("pk-crumbs")
        .child(BreadcrumbItem::new("Home").on_click(|g: &mut Gallery| g.pickers.crumbs += 1))
        .child(BreadcrumbItem::new("Projects"))
        .child(BreadcrumbItem::new("Launch plan"))
        .build(t);
    let header_crumb = Breadcrumb::<Gallery>::new("pk-crumbs-header")
        .text_size(14.0)
        .child(BreadcrumbItem::new("Launch plan").color(t.fg).truncate())
        .build(t);

    section(
        t,
        "PICKERS & CONTAINERS",
        col()
            .gap(20.0)
            .child(calendars)
            .child(colors)
            .child(tab_bars(s, t))
            .child(scrolls(t))
            .child(labeled(t, "group boxes", group_boxes(t)))
            .child(collapsibles)
            .child(lists)
            .child(labeled(
                t,
                "breadcrumb",
                col().gap(8.0).child(crumbs).child(header_crumb),
            )),
    )
}

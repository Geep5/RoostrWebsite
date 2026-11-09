//! A query or collection's table, drawn as Roostr's (roostr-sets `SetTable`): a view bar
//! (Table, search, filter, sort, settings, New), a header of property icons and names with an
//! add-column button, rows ruled apart with a bold, iconed name and properties as pills, tags,
//! checkboxes and text, and a New Object row. The columns take the view's widths and the table
//! scrolls sideways when they are wider than the page.

use rune::{Color, El, Icon, TextStyle, col, el, oklch, row, text};
use rune_kit::Kit;

use crate::App;
use crate::data::{Cell, Pill};

const HEADER_H: f32 = 36.0;
const ROW_H: f32 = 41.0;
const PLUS_W: f32 = 44.0;
const PAD: f32 = 10.0;

static SEARCH: Icon = Icon {
    name: "table-search",
    view: 16.0,
    body: r#"<circle cx="7" cy="7" r="4.25" stroke="currentColor" stroke-width="1.4" fill="none"/><path d="M10.2 10.2L13.5 13.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>"#,
};
static FILTER: Icon = Icon {
    name: "table-filter",
    view: 16.0,
    body: r#"<path d="M2.5 4.5H13.5M4.5 8H11.5M6.5 11.5H9.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>"#,
};
static SORT: Icon = Icon {
    name: "table-sort",
    view: 16.0,
    body: r#"<path d="M5 3V13M5 13L2.8 10.8M5 13L7.2 10.8M11 13V3M11 3L8.8 5.2M11 3L13.2 5.2" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static SETTINGS: Icon = Icon {
    name: "table-settings",
    view: 16.0,
    body: r#"<path d="M2.5 5H13.5M2.5 11H13.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/><circle cx="6" cy="5" r="1.7" fill="currentColor"/><circle cx="10" cy="11" r="1.7" fill="currentColor"/>"#,
};
static GRID: Icon = Icon {
    name: "table-grid",
    view: 16.0,
    body: r#"<rect x="2.5" y="3" width="11" height="10" rx="1.5" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M2.5 6.5H13.5M2.5 9.75H13.5M6.5 6.5V13" stroke="currentColor" stroke-width="1.2"/>"#,
};
static PLUS: Icon = Icon {
    name: "table-plus",
    view: 16.0,
    body: r#"<path d="M8 3.5V12.5M3.5 8H12.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>"#,
};

fn ui(k: &Kit) -> TextStyle {
    TextStyle { size: 13.0, line_height: 18.0, ..k.ty.ui }
}

fn small(k: &Kit) -> TextStyle {
    TextStyle { size: 12.0, line_height: 16.0, ..k.ty.ui }
}

/// An emoji at `size`, or nothing.
fn glyph(icon: &str, size: f32, k: &Kit) -> Option<El> {
    (!icon.is_empty()).then(|| text(icon.to_owned()).font(TextStyle { size, line_height: size + 4.0, ..k.ty.ui }))
}

fn icon_button(key: &str, icon: &'static Icon, ink: Color, k: &Kit) -> El {
    k.interactive(el().key(key.to_owned()).size(30.0, 30.0).radius(8.0).center().child(rune::icon(icon, 16.0).color(ink)))
}

/// Table, search, filter, sort and settings, then New: right-aligned above the table.
fn view_bar(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let filtered = s.page.type_name == "query";
    let table = row()
        .gap(6.0)
        .px(8.0)
        .h(30.0)
        .center_y()
        .child(rune::icon(&GRID, 14.0).color(c.text_faint))
        .child(text("Table").font(small(k)).color(c.text_muted));
    // Roostr's accent New button.
    let new = k.interactive_with(
        row().key("table-new").h(28.0).px(12.0).center().radius(7.0).fill(rune::tok::accent(0, k.dark)).child(text("New").font(TextStyle { weight: 600.0, ..ui(k) }).color(c.white)),
        Color::WHITE.alpha(0.12),
        Color::BLACK.alpha(0.12),
    );
    row()
        .w_full()
        .justify_end()
        .center_y()
        .gap(4.0)
        .pb(6.0)
        .child(k.interactive(table.radius(8.0)))
        .child(icon_button("table-search", &SEARCH, c.icon_muted, k))
        .child(icon_button("table-filter", &FILTER, if filtered { c.text_accent } else { c.icon_muted }, k))
        .child(icon_button("table-sort", &SORT, if s.page.sort.is_some() { c.text_accent } else { c.icon_muted }, k))
        .child(icon_button("table-settings", &SETTINGS, c.icon_muted, k))
        .child(col().pl(6.0).child(new))
}

/// A header cell: the property's icon and name, and the sort when it is by this column.
fn header(s: &App, key: &str, name: &str, icon: &str, width: f32, k: &Kit) -> El {
    let c = &k.c;
    let arrow = match &s.page.sort {
        Some((by, asc)) if by == key => if *asc { " ↑" } else { " ↓" },
        _ => "",
    };
    let mut cell = row().w(width).h(HEADER_H).px(PAD).gap(6.0).center_y().clip_rounded(0.0);
    if let Some(g) = glyph(icon, 13.0, k) {
        cell = cell.child(g);
    }
    let by = key.to_owned();
    k.interactive(
        cell.key(format!("th-{key}"))
            .child(text(format!("{name}{arrow}")).font(ui(k)).color(c.text_muted).truncate())
            .on_press(move |s: &mut App| sort_by(s, &by)),
    )
}

/// A click on a header sorts by it, again the other way.
fn sort_by(s: &mut App, key: &str) {
    let asc = !matches!(&s.page.sort, Some((by, true)) if by == key);
    s.page.sort = Some((key.to_owned(), asc));
    let Some(set) = s.page.set.as_mut() else { return };
    let at = set.columns.iter().position(|c| c.key == key);
    set.rows.sort_by(|a, b| {
        let (x, y) = match at {
            Some(i) => (a.cells[i].sort_text(), b.cells[i].sort_text()),
            None => (a.name.clone(), b.name.clone()),
        };
        // Empty cells last either way, as Roostr sorts.
        let order = match (x.is_empty(), y.is_empty()) {
            (true, false) => return std::cmp::Ordering::Greater,
            (false, true) => return std::cmp::Ordering::Less,
            _ => rune_kit::natural_cmp(&x.to_lowercase(), &y.to_lowercase()),
        };
        if asc { order } else { order.reverse() }
    });
}

fn pill(p: &Pill, k: &Kit) -> El {
    let c = &k.c;
    let mut e = row().h(24.0).px(7.0).gap(5.0).center_y().radius(6.0).fill(c.layer_02).border(1.0, c.border_weak);
    if let Some(g) = glyph(&p.icon, 12.0, k) {
        e = e.child(g);
    }
    e.child(text(p.name.clone()).font(small(k)).color(c.text_base).truncate())
}

/// A tag's fill and ink for Roostr's option colour names.
fn tag_colors(name: &str, dark: bool) -> (Color, Color) {
    let hue = match name {
        "yellow" => Some(95.0),
        "orange" => Some(55.0),
        "red" => Some(25.0),
        "pink" => Some(350.0),
        "purple" => Some(305.0),
        "blue" => Some(255.0),
        "ice" => Some(225.0),
        "teal" => Some(190.0),
        "lime" => Some(130.0),
        _ => None,
    };
    let chroma = |c: f32| if hue.is_some() { c } else { 0.0 };
    let h = hue.unwrap_or(0.0);
    if dark { (oklch(0.36, chroma(0.07), h), oklch(0.9, chroma(0.08), h)) } else { (oklch(0.93, chroma(0.05), h), oklch(0.38, chroma(0.11), h)) }
}

fn tag(t: &str, color: &str, k: &Kit) -> El {
    let (fill, ink) = tag_colors(color, k.dark);
    row().h(22.0).px(8.0).center_y().radius(5.0).fill(fill).child(text(t.to_owned()).font(small(k)).color(ink))
}

fn checkbox(on: bool, k: &Kit) -> El {
    let c = &k.c;
    el().size(16.0, 16.0)
        .radius(4.0)
        .border(1.5, if on { c.text_accent } else { c.border_strong })
        .fill(if on { c.text_accent } else { Color::TRANSPARENT })
        .center()
        .child(if on { text("✓").font(TextStyle { size: 11.0, weight: 700.0, ..k.ty.ui }).color(c.white) } else { el() })
}

fn cell(v: &Cell, width: f32, k: &Kit) -> El {
    let c = &k.c;
    let mut e = row().w(width).h(ROW_H).px(PAD).gap(6.0).center_y().clip_rounded(0.0);
    e = match v {
        Cell::Empty => e,
        Cell::Text(t) => e.child(text(t.clone()).font(ui(k)).color(c.text_base).truncate()),
        Cell::Objects(pills) => pills.iter().fold(e, |e, p| e.child(pill(p, k))),
        Cell::Tags(tags) => tags.iter().fold(e, |e, (t, color)| e.child(tag(t, color, k))),
        Cell::Check(on) => e.child(checkbox(*on, k)),
    };
    e
}

fn rule(width: f32, k: &Kit) -> El {
    el().w(width).h(1.0).fill(k.c.border_weak)
}

pub fn view(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let Some(set) = s.page.set.as_ref() else { return el() };
    let total = set.name_width + set.columns.iter().map(|c| c.width).sum::<f32>() + PLUS_W;
    // The Name column takes what the page has left over, as Roostr's does; wider, it scrolls.
    let avail = rune::memo::layout_with("table-scroll", |r| r[2].round()).unwrap_or(total);
    let fill = avail.max(total);
    let name_w = set.name_width + (fill - total);
    let plus = row()
        .w(PLUS_W)
        .h(HEADER_H)
        .center()
        .child(k.interactive(el().key("table-add-column").size(26.0, 26.0).radius(7.0).border(1.0, c.border_base).center().child(rune::icon(&PLUS, 14.0).color(c.icon_muted))));
    let head = set
        .columns
        .iter()
        .fold(row().w(fill).child(header(s, "name", "Name", "", name_w, k)), |h, col| h.child(header(s, &col.key, &col.name, &col.icon, col.width, k)))
        .child(plus);
    let mut grid = col().w(fill).child(head).child(el().w(fill).h(1.0).fill(c.border_base));
    for r in &set.rows {
        let mut name = row().w(name_w).h(ROW_H).px(PAD).gap(8.0).center_y().clip_rounded(0.0);
        if let Some(g) = glyph(&r.icon, 15.0, k) {
            name = name.child(g);
        }
        name = name.child(text(r.name.clone()).font(TextStyle { weight: 600.0, ..ui(k) }).color(c.text_base).truncate());
        let line = r.cells.iter().zip(&set.columns).fold(row().w(fill).child(name), |line, (v, col)| line.child(cell(v, col.width, k)));
        let id = r.id.clone();
        grid = grid
            .child(
                line.key(format!("row-{}", r.id))
                    .hover(c.hover)
                    .on_double_press(move |s: &mut App| crate::open(s, &id)),
            )
            .child(rule(fill, k));
    }
    let new_row = k.interactive(
        row()
            .key("table-new-object")
            .h(ROW_H)
            .px(PAD)
            .gap(6.0)
            .center_y()
            .radius(6.0)
            .child(rune::icon(&PLUS, 14.0).color(c.text_faint))
            .child(text("New Object").font(ui(k)).color(c.text_muted)),
    );
    let count = text(format!("{} objects", set.rows.len())).font(small(k)).color(c.text_faint);
    col()
        .w_full()
        .pt(22.0)
        .child(view_bar(s, k))
        .child(col().key("table-scroll").w_full().scroll_x().child(grid))
        .child(row().w_full().justify_between().center_y().child(new_row).child(count))
}

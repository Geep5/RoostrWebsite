//! The sidebar's New menu (`createMenuBody`, `+layout.svelte:896-905`, logic
//! 149-207, CSS 1796-1857): a type search over the space's Default type,
//! Query, Collection and the space's other types; ↑/↓ highlight, Enter creates.
//! The choices are a roostr-ui [`Command`] list (one group per kind of
//! choice, separated) under the menu's own type search. On a desktop it drops
//! from the sidebar's New pill (keyed `create-btn`); on a phone it docks above
//! the bottom bar (`.m-create-menu`).

use roostr_core::State;
use roostr_core::create::{FALLBACK_TYPE, creatable_types, default_type_key};
use roostr_core::objects::type_glyph;
use roostr_rt::Task;
use roostr_ui::overlay::{self, Placement};
use roostr_ui::{
    Command, CommandGroup, CommandItem, CommandState, IndexPath, Input, InputState, Theme,
    menu_panel, motion, radius, rems, ty,
};
use rune::{Color, Cursor, Cx, El, Key, KeyEvent, Mods, Tx, col, el, row, text};

use crate::shell::{obj_icon, space_id};

/// The Command list's key (its rows are `create-list-item-<section>-<row>`).
const LIST: &str = "create-list";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Default,
    Fixed,
    Type,
}

struct Choice {
    /// What to create: "query", "collection" or a type key.
    key: String,
    icon: String,
    name: String,
    group: Group,
}

/// The New menu's state (a slot): open, its type search and the list's highlight
/// (nothing highlighted until ↓ or a hover).
#[derive(Default)]
struct CreateMenu {
    open: bool,
    input: InputState,
    list: CommandState,
    /// The space's Default type (`FALLBACK_TYPE` until fetched).
    default_key: String,
    /// The Default type's fetch (dropped, so cancelled, when the menu closes).
    fetch: Option<Task<()>>,
}

/// Open the menu (nothing when it is open): the type search takes the keys and the
/// space's Default type is looked up.
pub(crate) fn open(s: &mut State) {
    if is_open(s) {
        return;
    }
    let space = space_id(s);
    let fetch = default_type_key(&space, s);
    let task = s.rt.spawn(async move |cx| {
        let key = fetch.await;
        cx.update(|s| {
            if space_id(s) == space {
                s.slot::<CreateMenu>().default_key = key;
            }
        });
    });
    let menu = s.slot::<CreateMenu>();
    menu.open = true;
    menu.input = InputState::new();
    menu.input.focus();
    menu.list = CommandState::new();
    menu.list.set_highlighted(None);
    menu.default_key = FALLBACK_TYPE.into();
    menu.fetch = Some(task);
}

pub(crate) fn close(s: &mut State) {
    let menu = s.slot::<CreateMenu>();
    menu.open = false;
    menu.fetch = None;
}

pub(crate) fn toggle(s: &mut State) {
    if is_open(s) { close(s) } else { open(s) }
}

pub(crate) fn is_open(s: &State) -> bool {
    s.get::<CreateMenu>().is_some_and(|m| m.open)
}

/// The space's Default type, Query, Collection, then its other types, filtered by the
/// type search (name or key, case-insensitive).
fn choices(s: &State) -> Vec<Choice> {
    let Some(menu) = s.get::<CreateMenu>() else {
        return Vec::new();
    };
    let types = creatable_types(s);
    let space = s.nav.space().to_string();
    let def = types.iter().position(|t| t.key == menu.default_key);
    let mut all = Vec::new();
    if let Some(ix) = def {
        let t = &types[ix];
        all.push(Choice {
            key: t.key.clone(),
            icon: t.icon.clone(),
            name: t.name.clone(),
            group: Group::Default,
        });
    }
    all.push(Choice {
        key: "query".into(),
        icon: type_glyph(&s.store, "query", &space),
        name: "Query".into(),
        group: Group::Fixed,
    });
    all.push(Choice {
        key: "collection".into(),
        icon: type_glyph(&s.store, "collection", &space),
        name: "Collection".into(),
        group: Group::Fixed,
    });
    for (i, t) in types.iter().enumerate() {
        if Some(i) != def {
            all.push(Choice {
                key: t.key.clone(),
                icon: t.icon.clone(),
                name: t.name.clone(),
                group: Group::Type,
            });
        }
    }
    let q = menu.input.value().trim().to_lowercase();
    if q.is_empty() {
        return all;
    }
    all.into_iter()
        .filter(|c| c.name.to_lowercase().contains(&q) || c.key.to_lowercase().contains(&q))
        .collect()
}

/// Each choice's place in the list, in choice order: a new section at each change of group.
fn paths(choices: &[Choice]) -> Vec<IndexPath> {
    let (mut section, mut row) = (0, 0);
    choices
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if i > 0 && c.group != choices[i - 1].group {
                section += 1;
                row = 0;
            }
            row += 1;
            IndexPath::new(row - 1).section(section)
        })
        .collect()
}

/// The highlighted choice (-1: none, the first ↓ lands on the first).
fn active(s: &State, paths: &[IndexPath]) -> i32 {
    s.get::<CreateMenu>()
        .and_then(|m| m.list.highlighted())
        .and_then(|h| paths.iter().position(|p| *p == h))
        .map_or(-1, |i| i as i32)
}

/// ↑/↓: highlight the previous / next choice (wrapping) and scroll it into view.
fn step(s: &mut State, down: bool, tx: &mut Tx) {
    let paths = paths(&choices(s));
    let n = paths.len() as i32;
    if n == 0 {
        return;
    }
    let active = active(s, &paths);
    let next = if down {
        (active + 1) % n
    } else if active <= 0 {
        n - 1
    } else {
        active - 1
    };
    let path = paths[next as usize];
    s.slot::<CreateMenu>().list.set_highlighted(Some(path));
    tx.scroll_into_view(format!("{LIST}-item-{}-{}", path.section, path.row));
}

/// Enter: create the highlighted choice (the first when none is).
fn create_active(s: &mut State) {
    let choices = choices(s);
    let active = active(s, &paths(&choices)).max(0) as usize;
    if let Some(c) = choices.get(active) {
        sidebar_create(s, c.key.clone());
    }
}

/// A choice clicked in the list.
fn pick(s: &mut State, path: IndexPath) {
    let choices = choices(s);
    if let Some(ix) = paths(&choices).iter().position(|p| *p == path) {
        sidebar_create(s, choices[ix].key.clone());
    }
}

/// `sidebarCreate`: close the menu, then create in the active space (a failure alerts).
fn sidebar_create(s: &mut State, kind: String) {
    close(s);
    let ch = space_id(s);
    let task = match kind.as_str() {
        "collection" => roostr_core::create::create_collection(&ch, s),
        "query" => roostr_core::create::create_query(&ch, s),
        _ => roostr_core::create::create_typed(&kind, &ch, "", s),
    };
    s.rt.spawn(async move |cx| {
        if let Err(error) = task.await {
            cx.update(|s| crate::modal::alert(s, error.to_string()));
        }
    })
    .detach();
}

/// The type search's keys: ↑/↓ step, Enter creates, Escape closes.
fn search_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.mods != Mods::default() {
        return false;
    }
    match ev.key {
        Key::Down => step(s, true, tx),
        Key::Up => step(s, false, tx),
        Key::Enter => create_active(s),
        Key::Escape => close(s),
        _ => return false,
    }
    true
}

/// The type search changed: refilter, nothing highlighted.
fn search_changed(s: &mut State, _: &mut Tx) {
    s.slot::<CreateMenu>().list.set_highlighted(None);
}

fn menu_input(s: &mut State) -> &mut InputState {
    &mut s.slot::<CreateMenu>().input
}

fn menu_list(s: &mut State) -> &mut CommandState {
    &mut s.slot::<CreateMenu>().list
}

/// `.type-search`: the kit input at padding 5×8, 13px, bg `--bg`, 1px border,
/// radius 6 (h28). Callers add its margins (4×6 by default).
pub(crate) fn type_search(t: &Theme, input: Input) -> Input {
    input
        .h(28.0)
        .px(8.0)
        .py(5.0)
        .fill(t.bg)
        .border_color(t.border)
        .radius(radius::SM)
        .text_size(13.0)
}

/// The open menu: anchored under the element keyed `"create-btn"` on a desktop (a
/// press outside or Escape closes it), docked above the bottom bar on a phone (its ＋
/// toggles it).
pub(crate) fn overlay(s: &State, _cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) {
    let Some(menu) = s.get::<CreateMenu>().filter(|m| m.open) else {
        return;
    };
    let mobile = crate::is_mobile(s);
    let height = crate::window_height(s);
    let body = menu_body(s, menu, t, mobile);
    if mobile {
        // `.m-create-menu`: 14 from the sides, 72 from the bottom, scrolling inside
        // itself below the top of the screen; it rises 4 pt into place as it fades in.
        let mut dock = col()
            .key("m-create-menu")
            .pin(Some(14.0), None, Some(14.0), Some(72.0))
            .items_stretch();
        if height > 0.0 {
            dock = dock.max_h(height - 96.0);
        }
        layer.push(
            dock.child(
                body.shrink()
                    .min_h(0.0)
                    .transition(motion::appear(4.0))
                    .instant_changes(),
            ),
        );
    } else {
        let body = if height > 0.0 {
            body.max_h(height - 110.0)
        } else {
            body
        };
        let surface = overlay::dismissable(body, "create-menu", close, layer);
        // Its top 32 below the pill's top (the pill is 30 tall), right edges aligned.
        layer.push(overlay::anchored(
            surface,
            "create-btn",
            Placement::BottomEnd,
            2.0,
        ));
    }
}

/// The menu panel: the type search over the choice list.
fn menu_body(s: &State, menu: &CreateMenu, t: &Theme, mobile: bool) -> El {
    let choices = choices(s);
    let paths = paths(&choices);
    let highlighted = menu.list.highlighted();
    let mut list = Command::new(LIST, &menu.list)
        .searchable(false)
        .bordered(false)
        .max_h(1_000_000.0)
        .fill(Color::TRANSPARENT)
        .text_color(t.fg)
        .on_state(menu_list)
        .on_confirm(|s: &mut State, path: IndexPath, _: &mut Tx| pick(s, path))
        .on_cancel(|s: &mut State, _: &mut Tx| close(s))
        .empty(
            row()
                .key("create-empty")
                .px(8.0)
                .py(6.0)
                .child(text("No matching type").font(ty::ui(12.5)).color(t.muted)),
        );
    if mobile {
        // Rows round at 12 − the list's inset on a phone (`radius::md`-ish).
        list = list.radius(12.0);
    }
    let mut group = CommandGroup::new();
    for (i, c) in choices.iter().enumerate() {
        if i > 0 && c.group != choices[i - 1].group {
            list = list.group(std::mem::take(&mut group)).separator();
        }
        let selected = highlighted == Some(paths[i]);
        group = group.item(
            CommandItem::new()
                .label(c.name.clone())
                .child(choice_row(s, t, c, selected, mobile)),
        );
    }
    if !choices.is_empty() {
        list = list.group(group);
    }
    // Not `grow`: in the menu's auto-height column a grown list has flex-basis
    // 0 and collapses; it takes its rows' height and shrinks under `max_h`.
    let list = list
        .build(t)
        .shrink()
        .min_h(0.0)
        // Out past the list's own inset: rows sit on the menu's padding.
        .mx(-rems(0.25))
        .my(-rems(0.25));
    let search = type_search(t, Input::new("create-search", &menu.input))
        .placeholder("Search types…")
        .on_edit(menu_input)
        .on_change(search_changed)
        .on_key(search_key)
        .build(t)
        .mb(4.0);
    let panel = if mobile {
        // `.m-create-menu`: the docked sheet's width, radius 14, no shadow.
        col()
            .items_stretch()
            .fill(t.panel)
            .border(1.0, t.border)
            .radius(radius::MODAL)
            .padding(6.0)
            .w_full()
    } else {
        menu_panel(t).min_w(184.0)
    };
    panel.key("create-menu").hit().child(search).child(list)
}

/// A choice: its icon and name, and "Default" at the end for the space's Default type.
/// A phone's rows are bigger touch targets (15 pt, 10×11 around).
fn choice_row(s: &State, t: &Theme, c: &Choice, selected: bool, mobile: bool) -> El {
    let ink = if selected {
        t.kit.accent_foreground
    } else {
        t.fg
    };
    let size = if mobile { 15.0 } else { 13.0 };
    let mut r = row()
        .key(format!("create-{}", c.key))
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .gap(if mobile { 10.0 } else { 8.0 })
        .child(obj_icon(s, t, &c.icon))
        .child(text(c.name.clone()).font(ty::ui(size)).color(ink));
    if mobile {
        r = r.px(2.0).py(5.0);
    }
    if c.group == Group::Default {
        r = r.child(el().grow(1.0)).child(
            row()
                .pl(12.0)
                .child(text("Default").font(ty::ui(11.0)).color(t.muted)),
        );
    }
    r
}

//! Roostr on Rune, a look-and-feel prototype: spaces and their pins in a sidebar, a page as
//! editable blocks (Return makes a block, Backspace on an empty one removes it), and a query as
//! a sortable table. Data comes from the local glon daemon, read-only; edits stay in memory.
//!
//! `cargo run --release` opens it. `-- --snap page.png --size 1200x800` renders headless.

mod data;

use std::borrow::Cow;

use data::{Daemon, Kind, Object, Set, Space};
use rune::{AppSpec, Chrome, Cx, El, Key, KeyEvent, TextEdit, TextStyle, Tx, col, el, row, text, text_input};
use rune_kit::shell::{self, PaneState, Sidebar, SidebarRow, SidebarSection, SidebarToggle, SplitView, Toolbar};
use rune_kit::{Cell, ControlSize, Kit, SortDir, Table, TableColumn, TableState};

struct Block {
    kind: Kind,
    edit: TextEdit,
}

/// The object in the main pane.
#[derive(Default)]
struct Page {
    id: String,
    icon: String,
    type_name: String,
    title: TextEdit,
    props: Vec<(String, String)>,
    blocks: Vec<Block>,
    set: Option<Set>,
    table: TableState,
}

struct App {
    sidebar: PaneState,
    daemon: Option<Daemon>,
    spaces: Vec<Space>,
    space: usize,
    /// Objects of the sample data, when there is no daemon.
    samples: Vec<Object>,
    page: Page,
    pins_closed: bool,
}

fn init() -> App {
    // `-- sample` shows the built-in sample instead of the local vault.
    let daemon = if rune::extra_args().iter().any(|a| a == "sample") { None } else { Daemon::connect() };
    with(daemon)
}

fn with(daemon: Option<Daemon>) -> App {
    let (spaces, samples) = match &daemon {
        Some(d) => (d.spaces(), Vec::new()),
        None => data::sample(),
    };
    // Start in a space with pins; `-- space=<name>` picks one.
    let want = rune::extra_args().iter().find_map(|a| a.strip_prefix("space=").map(str::to_lowercase));
    let space = spaces
        .iter()
        .position(|s| want.as_deref().is_some_and(|w| s.name.to_lowercase().contains(w)))
        .or_else(|| spaces.iter().position(|s| !s.pins.is_empty()))
        .unwrap_or(0);
    let mut app = App { sidebar: PaneState::new(260.0).range(180.0, 420.0), daemon, spaces, space, samples, page: Page::default(), pins_closed: false };
    // `-- open=<pin name>` opens that pin instead of the first.
    let pick = rune::extra_args().iter().find_map(|a| a.strip_prefix("open=").map(str::to_lowercase));
    let pins = app.spaces.get(app.space).map(|s| &s.pins[..]).unwrap_or_default();
    let chosen = pick.and_then(|w| pins.iter().find(|p| p.name.to_lowercase().contains(&w))).or(pins.first()).map(|p| p.id.clone());
    if let Some(first) = chosen {
        open(&mut app, &first);
    }
    app
}

fn open(s: &mut App, id: &str) {
    let object = match &s.daemon {
        Some(d) => d.object(id),
        None => s.samples.iter().position(|o| o.id == id).map(|i| {
            let o = &s.samples[i];
            Object {
                id: o.id.clone(),
                name: o.name.clone(),
                icon: o.icon.clone(),
                type_name: o.type_name.clone(),
                props: o.props.clone(),
                body: o.body.iter().map(|b| data::Block { kind: b.kind, text: b.text.clone() }).collect(),
                set: o.set.as_ref().map(|set| Set {
                    columns: set.columns.clone(),
                    rows: set.rows.iter().map(|r| data::Row { id: r.id.clone(), cells: r.cells.clone() }).collect(),
                }),
            }
        }),
    };
    let Some(o) = object else { return };
    let mut blocks: Vec<Block> = o.body.into_iter().map(|b| Block { kind: b.kind, edit: TextEdit::new(b.text) }).collect();
    if blocks.is_empty() && o.set.is_none() {
        blocks.push(Block { kind: Kind::Paragraph, edit: TextEdit::new("") });
    }
    s.page = Page { id: o.id, icon: o.icon, type_name: o.type_name, title: TextEdit::new(o.name), props: o.props, blocks, set: o.set, table: TableState::new() };
}

// ── Blocks ──────────────────────────────────────────────────────

/// Return splits the block at the caret (a list item continues the list), Backspace in an
/// empty block removes it and moves to the one before.
fn block_key(s: &mut App, i: usize, ev: &KeyEvent) -> bool {
    let blocks = &mut s.page.blocks;
    match ev.key {
        Key::Enter if !ev.mods.shift => {
            let edit = &mut blocks[i].edit;
            let mut at = edit.focus.min(edit.text.len());
            while !edit.text.is_char_boundary(at) {
                at -= 1;
            }
            let rest = edit.text.split_off(at);
            edit.anchor = at;
            edit.focus = at;
            edit.focused = false;
            let kind = match blocks[i].kind {
                k @ (Kind::Bullet | Kind::Numbered) => k,
                Kind::Check(_) => Kind::Check(false),
                _ => Kind::Paragraph,
            };
            let mut next = TextEdit::new(rest);
            next.focused = true;
            blocks.insert(i + 1, Block { kind, edit: next });
            true
        }
        Key::Backspace if blocks[i].edit.text.is_empty() && i > 0 && blocks.len() > 1 => {
            blocks.remove(i);
            let prev = &mut blocks[i - 1].edit;
            prev.anchor = prev.text.len();
            prev.focus = prev.text.len();
            prev.focused = true;
            true
        }
        _ => false,
    }
}

fn input(edit: &TextEdit, style: TextStyle, ink: rune::Color, i: usize, k: &Kit) -> El {
    text_input(edit, style)
        .key(format!("block-{i}"))
        .w_full()
        .color(ink)
        .caret(ink, k.selection())
        .placeholder(if edit.focused { "Type something…" } else { "" }, k.c.text_faint)
        .on_edit(move |s: &mut App| &mut s.page.blocks[i].edit)
        .on_input_key(move |s: &mut App, ev: &KeyEvent, _: &mut Tx| block_key(s, i, ev))
}

fn block(s: &App, i: usize, number: usize, k: &Kit) -> El {
    let b = &s.page.blocks[i];
    let c = &k.c;
    let body = TextStyle { size: 15.0, line_height: 24.0, ..k.ty.body };
    let heading = |size: f32| TextStyle { size, weight: 650.0, line_height: size * 1.3, ..k.ty.body };
    let lead = |m: El, e: El| row().w_full().gap(6.0).items_start().child(col().w(22.0).pt(2.0).items_center().child(m)).child(e);
    match b.kind {
        Kind::Paragraph => input(&b.edit, body, c.text_base, i, k),
        Kind::H1 => col().w_full().pt(14.0).child(input(&b.edit, heading(26.0), c.text_base, i, k)),
        Kind::H2 => col().w_full().pt(10.0).child(input(&b.edit, heading(21.0), c.text_base, i, k)),
        Kind::H3 => col().w_full().pt(6.0).child(input(&b.edit, heading(17.0), c.text_base, i, k)),
        Kind::Quote => row()
            .w_full()
            .gap(12.0)
            .child(el().w(3.0).h_full().min_h(24.0).radius(1.5).fill(c.border_strong))
            .child(input(&b.edit, TextStyle { size: 16.0, ..body }, c.text_muted, i, k)),
        Kind::Code => col().w_full().padding(12.0).radius(k.radius.md).fill(c.layer_02).child(input(&b.edit, k.ty.mono, c.text_base, i, k)),
        Kind::Bullet => lead(text("•").font(body).color(c.text_muted), input(&b.edit, body, c.text_base, i, k)),
        Kind::Numbered => lead(text(format!("{number}.")).font(body).color(c.text_muted), input(&b.edit, body, c.text_base, i, k)),
        Kind::Check(done) => {
            let mark = el()
                .key(format!("check-{i}"))
                .role("checkbox")
                .checked(done)
                .label("Done")
                .size(16.0, 16.0)
                .mt(2.0)
                .radius(4.0)
                .border(1.5, if done { c.text_accent } else { c.border_strong })
                .fill(if done { c.text_accent } else { rune::Color::TRANSPARENT })
                .center()
                .child(if done { text("✓").font(TextStyle { size: 11.0, weight: 700.0, ..body }).color(c.white) } else { el() })
                .on_press(move |s: &mut App| {
                    if let Kind::Check(d) = &mut s.page.blocks[i].kind {
                        *d = !*d;
                    }
                });
            lead(mark, input(&b.edit, body, if done { c.text_faint } else { c.text_base }, i, k))
        }
        Kind::Divider => col().w_full().py(10.0).child(el().w_full().h(1.0).fill(c.border_base)),
    }
}

// ── Page ────────────────────────────────────────────────────────

fn chip(label: &str, k: &Kit) -> El {
    row().px(8.0).py(3.0).radius(6.0).fill(k.c.layer_02).child(text(label.to_owned()).font(k.ty.text_small).color(k.c.text_muted))
}

fn page(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let p = &s.page;
    let title = text_input(&p.title, TextStyle { size: 34.0, weight: 700.0, line_height: 42.0, ..k.ty.body })
        .key("title")
        .w_full()
        .color(c.text_base)
        .caret(c.text_base, k.selection())
        .placeholder("Untitled", c.text_faint)
        .on_edit(|s: &mut App| &mut s.page.title);
    let mut head = col().w_full().gap(10.0);
    if !p.icon.is_empty() && p.icon.chars().count() <= 8 {
        head = head.child(text(p.icon.clone()).font(TextStyle { size: 44.0, line_height: 54.0, ..k.ty.body }));
    }
    head = head.child(title).child(row().gap(6.0).child(chip(&type_label(&p.type_name), k)));
    if !p.props.is_empty() {
        let props = p.props.iter().fold(col().w_full().gap(2.0).pt(6.0), |list, (name, value)| {
            list.child(
                row()
                    .w_full()
                    .gap(12.0)
                    .py(4.0)
                    .child(text(name.clone()).font(k.ty.ui).color(c.text_muted).w(140.0))
                    .child(text(value.clone()).font(k.ty.ui).color(c.text_base).grow(1.0).truncate()),
            )
        });
        head = head.child(props);
    }
    let mut number = 0;
    let body = (0..p.blocks.len()).fold(col().w_full().gap(4.0).pt(18.0), |list, i| {
        number = if p.blocks[i].kind == Kind::Numbered { number + 1 } else { 0 };
        list.child(block(s, i, number, k))
    });
    let mut column = col().key(format!("page-{}", p.id)).w_full().max_w(if p.set.is_some() { 1100.0 } else { 760.0 }).px(48.0).pt(36.0).pb(120.0).child(head);
    if p.set.is_some() {
        column = column.child(col().w_full().pt(18.0).h(560.0).child(query_table(s, k)));
    }
    if !p.blocks.is_empty() {
        column = column.child(body);
    }
    col().key("page-scroll").w_full().h_full().scroll(rune::ScrollSpec { stick_bottom: false }).items_center().child(column)
}

fn type_label(t: &str) -> String {
    let mut c = t.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect::<String>().replace('_', " ")).unwrap_or_else(|| "Object".into())
}

// ── Query ───────────────────────────────────────────────────────

fn set(s: &App) -> &Set {
    s.page.set.as_ref().expect("a query page")
}

fn query_table(s: &App, k: &Kit) -> El {
    let columns = set(s).columns.clone();
    let table = columns
        .iter()
        .enumerate()
        .fold(
            Table::new(|s: &App| &s.page.table, |s: &mut App| &mut s.page.table, |s: &App| set(s).rows.len(), |s: &App, i| Cow::Borrowed(set(s).rows[i].id.as_str())).name("Query"),
            |t, (n, (key, name))| t.column(TableColumn::new(key.clone(), name.clone()).flex(if n == 0 { 2.0 } else { 1.0 }).min(90.0).sortable()),
        )
        .cell(|s: &App, i, col| {
            let set = set(s);
            let at = set.columns.iter().position(|(k, _)| k == col).unwrap_or(0);
            let value = set.rows[i].cells.get(at).cloned().unwrap_or_default();
            if at == 0 { Cell::text(value) } else { Cell::text(value).muted() }
        })
        .on_sort(|s: &mut App, col, dir, _| sort(s, col, dir))
        .on_open(|s: &mut App, id, _| {
            let id = id.to_owned();
            open(s, &id);
        })
        .build(s, k)
        .grow(1.0)
        .min_h(0.0);
    let count = text(format!("{} objects", set(s).rows.len())).font(k.ty.text_small).color(k.c.text_faint);
    col()
        .w_full()
        .h_full()
        .gap(8.0)
        .child(col().key("query-frame").w_full().grow(1.0).min_h(0.0).radius(k.radius.md).border(1.0, k.c.border_base).clip_rounded(k.radius.md).child(table))
        .child(count)
}

fn sort(s: &mut App, col: &str, dir: SortDir) {
    let Some(set) = s.page.set.as_mut() else { return };
    let at = set.columns.iter().position(|(k, _)| k == col).unwrap_or(0);
    set.rows.sort_by(|a, b| dir.apply(rune_kit::natural_cmp(a.cells.get(at).map_or("", |s| s), b.cells.get(at).map_or("", |s| s))));
}

// ── Shell ───────────────────────────────────────────────────────

fn emoji(icon: &str, fallback: &str, k: &Kit) -> El {
    // Image icons arrive as data, not emoji: those show the fallback.
    let shown = if icon.is_empty() || icon.chars().count() > 8 { fallback.to_owned() } else { icon.to_owned() };
    row().size(18.0, 18.0).center().child(text(shown).font(TextStyle { size: 13.0, ..k.ty.ui }).color(k.c.text_muted))
}

fn sidebar(s: &App, k: &Kit) -> El {
    let spaces = s.spaces.iter().enumerate().map(|(i, sp)| {
        SidebarRow::new(sp.name.clone())
            .avatar(emoji(&sp.icon, &sp.name.chars().next().unwrap_or('•').to_string(), k))
            .height(30.0)
            .selected(i == s.space)
            .on_select(move |s: &mut App, _| {
                s.space = i;
                if let Some(id) = s.spaces[i].pins.first().map(|p| p.id.clone()) {
                    open(s, &id);
                }
            })
            .build(k)
            .key(format!("space-{}", sp.id))
    });
    let pins: Vec<El> = s
        .spaces
        .get(s.space)
        .map(|sp| {
            sp.pins
                .iter()
                .map(|p| {
                    let id = p.id.clone();
                    SidebarRow::new(p.name.clone())
                        .avatar(emoji(&p.icon, "📄", k))
                        .height(30.0)
                        .selected(s.page.id == p.id)
                        .on_select(move |s: &mut App, _| open(s, &id))
                        .build(k)
                        .key(format!("pin-{}", p.id))
                })
                .collect()
        })
        .unwrap_or_default();
    let n = pins.len();
    Sidebar::new()
        .label("Roostr")
        .section(SidebarSection::new("Pinned").count(n).collapsed(s.pins_closed).on_toggle(|s: &mut App, _| s.pins_closed ^= true).rows(pins))
        .section(SidebarSection::new("Spaces").count(s.spaces.len()).rows(spaces))
        .build(k)
}

fn view(s: &App, cx: &mut Cx<App>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let space = s.spaces.get(s.space).map_or("Roostr", |sp| sp.name.as_str());
    let title = if s.page.title.text.is_empty() { "Untitled".to_owned() } else { s.page.title.text.clone() };
    let source = if s.daemon.is_some() { "Local vault · read-only" } else { "Sample data" };
    let toolbar = Toolbar::new()
        .title(format!("{space}  /  {title}"))
        .trailing(text(source).font(k.ty.text_small).color(k.c.text_faint));
    SplitView::new(page(s, &k))
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar(s, &k))
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(18.0, 10.0)
        .sidebar_toggle(SidebarToggle::new().size(ControlSize::Large).past_lights(16.0).edge(18.0).gap(4.0).in_header())
        .reduced_motion(cx.is_reduced_motion())
        .build(&k)
}

fn on_key(s: &mut App, ev: &KeyEvent, _: &mut Tx) -> bool {
    if shell::is_sidebar_toggle(ev) {
        s.sidebar.toggle();
        return true;
    }
    false
}

fn main() {
    let chrome = Chrome { full_size_content: true, traffic_lights: Some((16.0, 25.0)), drag_regions: true };
    rune::App::new(AppSpec { on_key: Some(on_key), ..AppSpec::new("Roostr (Rune prototype)", (1240.0, 820.0), init, view) }).chrome(chrome).run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Mods, Runtime};

    fn key(rt: &mut Runtime<App>, key: Key) {
        rt.key(KeyEvent { key, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn typing_in_a_block_edits_it_and_return_starts_the_next() {
        let env = Env { system_dark: true, scale: 2.0 };
        let mut rt = Runtime::new(with(None), view, (1240.0, 820.0), env);
        rt.frame(0.0);
        assert_eq!(rt.state.page.title.text, "Launch plan");
        let (x, y) = rt.find("block-1").expect("the paragraph under Goals");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        key(&mut rt, Key::End);
        key(&mut rt, Key::Text(" Soon.".into()));
        assert!(rt.state.page.blocks[1].edit.text.ends_with("first. Soon."), "{}", rt.state.page.blocks[1].edit.text);
        let before = rt.state.page.blocks.len();
        key(&mut rt, Key::Enter);
        key(&mut rt, Key::Text("A new block".into()));
        assert_eq!(rt.state.page.blocks.len(), before + 1);
        assert_eq!(rt.state.page.blocks[2].edit.text, "A new block");
        // Backspace in an emptied block takes it away again.
        for _ in 0.."A new block".len() {
            key(&mut rt, Key::Backspace);
        }
        key(&mut rt, Key::Backspace);
        assert_eq!(rt.state.page.blocks.len(), before);
    }

    #[test]
    fn a_checkbox_toggles_and_the_query_opens_as_a_table() {
        let env = Env { system_dark: false, scale: 2.0 };
        let mut rt = Runtime::new(with(None), view, (1240.0, 820.0), env);
        rt.frame(0.0);
        let (x, y) = rt.find("check-3").expect("the second task");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(matches!(rt.state.page.blocks[3].kind, Kind::Check(true)));
        let (x, y) = rt.find("pin-q1").expect("the query pin");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.page.set.as_ref().map(|s| s.rows.len()), Some(3));
    }
}

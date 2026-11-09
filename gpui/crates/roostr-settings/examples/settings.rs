//! Exercise every roostr-settings view against the test daemon.
//!
//! `cargo run -p roostr-settings --example settings -- [settings|space|install|gate] [space-id]`
//! starts on that view (default: the space page of the first space);
//! `--palette light|system|dark` picks (and remembers) the theme.

use roostr_core::State;
use roostr_settings::{Btn, open_settings};
use roostr_ui::Theme;
use rune::{Cursor, El, ScrollSpec, col, row};

#[derive(Clone, Copy, PartialEq, Default)]
enum Page {
    #[default]
    Space,
    Install,
    Gate,
}

/// The example's own state.
#[derive(Default)]
struct Demo {
    page: Page,
    /// The space shown on the space page ("" = the first space, once loaded).
    space: String,
}

fn register(s: &mut State) {
    roostr_settings::register(s);
    let args = rune::extra_args();
    let mut args = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with("--"));
    let mode = args.next().unwrap_or("space");
    let extra = args.next().map(str::to_owned).unwrap_or_default();
    if let Some(i) = rune::extra_args().iter().position(|a| a == "--palette")
        && let Some(name) = rune::extra_args().get(i + 1)
    {
        s.ui.set_theme_name(name);
    }
    let demo = s.slot::<Demo>();
    demo.page = match mode {
        "install" => Page::Install,
        "gate" => Page::Gate,
        _ => Page::Space,
    };
    if mode != "settings" {
        demo.space = extra;
    }
    if mode == "settings" {
        open_settings(s);
    }
    s.on_poll(prepare);
}

/// Opens the shown space's settings and the install poll once the store knows the spaces.
fn prepare(s: &mut State) -> bool {
    if !s.store.loaded {
        return false;
    }
    let demo = s.slot::<Demo>();
    let page = demo.page;
    if demo.space.is_empty() {
        match s.store.channels.first() {
            Some(space) => s.slot::<Demo>().space = space.id.clone(),
            None => return false,
        }
    }
    let space = s.slot::<Demo>().space.clone();
    roostr_settings::open_space_manage(s, &space);
    if page == Page::Install {
        roostr_settings::start_install_poll(s);
    }
    false
}

fn tab(t: &Theme, key: &'static str, label: &'static str, page: Option<Page>, active: bool) -> El {
    let b = Btn::small(t, key, label)
        .radius(7.0)
        .on_press(move |s: &mut State| match page {
            Some(page) => s.slot::<Demo>().page = page,
            None => open_settings(s),
        });
    let b = if active {
        b.colors(Some(t.hover), Some(t.border), t.fg)
    } else {
        b.colors(None, Some(t.border), t.fg)
    };
    b.build(t).cursor(Cursor::Pointer)
}

fn view(s: &State, cx: &mut rune::Cx<State>) -> El {
    let t = roostr_ui::theme(roostr_settings::theme_pref(s), cx);
    let mut layer = Vec::new();
    let demo = s.get::<Demo>();
    let page = demo.map(|d| d.page).unwrap_or_default();
    let space = demo.map(|d| d.space.clone()).unwrap_or_default();
    let body = match page {
        Page::Space => col()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col()
                    .w_full()
                    .max_w(920.0)
                    .self_center()
                    .pt(24.0)
                    .pb(80.0)
                    .child(roostr_settings::space_manage(s, cx, t, &mut layer, &space)),
            ),
        Page::Install => col().grow(1.0).padding(24.0).child(
            col()
                .w_full()
                .max_w(920.0)
                .self_center()
                .child(roostr_settings::install_computer(s, cx, t)),
        ),
        Page::Gate => col()
            .grow(1.0)
            .min_h(0.0)
            .child(roostr_settings::identity_gate(s, cx, t, &mut layer)),
    };
    roostr_settings::settings_modal(s, cx, t, &mut layer);
    let tabs = row()
        .gap(8.0)
        .padding(10.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
        .child(tab(t, "open-settings", "Settings…", None, false))
        .child(tab(
            t,
            "show-space",
            "Space settings",
            Some(Page::Space),
            page == Page::Space,
        ))
        .child(tab(
            t,
            "show-install",
            "Install computer",
            Some(Page::Install),
            page == Page::Install,
        ))
        .child(tab(
            t,
            "show-gate",
            "Identity gate",
            Some(Page::Gate),
            page == Page::Gate,
        ));
    let base = col().w_full().h_full().fill(t.bg).child(tabs).child(body);
    roostr_ui::root(base, layer)
}

fn main() {
    roostr_host::run_example("Settings", register, view);
}

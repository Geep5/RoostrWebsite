//! Editor example: `cargo run -p roostr-editor --example editor -- [object-id] [--readonly]`.
//!
//! Opens the object's body in the block editor (default: the seeded
//! "Editor showcase" page with every block style) under its name, as an
//! object page shows it. A File object id opens its File page instead.
//! `--readonly` shows the body without editing. Headless:
//! `-- <id> --snap out.png --size 1280x860`, and Rune's `--script` drives it
//! (clicks, typing, keys) on the virtual clock.

use roostr_core::State;
use rune::{Cx, El, ScrollSpec, col, el, text};

/// The object the example shows and whether it is a File.
#[derive(Default)]
struct Shown {
    id: String,
    file: bool,
}

fn object_id() -> String {
    rune::extra_args()
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "seed-page-showcase".into())
}

fn register(s: &mut State) {
    roostr_editor::register(s);
    roostr_properties::register(s);
    let id = object_id();
    let read_only = rune::extra_args().iter().any(|a| a == "--readonly");
    s.slot::<Shown>().id = id.clone();
    // A File object gets its File page; anything else its body.
    let fetch = s.store.fetch_object(&id);
    s.rt.spawn(async move |cx| {
        let file = fetch.await.is_ok_and(|o| o.type_key == "file");
        cx.update(|s| {
            s.slot::<Shown>().file = file;
            if file {
                roostr_editor::open_file(s, &id);
            } else {
                roostr_editor::open(s, &id, read_only);
            }
        });
    })
    .detach();
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(roostr_ui::ThemePref::Dark, cx);
    let Some(shown) = s.get::<Shown>() else {
        return el();
    };
    let mut layer = Vec::new();
    let name = s
        .store
        .summary(&shown.id)
        .map(|o| o.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Untitled".into());
    let body = if shown.file {
        roostr_editor::file_view(s, cx, t, &mut layer, &shown.id)
    } else {
        roostr_editor::view(s, cx, t, &mut layer, &shown.id)
    };
    let page = col()
        .w_full()
        .max_w(920.)
        .px(32.)
        .pt(24.)
        .pb(80.)
        .child(
            el().ml(roostr_editor::GUTTER_W).mb(12.).child(
                text(name)
                    .font(rune::TextStyle::new(34., 750.))
                    .color(t.fg)
                    .wrap(),
            ),
        )
        .child(body);
    let scroll = col()
        .key("page-scroll")
        .w_full()
        .h_full()
        .min_h(0.)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .items_center()
        .child(page);
    roostr_ui::root(scroll, layer)
}

fn main() {
    roostr_host::run_example("Editor", register, view);
}

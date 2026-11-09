//! The roostr-ui gallery: palettes, type, icons and every component, in the system appearance
//! (`--theme dark|light` picks it headless).
//!
//! `cargo run -p roostr-ui --example gallery -- --snap /tmp/gallery-dark.png --theme dark`

mod basics;
mod forms;
mod foundation;
mod overlays;
mod pickers;

use roostr_ui::{Theme, ThemePref, ty};
use rune::{AppSpec, Cx, El, ScrollSpec, col, row, text};

/// The gallery's state: one part per section.
#[derive(Default)]
pub struct Gallery {
    pub basics: basics::State,
    pub forms: forms::State,
    pub overlays: overlays::State,
    pub pickers: pickers::State,
    /// Image icons (`emoji_or_image_icon`), made once Rune hands over its waker.
    pub images: Option<roostr_ui::Images>,
}

fn init() -> Gallery {
    Gallery::default()
}

fn setup(s: &mut Gallery, waker: rune::Waker) {
    let images = roostr_ui::Images::new(waker, 2.0);
    // Decode the sample icon now, so a one-frame snapshot shows it.
    if let Some(image) = images.image(foundation::SAMPLE_ICON, 24.0) {
        let _ = image.wait();
    }
    s.images = Some(images);
}

/// A titled card holding one group of components.
pub fn section(t: &Theme, title: &str, body: El) -> El {
    col()
        .gap(12.0)
        .padding(16.0)
        .radius(roostr_ui::radius::PANE)
        .fill(t.bg)
        .border(1.0, t.border)
        .child(
            text(title.to_owned())
                .font(ty::ui(11.0).semibold())
                .color(t.muted),
        )
        .child(body)
}

/// A label over a component, for the gallery's rows.
pub fn labeled(t: &Theme, label: &str, body: El) -> El {
    col()
        .gap(6.0)
        .child(text(label.to_owned()).font(ty::ui(11.0)).color(t.muted))
        .child(body)
}

use roostr_ui::ty::Weights as _;

fn view(s: &Gallery, cx: &mut Cx<Gallery>) -> El {
    let t = roostr_ui::theme(ThemePref::System, cx);
    let mut layer = Vec::new();
    let column = || col().w_frac(1.0 / 3.0).shrink().gap(16.0);
    let columns = row()
        .w_full()
        .gap(16.0)
        .items_start()
        .child(column().child(basics::view(s, t, &mut layer)))
        .child(
            column()
                .child(foundation::view(s, t))
                .child(forms::view(s, t, &mut layer)),
        )
        .child(
            column()
                .child(overlays::view(s, t, &mut layer))
                .child(pickers::view(s, t, &mut layer)),
        );
    let page = col()
        .key("gallery-scroll")
        .w_full()
        .h_full()
        .fill(t.gutter)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(col().w_full().padding(16.0).child(columns));
    roostr_ui::root(page, layer)
}

fn main() {
    rune::run(AppSpec {
        fonts: roostr_ui::fonts::FONTS,
        families: roostr_ui::fonts::FAMILIES,
        setup: Some(setup),
        ..AppSpec::new("roostr-ui gallery", (2440.0, 2480.0), init, view)
    });
}

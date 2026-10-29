//! `/`: the marketing landing (`src/routes/+page.svelte`). One screen, never
//! a scroll: every size scales with the viewport (`vw`/`vh` = this view's
//! bounds), including the page's three `@media` layouts.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::nav::{self, Route, SitePage};
use roostr_core::theme::fonts;
use roostr_core::ui::{self, SpacedText};

use crate::assets::{GITHUB_MARK, SiteImage};

/// A line in `family` at `size` with CSS `letter-spacing: <spacing>px`.
fn spaced(family: &'static str, size: f32, spacing: f32) -> SpacedText {
    SpacedText::new(spacing / size)
        .family(family)
        .size(px(size))
}

const GITHUB_URL: &str = "https://github.com/Geep5/Roostr";

fn cream() -> Hsla {
    rgb(0xedcf7f).into()
}
fn ink() -> Hsla {
    rgb(0x1b1b1b).into()
}
fn red() -> Hsla {
    rgb(0xca3433).into()
}
/// Text on red (`#fff6e3`).
fn paper() -> Hsla {
    rgb(0xfff6e3).into()
}

/// CSS `clamp(min, value, max)`.
fn clamp(min: f32, value: f32, max: f32) -> f32 {
    value.max(min).min(max)
}

/// The CSS `ease` timing function, `cubic-bezier(0.25, 0.1, 0.25, 1)`.
fn ease(t: f32) -> f32 {
    let (x1, y1, x2, y2) = (0.25f32, 0.1f32, 0.25f32, 1.0f32);
    let bezier = |a: f32, b: f32, s: f32| {
        3. * a * s * (1. - s).powi(2) + 3. * b * s * s * (1. - s) + s.powi(3)
    };
    // Solve x(s) = t by bisection, then evaluate y(s).
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.;
        if bezier(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(y1, y2, (lo + hi) / 2.)
}

fn mix(from: Hsla, to: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(from), Rgba::from(to));
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }
    .into()
}

/// Hover state of one `.btn` (its 0.15s transform/background transition).
#[derive(Clone, Copy, Default)]
struct Hover {
    on: bool,
    /// Bumped on every change; restarts the transition.
    generation: usize,
}

/// The responsive sizes of one layout pass.
struct Metrics {
    w: f32,
    h: f32,
    /// `@media (max-width: 880px)`.
    narrow: bool,
    /// `@media (max-width: 880px) and (max-height: 700px)`.
    short: bool,
    /// `@media (max-width: 400px)`.
    tiny: bool,
    /// Too short for the one-screen layout (an embed, a small window): no
    /// rooster, the parts that don't fit are left out, and whatever still
    /// doesn't fit scrolls instead of overlapping.
    compact: bool,
    /// The three feature cards fit.
    features: bool,
    /// The footer fits.
    footer: bool,
    /// The lede paragraph fits.
    lede: bool,
    /// The "Open source · …" line beside the button fits.
    tagline: bool,
}

impl Metrics {
    fn vw(&self, f: f32) -> f32 {
        self.w * f / 100.
    }
    fn vh(&self, f: f32) -> f32 {
        self.h * f / 100.
    }
}

pub(crate) struct LandingPage {
    /// This view's bounds from the last paint (the page's viewport).
    viewport: Rc<Cell<Option<Size<Pixels>>>>,
    hovers: HashMap<&'static str, Hover>,
}

impl LandingPage {
    pub(crate) fn new() -> Self {
        Self {
            viewport: Rc::default(),
            hovers: HashMap::new(),
        }
    }

    /// `.btn` with its hover lift and colour transition.
    #[allow(clippy::too_many_arguments)]
    fn button(
        &self,
        name: &'static str,
        rest: Hsla,
        hover: Hsla,
        text: Hsla,
        padding: (f32, f32),
        font_size: f32,
        content: AnyElement,
        on_click: impl Fn(&mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.hovers.get(name).copied().unwrap_or_default();
        let animate = state.generation > 0;
        div()
            .id(name)
            .relative()
            .flex()
            .items_center()
            .gap(px(9.))
            .flex_none()
            .cursor_pointer()
            .rounded_full()
            .border_2()
            .px(px(padding.1))
            .py(px(padding.0))
            .text_size(px(font_size))
            .font_weight(FontWeight(650.))
            .text_color(text)
            .child(content)
            .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                let entry = this.hovers.entry(name).or_default();
                if entry.on != *on {
                    entry.on = *on;
                    entry.generation += 1;
                    cx.notify();
                }
            }))
            .on_click(move |_, _, cx| on_click(cx))
            .with_animation(
                (name, state.generation),
                Animation::new(Duration::from_millis(150)).with_easing(ease),
                move |el, t| {
                    let progress = match (animate, state.on) {
                        (false, _) => 0.,
                        (true, true) => t,
                        (true, false) => 1. - t,
                    };
                    let color = mix(rest, hover, progress);
                    el.bg(color).border_color(color).top(px(-2. * progress))
                },
            )
            .into_any_element()
    }

    fn header(&self, m: &Metrics, cx: &mut Context<Self>) -> impl IntoElement {
        let (btn_pad, btn_size) = if m.narrow {
            ((9., 18.), 14.)
        } else {
            ((12., 24.), 15.)
        };
        let gh_pad = if m.tiny { (9., 11.) } else { btn_pad };
        let gh_content = div()
            .flex()
            .items_center()
            .gap(px(9.))
            .line_height(relative(1.55))
            .child(img(GITHUB_MARK.clone()).size(px(17.)).flex_none())
            .when(!m.tiny, |el| el.child("GitHub"))
            .into_any_element();
        let pad = if m.narrow {
            clamp(8., m.vh(1.6), 16.)
        } else {
            clamp(12., m.vh(2.6), 26.)
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .py(px(pad))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .id("wordmark")
                            .cursor_pointer()
                            .on_click(|_, _, cx| nav::navigate(Route::Site(SitePage::Landing), cx))
                            .child(
                                spaced(fonts::ARCHIVO_BLACK, 26., -0.5)
                                    .line_height(px(26. * 1.55))
                                    .run("roostr", Some(ink()))
                                    .run(".", Some(red())),
                            ),
                    )
                    // A narrow embed has no room for the pill beside Guide.
                    .when(!m.tiny, |row| {
                        row.child(
                            div()
                                .id("alpha")
                                .relative()
                                .top(px(1.))
                                .rounded_full()
                                .bg(red())
                                .pt(px(3.))
                                .pb(px(2.))
                                .px(px(9.))
                                .child(
                                    spaced(fonts::INTER, 11., 1.5)
                                        .weight(FontWeight(750.))
                                        .line_height(px(11. * 1.4))
                                        .run("ALPHA", Some(paper())),
                                )
                                .tooltip(ui::tooltip(
                                    "Roostr is in alpha: expect rough edges and changes",
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(if m.tiny { 14. } else { 22. }))
                    .child(
                        div()
                            .id("nav-guide")
                            .cursor_pointer()
                            .text_size(px(15.))
                            .font_weight(FontWeight(650.))
                            .text_color(ink())
                            .hover(|s| s.text_color(red()))
                            .child("Guide")
                            .on_click(|_, _, cx| nav::navigate(Route::Site(SitePage::Guide), cx)),
                    )
                    .child(self.button(
                        "btn-github",
                        ink(),
                        red(),
                        cream(),
                        gh_pad,
                        btn_size,
                        gh_content,
                        |cx| cx.open_url(GITHUB_URL),
                        cx,
                    )),
            )
    }

    fn hero_copy(
        &self,
        m: &Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (h1_size, lede_size, lede_lh) = if m.narrow {
            (
                clamp(28., m.vw(10.).min(m.vh(6.)), 60.),
                if m.short {
                    13.
                } else {
                    clamp(13.5, m.vh(1.9), 16.)
                },
                1.45,
            )
        } else {
            (
                clamp(36., m.vw(6.8).min(m.vh(9.)).min(m.vh(21.6) - 96.), 96.),
                clamp(15., m.vh(2.1), 19.),
                1.55,
            )
        };
        let h1_line = |text: &str, dot: bool| {
            let line = spaced(fonts::ARCHIVO_BLACK, h1_size, -1.5)
                .line_height(px(h1_size * 0.98))
                .run(text.to_string(), Some(ink()));
            if dot {
                line.run(".", Some(red()))
            } else {
                line
            }
        };
        let lines = if m.narrow {
            vec![h1_line("YOUR TEAM,", false), h1_line("HUMAN AND NOT", true)]
        } else {
            vec![
                h1_line("YOUR TEAM,", false),
                h1_line("HUMAN", false),
                h1_line("AND NOT", true),
            ]
        };

        // The lede's optical size follows its font size (Chrome's
        // `font-optical-sizing: auto`); see `INTER_INSTANCES`.
        let lede_family = SharedString::from(format!(
            "Inter Lede {}",
            lede_size.round().clamp(15., 19.) as u32
        ));
        // `.lede { max-width: 46ch }`: ch is the advance of "0" in the lede's font.
        let lede_font = Font {
            weight: FontWeight(450.),
            ..font(lede_family.clone())
        };
        let ch = window
            .text_system()
            .ch_advance(window.text_system().resolve_font(&lede_font), px(lede_size))
            .unwrap_or(px(lede_size * 0.6));

        // The headline says "people + agents" now: no eyebrow above it.
        let copy = div().flex().flex_col();
        let (btn_pad, btn_size) = if m.narrow {
            ((9., 18.), 14.)
        } else {
            ((12., 24.), 15.)
        };
        copy.child(
            div()
                .flex()
                .flex_col()
                .mb(px(clamp(10., m.vh(2.4), 24.)))
                .children(lines),
        )
        .when(m.lede, |copy| {
            copy.child(
                div()
                    .max_w(ch * 46.)
                    .mb(px(clamp(12., m.vh(3.), 30.)))
                    .text_size(px(lede_size))
                    .line_height(relative(lede_lh))
                    .font_family(lede_family)
                    .font_weight(FontWeight(450.))
                    .child(
                        "Run your company, your household or a side project with agents that work like teammates. \
                         Each has a job, a schedule and its own logins, and works on your real records, \
                         like emails, people and tasks, right where you can see and fix what it did.",
                    ),
            )
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(14.))
                .gap_y(px(if m.narrow { 8. } else { 14. }))
                .child(self.button(
                    "btn-open",
                    red(),
                    ink(),
                    paper(),
                    btn_pad,
                    btn_size,
                    div().line_height(relative(1.55)).child("Open Roostr").into_any_element(),
                    |cx| nav::navigate(Route::Home, cx),
                    cx,
                ))
                .when(m.tagline, |row| {
                    row.child(
                        div()
                            .max_w_full()
                            .text_size(px(if m.narrow { 12. } else { 14. }))
                            .font_weight(FontWeight(550.))
                            .opacity(0.75)
                            .child("Open source · end-to-end encrypted · agents run on your own computers"),
                    )
                }),
        )
    }

    /// `#rooster`: the animated hero, or its poster under reduced motion.
    /// The animation is 60 frames of 900×900 and takes a while to decode
    /// (about 12s in debug builds), so its first frame stands in until it is
    /// ready, and the poster if it fails to decode.
    fn hero_art(&self, max_width: f32, cx: &App) -> impl IntoElement {
        let still = |image: SiteImage| {
            move || {
                img(image.source())
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element()
            }
        };
        let hero = if cx.reduce_motion() {
            img(SiteImage::RoosterPoster.source())
        } else {
            img(SiteImage::RoosterAnim.source())
                .with_loading(still(SiteImage::RoosterFirstFrame))
                .with_fallback(still(SiteImage::RoosterPoster))
        };
        div()
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .min_h_0()
            .child(
                hero.id("rooster")
                    .w(px(max_width.min(560.)))
                    .h_full()
                    .max_h(px(900.))
                    .object_fit(ObjectFit::Contain),
            )
    }

    fn hero(
        &self,
        m: &Metrics,
        inner_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pad_top = clamp(6., m.vh(1.6), 24.);
        let pad_bottom = clamp(22., m.vh(4.5), 48.);
        if m.compact {
            // Natural height, top to bottom: the copy alone.
            return div()
                .flex_none()
                .pt(px(pad_top))
                .pb(px(pad_bottom))
                .child(self.hero_copy(m, window, cx))
                .into_any_element();
        }
        if m.narrow {
            // A phone column scrolls: the rooster gets a fixed share of the
            // screen above the copy instead of whatever height is left, which
            // a longer lede could take to nothing and overlap the header.
            let gap = clamp(6., m.vh(1.5), 16.);
            let art_h = clamp(140., m.vh(24.), 280.);
            return div()
                .flex_none()
                .pt(px(pad_top))
                .pb(px(pad_bottom))
                .flex()
                .flex_col()
                .gap(px(gap))
                .when(!m.short, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .h(px(art_h))
                            .child(self.hero_art(inner_width, cx)),
                    )
                })
                .child(self.hero_copy(m, window, cx))
                .into_any_element();
        }
        let hero = div().flex_1().min_h_0().pt(px(pad_top)).pb(px(pad_bottom));
        // `grid-template-columns: minmax(540px, 1.1fr) 1fr; gap: 40px`.
        let free = (inner_width - 40.).max(0.);
        let copy_width = (free * 1.1 / 2.1).max(540.);
        let art_width = (free - copy_width).max(0.);
        hero.flex()
            .gap(px(40.))
            .child(
                div()
                    .flex_none()
                    .w(px(copy_width))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(self.hero_copy(m, window, cx)),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(art_width))
                    .h_full()
                    .child(self.hero_art(art_width, cx)),
            )
            .into_any_element()
    }

    fn features(&self, m: &Metrics, inner_width: f32) -> impl IntoElement {
        let h2_size = if m.narrow {
            clamp(14., m.vh(2.), 18.)
        } else {
            clamp(14., m.vh(2.3).min(m.vw(1.6)), 21.)
        };
        let p_size = match (m.narrow, m.short) {
            (true, true) => 11.5,
            (true, false) => clamp(12., m.vh(1.55), 14.),
            _ => clamp(13., m.vh(1.75), 15.5),
        };
        let p_lh = if m.narrow { 1.35 } else { 1.55 };
        let h2_mb = if m.narrow {
            2.
        } else {
            clamp(4., m.vh(1.), 10.)
        };
        let feature = |ix: usize, n: &str, title: &str, body: AnyElement| {
            let heading = div()
                .flex()
                .mb(px(h2_mb))
                .child(
                    spaced(fonts::ARCHIVO_BLACK, h2_size, -0.3)
                        .line_height(px(h2_size * 1.55))
                        .run(n.to_string(), Some(red())),
                )
                .child(div().w(px(10.)).flex_none())
                .child(
                    spaced(fonts::ARCHIVO_BLACK, h2_size, -0.3)
                        .line_height(px(h2_size * 1.55))
                        .run(title.to_uppercase(), Some(ink())),
                );
            let text = div()
                .text_size(px(p_size))
                .line_height(relative(p_lh))
                .opacity(0.85)
                .child(body);
            let cell = div().min_w_0().child(heading).child(text);
            if m.narrow {
                let pad = clamp(6., m.vh(1.2), 12.);
                cell.py(px(pad))
                    .when(ix > 0, |el| el.border_t(px(1.5)).border_color(ink()))
            } else {
                // `grid-template-columns: repeat(3, 1fr)`: equal border boxes.
                let cell = cell
                    .flex_none()
                    .w(px(inner_width / 3.))
                    .pt(px(clamp(12., m.vh(2.6), 30.)))
                    .pb(px(clamp(12., m.vh(2.8), 34.)))
                    .pr(px(28.));
                if ix == 0 {
                    cell
                } else {
                    cell.pl(px(28.)).border_l_2().border_color(ink())
                }
            }
        };
        div()
            .flex()
            .when(m.narrow, |el| el.flex_col())
            .border_t_2()
            .border_color(ink())
            .child(feature(
                0,
                "01",
                "One space per world",
                "A space holds one part of your life, with its own people, types, logins and agents. Share it with the people in it; everything is encrypted end to end."
                    .into_any_element(),
            ))
            .child(feature(
                1,
                "02",
                "Agents with jobs",
                "Give an agent a role, a schedule and the logins it needs. It triages the inbox, updates the site, chases leads, and every change it makes is an object you can open."
                    .into_any_element(),
            ))
            .child(feature(
                2,
                "03",
                "Everything is an object",
                "Emails, texts, people, files and bookmarks come in as linked objects. Any set becomes a table, board or calendar, and agents and people work on the same ones."
                    .into_any_element(),
            ))
    }

    fn footer(&self, m: &Metrics) -> impl IntoElement {
        div()
            .flex()
            .justify_between()
            .border_t_2()
            .border_color(ink())
            .pt(px(clamp(8., m.vh(1.8), 20.)))
            .pb(px(clamp(10., m.vh(2.4), 26.)))
            .text_size(px(if m.narrow { 12. } else { 14. }))
            .font_weight(FontWeight(550.))
            .opacity(0.8)
            .child("© 2026 roostr")
            .child("made of objects")
    }
}

impl Render for LandingPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size = self
            .viewport
            .get()
            .unwrap_or_else(|| window.viewport_size());
        let (w, h) = (f32::from(size.width), f32::from(size.height));
        let narrow = w <= 880.;
        let compact = h <= if narrow { 560. } else { 640. };
        // Natural height and a scrollbar: short windows and every phone column.
        let scroll = compact || narrow;
        let m = Metrics {
            w,
            h,
            narrow,
            short: narrow && h <= 700.,
            tiny: w <= 400.,
            compact,
            features: !compact || h >= 480.,
            footer: !compact,
            lede: h >= 360.,
            tagline: h >= 300.,
        };
        let pad_x = if m.narrow { 20. } else { 32. };
        let inner_width = w.min(1200.) - 2. * pad_x;
        let viewport = self.viewport.clone();
        let column = div()
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(1200.))
            .px(px(pad_x))
            .map(|c| if scroll { c.min_h_full() } else { c.h_full() })
            .child(self.header(&m, cx))
            .child(self.hero(&m, inner_width, window, cx))
            .when(m.features, |c| c.child(self.features(&m, inner_width)))
            .when(m.footer, |c| c.child(self.footer(&m)));
        div()
            .id("landing")
            .relative()
            .size_full()
            .map(|d| if scroll { d.overflow_y_scroll() } else { d.overflow_hidden() })
            .bg(cream())
            .text_color(ink())
            .font_family(fonts::INTER)
            .text_size(px(17.))
            .line_height(relative(1.55))
            .child(
                canvas(
                    move |bounds, window, _| {
                        if viewport.get() != Some(bounds.size) {
                            viewport.set(Some(bounds.size));
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .w_full()
                    .map(|d| if scroll { d.min_h_full() } else { d.h_full() })
                    .child(column),
            )
    }
}

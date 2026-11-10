//! `/`: the marketing landing (`src/routes/+page.svelte`). One screen, never a scroll: every
//! size scales with the viewport (`vw`/`vh` = this view's bounds), including the page's three
//! `@media` layouts.

use std::collections::HashSet;

use roostr_core::State;
use roostr_core::nav::{self, Route, SitePage};
use roostr_ui::color::rgb;
use roostr_ui::fonts::family;
use roostr_ui::{ScrollArea, SpacedText, Theme, motion};
use rune::{Color, Cursor, Cx, El, SurfaceFit, TextStyle, Tx, col, el, row, text};

use crate::assets::{self, GITHUB_MARK, Hero};

const GITHUB_URL: &str = "https://github.com/Geep5/Roostr";

/// The landing's root element: its size is the page's viewport.
const ROOT: &str = "site-landing";

fn cream() -> Color {
    rgb(0xedcf7f)
}
fn ink() -> Color {
    rgb(0x1b1b1b)
}
fn red() -> Color {
    rgb(0xca3433)
}
/// Text on red (`#fff6e3`).
fn paper() -> Color {
    rgb(0xfff6e3)
}

/// CSS `clamp(min, value, max)`.
fn clamp(min: f32, value: f32, max: f32) -> f32 {
    value.max(min).min(max)
}

/// The UI font (Intern, Inter after it) at `size` and `weight`, the page's `line-height: 1.55`.
fn inter(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight)
        .family(rune::Family::Sans)
        .lh(size * 1.55)
}

/// A line in `family` at `size` with CSS `letter-spacing: <spacing>px`.
fn spaced(family: rune::Family, size: f32, spacing: f32) -> SpacedText {
    SpacedText::new(spacing / size).family(family).size(size)
}

/// Which `.btn`s and links the pointer is over (their hover lift and colour).
#[derive(Default)]
pub(crate) struct Hovers(HashSet<&'static str>);

fn hovered(s: &State, name: &str) -> bool {
    s.get::<Hovers>().is_some_and(|h| h.0.contains(name))
}

/// Track the pointer over the element `name`.
fn on_hover(e: El, name: &'static str) -> El {
    e.on_hover_change(move |s: &mut State, on: bool, _: &mut Tx| {
        let hovers = &mut s.slot::<Hovers>().0;
        if on {
            hovers.insert(name);
        } else {
            hovers.remove(name);
        }
    })
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
    /// Too short for the one-screen layout (an embed, a small window): no rooster, the parts
    /// that don't fit are left out, and whatever still doesn't fit scrolls instead of
    /// overlapping.
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
    fn new(w: f32, h: f32) -> Self {
        let narrow = w <= 880.;
        let compact = h <= if narrow { 560. } else { 640. };
        Metrics {
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
        }
    }
    fn vw(&self, f: f32) -> f32 {
        self.w * f / 100.
    }
    fn vh(&self, f: f32) -> f32 {
        self.h * f / 100.
    }
    /// Natural height and a scrollbar: short windows and every phone column.
    fn scroll(&self) -> bool {
        self.compact || self.narrow
    }
    /// `.btn` padding (vertical, horizontal) and font size.
    fn button(&self) -> ((f32, f32), f32) {
        if self.narrow {
            ((9., 18.), 14.)
        } else {
            ((12., 24.), 15.)
        }
    }
}

/// `.btn` with its hover lift and colour transition (0.15 s).
#[allow(clippy::too_many_arguments)]
fn button(
    s: &State,
    name: &'static str,
    rest: Color,
    hover: Color,
    padding: (f32, f32),
    content: El,
    on_press: impl Fn(&mut State, &mut Tx) + 'static,
) -> El {
    let on = hovered(s, name);
    let color = if on { hover } else { rest };
    on_hover(
        row()
            .key(name)
            .items_center()
            .gap(9.)
            .cursor(Cursor::Pointer)
            .radius(999.)
            .fill(color)
            .border(2., color)
            .border_layout()
            .px(padding.1)
            .py(padding.0)
            .motion(motion::CONTROL.spring())
            .offset(0., if on { -2. } else { 0. })
            .offset_motion(motion::CONTROL.anim())
            .child(content)
            .on_press_tx(on_press),
        name,
    )
}

fn header(s: &State, m: &Metrics) -> El {
    let (btn_pad, btn_size) = m.button();
    let gh_pad = if m.tiny { (9., 11.) } else { btn_pad };
    let mut gh = row().items_center().gap(9.);
    if let Some(images) = s.get::<roostr_ui::Images>() {
        let mark = images.bundled("site:github-mark", GITHUB_MARK, 17.);
        gh = gh.child(el().size(17., 17.).pixels(mark.pixels()));
    }
    if !m.tiny {
        gh = gh.child(text("GitHub").font(inter(btn_size, 650.)).color(cream()));
    }
    let pad = if m.narrow {
        clamp(8., m.vh(1.6), 16.)
    } else {
        clamp(12., m.vh(2.6), 26.)
    };
    let wordmark = spaced(family::ARCHIVO_BLACK, 26., -0.5)
        .line_height(26. * 1.55)
        .run("roostr", Some(ink()))
        .run(".", Some(red()))
        .build(Theme::dark())
        .key("wordmark")
        .cursor(Cursor::Pointer)
        .on_press(|s: &mut State| nav::navigate(Route::Site(SitePage::Landing), s));
    let mut brand = row().items_center().gap(10.).child(wordmark);
    // A narrow embed has no room for the pill beside Guide.
    if !m.tiny {
        let pill = row()
            .key("alpha")
            .offset(0., 1.)
            .radius(999.)
            .fill(red())
            .pt(3.)
            .pb(2.)
            .px(9.)
            .child(
                spaced(rune::Family::Sans, 11., 1.5)
                    .weight(750.)
                    .line_height(11. * 1.4)
                    .run("ALPHA", Some(paper()))
                    .build(Theme::dark()),
            );
        brand = brand.child(roostr_ui::tooltip(
            pill,
            "Roostr is in alpha: expect rough edges and changes",
        ));
    }
    let guide = on_hover(
        text("Guide")
            .key("nav-guide")
            .font(inter(15., 650.))
            .color(if hovered(s, "nav-guide") {
                red()
            } else {
                ink()
            })
            .cursor(Cursor::Pointer)
            .on_press(|s: &mut State| nav::navigate(Route::Site(SitePage::Guide), s)),
        "nav-guide",
    );
    row()
        .w_full()
        .items_center()
        .justify_between()
        .py(pad)
        .child(brand)
        .child(
            row()
                .items_center()
                .gap(if m.tiny { 14. } else { 22. })
                .child(guide)
                .child(button(
                    s,
                    "btn-github",
                    ink(),
                    red(),
                    gh_pad,
                    gh,
                    |_, tx| {
                        if let Err(error) = tx.open_url(GITHUB_URL) {
                            log::warn!("could not open {GITHUB_URL}: {error}");
                        }
                    },
                )),
        )
}

/// The headline, lede and actions, in a column `width` wide.
fn hero_copy(s: &State, m: &Metrics, width: f32) -> El {
    let lines: &[(&str, bool)] = if m.narrow {
        &[("THE WORKSPACE", false), ("PEOPLE AND", false), ("AGENTS SHARE", true)]
    } else {
        &[("THE WORKSPACE", false), ("PEOPLE AND AGENTS", false), ("SHARE", true)]
    };
    // The widest line must fit the column: measure it in Archivo Black at 100pt (plus the
    // red period's room) and cap the size so it spans at most the column width.
    let probe = TextStyle::new(100., 400.).family(family::ARCHIVO_BLACK);
    let widest = lines
        .iter()
        .map(|(line, _)| rune::text::measure(&format!("{line}."), &[], probe, None).0)
        .fold(1., f32::max);
    let fit = width / widest * 100. * 0.97;
    let (h1_size, lede_size, lede_lh) = if m.narrow {
        (
            clamp(24., m.vw(10.).min(m.vh(6.)), 60.).min(fit),
            if m.short {
                13.
            } else {
                clamp(13.5, m.vh(1.9), 16.)
            },
            1.45,
        )
    } else {
        (
            clamp(36., m.vw(6.8).min(m.vh(9.)).min(m.vh(21.6) - 96.), 96.).min(fit),
            clamp(15., m.vh(2.1), 19.),
            1.55,
        )
    };
    let h1_line = |line: &str, dot: bool| {
        let mut l = spaced(family::ARCHIVO_BLACK, h1_size, -1.5)
            .line_height(h1_size * 0.98)
            .run(line, Some(ink()));
        if dot {
            l = l.run(".", Some(red()));
        }
        l.build(Theme::dark())
    };
    let lines: Vec<El> = lines.iter().map(|(line, dot)| h1_line(line, *dot)).collect();
    // The headline says "people + agents" now: no eyebrow above it.
    let mut copy = col().child(col().mb(clamp(10., m.vh(2.4), 24.)).children(lines));
    if m.lede {
        let lede = TextStyle::new(lede_size, 450.)
            .family(rune::Family::Sans)
            .lh(lede_size * lede_lh);
        // `.lede { max-width: 46ch }`: ch is the advance of "0" in the lede's font (46 of
        // them measured as one line, as text widths round up to whole points).
        let (max_width, _) = rune::text::measure(&"0".repeat(46), &[], lede, None);
        copy = copy.child(
            text(
                "Agents get jobs, schedules and logins. You see everything they do, in a place built for you.",
            )
            .font(lede)
            .color(ink())
            .wrap()
            .max_w(max_width)
            .mb(clamp(12., m.vh(3.), 30.)),
        );
    }
    let (btn_pad, btn_size) = m.button();
    let mut actions = row()
        .flex_wrap()
        .items_center()
        .gaps(if m.narrow { 8. } else { 14. }, 14.)
        .child(button(
            s,
            "btn-open",
            red(),
            ink(),
            btn_pad,
            text("Open Roostr")
                .font(inter(btn_size, 650.))
                .color(paper()),
            |s, _| nav::navigate(Route::Home, s),
        ));
    if m.tagline {
        actions = actions.child(
            text("Open source · end-to-end encrypted · agents run on your own computers")
                .font(inter(if m.narrow { 12. } else { 14. }, 550.))
                .color(ink())
                .opacity(0.75)
                .wrap()
                .max_w(width),
        );
    }
    copy.child(actions)
}

/// `#rooster`: the animated hero, or its poster under reduced motion (see
/// [`assets::show_hero`]), fitted into `max_width` × the height it is given.
fn hero_art(s: &State, cx: &Cx<State>, max_width: f32) -> El {
    let reduced = cx.is_reduced_motion();
    let mut art = el()
        .key("rooster")
        .w(max_width.min(560.))
        .h_full()
        .max_h(900.)
        .on_layout(move |s: &mut State, _| assets::show_hero(s, reduced));
    if let Some(hero) = s.get::<Hero>() {
        assets::prime_hero(hero, reduced);
        art = art.pixels_fit(&hero.pixels, SurfaceFit::Fit);
    }
    row().w_full().h_full().min_h(0.).center().child(art)
}

fn hero(s: &State, cx: &Cx<State>, m: &Metrics, inner_width: f32) -> El {
    let pad_top = clamp(6., m.vh(1.6), 24.);
    let pad_bottom = clamp(22., m.vh(4.5), 48.);
    if m.compact {
        // Natural height, top to bottom: the copy alone.
        return col()
            .pt(pad_top)
            .pb(pad_bottom)
            .child(hero_copy(s, m, inner_width));
    }
    if m.narrow {
        // A phone column scrolls: the rooster gets a fixed share of the screen above the copy
        // instead of whatever height is left, which a longer lede could take to nothing and
        // overlap the header.
        let mut column = col()
            .pt(pad_top)
            .pb(pad_bottom)
            .gap(clamp(6., m.vh(1.5), 16.));
        if !m.short {
            column = column.child(
                col()
                    .w_full()
                    .h(clamp(140., m.vh(24.), 280.))
                    .child(hero_art(s, cx, inner_width)),
            );
        }
        return column.child(hero_copy(s, m, inner_width));
    }
    // `grid-template-columns: minmax(540px, 1.1fr) 1fr; gap: 40px`.
    let free = (inner_width - 40.).max(0.);
    let copy_width = (free * 1.1 / 2.1).max(540.);
    let art_width = (free - copy_width).max(0.);
    row()
        .grow(1.)
        .min_h(0.)
        .pt(pad_top)
        .pb(pad_bottom)
        .gap(40.)
        .child(
            col()
                .w(copy_width)
                .h_full()
                .justify_center()
                .child(hero_copy(s, m, copy_width)),
        )
        .child(
            col()
                .w(art_width)
                .h_full()
                .child(hero_art(s, cx, art_width)),
        )
}

fn features(m: &Metrics, inner_width: f32) -> El {
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
    let feature = |ix: usize, n: &str, title: &str, body: &'static str| {
        let heading = |s: String, color: Color| {
            spaced(family::ARCHIVO_BLACK, h2_size, -0.3)
                .line_height(h2_size * 1.55)
                .run(s, Some(color))
                .build(Theme::dark())
        };
        let cell = col()
            .min_w(0.)
            .child(
                row()
                    .mb(h2_mb)
                    .child(heading(n.to_string(), red()))
                    .child(el().w(10.))
                    .child(heading(title.to_uppercase(), ink())),
            )
            .child(
                text(body)
                    .font(
                        TextStyle::new(p_size, 400.)
                            .family(rune::Family::Sans)
                            .lh(p_size * p_lh),
                    )
                    .color(ink())
                    .opacity(0.85)
                    .wrap(),
            );
        if m.narrow {
            let cell = cell.w_full().py(clamp(6., m.vh(1.2), 12.));
            if ix > 0 {
                cell.border_sides([1.5, 0., 0., 0.], ink()).border_layout()
            } else {
                cell
            }
        } else {
            // `grid-template-columns: repeat(3, 1fr)`: equal border boxes.
            let cell = cell
                .w(inner_width / 3.)
                .pt(clamp(12., m.vh(2.6), 30.))
                .pb(clamp(12., m.vh(2.8), 34.))
                .pr(28.);
            if ix == 0 {
                cell
            } else {
                cell.pl(28.)
                    .border_sides([0., 0., 0., 2.], ink())
                    .border_layout()
            }
        }
    };
    let cards = [
        feature(
            0,
            "01",
            "Agents with jobs",
            "Give each agent a role, a schedule, a model, a system prompt and only the logins it needs. It triages the inbox, chases leads and does the books, on your own computers.",
        ),
        feature(
            1,
            "02",
            "Run them like a team",
            "See every agent's status, errors, last run and goals in one place. Everything it touches is a real record you can open, check and fix. Start with one agent, then add ten.",
        ),
        feature(
            2,
            "03",
            "One space per world",
            "Your company, your household, a side project. Each space has its own people, agents, logins and records, shared with the people in it and encrypted end to end.",
        ),
    ];
    let container = if m.narrow { col() } else { row() };
    container
        .w_full()
        .border_sides([2., 0., 0., 0.], ink())
        .border_layout()
        .children(cards)
}

fn footer(m: &Metrics) -> El {
    let font = inter(if m.narrow { 12. } else { 14. }, 550.);
    row()
        .w_full()
        .justify_between()
        .border_sides([2., 0., 0., 0.], ink())
        .border_layout()
        .pt(clamp(8., m.vh(1.8), 20.))
        .pb(clamp(10., m.vh(2.4), 26.))
        .opacity(0.8)
        .child(text("© 2026 roostr").font(font).color(ink()))
        .child(text("made of objects").font(font).color(ink()))
}

/// The landing page, filling its parent.
pub(crate) fn view(s: &State, cx: &mut Cx<State>) -> El {
    let [w, h] = cx.layout_size(ROOT).unwrap_or([1280., 800.]);
    let m = Metrics::new(w, h);
    let scroll = m.scroll();
    let pad_x = if m.narrow { 20. } else { 32. };
    let inner_width = w.min(1200.) - 2. * pad_x;
    let mut column = col()
        .w_full()
        .max_w(1200.)
        .px(pad_x)
        .child(header(s, &m))
        .child(hero(s, cx, &m, inner_width));
    if m.features {
        column = column.child(features(&m, inner_width));
    }
    if m.footer {
        column = column.child(footer(&m));
    }
    let page = row().w_full().justify_center();
    let body = if scroll {
        ScrollArea::vertical("site-landing-scroll", page.min_h(h).child(column.min_h(h)))
            .grow()
            .w_full()
            .build(Theme::dark())
    } else {
        page.h_full().child(column.h_full())
    };
    col()
        .key(ROOT)
        .w_full()
        .h_full()
        .fill(cream())
        .clip_rounded(0.)
        .child(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_follow_the_pages_media_queries() {
        let desk = Metrics::new(1440., 900.);
        assert!(!desk.narrow && !desk.compact && desk.features && desk.footer && !desk.scroll());
        let phone = Metrics::new(390., 844.);
        assert!(phone.narrow && phone.tiny && !phone.short && phone.scroll());
        let embed = Metrics::new(1000., 500.);
        assert!(embed.compact && embed.features && !embed.footer && embed.scroll());
        let sliver = Metrics::new(1000., 320.);
        assert!(!sliver.features && !sliver.lede && sliver.tagline);
    }
}

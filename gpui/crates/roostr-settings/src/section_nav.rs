//! Settings' section navigation: a side panel beside the scrolling column on
//! desktop, a row of chips pinned above it on a phone. Both list the shown
//! sections in order; picking one glides the column to it, and the entry of
//! the section in view follows the scrolling (scroll-spy).
//!
//! The column reports its scroll position ([`Spy::scrolled`]) and each section
//! card its top ([`Spy::card_at`]); the section in view comes from those.

use std::time::Duration;

use web_time::Instant;

use roostr_core::State;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Theme, motion, radius, ty};
use rune::{Cursor, El, ScrollSpec, Tx, col, row, text};

/// Width of the desktop side panel; the dialog widens by as much.
pub(crate) const PANEL_W: f32 = 168.0;
/// A picked section's card comes to rest this far below the top of the area.
pub(crate) const LANDING: f32 = 8.0;
/// The section in view is the last one whose card starts above this line,
/// measured from the top of the area.
pub(crate) const SPY_LINE: f32 = 48.0;
/// The glide to a picked section (`scrollIntoView({ behavior: 'smooth' })`).
const GLIDE: motion::Tween = motion::Tween::ease(320);
/// The column's key: the scroll viewport the glide moves.
pub(crate) const SCROLL_KEY: &str = "settings-scroll";

/// The id-safe form of a section title ("Where your vault lives" →
/// "where-your-vault-lives"), for element keys and test lookups.
pub(crate) fn slug(title: &str) -> String {
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

/// The scroll-spy's inputs and the highlighted section.
#[derive(Default)]
pub(crate) struct Spy {
    /// The column's scroll offset and its largest.
    offset: f32,
    max: f32,
    /// The column's top in window points.
    area_top: f32,
    /// Each section card's top in window points (after scrolling).
    cards: Vec<f32>,
    /// The section last picked, the offset its glide ends at, and when it
    /// started: it stays highlighted while the glide runs and then until the
    /// area scrolls away from there (a section too low to reach the top
    /// would lose it to the scroll-spy otherwise).
    pick: Option<(usize, f32, Instant)>,
    /// The highlighted section.
    pub(crate) active: usize,
}

impl Spy {
    /// The column's scroll position changed (or it was laid out).
    pub(crate) fn scrolled(&mut self, offset: f32, max: f32, now: Instant) -> bool {
        let moved = (offset - self.offset).abs() > 0.01 || (max - self.max).abs() > 0.01;
        self.offset = offset;
        self.max = max;
        moved && self.update(now)
    }

    /// The column's top in the window.
    pub(crate) fn area_at(&mut self, top: f32, now: Instant) -> bool {
        self.area_top = top;
        self.update(now)
    }

    /// Section `ix`'s card is at `top` in the window (of `count` sections).
    pub(crate) fn card_at(&mut self, ix: usize, count: usize, top: f32, now: Instant) -> bool {
        self.cards.resize(count, f32::MAX);
        self.cards[ix] = top;
        self.update(now)
    }

    /// The offset that rests section `ix` [`LANDING`] below the top.
    fn target(&self, ix: usize) -> Option<f32> {
        let top = *self.cards.get(ix)?;
        let content_top = top - self.area_top + self.offset;
        Some((content_top - LANDING).clamp(0.0, self.max.max(0.0)))
    }

    /// Glide the column to section `ix`.
    pub(crate) fn pick(&mut self, ix: usize, now: Instant, tx: &mut Tx) {
        let Some(target) = self.target(ix) else {
            return;
        };
        tx.scroll_to_with(SCROLL_KEY, 0.0, target, GLIDE.spring());
        self.pick = Some((ix, target, now));
        self.active = ix;
    }

    /// Recompute the highlighted section; true when it changed.
    fn update(&mut self, now: Instant) -> bool {
        let glide = Duration::from_secs_f32(GLIDE.duration) + Duration::from_millis(100);
        let active = match self.pick {
            Some((ix, rest, at))
                if now.duration_since(at) < glide || (self.offset - rest).abs() <= 0.5 =>
            {
                ix
            }
            _ => {
                self.pick = None;
                self.in_view()
            }
        };
        let changed = active != self.active;
        self.active = active;
        changed
    }

    /// The scroll-spy: the last section whose card starts above the spy
    /// line, or the last section once the area is scrolled to its end.
    fn in_view(&self) -> usize {
        let count = self.cards.len();
        if count > 0 && self.max > 0.0 && self.offset >= self.max - 1.0 {
            return count - 1;
        }
        let line = self.area_top + SPY_LINE;
        (0..count)
            .take_while(|&ix| self.cards[ix] <= line)
            .last()
            .unwrap_or(0)
    }
}

/// Picks section `ix` (a panel entry or chip pressed).
fn go_to(s: &mut State, ix: usize, tx: &mut Tx) {
    let now = s.rt.now();
    if let Some(settings) = crate::settings::open_mut(s) {
        settings.spy.pick(ix, now, tx);
    }
}

/// Desktop: the side panel, one row per section.
pub(crate) fn panel(t: &Theme, titles: &[&'static str], active: usize) -> El {
    col()
        .key("settings-nav")
        .w(PANEL_W)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .pl(14.0)
        .pr(4.0)
        .pt(14.0)
        .pb(22.0)
        .gap(2.0)
        .children(titles.iter().enumerate().map(|(ix, title)| {
            let current = ix == active;
            let mut entry = row()
                .key(format!("settings-nav-{}", slug(title)))
                .role("button")
                .label(*title)
                .px(10.0)
                .py(6.0)
                .radius(radius::SM)
                .cursor(Cursor::Pointer)
                .hover_group()
                .hover(t.hover)
                .on_press_tx(move |s: &mut State, tx: &mut Tx| go_to(s, ix, tx))
                .child(label(
                    t,
                    title,
                    13.0,
                    17.0,
                    current,
                    current.then_some("settings-nav-active"),
                    false,
                ));
            if current {
                entry = entry.fill(t.hover);
            }
            entry
        }))
}

/// Phone: the chip row pinned above the column, scrolling sideways; the
/// active chip keeps itself in view ([`chip_key`]).
pub(crate) fn chips(t: &Theme, titles: &[&'static str], active: usize) -> El {
    row()
        .key("settings-chips")
        .w_full()
        .scroll_x()
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
        .child(
            row()
                .gap(6.0)
                .px(22.0)
                .pt(2.0)
                .pb(10.0)
                .children(titles.iter().enumerate().map(|(ix, title)| {
                    let current = ix == active;
                    let mut chip = row()
                        .key(chip_key(title))
                        .role("button")
                        .label(*title)
                        .px(12.0)
                        .py(5.0)
                        .radius(999.0)
                        .border(1.0, if current { t.muted } else { t.border })
                        .cursor(Cursor::Pointer)
                        .on_press_tx(move |s: &mut State, tx: &mut Tx| go_to(s, ix, tx))
                        .child(label(
                            t,
                            title,
                            12.0,
                            16.0,
                            current,
                            current.then_some("settings-chip-active"),
                            true,
                        ));
                    if current {
                        chip = chip.fill(t.hover);
                    }
                    chip
                })),
        )
}

/// The key of a section's chip.
pub(crate) fn chip_key(title: &str) -> String {
    format!("settings-chip-{}", slug(title))
}

/// An entry's title; the active entry's carries `marker` as its key (the
/// entry itself carries the section's).
fn label(
    t: &Theme,
    title: &'static str,
    size: f32,
    line: f32,
    current: bool,
    marker: Option<&'static str>,
    chip: bool,
) -> El {
    let mut style = ty::ui(size).lh(line);
    if current {
        style = style.weight(ty::MEDIUM);
    }
    let mut e = text(title)
        .font(style)
        .color(if current { t.fg } else { t.muted })
        .group_color(t.fg)
        .min_w(0.0);
    if !chip {
        // Panel entries wrap inside the panel; chips stay on one line.
        e = e.wrap().grow(1.0);
    }
    if let Some(marker) = marker {
        e = e.key(marker);
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Where your vault lives"), "where-your-vault-lives");
        assert_eq!(slug("Chat color"), "chat-color");
    }

    #[test]
    fn spy_follows_cards_and_the_end() {
        let now = Instant::now();
        let mut spy = Spy::default();
        spy.area_at(100.0, now);
        for (ix, top) in [100.0, 300.0, 600.0].into_iter().enumerate() {
            spy.card_at(ix, 3, top, now);
        }
        spy.scrolled(0.0, 900.0, now);
        assert_eq!(spy.active, 0);
        for (ix, top) in [-200.0, 0.0, 140.0].into_iter().enumerate() {
            spy.card_at(ix, 3, top, now);
        }
        spy.scrolled(300.0, 900.0, now);
        assert_eq!(spy.active, 2, "the card above the spy line");
        spy.scrolled(900.0, 900.0, now);
        assert_eq!(spy.active, 2);
    }
}

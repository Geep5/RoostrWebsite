//! Settings' section navigation: a side panel beside the scrolling column on
//! desktop, a row of chips pinned above it on a phone. Both list the shown
//! sections in order; picking one glides the column to it, and the entry of
//! the section in view follows the scrolling (scroll-spy).
//!
//! The sections are consecutive children of the column's scroll area, from
//! child `first` on, so their bounds come from its [`ScrollHandle`].

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::{motion, theme};

use crate::settings::Settings;

/// Width of the desktop side panel; the dialog widens by as much.
pub(crate) const PANEL_W: Pixels = px(168.);
/// A picked section's card comes to rest this far below the top of the area.
const LANDING: Pixels = px(8.);
/// The section in view is the last one whose card starts above this line,
/// measured from the top of the area.
const SPY_LINE: Pixels = px(48.);
/// The glide to a picked section (`scrollIntoView({ behavior: 'smooth' })`).
const GLIDE: motion::Tween = motion::Tween::ease(320);

/// The id-safe form of a section title ("Where your vault lives" →
/// "where-your-vault-lives"), for element ids and test selectors.
pub(crate) fn slug(title: &str) -> String {
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

#[derive(Default)]
pub(crate) struct SectionNav {
    /// The section last picked and the offset its glide ends at: it stays
    /// highlighted until the area scrolls away from there (a section too
    /// low to reach the top would lose it to the scroll-spy otherwise).
    pick: Option<(usize, Pixels)>,
    glide: Option<Glide>,
    /// The phone's chip row, and the chip last scrolled into it.
    chips: ScrollHandle,
    chip_shown: Option<usize>,
}

struct Glide {
    offset: motion::ValueState,
    target: Pixels,
    /// The offset this glide last set: any other means the user scrolled,
    /// which ends the glide.
    last: Pixels,
}

impl SectionNav {
    /// Glide `scroll` to section `ix` (the area's child `first + ix`).
    pub(crate) fn pick(&mut self, ix: usize, first: usize, scroll: &ScrollHandle) {
        let Some(card) = scroll.bounds_for_item(first + ix) else {
            return;
        };
        let area = scroll.bounds();
        let target = (area.top() + LANDING - card.top()).clamp(-scroll.max_offset().y, px(0.));
        let from = scroll.offset().y;
        self.glide = Some(Glide {
            offset: motion::ValueState::new(f64::from(from)),
            target,
            last: from,
        });
        self.pick = Some((ix, target));
    }

    /// Advance the glide and return the section in view, out of `count`.
    /// Runs each render: scrolling the area re-renders its view.
    pub(crate) fn frame(
        &mut self,
        first: usize,
        count: usize,
        scroll: &ScrollHandle,
        chips: bool,
        window: &mut Window,
        cx: &App,
    ) -> usize {
        if let Some(glide) = &mut self.glide {
            let now = scroll.offset();
            if (now.y - glide.last).abs() > px(0.5) {
                self.glide = None;
            } else {
                let frame =
                    glide
                        .offset
                        .frame(f64::from(glide.target), GLIDE.transition(), window, cx);
                glide.last = px(frame.value() as f32);
                scroll.set_offset(point(now.x, glide.last));
                if !frame.is_animating() {
                    self.glide = None;
                }
            }
        }
        let active = match self.pick {
            Some((ix, rest))
                if self.glide.is_some() || (scroll.offset().y - rest).abs() <= px(0.5) =>
            {
                ix
            }
            _ => {
                self.pick = None;
                in_view(first, count, scroll)
            }
        };
        if chips && self.chip_shown != Some(active) {
            self.chips.scroll_to_item(active);
            self.chip_shown = Some(active);
        }
        active.min(count.saturating_sub(1))
    }

    /// Desktop: the side panel, one row per section.
    pub(crate) fn panel(
        &self,
        titles: &[&'static str],
        active: usize,
        cx: &mut Context<Settings>,
    ) -> Stateful<Div> {
        div()
            .id("settings-nav")
            .debug_selector(|| "settings-nav".into())
            .flex_none()
            .w(PANEL_W)
            .overflow_y_scroll()
            .pl(px(14.))
            .pr(px(4.))
            .pt(px(14.))
            .pb(px(22.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(titles.iter().enumerate().map(|(ix, title)| {
                let current = ix == active;
                let selector = format!("settings-nav-{}", slug(title));
                div()
                    .id(("settings-nav", ix))
                    .debug_selector(move || selector)
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(theme::radius::sm())
                    .text_size(px(13.))
                    .line_height(px(17.))
                    .cursor_pointer()
                    .text_color(if current { theme::fg() } else { theme::muted() })
                    .when(current, |d| {
                        d.bg(theme::hover()).font_weight(FontWeight::MEDIUM)
                    })
                    .hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to_section(ix, cx)))
                    .child(label(title, current.then_some("settings-nav-active")))
            }))
    }

    /// Phone: the chip row pinned above the column, scrolling sideways; the
    /// active chip keeps itself in view.
    pub(crate) fn chips(
        &self,
        titles: &[&'static str],
        active: usize,
        cx: &mut Context<Settings>,
    ) -> Stateful<Div> {
        div()
            .id("settings-chips")
            .debug_selector(|| "settings-chips".into())
            .flex_none()
            .w_full()
            .flex()
            .gap(px(6.))
            .px(px(22.))
            .pt(px(2.))
            .pb(px(10.))
            .overflow_x_scroll()
            .track_scroll(&self.chips)
            .border_b_1()
            .border_color(theme::border())
            .children(titles.iter().enumerate().map(|(ix, title)| {
                let current = ix == active;
                let selector = format!("settings-chip-{}", slug(title));
                div()
                    .id(("settings-chip", ix))
                    .debug_selector(move || selector)
                    .flex_none()
                    .px(px(12.))
                    .py(px(5.))
                    .rounded_full()
                    .border_1()
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .border_color(if current {
                        theme::muted()
                    } else {
                        theme::border()
                    })
                    .text_color(if current { theme::fg() } else { theme::muted() })
                    .when(current, |d| {
                        d.bg(theme::hover()).font_weight(FontWeight::MEDIUM)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to_section(ix, cx)))
                    .child(label(title, current.then_some("settings-chip-active")))
            }))
    }
}

/// An entry's title; the active entry's carries `marker`, a test selector
/// (the entry itself carries the section's).
fn label(title: &'static str, marker: Option<&'static str>) -> Div {
    div()
        .when_some(marker, |d, marker| d.debug_selector(|| marker.into()))
        .child(title)
}

/// The scroll-spy: the last of `count` sections whose card starts above the
/// spy line, or the last section once the area is scrolled to its end.
fn in_view(first: usize, count: usize, scroll: &ScrollHandle) -> usize {
    let area = scroll.bounds();
    let offset = scroll.offset().y;
    let max = scroll.max_offset().y;
    if count > 0 && max > px(0.) && -offset >= max - px(1.) {
        return count - 1;
    }
    let line = area.top() - offset + SPY_LINE;
    (0..count)
        .take_while(|ix| {
            scroll
                .bounds_for_item(first + ix)
                .is_some_and(|card| card.top() <= line)
        })
        .last()
        .unwrap_or(0)
}

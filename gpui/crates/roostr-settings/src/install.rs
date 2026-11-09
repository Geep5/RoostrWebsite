//! The space-home card that says a computer is missing (`InstallComputer.svelte`).
//!
//! Without a computer nothing acts: agents are minted and answered by a
//! machine's harness, recurring work fires there, and credentials live
//! there. Shown only while no machine has registered at all; once one has,
//! each agent names the computer it runs on in its own Served by property.

use std::time::Duration;

use roostr_core::State;
use roostr_core::serving::fetch_machines;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Alert, Button, Glyph, Theme, ty};
use rune::{Cycle, El, col, el, row, text};

use crate::widgets::{Span, rich};

/// A harness registers itself seconds after it starts, so the card has to
/// notice without a reload: that moment is the whole payoff.
const POLL: Duration = Duration::from_secs(5);

/// The card's state: the roster read and its 5 s poll.
#[derive(Default)]
pub struct InstallComputer {
    machine_count: usize,
    /// The first roster read has landed: until then the card stays hidden.
    loaded: bool,
    error: String,
    /// "Hide for now": until the app restarts, like the web component state.
    dismissed: bool,
    poll: Option<roostr_rt::Task<()>>,
}

impl InstallComputer {
    fn visible(&self) -> bool {
        self.loaded && self.machine_count == 0 && !self.dismissed
    }
}

/// Start reading the machine roster (now and every 5 s). Idempotent.
pub fn start_install_poll(s: &mut State) {
    if s.slot::<InstallComputer>().poll.is_some() {
        return;
    }
    let task = s.rt.spawn(async move |cx| {
        loop {
            let fetch = cx.update(|s| fetch_machines(s));
            let result = fetch.await;
            cx.update(|s| {
                let card = s.slot::<InstallComputer>();
                match result {
                    Ok((_, machines)) => {
                        card.machine_count = machines.len();
                        card.error.clear();
                    }
                    Err(error) => card.error = error.to_string(),
                }
                card.loaded = true;
            });
            cx.timer(POLL).await;
        }
    });
    s.slot::<InstallComputer>().poll = Some(task);
}

/// Stop the roster poll (the home page is no longer shown).
pub fn stop_install_poll(s: &mut State) {
    s.slot::<InstallComputer>().poll = None;
}

/// GPUI's `ease_in_out` (quadratic).
const fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        let x = -2.0 * t + 2.0;
        1.0 - x * x / 2.0
    }
}

const PULSE_STOPS: usize = 33;

/// The dot's opacity over one 1.6 s loop: `0.25 + 0.75 * (1 - |2e - 1|)` of the eased time.
const PULSE: [(f32, f32); PULSE_STOPS] = {
    let mut stops = [(0.0, 0.0); PULSE_STOPS];
    let mut i = 0;
    while i < PULSE_STOPS {
        let x = i as f32 / (PULSE_STOPS - 1) as f32;
        let d = 2.0 * ease_in_out(x) - 1.0;
        let d = if d < 0.0 { -d } else { d };
        stops[i] = (x, 0.25 + 0.75 * (1.0 - d));
        i += 1;
    }
    stops
};

fn step(t: &Theme, n: usize, body: El) -> El {
    row()
        .child(
            row().w(20.0).pr(5.0).justify_end().child(
                text(format!("{n}."))
                    .font(ty::ui(13.0).lh(21.0))
                    .color(t.muted),
            ),
        )
        .child(col().grow(1.0).shrink().min_w(0.0).child(body))
}

/// "Install a computer" callout for the space home: empty until the roster
/// loads, while a machine exists, and after "Hide for now".
pub fn install_computer(s: &State, _cx: &mut rune::Cx<State>, t: &Theme) -> El {
    let Some(card) = s.get::<InstallComputer>().filter(|c| c.visible()) else {
        return el().key("install-computer");
    };
    let muted = t.muted;
    let steps = |n, spans: &[Span]| step(t, n, rich(t, spans, muted, 13.0, 21.0));
    let mut footer = row()
        .flex_wrap()
        .center_y()
        .gap(8.0)
        .child(
            // The only moving thing on the card: "still looking", not broken.
            el().key("install-pulse")
                .size(8.0, 8.0)
                .radius(4.0)
                .fill(t.accent)
                .cycle(Cycle {
                    period: 1.6,
                    phase: 0.0,
                    stops: &PULSE,
                    still: 1.0,
                    fps: 30.0,
                }),
        )
        .child(
            text("Waiting for a computer to appear…")
                .font(ty::ui(12.0).lh(15.0))
                .color(muted),
        );
    if s.harness.is_some() {
        footer = footer.child(
            text("This app is connected to a local daemon — start its harness and it will show up here.")
                .font(ty::ui(12.0).lh(15.0))
                .color(muted)
                .wrap()
                .w_full(),
        );
    }
    let mut e = col()
        .key("install-computer")
        .gap(10.0)
        .mb(14.0)
        .px(16.0)
        .py(14.0)
        .radius(14.0)
        .border(1.0, t.border)
        .fill(t.panel)
        .child(
            row()
                .items_start()
                .gap(12.0)
                .child(text("🖥️").font(ty::ui(22.0).lh(24.0)))
                .child(
                    col()
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .child(
                            text("Install a computer")
                                .font(ty::ui(15.0).lh(19.0).weight(ty::SEMIBOLD))
                                .color(t.fg)
                                .mb(4.0),
                        )
                        .child(
                            text(
                                "Agents run on a computer you own: it answers discussions, fires recurring work, and holds the logins. \
                                 Until one is installed, this space stores objects but nothing acts on them.",
                            )
                            .font(ty::ui(13.0).lh(19.0))
                            .color(muted)
                            .wrap(),
                        ),
                )
                .child(
                    Button::new("ic-dismiss")
                        .ghost()
                        .icon(Glyph::Close)
                        .square(26.0)
                        .radius(7.0)
                        .text_color(muted)
                        .tooltip("Hide for now")
                        .on_press(|s: &mut State| s.slot::<InstallComputer>().dismissed = true)
                        .build(t),
                ),
        )
        .child(
            col()
                .child(steps(
                    1,
                    &[
                        Span::Plain(
                            "On the computer you want to use, install Roostr and start it as a service, so it runs at boot and comes back if it crashes: ",
                        ),
                        Span::Code(" bun run service install "),
                        Span::Plain(" in "),
                        Span::Code(" harness/ "),
                        Span::Plain("."),
                    ],
                ))
                .child(steps(
                    2,
                    &[
                        Span::Plain("Sign that computer in with "),
                        Span::Bold("your own key"),
                        Span::Plain(
                            " — same identity, so it joins this space instead of making a new one.",
                        ),
                    ],
                ))
                .child(steps(
                    3,
                    &[
                        Span::Plain(
                            "It registers itself here within seconds. Then pick it in each agent's ",
                        ),
                        Span::Bold("Served by"),
                        Span::Plain(" property."),
                    ],
                )),
        )
        .child(footer);
    if !card.error.is_empty() {
        e = e.child(Alert::error("ic-error", card.error.clone()).build(t));
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_until_loaded_without_machines_and_not_dismissed() {
        let mut card = InstallComputer::default();
        assert!(!card.visible());
        card.loaded = true;
        assert!(card.visible());
        card.machine_count = 1;
        assert!(!card.visible());
        card.machine_count = 0;
        card.dismissed = true;
        assert!(!card.visible());
    }

    #[test]
    fn pulse_runs_dim_bright_dim() {
        assert!((PULSE[0].1 - 0.25).abs() < 1e-6);
        assert!((PULSE[PULSE_STOPS / 2].1 - 1.0).abs() < 1e-6);
        assert!((PULSE[PULSE_STOPS - 1].1 - 0.25).abs() < 1e-6);
        assert!(PULSE.windows(2).all(|w| w[0].0 < w[1].0));
    }
}

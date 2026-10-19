//! The space-home card that says a computer is missing (`InstallComputer.svelte`).
//!
//! Without a computer nothing acts: agents are minted and answered by a
//! machine's harness, recurring work fires there, and credentials live
//! there. Shown only while no machine has registered at all; once one has,
//! each agent names the computer it runs on in its own Served by property.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::harness::harness;
use roostr_core::serving::fetch_machines;
use roostr_core::theme;

use crate::widgets::{Span, rich};

/// A harness registers itself seconds after it starts, so the card has to
/// notice without a reload: that moment is the whole payoff.
const POLL: Duration = Duration::from_secs(5);

/// "Install a computer" callout for the space home.
pub struct InstallComputer {
    machine_count: usize,
    /// The first roster read has landed: until then the card stays hidden.
    loaded: bool,
    error: SharedString,
    /// "Hide for now": this card instance only, like the web component state.
    dismissed: bool,
    _poll: Task<()>,
}

impl InstallComputer {
    /// Start reading the machine roster (now and every 5 s).
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let Ok(task) = this.update(cx, |_, cx| fetch_machines(cx)) else {
                    return;
                };
                let result = task.await;
                let Ok(()) = this.update(cx, |this, cx| {
                    match result {
                        Ok((_, machines)) => {
                            this.machine_count = machines.len();
                            this.error = SharedString::default();
                        }
                        Err(error) => this.error = error.to_string().into(),
                    }
                    this.loaded = true;
                    cx.notify();
                }) else {
                    return;
                };
                cx.background_executor().timer(POLL).await;
            }
        });
        Self {
            machine_count: 0,
            loaded: false,
            error: SharedString::default(),
            dismissed: false,
            _poll: poll,
        }
    }
}

fn step(n: usize, body: impl IntoElement) -> Div {
    div()
        .flex()
        .child(
            div()
                .flex_none()
                .w(px(20.))
                .pr(px(5.))
                .flex()
                .justify_end()
                .child(format!("{n}.")),
        )
        .child(div().flex_1().min_w_0().child(body))
}

impl Render for InstallComputer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.loaded || self.machine_count > 0 || self.dismissed {
            return div().id("install-computer");
        }
        let muted = theme::muted();
        div()
            .id("install-computer")
            .flex()
            .flex_col()
            .gap(px(10.))
            .mb(px(14.))
            .px(px(16.))
            .py(px(14.))
            .rounded(px(14.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::panel())
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .child(div().flex_none().text_size(px(22.)).line_height(px(24.)).child("🖥️"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .mb(px(4.))
                                    .text_size(px(15.))
                                    .line_height(px(19.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Install a computer"),
                            )
                            .child(
                                div().text_size(px(13.)).line_height(px(19.)).text_color(muted).child(
                                    "Agents run on a computer you own: it answers discussions, fires recurring work, and holds the logins. \
                                     Until one is installed, this space stores objects but nothing acts on them.",
                                ),
                            ),
                    )
                    .child(
                        div()
                            .id("ic-dismiss")
                            .flex_none()
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(7.))
                            .border_1()
                            .border_color(transparent_black())
                            .text_color(muted)
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme::fg()).border_color(theme::border()))
                            .tooltip(roostr_core::ui::tooltip("Hide for now"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.dismissed = true;
                                cx.notify();
                            }))
                            .child("✕"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .text_size(px(13.))
                    .line_height(px(21.))
                    .text_color(muted)
                    .child(step(
                        1,
                        rich(
                            &[
                                Span::Plain(
                                    "On the computer you want to use, install Roostr and start it as a service, so it runs at boot and comes back if it crashes: ",
                                ),
                                Span::Code(" bun run service install "),
                                Span::Plain(" in "),
                                Span::Code(" harness/ "),
                                Span::Plain("."),
                            ],
                            muted,
                            window,
                        ),
                    ))
                    .child(step(
                        2,
                        rich(
                            &[
                                Span::Plain("Sign that computer in with "),
                                Span::Bold("your own key"),
                                Span::Plain(" — same identity, so it joins this space instead of making a new one."),
                            ],
                            muted,
                            window,
                        ),
                    ))
                    .child(step(
                        3,
                        rich(
                            &[
                                Span::Plain("It registers itself here within seconds. Then pick it in each agent's "),
                                Span::Bold("Served by"),
                                Span::Plain(" property."),
                            ],
                            muted,
                            window,
                        ),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .line_height(px(15.))
                    .text_color(muted)
                    .child(
                        div().size(px(8.)).rounded_full().bg(theme::accent()).with_animation(
                            "ic-pulse",
                            Animation::new(Duration::from_millis(1600)).repeat().with_easing(ease_in_out),
                            // The only moving thing on the card: "still looking", not broken.
                            |dot, t| dot.opacity(0.25 + 0.75 * (1. - (2. * t - 1.).abs())),
                        ),
                    )
                    .child("Waiting for a computer to appear…")
                    .when(harness(cx).is_some(), |d| {
                        d.child(
                            div()
                                .w_full()
                                .child("This app is connected to a local daemon — start its harness and it will show up here."),
                        )
                    }),
            )
            .when(!self.error.is_empty(), |d| {
                d.child(div().text_size(px(12.)).text_color(theme::red()).child(self.error.clone()))
            })
    }
}

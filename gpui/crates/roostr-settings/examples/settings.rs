//! Exercise every roostr-settings view against the test daemon.
//!
//! `cargo run -p roostr-settings --example settings -- [settings|space|install|gate] [space-id]`
//! starts on that view (default: the space page of the first space).

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::{Store, theme};
use roostr_settings::{IdentityGate, InstallComputer, SpaceManage, open_settings};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Space,
    Install,
    Gate,
}

struct Demo {
    page: Page,
    space: Entity<SpaceManage>,
    install: Entity<InstallComputer>,
    gate: Entity<IdentityGate>,
    open_settings_on_start: bool,
}

impl Demo {
    fn tab(
        &self,
        id: &'static str,
        label: &'static str,
        page: Option<Page>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = page.is_some_and(|p| p == self.page);
        div()
            .id(id)
            .px(px(12.))
            .py(px(5.))
            .rounded(px(7.))
            .text_size(px(13.))
            .cursor_pointer()
            .border_1()
            .border_color(theme::border())
            .when(active, |d| d.bg(theme::hover()))
            .hover(|s| s.border_color(theme::accent()))
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| match page {
                Some(page) => {
                    this.page = page;
                    cx.notify();
                }
                None => open_settings(window, cx),
            }))
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.open_settings_on_start) {
            open_settings(window, cx);
        }
        let body: AnyElement = match self.page {
            Page::Space => div()
                .id("page-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    div()
                        .max_w(px(920.))
                        .mx_auto()
                        .pt(px(24.))
                        .pb(px(80.))
                        .child(self.space.clone()),
                )
                .into_any_element(),
            Page::Install => div()
                .flex_1()
                .p(px(24.))
                .child(div().max_w(px(920.)).mx_auto().child(self.install.clone()))
                .into_any_element(),
            Page::Gate => div()
                .flex_1()
                .min_h_0()
                .child(self.gate.clone())
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .p(px(10.))
                    .border_b_1()
                    .border_color(theme::border())
                    .child(self.tab("open-settings", "Settings…", None, cx))
                    .child(self.tab("show-space", "Space settings", Some(Page::Space), cx))
                    .child(self.tab("show-install", "Install computer", Some(Page::Install), cx))
                    .child(self.tab("show-gate", "Identity gate", Some(Page::Gate), cx)),
            )
            .child(body)
    }
}

fn main() {
    roostr_core::dev::run_example("Settings", |args, window, cx| {
        let mode = args.first().map(String::as_str).unwrap_or("space");
        let space_id = args
            .get(1)
            .cloned()
            .or_else(|| {
                Store::global(cx)
                    .read(cx)
                    .channels
                    .first()
                    .map(|s| s.id.clone())
            })
            .unwrap_or_default();
        roostr_settings::init(cx);
        cx.new(|cx| Demo {
            page: match mode {
                "install" => Page::Install,
                "gate" => Page::Gate,
                _ => Page::Space,
            },
            space: cx.new(|cx| SpaceManage::new(space_id, window, cx)),
            install: cx.new(|cx| InstallComputer::new(window, cx)),
            gate: cx.new(|cx| IdentityGate::new(window, cx)),
            open_settings_on_start: mode == "settings",
        })
        .into()
    });
}

//! Roostr desktop app: one window holding the shell (`roostr-shell`) with a
//! page per route from every feature crate, against the local glon daemon
//! named by the environment (`GLON_DATA` / `ROOSTR_API`), or - `--replica`,
//! `ROOSTR_BACKEND=replica` or the Settings choice - against this device's
//! own replica (`roostr-replica`), no daemon needed.

use std::sync::Arc;

use gpui_kit::component::TitleBar;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::daemon::{DaemonBackend, DaemonConfig};
use roostr_core::harness::Harness;
use roostr_core::mode::{self, Mode};
use roostr_core::nav::Route;
use roostr_core::theme;
use roostr_replica::ReplicaBackend;
use roostr_shell::{NewObject, OpenSettings, Search, Workspace};

actions!(roostr, [About, Quit]);

/// The window content: the title bar strip, then the shell.
struct AppRoot {
    workspace: Entity<Workspace>,
}

impl Render for AppRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::gutter())
            .line_height(relative(theme::LINE_HEIGHT_NORMAL))
            .text_color(theme::fg())
            .child(TitleBar::new())
            .child(div().flex_1().min_h_0().child(self.workspace.clone()))
    }
}

/// Shown instead of the app when its backend can't be opened.
struct ConfigError {
    title: &'static str,
    message: SharedString,
    /// Offer the other mode (a daemon that can't be reached: this device's replica).
    switch_to: Option<Mode>,
}

impl Render for ConfigError {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let switch_to = self.switch_to;
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::bg())
            .text_color(theme::fg())
            .font_family(theme::fonts::UI)
            .child(TitleBar::new())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.))
                    .p(px(48.))
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::BOLD)
                            .child(self.title),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::muted())
                            .child(self.message.clone()),
                    )
                    .when_some(switch_to, |d, mode| {
                        d.child(
                            div()
                                .id("switch-mode")
                                .mt(px(8.))
                                .px(px(14.))
                                .py(px(8.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(theme::border())
                                .cursor_pointer()
                                .text_size(px(13.))
                                .child(match mode {
                                    Mode::Replica => {
                                        "Use Roostr without the daemon (this device's own replica)"
                                    }
                                    Mode::Daemon => "Use the daemon on this computer",
                                })
                                .on_click(move |_, _, cx| mode::switch(mode, cx)),
                        )
                    }),
            )
    }
}

fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1440.), px(900.)),
            cx,
        ))),
        ..TitleBar::window_options()
    }
}

fn set_menus(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &About, cx| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            let detail = format!("Version {}", env!("CARGO_PKG_VERSION"));
            let answer = window.prompt(PromptLevel::Info, "Roostr", Some(&detail), &["OK"], cx);
            cx.background_spawn(async move {
                let _ = answer.await;
            })
            .detach();
        });
    });
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-n", NewObject, None),
    ]);
    cx.set_menus(vec![
        Menu {
            name: "Roostr".into(),
            items: vec![
                MenuItem::action("About Roostr", About),
                MenuItem::separator(),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit Roostr", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New…", NewObject),
                MenuItem::action("Search", Search),
            ],
            disabled: false,
        },
    ]);
}

/// `--replica` / `--daemon`, else `ROOSTR_BACKEND`, else the choice saved in
/// Settings, else the daemon.
fn chosen_mode() -> Mode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == mode::REPLICA_FLAG) {
        return Mode::Replica;
    }
    if args.iter().any(|a| a == mode::DAEMON_FLAG) {
        return Mode::Daemon;
    }
    std::env::var("ROOSTR_BACKEND")
        .ok()
        .and_then(|v| Mode::parse(&v))
        .or_else(mode::saved)
        .unwrap_or(Mode::Daemon)
}

/// `roostr [path]`: the web path to open (`/app/object/<id>`, `/guide`, …).
fn route_arg() -> Option<String> {
    std::env::args().skip(1).find(|a| !a.starts_with("--"))
}

fn open_app(cx: &mut App) {
    roostr_app::init(cx);
    open_window(window_options(cx), cx, |window, cx| {
        let workspace = roostr_app::workspace(window, cx);
        if let Some(path) = route_arg() {
            roostr_app::open_when_ready(Route::parse(&path), cx);
        }
        cx.new(|_| AppRoot { workspace })
    })
    .expect("open the Roostr window");
}

fn open_error(title: &'static str, message: String, switch_to: Option<Mode>, cx: &mut App) {
    theme::init(cx);
    let message = SharedString::from(message);
    open_window(window_options(cx), cx, |_, cx| {
        cx.new(|_| ConfigError {
            title,
            message,
            switch_to,
        })
    })
    .expect("open the Roostr window");
}

fn main() {
    // Warnings and errors from every crate go to stderr; `RUST_LOG` overrides.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    application().with_assets(roostr_core::Assets).run(|cx| {
        gpui_kit::init(cx);
        set_menus(cx);
        match chosen_mode() {
            Mode::Replica => {
                let dir = ReplicaBackend::default_dir();
                match futures::executor::block_on(ReplicaBackend::open_native(&dir)) {
                    Ok(backend) => {
                        // No harness: computer features show "Roostr isn't running on this computer".
                        roostr_core::init(Arc::new(backend), None, cx);
                        open_app(cx);
                    }
                    Err(error) => open_error(
                        "Roostr can't open this device's replica",
                        format!("{}: {error:#}", dir.display()),
                        Some(Mode::Daemon),
                        cx,
                    ),
                }
            }
            Mode::Daemon => match DaemonConfig::from_env() {
                Ok(config) => {
                    let harness = Arc::new(Harness::new(config.clone()));
                    roostr_core::init(Arc::new(DaemonBackend::new(config)), Some(harness), cx);
                    open_app(cx);
                }
                Err(error) => open_error(
                    "Roostr can't reach its daemon",
                    format!("{error:#}"),
                    Some(Mode::Replica),
                    cx,
                ),
            },
        }
        cx.activate(true);
    });
}

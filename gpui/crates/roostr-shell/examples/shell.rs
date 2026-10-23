//! Shell dev harness: the full workspace (rail, sidebar, tabs, header, search,
//! New menu, pinned widgets, drawers) against the daemon named by the
//! environment, with a plain page per route standing in for the real pages.
//!
//! ```sh
//! GLON_DATA=/tmp/roostr-gpui-vault ROOSTR_UI_STATE=/tmp/roostr-ui-shell.json \
//!   cargo run -p roostr-shell --example shell [-- <object id to open>]
//! ```
//!
//! Object pages publish stand-in pane views so the Chat and Properties
//! columns, their strips, drawer tab, resizers and the header's pane button
//! can be exercised too.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use roostr_core::daemon::{DaemonBackend, DaemonConfig};
use roostr_core::harness::Harness;
use roostr_core::nav::{NavEvent, Navigator, Route};
use roostr_core::store::Store;
use roostr_core::{PaneState, PaneViews, theme};
use roostr_shell::{PageFactory, Workspace};

/// A stand-in page: the route and what the store knows about it.
struct RoutePage {
    route: Route,
    pane: Option<PaneViews>,
}

/// A stand-in for one part of the pane: (object id, part).
struct PanePlaceholder(String, &'static str);

impl Render for PanePlaceholder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .text_color(theme::muted())
            .text_size(px(13.))
            .child(format!("{} for {}", self.1, self.0))
    }
}

impl RoutePage {
    fn new(route: Route, cx: &mut Context<Self>) -> Self {
        cx.observe(&Store::global(cx), |_, _, cx| cx.notify())
            .detach();
        let pane = match &route {
            Route::Object(id) => {
                let mut part = |name| AnyView::from(cx.new(|_| PanePlaceholder(id.clone(), name)));
                Some(PaneViews {
                    sheet: part("Pane sheet"),
                    chat: Some(part("Chat")),
                    properties: part("Properties"),
                })
            }
            _ => None,
        };
        cx.subscribe(&Navigator::global(cx), |this, nav, event: &NavEvent, cx| {
            if *event == NavEvent::Navigated && *nav.read(cx).route() == this.route {
                this.publish(cx);
            }
        })
        .detach();
        let mut this = Self { route, pane };
        this.publish(cx);
        this
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = &self.pane {
            PaneState::global(cx).update(cx, |p, cx| p.publish(true, true, 3, 1, cx));
            roostr_shell::set_pane(Some(pane.clone()), cx);
        }
    }
}

impl Render for RoutePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx).read(cx);
        let detail = match &self.route {
            Route::Object(id) => store
                .summary(id)
                .map(|s| format!("{} {} ({})", s.icon, s.name, s.type_key))
                .or_else(|| store.space(id).map(|s| format!("Space {}", s.name)))
                .unwrap_or_else(|| "(not in summaries)".into()),
            Route::Home => "Nothing open in this space yet. Pick something in the sidebar, find anything with search, or make something new from the New menu.".into(),
            _ => String::new(),
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .text_size(px(14.))
            .child(div().text_color(theme::fg()).child(self.route.path()))
            .child(div().text_color(theme::muted()).child(detail))
    }
}

struct Root {
    workspace: Entity<Workspace>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::gutter())
            .line_height(relative(theme::LINE_HEIGHT_NORMAL))
            .child(TitleBar::new())
            .child(div().flex_1().min_h_0().child(self.workspace.clone()))
    }
}

fn main() {
    let open: Option<String> = std::env::args().nth(1);
    application()
        .with_assets(roostr_core::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let config = DaemonConfig::from_env().unwrap_or_else(|error| panic!("{error:#}"));
            let harness = Arc::new(Harness::new(config.clone()));
            roostr_core::init(Arc::new(DaemonBackend::new(config)), Some(harness), cx);
            roostr_properties::init(cx);
            roostr_shell::init(cx);
            let pages: PageFactory = Rc::new(|route: &Route, _: &mut Window, cx: &mut App| {
                cx.new(|cx| RoutePage::new(route.clone(), cx)).into()
            });
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1440.), px(900.)),
                    cx,
                ))),
                ..TitleBar::window_options()
            };
            open_window(options, cx, move |window, cx| {
                let workspace = cx.new(|cx| {
                    let mut ws = Workspace::new(pages, window, cx);
                    ws.on_open_settings(|_, _| log::info!("Settings requested"));
                    ws
                });
                if let Some(id) = open {
                    // Once the shell picked the space (boot), open the object in its active tab.
                    let nav = Navigator::global(cx);
                    let mut pending = Some(id);
                    cx.observe(&nav, move |nav, cx| {
                        if !nav.read(cx).space().is_empty()
                            && let Some(id) = pending.take()
                        {
                            roostr_core::nav::navigate(Route::Object(id), cx);
                        }
                    })
                    .detach();
                }
                cx.new(|_| Root { workspace })
            })
            .expect("open shell window");
            cx.activate(true);
        });
}

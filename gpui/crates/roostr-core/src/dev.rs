//! Development harness for each feature crate's `examples/`: one call boots
//! gpui-kit + the core globals against the daemon named by the environment
//! (`GLON_DATA`, `ROOSTR_API`, see [`crate::daemon::DaemonConfig`]) and opens
//! a window hosting one view inside a page-like frame. It waits for the store
//! to finish its first load before building the view.
//!
//! ```ignore
//! fn main() {
//!     roostr_core::dev::run_example("Editor", |args, window, cx| {
//!         let id = args.first().cloned().unwrap_or_default();
//!         cx.new(|cx| Editor::new(id, false, window, cx)).into()
//!     });
//! }
//! ```

use std::sync::Arc;

use gpui_kit::component::TitleBar;
use gpui_kit::*;

use crate::daemon::{DaemonBackend, DaemonConfig};
use crate::harness::Harness;
use crate::{Connection, Store, theme};

type Build = Box<dyn FnOnce(Vec<String>, &mut Window, &mut App) -> AnyView>;

struct ExampleRoot {
    title: SharedString,
    args: Vec<String>,
    build: Option<Build>,
    view: Option<AnyView>,
}

impl Render for ExampleRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx);
        let (loaded, connection) = {
            let s = store.read(cx);
            (s.loaded, s.connection.clone())
        };
        if self.view.is_none()
            && loaded
            && let Some(build) = self.build.take()
        {
            self.view = Some(build(self.args.clone(), window, cx));
        }
        let body: AnyElement = match (&self.view, connection) {
            (Some(view), _) => view.clone().into_any_element(),
            (None, Connection::Failed(error)) => div()
                .p_4()
                .text_color(theme::red())
                .child(error)
                .into_any_element(),
            (None, _) => div()
                .p_4()
                .text_color(theme::muted())
                .child("Opening your vault…")
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::gutter())
            .text_color(theme::fg())
            .text_size(px(15.))
            .line_height(relative(theme::LINE_HEIGHT_NORMAL))
            .child(
                TitleBar::new().child(
                    div()
                        .text_sm()
                        .text_color(theme::muted())
                        .child(self.title.clone()),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .m(px(6.))
                    .rounded(theme::radius::page())
                    .bg(theme::bg())
                    .overflow_hidden()
                    .child(body),
            )
    }
}

/// Boot and show `build`'s view. `build` receives the command-line arguments
/// after the program name (e.g. an object id).
pub fn run_example(
    title: &str,
    build: impl FnOnce(Vec<String>, &mut Window, &mut App) -> AnyView + 'static,
) {
    let title = SharedString::from(title.to_string());
    let args: Vec<String> = std::env::args().skip(1).collect();
    application().with_assets(crate::Assets).run(move |cx| {
        gpui_kit::init(cx);
        let config = DaemonConfig::from_env().unwrap_or_else(|error| panic!("{error:#}"));
        let harness = Arc::new(Harness::new(config.clone()));
        crate::init(Arc::new(DaemonBackend::new(config)), Some(harness), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1280.), px(860.)),
                cx,
            ))),
            ..TitleBar::window_options()
        };
        open_window(options, cx, move |_, cx| {
            cx.new(move |cx| {
                cx.observe(&Store::global(cx), |_, _, cx| cx.notify())
                    .detach();
                ExampleRoot {
                    title,
                    args,
                    build: Some(Box::new(build)),
                    view: None,
                }
            })
        })
        .expect("open example window");
        cx.activate(true);
    });
}

//! `cargo run -p roostr-site --example site -- landing|guide|privacy|join:<blob>`
//!
//! Shows one website page. Links between site pages swap the page in place
//! (the app's shell does the same through its page factory); `/app` links
//! are logged, since this window has no workspace.

use gpui_kit::*;
use roostr_core::nav::{NavEvent, Navigator, Route, SitePage};
use roostr_site::SiteView;

struct Host {
    view: Entity<SiteView>,
    _nav: Subscription,
}

impl Host {
    fn new(page: SitePage, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.new(|cx| SiteView::new(page, window, cx));
        let nav = cx.subscribe_in(
            &Navigator::global(cx),
            window,
            |this, nav, event: &NavEvent, window, cx| {
                if *event != NavEvent::Navigated {
                    return;
                }
                match nav.read(cx).route().clone() {
                    Route::Site(page) => {
                        this.view = cx.new(|cx| SiteView::new(page, window, cx));
                        cx.notify();
                    }
                    other => println!("navigate: {}", other.path()),
                }
            },
        );
        Self { view, _nav: nav }
    }
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.view.clone())
    }
}

fn main() {
    roostr_core::dev::run_example("Site", |args, window, cx| {
        roostr_site::init(cx);
        let arg = args.first().cloned().unwrap_or_else(|| "landing".into());
        let page = match arg.as_str() {
            "guide" => SitePage::Guide,
            "privacy" => SitePage::Privacy,
            other => match other.strip_prefix("join:") {
                Some(blob) => SitePage::Join(blob.to_string()),
                None => SitePage::Landing,
            },
        };
        cx.new(|cx| Host::new(page, window, cx)).into()
    });
}

//! Website pages shown inside the app window: the landing (`/`), the guide
//! (`/guide`), the privacy policy (`/privacy`) and the space join request
//! landing (`/j#r2.<blob>`), ported from `src/routes/{+page,guide,privacy,j}`.

mod assets;
mod doc;
mod guide;
mod join;
mod landing;
mod privacy;
mod text;

use gpui_kit::*;
use roostr_core::nav::SitePage;

actions!(
    site,
    [
        ScrollUp, ScrollDown, PageUp, PageDown, ScrollHome, ScrollEnd
    ]
);

const CONTEXT: &str = "SitePage";

/// Static instances of the bundled variable Inter, subset to the landing's
/// characters. GPUI picks faces by weight and cannot set variation axes, so
/// without these every weight renders as the default (400, `opsz` 14):
/// - family "Inter" at the landing's other weights (`opsz` 14, its small text);
/// - families "Inter Lede 15".."Inter Lede 19": the 450-weight lede at the
///   optical size Chrome gives its viewport-scaled font size.
static INTER_INSTANCES: [&[u8]; 9] = [
    include_bytes!("../assets/fonts/Inter-500.ttf"),
    include_bytes!("../assets/fonts/Inter-550.ttf"),
    include_bytes!("../assets/fonts/Inter-650.ttf"),
    include_bytes!("../assets/fonts/Inter-750.ttf"),
    include_bytes!("../assets/fonts/InterLede-15.ttf"),
    include_bytes!("../assets/fonts/InterLede-16.ttf"),
    include_bytes!("../assets/fonts/InterLede-17.ttf"),
    include_bytes!("../assets/fonts/InterLede-18.ttf"),
    include_bytes!("../assets/fonts/InterLede-19.ttf"),
];

/// Set once [`init`] has run.
struct Initialized;

impl Global for Initialized {}

/// Register the landing's font weights and browser-style keyboard scrolling
/// on the scrolled site pages, once: a browser tab that opens on a website
/// page calls this before its vault (and `roostr_app::init`) starts. Call
/// after `theme::init`.
pub fn init(cx: &mut App) {
    if cx.has_global::<Initialized>() {
        return;
    }
    cx.set_global(Initialized);
    let fonts = INTER_INSTANCES
        .iter()
        .map(|bytes| std::borrow::Cow::Borrowed(*bytes))
        .collect();
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        log::warn!("could not load the landing's Inter weights: {error:#}");
    }
    cx.bind_keys([
        KeyBinding::new("up", ScrollUp, Some(CONTEXT)),
        KeyBinding::new("down", ScrollDown, Some(CONTEXT)),
        KeyBinding::new("pageup", PageUp, Some(CONTEXT)),
        KeyBinding::new("alt-up", PageUp, Some(CONTEXT)),
        KeyBinding::new("shift-space", PageUp, Some(CONTEXT)),
        KeyBinding::new("pagedown", PageDown, Some(CONTEXT)),
        KeyBinding::new("alt-down", PageDown, Some(CONTEXT)),
        KeyBinding::new("space", PageDown, Some(CONTEXT)),
        KeyBinding::new("home", ScrollHome, Some(CONTEXT)),
        KeyBinding::new("end", ScrollEnd, Some(CONTEXT)),
        // Command-Up/Down, as macOS scrolls a page. In a browser Command is
        // GPUI's `secondary` key on a Mac (vendor/gpui-pre-web).
        #[cfg(not(target_family = "wasm"))]
        KeyBinding::new("cmd-up", ScrollHome, Some(CONTEXT)),
        #[cfg(not(target_family = "wasm"))]
        KeyBinding::new("cmd-down", ScrollEnd, Some(CONTEXT)),
        #[cfg(target_family = "wasm")]
        KeyBinding::new("secondary-up", ScrollHome, Some(CONTEXT)),
        #[cfg(target_family = "wasm")]
        KeyBinding::new("secondary-down", ScrollEnd, Some(CONTEXT)),
    ]);
}

/// One website page (`Route::Site`), filling its parent and owning its scroll.
pub struct SiteView {
    body: AnyView,
}

impl SiteView {
    /// Build the view for `page`.
    pub fn new(page: SitePage, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let body: AnyView = match page {
            SitePage::Landing => cx.new(|_| landing::LandingPage::new()).into(),
            SitePage::Guide => cx
                .new(|cx| doc::DocPage::new(guide::BLOCKS, guide::style(), window, cx))
                .into(),
            SitePage::Privacy => cx
                .new(|cx| doc::DocPage::new(privacy::BLOCKS, privacy::style(), window, cx))
                .into(),
            SitePage::Join(blob) => cx.new(|cx| join::JoinPage::new(&blob, window, cx)).into(),
        };
        Self { body }
    }
}

impl Render for SiteView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.body.clone())
    }
}

/// Wire the keyboard scroll actions and click-to-focus onto a page root.
fn scroll_keys<T: 'static>(
    root: Stateful<Div>,
    focus: &FocusHandle,
    cx: &mut Context<T>,
    handle: fn(&T) -> &ScrollHandle,
) -> Stateful<Div> {
    let page = move |this: &T| f32::from(handle(this).bounds().size.height) * 0.875;
    let focus_on_click = focus.clone();
    root.key_context(CONTEXT)
        .track_focus(focus)
        .on_action(cx.listener(move |this, _: &ScrollUp, _, cx| scroll_by(handle(this), -40., cx)))
        .on_action(cx.listener(move |this, _: &ScrollDown, _, cx| scroll_by(handle(this), 40., cx)))
        .on_action(
            cx.listener(move |this, _: &PageUp, _, cx| scroll_by(handle(this), -page(this), cx)),
        )
        .on_action(
            cx.listener(move |this, _: &PageDown, _, cx| scroll_by(handle(this), page(this), cx)),
        )
        .on_action(cx.listener(move |this, _: &ScrollHome, _, cx| {
            scroll_by(handle(this), f32::NEG_INFINITY, cx)
        }))
        .on_action(
            cx.listener(move |this, _: &ScrollEnd, _, cx| {
                scroll_by(handle(this), f32::INFINITY, cx)
            }),
        )
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus_on_click, cx)
        })
}

/// Scroll down by `dy` (up when negative), within the content.
fn scroll_by<T: 'static>(handle: &ScrollHandle, dy: f32, cx: &mut Context<T>) {
    let offset = handle.offset();
    let max = f32::from(handle.max_offset().y);
    let y = (-f32::from(offset.y) + dy).clamp(0., max.max(0.));
    handle.set_offset(point(offset.x, px(-y)));
    cx.notify();
}

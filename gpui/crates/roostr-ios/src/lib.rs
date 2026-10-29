//! Roostr on iPhone and iPad: the desktop app's views (`roostr-app`) over
//! this device's own replica, on GPUI's UIKit platform (`gpui-pre-ios`). The
//! Xcode host in the RoostrIOS repository links this static library and
//! calls [`roostr_main`].
//!
//! The replica is the Swift app's: the change store is
//! `Application Support/Roostr/roostr.sqlite` (the Rust store migrates the
//! Swift schema in place), the identity and space keys stay in the Keychain
//! and the relay list in `UserDefaults` ([`local`]). Around it the app does
//! what the Swift shell did: reconnect sync when it returns to the
//! foreground and when the network path changes, mirror recurring objects
//! into local notifications ([`reminders`]), and open the object a
//! notification names.

#![cfg(target_os = "ios")]

mod local;
mod network;
mod reduce_motion;
mod reminders;

use std::ffi::c_int;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::*;
use roostr_core::layout::SyncNow;
use roostr_core::nav::Route;
use roostr_core::theme;
use roostr_replica::{ReplicaBackend, ReplicaParts};
use roostr_shell::Workspace;

/// The vault directory: `$ROOSTR_DB`'s directory when set (development),
/// else `Application Support/Roostr`, where the Swift app kept it.
fn database_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ROOSTR_DB") {
        return path.into();
    }
    let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
    home.join("Library/Application Support/Roostr/roostr.sqlite")
}

fn open_replica() -> anyhow::Result<ReplicaBackend> {
    let database = database_path();
    let dir = database
        .parent()
        .map_or_else(std::env::temp_dir, PathBuf::from);
    std::fs::create_dir_all(&dir)?;
    let store = futures::executor::block_on(roostr_replica_store::SqliteChangeStore::open(
        database.clone(),
    ))
    .map_err(|error| anyhow::anyhow!("{error}"))?;
    let kv = local::IosLocalKv::open(&dir.join("local.json"))?;
    // A fresh install starts lite; the Swift app's vault, when there is one, stays full.
    // Record sync unless `local.json` sets `roostr-records` to "0".
    let store = Arc::new(store);
    let backend = ReplicaBackend::new(ReplicaParts {
        engine: roostr_engine::Engine::native().map_err(|error| anyhow::anyhow!("{error}"))?,
        store: store.clone(),
        kv: Arc::new(kv),
        label: "this device's replica".into(),
    })
    .lite_by_default();
    futures::executor::block_on(backend.records_if_flagged(store))
}

/// The window content: the app inside the safe area and above the
/// on-screen keyboard, where the iOS web view sat.
struct AppRoot {
    workspace: Entity<Workspace>,
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let visible = window.fully_visible_bounds();
        div()
            .size_full()
            .bg(theme::gutter())
            .pt(visible.origin.y)
            .pl(visible.origin.x)
            .pr(viewport.width - visible.origin.x - visible.size.width)
            .pb(viewport.height - visible.origin.y - visible.size.height)
            .line_height(relative(theme::LINE_HEIGHT_NORMAL))
            .text_color(theme::fg())
            .child(div().size_full().child(self.workspace.clone()))
    }
}

/// Shown instead of the app when the replica can't be opened.
struct OpenError {
    message: SharedString,
}

impl Render for OpenError {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .pt(px(120.))
            .px(px(24.))
            .bg(theme::bg())
            .text_color(theme::fg())
            .font_family(theme::fonts::UI)
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::BOLD)
                    .child("Roostr can't open this device's replica"),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child(self.message.clone()),
            )
    }
}

/// What UIKit reports outside GPUI, delivered to the running app.
struct Signals {
    lifecycle: mpsc::UnboundedReceiver<AppLifecyclePhase>,
    network: mpsc::UnboundedReceiver<()>,
    taps: mpsc::UnboundedReceiver<String>,
    urls: mpsc::UnboundedReceiver<String>,
    keyboard_dismissed: mpsc::UnboundedReceiver<()>,
}

/// A deep link's web path: `roostr://app/app/object/<id>` → `/app/object/<id>`
/// (the Swift app served the site at `roostr://app/`).
fn link_path(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("roostr://")?;
    let path = rest.strip_prefix("app").unwrap_or(rest);
    path.starts_with('/').then_some(path)
}

/// Open `route` now, or once the shell has picked the space (a cold start).
fn open_route(route: Route, cx: &mut App) {
    if roostr_core::Navigator::global(cx)
        .read(cx)
        .space()
        .is_empty()
    {
        roostr_app::open_when_ready(route, cx);
    } else {
        roostr_core::nav::navigate(route, cx);
    }
}

fn start(backend: ReplicaBackend, signals: Signals, cx: &mut App) {
    let Signals {
        mut lifecycle,
        mut network,
        mut taps,
        mut urls,
        mut keyboard_dismissed,
    } = signals;
    roostr_core::init(Arc::new(backend.clone()), None, cx);
    cx.set_global(SyncNow(Rc::new({
        let backend = backend.clone();
        move |_| backend.resume()
    })));
    roostr_app::init(cx);
    reminders::follow(backend.clone(), cx).detach();

    // Returning from the background reconnects sync: a suspended app's
    // sockets can be dead without knowing it.
    let resume = backend.clone();
    cx.spawn(async move |_| {
        let mut backgrounded = false;
        while let Some(phase) = lifecycle.next().await {
            match phase {
                AppLifecyclePhase::Background | AppLifecyclePhase::Disconnected => {
                    backgrounded = true
                }
                AppLifecyclePhase::Active if backgrounded => {
                    backgrounded = false;
                    resume.resume();
                }
                _ => {}
            }
        }
    })
    .detach();
    cx.spawn(async move |_| {
        while network.next().await.is_some() {
            backend.resume();
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Some(object_id) = taps.next().await {
            cx.update(|cx| open_route(Route::Object(object_id), cx));
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Some(url) = urls.next().await {
            if let Some(path) = link_path(&url) {
                let route = Route::parse(path);
                cx.update(|cx| open_route(route, cx));
            }
        }
    })
    .detach();
    // Dismissing the keyboard ends editing, as it blurred the focused field
    // in the iOS web view: the editor then shows remote edits to the line
    // the caret was in (BlockNode.svelte re-renders every block but the
    // focused one).
    cx.spawn(async move |cx| {
        while keyboard_dismissed.next().await.is_some() {
            cx.update(|cx| {
                for window in cx.windows() {
                    let _ = window.update(cx, |_, window, cx| window.blur(cx));
                }
            });
        }
    })
    .detach();

    if let Err(error) = open_window(WindowOptions::default(), cx, |window, cx| {
        let workspace = roostr_app::workspace(window, cx);
        cx.new(|_| AppRoot { workspace })
    }) {
        log::error!("could not open the Roostr window: {error:#}");
    }
}

/// Make the bundled Twemoji known to CoreText by name, before GPUI loads
/// any font: every font's cascade falls back to it on iOS
/// (vendor/gpui-pre-apple).
fn register_emoji_font() {
    use std::ffi::c_void;
    #[link(name = "CoreText", kind = "framework")]
    unsafe extern "C" {
        fn CTFontManagerRegisterGraphicsFont(font: *mut c_void, error: *mut *mut c_void) -> bool;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDataProviderCreateWithData(
            info: *mut c_void,
            data: *const c_void,
            size: usize,
            release: Option<unsafe extern "C" fn(*mut c_void, *const c_void, usize)>,
        ) -> *mut c_void;
        fn CGFontCreateWithDataProvider(provider: *mut c_void) -> *mut c_void;
        fn CGDataProviderRelease(provider: *mut c_void);
        fn CGFontRelease(font: *mut c_void);
    }
    let bytes = theme::emoji_font_data_ios();
    unsafe {
        // The bytes are static, so the provider needs no release callback.
        let provider = CGDataProviderCreateWithData(
            std::ptr::null_mut(),
            bytes.as_ptr().cast(),
            bytes.len(),
            None,
        );
        let font = CGFontCreateWithDataProvider(provider);
        if font.is_null() || !CTFontManagerRegisterGraphicsFont(font, std::ptr::null_mut()) {
            log::warn!("could not register the emoji font with CoreText");
        }
        if !font.is_null() {
            CGFontRelease(font);
        }
        CGDataProviderRelease(provider);
    }
}

/// The app's entry point, called by the Xcode host's `main`. Runs UIKit's
/// main loop and never returns.
#[unsafe(no_mangle)]
pub extern "C" fn roostr_main() -> c_int {
    // Warnings and errors go to stderr (the device console); `RUST_LOG` overrides.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    register_emoji_font();
    // Before UIKit finishes launching, so a tap that launched the app arrives.
    let taps = reminders::install();
    let network = network::watch();
    let platform = gpui_ios::current_platform(false);
    let (phase_tx, lifecycle) = mpsc::unbounded();
    platform.on_app_lifecycle(Box::new(move |phase| {
        let _ = phase_tx.unbounded_send(phase);
    }));
    let (url_tx, urls) = mpsc::unbounded();
    let (dismissed_tx, keyboard_dismissed) = mpsc::unbounded();
    gpui_ios::ios::on_keyboard_dismissed(move || {
        let _ = dismissed_tx.unbounded_send(());
    });
    let app = Application::with_platform(platform).with_assets(roostr_core::Assets);
    app.on_open_urls(move |opened| {
        for url in opened {
            let _ = url_tx.unbounded_send(url);
        }
    });
    let signals = Signals {
        lifecycle,
        network,
        taps,
        urls,
        keyboard_dismissed,
    };
    app.run(move |cx| {
        gpui_kit::init(cx);
        reduce_motion::follow(cx);
        gpui_ios::ios::set_status_bar_style(gpui_ios::StatusBarContentStyle::Light);
        match open_replica() {
            Ok(backend) => start(backend, signals, cx),
            Err(error) => {
                log::error!("could not open the replica: {error:#}");
                theme::init(cx);
                let message = SharedString::from(format!("{error:#}"));
                if let Err(error) = open_window(WindowOptions::default(), cx, |_, cx| {
                    cx.new(|_| OpenError { message })
                }) {
                    log::error!("could not open the Roostr window: {error:#}");
                }
            }
        }
        cx.activate(true);
    });
    0
}

#[cfg(test)]
mod tests {
    use super::link_path;

    #[test]
    fn deep_links_name_web_paths() {
        assert_eq!(
            link_path("roostr://app/app/object/abc"),
            Some("/app/object/abc")
        );
        assert_eq!(link_path("roostr:///app/graph"), Some("/app/graph"));
        assert_eq!(link_path("https://roostr.space/app"), None);
    }
}

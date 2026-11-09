//! The app: UIKit's application and scene delegates, the window they build, and the state the
//! window runs: this device's replica with every feature registered, as the desktop binary
//! registers them (`crates/roostr/src/main.rs`).
//!
//! The replica is the Swift app's: the change store is `Application Support/Roostr/roostr.sqlite`
//! (the Rust store migrates the Swift schema in place), the identity and space keys stay in the
//! Keychain and the relay list in `UserDefaults` ([`crate::local`]). Around it the app does what
//! the Swift shell did: reconnect sync when it returns to the foreground and when the network
//! path changes, mirror recurring objects into local notifications ([`crate::reminders`]), and
//! open the object a notification or a `roostr://` link names.

use std::cell::RefCell;
use std::ffi::c_int;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use block2::RcBlock;
use futures::StreamExt as _;
use futures::channel::mpsc;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    ClassType, MainThreadMarker, MainThreadOnly, define_class, extern_class, extern_methods,
    msg_send, sel,
};
use objc2_core_foundation::CGRect;
use objc2_foundation::{
    NSDictionary, NSNotification, NSNotificationCenter, NSObject, NSSet, NSString, NSValue,
};
use objc2_ui_kit::{
    NSValueUIGeometryExtensions, UIApplication, UIApplicationDelegate,
    UIApplicationLaunchOptionsKey, UIColor, UIKeyboardFrameEndUserInfoKey,
    UIKeyboardWillChangeFrameNotification, UIKeyboardWillHideNotification, UIOpenURLContext,
    UIScene, UISceneConfiguration, UISceneConnectionOptions, UISceneDelegate, UISceneSession,
    UISceneWillConnectNotification, UIWindow, UIWindowScene, UIWindowSceneDelegate,
};
use roostr_core::State;
use roostr_core::layout::SyncNow;
use roostr_core::nav::Route;
use roostr_replica::{ReplicaBackend, ReplicaParts};
use roostr_ui::ty::Weights as _;
use roostr_ui::{ThemePref, ty};
use rune::{AppSpec, Cx, El, KeyEvent, Tx, col, text};

use crate::host::{Shell, shell};
use crate::links::link_path;
use crate::text_view::TextInputView;
use crate::views::{MetalView, ViewController};
use crate::{local, network, reduce_motion, reminders};

/// The window's size before UIKit lays it out (an iPhone's, in points).
const SIZE: (f32, f32) = (390., 844.);

/// Shown instead of the app when the replica can't be opened.
struct OpenError(String);

/// What UIKit reports outside the window, from before the scene connects.
struct Signals {
    taps: mpsc::UnboundedReceiver<String>,
    network: mpsc::UnboundedReceiver<()>,
}

thread_local! {
    static SIGNALS: RefCell<Option<Signals>> = const { RefCell::new(None) };
    /// Notification observers, kept for the life of the app.
    static OBSERVERS: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>> =
        const { RefCell::new(Vec::new()) };
}

/// Keeps a notification observer for the life of the app.
pub fn keep_observer(token: Retained<ProtocolObject<dyn NSObjectProtocol>>) {
    OBSERVERS.with_borrow_mut(|observers| observers.push(token));
}

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

/// The app's state over this device's replica, every feature registered and the device's
/// signals followed; or the reason the replica can't open.
fn start(signals: Option<Signals>) -> (State, Option<ReplicaBackend>) {
    let backend = match open_replica() {
        Ok(backend) => backend,
        Err(error) => {
            log::error!("could not open the replica: {error:#}");
            let mut s = State::unopened();
            s.put(OpenError(format!("{error:#}")));
            return (s, None);
        }
    };
    let mut s = State::new(Arc::new(backend.clone()), None);
    s.sync_now = Some(SyncNow(Rc::new({
        let backend = backend.clone();
        move |_| backend.resume()
    })));
    roostr_app::register(&mut s);
    reminders::follow(backend.clone(), &mut s);
    if let Some(Signals {
        mut taps,
        mut network,
    }) = signals
    {
        let resume = backend.clone();
        s.rt.spawn(async move |_| {
            while network.next().await.is_some() {
                resume.resume();
            }
        })
        .detach();
        s.rt.spawn(async move |cx| {
            while let Some(object_id) = taps.next().await {
                cx.update(|s| roostr_shell::open_when_ready(s, Route::Object(object_id)));
            }
        })
        .detach();
    }
    (s, Some(backend))
}

/// The window: the shell, its root filled with the gutter colour, which the host also paints
/// around the safe area (behind the status bar and the home indicator), where the iOS web view
/// and the GPUI build showed it.
fn view(s: &State, cx: &mut Cx<State>) -> El {
    match s.get::<OpenError>() {
        Some(error) => open_error(&error.0, cx),
        None => {
            let gutter = roostr_shell::theme(s, cx).gutter;
            roostr_shell::view(s, cx).fill(gutter)
        }
    }
}

/// The replica's failure.
fn open_error(message: &str, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    roostr_ui::root(
        col()
            .w_full()
            .h_full()
            .items_center()
            .gap(14.)
            .pt(120.)
            .px(24.)
            .fill(t.bg)
            .child(
                text("Roostr can't open this device's replica")
                    .font(ty::ui(16.).bold())
                    .color(t.fg),
            )
            .child(
                text(message.to_owned())
                    .font(ty::ui(13.))
                    .color(t.muted)
                    .wrap(),
            ),
        [],
    )
    .fill(t.bg)
}

fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    s.get::<OpenError>().is_none() && roostr_app::on_key(s, ev, tx)
}

/// Opens a `roostr://` link's page once the shell has picked the space.
fn open_link(url: &str) {
    let Some(path) = link_path(url) else {
        log::warn!("not a Roostr link: {url}");
        return;
    };
    let route = Route::parse(path);
    if let Some(shell) = shell() {
        shell.push(move |rt| {
            rt.update(|s, _| {
                if s.get::<OpenError>().is_none() {
                    roostr_shell::open_when_ready(s, route);
                }
            })
        });
    }
}

fn open_url_contexts(contexts: &NSSet<UIOpenURLContext>) {
    for context in contexts.iter() {
        if let Some(url) = context.URL().absoluteString() {
            open_link(&url.to_string());
        }
    }
}

/// Builds the window in `scene` and starts the app in it.
fn connect(scene: &UIWindowScene, options: Option<&SceneConnectionOptions>) {
    if shell().is_some() {
        log::error!("Roostr runs in one window scene");
        return;
    }
    let mtm = MainThreadMarker::from(scene);
    let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
    let controller = ViewController::new(mtm);
    let metal = MetalView::new(CGRect::ZERO, mtm);
    let text = TextInputView::new(mtm);
    metal.addSubview(&text);
    let Some(root) = controller.view() else {
        log::error!("the root view controller has no view");
        return;
    };
    // The launch screen's colour until the first frame paints the app's.
    let launch = UIColor::colorNamed(&NSString::from_str("LaunchBackground"))
        .unwrap_or_else(UIColor::blackColor);
    root.setBackgroundColor(Some(&launch));
    root.addSubview(&metal);
    window.setRootViewController(Some(&controller));

    let (state, backend) = start(SIGNALS.with_borrow_mut(Option::take));
    let spec = AppSpec {
        on_key: Some(on_key),
        // `LayerHost` takes the state built above; `init` is never called.
        ..roostr_host::spec("Roostr", SIZE, State::unopened, view)
    };
    let shell = Shell::install(
        window.clone(),
        controller,
        metal,
        text,
        spec,
        state,
        backend,
        mtm,
    );
    reduce_motion::follow();
    observe_keyboard();
    window.makeKeyAndVisible();
    root.setNeedsLayout();
    shell.request_tick();
    if let Some(contexts) = options.and_then(SceneConnectionOptions::url_contexts) {
        open_url_contexts(&contexts);
    }
}

/// The keyboard's frame: the Metal view stays above it.
fn observe_keyboard() {
    let changed = RcBlock::new(|notification: NonNull<NSNotification>| {
        let Some(shell) = shell() else {
            return;
        };
        // SAFETY: UIKit passes a valid notification.
        let notification = unsafe { notification.as_ref() };
        let Some(info) = notification.userInfo() else {
            return;
        };
        // SAFETY: a UIKit constant.
        let key = unsafe { UIKeyboardFrameEndUserInfoKey };
        let Some(frame) = info
            .objectForKey(key)
            .and_then(|value| value.downcast::<NSValue>().ok())
        else {
            return;
        };
        let Some(root) = shell.controller.view() else {
            return;
        };
        let space = shell.window.screen().coordinateSpace();
        // SAFETY: an NSValue from UIKit's keyboard frame key holds a CGRect.
        let frame = unsafe { frame.CGRectValue() };
        // SAFETY: `-[UIView convertRect:fromCoordinateSpace:]` with the screen's space.
        let frame: CGRect =
            unsafe { msg_send![&*root, convertRect: frame, fromCoordinateSpace: &*space] };
        let bounds = root.bounds();
        let bottom = bounds.origin.y + bounds.size.height;
        let overlap = if frame.size.height <= 0. || frame.size.width <= 0. {
            0.
        } else {
            bottom - frame.origin.y
        };
        shell
            .keyboard
            .set_overlap(&shell, overlap.clamp(0., bounds.size.height));
    });
    let hidden = RcBlock::new(|_: NonNull<NSNotification>| {
        if let Some(shell) = shell() {
            shell.keyboard.set_overlap(&shell, 0.);
        }
    });
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: UIKit's own notifications on the default center, delivered on the posting
    // (main) thread; the blocks only read them and update the shell.
    unsafe {
        keep_observer(center.addObserverForName_object_queue_usingBlock(
            Some(UIKeyboardWillChangeFrameNotification),
            None,
            None,
            &changed,
        ));
        keep_observer(center.addObserverForName_object_queue_usingBlock(
            Some(UIKeyboardWillHideNotification),
            None,
            None,
            &hidden,
        ));
    }
}

extern_class!(
    #[unsafe(super = NSObject)]
    #[name = "UISceneConnectionOptions"]
    struct SceneConnectionOptions;
);

impl SceneConnectionOptions {
    extern_methods!(
        // iOS 18 can return nil on an ordinary launch despite the SDK's nonnull annotation.
        #[unsafe(method(URLContexts))]
        #[unsafe(method_family = none)]
        fn url_contexts(&self) -> Option<Retained<NSSet<UIOpenURLContext>>>;
    );
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "RoostrAppDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    impl AppDelegate {
        // A scene session restored from the app this one replaced (the SwiftUI app, or the
        // GPUI build's `GPUIIosSceneDelegate`) keeps that app's delegate class: UIKit asks
        // `application:configurationForConnectingSceneSession:` only for new sessions, so the
        // restored scene connects to a delegate that hosts nothing. UIKit posts this right
        // after the delegate's `scene:willConnectToSession:options:`; Roostr's scene delegate
        // takes such a scene over here, on every launch, as the session keeps its
        // configuration.
        #[unsafe(method(sceneWillConnect:))]
        fn scene_will_connect(&self, notification: &NSNotification) {
            let Some(scene) = notification
                .object()
                .and_then(|object| object.downcast::<UIWindowScene>().ok())
            else {
                return;
            };
            let ours = scene
                .delegate()
                .is_some_and(|delegate| delegate.isKindOfClass(SceneDelegate::class()));
            if ours {
                return;
            }
            log::warn!("taking over a window scene restored with another scene delegate");
            let delegate: Retained<SceneDelegate> =
                unsafe { msg_send![SceneDelegate::alloc(self.mtm()), init] };
            scene.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            SCENE_DELEGATE.with_borrow_mut(|slot| *slot = Some(delegate));
            // The connection options went to the replaced delegate.
            connect(&scene, None);
        }
    }

    unsafe impl UIApplicationDelegate for AppDelegate {
        #[unsafe(method(application:didFinishLaunchingWithOptions:))]
        unsafe fn did_finish_launching(
            &self,
            _application: &UIApplication,
            _options: Option<&NSDictionary<UIApplicationLaunchOptionsKey, AnyObject>>,
        ) -> bool {
            unsafe {
                NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                    self,
                    sel!(sceneWillConnect:),
                    Some(UISceneWillConnectNotification),
                    None,
                );
            }
            true
        }

        #[unsafe(method_id(application:configurationForConnectingSceneSession:options:))]
        fn configuration_for_scene(
            &self,
            _application: &UIApplication,
            session: &UISceneSession,
            _options: &UISceneConnectionOptions,
        ) -> Retained<UISceneConfiguration> {
            let configuration = UISceneConfiguration::initWithName_sessionRole(
                UISceneConfiguration::alloc(self.mtm()),
                Some(&NSString::from_str("Default Configuration")),
                &session.role(),
            );
            unsafe { configuration.setDelegateClass(Some(SceneDelegate::class())) };
            configuration
        }
    }
);

thread_local! {
    /// A scene's delegate is held weakly by the scene: the one the app delegate installed
    /// itself lives here.
    static SCENE_DELEGATE: RefCell<Option<Retained<SceneDelegate>>> = const { RefCell::new(None) };
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "RoostrSceneDelegate"]
    struct SceneDelegate;

    unsafe impl NSObjectProtocol for SceneDelegate {}
    unsafe impl UIWindowSceneDelegate for SceneDelegate {}

    impl SceneDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn will_connect(
            &self,
            scene: &UIScene,
            _session: &UISceneSession,
            options: &SceneConnectionOptions,
        ) {
            if let Some(scene) = scene.downcast_ref::<UIWindowScene>() {
                connect(scene, Some(options));
            }
        }

        #[unsafe(method(sceneDidBecomeActive:))]
        fn did_become_active(&self, _scene: &UIScene) {
            if let Some(shell) = shell() {
                shell.set_active(true);
                shell.push(|rt| rt.window_focus(true));
            }
        }

        #[unsafe(method(sceneWillResignActive:))]
        fn will_resign_active(&self, _scene: &UIScene) {
            if let Some(shell) = shell() {
                shell.push(|rt| rt.window_focus(false));
            }
        }

        #[unsafe(method(sceneWillEnterForeground:))]
        fn will_enter_foreground(&self, _scene: &UIScene) {
            if let Some(shell) = shell() {
                shell.set_active(true);
            }
        }

        #[unsafe(method(sceneDidEnterBackground:))]
        fn did_enter_background(&self, _scene: &UIScene) {
            if let Some(shell) = shell() {
                shell.set_active(false);
            }
        }

        #[unsafe(method(scene:openURLContexts:))]
        fn open_url_contexts(&self, _scene: &UIScene, contexts: &NSSet<UIOpenURLContext>) {
            open_url_contexts(contexts);
        }
    }
);

/// The app's entry point, called by the Xcode host's `main`. Runs UIKit's main loop and never
/// returns.
#[unsafe(no_mangle)]
pub extern "C" fn roostr_main() -> c_int {
    // Warnings and errors go to stderr (the device console); `RUST_LOG` overrides.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    // Metal and its pipelines warm up while UIKit launches.
    rune::prewarm_gpu_on(rune::GpuBackend::Metal);
    rune::embed::prewarm();
    // Before UIKit finishes launching, so a tap that launched the app arrives.
    let taps = reminders::install();
    let network = network::watch();
    SIGNALS.with_borrow_mut(|slot| *slot = Some(Signals { taps, network }));
    let mtm = MainThreadMarker::new().expect("roostr_main runs on the main thread");
    // UIKit can restore a scene delegate by class name without asking for a configuration.
    SceneDelegate::class();
    let delegate = NSString::from_class(AppDelegate::class());
    UIApplication::main(None, Some(&delegate), mtm);
}

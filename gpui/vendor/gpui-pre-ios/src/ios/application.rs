use super::{CallbackSlot, platform::IosPlatformState, window::IosWindowState};
use gpui::{AnyWindowHandle, AppLifecyclePhase, WindowVisibility};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, extern_class,
    extern_methods, msg_send,
    rc::{Allocated, Retained},
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_foundation::{
    NSDictionary, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol, NSRunLoop,
    NSRunLoopCommonModes, NSSet, NSString,
};
use objc2_quartz_core::CADisplayLink;
use objc2_ui_kit::{
    UIApplication, UIApplicationDelegate, UIApplicationLaunchOptionsKey, UIOpenURLContext, UIScene,
    UISceneConfiguration, UISceneConnectionOptions, UISceneDelegate, UISceneSession,
    UISceneWillConnectNotification, UIWindowScene, UIWindowSceneDelegate,
};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

#[derive(Default)]
pub(super) struct IosApplicationState {
    running: Cell<bool>,
    finish_launching: CallbackSlot<Box<dyn FnOnce()>>,
    pub(super) quit: CallbackSlot<Box<dyn FnMut()>>,
    pub(super) open_urls: CallbackSlot<Box<dyn FnMut(Vec<String>)>>,
    pub(super) app_lifecycle: CallbackSlot<Box<dyn FnMut(AppLifecyclePhase)>>,
    pub(super) memory_warning: CallbackSlot<Box<dyn FnMut()>>,
    scene: RefCell<Option<Retained<UIWindowScene>>>,
    // The link retains its delegate target, which only weakly references this state.
    display_link: RefCell<Option<Retained<CADisplayLink>>>,
    windows: RefCell<Vec<Weak<IosWindowState>>>,
}

thread_local! {
    // UIKit constructs delegates by class name, including on scene restoration.
    // Only delegate initialization and the legacy status-bar setter consult this.
    static CURRENT_PLATFORM: RefCell<Weak<IosPlatformState>> = RefCell::default();
}

pub(super) fn current_platform() -> Option<Rc<IosPlatformState>> {
    CURRENT_PLATFORM.with_borrow(Weak::upgrade)
}

pub(super) fn run(platform: &Rc<IosPlatformState>, on_finish_launching: Box<dyn FnOnce()>) {
    let main_thread = MainThreadMarker::new().expect("UIKit requires the main thread");
    CURRENT_PLATFORM.with_borrow_mut(|current| {
        assert!(
            current.upgrade().is_none(),
            "the iOS application is already running"
        );
        *current = Rc::downgrade(platform);
    });
    assert!(
        !platform.application.running.replace(true),
        "the iOS application is already running"
    );
    platform
        .application
        .finish_launching
        .set(on_finish_launching);
    // UIKit can restore a scene delegate by class name without asking for a new configuration.
    SceneDelegate::class();
    let delegate_class = NSString::from_class(AppDelegate::class());
    UIApplication::main(None, Some(&delegate_class), main_thread);
}

impl IosApplicationState {
    fn set_display_link_paused(&self, paused: bool) {
        let display_link = self.display_link.borrow().clone();
        if let Some(display_link) = display_link {
            display_link.setPaused(paused);
        }
    }

    fn stop_display_link(&self) {
        let display_link = self.display_link.borrow_mut().take();
        if let Some(display_link) = display_link {
            display_link.invalidate();
        }
    }

    pub(super) fn window_scene(&self) -> Option<Retained<UIWindowScene>> {
        self.scene.borrow().clone()
    }

    pub(super) fn register_window(&self, window: &Rc<IosWindowState>) {
        self.windows.borrow_mut().push(Rc::downgrade(window));
    }

    pub(super) fn unregister_window(&self, window: &Rc<IosWindowState>) {
        let window = Rc::downgrade(window);
        self.windows
            .borrow_mut()
            .retain(|entry| !entry.ptr_eq(&window));
    }

    /// roostr: the window UIKit made key, else the newest open one. A scene
    /// shows one window, the one in use, as the browser's single window is.
    pub(super) fn active_window(&self) -> Option<AnyWindowHandle> {
        let windows: Vec<Rc<IosWindowState>> = self
            .windows
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        windows
            .iter()
            .find(|window| window.is_key())
            .or(windows.last())
            .map(|window| window.handle())
    }

    pub(super) fn with_windows(&self, mut callback: impl FnMut(&IosWindowState)) {
        // Native callbacks may open or close windows. Do not hold a registry borrow
        // across them, and skip snapshot entries that have since been unregistered.
        let windows = self.windows.borrow().clone();
        for window in windows {
            if self
                .windows
                .borrow()
                .iter()
                .any(|entry| entry.ptr_eq(&window))
            {
                // The snapshot's Weak keeps its allocation identity from being reused.
                // The strong guard survives synchronous closure of this window.
                if let Some(window) = window.upgrade() {
                    callback(&window);
                }
            }
        }
    }

    fn finish_launching(&self) {
        let callback = self.finish_launching.take();
        if let Some(callback) = callback {
            callback();
        }
    }

    fn notify_memory_warning(&self) {
        self.memory_warning.with(|callback| callback());
    }

    fn notify_app_lifecycle(&self, phase: AppLifecyclePhase) {
        match phase {
            AppLifecyclePhase::Foreground => self.with_windows(|window| {
                window.notify_visibility_change(WindowVisibility::Visible);
            }),
            AppLifecyclePhase::Active => {
                self.with_windows(|window| window.notify_active_status_change(true));
            }
            AppLifecyclePhase::Inactive => {
                self.with_windows(|window| window.notify_active_status_change(false));
            }
            AppLifecyclePhase::Background => {
                self.with_windows(|window| window.notify_active_status_change(false));
                // An activation observer may have closed a window; revisit the registry.
                self.with_windows(|window| {
                    window.notify_visibility_change(WindowVisibility::Hidden)
                });
            }
        }
        self.app_lifecycle.with(|callback| callback(phase));
    }

    fn open_urls(&self, contexts: &NSSet<UIOpenURLContext>) {
        if contexts.is_empty() {
            return;
        }
        let urls = contexts
            .iter()
            .filter_map(|context| context.URL().absoluteString().map(|url| url.to_string()))
            .collect();
        self.open_urls.with(|callback| callback(urls));
    }
}

impl Drop for IosApplicationState {
    fn drop(&mut self) {
        self.stop_display_link();
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
    #[name = "GPUIIosAppDelegate"]
    #[ivars = Weak<IosPlatformState>]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    impl AppDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let platform = current_platform()
                .map(|platform| Rc::downgrade(&platform))
                .unwrap_or_default();
            let this = this.set_ivars(platform);
            unsafe { msg_send![super(this), init] }
        }

        // roostr: a scene session restored from the app this one replaced (the
        // SwiftUI app, on an upgraded install) keeps that app's delegate class.
        // UIKit asks `application:configurationForConnectingSceneSession:` only
        // for new sessions, so the restored scene connects to SwiftUI's
        // `AppSceneDelegate`, which hosts nothing (a black screen) and traps
        // ("Missing scene item!") when UIKit saves the scene's restoration
        // state on backgrounding. Destroying the session does not help: the
        // system reconnects the same one. UIKit posts this notification right
        // after the delegate's `scene:willConnectToSession:options:`; GPUI's
        // scene delegate takes such a scene over here, on every launch, as
        // the session keeps its configuration. GPUI's delegate implements no
        // `stateRestorationActivityForScene:`, so UIKit saves no state for it.
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
            // The connection options went to the replaced delegate.
            delegate.connect(&scene, None);
        }
    }

    unsafe impl UIApplicationDelegate for AppDelegate {
        #[unsafe(method(application:didFinishLaunchingWithOptions:))]
        unsafe fn did_finish_launching(
            &self,
            _application: &UIApplication,
            _options: Option<&NSDictionary<UIApplicationLaunchOptionsKey, AnyObject>>,
        ) -> bool {
            // roostr: see `sceneWillConnect:`; scenes connect after launch.
            unsafe {
                NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                    self,
                    sel!(sceneWillConnect:),
                    Some(UISceneWillConnectNotification),
                    None,
                );
            }
            // GPUI's launch callback runs when a scene is ready to host its window.
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
            unsafe {
                configuration.setDelegateClass(Some(SceneDelegate::class()));
            }
            configuration
        }

        #[unsafe(method(applicationDidReceiveMemoryWarning:))]
        fn did_receive_memory_warning(&self, _application: &UIApplication) {
            if let Some(platform) = self.ivars().upgrade() {
                platform.application.notify_memory_warning();
            }
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _application: &UIApplication) {
            if let Some(platform) = self.ivars().upgrade() {
                if let Some(mut callback) = platform.application.quit.take() {
                    callback();
                }
            }
        }
    }
);

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "GPUIIosSceneDelegate"]
    #[ivars = Weak<IosPlatformState>]
    struct SceneDelegate;

    unsafe impl NSObjectProtocol for SceneDelegate {}
    unsafe impl UIWindowSceneDelegate for SceneDelegate {}

    impl SceneDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let platform = current_platform()
                .map(|platform| Rc::downgrade(&platform))
                .unwrap_or_default();
            let this = this.set_ivars(platform);
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(renderFrame:))]
        fn render_frame(&self, _display_link: &CADisplayLink) {
            if let Some(platform) = self.ivars().upgrade() {
                platform.application.with_windows(IosWindowState::request_frame);
            }
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
                self.connect(scene, options.url_contexts());
            }
        }

        #[unsafe(method(sceneDidDisconnect:))]
        fn did_disconnect(&self, scene: &UIScene) {
            if let Some(platform) = self.platform_for_scene(scene) {
                platform.application.stop_display_link();
                platform.application.scene.borrow_mut().take();
                // roostr: gpui-pre 0.3.9 has no `Disconnected` phase; a
                // disconnected scene is backgrounded.
                platform.application.notify_app_lifecycle(AppLifecyclePhase::Background);
            }
        }

        #[unsafe(method(sceneWillEnterForeground:))]
        fn will_enter_foreground(&self, scene: &UIScene) {
            if let Some(platform) = self.platform_for_scene(scene) {
                platform.application.set_display_link_paused(false);
                platform.application.notify_app_lifecycle(AppLifecyclePhase::Foreground);
            }
        }

        #[unsafe(method(sceneDidBecomeActive:))]
        fn did_become_active(&self, scene: &UIScene) {
            if let Some(platform) = self.platform_for_scene(scene) {
                platform.application.notify_app_lifecycle(AppLifecyclePhase::Active);
            }
        }

        #[unsafe(method(sceneWillResignActive:))]
        fn will_resign_active(&self, scene: &UIScene) {
            if let Some(platform) = self.platform_for_scene(scene) {
                platform.application.notify_app_lifecycle(AppLifecyclePhase::Inactive);
            }
        }

        #[unsafe(method(sceneDidEnterBackground:))]
        fn did_enter_background(&self, scene: &UIScene) {
            if let Some(platform) = self.platform_for_scene(scene) {
                platform.application.set_display_link_paused(true);
                platform.application.notify_app_lifecycle(AppLifecyclePhase::Background);
            }
        }

        #[unsafe(method(scene:openURLContexts:))]
        fn open_url_contexts(&self, scene: &UIScene, contexts: &NSSet<UIOpenURLContext>) {
            if let Some(platform) = self.platform_for_scene(scene) {
                platform.application.open_urls(contexts);
            }
        }
    }
);

impl SceneDelegate {
    fn connect(
        &self,
        scene: &UIWindowScene,
        url_contexts: Option<Retained<NSSet<UIOpenURLContext>>>,
    ) {
        let Some(platform) = self.ivars().upgrade() else {
            return;
        };
        let application = &platform.application;
        if application.window_scene().is_some() {
            log::error!("GPUI iOS currently supports one connected window scene");
            return;
        }
        *application.scene.borrow_mut() = Some(scene.retain());
        application.finish_launching();
        // UIKit can reconnect a scene without restarting the Rust application.
        application.with_windows(|window| window.attach_to_scene(scene));
        if let Some(contexts) = url_contexts {
            application.open_urls(&contexts);
        }

        application.stop_display_link();
        let display_link = unsafe {
            let display_link =
                CADisplayLink::displayLinkWithTarget_selector(self, sel!(renderFrame:));
            display_link.setPaused(true);
            display_link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes);
            display_link
        };
        *application.display_link.borrow_mut() = Some(display_link);
    }

    fn platform_for_scene(&self, scene: &UIScene) -> Option<Rc<IosPlatformState>> {
        let platform = self.ivars().upgrade()?;
        let owns_scene = platform
            .application
            .window_scene()
            .is_some_and(|current| std::ptr::eq::<UIScene>(&**current, scene));
        owns_scene.then_some(platform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn launch_callback_runs_once_and_can_register_callbacks() {
        let state = Rc::new(IosApplicationState::default());
        let launches = Rc::new(Cell::new(0));
        state.finish_launching.set(Box::new({
            let state = Rc::downgrade(&state);
            let launches = launches.clone();
            move || {
                launches.set(launches.get() + 1);
                state.upgrade().unwrap().memory_warning.set(Box::new(|| {}));
            }
        }));
        state.finish_launching();
        state.finish_launching();
        assert_eq!(launches.get(), 1);
        assert!(state.memory_warning.take().is_some());
    }

    #[test]
    fn lifecycle_callback_replacement_survives_dispatch() {
        let state = Rc::new(IosApplicationState::default());
        let events = Rc::new(RefCell::new(Vec::new()));
        state.app_lifecycle.set(Box::new({
            let state = Rc::downgrade(&state);
            let events = events.clone();
            move |phase| {
                events.borrow_mut().push((0, phase));
                state.upgrade().unwrap().app_lifecycle.set(Box::new({
                    let events = events.clone();
                    move |phase| events.borrow_mut().push((1, phase))
                }));
            }
        }));
        state.notify_app_lifecycle(AppLifecyclePhase::Foreground);
        state.notify_app_lifecycle(AppLifecyclePhase::Active);
        assert_eq!(
            *events.borrow(),
            [
                (0, AppLifecyclePhase::Foreground),
                (1, AppLifecyclePhase::Active)
            ]
        );
    }

    #[test]
    fn memory_warning_callback_is_restored_after_dispatch() {
        let state = Rc::new(IosApplicationState::default());
        let warnings = Rc::new(Cell::new(0));
        state.memory_warning.set(Box::new({
            let state = Rc::downgrade(&state);
            let warnings = warnings.clone();
            move || {
                warnings.set(warnings.get() + 1);
                state.upgrade().unwrap().quit.set(Box::new(|| {}));
            }
        }));
        state.notify_memory_warning();
        state.notify_memory_warning();
        assert_eq!(warnings.get(), 2);
        assert!(state.quit.take().is_some());
    }

    #[test]
    fn lifecycle_removal_survives_reentrant_dispatch() {
        let state = Rc::new(IosApplicationState::default());
        let events = Rc::new(RefCell::new(Vec::new()));
        state.app_lifecycle.set(Box::new({
            let state = Rc::downgrade(&state);
            let events = events.clone();
            move |phase| {
                events.borrow_mut().push(phase);
                let state = state.upgrade().unwrap();
                state.app_lifecycle.take();
                state.notify_app_lifecycle(AppLifecyclePhase::Inactive);
            }
        }));
        state.notify_app_lifecycle(AppLifecyclePhase::Background);
        state.notify_app_lifecycle(AppLifecyclePhase::Foreground);
        assert_eq!(*events.borrow(), [AppLifecyclePhase::Background]);
    }

    #[test]
    fn application_callbacks_are_not_shared_between_platforms() {
        let first = IosApplicationState::default();
        let second = IosApplicationState::default();
        let warnings = Rc::new(Cell::new(0));
        first.memory_warning.set(Box::new({
            let warnings = warnings.clone();
            move || warnings.set(warnings.get() + 1)
        }));
        second.notify_memory_warning();
        assert_eq!(warnings.get(), 0);
        first.notify_memory_warning();
        assert_eq!(warnings.get(), 1);
    }
}

//! The Rune runtime in the window: a [`LayerHost`] drawing Roostr's [`State`] into the Metal
//! view, driven by a `CADisplayLink` that runs only while there is something to do.
//!
//! The display link ticks while the last frame asked for another (springs, reveals, queued
//! input), while input waits, and after a wake: the waker the state's executor got
//! (`roostr_host::setup`) fires from any thread when a future or a download is ready, and a
//! timer fires when the runtime asked to wake later (`EmbedFrame::wake_after_secs`). Otherwise
//! the link is paused and the app draws nothing. In the background it stays paused: Metal may
//! not present there.
//!
//! UIKit's callbacks never touch the runtime directly: they queue jobs ([`Shell::push`]) that
//! the next tick runs before its frame, so a callback UIKit makes while a frame runs (a
//! keyboard notification from becoming first responder, say) can't re-enter it.
//!
//! The system pasteboard is mirrored through a Rune memory clipboard: what the app copies is
//! written to `UIPasteboard`; the pasteboard is read only when the person pastes (the edit
//! menu, ⌘V), so iOS never asks them to allow a paste the app made up.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dispatch2::{DispatchQueue, DispatchTime};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSRunLoop;
use objc2_quartz_core::CADisplayLink;
use objc2_ui_kit::{UIAccessibilityIsReduceMotionEnabled, UIColor, UITraitEnvironment, UIWindow};
use roostr_core::State;
use roostr_replica::ReplicaBackend;
use rune::clipboard::MemoryClipboard;
use rune::embed::LayerHost;
use rune::{AppSpec, ClipboardContent, Key, KeyEvent, Mods, Runtime, Waker};

use crate::keyboard::Keyboard;
use crate::pasteboard;
use crate::text_bridge::Input;
use crate::text_view::TextInputView;
use crate::views::{FrameTarget, MetalView, ViewController};

/// A change for the runtime, run on the next tick before its frame.
pub type Job = Box<dyn FnOnce(&mut Runtime<State>)>;

thread_local! {
    /// The one window's shell (one window scene).
    static SHELL: RefCell<Option<Rc<Shell>>> = const { RefCell::new(None) };
}

/// The window's shell, once the scene connected.
pub fn shell() -> Option<Rc<Shell>> {
    SHELL.with_borrow(Clone::clone)
}

/// The window, its views, and the runtime drawing into them.
pub struct Shell {
    pub window: Retained<UIWindow>,
    pub controller: Retained<ViewController>,
    pub view: Retained<MetalView>,
    pub text: Retained<TextInputView>,
    display_link: Retained<CADisplayLink>,
    spec: AppSpec<State>,
    /// The state, until the first tick with a size builds the runtime around it.
    state: RefCell<Option<State>>,
    host: RefCell<Option<LayerHost<State>>>,
    jobs: RefCell<Vec<Job>>,
    /// A wake is on its way to the main queue (set from any thread).
    woken: Arc<AtomicBool>,
    /// The current wake-up timer; a timer that fires after another was set does nothing.
    timer: Cell<u64>,
    /// The Metal view's size (points) and scale, until the next tick applies it.
    viewport: Cell<Option<((f32, f32), f32)>>,
    /// The scene is in the foreground: frames may present.
    active: Cell<bool>,
    /// The app went to the background since it was last active.
    backgrounded: Cell<bool>,
    pub clipboard: MemoryClipboard,
    /// What the pasteboard and the memory clipboard last agreed on.
    mirrored: RefCell<Option<ClipboardContent>>,
    pub keyboard: Keyboard,
    /// The replica, for reconnecting sync (`None` when it couldn't open).
    pub backend: Option<ReplicaBackend>,
    /// The colour behind the Metal view (the app root's fill), sRGB.
    background: Cell<[f32; 4]>,
    /// The status bar draws light content (the app is dark).
    pub light_status_bar: Cell<bool>,
}

impl Shell {
    /// Builds the window's shell and makes it current: `spec` and `state` become the runtime
    /// on the first tick once the view has a size.
    pub fn install(
        window: Retained<UIWindow>,
        controller: Retained<ViewController>,
        view: Retained<MetalView>,
        text: Retained<TextInputView>,
        spec: AppSpec<State>,
        state: State,
        backend: Option<ReplicaBackend>,
        mtm: MainThreadMarker,
    ) -> Rc<Shell> {
        let target = FrameTarget::new(mtm);
        // SAFETY: `FrameTarget` implements `tick:`; the link retains its target.
        let display_link = unsafe {
            let link = CADisplayLink::displayLinkWithTarget_selector(&target, objc2::sel!(tick:));
            link.setPaused(true);
            link.addToRunLoop_forMode(
                &NSRunLoop::mainRunLoop(),
                objc2_foundation::NSRunLoopCommonModes,
            );
            link
        };
        let shell = Rc::new(Shell {
            window,
            controller,
            view,
            text,
            display_link,
            spec,
            state: RefCell::new(Some(state)),
            host: RefCell::new(None),
            jobs: RefCell::new(Vec::new()),
            woken: Arc::new(AtomicBool::new(false)),
            timer: Cell::new(0),
            viewport: Cell::new(None),
            active: Cell::new(true),
            backgrounded: Cell::new(false),
            clipboard: rune::clipboard::memory(),
            mirrored: RefCell::new(None),
            keyboard: Keyboard::default(),
            backend,
            background: Cell::new([0.; 4]),
            light_status_bar: Cell::new(true),
        });
        SHELL.with_borrow_mut(|slot| *slot = Some(shell.clone()));
        shell
    }

    /// The waker the runtime and the state's executor get: from any thread, it asks the main
    /// queue for a tick.
    fn waker(&self) -> Waker {
        let woken = self.woken.clone();
        Arc::new(move || {
            if !woken.swap(true, Ordering::AcqRel) {
                DispatchQueue::main().exec_async(|| {
                    if let Some(shell) = shell() {
                        shell.request_tick();
                    }
                });
            }
        })
    }

    /// Runs the display link (a tick at the next vsync) unless the app is in the background.
    pub fn request_tick(&self) {
        if self.active.get() {
            self.display_link.setPaused(false);
        }
    }

    /// Queues `job` for the next tick.
    pub fn push(&self, job: impl FnOnce(&mut Runtime<State>) + 'static) {
        self.jobs.borrow_mut().push(Box::new(job));
        self.request_tick();
    }

    /// Queues the bridge's keyboard input.
    pub fn send(&self, inputs: Vec<Input>) {
        if inputs.is_empty() {
            return;
        }
        self.push(move |rt| {
            for input in inputs {
                match input {
                    Input::Key(ev) => rt.key(ev),
                    Input::Ime(ev) => rt.ime(ev),
                }
            }
        });
    }

    /// Queues a key as the hardware keyboard would send it.
    pub fn key(&self, key: Key, mods: Mods) {
        self.push(move |rt| rt.key(KeyEvent { key, mods }));
    }

    /// The person pasted (the edit menu's Paste, ⌘V): the pasteboard's content goes to the
    /// focused field (through its paste hook: the editor turns a link into a bookmark, copied
    /// blocks into blocks) or the window's paste handler, as a desktop paste does. It also
    /// becomes the memory clipboard's, which the app's own clipboard reads see.
    pub fn paste(&self) {
        let Some(content) = pasteboard::read() else {
            return;
        };
        self.clipboard.set_content(content.clone());
        *self.mirrored.borrow_mut() = Some(content.clone());
        self.push(move |rt| rt.paste_content(content));
    }

    /// The Metal view's new size (points) and scale.
    pub fn set_viewport(&self, size: (f32, f32), scale: f32) {
        self.viewport.set(Some((size, scale)));
        self.request_tick();
    }

    /// The scene came to the foreground (`true`) or went to the background (`false`), where
    /// Metal may not present: the display link stays paused there.
    pub fn set_active(&self, active: bool) {
        if self.active.replace(active) == active {
            return;
        }
        if active {
            if self.backgrounded.replace(false)
                && let Some(backend) = &self.backend
            {
                // A suspended app's sockets can be dead without knowing it.
                backend.resume();
            }
            self.request_tick();
        } else {
            self.backgrounded.set(true);
            self.display_link.setPaused(true);
        }
    }

    /// The system's light/dark appearance changed.
    pub fn set_dark(&self, dark: bool) {
        self.push(move |rt| rt.set_system_dark(dark));
    }

    /// Settings → Accessibility → Reduce Motion changed.
    pub fn set_reduced_motion(&self, reduced: bool) {
        self.push(move |rt| rt.set_system_reduced_motion(reduced));
    }

    fn dark(&self) -> bool {
        crate::views::is_dark(&self.view.traitCollection())
    }

    /// Builds the runtime once the view has a size.
    fn ensure_host(&self) {
        if self.host.borrow().is_some() {
            return;
        }
        let Some((size, scale)) = self.viewport.get() else {
            return;
        };
        let Some(state) = self.state.borrow_mut().take() else {
            return;
        };
        self.viewport.set(None);
        let layer: *mut c_void = Retained::as_ptr(&self.view.layer()).cast_mut().cast();
        // SAFETY: the view owns the layer and outlives the host (both live as long as the
        // shell); this runs on the main thread.
        let host = unsafe {
            LayerHost::from_core_animation_layer(
                layer,
                &self.spec,
                state,
                size,
                scale,
                self.dark(),
                self.waker(),
            )
        };
        match host {
            Ok(mut host) => {
                let rt = host.runtime_mut();
                rt.use_clipboard(&self.clipboard);
                rt.set_system_reduced_motion(UIAccessibilityIsReduceMotionEnabled());
                rt.window_focus(true);
                *self.host.borrow_mut() = Some(host);
            }
            Err(error) => log::error!("Roostr can't draw: {error}"),
        }
    }

    /// One display-link tick: queued input and changes, a frame, then what the frame changed
    /// for UIKit (the keyboard, the pasteboard, the colour behind the app), and whether to keep
    /// ticking.
    pub fn tick(&self) {
        self.woken.store(false, Ordering::Release);
        if !self.active.get() {
            self.display_link.setPaused(true);
            return;
        }
        self.ensure_host();
        let jobs = std::mem::take(&mut *self.jobs.borrow_mut());
        let drawn = {
            let mut host = self.host.borrow_mut();
            let Some(host) = host.as_mut() else {
                self.jobs.borrow_mut().splice(0..0, jobs);
                self.display_link.setPaused(true);
                return;
            };
            if let Some((size, scale)) = self.viewport.take() {
                host.resize(size, scale);
            }
            for job in jobs {
                job(host.runtime_mut());
            }
            let frame = host.frame();
            let rt = host.runtime();
            Drawn {
                animating: frame.animating,
                wake_after: frame.wake_after_secs,
                ime_area: rt.ime_area,
                secure: rt.has_secure_focus(),
                clear: rt.clear_color(),
            }
        };
        self.mirror_clipboard();
        let settling = self.keyboard.sync(self, drawn.ime_area, drawn.secure);
        self.set_background(drawn.clear);
        let busy = drawn.animating
            || settling
            || !self.jobs.borrow().is_empty()
            || self.woken.load(Ordering::Acquire);
        self.display_link.setPaused(!busy);
        if !busy && let Some(after) = drawn.wake_after {
            self.wake_in(after);
        }
    }

    /// A tick in `seconds`, unless another timer replaces this one first.
    fn wake_in(&self, seconds: f64) {
        let generation = self.timer.get().wrapping_add(1);
        self.timer.set(generation);
        let Ok(when) = DispatchTime::try_from(Duration::from_secs_f64(seconds.max(0.))) else {
            return;
        };
        let _ = DispatchQueue::main().after(when, move || {
            if let Some(shell) = shell()
                && shell.timer.get() == generation
            {
                shell.request_tick();
            }
        });
    }

    /// Writes what the app copied (or cleared: a secret copy expiring) to the pasteboard.
    fn mirror_clipboard(&self) {
        let content = self.clipboard.content();
        let mut mirrored = self.mirrored.borrow_mut();
        if same_content(content.as_ref(), mirrored.as_ref()) {
            return;
        }
        pasteboard::write(content.as_ref());
        *mirrored = content;
    }

    /// The colour around the app (behind the status bar and the home indicator, and under the
    /// keyboard as it moves), and the status bar's content to match it.
    fn set_background(&self, color: [f32; 4]) {
        if self.background.replace(color) == color {
            return;
        }
        let [r, g, b, _] = color.map(f64::from);
        let ui = UIColor::colorWithRed_green_blue_alpha(r, g, b, 1.);
        if let Some(root) = self.controller.view() {
            root.setBackgroundColor(Some(&ui));
        }
        let light = 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5;
        if self.light_status_bar.replace(light) != light {
            self.controller.setNeedsStatusBarAppearanceUpdate();
        }
    }

    /// Lays the Metal view out inside the safe area and above the keyboard.
    pub fn layout(&self) {
        let Some(root) = self.controller.view() else {
            return;
        };
        let bounds = root.bounds();
        let insets = root.safeAreaInsets();
        let bottom = insets.bottom.max(self.keyboard.overlap());
        let frame = CGRect::new(
            CGPoint::new(insets.left, insets.top),
            CGSize::new(
                (bounds.size.width - insets.left - insets.right).max(0.),
                (bounds.size.height - insets.top - bottom).max(0.),
            ),
        );
        if self.view.frame() != frame {
            self.view.setFrame(frame);
        }
        let scale = self.window.screen().scale();
        self.view.layer().setContentsScale(scale);
        self.set_viewport(
            (frame.size.width as f32, frame.size.height as f32),
            scale as f32,
        );
    }
}

/// What a frame left for UIKit.
struct Drawn {
    animating: bool,
    wake_after: Option<f64>,
    ime_area: Option<[f32; 4]>,
    secure: bool,
    clear: [f32; 4],
}

fn same_content(a: Option<&ClipboardContent>, b: Option<&ClipboardContent>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.text() == b.text() && a.html() == b.html(),
        _ => false,
    }
}

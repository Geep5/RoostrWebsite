//! The window's UIKit views: the root view controller, the Metal view Rune draws into (touches,
//! hardware keys, trackpad scrolling, the edit menu), and the display link's target.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_core_foundation::CGRect;
use objc2_foundation::{NSObject, NSSet};
use objc2_quartz_core::{CADisplayLink, CAMetalLayer};
use objc2_ui_kit::{
    UIEditMenuConfiguration, UIEditMenuInteraction, UIEditMenuInteractionAnimating,
    UIEditMenuInteractionDelegate, UIEvent, UIGestureRecognizerState, UIKeyModifierFlags,
    UIPanGestureRecognizer, UIPasteboard, UIPress, UIPressesEvent, UIScrollTypeMask,
    UIStatusBarStyle, UITouch, UITouchPhase, UITraitCollection, UITraitEnvironment,
    UIUserInterfaceStyle, UIView, UIViewController,
};
use rune::{Key, Mods, TouchPhase};

use crate::host::shell;
use crate::text_bridge::{Input, Press, Pressed, hardware_key};

define_class!(
    /// The display link's target: each vsync while unpaused, one tick of the shell.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "RoostrFrameTarget"]
    pub struct FrameTarget;

    unsafe impl NSObjectProtocol for FrameTarget {}

    impl FrameTarget {
        #[unsafe(method(tick:))]
        fn tick(&self, _link: &CADisplayLink) {
            if let Some(shell) = shell() {
                shell.tick();
            }
        }
    }
);

impl FrameTarget {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    /// The window's root: lays the Metal view out in the safe area, above the keyboard.
    #[unsafe(super = UIViewController)]
    #[thread_kind = MainThreadOnly]
    #[name = "RoostrViewController"]
    pub struct ViewController;

    unsafe impl NSObjectProtocol for ViewController {}

    impl ViewController {
        #[unsafe(method(preferredStatusBarStyle))]
        fn preferred_status_bar_style(&self) -> UIStatusBarStyle {
            if shell().is_none_or(|shell| shell.light_status_bar.get()) {
                UIStatusBarStyle::LightContent
            } else {
                UIStatusBarStyle::DarkContent
            }
        }

        #[unsafe(method(viewDidLayoutSubviews))]
        fn view_did_layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidLayoutSubviews] };
            if let Some(shell) = shell() {
                shell.layout();
            }
        }

        #[unsafe(method(traitCollectionDidChange:))]
        fn trait_collection_did_change(&self, previous: Option<&UITraitCollection>) {
            let _: () = unsafe { msg_send![super(self), traitCollectionDidChange: previous] };
            if let Some(shell) = shell() {
                let dark = is_dark(&self.traitCollection());
                if previous.is_none_or(|p| is_dark(p) != dark) {
                    shell.set_dark(dark);
                }
                shell.layout();
            }
        }
    }
);

/// Whether `traits` are the dark appearance.
pub fn is_dark(traits: &UITraitCollection) -> bool {
    // SAFETY: a trait collection UIKit handed the main thread.
    unsafe { traits.userInterfaceStyle() == UIUserInterfaceStyle::Dark }
}

impl ViewController {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    /// Rune's surface: a `CAMetalLayer`-backed view that hands touches, hardware keys and
    /// trackpad scrolls to the shell.
    #[unsafe(super = UIView)]
    #[thread_kind = MainThreadOnly]
    #[name = "RoostrMetalView"]
    #[ivars = Cell<(f64, f64)>]
    pub struct MetalView;

    unsafe impl NSObjectProtocol for MetalView {}

    impl MetalView {
        #[unsafe(method(layerClass))]
        fn layer_class() -> &'static AnyClass {
            CAMetalLayer::class()
        }

        #[unsafe(method(canBecomeFirstResponder))]
        fn can_become_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.touches(touches);
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn touches_moved(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.touches(touches);
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.touches(touches);
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.touches(touches);
        }

        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let rest = presses_began(presses, false, false);
            if !rest.is_empty() {
                unsafe { msg_send![super(self), pressesBegan: &*rest, withEvent: event] }
            }
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let rest = presses_ended(presses);
            if !rest.is_empty() {
                unsafe { msg_send![super(self), pressesEnded: &*rest, withEvent: event] }
            }
        }

        #[unsafe(method(pressesCancelled:withEvent:))]
        fn presses_cancelled(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let rest = presses_ended(presses);
            if !rest.is_empty() {
                unsafe { msg_send![super(self), pressesCancelled: &*rest, withEvent: event] }
            }
        }

        // The edit menu's commands and ⌘V, which UIKit sends up the responder chain from the
        // text input view (a subview) when it is the first responder.
        #[unsafe(method(canPerformAction:withSender:))]
        fn can_perform_action(&self, action: Sel, sender: Option<&AnyObject>) -> bool {
            match can_perform(action) {
                Some(can) => can,
                None => unsafe { msg_send![super(self), canPerformAction: action, withSender: sender] },
            }
        }

        #[unsafe(method(cut:))]
        fn cut(&self, _sender: Option<&AnyObject>) {
            perform(sel!(cut:));
        }

        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            perform(sel!(copy:));
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) {
            perform(sel!(paste:));
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: Option<&AnyObject>) {
            perform(sel!(selectAll:));
        }

        /// A trackpad or mouse-wheel scroll (iPad): Rune's wheel.
        #[unsafe(method(handleScroll:))]
        fn handle_scroll(&self, recognizer: &UIPanGestureRecognizer) {
            let state = recognizer.state();
            let translation = recognizer.translationInView(Some(self));
            if state == UIGestureRecognizerState::Began {
                self.ivars().set((0., 0.));
            }
            let (lx, ly) = self.ivars().replace((translation.x, translation.y));
            let (dx, dy) = ((translation.x - lx) as f32, (translation.y - ly) as f32);
            let at = recognizer.locationInView(Some(self));
            let (x, y) = (at.x as f32, at.y as f32);
            if let Some(shell) = shell() {
                shell.push(move |rt| rt.wheel_xy(x, y, dx, dy));
            }
        }
    }

    unsafe impl UIEditMenuInteractionDelegate for MetalView {
        #[unsafe(method(editMenuInteraction:willPresentMenuForConfiguration:animator:))]
        fn edit_menu_will_present(
            &self,
            _interaction: &UIEditMenuInteraction,
            _configuration: &UIEditMenuConfiguration,
            _animator: &ProtocolObject<dyn UIEditMenuInteractionAnimating>,
        ) {
            if let Some(shell) = shell() {
                shell.keyboard.menu_shown.set(true);
            }
        }

        #[unsafe(method(editMenuInteraction:willDismissMenuForConfiguration:animator:))]
        fn edit_menu_will_dismiss(
            &self,
            _interaction: &UIEditMenuInteraction,
            _configuration: &UIEditMenuConfiguration,
            _animator: &ProtocolObject<dyn UIEditMenuInteractionAnimating>,
        ) {
            if let Some(shell) = shell() {
                shell.keyboard.menu_shown.set(false);
            }
        }
    }
);

impl MetalView {
    pub fn new(frame: CGRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Cell::new((0., 0.)));
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setUserInteractionEnabled(true);
        this.setMultipleTouchEnabled(true);
        // Trackpad and mouse-wheel scrolls reach a UIKit app only through a pan recognizer
        // that accepts scroll types. With no touch types it never claims a finger, so fingers
        // keep reaching `touchesBegan` and Rune's own panning.
        let target: &AnyObject = &this;
        // SAFETY: the view implements `handleScroll:` and owns the recognizer.
        let scroll = unsafe {
            UIPanGestureRecognizer::initWithTarget_action(
                UIPanGestureRecognizer::alloc(mtm),
                Some(target),
                Some(sel!(handleScroll:)),
            )
        };
        scroll.setAllowedScrollTypesMask(UIScrollTypeMask::All);
        // No touch types: fingers stay Rune's.
        scroll.setAllowedTouchTypes(&objc2_foundation::NSArray::new());
        scroll.setCancelsTouchesInView(false);
        this.addGestureRecognizer(&scroll);
        this
    }

    /// Every finger as Rune's touches, `id` its `UITouch` (stable for the touch's life).
    fn touches(&self, touches: &NSSet<UITouch>) {
        let Some(shell) = shell() else {
            return;
        };
        for touch in touches.iter() {
            let phase = match touch.phase() {
                UITouchPhase::Began => TouchPhase::Started,
                UITouchPhase::Moved | UITouchPhase::Stationary => TouchPhase::Moved,
                UITouchPhase::Ended => TouchPhase::Ended,
                UITouchPhase::Cancelled => TouchPhase::Cancelled,
                _ => continue,
            };
            let id = Retained::as_ptr(&touch) as usize as u64;
            let at = touch.locationInView(Some(self));
            let (x, y) = (at.x as f32, at.y as f32);
            let time = touch.timestamp();
            shell.keyboard.touch(&shell, id, phase, (x, y));
            shell.push(move |rt| rt.touch(id, phase, x, y, time));
        }
    }
}

fn mods(flags: UIKeyModifierFlags) -> Mods {
    let cmd = flags.contains(UIKeyModifierFlags::Command);
    Mods {
        shift: flags.contains(UIKeyModifierFlags::Shift),
        cmd,
        alt: flags.contains(UIKeyModifierFlags::Alternate),
        ctrl: flags.contains(UIKeyModifierFlags::Control),
        logo: cmd,
    }
}

/// Hardware key presses: the ones the shell sends Rune itself, and the rest for UIKit (returned,
/// to pass on to `super`). `typing`: the text input view is the first responder.
pub fn presses_began(
    presses: &NSSet<UIPress>,
    typing: bool,
    composing: bool,
) -> Retained<NSSet<UIPress>> {
    let (Some(shell), Some(mtm)) = (shell(), MainThreadMarker::new()) else {
        return presses.copy_set();
    };
    let mut rest = Vec::new();
    for press in presses.iter() {
        let Some(key) = press.key(mtm) else {
            rest.push(press);
            continue;
        };
        let event = Press {
            code: key.keyCode().0,
            base: key.charactersIgnoringModifiers().to_string(),
            characters: key.characters().to_string(),
            mods: mods(key.modifierFlags()),
        };
        match hardware_key(&event, typing, composing) {
            Pressed::Key(ev) => {
                shell
                    .keyboard
                    .presses
                    .borrow_mut()
                    .insert(Retained::as_ptr(&press) as usize);
                if typing {
                    // The key may move the caret anywhere, or focus away.
                    let secure = shell.keyboard.doc.borrow().secure();
                    let out = shell.keyboard.doc.borrow_mut().restart(secure);
                    shell.send(out);
                }
                shell.keyboard.selected_all.set(ev.key == Key::Command('a'));
                shell.send(vec![Input::Key(ev)]);
            }
            Pressed::Paste => {
                shell
                    .keyboard
                    .presses
                    .borrow_mut()
                    .insert(Retained::as_ptr(&press) as usize);
                perform(sel!(paste:));
            }
            Pressed::Text => rest.push(press),
        }
    }
    NSSet::from_retained_slice(&rest)
}

/// Presses ending: the ones the shell took are forgotten; the rest are UIKit's.
pub fn presses_ended(presses: &NSSet<UIPress>) -> Retained<NSSet<UIPress>> {
    let Some(shell) = shell() else {
        return presses.copy_set();
    };
    let mut handled = shell.keyboard.presses.borrow_mut();
    let rest: Vec<_> = presses
        .iter()
        .filter(|press| !handled.remove(&(Retained::as_ptr(press) as usize)))
        .collect();
    NSSet::from_retained_slice(&rest)
}

trait CopySet {
    fn copy_set(&self) -> Retained<NSSet<UIPress>>;
}

impl CopySet for NSSet<UIPress> {
    fn copy_set(&self) -> Retained<NSSet<UIPress>> {
        let all: Vec<_> = self.iter().collect();
        NSSet::from_retained_slice(&all)
    }
}

/// Whether the edit command `action` applies now; `None` when it is no edit command.
pub fn can_perform(action: Sel) -> Option<bool> {
    let shell = shell()?;
    let keyboard = &shell.keyboard;
    if action == sel!(paste:) {
        // `hasStrings` reads no contents: iOS asks nothing.
        Some(unsafe { UIPasteboard::generalPasteboard().hasStrings() })
    } else if action == sel!(selectAll:) {
        Some(keyboard.focused())
    } else if action == sel!(copy:) || action == sel!(cut:) {
        Some(keyboard.focused() && keyboard.selected_all.get() && !keyboard.doc.borrow().secure())
    } else {
        None
    }
}

/// Runs edit command `action` as its desktop shortcut would.
pub fn perform(action: Sel) {
    let Some(shell) = shell() else {
        return;
    };
    let command = Mods {
        cmd: true,
        logo: true,
        ..Mods::default()
    };
    let keyboard = &shell.keyboard;
    if keyboard.focused() {
        let secure = keyboard.doc.borrow().secure();
        let out = keyboard.doc.borrow_mut().restart(secure);
        shell.send(out);
    }
    if action == sel!(paste:) {
        keyboard.selected_all.set(false);
        shell.paste();
    } else if action == sel!(selectAll:) {
        keyboard.selected_all.set(true);
        shell.key(Key::Command('a'), command);
        // The menu stays up over what Select All selected, now with Cut and Copy.
        if keyboard.menu_shown.get() {
            keyboard.show_menu(&shell);
        }
    } else if action == sel!(copy:) {
        shell.key(Key::Command('c'), command);
    } else if action == sel!(cut:) {
        keyboard.selected_all.set(false);
        shell.key(Key::Command('x'), command);
    }
}

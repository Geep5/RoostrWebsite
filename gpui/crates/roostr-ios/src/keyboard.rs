//! When the on-screen keyboard is up, and what it is told.
//!
//! The text input view ([`crate::text_view`]) is the first responder, so the keyboard shows,
//! while a Rune text field or the editor has the caret (`Runtime::ime_area`, or a secure field
//! with focus) and the person asked for it. As iOS WebKit does, focus raises the keyboard only
//! when it comes from a tap (a field focused within [`TAP_FOCUS`] of one, or a tap on the line
//! the caret is in), moves on from a field whose keyboard is up, or with a hardware keyboard
//! attached; a field the app focuses on its own (a page's autofocus) waits for a tap.
//!
//! Dismissing the keyboard (the iPad's hide key, a swipe down over the content) ends editing,
//! as it blurred the focused field in the iOS web view. A tap on the caret, or a press held
//! [`HOLD`] in the field, shows the edit menu (Paste, Select All; Cut and Copy over what Select
//! All selected).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, msg_send};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_ui_kit::{
    UIEditMenuConfiguration, UIEditMenuInteraction, UITextInput, UITextInputDelegate,
};
use rune::TouchPhase;

use crate::host::Shell;
use crate::text_bridge::{Document, visible_caret};

/// How long after a tap a field the app focuses still counts as tapped (an editor creating the
/// line that was tapped, a page opened by the tap focusing its title).
const TAP_FOCUS: Duration = Duration::from_secs(1);
/// How far (points) a finger travels before its touch is no tap (Rune's touch slop).
const TAP_SLOP: f32 = 8.;
/// A press held this long without moving shows the edit menu.
const HOLD: Duration = Duration::from_millis(500);
/// A downward drag this long while the keyboard is up dismisses it.
const DISMISS_DISTANCE: f32 = 24.;
/// How close (points) above or below the caret's line a tap counts as on it.
const LINE_SLOP: f32 = 16.;

/// A finger that may still be a tap.
#[derive(Clone, Copy)]
struct Track {
    id: u64,
    start: (f32, f32),
    at: Instant,
    moved: bool,
    /// The keyboard was up when it came down: a drag down may dismiss it.
    keyboard_up: bool,
    /// The edit menu was showing when it came down (the tap closes it, not reopens it).
    menu_was_open: bool,
}

/// A tap the next frame handles, then the keyboard looks at.
#[derive(Clone, Copy)]
struct Tap {
    at: (f32, f32),
    /// The caret before the tap.
    caret: Option<[f32; 4]>,
    held: bool,
    menu_was_open: bool,
}

#[derive(Default)]
pub struct Keyboard {
    /// The focused field's text as UIKit sees it.
    pub doc: RefCell<Document>,
    /// A field takes text.
    focused: Cell<bool>,
    /// The caret's rect in the Metal view, points.
    caret: Cell<Option<[f32; 4]>>,
    /// The keyboard should be up while a field takes text.
    wanted: Cell<bool>,
    /// The shell itself is changing the first responder.
    reconciling: Cell<bool>,
    /// Ticks since the field lost focus while the keyboard was up.
    unfocused_ticks: Cell<u8>,
    last_tap: Cell<Option<Instant>>,
    tap: Cell<Option<Tap>>,
    track: Cell<Option<Track>>,
    /// Points of the window the keyboard covers.
    overlap: Cell<f64>,
    /// Select All just selected the whole field (the edit menu offers Cut and Copy).
    pub selected_all: Cell<bool>,
    menu: RefCell<Option<Retained<UIEditMenuInteraction>>>,
    pub menu_shown: Cell<bool>,
    /// Hardware key presses the shell handled, which UIKit must not see end.
    pub presses: RefCell<HashSet<usize>>,
}

impl Keyboard {
    pub fn overlap(&self) -> f64 {
        self.overlap.get()
    }

    pub fn focused(&self) -> bool {
        self.focused.get()
    }

    pub fn caret(&self) -> Option<[f32; 4]> {
        self.caret.get()
    }

    /// Whether the shell is the one changing the first responder.
    pub fn reconciling(&self) -> bool {
        self.reconciling.get()
    }

    /// The keyboard now covers `overlap` points of the window's bottom. The Metal view shrinks
    /// above it; when it grew, the focused field scrolls into view.
    pub fn set_overlap(&self, shell: &Shell, overlap: f64) {
        let overlap = overlap.max(0.);
        let before = self.overlap.replace(overlap);
        if (before - overlap).abs() < 0.5 {
            return;
        }
        shell.layout();
        if overlap > before {
            shell.push(|rt| {
                rt.update(|_, tx| {
                    if let Some(focused) = tx.focused().map(str::to_owned) {
                        tx.scroll_into_view(focused);
                    }
                })
            });
        }
    }

    /// The person dismissed the keyboard: editing ends.
    pub fn dismissed(&self, shell: &Shell) {
        self.wanted.set(false);
        self.selected_all.set(false);
        shell.push(|rt| rt.update(|_, tx| tx.blur()));
    }

    /// A touch on the Metal view, before the runtime hears it.
    pub fn touch(&self, shell: &Shell, id: u64, phase: TouchPhase, at: (f32, f32)) {
        match phase {
            TouchPhase::Started => {
                let menu_was_open = self.dismiss_menu();
                if self.track.get().is_none() {
                    self.track.set(Some(Track {
                        id,
                        start: at,
                        at: Instant::now(),
                        moved: false,
                        keyboard_up: self.overlap.get() > 0. && shell.text.isFirstResponder(),
                        menu_was_open,
                    }));
                }
            }
            TouchPhase::Moved => {
                let Some(mut track) = self.track.get().filter(|t| t.id == id) else {
                    return;
                };
                let (dx, dy) = (at.0 - track.start.0, at.1 - track.start.1);
                if dx.hypot(dy) > TAP_SLOP {
                    track.moved = true;
                }
                if track.keyboard_up && dy >= DISMISS_DISTANCE && dy > dx.abs() * 1.25 {
                    track.keyboard_up = false;
                    self.dismissed(shell);
                }
                self.track.set(Some(track));
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                let Some(track) = self.track.get().filter(|t| t.id == id) else {
                    return;
                };
                self.track.set(None);
                // The touch may have moved the caret or selected: what UIKit knew is stale.
                if self.focused.get() {
                    let secure = self.doc.borrow().secure();
                    let out = self.doc.borrow_mut().restart(secure);
                    shell.send(out);
                }
                self.selected_all.set(false);
                if phase == TouchPhase::Ended && !track.moved {
                    self.last_tap.set(Some(Instant::now()));
                    self.tap.set(Some(Tap {
                        at,
                        caret: self.caret.get(),
                        held: track.at.elapsed() >= HOLD,
                        menu_was_open: track.menu_was_open,
                    }));
                }
            }
        }
    }

    /// After a frame: the first responder, the caret and the document follow the runtime.
    /// True while the keyboard waits a tick for a field-to-field hand-off.
    pub fn sync(&self, shell: &Shell, ime_area: Option<[f32; 4]>, secure: bool) -> bool {
        let text = &shell.text;
        let focused = ime_area.is_some() || secure;
        let was = self.focused.replace(focused);
        self.caret.set(ime_area);
        let tap = self.tap.take();
        let mut settling = false;
        if focused {
            self.unfocused_ticks.set(0);
            let changed_kind = self.doc.borrow().secure() != secure;
            if !was || changed_kind {
                let out = self.doc.borrow_mut().restart(secure);
                shell.send(out);
                let tapped = self
                    .last_tap
                    .get()
                    .is_some_and(|t| t.elapsed() <= TAP_FOCUS);
                if tapped || text.isFirstResponder() || hardware_keyboard_attached() {
                    self.wanted.set(true);
                }
                if changed_kind && text.isFirstResponder() {
                    text.reloadInputViews();
                }
            } else if let Some(tap) = tap {
                let on_line = ime_area.is_some_and(|c| {
                    tap.at.1 >= c[1] - LINE_SLOP && tap.at.1 <= c[1] + c[3] + LINE_SLOP
                });
                let on_caret = ime_area.is_some_and(|c| {
                    tap.caret == Some(c) && (tap.at.0 - (c[0] + c[2] / 2.)).abs() <= 24. && on_line
                });
                if !self.wanted.get() && on_line {
                    self.wanted.set(true);
                } else if self.wanted.get()
                    && text.isFirstResponder()
                    && (tap.held || on_caret)
                    && !tap.menu_was_open
                {
                    self.show_menu(shell);
                }
            }
        } else if text.isFirstResponder() && self.wanted.get() {
            // Focus may be moving to another field: keep the keyboard one more tick.
            let ticks = self.unfocused_ticks.get() + 1;
            self.unfocused_ticks.set(ticks);
            if ticks < 2 {
                settling = true;
            } else {
                self.wanted.set(false);
            }
        } else {
            self.wanted.set(false);
        }
        if !focused {
            self.selected_all.set(false);
            if was {
                let out = self.doc.borrow_mut().restart(false);
                shell.send(out);
            }
        }
        let up = self.wanted.get() && (focused || settling);
        if up && !text.isFirstResponder() {
            self.reconcile(|| text.becomeFirstResponder());
        } else if !up && text.isFirstResponder() {
            self.reconcile(|| text.resignFirstResponder());
            // Hardware keys keep reaching the app.
            shell.view.becomeFirstResponder();
        } else if !up && !shell.view.isFirstResponder() && !text.isFirstResponder() {
            shell.view.becomeFirstResponder();
        }
        if let Some(caret) = ime_area {
            let [x, y, w, h] = visible_caret(caret).map(f64::from);
            text.setFrame(CGRect::new(CGPoint::new(x, y), CGSize::new(w, h)));
        }
        if self.doc.borrow_mut().take_reset() {
            text.document_changed();
        }
        settling
    }

    fn reconcile(&self, f: impl FnOnce() -> bool) {
        let before = self.reconciling.replace(true);
        f();
        self.reconciling.set(before);
    }

    /// UIKit took the first responder from the text input view (the iPad's hide-keyboard key,
    /// another view): editing ends.
    pub fn resigned(&self, shell: &Shell) {
        if self.wanted.get() {
            self.dismissed(shell);
        }
    }

    /// The edit menu over the caret.
    pub fn show_menu(&self, shell: &Shell) {
        let Some(caret) = self.caret.get() else {
            return;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let menu = self
            .menu
            .borrow_mut()
            .get_or_insert_with(|| {
                let menu = UIEditMenuInteraction::initWithDelegate(
                    UIEditMenuInteraction::alloc(mtm),
                    Some(ProtocolObject::from_ref(&*shell.view)),
                );
                shell.view.addInteraction(ProtocolObject::from_ref(&*menu));
                menu
            })
            .clone();
        let point = CGPoint::new(f64::from(caret[0] + caret[2] / 2.), f64::from(caret[1]));
        // SAFETY: a nil identifier and a point in the interaction's view.
        let configuration = unsafe {
            UIEditMenuConfiguration::configurationWithIdentifier_sourcePoint(None, point, mtm)
        };
        menu.presentEditMenuWithConfiguration(&configuration);
    }

    /// Closes the edit menu; whether it was open.
    fn dismiss_menu(&self) -> bool {
        let shown = self.menu_shown.replace(false);
        if shown && let Some(menu) = &*self.menu.borrow() {
            menu.dismissMenu();
        }
        shown
    }
}

/// Tells UIKit the document changed under it (a new session): the keyboard reads the text
/// around the caret again (capitalization, predictions).
pub fn notify_document_changed(input: &ProtocolObject<dyn UITextInput>) {
    if let Some(delegate) = input.inputDelegate() {
        let input = Some(input);
        delegate.textWillChange(input);
        delegate.selectionWillChange(input);
        delegate.selectionDidChange(input);
        delegate.textDidChange(input);
    }
}

/// Whether a hardware keyboard is connected (GameController's `GCKeyboard.coalescedKeyboard`).
/// Focus then raises no on-screen keyboard, so every focus takes the first responder.
fn hardware_keyboard_attached() -> bool {
    #[link(name = "GameController", kind = "framework")]
    unsafe extern "C" {}
    let Some(class) = objc2::runtime::AnyClass::get(c"GCKeyboard") else {
        return false;
    };
    // SAFETY: `+[GCKeyboard coalescedKeyboard]` returns a nullable object.
    let keyboard: Option<Retained<AnyObject>> = unsafe { msg_send![class, coalescedKeyboard] };
    keyboard.is_some()
}

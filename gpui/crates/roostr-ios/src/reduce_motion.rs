//! Settings → Accessibility → Motion → Reduce Motion, followed live
//! (`UIAccessibilityIsReduceMotionEnabled` and its change notification): with it on, Rune
//! snaps every transition and the looping indicators hold still. The runtime reads the
//! setting when it starts ([`crate::host::Shell`]); this follows its changes.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue};
use objc2_ui_kit::{
    UIAccessibilityIsReduceMotionEnabled, UIAccessibilityReduceMotionStatusDidChangeNotification,
};

use crate::host::shell;

pub fn follow() {
    let observer = RcBlock::new(move |_: NonNull<NSNotification>| {
        if let Some(shell) = shell() {
            shell.set_reduced_motion(UIAccessibilityIsReduceMotionEnabled());
        }
    });
    // SAFETY: UIKit's own notification on the default center; the block runs on the main
    // queue and only reads the setting.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(UIAccessibilityReduceMotionStatusDidChangeNotification),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &observer,
        )
    };
    crate::app::keep_observer(token);
}

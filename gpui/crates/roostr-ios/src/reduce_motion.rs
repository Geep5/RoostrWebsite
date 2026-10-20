//! Settings → Accessibility → Motion → Reduce Motion, followed live
//! (`UIAccessibilityIsReduceMotionEnabled` and its change notification):
//! with it on, every transition snaps (`roostr_core::motion`) and the
//! looping indicators hold still.

use std::ptr::NonNull;

use block2::RcBlock;
use futures::channel::mpsc;
use gpui_kit::App;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue};
use objc2_ui_kit::{
    UIAccessibilityIsReduceMotionEnabled, UIAccessibilityReduceMotionStatusDidChangeNotification,
};

pub fn follow(cx: &mut App) {
    let (tx, changes) = mpsc::unbounded();
    let observer = RcBlock::new(move |_: NonNull<NSNotification>| {
        let _ = tx.unbounded_send(UIAccessibilityIsReduceMotionEnabled());
    });
    // SAFETY: UIKit's own notification on the default center; the block runs
    // on the main queue and only reads the setting and sends.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(UIAccessibilityReduceMotionStatusDidChangeNotification),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &observer,
        )
    };
    // Observed for the life of the app.
    std::mem::forget(token);
    roostr_core::motion::follow_reduce_motion(UIAccessibilityIsReduceMotionEnabled(), changes, cx);
}

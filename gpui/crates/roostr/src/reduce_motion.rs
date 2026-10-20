//! System Settings → Accessibility → Display → Reduce motion, followed live
//! (`NSWorkspace.accessibilityDisplayShouldReduceMotion` and its change
//! notification): with it on, every transition snaps (`roostr_core::motion`)
//! and the looping indicators hold still.

use std::ptr::NonNull;

use block2::RcBlock;
use futures::channel::mpsc;
use gpui_kit::App;
use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
use objc2_foundation::{NSNotification, NSOperationQueue};

pub fn follow(cx: &mut App) {
    let workspace = NSWorkspace::sharedWorkspace();
    let (tx, changes) = mpsc::unbounded();
    let observer = RcBlock::new(move |_: NonNull<NSNotification>| {
        let reduce = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        let _ = tx.unbounded_send(reduce);
    });
    // SAFETY: the notification is NSWorkspace's own, posted to its center;
    // the block runs on the main queue and only reads a property and sends.
    let token = unsafe {
        workspace
            .notificationCenter()
            .addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &observer,
            )
    };
    // Observed for the life of the app.
    std::mem::forget(token);
    roostr_core::motion::follow_reduce_motion(
        workspace.accessibilityDisplayShouldReduceMotion(),
        changes,
        cx,
    );
}

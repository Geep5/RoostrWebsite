//! Network path changes (`AppModel`'s `NWPathMonitor`): back online, or onto
//! another interface (Wi-Fi ⇄ cellular). Sockets bound to the old path are
//! dead, so sync reconnects at once instead of waiting for its keepalive.

use std::ffi::{CStr, c_char, c_int, c_void};

use block2::{DynBlock, RcBlock};
use futures::channel::mpsc;
use objc2::runtime::Bool;
use parking_lot::Mutex;

type NwObject = *mut c_void;

/// `nw_path_status_satisfied`.
const SATISFIED: c_int = 1;
/// `QOS_CLASS_UTILITY`.
const QOS_UTILITY: isize = 0x11;

#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    fn nw_path_monitor_create() -> NwObject;
    fn nw_path_monitor_set_update_handler(monitor: NwObject, handler: &DynBlock<dyn Fn(NwObject)>);
    fn nw_path_monitor_set_queue(monitor: NwObject, queue: *mut c_void);
    fn nw_path_monitor_start(monitor: NwObject);
    fn nw_path_get_status(path: NwObject) -> c_int;
    fn nw_path_enumerate_interfaces(
        path: NwObject,
        enumerator: &DynBlock<dyn Fn(NwObject) -> Bool>,
    );
    fn nw_interface_get_name(interface: NwObject) -> *const c_char;
}

unsafe extern "C" {
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
}

/// The interfaces of a satisfied path; `None` while offline.
fn interfaces(path: NwObject) -> Option<Vec<String>> {
    if unsafe { nw_path_get_status(path) } != SATISFIED {
        return None;
    }
    let names = std::sync::Arc::new(Mutex::new(Vec::new()));
    let collect = {
        let names = names.clone();
        RcBlock::new(move |interface: NwObject| {
            let name = unsafe { nw_interface_get_name(interface) };
            if !name.is_null() {
                let name = unsafe { CStr::from_ptr(name) };
                names.lock().push(name.to_string_lossy().into_owned());
            }
            Bool::YES
        })
    };
    unsafe { nw_path_enumerate_interfaces(path, &collect) };
    Some(std::mem::take(&mut *names.lock()))
}

/// Start monitoring for the life of the process; yields once per change
/// after the first path update.
pub fn watch() -> mpsc::UnboundedReceiver<()> {
    let (tx, rx) = mpsc::unbounded();
    // (seen an update, the last path's interfaces)
    let last: Mutex<(bool, Option<Vec<String>>)> = Mutex::new((false, None));
    let handler = RcBlock::new(move |path: NwObject| {
        let interfaces = interfaces(path);
        let mut last = last.lock();
        let changed = last.0 && interfaces.is_some() && interfaces != last.1;
        *last = (true, interfaces);
        if changed {
            let _ = tx.unbounded_send(());
        }
    });
    unsafe {
        let monitor = nw_path_monitor_create();
        nw_path_monitor_set_update_handler(monitor, &handler);
        nw_path_monitor_set_queue(monitor, dispatch_get_global_queue(QOS_UTILITY, 0));
        // The monitor runs until the process exits; its reference is never released.
        nw_path_monitor_start(monitor);
    }
    rx
}

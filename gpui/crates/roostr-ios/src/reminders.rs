//! Device reminders for recurring objects (`Reminders.swift`). The phone runs
//! no scheduler and no agents; it only knows every object's next occurrence
//! from the replica, so after each commit it mirrors the soonest ones into
//! pending local notifications. Tapping one opens the object.
//!
//! iOS keeps at most 64 pending notifications per app; the soonest 64 are
//! scheduled and the set is rebuilt on every change, so nothing drifts.
//! Permission is asked the first time there is something to remind about,
//! never on a bare launch; a denial leaves the rest of the app untouched.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ptr::NonNull;
use std::time::Duration;

use block2::{DynBlock, RcBlock};
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::{App, Task};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSettings, UNNotificationSound, UNTimeIntervalNotificationTrigger,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use parking_lot::Mutex;
use roostr_core::Backend as _;
use roostr_core::backend::BackendEvent;
use roostr_replica::{Occurrence, ReplicaBackend};

const PREFIX: &str = "repeat:";
const OBJECT_KEY: &str = "objectId";
const LIMIT: usize = 64;

define_class!(
    #[unsafe(super = NSObject)]
    #[name = "RoostrNotificationDelegate"]
    #[ivars = mpsc::UnboundedSender<String>]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion.call((
                UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::Sound,
            ));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &DynBlock<dyn Fn()>,
        ) {
            let info = response.notification().request().content().userInfo();
            let key = NSString::from_str(OBJECT_KEY);
            let object_id = info
                .objectForKey(&key)
                .and_then(|value| value.downcast::<NSString>().ok());
            if let Some(object_id) = object_id {
                let _ = self.ivars().unbounded_send(object_id.to_string());
            }
            completion.call(());
        }
    }
);

thread_local! {
    /// The notification center holds its delegate weakly.
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
}

fn center() -> Retained<UNUserNotificationCenter> {
    UNUserNotificationCenter::currentNotificationCenter()
}

/// Become the notification center's delegate. Call before UIKit finishes
/// launching, so the tap that launched the app is delivered too. Yields
/// the object id of every tapped reminder.
pub fn install() -> mpsc::UnboundedReceiver<String> {
    let (tx, rx) = mpsc::unbounded();
    let delegate = Delegate::alloc().set_ivars(tx);
    let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
    center().setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    DELEGATE.with_borrow_mut(|slot| *slot = Some(delegate));
    rx
}

/// A completion handler called once, from any thread, as a future.
fn once<T: Send + 'static>() -> (impl Fn(T) + Send + Sync + 'static, oneshot::Receiver<T>) {
    let (tx, rx) = oneshot::channel();
    let tx = Mutex::new(Some(tx));
    (
        move |value| {
            if let Some(tx) = tx.lock().take() {
                let _ = tx.send(value);
            }
        },
        rx,
    )
}

async fn authorization_status(center: &UNUserNotificationCenter) -> UNAuthorizationStatus {
    let (done, rx) = once();
    let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        done(unsafe { settings.as_ref() }.authorizationStatus());
    });
    center.getNotificationSettingsWithCompletionHandler(&block);
    rx.await.unwrap_or(UNAuthorizationStatus::NotDetermined)
}

async fn request_authorization(center: &UNUserNotificationCenter) -> bool {
    let (done, rx) = once();
    let block = RcBlock::new(move |granted: Bool, _: *mut NSError| done(granted.as_bool()));
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Sound
            | UNAuthorizationOptions::Badge,
        &block,
    );
    rx.await.unwrap_or(false)
}

/// Identifiers of this app's pending reminders.
async fn pending(center: &UNUserNotificationCenter) -> Vec<String> {
    let (done, rx) = once();
    let block = RcBlock::new(move |requests: NonNull<NSArray<UNNotificationRequest>>| {
        let requests = unsafe { requests.as_ref() };
        done(
            requests
                .iter()
                .map(|request| request.identifier().to_string())
                .filter(|id| id.starts_with(PREFIX))
                .collect::<Vec<_>>(),
        );
    });
    center.getPendingNotificationRequestsWithCompletionHandler(&block);
    rx.await.unwrap_or_default()
}

async fn add(
    center: &UNUserNotificationCenter,
    occurrence: &Occurrence,
    now: i64,
) -> Result<(), String> {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(if occurrence.name.is_empty() {
        "Untitled"
    } else {
        &occurrence.name
    }));
    content.setBody(&NSString::from_str(if occurrence.agent_owned {
        "Due now · runs on the machine serving this space"
    } else {
        "Due now"
    }));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    let key = NSString::from_str(OBJECT_KEY);
    let value = NSString::from_str(&occurrence.object_id);
    let info: Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_slices(&[&*key], &[value.as_ref()]);
    // SAFETY: the key type is erased only; a dictionary of strings is a
    // property-list user info.
    unsafe { content.setUserInfo(&Retained::cast_unchecked::<NSDictionary>(info)) };
    let seconds = ((occurrence.at - now) as f64 / 1000.).max(1.);
    let trigger =
        UNTimeIntervalNotificationTrigger::triggerWithTimeInterval_repeats(seconds, false);
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&format!("{PREFIX}{}", occurrence.id())),
        &content,
        Some(&trigger),
    );
    let (done, rx) = once();
    let block = RcBlock::new(move |error: *mut NSError| {
        done(unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string()));
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&block));
    match rx.await {
        Ok(Some(error)) => Err(error),
        _ => Ok(()),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// Follow `backend`: reschedule after start and after every burst of commits.
pub fn follow(backend: ReplicaBackend, cx: &mut App) -> Task<()> {
    cx.spawn(async move |cx| {
        let center = center();
        let mut authorized = false;
        let mut events = backend.events();
        loop {
            reschedule(&backend, &center, &mut authorized, cx).await;
            loop {
                match events.next().await {
                    None => return,
                    Some(Ok(BackendEvent::Committed(_) | BackendEvent::Hello)) => break,
                    Some(_) => {}
                }
            }
            // A sync pass commits in bursts; one reschedule covers each.
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            while let Some(Some(_)) = events.next().now_or_never() {}
        }
    })
}

async fn reschedule(
    backend: &ReplicaBackend,
    center: &UNUserNotificationCenter,
    authorized: &mut bool,
    cx: &gpui_kit::AsyncApp,
) {
    if !backend.has_key() {
        // Logged out: nothing of the old identity may fire.
        center.removeAllPendingNotificationRequests();
        return;
    }
    let now = now_ms();
    let upcoming = {
        let backend = backend.clone();
        cx.background_executor()
            .spawn(async move { backend.upcoming_occurrences(now, LIMIT).await })
            .await
    };
    let upcoming = match upcoming {
        Ok(upcoming) => upcoming,
        Err(error) => {
            log::warn!("[reminders] error={error:#}");
            return;
        }
    };
    if upcoming.is_empty() && !*authorized {
        log::info!("[reminders] pending=0 (nothing upcoming)");
        return;
    }
    *authorized = match authorization_status(center).await {
        UNAuthorizationStatus::NotDetermined => request_authorization(center).await,
        UNAuthorizationStatus::Authorized
        | UNAuthorizationStatus::Provisional
        | UNAuthorizationStatus::Ephemeral => true,
        _ => false,
    };
    if !*authorized {
        log::info!("[reminders] pending=0 (not authorized)");
        return;
    }
    let wanted: HashSet<String> = upcoming
        .iter()
        .map(|occurrence| format!("{PREFIX}{}", occurrence.id()))
        .collect();
    let present = pending(center).await;
    let stale: Vec<_> = present
        .iter()
        .filter(|id| !wanted.contains(*id))
        .map(|id| NSString::from_str(id))
        .collect();
    if !stale.is_empty() {
        center.removePendingNotificationRequestsWithIdentifiers(&NSArray::from_retained_slice(
            &stale,
        ));
    }
    let present: HashSet<String> = present.into_iter().collect();
    for occurrence in &upcoming {
        if present.contains(&format!("{PREFIX}{}", occurrence.id())) {
            continue;
        }
        if let Err(error) = add(center, occurrence, now).await {
            log::warn!("[reminders] error={error}");
            return;
        }
    }
    let scheduled = pending(center).await;
    log::info!(
        "[reminders] pending={} {}",
        scheduled.len(),
        scheduled
            .iter()
            .filter_map(|id| id[PREFIX.len()..].split(':').next())
            .collect::<Vec<_>>()
            .join(",")
    );
}

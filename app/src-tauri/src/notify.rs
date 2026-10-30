//! OS notifications that know where they point.
//!
//! A reply from Claude is worth interrupting for even while Threshold is
//! the frontmost app, because the reviewer is reading the diff, not
//! watching the conversation. And clicking one has to land on the thread
//! it announced — a banner that only brings the window forward leaves the
//! reviewer to go looking for what it was about.
//!
//! The notification plugin cannot do either on macOS: it goes through
//! `NSUserNotificationCenter`, which suppresses banners for the frontmost
//! app and does not report which notification was clicked. So this talks
//! to `UNUserNotificationCenter` directly, and reports a click as a
//! `notification-open` event carrying the review and thread.
//!
//! `UNUserNotificationCenter` only exists for a bundled app — reaching for
//! it from `tauri dev`, where the binary has no bundle identifier, raises
//! an Objective-C exception. [`available`] is false there, and the UI
//! falls back to its own toasts.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

/// Where a click on a notification should take the reviewer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub review: Option<String>,
    /// Root seq of the thread, when the notification is about one.
    pub thread: Option<u64>,
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Target;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, AnyThread};
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
        UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use tauri::{AppHandle, Emitter, Manager};

    static APP: OnceLock<AppHandle> = OnceLock::new();
    static GRANTED: AtomicBool = AtomicBool::new(false);
    static NEXT: AtomicU64 = AtomicU64::new(1);
    /// Targets by request identifier. Held here rather than in the
    /// notification's `userInfo`, which would mean marshalling through
    /// `NSDictionary` for two fields. A notification clicked after a
    /// restart has no entry and only brings the window forward, which is
    /// the right outcome for a thread that may no longer be current.
    static TARGETS: OnceLock<Mutex<HashMap<String, Target>>> = OnceLock::new();

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "ThresholdNotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            /// Present even when frontmost: that is the case this module
            /// exists for.
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                handler: &block2::DynBlock<dyn Fn()>,
            ) {
                let id = response.notification().request().identifier().to_string();
                let target = TARGETS.get().and_then(|t| t.lock().ok()?.remove(&id));
                if let Some(app) = APP.get() {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.unminimize();
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                    if let Some(target) = target {
                        let _ = app.emit("notification-open", target);
                    }
                }
                handler.call(());
            }
        }
    );

    impl Delegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    fn bundled() -> bool {
        NSBundle::mainBundle().bundleIdentifier().is_some()
    }

    pub fn init(app: &AppHandle) {
        if !bundled() {
            return;
        }
        let _ = APP.set(app.clone());
        let _ = TARGETS.set(Mutex::new(HashMap::new()));
        let center = UNUserNotificationCenter::currentNotificationCenter();
        // The center holds its delegate weakly; this one lives as long as
        // the process.
        let delegate = Delegate::new();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        std::mem::forget(delegate);
        let done = RcBlock::new(|granted: Bool, _err: *mut NSError| {
            GRANTED.store(granted.as_bool(), Ordering::SeqCst);
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &done,
        );
    }

    pub fn available() -> bool {
        bundled() && GRANTED.load(Ordering::SeqCst)
    }

    pub fn send(title: &str, subtitle: Option<&str>, body: &str, group: Option<&str>, target: Target) -> bool {
        if !available() {
            return false;
        }
        let id = format!("threshold-{}", NEXT.fetch_add(1, Ordering::SeqCst));
        if let Some(t) = TARGETS.get() {
            if let Ok(mut t) = t.lock() {
                t.insert(id.clone(), target);
            }
        }
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        if let Some(s) = subtitle {
            content.setSubtitle(&NSString::from_str(s));
        }
        content.setBody(&NSString::from_str(body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        // Grouped by review in Notification Center, so a busy thread does
        // not bury the rest.
        if let Some(g) = group {
            content.setThreadIdentifier(&NSString::from_str(g));
        }
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&id), &content, None);
        UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, None);
        true
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Target;
    use tauri::AppHandle;
    pub fn init(_app: &AppHandle) {}
    pub fn available() -> bool {
        false
    }
    pub fn send(_: &str, _: Option<&str>, _: &str, _: Option<&str>, _: Target) -> bool {
        false
    }
}

pub fn init(app: &AppHandle) {
    imp::init(app)
}

/// Whether native notifications will be shown and their clicks reported.
#[tauri::command]
pub fn native_notifications() -> bool {
    imp::available()
}

/// Post a notification. Returns false when native notifications are not
/// available, so the caller can say it some other way.
#[tauri::command]
pub fn notify(title: String, subtitle: Option<String>, body: String, review: Option<String>, thread: Option<u64>) -> bool {
    let group = review.clone();
    imp::send(&title, subtitle.as_deref(), &body, group.as_deref(), Target { review, thread })
}

//! macOS-only pieces Tauri doesn't wrap: notifications you can answer (UserNotifications, with a category per card
//! carrying its options, and an inline reply for text questions) and the Services entries ("Add to Board",
//! "Open in Board"). Callbacks land in `crate::on_notification` and `crate::on_service` on the main thread.
use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSPasteboard, NSPasteboardTypeString, NSPasteboardTypeURL};
use objc2_foundation::{NSArray, NSError, NSObjectProtocol, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction, UNNotificationActionOptions,
    UNNotificationCategory, UNNotificationCategoryOptions, UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNTextInputNotificationAction,
    UNTextInputNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use crate::logic::Item;

/// What the user did with a notification.
pub enum Response {
    Open(String),
    Option(String, usize),
    Reply(String, String),
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this type has no Drop.
    // Not main-thread-only: UserNotifications may call it on its own queue; the callback is thread-safe.
    #[unsafe(super(NSObject))]
    #[name = "BoardNotificationDelegate"]
    struct NotificationDelegate;

    unsafe impl NSObjectProtocol for NotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        // Show banners even while Board is the front app (the menu bar is always "open").
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
            let card = response.notification().request().identifier().to_string();
            let action = response.actionIdentifier().to_string();
            let reply = response.downcast_ref::<UNTextInputNotificationResponse>().map(|r| r.userText().to_string());
            let result = if let Some(text) = reply {
                Some(Response::Reply(card, text))
            } else if let Some(index) = action.strip_prefix("opt:").and_then(|i| i.parse::<usize>().ok()) {
                Some(Response::Option(card, index))
            } else if action == unsafe { UNNotificationDefaultActionIdentifier }.to_string() || action == "open" {
                Some(Response::Open(card))
            } else {
                None
            };
            if let Some(r) = result {
                crate::on_notification(r);
            }
            handler.call(());
        }
    }
);

define_class!(
    // SAFETY: as above.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BoardServicesProvider"]
    struct ServicesProvider;

    unsafe impl NSObjectProtocol for ServicesProvider {}

    impl ServicesProvider {
        // Services menu → "Add to Board": the selected text starts a Quick add.
        #[unsafe(method(addToBoard:userData:error:))]
        fn add_to_board(&self, pasteboard: &NSPasteboard, _data: Option<&NSString>, _error: *mut *mut NSString) {
            let text = unsafe { pasteboard.stringForType(NSPasteboardTypeString) }.map(|s| s.to_string()).unwrap_or_default();
            crate::on_service(false, text);
        }

        // Services menu → "Open in Board": a selected board.zem.cool link opens in the app.
        #[unsafe(method(openInBoard:userData:error:))]
        fn open_in_board(&self, pasteboard: &NSPasteboard, _data: Option<&NSString>, _error: *mut *mut NSString) {
            let text = unsafe { pasteboard.stringForType(NSPasteboardTypeURL) }
                .or_else(|| unsafe { pasteboard.stringForType(NSPasteboardTypeString) })
                .map(|s| s.to_string())
                .unwrap_or_default();
            crate::on_service(true, text);
        }
    }
);

thread_local! {
    static KEEP: RefCell<Vec<Retained<NSObject>>> = const { RefCell::new(Vec::new()) };
}

fn center() -> Retained<UNUserNotificationCenter> {
    UNUserNotificationCenter::currentNotificationCenter()
}

/// Once, on the main thread at launch: the notification delegate and the Services provider.
pub fn install(mtm: MainThreadMarker) {
    let delegate: Retained<NotificationDelegate> = unsafe { msg_send![NotificationDelegate::alloc(), init] };
    center().setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let provider: Retained<ServicesProvider> = unsafe { msg_send![ServicesProvider::alloc(mtm), init] };
    let app = NSApplication::sharedApplication(mtm);
    unsafe { app.setServicesProvider(Some(&*provider as &AnyObject)) };
    objc2_app_kit::NSUpdateDynamicServices();
    KEEP.with(|k| {
        k.borrow_mut().push(Retained::into_super(delegate));
        k.borrow_mut().push(Retained::into_super(provider));
    });
}

/// Asks once for permission (macOS remembers the answer).
pub fn request_permission() {
    let block = block2::RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
    center().requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound | UNAuthorizationOptions::Badge,
        &block,
    );
}

fn category_id(item: &Item) -> String {
    if item.is_choice() {
        format!("choice:{}", item.id)
    } else if item.is_text() {
        "text".into()
    } else {
        "open".into()
    }
}

/// One category per waiting choice card (its options as actions, recommended first), plus "text" (inline reply) and
/// "open". Replaces the whole set, so answered cards' categories go with them.
pub fn set_categories(items: &[Item]) {
    let ns = |s: &str| NSString::from_str(s);
    let mut categories: Vec<Retained<UNNotificationCategory>> = Vec::new();
    let open = UNNotificationAction::actionWithIdentifier_title_options(&ns("open"), &ns("Open in Board"), UNNotificationActionOptions::Foreground);
    for item in items.iter().filter(|i| i.is_choice()) {
        let mut actions: Vec<Retained<UNNotificationAction>> = item
            .ordered_options()
            .iter()
            .take(6)
            .enumerate()
            .map(|(i, option)| {
                UNNotificationAction::actionWithIdentifier_title_options(
                    &ns(&format!("opt:{i}")),
                    &ns(option),
                    UNNotificationActionOptions::AuthenticationRequired,
                )
            })
            .collect();
        actions.push(open.clone());
        categories.push(UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
            &ns(&category_id(item)),
            &NSArray::from_retained_slice(&actions),
            &NSArray::new(),
            UNNotificationCategoryOptions::empty(),
        ));
    }
    let reply = UNTextInputNotificationAction::actionWithIdentifier_title_options_textInputButtonTitle_textInputPlaceholder(
        &ns("reply"),
        &ns("Reply"),
        UNNotificationActionOptions::AuthenticationRequired,
        &ns("Send"),
        &ns("Your answer"),
    );
    categories.push(UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &ns("text"),
        &NSArray::from_retained_slice(&[Retained::into_super(reply), open.clone()]),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    ));
    categories.push(UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &ns("open"),
        &NSArray::from_retained_slice(&[open]),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    ));
    center().setNotificationCategories(&NSSet::from_retained_slice(&categories));
}

/// A Needs-me card as a notification: the title, who asked and the project, the full question, and its answers.
/// The request id is the card id, so a card that's answered anywhere can be taken back.
pub fn post(item: &Item) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&item.title));
    let byline = item.byline();
    if !byline.is_empty() {
        content.setSubtitle(&NSString::from_str(&byline));
    }
    let body = item.text.clone().filter(|t| t.trim() != item.title.trim()).unwrap_or_default();
    content.setBody(&NSString::from_str(if body.is_empty() && item.is_choice() { "Choose an answer" } else { &body }));
    content.setCategoryIdentifier(&NSString::from_str(&category_id(item)));
    content.setThreadIdentifier(&NSString::from_str("needs"));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&item.id), &content, None);
    center().addNotificationRequest_withCompletionHandler(&request, None);
}

/// A plain one-off note (connected, sent, couldn't send). Its id never matches a card.
pub fn note(title: &str, body: &str) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&format!("note:{title}")), &content, None);
    center().addNotificationRequest_withCompletionHandler(&request, None);
}

/// Takes back notifications for cards no longer waiting (answered here, on the Board or on another device).
pub fn remove(ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let ids: Vec<Retained<NSString>> = ids.iter().map(|id| NSString::from_str(id)).collect();
    center().removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&ids));
}

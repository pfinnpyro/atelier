//! The parts of being a Mac app that winit leaves out: documents opened
//! from Finder (or Open With, or `open -a`), Quit from the Dock, and the
//! menu bar.
//!
//! macOS does not put a file Finder opens on the command line. It sends an
//! "open documents" Apple Event, and winit 0.30 has no handler for it, so
//! AppKit answered that Atelier cannot open PDFs. The handler here takes the
//! event, queues the paths and wakes the viewer, which opens them in tabs.

// AppKit is only reachable through Objective-C messages. The unsafe code is
// kept to this module: declaring the one handler class and sending the few
// messages that install it and read its events.
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use eframe::egui;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSString,
};

/// What arrived from the system or the menu bar since the last frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    Open(PathBuf),
    Quit,
    Menu(String),
}

static INCOMING: Mutex<Vec<Incoming>> = Mutex::new(Vec::new());
static WAKE: OnceLock<egui::Context> = OnceLock::new();

pub(crate) fn push(item: Incoming) {
    if let Ok(mut queue) = INCOMING.lock() {
        queue.push(item);
    }
    if let Some(ctx) = WAKE.get() {
        ctx.request_repaint();
    }
}

/// Everything queued so far. Files opened while Atelier was still starting
/// wait here until the first frame.
pub fn take() -> Vec<Incoming> {
    INCOMING
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// Lets later events wake the viewer when it is idle.
pub fn set_waker(ctx: &egui::Context) {
    let _ = WAKE.set(ctx.clone());
}

/// Four-character Apple Event codes.
const fn code(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}
const CORE_EVENT_CLASS: u32 = code(b"aevt");
const OPEN_DOCUMENTS: u32 = code(b"odoc");
const QUIT_APPLICATION: u32 = code(b"quit");
const DIRECT_OBJECT: u32 = code(b"----");

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AtelierAppleEvents"]
    struct AppleEvents;

    impl AppleEvents {
        /// AppKit installs its own handlers for the core events while it
        /// finishes launching, so ours go in at this notification, which is
        /// after that and before the launch's own Open Documents event.
        #[unsafe(method(willFinishLaunching:))]
        fn will_finish_launching(&self, _note: &NSNotification) {
            let manager = NSAppleEventManager::sharedAppleEventManager();
            unsafe {
                let _: () = msg_send![&manager,
                    setEventHandler: self,
                    andSelector: sel!(openDocuments:withReplyEvent:),
                    forEventClass: CORE_EVENT_CLASS,
                    andEventID: OPEN_DOCUMENTS];
                let _: () = msg_send![&manager,
                    setEventHandler: self,
                    andSelector: sel!(quit:withReplyEvent:),
                    forEventClass: CORE_EVENT_CLASS,
                    andEventID: QUIT_APPLICATION];
            }
        }

        #[unsafe(method(openDocuments:withReplyEvent:))]
        fn open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            let files: Option<Retained<NSAppleEventDescriptor>> =
                unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
            let Some(files) = files else { return };
            let count = files.numberOfItems();
            let items: Vec<Retained<NSAppleEventDescriptor>> = if count < 1 {
                // A single file can come as the descriptor itself.
                vec![files]
            } else {
                // Apple Event lists count from one.
                (1..=count).filter_map(|i| files.descriptorAtIndex(i)).collect()
            };
            for item in items {
                if let Some(path) = item.fileURLValue().and_then(|url| url.path()) {
                    push(Incoming::Open(PathBuf::from(path.to_string())));
                }
            }
        }

        /// Quit from the Dock, or at log out: close the window the way its
        /// close button does, so unsaved changes are asked about first.
        #[unsafe(method(quit:withReplyEvent:))]
        fn quit(&self, _event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            push(Incoming::Quit);
        }
    }
);

/// Starts listening for the open and quit events. Called from `main` on
/// the main thread before the event loop starts.
pub fn listen_for_apple_events() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let this = AppleEvents::alloc(mtm).set_ivars(());
    let handler: Retained<AppleEvents> = unsafe { msg_send![super(this), init] };
    let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &handler,
            sel!(willFinishLaunching:),
            Some(&name),
            None,
        );
    }
    // Neither the notification centre nor the event manager keeps the
    // handler alive, and it is needed for as long as the app runs.
    std::mem::forget(handler);
}

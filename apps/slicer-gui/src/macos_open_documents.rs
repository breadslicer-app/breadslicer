/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Receives the open-documents Apple Event, which is how macOS hands an
//! application the files the user opened from the Finder.
//!
//! macOS does not pass those files in `argv` the way Linux and Windows do, so
//! without this the application starts and silently ignores them.

use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, define_class, msg_send, sel};
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSObject,
    NSObjectProtocol, NSString,
};

use crate::mesh_loader::is_supported_mesh_extension;

/// Packs a 4-character code into the `u32` the Apple Event API takes.
/// `objc2-core-services` binds none of the codes used here, so they are spelled
/// out as constants below.
const fn four_char_code(code: &[u8; 4]) -> u32 {
    ((code[0] as u32) << 24) | ((code[1] as u32) << 16) | ((code[2] as u32) << 8) | (code[3] as u32)
}

const CORE_EVENT_CLASS: u32 = four_char_code(b"aevt");
const OPEN_DOCUMENTS_EVENT: u32 = four_char_code(b"odoc");
const DIRECT_OBJECT_KEY: u32 = four_char_code(b"----");

const WILL_FINISH_LAUNCHING: &str = "NSApplicationWillFinishLaunchingNotification";

/// Where [`install`] routes the paths it receives. Held per thread because the
/// handler and the Slint event loop share the main thread, and the application
/// state the sink reaches for is not `Send`.
type PathSink = Box<dyn Fn(Vec<PathBuf>)>;

thread_local! {
    static SINK: RefCell<Option<PathSink>> = const { RefCell::new(None) };
    static HANDLER: Retained<OpenDocumentsHandler> = OpenDocumentsHandler::new();
}

// FFI: the Apple Event API dispatches to an Objective-C selector, so the
// handler has to be an Objective-C class.
#[allow(unsafe_code)]
mod handler {
    use super::*;

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "SCSlicerOpenDocumentsHandler"]
        #[thread_kind = AllocAnyThread]
        pub(super) struct OpenDocumentsHandler;

        unsafe impl NSObjectProtocol for OpenDocumentsHandler {}

        impl OpenDocumentsHandler {
            #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
            fn handle_open_documents(
                &self,
                event: &NSAppleEventDescriptor,
                _reply: &NSAppleEventDescriptor,
            ) {
                deliver(supported_paths_in(event));
            }
        }
    );

    impl OpenDocumentsHandler {
        pub(super) fn new() -> Retained<Self> {
            unsafe { msg_send![Self::alloc(), init] }
        }
    }
}

use handler::OpenDocumentsHandler;

/// Starts routing documents opened from the Finder to `sink`. Call before the
/// event loop runs: the document that launched the application arrives during
/// startup, and a later call misses it.
pub(crate) fn install(sink: impl Fn(Vec<PathBuf>) + 'static) {
    SINK.with_borrow_mut(|slot| *slot = Some(Box::new(sink)));
    // Registration waits for the notification rather than happening here.
    // AppKit installs its own open-documents handler before it posts
    // `NSApplicationWillFinishLaunchingNotification`, and dispatches the launch
    // document between that notification and
    // `NSApplicationDidFinishLaunchingNotification`. Registering earlier is
    // undone by AppKit; registering at `DidFinishLaunching` is too late.
    observe_will_finish_launching();
}

// FFI: adding a notification observer takes an Objective-C block.
#[allow(unsafe_code)]
fn observe_will_finish_launching() {
    let block = RcBlock::new(|_notification: NonNull<NSNotification>| register_handler());
    let center = NSNotificationCenter::defaultCenter();
    // The observer is never removed: it is wanted for as long as the process
    // runs, and `addObserverForName` returns a token that has to outlive it
    // either way.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(&NSString::from_str(WILL_FINISH_LAUNCHING)),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(token);
    std::mem::forget(block);
}

// FFI: no safe binding exists for installing an Apple Event handler.
#[allow(unsafe_code)]
fn register_handler() {
    let manager = NSAppleEventManager::sharedAppleEventManager();
    HANDLER.with(|handler| {
        let object: &AnyObject = handler.as_ref();
        unsafe {
            manager.setEventHandler_andSelector_forEventClass_andEventID(
                object,
                sel!(handleOpenDocuments:withReplyEvent:),
                CORE_EVENT_CLASS,
                OPEN_DOCUMENTS_EVENT,
            );
        }
    });
}

/// Picks out the paths the mesh loader can open. The result can be shorter than
/// what the user opened, or empty, and the caller gets no sign of what was
/// dropped.
fn supported_paths_in(event: &NSAppleEventDescriptor) -> Vec<PathBuf> {
    // The files travel under the event's primary parameter, as a list even when
    // the user opened only one.
    let Some(list) = event.paramDescriptorForKeyword(DIRECT_OBJECT_KEY) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    // Apple Event lists count from 1.
    for index in 1..=list.numberOfItems() {
        let Some(path) = list
            .descriptorAtIndex(index)
            .and_then(|item| item.fileURLValue())
            .and_then(|url| url.path())
            .map(|path| PathBuf::from(path.to_string()))
        else {
            continue;
        };
        if !is_supported_mesh_extension(&path) {
            eprintln!(
                "Ignoring file with unsupported extension: {}",
                path.display()
            );
            continue;
        }
        paths.push(path);
    }
    paths
}

/// Hands `paths` to the sink from the event loop rather than from the handler.
/// The launch event arrives while AppKit is still starting the application,
/// which is not a point at which to open windows.
fn deliver(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let posted = slint::invoke_from_event_loop(move || {
        SINK.with_borrow(|slot| {
            if let Some(sink) = slot {
                sink(paths);
            }
        });
    });
    if let Err(err) = posted {
        eprintln!("Could not open the files the Finder sent: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::four_char_code;

    #[test]
    fn four_char_codes_match_the_values_the_apple_event_api_defines() {
        // Spelled out here because `objc2-core-services` does not bind them; a
        // wrong code registers a handler for an event that never arrives, which
        // looks exactly like the feature not working.
        assert_eq!(four_char_code(b"aevt"), 0x6165_7674);
        assert_eq!(four_char_code(b"odoc"), 0x6f64_6f63);
        assert_eq!(four_char_code(b"----"), 0x2d2d_2d2d);
    }
}

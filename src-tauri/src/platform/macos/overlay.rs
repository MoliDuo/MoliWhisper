//! Shows the overlay without activating the app or taking focus.

use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
use tauri::{Runtime, WebviewWindow};

/// NSPopUpMenuWindowLevel: above normal and floating windows and the Dock.
const LEVEL: isize = 101;

fn with_ns_window<R: Runtime>(
    window: &WebviewWindow<R>,
    f: impl FnOnce(&NSWindow) + Send + 'static,
) {
    let Ok(ptr) = window.ns_window() else {
        return;
    };
    let ptr = ptr as usize;
    let _ = window.run_on_main_thread(move || {
        // SAFETY: the pointer is the live NSWindow of the webview window, used on the main thread.
        let ns = unsafe { &*(ptr as *const NSWindow) };
        f(ns);
    });
}

pub fn prepare_overlay<R: Runtime>(window: &WebviewWindow<R>) {
    with_ns_window(window, |ns| {
        ns.setLevel(LEVEL);
        ns.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        ns.setIgnoresMouseEvents(true);
        ns.setHidesOnDeactivate(false);
    });
}

/// Orders the window in without making it key, so focus stays put.
pub fn show_overlay<R: Runtime>(window: &WebviewWindow<R>) {
    with_ns_window(window, |ns| ns.orderFrontRegardless());
}

pub fn hide_overlay<R: Runtime>(window: &WebviewWindow<R>) {
    with_ns_window(window, |ns| ns.orderOut(None));
}

//! Global key hook: an active CGEventTap on its own run-loop thread.
//!
//! The tap needs the Accessibility permission. Without it the thread polls
//! `AXIsProcessTrusted` and creates the tap as soon as the user grants it,
//! so no restart is needed.

// The CoreGraphics constants keep their C names.
#![allow(non_upper_case_globals)]

use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::time::Duration;

use moli_core::hotkey::{Key, KeyEvent, ModKey, Mods};

use super::ffi::*;
use super::paste::INJECTED;
use crate::hotkey::{Hook, HookStatus};

const PERMISSION_POLL: Duration = Duration::from_secs(1);
const RETRY: Duration = Duration::from_secs(2);

struct Tap {
    hook: Arc<Hook>,
    port: AtomicPtr<c_void>,
}

pub fn start_key_hook(hook: Arc<Hook>) {
    let spawned = std::thread::Builder::new()
        .name("key-hook".into())
        .spawn(move || run(hook));
    if let Err(e) = spawned {
        log::error!("could not start the key hook thread: {e}");
    }
}

fn run(hook: Arc<Hook>) {
    while !unsafe { AXIsProcessTrusted() } {
        hook.set_status(HookStatus::NeedsPermission);
        std::thread::sleep(PERMISSION_POLL);
    }
    // Lives as long as the process; the callback holds a raw pointer to it.
    let tap: &'static Tap = Box::leak(Box::new(Tap {
        hook,
        port: AtomicPtr::new(ptr::null_mut()),
    }));
    let mask = (1u64 << kCGEventKeyDown) | (1 << kCGEventKeyUp) | (1 << kCGEventFlagsChanged);
    let port = loop {
        let port = unsafe {
            CGEventTapCreate(
                kCGSessionEventTap,
                kCGHeadInsertEventTap,
                kCGEventTapOptionDefault,
                mask,
                callback,
                tap as *const Tap as *mut c_void,
            )
        };
        if !port.is_null() {
            break port;
        }
        tap.hook
            .set_status(HookStatus::Failed("无法创建键盘监听".into()));
        std::thread::sleep(RETRY);
    };
    tap.port.store(port, Ordering::Release);
    unsafe {
        let source = CFMachPortCreateRunLoopSource(ptr::null(), port, 0);
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
        CGEventTapEnable(port, true);
    }
    tap.hook.set_status(HookStatus::Running);
    unsafe { CFRunLoopRun() };
    log::error!("key hook run loop exited");
}

unsafe extern "C" fn callback(
    _proxy: CGEventTapProxy,
    ty: CGEventType,
    event: CGEventRef,
    user: *mut c_void,
) -> CGEventRef {
    // SAFETY: `user` is the leaked `Tap` passed to CGEventTapCreate.
    let tap = unsafe { &*(user as *const Tap) };
    if ty == kCGEventTapDisabledByTimeout || ty == kCGEventTapDisabledByUserInput {
        log::warn!("event tap was disabled ({ty:#x}); re-enabling");
        unsafe { CGEventTapEnable(tap.port.load(Ordering::Acquire), true) };
        return event;
    }
    if unsafe { CGEventGetIntegerValueField(event, kCGEventSourceUserData) } == INJECTED {
        return event; // our own paste shortcut
    }
    let Some(ev) = (unsafe { key_event(ty, event) }) else {
        return event;
    };
    // A panic must not unwind into Core Graphics.
    match std::panic::catch_unwind(AssertUnwindSafe(|| tap.hook.handle(ev))) {
        Ok(true) => ptr::null_mut(),
        Ok(false) => event,
        Err(_) => {
            log::error!("key hook handler panicked");
            event
        }
    }
}

unsafe fn key_event(ty: CGEventType, event: CGEventRef) -> Option<KeyEvent> {
    let code = unsafe { CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode) } as u32;
    let flags = unsafe { CGEventGetFlags(event) };
    let mods = Mods {
        ctrl: flags & kCGEventFlagMaskControl != 0,
        shift: flags & kCGEventFlagMaskShift != 0,
        alt: flags & kCGEventFlagMaskAlternate != 0,
        meta: flags & kCGEventFlagMaskCommand != 0,
    };
    match ty {
        kCGEventKeyDown | kCGEventKeyUp => Some(KeyEvent {
            key: Key::Code(code),
            down: ty == kCGEventKeyDown,
            mods,
            repeat: unsafe { CGEventGetIntegerValueField(event, kCGKeyboardEventAutorepeat) } != 0,
        }),
        kCGEventFlagsChanged => {
            let (key, bit) = match code {
                54 => (ModKey::RightMeta, NX_DEVICERCMDKEYMASK),
                55 => (ModKey::LeftMeta, NX_DEVICELCMDKEYMASK),
                56 => (ModKey::LeftShift, NX_DEVICELSHIFTKEYMASK),
                60 => (ModKey::RightShift, NX_DEVICERSHIFTKEYMASK),
                58 => (ModKey::LeftAlt, NX_DEVICELALTKEYMASK),
                61 => (ModKey::RightAlt, NX_DEVICERALTKEYMASK),
                59 => (ModKey::LeftCtrl, NX_DEVICELCTLKEYMASK),
                62 => (ModKey::RightCtrl, NX_DEVICERCTLKEYMASK),
                63 => (ModKey::Fn, kCGEventFlagMaskSecondaryFn),
                _ => return None, // Caps Lock and friends
            };
            Some(KeyEvent {
                key: Key::Mod(key),
                down: flags & bit != 0,
                mods,
                repeat: false,
            })
        }
        _ => None,
    }
}

//! Non-blocking window zoom on macOS.
//!
//! `-[NSWindow zoom:]` animates with a blocking animation, so WKWebView's
//! frames queue up and the page freezes until the zoom ends. We answer
//! `windowShouldZoom:toFrame:` with `NO` and run the same frame change through
//! the window's animator on the ordinary run loop instead. The method is added
//! to tao's window delegate at runtime, since tao does not implement it.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{ffi, msg_send, sel};
use objc2_app_kit::{NSAnimationContext, NSEvent, NSWindow};
use objc2_foundation::NSRect;

/// Pre-zoom frame to restore on un-zoom, per window (the hook is added to the
/// delegate class, so every tao window shares it). AppKit's own record is
/// taken during the zoom we veto, so it cannot be used.
static RESTORE: Mutex<Option<HashMap<usize, NSRect>>> = Mutex::new(None);

/// Route the zoom of `ns_window` (`*mut NSWindow`) through the animator.
/// Main thread only.
pub fn install(ns_window: *mut c_void) {
    if ns_window.is_null() {
        tracing::warn!("main window has no NSWindow; zoom stays native");
        return;
    }
    let window: &NSWindow = unsafe { &*ns_window.cast::<NSWindow>() };
    let Some(delegate) = window.delegate() else {
        tracing::warn!("main window has no delegate; zoom stays native");
        return;
    };
    let class: *const AnyClass = unsafe { msg_send![&*delegate, class] };
    // BOOL (self, _cmd, NSWindow *, NSRect)
    let types = c"B@:@{CGRect={CGPoint=dd}{CGSize=dd}}";
    let added = unsafe {
        ffi::class_addMethod(
            class.cast_mut(),
            sel!(windowShouldZoom:toFrame:),
            std::mem::transmute::<ShouldZoom, Imp>(should_zoom),
            types.as_ptr(),
        )
    };
    if !added.as_bool() {
        tracing::warn!(
            "window delegate already handles windowShouldZoom:toFrame:; zoom stays native"
        );
    }
}

type ShouldZoom = extern "C-unwind" fn(&AnyObject, Sel, &NSWindow, NSRect) -> Bool;

extern "C-unwind" fn should_zoom(
    _this: &AnyObject,
    _cmd: Sel,
    window: &NSWindow,
    proposed: NSRect,
) -> Bool {
    let key = window as *const NSWindow as usize;

    // Since macOS 15, dragging a zoomed window by its title bar un-zooms it
    // first and asks us the same question. Moving the frame ourselves mid-drag
    // ends the drag, so let AppKit un-zoom natively; its proposal is the right
    // pre-zoom frame. Every user-requested zoom arrives with the button up.
    if NSEvent::pressedMouseButtons() & 1 != 0 {
        let mut guard = RESTORE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(restore) = guard.as_mut() {
            restore.remove(&key);
        }
        return Bool::YES;
    }

    let target = {
        let mut guard = RESTORE.lock().unwrap_or_else(|e| e.into_inner());
        let restore = guard.get_or_insert_with(HashMap::new);
        if window.isZoomed() {
            // Launched already zoomed: nothing saved, AppKit's proposal is best.
            restore.remove(&key).unwrap_or(proposed)
        } else {
            restore.insert(key, window.frame());
            proposed
        }
    };

    unsafe {
        NSAnimationContext::beginGrouping();
        NSAnimationContext::currentContext().setDuration(window.animationResizeTime(target));
        let animator: Retained<NSWindow> = msg_send![window, animator];
        animator.setFrame_display(target, true);
        NSAnimationContext::endGrouping();
    }
    Bool::NO
}

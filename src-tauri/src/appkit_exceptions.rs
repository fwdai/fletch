//! AppKit exceptions raised during event dispatch are logged, not fatal.
//!
//! tao overrides `-[NSApplication sendEvent:]` with an `extern "C"` function,
//! which must not unwind. An Objective-C exception AppKit raises while
//! dispatching an event unwinds into that frame and Rust aborts the process
//! ("panic in a function that cannot unwind"), where AppKit's own run loop
//! would have logged it and gone on. macOS 27's Writing Tools affordance does
//! this: on the app's reactivation it tears down its remote view, then fails an
//! assertion (`NSCampoLightweightUIController.m`) on the next `mouseEntered:`.
//!
//! tao hands every event it does not handle itself to `[super sendEvent:]`, so
//! we swap NSApplication's own implementation — the one that super call
//! reaches — for one that runs the original inside `@try` and logs what it
//! catches. The exception stops below tao's frame. tao's one other path, a
//! Cmd+keyUp sent straight to the key window, is left as is.

use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{exception, msg_send, sel};
use objc2_foundation::NSString;

/// `void (self, _cmd, NSEvent *)`
type SendEvent = unsafe extern "C-unwind" fn(&AnyObject, Sel, &AnyObject);

/// AppKit's `-[NSApplication sendEvent:]`, which [`send_event`] stands in for.
static ORIGINAL: OnceLock<SendEvent> = OnceLock::new();

/// Swap in [`send_event`]. Idempotent; main thread only, before the event loop
/// runs (events are dispatched on the main thread, so none can arrive between
/// recording the original and swapping it out).
pub fn install() {
    let Some(class) = AnyClass::get(c"NSApplication") else {
        tracing::warn!("no NSApplication class; AppKit exceptions stay fatal");
        return;
    };
    let Some(method) = class.instance_method(sel!(sendEvent:)) else {
        tracing::warn!("NSApplication has no sendEvent:; AppKit exceptions stay fatal");
        return;
    };
    if ORIGINAL.get().is_some() {
        return;
    }
    let original = method.implementation();
    let _ = ORIGINAL.set(unsafe { std::mem::transmute::<Imp, SendEvent>(original) });
    unsafe { method.set_implementation(std::mem::transmute::<SendEvent, Imp>(send_event)) };
}

extern "C-unwind" fn send_event(this: &AnyObject, cmd: Sel, event: &AnyObject) {
    let original = *ORIGINAL.get().expect("recorded before the swap");
    guarded(|| unsafe { original(this, cmd, event) });
}

/// Run `f`, catching an Objective-C exception it raises: logged, and `None`
/// returned in place of the value. Rust panics pass through untouched.
fn guarded<R>(f: impl FnOnce() -> R) -> Option<R> {
    match exception::catch(AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(exception) => {
            // The name is one of a fixed vocabulary (`NSInternalInconsistencyException`,
            // `NSRangeException`, …); the reason is free-form, so it stays in
            // the log file (`sentry_scrub` drops `reason`).
            let exception: Option<&AnyObject> = exception.as_deref().map(AsRef::as_ref);
            let name = exception.and_then(exception_name);
            let reason = exception.map(|e| format!("{e:?}"));
            tracing::error!(
                exception_name = name.as_deref().unwrap_or("nil"),
                reason = reason.as_deref().unwrap_or("nil"),
                "AppKit raised an exception during event dispatch; dropped it"
            );
            None
        }
    }
}

fn exception_name(exception: &AnyObject) -> Option<String> {
    let name: Option<Retained<NSString>> = unsafe { msg_send![exception, name] };
    name.map(|name| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// An out-of-range `characterAtIndex:` raises an NSException (which one
    /// is Foundation's business); it is caught and named instead of unwinding.
    #[test]
    fn an_objc_exception_is_caught() {
        let empty = NSString::new();
        let out = guarded(|| {
            let _: u16 = unsafe { msg_send![&empty, characterAtIndex: 5usize] };
        });
        assert!(out.is_none());

        let caught = exception::catch(AssertUnwindSafe(|| {
            let _: u16 = unsafe { msg_send![&empty, characterAtIndex: 5usize] };
        }))
        .unwrap_err()
        .expect("an NSException");
        let name = exception_name(caught.as_ref()).expect("a named exception");
        assert!(
            name.starts_with("NS") && name.ends_with("Exception"),
            "{name}"
        );
    }

    /// `[super sendEvent:]` from tao's subclass reaches our wrapper, and a
    /// second install does not wrap the wrapper.
    #[test]
    fn install_swaps_in_the_wrapper_once() {
        install();
        install();
        let method = AnyClass::get(c"NSApplication")
            .and_then(|class| class.instance_method(sel!(sendEvent:)))
            .unwrap();
        let wrapper = unsafe { std::mem::transmute::<SendEvent, Imp>(send_event) };
        assert_eq!(method.implementation() as usize, wrapper as usize);
        assert_ne!(*ORIGINAL.get().unwrap() as usize, wrapper as usize);
    }

    #[test]
    fn a_normal_call_returns_its_value() {
        assert_eq!(guarded(|| 7), Some(7));
    }
}

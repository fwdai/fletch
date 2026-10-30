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

use objc2::exception::{self, Exception};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::sel;
use objc2_foundation::NSException;

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
    if let Err(exception) =
        exception::catch(AssertUnwindSafe(|| unsafe { original(this, cmd, event) }))
    {
        log_dropped(exception);
    }
}

/// The name is a constant the throwing framework defines
/// (`NSInternalInconsistencyException`, …) and reaches Sentry; the free-form
/// reason stays in the log file (`sentry_scrub` drops `reason`).
fn log_dropped(exception: Option<Retained<Exception>>) {
    match exception.map(NSException::from_exception) {
        Some(Ok(e)) => tracing::error!(
            exception_name = %e.name(),
            reason = %e.reason().map(|r| r.to_string()).unwrap_or_default(),
            "AppKit raised an exception during event dispatch; dropped it"
        ),
        _ => tracing::error!("AppKit raised a non-NSException during event dispatch; dropped it"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::msg_send;
    use objc2_foundation::NSString;

    /// An out-of-range `characterAtIndex:` raises an NSException (which one
    /// is Foundation's business): caught, read, and logged without unwinding.
    #[test]
    fn an_nsexception_is_caught_and_read() {
        let empty = NSString::new();
        let caught = exception::catch(AssertUnwindSafe(|| {
            let _: u16 = unsafe { msg_send![&empty, characterAtIndex: 5usize] };
        }))
        .unwrap_err();
        let e = NSException::from_exception(caught.clone().expect("an exception"))
            .expect("an NSException");
        let name = e.name().to_string();
        assert!(
            name.starts_with("NS") && name.ends_with("Exception"),
            "{name}"
        );
        assert!(e.reason().is_some());
        log_dropped(caught);
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
}

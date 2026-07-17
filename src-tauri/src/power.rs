//! Idle-sleep prevention + a single "how much work is live" signal.
//!
//! # Why
//! Fletch fleets run for tens of minutes with no keyboard/mouse input. On a
//! laptop the system idle-sleeps and freezes every agent child mid-turn. We
//! therefore hold a macOS power assertion whenever *any* agent is `Running` or
//! *any* workflow run is active (pending/running), and release it once
//! everything goes idle.
//!
//! # Mechanism (option (a): IOKit)
//! We take an `IOPMAssertionCreateWithName` assertion of type
//! `PreventUserIdleSystemSleep` and drop it with `IOPMAssertionRelease`. This is
//! preferred over spawning `caffeinate -i` because it needs no child process to
//! babysit (one more thing to kill on quit), reports a user-visible name in
//! `pmset -g assertions` / Activity Monitor, and is pure in-process FFI — no new
//! crate, just a tiny `extern "C"` block linked against the `IOKit` /
//! `CoreFoundation` frameworks (mirrors how `sandbox/docker/auth.rs` gates
//! macOS-only native calls behind `cfg`). `PreventUserIdleSystemSleep` lets the
//! *display* sleep — we only keep the *system* awake — so a closed-lid clamshell
//! still suspends (expected; that's a hard power event, not idle).
//!
//! # No polling
//! The monitor is fed by transitions only: agent status comes off the
//! supervisor's existing `status_tx` broadcast (subscribed once at setup); run
//! activity is toggled where a run's drive task is registered/removed in the
//! scheduler (`spawn_drive_task`). A 30s debounce on release avoids flapping the
//! assertion between back-to-back turns.
//!
//! # Non-macOS
//! The assertion calls compile to no-ops; the activity bookkeeping and the
//! `on_change` callback (which drives the menu-bar status line) still run, so
//! the tray text is correct on every platform.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;

/// Keep the assertion this long after the last activity ends, so quick
/// idle→busy→idle flaps between turns don't thrash it.
const RELEASE_DEBOUNCE: Duration = Duration::from_secs(30);

/// User-visible assertion name (shows in `pmset -g assertions`).
#[cfg(target_os = "macos")]
const ASSERTION_NAME: &str = "Fletch: agents working";

type ChangeCb = Arc<dyn Fn(usize, usize) + Send + Sync>;

/// Process-wide activity monitor. Mirrors the in-process singleton idiom used
/// elsewhere (e.g. `sandbox::set_selected_engine_kind`) so code deep in the
/// spawn/scheduler paths — which has no `AppHandle` — can report activity
/// without threading state through.
pub struct ActivityMonitor {
    inner: Mutex<Inner>,
    /// Real side effects (IOKit assertion, tray callback) only fire once the
    /// app arms the monitor at setup. Keeps `cargo test` — which exercises the
    /// scheduler's run registry heavily — from touching real power assertions.
    armed: AtomicBool,
}

struct Inner {
    /// Ids of agents currently `Running`.
    agents: HashSet<String>,
    /// Ids of workflow runs currently being driven (pending/running).
    runs: HashSet<String>,
    /// Bumped on every recompute; a scheduled debounced release only fires if
    /// its captured generation still matches (i.e. no activity since).
    release_gen: u64,
    /// Live IOKit assertion id, if held.
    #[cfg(target_os = "macos")]
    assertion: Option<u32>,
    /// Invoked with (running_agents, active_runs) after every change, so the
    /// menu-bar status line can re-render. Set once at setup.
    on_change: Option<ChangeCb>,
}

impl ActivityMonitor {
    pub fn global() -> &'static ActivityMonitor {
        static MONITOR: OnceLock<ActivityMonitor> = OnceLock::new();
        MONITOR.get_or_init(|| ActivityMonitor {
            inner: Mutex::new(Inner {
                agents: HashSet::new(),
                runs: HashSet::new(),
                release_gen: 0,
                #[cfg(target_os = "macos")]
                assertion: None,
                on_change: None,
            }),
            armed: AtomicBool::new(false),
        })
    }

    /// Enable real side effects and register the status-line callback. Called
    /// once from `setup` in the real app.
    pub fn arm(&self, on_change: ChangeCb) {
        {
            let mut inner = self.inner.lock();
            inner.on_change = Some(on_change);
        }
        self.armed.store(true, Ordering::SeqCst);
        // Render the initial (idle) status line immediately.
        self.apply();
    }

    /// Mark an agent running (or not). No-op until armed.
    pub fn set_agent_running(&self, agent_id: &str, running: bool) {
        if !self.armed.load(Ordering::SeqCst) {
            return;
        }
        {
            let mut inner = self.inner.lock();
            let changed = if running {
                inner.agents.insert(agent_id.to_string())
            } else {
                inner.agents.remove(agent_id)
            };
            if !changed {
                return;
            }
        }
        self.apply();
    }

    /// Replace the full set of running agents (used to resync after a lagged
    /// broadcast receiver). No-op until armed.
    pub fn resync_agents(&self, running: HashSet<String>) {
        if !self.armed.load(Ordering::SeqCst) {
            return;
        }
        {
            let mut inner = self.inner.lock();
            if inner.agents == running {
                return;
            }
            inner.agents = running;
        }
        self.apply();
    }

    /// Mark a workflow run active (or not). No-op until armed.
    pub fn set_run_active(&self, run_id: &str, active: bool) {
        if !self.armed.load(Ordering::SeqCst) {
            return;
        }
        {
            let mut inner = self.inner.lock();
            let changed = if active {
                inner.runs.insert(run_id.to_string())
            } else {
                inner.runs.remove(run_id)
            };
            if !changed {
                return;
            }
        }
        self.apply();
    }

    /// Recompute desired assertion state + fire the status-line callback.
    fn apply(&self) {
        let (agents_n, runs_n, cb) = {
            let mut inner = self.inner.lock();
            let active = !inner.agents.is_empty() || !inner.runs.is_empty();
            // Any recompute cancels a pending debounced release.
            inner.release_gen = inner.release_gen.wrapping_add(1);
            if active {
                ensure_assertion(&mut inner);
            } else {
                schedule_release(inner.release_gen);
            }
            (inner.agents.len(), inner.runs.len(), inner.on_change.clone())
        };
        if let Some(cb) = cb {
            cb(agents_n, runs_n);
        }
    }
}

/// Debounced release: after [`RELEASE_DEBOUNCE`], drop the assertion iff still
/// idle and no newer recompute has happened (generation match).
fn schedule_release(generation: u64) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(RELEASE_DEBOUNCE).await;
        let m = ActivityMonitor::global();
        let mut inner = m.inner.lock();
        if inner.release_gen == generation && inner.agents.is_empty() && inner.runs.is_empty() {
            release_assertion(&mut inner);
        }
    });
}

// ───────────────────────────── platform impl ────────────────────────────────

#[cfg(target_os = "macos")]
fn ensure_assertion(inner: &mut Inner) {
    if inner.assertion.is_some() {
        return;
    }
    inner.assertion = macos::create_assertion();
    if inner.assertion.is_none() {
        tracing::warn!("failed to create IOKit sleep assertion; system may idle-sleep mid-run");
    }
}

#[cfg(target_os = "macos")]
fn release_assertion(inner: &mut Inner) {
    if let Some(id) = inner.assertion.take() {
        macos::release_assertion(id);
    }
}

#[cfg(not(target_os = "macos"))]
fn ensure_assertion(_inner: &mut Inner) {}

#[cfg(not(target_os = "macos"))]
fn release_assertion(_inner: &mut Inner) {}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{c_void, CString};
    use std::os::raw::c_char;

    type CFStringRef = *const c_void;

    // kIOPMAssertionLevelOn. kIOReturnSuccess == 0. kCFStringEncodingUTF8.
    const ASSERTION_LEVEL_ON: u32 = 255;
    const IO_RETURN_SUCCESS: i32 = 0;
    const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[link(name = "IOKit", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            assertion_level: u32,
            assertion_name: CFStringRef,
            assertion_id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(assertion_id: u32) -> i32;
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    /// Build an owned `CFString` from a Rust `&str`. Caller must `CFRelease` it.
    unsafe fn cfstr(s: &str) -> Option<CFStringRef> {
        let c = CString::new(s).ok()?;
        let r = CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), CF_STRING_ENCODING_UTF8);
        if r.is_null() {
            None
        } else {
            Some(r)
        }
    }

    pub(super) fn create_assertion() -> Option<u32> {
        unsafe {
            let typ = cfstr("PreventUserIdleSystemSleep")?;
            let name = match cfstr(super::ASSERTION_NAME) {
                Some(n) => n,
                None => {
                    CFRelease(typ);
                    return None;
                }
            };
            let mut id: u32 = 0;
            let rc = IOPMAssertionCreateWithName(typ, ASSERTION_LEVEL_ON, name, &mut id);
            CFRelease(typ);
            CFRelease(name);
            if rc == IO_RETURN_SUCCESS {
                tracing::debug!(assertion_id = id, "held sleep assertion");
                Some(id)
            } else {
                None
            }
        }
    }

    pub(super) fn release_assertion(id: u32) {
        unsafe {
            IOPMAssertionRelease(id);
        }
        tracing::debug!(assertion_id = id, "released sleep assertion");
    }
}

/// Human-readable menu-bar status line. Static text; updated on every change.
pub fn status_line(agents: usize, runs: usize) -> String {
    if agents == 0 && runs == 0 {
        return "No active agents".to_string();
    }
    let a = format!("{agents} agent{} working", if agents == 1 { "" } else { "s" });
    let r = format!("{runs} run{} active", if runs == 1 { "" } else { "s" });
    format!("{a} · {r}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_formats_singular_and_plural() {
        assert_eq!(status_line(0, 0), "No active agents");
        assert_eq!(status_line(1, 0), "1 agent working · 0 runs active");
        assert_eq!(status_line(2, 1), "2 agents working · 1 run active");
    }

    #[test]
    fn unarmed_monitor_ignores_updates() {
        // The global starts unarmed under test; mutators must be inert so tests
        // never create real IOKit assertions.
        let m = ActivityMonitor::global();
        m.set_agent_running("a1", true);
        m.set_run_active("r1", true);
        let inner = m.inner.lock();
        assert!(inner.agents.is_empty());
        assert!(inner.runs.is_empty());
    }
}

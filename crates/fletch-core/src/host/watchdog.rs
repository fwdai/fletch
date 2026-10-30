//! A liveness watchdog for the engine runtime.
//!
//! A runtime whose every worker is pinned cannot log that it is: every task
//! that would is waiting for a worker. So a task *on* the runtime stamps a
//! heartbeat every [`BEAT`], and a plain `std::thread` reads it every
//! [`CHECK`]. A stamp older than [`STALL_AFTER`] is logged as one ERROR per
//! episode (Sentry gets it through the tracing integration), then an INFO
//! once the heartbeat moves again.
//!
//! Both sides read [`Instant`], which stops while the machine sleeps
//! (`CLOCK_UPTIME_RAW` on macOS, `CLOCK_MONOTONIC` on Linux), so a laptop
//! waking up is not a stall. A paused process (debugger, SIGSTOP, App Nap) is
//! caught by the watchdog's own oversleep instead ([`Detector::observe`]).
//!
//! The thread holds nothing the runtime needs: the stamp is an atomic, and the
//! report's figures are tokio's stable metrics.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use tokio::runtime::Handle;

/// How often the heartbeat task stamps.
const BEAT: Duration = Duration::from_secs(1);
/// How often the watchdog thread reads the stamp.
const CHECK: Duration = Duration::from_secs(3);
/// A stamp this old is a stall. Far above any legitimate scheduling delay (a
/// worker blocked on one slow sync call holds one worker, not the heartbeat),
/// short enough that a freeze is in the log before the user gives up on it.
const STALL_AFTER: Duration = Duration::from_secs(15);

/// The stamp once the heartbeat task is gone: its runtime shut down (app
/// exit), so the thread ends rather than report the silence as a stall.
const STOPPED: u64 = u64::MAX;

/// Start watching `handle`'s runtime. Once per process; later calls do
/// nothing. Never panics: a watchdog that cannot start logs a warning and the
/// engine runs unwatched.
pub fn start(handle: &Handle) {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let metrics = handle.clone();
        let timing = Timing {
            beat: BEAT,
            check: CHECK,
            stall_after: STALL_AFTER,
        };
        if let Err(e) = watch(handle, timing, move |transition| {
            report(&metrics, transition)
        }) {
            tracing::warn!(error = %e, "runtime watchdog did not start");
        }
    });
}

fn report(handle: &Handle, transition: Transition) {
    match transition {
        Transition::Stalled { stalled_ms } => {
            let metrics = handle.metrics();
            tracing::error!(
                stalled_ms,
                num_workers = metrics.num_workers(),
                num_alive_tasks = metrics.num_alive_tasks(),
                global_queue_depth = metrics.global_queue_depth(),
                "engine runtime stalled: no worker has polled the heartbeat"
            );
        }
        Transition::Recovered { stalled_ms } => {
            tracing::info!(stalled_ms, "engine runtime recovered");
        }
    }
}

/// The watchdog's periods; production uses the consts above, the tests
/// milliseconds.
#[derive(Debug, Clone, Copy)]
struct Timing {
    beat: Duration,
    check: Duration,
    stall_after: Duration,
}

/// What the watchdog reports: once when an episode begins, once when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transition {
    /// The heartbeat is `stalled_ms` old.
    Stalled { stalled_ms: u64 },
    /// The heartbeat moved again, `stalled_ms` after its last stamp before
    /// the stall.
    Recovered { stalled_ms: u64 },
}

/// Milliseconds since `origin`; both sides of the heartbeat count from the
/// same one. Saturates short of [`STOPPED`] (it would take 584 million years).
fn millis_since(origin: Instant) -> u64 {
    u64::try_from(origin.elapsed().as_millis()).map_or(STOPPED - 1, |ms| ms.min(STOPPED - 1))
}

/// Start the watchdog thread, then the heartbeat on `handle` it reads (so a
/// thread that fails to start leaves no heartbeat behind), handing each
/// [`Transition`] to `on_transition` (on that thread).
fn watch(
    handle: &Handle,
    timing: Timing,
    mut on_transition: impl FnMut(Transition) + Send + 'static,
) -> std::io::Result<()> {
    let origin = Instant::now();
    let stamp = Arc::new(AtomicU64::new(0));

    let read = stamp.clone();
    std::thread::Builder::new()
        .name("runtime-watchdog".into())
        .spawn(move || {
            let mut detector = Detector::new(timing.stall_after);
            loop {
                std::thread::sleep(timing.check);
                let last = read.load(Ordering::Relaxed);
                if last == STOPPED {
                    return;
                }
                if let Some(transition) = detector.observe(last, millis_since(origin)) {
                    on_transition(transition);
                }
            }
        })?;

    /// Marks the stamp [`STOPPED`] when the runtime drops the heartbeat task.
    struct Stopped(Arc<AtomicU64>);
    impl Drop for Stopped {
        fn drop(&mut self) {
            self.0.store(STOPPED, Ordering::Relaxed);
        }
    }
    let beating = Stopped(stamp);
    handle.spawn(async move {
        let beating = beating;
        loop {
            beating.0.store(millis_since(origin), Ordering::Relaxed);
            tokio::time::sleep(timing.beat).await;
        }
    });
    Ok(())
}

/// The stall/recovery state machine, apart from the clock and the threads so
/// it can be tested on numbers.
#[derive(Debug)]
struct Detector {
    stall_after_ms: u64,
    /// The stamp the current stall began at; `None` while healthy.
    stalled_at: Option<u64>,
    /// When the previous sample was read.
    checked_at: u64,
}

impl Detector {
    fn new(stall_after: Duration) -> Self {
        Self {
            stall_after_ms: u64::try_from(stall_after.as_millis()).unwrap_or(u64::MAX),
            stalled_at: None,
            checked_at: 0,
        }
    }

    /// Read the latest stamp `last` at time `now` (both in ms since the
    /// origin): a transition when an episode begins or ends, else nothing.
    /// An episode ends when the stamp moves at all, so a runtime that polls
    /// the heartbeat once and stalls again is a second episode, not one long
    /// one that never reported recovering.
    ///
    /// A sample read a stall's length or more after the previous one is
    /// skipped: the watchdog was not scheduled either, so the whole process
    /// was paused (debugger, SIGSTOP, App Nap), not just the runtime.
    fn observe(&mut self, last: u64, now: u64) -> Option<Transition> {
        let since_check = now.saturating_sub(self.checked_at);
        self.checked_at = now;
        if since_check >= self.stall_after_ms {
            return None;
        }
        match self.stalled_at {
            None => {
                let age = now.saturating_sub(last);
                (age >= self.stall_after_ms).then(|| {
                    self.stalled_at = Some(last);
                    Transition::Stalled { stalled_ms: age }
                })
            }
            Some(at) if last != at => {
                self.stalled_at = None;
                Some(Transition::Recovered {
                    stalled_ms: last.saturating_sub(at),
                })
            }
            Some(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_report_per_episode_and_one_on_recovery() {
        let mut detector = Detector::new(Duration::from_millis(100));
        assert_eq!(detector.observe(0, 50), None);
        assert_eq!(
            detector.observe(0, 120),
            Some(Transition::Stalled { stalled_ms: 120 })
        );
        // Still the same stamp: the same episode, not reported again.
        assert_eq!(detector.observe(0, 180), None);
        assert_eq!(
            detector.observe(250, 260),
            Some(Transition::Recovered { stalled_ms: 250 })
        );
        assert_eq!(detector.observe(300, 340), None);
        // A second episode is reported on its own.
        assert_eq!(
            detector.observe(300, 410),
            Some(Transition::Stalled { stalled_ms: 110 })
        );
    }

    /// A gap between samples as long as a stall means the process was not
    /// running at all: the stale stamp it finds is not a runtime stall.
    #[test]
    fn a_paused_process_is_not_a_stall() {
        let mut detector = Detector::new(Duration::from_millis(100));
        assert_eq!(detector.observe(0, 50), None);
        assert_eq!(detector.observe(50, 5_000), None);
        assert_eq!(detector.observe(5_030, 5_060), None);
        assert_eq!(
            detector.observe(5_030, 5_140),
            Some(Transition::Stalled { stalled_ms: 110 })
        );
    }

    /// The real thing on a real runtime: both workers of a two-worker runtime
    /// blocked on purpose is reported as a stall, and their release as a
    /// recovery, each within a bounded wait.
    #[test]
    fn a_runtime_with_every_worker_blocked_is_reported_and_its_recovery_too() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let timing = Timing {
            beat: Duration::from_millis(10),
            check: Duration::from_millis(20),
            stall_after: Duration::from_millis(1_000),
        };
        watch(runtime.handle(), timing, move |transition| {
            let _ = tx.send(transition);
        })
        .unwrap();

        // Healthy for longer than `stall_after`: nothing to report.
        assert!(rx.recv_timeout(Duration::from_millis(1_500)).is_err());

        // Pin both workers until released: each holds its worker in a sync
        // wait, the shape of the freeze. Bounded, so a failing assertion
        // below cannot leave the runtime's drop waiting on them forever.
        let mut release = Vec::new();
        for _ in 0..2 {
            let (tx, pinned) = std::sync::mpsc::channel::<()>();
            release.push(tx);
            runtime.spawn(async move {
                let _ = pinned.recv_timeout(Duration::from_secs(10));
            });
        }
        let stall = rx.recv_timeout(Duration::from_secs(5));
        // One report per episode, however long it lasts.
        let repeat = rx.recv_timeout(Duration::from_millis(200));
        drop(release);
        let recovery = rx.recv_timeout(Duration::from_secs(5));

        match stall {
            Ok(Transition::Stalled { stalled_ms }) => assert!(stalled_ms >= 1_000),
            other => panic!("expected a stall, got {other:?}"),
        }
        assert!(repeat.is_err(), "{repeat:?}");
        match recovery {
            Ok(Transition::Recovered { stalled_ms }) => assert!(stalled_ms >= 1_000),
            other => panic!("expected a recovery, got {other:?}"),
        }
    }

    /// A runtime that shuts down ends its watchdog instead of reporting the
    /// silence as a stall.
    #[test]
    fn a_runtime_shutting_down_is_not_a_stall() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let timing = Timing {
            beat: Duration::from_millis(10),
            check: Duration::from_millis(20),
            stall_after: Duration::from_millis(1_000),
        };
        watch(runtime.handle(), timing, move |transition| {
            let _ = tx.send(transition);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        drop(runtime);
        // The thread ends and drops the sender: a disconnect, not a report.
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        );
    }
}

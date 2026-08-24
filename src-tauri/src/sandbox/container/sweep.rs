//! The startup orphan sweep's retry schedule, shared by every container
//! runtime.
//!
//! A single 2s probe at startup loses the race against a runtime that takes
//! tens of seconds after login to answer, and the sweeps are the only thing
//! that ever reclaims a dead instance's containers — so the probe retries on
//! [`SWEEP_RETRY_INTERVAL`], bounded by [`SWEEP_RETRY_BUDGET`].

use std::time::Duration;

/// Wait between probes while the runtime comes up. Must stay above each probe's
/// 5s cache TTL, or a poll re-reads the same cached answer.
pub(crate) const SWEEP_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// How long the startup sweep waits on a down runtime before giving up — long
/// enough for a cold login, bounded so a machine without the runtime never
/// keeps a thread ticking. It gates when waits stop being *scheduled*, so the
/// final probe can land one [`SWEEP_RETRY_INTERVAL`] past it.
pub(crate) const SWEEP_RETRY_BUDGET: Duration = Duration::from_secs(10 * 60);

/// What the startup sweep thread does after one probe — see [`sweep_step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SweepStep {
    /// The runtime answered: run the sweeps, once, and finish.
    Sweep,
    /// The runtime is down but may still be booting: wait and probe again.
    Retry,
    /// Nothing to wait for, or the budget is spent: the thread exits having
    /// swept nothing.
    Stop,
}

/// The retry schedule, as a pure decision over "usable now?" and "installed at
/// all?". A missing binary stops immediately — only "installed but down" is a
/// state worth waiting on.
pub(crate) fn sweep_step(usable: bool, installed: bool, elapsed: Duration) -> SweepStep {
    if usable {
        SweepStep::Sweep
    } else if installed && elapsed < SWEEP_RETRY_BUDGET {
        SweepStep::Retry
    } else {
        SweepStep::Stop
    }
}

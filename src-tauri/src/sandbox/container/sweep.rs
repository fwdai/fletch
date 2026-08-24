//! The startup orphan sweep's retry schedule, shared by every container
//! runtime.
//!
//! A single probe at startup loses a race it usually can't win: a container
//! runtime takes tens of seconds after login to answer (Docker Desktop's boot,
//! a `podman machine` starting), the probe times out at 2s, and a Fletch
//! launched from the dock or as a login item therefore reports "down" and
//! sweeps nothing. The sweeps are the only thing that ever reclaims a dead
//! instance's containers, so a user who loses that race every launch
//! accumulates them forever. Retrying costs an idle thread and one probe
//! round-trip per [`SWEEP_RETRY_INTERVAL`], bounded by [`SWEEP_RETRY_BUDGET`].

use std::time::Duration;

/// How long the startup sweep waits between probes while the runtime is still
/// coming up. Comfortably above each probe's 5s cache TTL, so every poll is one
/// genuine round-trip rather than a re-read of the same cached answer, and
/// short enough that the sweeps start within half a minute of the runtime
/// finishing its boot.
pub(crate) const SWEEP_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// How long the startup sweep keeps waiting for a down runtime before giving
/// up. Ten minutes covers a cold login — Docker Desktop's own 20-60s (or a
/// `podman machine` boot) plus a slow disk, a VPN prompt, or a user who starts
/// it by hand a few minutes in — while staying strictly bounded: a machine
/// where the runtime simply never runs must not keep a thread ticking for the
/// life of the app. The budget gates when we stop *scheduling* waits, so the
/// final probe can land up to one [`SWEEP_RETRY_INTERVAL`] past it. Giving up
/// is cheap: the next app start sweeps again, so the cost of a miss is one
/// run's worth of unreclaimed containers, not a permanent leak.
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

/// The retry schedule as a pure decision over the two questions every runtime
/// probe answers: is it usable now, and is it even installed.
///
/// A missing binary stops immediately rather than retrying: a runtime being
/// *installed* mid-run is rare, the next app start covers it, and each retry
/// would re-pay the binary-resolution stat walk for a state that essentially
/// never flips. "Installed but down" is the state we expect to flip — that's
/// the whole point of the retry.
pub(crate) fn sweep_step(usable: bool, installed: bool, elapsed: Duration) -> SweepStep {
    if usable {
        SweepStep::Sweep
    } else if installed && elapsed < SWEEP_RETRY_BUDGET {
        SweepStep::Retry
    } else {
        SweepStep::Stop
    }
}

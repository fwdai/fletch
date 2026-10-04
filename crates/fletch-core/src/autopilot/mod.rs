//! Autopilot: nurse an open PR to mergeable, unattended, on the host.
//!
//! - [`readiness`] / [`step`] are the pure decisions, ported from the desktop's
//!   `src/readiness.ts` / `src/autopilot.ts` with every test case.

pub mod readiness;
pub mod step;

//! Ad-hoc verification: the project's deterministic checks run in one of an
//! agent's checkouts. The body the desktop's `run_verification` command calls,
//! and the one the host's autopilot calls to judge a `fix-checks` cycle — which
//! is why `run_verification` itself can stay off the wire.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::supervisor::Supervisor;
use crate::verify::{VerificationReport, Verifier};

use super::files::agent_repo_checkout;

/// Default wall-clock budget for an ad-hoc verification run's checks, matching
/// the workflow tests gate's `DEFAULT_TESTS_TIMEOUT_SECS` (15 min). Ad-hoc
/// checkouts have no step budget to draw from.
const VERIFY_TIMEOUT_SECS: u64 = 900;

/// Releases an agent's `verify_inflight` slot however the run returns —
/// including the `?` early exits below, which a manual `remove` would leak.
struct VerifyGuard {
    supervisor: Arc<Supervisor>,
    agent_id: String,
}

impl Drop for VerifyGuard {
    fn drop(&mut self) {
        self.supervisor
            .verify_inflight
            .lock()
            .remove(&self.agent_id);
    }
}

/// Run install → test → lint in an agent's checkout (`subdir`, primary when
/// `None`), layering the project's `run.test` / `run.install` / `run.lint`
/// overrides over detection, the same layering the workflow tests gate uses.
pub async fn run_verification_impl(
    supervisor: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<VerificationReport> {
    let (repo, checkout) = agent_repo_checkout(supervisor, agent_id, subdir)?;
    // Serialize against the turn-end verification (`trigger_turn_end_verification`)
    // and any other in-flight run for this agent: both drive the same commands
    // in the agent's checkout and would race on the tree and any shared build
    // cache. Coarse on purpose — keyed by agent, not checkout, since two
    // checkouts of one agent still contend for CPU and a dependency cache.
    // Refusing beats queueing: neither the Run panel nor autopilot wants to
    // block behind a 15-minute test run.
    if !supervisor
        .verify_inflight
        .lock()
        .insert(agent_id.to_string())
    {
        return Err(Error::Other(format!(
            "verification already running for agent {agent_id}"
        )));
    }
    let _guard = VerifyGuard {
        supervisor: supervisor.clone(),
        agent_id: agent_id.to_string(),
    };
    let project_id = supervisor
        .workspace
        .agent(agent_id)
        .map(|r| r.project_id)
        .unwrap_or_default();
    let setting = |key: &str| -> Option<String> {
        if project_id.is_empty() {
            None
        } else {
            supervisor.workspace.project_setting(&project_id, key)
        }
    };
    let verifier = Verifier::new(
        setting("run.test"),
        setting("run.install"),
        setting("run.lint"),
        VERIFY_TIMEOUT_SECS,
    )?;
    // The project's shared run env — the same membrane as the Run panel.
    let env = if project_id.is_empty() {
        Vec::new()
    } else {
        supervisor
            .workspace
            .run_env(&project_id, &repo.repo_path, agent_id, &checkout)
    };
    let report = verifier.verify(&checkout, &env).await;
    tracing::info!(
        agent_id = %agent_id,
        passed = report.passed(),
        checks = report.checks.len(),
        "ran ad-hoc verification"
    );
    Ok(report)
}

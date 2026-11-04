//! The autopilot policy: whether to act on what [`next_rung`] says, and when to
//! stop. A port of the desktop's `src/autopilot.ts`, which no longer acts — the
//! host's copy is the only one that does.
//!
//! The unit is a CYCLE, not a turn: dispatch → agent turn → await evidence →
//! verdict. A delegation ends when the agent's turn does ("checks are
//! re-running"); a cycle ends only once the world has said whether the fix
//! worked.
//!
//! Autopilot has one job: once a PR is open, nurse it to mergeable — failing
//! checks, a branch behind or conflicting with its base, review comments.
//! Everything else (no PR, uncommitted work, a review gate, a draft, a thread
//! the agent pushed back on) is a quiet no-op.
//!
//! Two brakes keep an unattended loop from burning turns and CI runs forever:
//! the state SIGNATURE (a cycle that ends on the world it started from changed
//! nothing, and a world proven barren is never re-entered) and the per-rung
//! attempt BUDGET, keyed to the SITUATION it was spent on. Both end in a
//! `give-up`, after which autopilot waits until the world changes.
//!
//! Pure: no clock of its own (`now` is a parameter), no IO.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use super::readiness::{detect_blockers, next_rung, Blocker, LadderContext, ReadinessInput, Rung};
use crate::git_state::{GitState, StatusKind};
use crate::supervisor::delegation::DelegationKind;
use crate::verify::VerificationReport;

/// Rungs autopilot may run on its own. Commit / push / open-pr are absent on
/// purpose: auto-committing someone's working tree is a different risk class
/// from fixing CI on work they already pushed — and it keeps `fix-checks`
/// (whose playbook runs `git add -A`) off a dirty tree.
pub const AUTOPILOT_RUNGS: [DelegationKind; 4] = [
    DelegationKind::FixChecks,
    DelegationKind::Resolve,
    DelegationKind::UpdateBranch,
    DelegationKind::ResolveComments,
];

/// Cycles one rung gets on one situation before autopilot gives up on it.
/// `fix-checks` gets three; the reconcile rungs two; review comments two, since
/// each cycle posts into a real conversation.
pub fn rung_budget(kind: DelegationKind) -> u32 {
    match kind {
        DelegationKind::FixChecks => 3,
        DelegationKind::Resolve
        | DelegationKind::UpdateBranch
        | DelegationKind::ResolveComments => 2,
        _ => 0,
    }
}

/// How a rung's cycle is judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// Run the project's own tests/lints and believe them — a code fix.
    Verify,
    /// Judge on the world alone — a reconcile can be correct and still surface
    /// a pre-existing test failure. The conservative default.
    State,
}

pub fn rung_evidence(kind: DelegationKind) -> Evidence {
    match kind {
        DelegationKind::FixChecks => Evidence::Verify,
        _ => Evidence::State,
    }
}

/// How long to wait for evidence once the agent's turn ends. Generous: a CI
/// run can take many minutes, and a false "no evidence" wastes a budget slot.
pub const EVIDENCE_TIMEOUT_MS: i64 = 15 * 60 * 1000;

/// `Working` spans the agent's turn; `AwaitingEvidence` the gap between the
/// turn ending and the world having something to say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CyclePhase {
    Working,
    AwaitingEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    pub rung: DelegationKind,
    /// 1-based, compared against [`rung_budget`].
    pub attempt: u32,
    /// The observable world at dispatch — see [`state_signature`].
    pub signature: String,
    pub phase: CyclePhase,
    /// Epoch ms the current phase was entered, for the evidence timeout.
    pub phase_since: i64,
}

/// Why autopilot gave up on a rung. A fact about what it did, recorded in the
/// checkout's history — not a state the user has to clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GiveUpReason {
    /// The rung's cycle budget for this situation is spent.
    BudgetSpent,
    /// A cycle ended on a signature that had already produced nothing.
    NoProgress,
    /// No evidence arrived within [`EVIDENCE_TIMEOUT_MS`].
    NoEvidence,
}

impl GiveUpReason {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            GiveUpReason::BudgetSpent => "budget-spent",
            GiveUpReason::NoProgress => "no-progress",
            GiveUpReason::NoEvidence => "no-evidence",
        }
    }
}

/// Per-checkout autopilot state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutopilotState {
    pub enrolled: bool,
    pub cycle: Option<Cycle>,
    /// Cycles spent per rung on the current `situation`. Reset on a successful
    /// cycle and whenever the situation changes.
    pub attempts: HashMap<DelegationKind, u32>,
    /// The blocker fingerprint `attempts` were spent on. Empty until the first
    /// dispatch.
    pub situation: String,
    /// Signatures that already produced a cycle with no progress. Kept across
    /// situations: a world autopilot proved it cannot change stays refused.
    pub barren: Vec<String>,
}

impl AutopilotState {
    /// Fresh state for a newly enrolled checkout.
    pub fn enrolled() -> Self {
        Self {
            enrolled: true,
            ..Self::default()
        }
    }

    /// Open a cycle for a dispatched rung. A situation other than the one the
    /// attempts were spent on starts the budget over; barren signatures stay.
    pub fn open_cycle(
        &mut self,
        rung: DelegationKind,
        signature: String,
        situation: String,
        now: i64,
    ) {
        if self.situation != situation {
            self.attempts.clear();
        }
        self.situation = situation;
        let attempt = self.attempts.get(&rung).copied().unwrap_or(0) + 1;
        self.cycle = Some(Cycle {
            rung,
            attempt,
            signature,
            phase: CyclePhase::Working,
            phase_since: now,
        });
    }

    /// Move the in-flight cycle to `phase`, restamping its clock.
    pub fn advance(&mut self, phase: CyclePhase, now: i64) {
        if let Some(cycle) = &mut self.cycle {
            cycle.phase = phase;
            cycle.phase_since = now;
        }
    }

    /// The cycle worked: clear it and give the rung its budget back, so a
    /// long-lived PR isn't capped for life.
    pub fn settle(&mut self, rung: DelegationKind) {
        self.cycle = None;
        self.attempts.insert(rung, 0);
    }

    /// The cycle failed (whether or not autopilot is giving up): clear it,
    /// count the attempt, and remember a barren world once.
    pub fn retry(&mut self, rung: DelegationKind, barren: Option<&str>) {
        self.cycle = None;
        *self.attempts.entry(rung).or_insert(0) += 1;
        if let Some(b) = barren {
            if !self.barren.iter().any(|s| s == b) {
                self.barren.push(b.to_string());
            }
        }
    }
}

/// A fingerprint of everything autopilot could act on: the head commit, the
/// sorted failing-check names, the sorted conflicted paths, and the sorted
/// thread ids (`!` marking a thread we replied to last). Two cycles with the
/// same signature faced the same world.
pub fn state_signature(input: &ReadinessInput) -> String {
    let git = input.git.as_ref();
    let sha = git.and_then(|g| g.head_sha.as_deref()).unwrap_or("no-head");
    let mut failing: Vec<&str> = input
        .checks
        .as_ref()
        .map(|c| c.required_failing.iter().map(String::as_str).collect())
        .unwrap_or_default();
    failing.sort_unstable();
    let mut conflicted: Vec<&str> = git
        .map(|g| {
            g.files
                .iter()
                .filter(|f| matches!(f.kind, StatusKind::Conflicted))
                .map(|f| f.path.as_str())
                .collect()
        })
        .unwrap_or_default();
    conflicted.sort_unstable();
    let mut threads: Vec<String> = input
        .comments
        .as_ref()
        .map(|c| {
            c.unresolved
                .iter()
                .map(|t| format!("{}{}", t.id, if t.we_replied_last { "!" } else { "" }))
                .collect()
        })
        .unwrap_or_default();
    threads.sort_unstable();
    format!(
        "{sha}|{}|{}|{}",
        failing.join(","),
        conflicted.join(","),
        threads.join(",")
    )
}

/// A fingerprint of the SITUATION — the blocker kinds plus the detail that
/// tells one instance from another. Scopes the attempt budget: a different
/// failing check or conflict set is a different problem and earns its own
/// tries. Deliberately not [`state_signature`], which excludes the merge gate
/// because it flickers through `unknown` while CI recomputes.
pub fn blocker_fingerprint(blockers: &[Blocker]) -> String {
    let mut parts: Vec<String> = blockers
        .iter()
        .map(|b| match b {
            Blocker::ChecksFailing { checks } => {
                let mut names = checks.clone();
                names.sort_unstable();
                format!("{}:{}", b.kind(), names.join(","))
            }
            Blocker::Conflicted { paths } => {
                let mut paths = paths.clone();
                paths.sort_unstable();
                format!("{}:{}", b.kind(), paths.join(","))
            }
            Blocker::ReviewUnaddressed { count } | Blocker::ReviewDisputed { count } => {
                format!("{}:{count}", b.kind())
            }
            _ => b.kind().to_string(),
        })
        .collect();
    parts.sort_unstable();
    parts.join("|")
}

/// Unstaged, non-conflicted edits — the user's in-flight work. The `resolve`
/// playbook finishes a merge with `git add -A`, so their presence means this is
/// not autopilot's merge to finish (the merge's own content is staged).
pub fn unstaged_edits(git: Option<&GitState>) -> usize {
    git.map_or(0, |g| {
        g.files
            .iter()
            .filter(|f| !f.staged && !matches!(f.kind, StatusKind::Conflicted))
            .count()
    })
}

/// Why there is nothing to do this tick. Diagnostics only — a waiting
/// autopilot is the norm, not news.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    NotEnrolled,
    AgentBusy,
    DelegationInFlight,
    /// No open PR: autopilot's job hasn't started.
    NoPr,
    /// The ladder wants something autopilot never does.
    NotMine,
    /// Finishing the merge would swallow the user's uncommitted edits.
    DirtyTree,
    /// A world autopilot already proved it cannot change.
    NoProgress,
    /// Every try for this rung on this situation has been spent.
    BudgetSpent,
    GateSettling,
    AwaitingEvidence,
    /// Nothing autopilot handles is wrong.
    NothingToDo,
}

/// What autopilot wants done about one checkout this tick. The caller performs
/// it and owns the state transition it implies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Open a cycle: hand `rung` to the agent, recording `signature` on the
    /// cycle and `situation` on the checkout.
    Dispatch {
        rung: DelegationKind,
        action: &'static str,
        params: BTreeMap<String, String>,
        signature: String,
        situation: String,
    },
    /// The turn ended: run local verification and enter `awaiting-evidence`.
    Verify,
    /// The turn ended on a state-judged rung: enter `awaiting-evidence`.
    AwaitEvidence,
    /// The cycle worked.
    Settle {
        rung: DelegationKind,
    },
    /// The cycle failed but budget remains.
    Retry {
        rung: DelegationKind,
        barren: Option<String>,
    },
    /// The cycle failed and autopilot is done with this rung on this
    /// situation. Same bookkeeping as `Retry`.
    GiveUp {
        rung: DelegationKind,
        reason: GiveUpReason,
        barren: Option<String>,
    },
    Wait {
        why: WaitReason,
    },
}

pub struct AutopilotInput<'a> {
    pub state: Option<&'a AutopilotState>,
    pub readiness: &'a ReadinessInput,
    pub ladder: &'a LadderContext,
    /// The agent is mid-turn — autopilot never interleaves with a turn it
    /// didn't start.
    pub agent_busy: bool,
    /// A delegation is already live on this checkout (possibly the user's).
    pub delegation_in_flight: bool,
    /// Local verification produced for the current cycle, or `None`.
    pub verification: Option<&'a VerificationReport>,
    pub now: i64,
}

fn wait(why: WaitReason) -> Effect {
    Effect::Wait { why }
}

/// Decide the next move for one checkout. Pure and total. Every reason NOT to
/// act is checked before any reason to act.
pub fn autopilot_step(input: &AutopilotInput) -> Effect {
    let Some(state) = input.state.filter(|s| s.enrolled) else {
        return wait(WaitReason::NotEnrolled);
    };

    // A cycle in flight is judged whatever the PR does meanwhile.
    if let Some(cycle) = &state.cycle {
        return judge_cycle(cycle, state, input);
    }

    let readiness = input.readiness;
    // `map_or`, not `is_none_or`: the crate's rust-version is 1.77.
    if readiness
        .pr
        .as_ref()
        .map_or(true, |p| p.state != crate::github::PrStatus::Open)
    {
        return wait(WaitReason::NoPr);
    }
    if input.agent_busy {
        return wait(WaitReason::AgentBusy);
    }
    if input.delegation_in_flight {
        return wait(WaitReason::DelegationInFlight);
    }

    match next_rung(readiness, input.ladder) {
        Rung::Delegate {
            kind,
            action,
            params,
            ..
        } => {
            if !AUTOPILOT_RUNGS.contains(&kind) {
                return wait(WaitReason::NotMine);
            }
            if kind == DelegationKind::Resolve && unstaged_edits(readiness.git.as_ref()) > 0 {
                return wait(WaitReason::DirtyTree);
            }
            let signature = state_signature(readiness);
            if state.barren.contains(&signature) {
                return wait(WaitReason::NoProgress);
            }
            let situation = blocker_fingerprint(&detect_blockers(readiness));
            let spent = if state.situation == situation {
                state.attempts.get(&kind).copied().unwrap_or(0)
            } else {
                0
            };
            if spent >= rung_budget(kind) {
                return wait(WaitReason::BudgetSpent);
            }
            Effect::Dispatch {
                rung: kind,
                action,
                params,
                signature,
                situation,
            }
        }
        // `gate-computing` / `unknown-state`: never act on an unsettled world.
        Rung::Wait { .. } => wait(WaitReason::GateSettling),
        // escalate / merge / ready / landed: none of it is autopilot's.
        _ => wait(WaitReason::NothingToDo),
    }
}

fn judge_cycle(cycle: &Cycle, state: &AutopilotState, input: &AutopilotInput) -> Effect {
    if cycle.phase == CyclePhase::Working {
        if input.agent_busy || input.delegation_in_flight {
            return wait(WaitReason::AwaitingEvidence);
        }
        return match rung_evidence(cycle.rung) {
            Evidence::Verify => Effect::Verify,
            Evidence::State => Effect::AwaitEvidence,
        };
    }

    // ── awaiting-evidence ──
    let moved = state_signature(input.readiness) != cycle.signature;
    let barren = (!moved).then(|| cycle.signature.clone());
    let give_up = |reason| Effect::GiveUp {
        rung: cycle.rung,
        reason,
        barren: barren.clone(),
    };
    let failed = || {
        // A second barren cycle on the same world: retrying is futile.
        if barren.as_ref().is_some_and(|b| state.barren.contains(b)) {
            return give_up(GiveUpReason::NoProgress);
        }
        if cycle.attempt >= rung_budget(cycle.rung) {
            return give_up(GiveUpReason::BudgetSpent);
        }
        Effect::Retry {
            rung: cycle.rung,
            barren: barren.clone(),
        }
    };

    // The project's own tests failing is decisive for a code fix, whatever CI
    // hasn't said yet.
    if rung_evidence(cycle.rung) == Evidence::Verify
        && input.verification.is_some_and(|v| !v.passed())
    {
        return failed();
    }

    let rung = next_rung(input.readiness, input.ladder);
    if matches!(rung, Rung::Wait { .. }) {
        if input.now - cycle.phase_since > EVIDENCE_TIMEOUT_MS {
            return give_up(GiveUpReason::NoEvidence);
        }
        return wait(WaitReason::AwaitingEvidence);
    }

    let still_blocked = matches!(rung, Rung::Delegate { kind, .. } if kind == cycle.rung);
    if still_blocked || !moved {
        return failed();
    }
    Effect::Settle { rung: cycle.rung }
}

#[cfg(test)]
#[path = "step_tests.rs"]
mod tests;

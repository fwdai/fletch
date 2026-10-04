//! Readiness: what stands between a checkout's work and landing, and what fixes
//! it. A port of `src/readiness.ts` (and the slice of `src/mergeGate.ts` it
//! reads), which the desktop keeps for rendering — the Git panel's main action
//! and Mission Control ask the same two questions in their own words:
//!
//! - [`detect_blockers`] — WHAT is wrong. The one forge-coupled function: it
//!   knows blockers are read out of `git status`, GitHub's merge gate and
//!   review threads.
//! - [`next_rung`] — WHAT TO DO about the most blocking thing. Pure and total:
//!   every state yields a rung, so no caller has to invent a fallback.
//!
//! The two copies must agree; `tests.rs` carries every case the TypeScript
//! tests pin, so a change to one that the other lacks shows up as a failure.

use std::collections::BTreeMap;

use crate::git_state::{GitState, StatusKind};
use crate::github::{MergeState, MergeableState, PrChecks, PrComments, PrState, PrStatus};
use crate::supervisor::delegation::DelegationKind;

/// A reason this work isn't ready to land. Ordered by the ladder, not by this
/// declaration — see [`next_rung`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// The working copy has unresolved conflict markers.
    Conflicted { paths: Vec<String> },
    /// Edits in the working copy that were never committed.
    Uncommitted { files: usize },
    /// Commits that exist only locally.
    Unpushed { commits: u32 },
    /// Pushed, but never proposed for review.
    Unsubmitted,
    /// Mainline moved; this can't merge cleanly as it stands.
    Diverged { mainline: String },
    /// Failing checks, by name. Every failing check, blocking whether or not it
    /// shuts the merge gate: a repo with no required checks reports a red run
    /// as `unstable`, and a failing spec is worth fixing there too.
    ChecksFailing { checks: Vec<String> },
    /// Review threads waiting on us.
    ReviewUnaddressed { count: usize },
    /// Open threads where we had the last word — a deliberate push-back a
    /// person has to settle.
    ReviewDisputed { count: usize },
    /// A human has to approve.
    ReviewRequired,
    /// Still a draft.
    Draft,
    /// The proposal was closed without landing.
    ProposalClosed,
}

impl Blocker {
    /// The TypeScript `kind` spelling — what [`blocker_fingerprint`] is built
    /// from, so the two ports fingerprint a situation identically.
    ///
    /// [`blocker_fingerprint`]: super::step::blocker_fingerprint
    pub fn kind(&self) -> &'static str {
        match self {
            Blocker::Conflicted { .. } => "conflicted",
            Blocker::Uncommitted { .. } => "uncommitted",
            Blocker::Unpushed { .. } => "unpushed",
            Blocker::Unsubmitted => "unsubmitted",
            Blocker::Diverged { .. } => "diverged",
            Blocker::ChecksFailing { .. } => "checks-failing",
            Blocker::ReviewUnaddressed { .. } => "review-unaddressed",
            Blocker::ReviewDisputed { .. } => "review-disputed",
            Blocker::ReviewRequired => "review-required",
            Blocker::Draft => "draft",
            Blocker::ProposalClosed => "proposal-closed",
        }
    }
}

/// Local work that appeared *after* a merge. Deliberately local-only: `ahead`
/// is measured against a base that stays stale until the next fetch, so a
/// squash-merge leaves `ahead > 0` with nothing new in the tree.
pub fn has_work_since_merge(git: &GitState) -> bool {
    !git.files.is_empty() || git.unpushed > 0
}

/// Everything one checkout's readiness is judged on. `None` is "not read" —
/// never "nothing wrong".
#[derive(Debug, Clone, Default)]
pub struct ReadinessInput {
    pub git: Option<GitState>,
    /// `None` when this checkout has no proposal open.
    pub pr: Option<PrState>,
    /// `None` when the checks read didn't resolve — distinct from a rollup of
    /// zero checks.
    pub checks: Option<PrChecks>,
    /// `None` when review threads haven't been read for this checkout.
    pub comments: Option<PrComments>,
}

/// GitHub's merge gate, as the one canonical situation every surface renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Situation {
    Ready,
    MergeableSoft,
    ChecksFailing,
    ReviewRequired,
    Behind,
    Conflicts,
    Draft,
    Computing,
    NoConflicts,
}

/// The two answers the ladder needs from `describeMergeGate`: what the gate
/// says is wrong, and whether GitHub would take the merge as it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeGate {
    pub situation: Situation,
    pub merge_allowed: bool,
}

/// `describeMergeGate` in `src/mergeGate.ts`. `merge_state: None` (no checks
/// data) falls back to `mergeable`, which reports conflict presence only —
/// never CI status, so it never opens the gate.
pub fn describe_merge_gate(
    merge_state: Option<MergeState>,
    checks_failed: usize,
    mergeable: MergeableState,
) -> MergeGate {
    let gate = |situation, merge_allowed| MergeGate {
        situation,
        merge_allowed,
    };
    match merge_state {
        Some(MergeState::Clean) => gate(Situation::Ready, true),
        // A failing check is something to fix even when GitHub would let the
        // merge through — `unstable` is what a repo with no required checks
        // reports for a red run.
        Some(MergeState::Unstable) if checks_failed > 0 => gate(Situation::ChecksFailing, true),
        Some(MergeState::Unstable) => gate(Situation::MergeableSoft, true),
        Some(MergeState::Blocked) if checks_failed > 0 => gate(Situation::ChecksFailing, false),
        Some(MergeState::Blocked) => gate(Situation::ReviewRequired, false),
        Some(MergeState::Behind) => gate(Situation::Behind, false),
        Some(MergeState::Dirty) => gate(Situation::Conflicts, false),
        Some(MergeState::Draft) => gate(Situation::Draft, false),
        Some(MergeState::Unknown | MergeState::HasHooks) => gate(Situation::Computing, false),
        None => match mergeable {
            MergeableState::Mergeable => gate(Situation::NoConflicts, false),
            MergeableState::Conflicting => gate(Situation::Conflicts, false),
            MergeableState::Unknown => gate(Situation::Computing, false),
        },
    }
}

/// Everything standing between this work and landing, most blocking first.
/// Empty means nothing is in the way — which is NOT "landed" or "still
/// loading"; ask [`next_rung`] for that distinction.
pub fn detect_blockers(input: &ReadinessInput) -> Vec<Blocker> {
    let Some(git) = &input.git else {
        return Vec::new();
    };
    let pr = input.pr.as_ref();
    let mut blockers = Vec::new();

    let conflicted: Vec<String> = git
        .files
        .iter()
        .filter(|f| matches!(f.kind, StatusKind::Conflicted))
        .map(|f| f.path.clone())
        .collect();
    if !conflicted.is_empty() {
        blockers.push(Blocker::Conflicted { paths: conflicted });
    } else if !git.files.is_empty() {
        // Mid-conflict, "uncommitted" restates the conflict.
        blockers.push(Blocker::Uncommitted {
            files: git.files.len(),
        });
    }

    if git.unpushed > 0 {
        blockers.push(Blocker::Unpushed {
            commits: git.unpushed,
        });
    }

    // Work no proposal covers. After a merge only commits the origin branch
    // lacks prove new work (see `has_work_since_merge`).
    let unproposed = match pr {
        None => git.ahead > 0,
        Some(p) => p.state == PrStatus::Merged && git.unpushed > 0,
    };
    if unproposed {
        blockers.push(Blocker::Unsubmitted);
    }
    if pr.is_some_and(|p| p.state == PrStatus::Closed) {
        blockers.push(Blocker::ProposalClosed);
    }

    if let Some(pr) = pr.filter(|p| p.state == PrStatus::Open) {
        let failing: Vec<String> = input
            .checks
            .as_ref()
            .map(|c| c.required_failing.clone())
            .unwrap_or_default();
        let gate = describe_merge_gate(
            input.checks.as_ref().map(|c| c.merge_state),
            failing.len(),
            pr.mergeable,
        );
        match gate.situation {
            Situation::Conflicts | Situation::Behind => blockers.push(Blocker::Diverged {
                mainline: git.parent_branch.clone(),
            }),
            Situation::ChecksFailing => blockers.push(Blocker::ChecksFailing { checks: failing }),
            Situation::ReviewRequired => blockers.push(Blocker::ReviewRequired),
            Situation::Draft => blockers.push(Blocker::Draft),
            // ready / mergeable-soft / no-conflicts / computing add none.
            _ => {}
        }
        // Split by who holds the conversation: a thread we replied to last is
        // a push-back awaiting a human, not work for the agent.
        let threads = input
            .comments
            .as_ref()
            .map(|c| c.unresolved.as_slice())
            .unwrap_or_default();
        let ours = threads.iter().filter(|t| t.we_replied_last).count();
        let theirs = threads.len() - ours;
        if theirs > 0 {
            blockers.push(Blocker::ReviewUnaddressed { count: theirs });
        }
        if ours > 0 {
            blockers.push(Blocker::ReviewDisputed { count: ours });
        }
    }

    blockers
}

/// Why the ladder says not to act yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitWhy {
    UnknownState,
    GateComputing,
}

/// How the ladder wants the most blocking problem handled. Plain data: the
/// caller performs the effect and owns any scoping (the host adds `repo` for a
/// secondary checkout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rung {
    /// Hand it to the agent. `action`/`params` name the playbook trigger; an
    /// empty `params` is the TypeScript `undefined`.
    Delegate {
        kind: DelegationKind,
        action: &'static str,
        params: BTreeMap<String, String>,
        blocker: Blocker,
    },
    /// Nothing left in the way and the forge's gate is open.
    Merge,
    /// Blocked on something only a human can clear.
    Escalate { blocker: Blocker },
    /// Don't act: the state isn't settled enough to trust.
    Wait { why: WaitWhy },
    /// Already landed.
    Landed,
    /// Nothing blocking, but the gate isn't open — merging is a decision.
    Ready,
}

/// The user's sticky commit mode, as the ladder sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitMode {
    Commit,
    CommitPush,
    CommitPr,
}

impl CommitMode {
    fn kind(self) -> DelegationKind {
        match self {
            CommitMode::Commit => DelegationKind::Commit,
            CommitMode::CommitPush => DelegationKind::CommitPush,
            CommitMode::CommitPr => DelegationKind::CommitPr,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderContext {
    /// Mainline branch, for the actions that name it.
    pub base: String,
    pub commit_mode: CommitMode,
}

fn params(pairs: &[(&str, String)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn delegate(kind: DelegationKind, pairs: &[(&str, String)], blocker: Blocker) -> Rung {
    Rung::Delegate {
        kind,
        action: kind.action(),
        params: params(pairs),
        blocker,
    }
}

/// The remediation ladder: the ONE ordering of what to do next. Most-blocking
/// first, so each rung's work isn't wasted by the next: reconcile the working
/// copy before committing it, publish before asking the forge, sync with
/// mainline before trusting a check result, fix checks before reading review
/// threads written against them.
pub fn next_rung(input: &ReadinessInput, ctx: &LadderContext) -> Rung {
    let Some(git) = &input.git else {
        return Rung::Wait {
            why: WaitWhy::UnknownState,
        };
    };
    let pr = input.pr.as_ref();
    // Landed is terminal only while nothing new appeared since the merge.
    if pr.is_some_and(|p| p.state == PrStatus::Merged) && !has_work_since_merge(git) {
        return Rung::Landed;
    }

    let blockers = detect_blockers(input);
    let find = |kind: &str| blockers.iter().find(|b| b.kind() == kind).cloned();

    if let Some(b) = find("conflicted") {
        return delegate(DelegationKind::Resolve, &[], b);
    }

    if let Some(b) = find("uncommitted") {
        // With a proposal already open, "open a PR" degrades to "push".
        let mode = if ctx.commit_mode == CommitMode::CommitPr
            && pr.is_some_and(|p| p.state == PrStatus::Open)
        {
            CommitMode::CommitPush
        } else {
            ctx.commit_mode
        };
        let pairs: &[(&str, String)] = if mode == CommitMode::CommitPr {
            &[("base", ctx.base.clone())]
        } else {
            &[]
        };
        return delegate(mode.kind(), pairs, b);
    }

    if let Some(b) = find("unsubmitted") {
        return delegate(DelegationKind::OpenPr, &[("base", ctx.base.clone())], b);
    }

    if let Some(b) = find("unpushed") {
        return delegate(DelegationKind::Push, &[], b);
    }

    if let Some(b) = find("diverged") {
        return delegate(
            DelegationKind::UpdateBranch,
            &[("base", ctx.base.clone())],
            b,
        );
    }

    if let Some(b) = find("checks-failing") {
        let Blocker::ChecksFailing { checks } = &b else {
            unreachable!("found by kind");
        };
        let failing = checks.join(", ");
        return delegate(DelegationKind::FixChecks, &[("failing", failing)], b);
    }

    // Human-owned gates: no remediation exists that the agent could run.
    if let Some(b) = find("review-required")
        .or_else(|| find("draft"))
        .or_else(|| find("proposal-closed"))
    {
        return Rung::Escalate { blocker: b };
    }

    if let Some(b) = find("review-unaddressed") {
        let Blocker::ReviewUnaddressed { count } = &b else {
            unreachable!("found by kind");
        };
        let count = count.to_string();
        return delegate(DelegationKind::ResolveComments, &[("count", count)], b);
    }

    // Only threads we already pushed back on remain: a person decides.
    if let Some(b) = find("review-disputed") {
        return Rung::Escalate { blocker: b };
    }

    // Only an open proposal HAS a merge gate.
    let Some(pr) = pr.filter(|p| p.state == PrStatus::Open) else {
        return Rung::Ready;
    };

    // Nothing blocking. Never merge off a gate that hasn't settled.
    let gate = describe_merge_gate(
        input.checks.as_ref().map(|c| c.merge_state),
        input
            .checks
            .as_ref()
            .map_or(0, |c| c.required_failing.len()),
        pr.mergeable,
    );
    if gate.situation == Situation::Computing {
        return Rung::Wait {
            why: WaitWhy::GateComputing,
        };
    }
    if gate.merge_allowed {
        Rung::Merge
    } else {
        Rung::Ready
    }
}

#[cfg(test)]
#[path = "readiness_tests.rs"]
pub(crate) mod tests;

//! What each isolation engine actually guarantees, stated in one place.
//!
//! The engines are **not equally capable**, and the differences are rooted in
//! their mechanisms rather than in how much work has gone into each. Seatbelt
//! rules are path-based and cover reads only nominally (`allow default`); Docker
//! sees nothing it wasn't mounted but enforces through *inode*-based mounts, so
//! a rename moves the guard. Neither is a superset of the other.
//!
//! Left implicit, that asymmetry degrades silently: a policy expressed once and
//! enforced by one engine reads, from the outside, as enforced by both. This
//! module makes each claim and its per-engine coverage a value, so the gap is
//! *typed* — visible to a reviewer, assertable in a test, and renderable to a
//! user — instead of living in a documentation caveat that drifts.
//!
//! One variant per claim in `docs/isolation.md`'s "what an agent can and cannot
//! do", so the two cannot disagree. This is deliberately the *claim* level, not
//! the path level: "show me exactly which paths this agent may write" needs a
//! live agent's launch context (its mailbox, its blackboard) and is a separate
//! report from "what does this engine guarantee".

use serde::Serialize;

use super::EngineKind;

/// A security property Fletch's isolation claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guarantee {
    ConfinedWrites,
    WithheldGitConfig,
    OpaqueAppData,
    UntouchableSourceGit,
    ConfinedReads,
    ConfinedNetwork,
    HostHeldCredentials,
}

/// How completely an engine delivers a [`Guarantee`].
///
/// `Partial` and `Unenforced` carry their reason, because a coverage gap nobody
/// can explain is a gap nobody closes — and because that reason is what a user
/// reading the isolation panel actually needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Enforced,
    Partial(&'static str),
    Unenforced(&'static str),
}

impl Guarantee {
    /// Every claim, in the order a reader should meet them: what holds
    /// everywhere first, then where the engines diverge.
    pub const ALL: &'static [Self] = &[
        Self::ConfinedWrites,
        Self::WithheldGitConfig,
        Self::OpaqueAppData,
        Self::UntouchableSourceGit,
        Self::ConfinedReads,
        Self::ConfinedNetwork,
        Self::HostHeldCredentials,
    ];

    /// The claim in plain English, phrased as what an agent *cannot* do.
    pub fn claim(self) -> &'static str {
        match self {
            Self::ConfinedWrites => "Cannot write outside its own workspace",
            Self::WithheldGitConfig => "Cannot make host-side git execute code it chose",
            Self::OpaqueAppData => "Cannot read or forge Fletch's own app data",
            Self::UntouchableSourceGit => "Cannot write your repository's .git",
            Self::ConfinedReads => "Cannot read the rest of your disk",
            Self::ConfinedNetwork => "Cannot reach arbitrary network hosts",
            Self::HostHeldCredentials => "Cannot hold your GitHub credentials",
        }
    }

    /// How completely `engine` delivers this claim.
    ///
    /// Exhaustive over both axes on purpose: the compiler, not a reviewer's
    /// memory, is what forces a new guarantee or a new engine to declare its
    /// coverage. That is this module's whole job.
    pub fn coverage(self, engine: EngineKind) -> Coverage {
        use EngineKind::{Docker, SandboxExec};
        match (self, engine) {
            // Both engines' primary job: seatbelt by SBPL deny-then-allow,
            // Docker by mounting nothing else writable.
            (Self::ConfinedWrites, _) => Coverage::Enforced,

            // Policy invariant 3 (`super::policy::GIT_EXEC_CONFIG_FILES`).
            (Self::WithheldGitConfig, SandboxExec) => Coverage::Enforced,
            (Self::WithheldGitConfig, Docker) => Coverage::Partial(
                "the checkout is bind-mounted read-write, so an agent can still write its own \
                 .git/config — nested read-only binds would stop a direct write but not a \
                 rename, because Docker mounts follow the inode where seatbelt's path rules do \
                 not. Instead host-side git refuses to run in a checkout whose config would \
                 execute a program (crate::git::hardening), which is engine-independent. That \
                 refusal sits at the git::cmd helper seam, so the few callers that build a \
                 command directly — push and fetch, which do not run filters or textconv — are \
                 not covered by it",
            ),

            // Seatbelt denies the app-data dir explicitly; Docker never mounts it.
            (Self::OpaqueAppData, _) => Coverage::Enforced,

            // Agents work in a clone with its own .git; under Docker the source
            // .git is never mounted, only its object store and only read-only.
            (Self::UntouchableSourceGit, _) => Coverage::Enforced,

            (Self::ConfinedReads, SandboxExec) => Coverage::Unenforced(
                "the profile starts from (allow default) and confines writes only — the agent \
                 runs as you and can read anything you can",
            ),
            (Self::ConfinedReads, Docker) => Coverage::Enforced,

            (Self::ConfinedNetwork, _) => Coverage::Unenforced(
                "neither engine restricts egress: seatbelt's (allow default) leaves it open and \
                 the container launch sets no --network. Treat every agent as network-capable",
            ),

            // Push/PR never run in the sandbox — they are brokered host-side
            // (`crate::rpc::git`), so no credential enters an agent's process.
            (Self::HostHeldCredentials, _) => Coverage::Partial(
                "credentials stay host-side, and the brokered publish ops are capability-gated \
                 (crate::rpc::caps): a workflow step agent cannot publish at all, and no agent \
                 can push the branch its work is reviewed against. Approval for the act itself \
                 (crate::rpc::approval) is available but OFF by default — autopilot publishes \
                 while nobody is watching, so defaulting it on would stall every unattended run. \
                 Until it is enabled, an agent publishes its own branch under your identity \
                 without asking",
            ),
        }
    }

    /// This claim's coverage under `engine`, flattened for the IPC boundary.
    fn status(self, engine: EngineKind) -> GuaranteeStatus {
        let (coverage, note) = match self.coverage(engine) {
            Coverage::Enforced => ("enforced", None),
            Coverage::Partial(why) => ("partial", Some(why)),
            Coverage::Unenforced(why) => ("unenforced", Some(why)),
        };
        GuaranteeStatus {
            claim: self.claim(),
            coverage,
            note,
        }
    }
}

/// One claim and its coverage, flattened for the IPC boundary.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct GuaranteeStatus {
    pub claim: &'static str,
    /// `"enforced"` | `"partial"` | `"unenforced"` — a stable wire string, so
    /// the UI can style by coverage without re-deriving it.
    pub coverage: &'static str,
    /// Why the coverage is less than complete; absent when it is `enforced`.
    pub note: Option<&'static str>,
}

/// What an agent stamped with `engine` is actually guaranteed.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct IsolationReport {
    /// The engine's settings spelling, so this joins the `sandbox_engine` value.
    pub engine: &'static str,
    pub guarantees: Vec<GuaranteeStatus>,
}

/// Describe what `engine` guarantees — the reviewable, renderable form of the
/// coverage matrix.
pub fn describe(engine: EngineKind) -> IsolationReport {
    IsolationReport {
        engine: engine.as_setting(),
        guarantees: Guarantee::ALL.iter().map(|g| g.status(engine)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENGINES: &[EngineKind] = EngineKind::ALL;

    /// A gap with no stated reason is a gap nobody can act on — and an empty
    /// note would render as a blank explanation in the UI.
    #[test]
    fn every_shortfall_explains_itself() {
        for &g in Guarantee::ALL {
            for &engine in ENGINES {
                assert!(!g.claim().is_empty(), "{g:?} has no claim text");
                if let Coverage::Partial(why) | Coverage::Unenforced(why) = g.coverage(engine) {
                    assert!(
                        why.len() > 20,
                        "{g:?} under {engine:?} needs a real reason, got {why:?}"
                    );
                }
            }
        }
    }

    /// `ALL` must be the complete, duplicate-free set. It drives both the report
    /// and the test above, so an omission would silently narrow *both* — the one
    /// drift the compiler cannot catch here.
    #[test]
    fn all_lists_every_guarantee_once() {
        // Pairwise, not `dedup()` — that only collapses *consecutive* repeats, so
        // a duplicate listed at a distance would slip through.
        for (i, claim) in Guarantee::ALL.iter().enumerate() {
            assert!(
                !Guarantee::ALL[i + 1..].contains(claim),
                "{claim:?} is listed twice"
            );
        }
        // Grown by hand alongside the enum; bump deliberately, so that adding a
        // variant without listing it fails here rather than vanishing from the
        // report.
        assert_eq!(
            Guarantee::ALL.len(),
            7,
            "a new Guarantee must be added to ALL"
        );
    }

    /// The asymmetries are the point of this module, so they are pinned rather
    /// than left to be rediscovered: each engine is stronger than the other on a
    /// different claim, and neither is a superset.
    #[test]
    fn the_engines_are_each_stronger_on_a_different_claim() {
        use EngineKind::{Docker, SandboxExec};
        assert_eq!(
            Guarantee::WithheldGitConfig.coverage(SandboxExec),
            Coverage::Enforced
        );
        // Asserted as "not fully enforced" rather than as one exact variant: the
        // property being pinned is the *asymmetry*, and Docker's coverage here has
        // already moved once (Unenforced → Partial, once the engine-independent
        // config refusal landed). Should it ever reach Enforced, this failing is
        // the correct outcome — the asymmetry would be gone and the claim's whole
        // treatment wants rethinking.
        assert_ne!(
            Guarantee::WithheldGitConfig.coverage(Docker),
            Coverage::Enforced
        );
        assert_eq!(
            Guarantee::ConfinedReads.coverage(Docker),
            Coverage::Enforced
        );
        assert!(matches!(
            Guarantee::ConfinedReads.coverage(SandboxExec),
            Coverage::Unenforced(_)
        ));
    }

    /// The report is what the UI and a reviewer read, so pin its shape: one
    /// entry per claim, in `ALL` order, tagged with the engine's settings
    /// spelling, and carrying a note exactly when coverage is incomplete.
    #[test]
    fn describe_reports_every_claim_for_the_engine() {
        for &engine in ENGINES {
            let report = describe(engine);
            assert_eq!(report.engine, engine.as_setting());
            assert_eq!(report.guarantees.len(), Guarantee::ALL.len());
            for (status, &g) in report.guarantees.iter().zip(Guarantee::ALL) {
                assert_eq!(status.claim, g.claim());
                assert_eq!(
                    status.note.is_none(),
                    status.coverage == "enforced",
                    "{g:?}: a note must accompany exactly the incomplete coverages"
                );
            }
        }
    }
}

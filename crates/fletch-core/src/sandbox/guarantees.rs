//! What each isolation engine actually guarantees, stated in one place.
//!
//! The engines are **not equally capable**, and the differences are rooted in
//! their mechanisms rather than in how much work has gone into each. Seatbelt
//! rules are path-based and cover reads only nominally (`allow default`); a box
//! sees nothing of the host but the folders the user granted it.
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

/// The engine-independent half of [`Guarantee::HostHeldCredentials`]'s
/// shortfall: what gates the brokered publish ops. A macro rather than a
/// `const` because [`Coverage::Partial`] carries a `&'static str` and only
/// `concat!` of literals can prepend an engine's lead-in to it: one copy of
/// user-facing text, prefixed per engine.
macro_rules! brokered_publish_gating {
    () => {
        "The brokered publish ops are capability-gated (crate::rpc::caps): a workflow step \
         agent cannot publish at all, and no agent can push the branch its work is reviewed \
         against. Approval for the act itself (crate::rpc::approval) is available but OFF by \
         default, so until it is enabled an agent publishes its own branch under your identity \
         without asking. Enabling it does not disturb autopilot or a clicked Git action — both \
         are recognised as already authorized — but it remains a secondary control either way: \
         with egress unrestricted, an agent never needed git_push to exfiltrate, so what this \
         gains is attribution, not confidentiality"
    };
}

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

    /// The claim in plain English, phrased as what an agent *cannot* do. The
    /// folders a user grants a workspace (`workspace::folder_grants`) are the
    /// one exception every engine makes, at the user's word, so the claims
    /// that a grant widens say so.
    pub fn claim(self) -> &'static str {
        match self {
            Self::ConfinedWrites => {
                "Cannot write outside its own workspace, except folders you made writable for it"
            }
            Self::WithheldGitConfig => "Cannot make host-side git execute code it chose",
            Self::OpaqueAppData => "Cannot read or forge Fletch's own app data",
            Self::UntouchableSourceGit => "Cannot write your repository's .git",
            Self::ConfinedReads => "Cannot read the rest of your disk, except folders you gave it",
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
        use EngineKind::{Box, SandboxExec};
        match (self, engine) {
            // Every engine's primary job: seatbelt by SBPL deny-then-allow, a
            // box by having no shared folder at all.
            (Self::ConfinedWrites, _) => Coverage::Enforced,

            // Policy invariant 3 (`super::policy::GIT_EXEC_CONFIG_FILES`).
            (Self::WithheldGitConfig, SandboxExec) => Coverage::Enforced,
            // Host git never runs in a box's checkout: the checkout is in the
            // guest, and the host reaches it only over the ssh transport.
            (Self::WithheldGitConfig, Box) => Coverage::Enforced,
            // Seatbelt denies the app-data dir explicitly; a box has no host
            // filesystem to reach it on.
            (Self::OpaqueAppData, _) => Coverage::Enforced,

            // Agents work in a clone with its own .git; a box holds a full copy
            // and no mount at all.
            (Self::UntouchableSourceGit, _) => Coverage::Enforced,

            (Self::ConfinedReads, SandboxExec) => Coverage::Unenforced(
                "the profile starts from (allow default) and confines writes only — the agent \
                 runs as you and can read anything you can, except the carve-outs it denies by \
                 name: Fletch's app data, the host's provider logins, ~/.ssh, ~/.config/gh, the \
                 Keychain, and the folders macOS guards with a privacy prompt (Downloads, \
                 Desktop, Documents, iCloud Drive, other apps' containers)",
            ),
            // A box sees only its own disks.
            (Self::ConfinedReads, Box) => Coverage::Enforced,

            (Self::ConfinedNetwork, SandboxExec) => Coverage::Unenforced(
                "seatbelt does not restrict egress: its (allow default) leaves it open. Treat \
                 every agent on this Mac as network-capable",
            ),
            // No network device: egress only through the host proxy's
            // allow-list, per project. `Enforced` carries no note, so the
            // allow-list is stated here, not in the panel; whether the panel
            // should say it is still open.
            (Self::ConfinedNetwork, Box) => Coverage::Enforced,

            // Push/PR never run in the sandbox — they are brokered host-side
            // (`crate::rpc::git`), so no credential enters an agent's process.
            // Seatbelt shares the host filesystem and env, so it has to withhold
            // the user's own git credentials by name (`seatbelt::
            // deny_host_git_credentials`).
            (Self::HostHeldCredentials, SandboxExec) => Coverage::Partial(concat!(
                "credentials stay host-side: the profile also withholds your own at their \
                 standard locations — ~/.ssh and the launchd ssh-agent socket, the gh login \
                 (~/.config/gh, plus $XDG_CONFIG_HOME/gh and $GH_CONFIG_DIR when set), git's \
                 stored credentials (~/.git-credentials, ~/.config/git/credentials, plus \
                 $XDG_CONFIG_HOME/git/credentials) and its credential-cache socket \
                 (~/.cache/git/credential/socket, ~/.git-credential-cache/socket, plus \
                 $XDG_CACHE_HOME/git/credential/socket) — strips GH_TOKEN and GITHUB_TOKEN from the \
                 agent's env, and sandbox clones use an HTTPS GitHub origin, so git in the \
                 sandbox has no credential path. Not covered: a credential store or cache \
                 socket at a path only your git config names, or an ssh-agent at another \
                 socket path (1Password, gpg-agent). ",
                brokered_publish_gating!(),
            )),
            // Nothing of the host's is in the guest: no mount, no inherited env,
            // only the launch's own env file.
            (Self::HostHeldCredentials, Box) => Coverage::Partial(concat!(
                "credentials stay host-side: the box shares no folder and inherits no \
                 environment from the host, only the launch's own env file, and sandbox clones \
                 use an HTTPS GitHub origin, so git in the box has no credential path. ",
                brokered_publish_gating!(),
            )),
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

//! What an agent may ask the host to do with credentials it never holds.
//!
//! Push and PR creation are brokered: the agent writes a mailbox request and
//! Fletch performs the operation host-side, where the token lives. That keeps
//! credentials out of the sandbox — but it does not constrain *what* gets
//! published. Until this grant existed, `git_push` validated nothing, so an
//! agent sitting on the review base pushed straight onto it, and `args.force`
//! made that a lease-guarded force-push.
//!
//! The grant is stamped when the agent's dispatcher is built at spawn and never
//! re-read — the discipline [`crate::sandbox::EngineKind`] uses, so changing
//! policy later cannot retroactively widen an agent that is already running.
//!
//! Three checks, at the points where the answer is knowable: [`AgentCaps::refuses`]
//! before any work happens; [`AgentCaps::refuses_branch`] once the target branch
//! has been resolved, which is the earliest moment it is known (it may come from
//! the checkout's HEAD rather than from the request); and [`AgentCaps::refuses_force`]
//! for the destructive case — a `--force-with-lease` push, whose lease passes for
//! any branch the agent just fetched, so it is fenced to the agent's *own* work
//! branch rather than any branch it can repoint its HEAD onto.

/// Ops that spend the host's GitHub credentials to *publish*. `git_fetch` is
/// credentialed too but reads only, so it is deliberately not gated here — a
/// step agent still needs it to refresh its base.
fn is_publish_op(op: &str) -> bool {
    matches!(op, "git_push" | "open_pr")
}

/// Conventional trunks no agent may publish to, whatever a repo declares as its
/// base — so a repo reviewed against a release branch still cannot have `main`
/// pushed. Compared case-insensitively: on a case-insensitive filesystem `Main`
/// and `main` are the same ref, and the stricter reading is the safe one.
const PROTECTED_TRUNKS: &[&str] = &["main", "master"];

/// Whether an agent may reach the credentialed publish ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publish {
    /// May push and open PRs, but never onto the branch its work is reviewed
    /// against (see [`AgentCaps::refuses_branch`]).
    OwnBranch,
    /// No credentialed publication at all. Carries its reason, so the refusal
    /// the agent reads tells it what to do instead.
    Denied(&'static str),
}

/// One agent's grant over the host-brokered ops.
///
/// A struct around a single field on purpose: this is the seam a per-agent
/// profile grows on. Network egress and MCP reach are the next two dimensions,
/// and they belong beside this one rather than as further parameters threaded
/// through the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCaps {
    pub publish: Publish,
}

impl AgentCaps {
    /// What an ordinary agent gets: publish its own work, never the branch that
    /// work is reviewed against.
    pub fn interactive() -> Self {
        Self {
            publish: Publish::OwnBranch,
        }
    }

    /// What a workflow run-owned step agent gets. A run publishes through its own
    /// finalize, which is `wf/`-namespace guarded; a step publishing directly
    /// would bypass that guard entirely.
    pub fn run_owned() -> Self {
        Self {
            publish: Publish::Denied(
                "workflow step agents cannot push or open PRs; the run publishes its \
                 wf/ branch when it finalizes",
            ),
        }
    }

    /// Why `op` may not run at all, if so. Checked before any work, so a denied
    /// publish never reaches git.
    pub fn refuses(self, op: &str) -> Option<&'static str> {
        match self.publish {
            Publish::Denied(why) if is_publish_op(op) => Some(why),
            _ => None,
        }
    }

    /// Why `branch` may not be published in a repo reviewed against `base`, if so.
    pub fn refuses_branch(self, branch: &str, base: &str) -> Option<String> {
        if let Publish::Denied(why) = self.publish {
            return Some(why.to_string());
        }
        is_review_target(branch, base).then(|| {
            format!(
                "refusing to publish to '{branch}': it is the branch this work is reviewed \
                 against. Push your own branch (e.g. fix/…) and open a pull request instead"
            )
        })
    }

    /// Why a *force* push to `branch` may not proceed, if so.
    ///
    /// SECURITY: a `--force-with-lease --force-if-includes` push rewrites remote
    /// history, and its lease passes for any branch the agent just fetched — so
    /// without this gate an agent can fetch a shared branch (`develop`, a
    /// `release/*`, a teammate's), `checkout -B` its HEAD onto it locally, and
    /// force-overwrite it under the user's GitHub identity. [`refuses_branch`]
    /// only fences the review base and the trunks; every *other* existing branch
    /// is force-clobberable once fetched. So force is confined to the agent's
    /// **own** work branch — `own_branch`, the name the host recorded when it
    /// materialized the branch (`AgentRecord.repos[].branch`), which, unlike the
    /// live `HEAD` the agent repoints at will, the agent cannot forge. `None`
    /// (no branch recorded yet, or a checkout that never materialized one) fails
    /// **closed**: the destructive op is refused rather than risked. A non-force
    /// push never reaches here and may still create a branch.
    ///
    /// [`refuses_branch`]: AgentCaps::refuses_branch
    pub fn refuses_force(self, branch: &str, own_branch: Option<&str>) -> Option<String> {
        // A denied grant never reaches git at all (the op gate stops it), but the
        // branch/force gates re-check it so no single call site can leak one.
        if let Publish::Denied(why) = self.publish {
            return Some(why.to_string());
        }
        // Compared as `is_review_target` compares: trimmed and case-insensitive,
        // because the app's target is a case-insensitive filesystem where `Fix/X`
        // and `fix/x` name the same loose ref.
        let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
        match own_branch.map(str::trim).filter(|s| !s.is_empty()) {
            Some(own) if same(branch, own) => None,
            Some(own) => Some(format!(
                "refusing to force-push '{}': force is limited to this agent's own branch \
                 '{own}'. Push without --force to create or update a different branch",
                branch.trim()
            )),
            None => Some(format!(
                "refusing to force-push '{}': this agent has no recorded work branch to \
                 authorize a force against yet. Push without --force first (e.g. to open \
                 your pull request); force becomes available for that branch afterward",
                branch.trim()
            )),
        }
    }
}

/// Whether `branch` is something no agent may publish to: the repo's own review
/// base, or a conventional trunk.
fn is_review_target(branch: &str, base: &str) -> bool {
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    same(branch, base) || PROTECTED_TRUNKS.iter().any(|trunk| same(branch, trunk))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exactly the two credential-spending ops. `git_fetch` must stay open — a
    /// step agent needs it to refresh its base — and gating a read-only op would
    /// break `update-branch` for every workflow.
    #[test]
    fn only_the_publishing_ops_are_gated() {
        assert!(is_publish_op("git_push"));
        assert!(is_publish_op("open_pr"));
        for open in [
            "git_fetch",
            "git_status",
            "pr_threads",
            "reply_thread",
            "echo",
        ] {
            assert!(
                !is_publish_op(open),
                "{open} must not be gated as publishing"
            );
        }
    }

    /// The hole this grant closes: an agent on the review base used to push
    /// straight onto it, because nothing validated the branch.
    #[test]
    fn an_ordinary_agent_may_not_publish_the_review_base() {
        let caps = AgentCaps::interactive();
        assert!(
            caps.refuses("git_push").is_none(),
            "the op itself is allowed"
        );
        assert!(caps.refuses_branch("fix/login", "main").is_none());

        for blocked in ["main", "master", "Main", " main "] {
            assert!(
                caps.refuses_branch(blocked, "main").is_some(),
                "{blocked:?} must be refused"
            );
        }
        // A repo reviewed against a release branch protects that branch *and*
        // still protects the conventional trunks.
        assert!(caps.refuses_branch("release/24", "release/24").is_some());
        assert!(caps.refuses_branch("main", "release/24").is_some());
        assert!(caps.refuses_branch("fix/x", "release/24").is_none());
    }

    /// A run-owned step agent is refused at the op gate, before any git runs —
    /// and the refusal names the path that does publish, so the agent isn't left
    /// guessing.
    #[test]
    fn a_run_owned_agent_is_refused_before_git_runs() {
        let caps = AgentCaps::run_owned();
        for op in ["git_push", "open_pr"] {
            let why = caps.refuses(op).expect("must be refused");
            assert!(
                why.contains("finalizes"),
                "the refusal must say what does publish"
            );
        }
        // Reads stay reachable: `update-branch` refreshes the base this way.
        assert!(caps.refuses("git_fetch").is_none());
        // And the branch-level checks agree, so no call site can let a denied
        // grant through on its own — force included.
        assert!(caps.refuses_branch("wf/anything", "main").is_some());
        assert!(caps
            .refuses_force("wf/anything", Some("wf/anything"))
            .is_some());
    }

    /// The gap this closes: `refuses_branch` fences the review base and the
    /// trunks, but a force push could still overwrite any *other* existing branch
    /// (develop, release/*, a teammate's) once fetched. Force is now confined to
    /// the agent's own recorded branch.
    #[test]
    fn force_is_confined_to_the_agents_own_branch() {
        let caps = AgentCaps::interactive();
        // The whole reason force exists: rewrite your own branch after a rebase.
        assert!(caps.refuses_force("fix/login", Some("fix/login")).is_none());
        // Case and whitespace fold to the same ref on the case-insensitive target.
        assert!(caps
            .refuses_force(" Fix/Login ", Some("fix/login"))
            .is_none());
        // A shared branch the agent fetched and repointed HEAD onto — refused,
        // even though `refuses_branch` alone would wave `develop` through.
        for other in ["develop", "release/24", "teammate/wip", "main"] {
            assert!(
                caps.refuses_force(other, Some("fix/login")).is_some(),
                "{other:?} is not the agent's own branch and must not be force-pushed"
            );
        }
        // Fail closed: with no recorded own branch, no force can be authorized.
        assert!(caps.refuses_force("fix/login", None).is_some());
        assert!(caps.refuses_force("develop", None).is_some());
    }
}

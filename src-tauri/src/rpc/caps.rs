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
//! Two checks, at the two points where the answer is knowable: [`AgentCaps::refuses`]
//! before any work happens, and [`AgentCaps::refuses_branch`] once the target
//! branch has been resolved, which is the earliest moment it is known (it may
//! come from the checkout's HEAD rather than from the request).

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
        // And the branch-level check agrees, so neither call site can let a
        // denied grant through on its own.
        assert!(caps.refuses_branch("wf/anything", "main").is_some());
    }
}

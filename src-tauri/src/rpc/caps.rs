//! What an agent may publish with credentials it never holds.
//!
//! Stamped at spawn and never re-read, so a later policy change cannot widen a
//! running agent. Three checks: [`AgentCaps::refuses`] before any work,
//! [`AgentCaps::refuses_branch`] once the branch is known (it may come from HEAD,
//! not the request), [`AgentCaps::refuses_force`] for the destructive case.

/// `git_fetch` is credentialed but read-only, so deliberately not gated.
fn is_publish_op(op: &str) -> bool {
    matches!(op, "git_push" | "open_pr")
}

/// Protected whatever the repo's base is; compared case-insensitively.
const PROTECTED_TRUNKS: &[&str] = &["main", "master"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publish {
    OwnBranch,
    /// Carries its reason so the refusal tells the agent what to do instead.
    Denied(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCaps {
    pub publish: Publish,
}

impl AgentCaps {
    pub fn interactive() -> Self {
        Self {
            publish: Publish::OwnBranch,
        }
    }

    /// A run publishes through its own `wf/`-guarded finalize; a step publishing
    /// directly would bypass it.
    pub fn run_owned() -> Self {
        Self {
            publish: Publish::Denied(
                "workflow step agents cannot push or open PRs; the run publishes its \
                 wf/ branch when it finalizes",
            ),
        }
    }

    /// Denied at the grant, not only in its brief, so a persuaded model still can't push.
    pub fn advisory() -> Self {
        Self {
            publish: Publish::Denied(
                "the project-manager chat proposes roadmap items; it never publishes \
                 code — write the work up as a roadmap item instead",
            ),
        }
    }

    pub fn refuses(self, op: &str) -> Option<&'static str> {
        match self.publish {
            Publish::Denied(why) if is_publish_op(op) => Some(why),
            _ => None,
        }
    }

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

    /// SECURITY: a `--force-with-lease` lease passes for any branch the agent just
    /// fetched, so force is confined to `own_branch`, the name the host recorded
    /// when it materialized the branch and the agent cannot forge. `None` fails
    /// closed. A non-force push never reaches here.
    pub fn refuses_force(self, branch: &str, own_branch: Option<&str>) -> Option<String> {
        // Re-checked here so no single call site can leak a denied grant.
        if let Publish::Denied(why) = self.publish {
            return Some(why.to_string());
        }
        // Trimmed and case-insensitive: on the target filesystem `Fix/X` and `fix/x`
        // name the same loose ref.
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

fn is_review_target(branch: &str, base: &str) -> bool {
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    same(branch, base) || PROTECTED_TRUNKS.iter().any(|trunk| same(branch, trunk))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(caps.refuses_branch("release/24", "release/24").is_some());
        assert!(caps.refuses_branch("main", "release/24").is_some());
        assert!(caps.refuses_branch("fix/x", "release/24").is_none());
    }

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
        assert!(caps.refuses("git_fetch").is_none());
        assert!(caps.refuses_branch("wf/anything", "main").is_some());
        assert!(caps
            .refuses_force("wf/anything", Some("wf/anything"))
            .is_some());
    }

    #[test]
    fn force_is_confined_to_the_agents_own_branch() {
        let caps = AgentCaps::interactive();
        assert!(caps.refuses_force("fix/login", Some("fix/login")).is_none());
        assert!(caps
            .refuses_force(" Fix/Login ", Some("fix/login"))
            .is_none());
        for other in ["develop", "release/24", "teammate/wip", "main"] {
            assert!(
                caps.refuses_force(other, Some("fix/login")).is_some(),
                "{other:?} is not the agent's own branch and must not be force-pushed"
            );
        }
        assert!(caps.refuses_force("fix/login", None).is_some());
        assert!(caps.refuses_force("develop", None).is_some());
    }

    #[test]
    fn an_advisory_chat_cannot_publish_anything() {
        let caps = AgentCaps::advisory();
        for op in ["git_push", "open_pr"] {
            let why = caps.refuses(op).expect("must be refused");
            assert!(
                why.contains("roadmap item"),
                "the refusal must name the deliverable"
            );
        }
        assert!(caps.refuses("git_fetch").is_none());
        assert!(caps.refuses("git_status").is_none());
        assert!(caps.refuses_branch("pm/notes", "main").is_some());
    }
}

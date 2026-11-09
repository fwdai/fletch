//! What an agent may publish with credentials it never holds.
//!
//! Stamped at spawn and never re-read, so a later policy change cannot widen a
//! running agent. Two checks: [`AgentCaps::refuses`] before any work, and
//! [`AgentCaps::refuses_publish`] once the branch is known (it may come from
//! HEAD, not the request) — the whole branch policy, force included, in one
//! place.
//!
//! The policy protects the branches where a push would land code without the
//! review this workspace exists for: the trunks, and the bases the work is
//! reviewed against. A branch this workspace created on origin is its own work
//! branch whatever is stacked on it later, so it is never one of those, and it
//! is the only kind of branch a force may rewrite.

/// `git_fetch` is credentialed but read-only, so deliberately not gated.
/// `pr_merge` is: merging lands the work on the base, which is what a denied
/// grant must never do. So is `workflow_dispatch`: it runs the repository's
/// workflows, and their secrets, against a ref of the agent's choosing.
fn is_publish_op(op: &str) -> bool {
    matches!(
        op,
        "git_push" | "open_pr" | "pr_merge" | "workflow_dispatch"
    )
}

/// Protected whatever the repo's base is; compared case-insensitively.
const PROTECTED_TRUNKS: &[&str] = &["main", "master"];

/// Trimmed and case-insensitive: on the target filesystem `Fix/X` and `fix/x`
/// name the same loose ref.
pub(crate) fn same_branch(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// What the host records about the branches a publish is judged against. Read
/// from the host's own records, never from the request or the checkout, both
/// of which the agent controls.
#[derive(Debug, Clone, Copy)]
pub struct BranchRecord<'a> {
    /// The branches this work is reviewed against: the one the checkout was
    /// forked from, and the base of its focused pull request (the same branch
    /// when nothing is stacked). Off limits unless the workspace created them.
    pub bases: &'a [&'a str],
    /// The branches this workspace's own pushes created on origin.
    pub owned: &'a [String],
}

impl BranchRecord<'_> {
    pub fn owns(&self, branch: &str) -> bool {
        self.owned.iter().any(|own| same_branch(own, branch))
    }
}

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

    /// The local reviewer: it reads the change and reports findings
    /// (`review_submit`); it never edits, commits or publishes. Denied at the
    /// grant like the PM chat, so a persuaded model still can't push.
    pub fn reviewer() -> Self {
        Self {
            publish: Publish::Denied(
                "the reviewer reads and reports; it never edits, commits or publishes — \
                 report your findings through review_submit instead",
            ),
        }
    }

    pub fn refuses(self, op: &str) -> Option<&'static str> {
        match self.publish {
            Publish::Denied(why) if is_publish_op(op) => Some(why),
            _ => None,
        }
    }

    /// Whether a push (forced or not) to `branch` is refused, and why. In order:
    /// a denied grant publishes nothing; the trunks are never published; a
    /// branch the workspace created is its own, pushed and forced freely; any
    /// other base is off limits; any other branch takes a plain push only.
    ///
    /// SECURITY: a `--force-with-lease` lease passes for any branch the agent
    /// just fetched, so force is confined to the names the host recorded when
    /// its own push created them, which the agent cannot forge. Nothing
    /// recorded fails closed. The owned exception is what lets a workspace keep
    /// pushing to the lower branch of its own stack after a PR of its own was
    /// opened against it: stacking changes nothing about who reviews that
    /// branch before it reaches the trunk.
    pub fn refuses_publish(
        self,
        branch: &str,
        force: bool,
        record: BranchRecord<'_>,
    ) -> Option<String> {
        if let Publish::Denied(why) = self.publish {
            return Some(why.to_string());
        }
        let branch = branch.trim();
        let review_base = || {
            format!(
                "refusing to publish to '{branch}': it is the branch this work is reviewed \
                 against. Push your own branch (e.g. fix/…) and open a pull request instead"
            )
        };
        if PROTECTED_TRUNKS
            .iter()
            .any(|trunk| same_branch(branch, trunk))
        {
            return Some(review_base());
        }
        if record.owns(branch) {
            return None;
        }
        if record.bases.iter().any(|base| same_branch(branch, base)) {
            return Some(review_base());
        }
        if !force {
            return None;
        }
        let owned: Vec<&str> = record
            .owned
            .iter()
            .map(|b| b.trim())
            .filter(|b| !b.is_empty())
            .collect();
        Some(if owned.is_empty() {
            format!(
                "refusing to force-push '{branch}': this agent has no recorded work branch to \
                 authorize a force against yet. Push without --force first (e.g. to open \
                 your pull request); force becomes available for that branch afterward"
            )
        } else {
            format!(
                "refusing to force-push '{branch}': force is limited to the branches this \
                 workspace created ({}). Push without --force to create or update a \
                 different branch",
                owned.join(", ")
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_publishing_ops_are_gated() {
        assert!(is_publish_op("git_push"));
        assert!(is_publish_op("open_pr"));
        assert!(is_publish_op("pr_merge"));
        assert!(is_publish_op("workflow_dispatch"));
        for open in [
            "git_fetch",
            "git_ls_remote",
            "git_status",
            "pr_view",
            "pr_list",
            "repo_view",
            "pr_checks",
            "pr_check_log",
            "run_list",
            "run_log",
            "release_list",
            "release_view",
            "pr_comments",
            "pr_threads",
            "reply_thread",
            "pr_edit",
            "pr_comment",
            "pr_close",
            "issue_view",
            "issue_list",
            "issue_comment",
            "echo",
        ] {
            assert!(
                !is_publish_op(open),
                "{open} must not be gated as publishing"
            );
        }
    }

    fn record<'a>(bases: &'a [&'a str], owned: &'a [String]) -> BranchRecord<'a> {
        BranchRecord { bases, owned }
    }

    fn owned(branches: &[&str]) -> Vec<String> {
        branches.iter().map(|b| b.to_string()).collect()
    }

    #[test]
    fn an_ordinary_agent_may_not_publish_the_review_base() {
        let caps = AgentCaps::interactive();
        assert!(
            caps.refuses("git_push").is_none(),
            "the op itself is allowed"
        );
        let none = owned(&[]);
        assert!(caps
            .refuses_publish("fix/login", false, record(&["main"], &none))
            .is_none());

        for blocked in ["main", "master", "Main", " main "] {
            assert!(
                caps.refuses_publish(blocked, false, record(&["main"], &none))
                    .is_some(),
                "{blocked:?} must be refused"
            );
        }
        let rel = record(&["release/24"], &none);
        assert!(caps.refuses_publish("release/24", false, rel).is_some());
        assert!(caps.refuses_publish("main", false, rel).is_some());
        assert!(caps.refuses_publish("fix/x", false, rel).is_none());
    }

    /// The branch the checkout was forked from and the focused PR's base are
    /// both off limits: after a stack, a push to the spawn base must not slip
    /// through because the record now points at the stack.
    #[test]
    fn every_recorded_base_is_protected() {
        let caps = AgentCaps::interactive();
        let mine = owned(&["feat/a"]);
        let bases = ["release/24", "feat/a"];
        assert!(caps
            .refuses_publish("release/24", false, record(&bases, &mine))
            .is_some());
        assert!(caps
            .refuses_publish("feat/b", false, record(&bases, &mine))
            .is_none());
    }

    /// A workspace keeps pushing — and forcing — the lower branch of its own
    /// stack: a branch it created is its own even once it is the base of its
    /// next pull request.
    #[test]
    fn a_branch_the_workspace_created_is_never_its_review_base() {
        let caps = AgentCaps::interactive();
        let mine = owned(&["feat/a", "feat/b"]);
        let stacked = record(&["main", "feat/a"], &mine);
        assert!(caps.refuses_publish("feat/a", false, stacked).is_none());
        assert!(caps.refuses_publish("feat/a", true, stacked).is_none());
        assert!(caps.refuses_publish(" Feat/A ", true, stacked).is_none());
        assert!(caps.refuses_publish("feat/b", true, stacked).is_none());
        // Owning a branch named like a trunk does not unlock the trunk.
        let trunkish = owned(&["main"]);
        assert!(caps
            .refuses_publish("main", false, record(&["main"], &trunkish))
            .is_some());
    }

    #[test]
    fn a_run_owned_agent_is_refused_before_git_runs() {
        let caps = AgentCaps::run_owned();
        for op in ["git_push", "open_pr", "pr_merge", "workflow_dispatch"] {
            let why = caps.refuses(op).expect("must be refused");
            assert!(
                why.contains("finalizes"),
                "the refusal must say what does publish"
            );
        }
        assert!(caps.refuses("git_fetch").is_none());
        let mine = owned(&["wf/anything"]);
        for force in [false, true] {
            assert!(caps
                .refuses_publish("wf/anything", force, record(&["main"], &mine))
                .is_some());
        }
    }

    #[test]
    fn force_is_confined_to_the_branches_the_workspace_created() {
        let caps = AgentCaps::interactive();
        let mine = owned(&["fix/login"]);
        let rec = record(&["main"], &mine);
        assert!(caps.refuses_publish("fix/login", true, rec).is_none());
        assert!(caps.refuses_publish(" Fix/Login ", true, rec).is_none());
        for other in ["develop", "release/24", "teammate/wip", "main"] {
            let why = caps.refuses_publish(other, true, rec).unwrap_or_else(|| {
                panic!("{other:?} is not the workspace's and must not be forced")
            });
            assert!(why.contains("force") || why.contains("reviewed against"));
            if other != "main" {
                assert!(
                    caps.refuses_publish(other, false, rec).is_none(),
                    "{other:?} still takes a plain push"
                );
            }
        }
        let none = owned(&[]);
        let why = caps
            .refuses_publish("fix/login", true, record(&["main"], &none))
            .expect("nothing recorded fails closed");
        assert!(why.contains("no recorded work branch"));
        assert!(caps
            .refuses_publish("develop", true, record(&["main"], &none))
            .is_some());
    }

    #[test]
    fn an_advisory_chat_cannot_publish_anything() {
        let caps = AgentCaps::advisory();
        for op in ["git_push", "open_pr", "pr_merge", "workflow_dispatch"] {
            let why = caps.refuses(op).expect("must be refused");
            assert!(
                why.contains("roadmap item"),
                "the refusal must name the deliverable"
            );
        }
        assert!(caps.refuses("git_fetch").is_none());
        assert!(caps.refuses("git_status").is_none());
        assert!(caps
            .refuses_publish("pm/notes", false, record(&["main"], &owned(&["pm/notes"])))
            .is_some());
    }
}

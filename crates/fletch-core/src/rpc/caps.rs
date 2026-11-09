//! What an agent may publish with credentials it never holds.
//!
//! Stamped at spawn and never re-read, so a later policy change cannot widen a
//! running agent. Two checks: [`AgentCaps::refuses`] before any work, and
//! [`AgentCaps::refuses_publish`] once the branch is known (it may come from
//! HEAD, not the request) — the whole branch policy, force included, in one
//! place, so no exception in one rule can slip past another.

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

/// What a publishing op is about to do to a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publication {
    /// `git_push`: update the branch, or create it on origin.
    Push { force: bool },
    /// `open_pr`: publish the branch, then open a pull request from it.
    OpenPr,
}

/// What the host recorded about the checkout. Never read from the request or
/// the checkout, both of which the agent controls.
#[derive(Debug, Clone, Copy)]
pub struct BranchRecords<'a> {
    /// The branch the checkout's work is reviewed against.
    pub base: &'a str,
    /// The branches this workspace created on origin: each one's push found
    /// no such branch there first (`workspace::owned_branches`).
    pub owned: &'a [String],
    /// Head branches of the checkout's PRs that are open, as last recorded
    /// (`workspace::open_pr_branches`). Includes adopted PRs, so it proves
    /// review is pending, never ownership.
    pub open_pr_heads: &'a [String],
}

/// Trimmed and case-insensitive: on the target filesystem `Fix/X` and `fix/x`
/// name the same loose ref.
pub(crate) fn same_branch(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
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

    /// The whole branch policy for `git_push` and `open_pr`, in order:
    ///
    /// 1. A denied grant publishes nothing.
    /// 2. A protected trunk is never published.
    /// 3. The recorded base is refused, with one exception: the lower branch of
    ///    the workspace's own stack takes a plain `git_push`, so the lower PR
    ///    can get follow-up commits. That needs both records: the workspace
    ///    created the branch (who may push), and it heads an open PR of the
    ///    checkout (where the commits get reviewed — without one they would land
    ///    on the base unreviewed). Never a force (it would rewrite the base of
    ///    the PR stacked on it) and never `open_pr` (which pushes before it
    ///    creates the PR).
    /// 4. SECURITY: force only on a branch this workspace created. A
    ///    `--force-with-lease` lease passes for any branch the agent just
    ///    fetched, so the lease alone authorizes nothing.
    pub fn refuses_publish(
        self,
        publication: Publication,
        branch: &str,
        records: BranchRecords<'_>,
    ) -> Option<String> {
        if let Publish::Denied(why) = self.publish {
            return Some(why.to_string());
        }
        let branch = branch.trim();
        let owned = records.owned.iter().any(|own| same_branch(own, branch));
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
        if same_branch(branch, records.base) {
            let under_review = records
                .open_pr_heads
                .iter()
                .any(|head| same_branch(head, branch));
            return match (publication, owned) {
                (_, false) => Some(review_base()),
                (Publication::Push { force: false }, true) if under_review => None,
                (Publication::Push { force: false }, true) => Some(format!(
                    "refusing to publish to '{branch}': it is the branch this work is reviewed \
                     against, and no open pull request of this workspace would review what you \
                     push to it. Push your own branch (e.g. fix/…) and open a pull request instead"
                )),
                (Publication::Push { force: true }, true) => Some(format!(
                    "refusing to force-push '{branch}': this workspace's stacked pull request \
                     targets it, and rewriting it would rewrite that pull request's base. \
                     Push without --force"
                )),
                (Publication::OpenPr, true) => Some(format!(
                    "refusing to open a pull request from '{branch}': this workspace's stacked \
                     pull request targets it. Push follow-up commits to it with git_push instead"
                )),
            };
        }
        if publication == (Publication::Push { force: true }) && !owned {
            return Some(if records.owned.is_empty() {
                format!(
                    "refusing to force-push '{branch}': force is limited to branches this \
                     workspace created on origin, and none is recorded yet. Push without \
                     --force first; force becomes available for a branch that push creates"
                )
            } else {
                format!(
                    "refusing to force-push '{branch}': force is limited to branches this \
                     workspace created on origin ({}). Push without --force to update a \
                     different branch",
                    records.owned.join(", ")
                )
            });
        }
        None
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

    const PUSH: Publication = Publication::Push { force: false };
    const FORCE: Publication = Publication::Push { force: true };
    const OPEN: Publication = Publication::OpenPr;

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// No open PRs: only the stacked-base rule reads them.
    fn records<'a>(base: &'a str, owned: &'a [String]) -> BranchRecords<'a> {
        with_open_prs(base, owned, &[])
    }

    fn with_open_prs<'a>(
        base: &'a str,
        owned: &'a [String],
        open_pr_heads: &'a [String],
    ) -> BranchRecords<'a> {
        BranchRecords {
            base,
            owned,
            open_pr_heads,
        }
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
            .refuses_publish(PUSH, "fix/login", records("main", &none))
            .is_none());

        for blocked in ["main", "master", "Main", " main "] {
            for publication in [PUSH, FORCE, OPEN] {
                assert!(
                    caps.refuses_publish(publication, blocked, records("main", &none))
                        .is_some(),
                    "{blocked:?} must be refused for {publication:?}"
                );
            }
        }
        assert!(caps
            .refuses_publish(PUSH, "release/24", records("release/24", &none))
            .is_some());
        assert!(caps
            .refuses_publish(PUSH, "main", records("release/24", &none))
            .is_some());
        assert!(caps
            .refuses_publish(PUSH, "fix/x", records("release/24", &none))
            .is_none());
    }

    /// A PR stacked on the workspace's own branch makes that branch the
    /// recorded base. The lower PR must still take follow-up commits, but only
    /// as a plain push: a force would rewrite the stacked PR's base, and
    /// `open_pr` pushes before it creates anything.
    #[test]
    fn an_owned_base_takes_a_plain_push_and_nothing_else() {
        let caps = AgentCaps::interactive();
        let own = owned(&["feat/backend", "feat/ui"]);
        let open = owned(&["feat/backend", "feat/ui"]);
        let stacked = with_open_prs("feat/backend", &own, &open);
        assert!(caps
            .refuses_publish(PUSH, "feat/backend", stacked)
            .is_none());
        assert!(caps
            .refuses_publish(PUSH, " Feat/Backend ", stacked)
            .is_none());

        let why = caps
            .refuses_publish(FORCE, "feat/backend", stacked)
            .expect("force on the base of a stacked PR must be refused");
        assert!(why.contains("without --force"), "{why}");
        let why = caps
            .refuses_publish(OPEN, "feat/backend", stacked)
            .expect("open_pr from the base of a stacked PR must be refused");
        assert!(why.contains("git_push"), "{why}");

        assert!(
            caps.refuses_publish(PUSH, "release/24", records("release/24", &own))
                .is_some(),
            "a base this workspace did not create stays refused"
        );
        let trunk = owned(&["main"]);
        assert!(
            caps.refuses_publish(PUSH, "main", records("main", &trunk))
                .is_some(),
            "a trunk is refused even if recorded as owned"
        );
        assert!(
            AgentCaps::run_owned()
                .refuses_publish(PUSH, "feat/backend", stacked)
                .is_some(),
            "a denied grant stays denied"
        );
    }

    /// The exception needs both records. Ownership alone would let commits
    /// land on the base unreviewed when the lower branch never got a PR, or its
    /// PR merged or closed (it then drops out of the open heads). An open PR
    /// alone proves nothing about who made the branch: adopted PRs, a
    /// teammate's included, are in the set.
    #[test]
    fn a_base_needs_both_ownership_and_an_open_pr() {
        let caps = AgentCaps::interactive();
        let own = owned(&["feat/backend"]);
        let none = owned(&[]);
        let why = caps
            .refuses_publish(
                PUSH,
                "feat/backend",
                with_open_prs("feat/backend", &own, &none),
            )
            .expect("an owned base with no open PR must be refused");
        assert!(why.contains("no open pull request"), "{why}");

        let teammate = owned(&["teammate/wip"]);
        let why = caps
            .refuses_publish(
                PUSH,
                "teammate/wip",
                with_open_prs("teammate/wip", &own, &teammate),
            )
            .expect("an adopted PR's branch must be refused");
        assert!(why.contains("reviewed against"), "{why}");
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
        let own = owned(&["wf/anything"]);
        for publication in [PUSH, FORCE, OPEN] {
            assert!(caps
                .refuses_publish(publication, "wf/anything", records("main", &own))
                .is_some());
        }
    }

    #[test]
    fn force_is_confined_to_branches_the_workspace_created() {
        let caps = AgentCaps::interactive();
        let own = owned(&["fix/login", "feat/older"]);
        for mine in ["fix/login", " Fix/Login ", "feat/older"] {
            assert!(
                caps.refuses_publish(FORCE, mine, records("main", &own))
                    .is_none(),
                "{mine:?} was created by this workspace"
            );
        }
        // Trunks are refused before force is considered (see above).
        for other in ["develop", "release/24", "teammate/wip"] {
            let why = caps
                .refuses_publish(FORCE, other, records("main", &own))
                .expect("force on a branch the workspace did not create");
            assert!(why.contains("force is limited"), "{why}");
            assert_eq!(
                caps.refuses_publish(PUSH, other, records("release/24", &own))
                    .is_some(),
                other == "release/24",
                "{other:?}: a plain push is only refused on the base"
            );
        }
        let none = owned(&[]);
        let why = caps
            .refuses_publish(FORCE, "fix/login", records("main", &none))
            .expect("force fails closed with nothing recorded");
        assert!(why.contains("none is recorded"), "{why}");
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
            .refuses_publish(PUSH, "pm/notes", records("main", &owned(&[])))
            .is_some());
    }
}

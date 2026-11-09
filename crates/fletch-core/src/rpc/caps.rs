//! What an agent may publish with credentials it never holds.
//!
//! The grant is stamped at spawn and never re-read, so a later policy change
//! cannot widen a running agent. Two checks: [`AgentCaps::refuses`] before any
//! work, and [`AgentCaps::refuses_publish`] once the branch is known (it may
//! come from HEAD, not the request) — the whole branch policy, force included,
//! in one place, so no exception in one rule can slip past another. The facts
//! that policy judges are not stamped: they are read when the push happens
//! (`rpc::git::ledger`), and [`AgentCaps::live_needs`] says which ones a
//! request needs so an ordinary push asks nothing of the network.

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

/// What the host knows about the branch a publish targets, read when the
/// publish happens. Never from the request or the checkout, both of which the
/// agent controls.
#[derive(Debug, Clone)]
pub struct BranchFacts<'a> {
    /// The branch the checkout's work is reviewed against, as recorded now.
    pub base: &'a str,
    /// This workspace created the branch on origin (`worktree_branches`).
    pub owned: bool,
    /// The facts only GitHub and origin can answer, asked when
    /// [`AgentCaps::live_needs`] says the request needs them.
    pub live: LiveFacts,
}

/// Facts asked of GitHub and origin at publish time. `None` / `NotAsked`:
/// not asked, or no answer — either way a rule that needs it refuses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveFacts {
    /// An open PR has the branch as its head: something reviews what lands.
    pub heads_open_pr: Option<bool>,
    /// An open PR has the branch as its base: rewriting it rewrites that PR.
    pub bases_open_pr: Option<bool>,
    /// Whether origin's branch is still the one this workspace made.
    pub lineage: Lineage,
}

/// Whether the branch on origin is still the one this workspace created: its
/// tip descends from the commit this workspace last published there. A branch
/// deleted and recreated by someone else under the same name does not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Lineage {
    #[default]
    NotAsked,
    /// Owned, but recorded before tips were (migration 0060's seed): nothing
    /// to compare against until this workspace pushes the branch again.
    Unrecorded,
    /// Origin holds no such branch: there is nothing to overwrite.
    Absent,
    /// Origin's tip, `at`, contains what this workspace last published — its
    /// own commits, and any others' on top of them.
    Continuous { at: String },
    /// Origin's tip does not contain what this workspace last published:
    /// rewritten or recreated by someone else.
    Broken,
    /// Couldn't tell: origin didn't answer, or its tip was never fetched into
    /// the checkout.
    Unknown,
}

/// Which [`LiveFacts`] a request needs; the rest stay unasked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiveNeeds {
    pub heads_open_pr: bool,
    pub bases_open_pr: bool,
    pub lineage: bool,
}

impl LiveNeeds {
    pub fn any(self) -> bool {
        self.heads_open_pr || self.bases_open_pr || self.lineage
    }
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

    /// The live facts [`Self::refuses_publish`] reads for this request. Only
    /// a force, or a push to the recorded base, of a branch this workspace
    /// created needs any: every other request is decided by the records alone,
    /// so an ordinary push asks nothing of GitHub or origin.
    pub fn live_needs(
        self,
        publication: Publication,
        branch: &str,
        base: &str,
        owned: bool,
    ) -> LiveNeeds {
        if matches!(self.publish, Publish::Denied(_)) || !owned || is_trunk(branch) {
            return LiveNeeds::default();
        }
        match (publication, same_branch(branch, base)) {
            (Publication::Push { force: false }, true) => LiveNeeds {
                heads_open_pr: true,
                lineage: true,
                ..LiveNeeds::default()
            },
            (Publication::Push { force: true }, false) => LiveNeeds {
                bases_open_pr: true,
                lineage: true,
                ..LiveNeeds::default()
            },
            _ => LiveNeeds::default(),
        }
    }

    /// The whole branch policy for `git_push` and `open_pr`, in order:
    ///
    /// 1. A denied grant publishes nothing.
    /// 2. A protected trunk is never published.
    /// 3. The recorded base is refused, with one exception: the lower branch of
    ///    the workspace's own stack takes a plain `git_push`, so the lower PR
    ///    can get follow-up commits. The workspace must have created it, it
    ///    must still be that branch (its lineage), and an open PR must have it
    ///    as head, so something reviews what lands. Never a force (it would
    ///    rewrite the base of the PR stacked on it) and never `open_pr` (which
    ///    pushes before it creates the PR).
    /// 4. SECURITY: force only on a branch this workspace created, still the
    ///    branch it created, and no open PR's base — whoever opened that PR,
    ///    at whatever depth of a stack. The lease alone authorizes nothing: it
    ///    passes for any branch the agent just fetched.
    ///
    /// A live fact this needs but lacks refuses: no answer is never a yes.
    pub fn refuses_publish(
        self,
        publication: Publication,
        branch: &str,
        facts: &BranchFacts<'_>,
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
        if is_trunk(branch) {
            return Some(review_base());
        }
        let live = &facts.live;
        if same_branch(branch, facts.base) {
            return match (publication, facts.owned) {
                (_, false) => Some(review_base()),
                (Publication::Push { force: true }, true) => Some(format!(
                    "refusing to force-push '{branch}': this workspace's stacked pull request \
                     targets it, and rewriting it would rewrite that pull request's base. \
                     Push without --force"
                )),
                (Publication::OpenPr, true) => Some(format!(
                    "refusing to open a pull request from '{branch}': this workspace's stacked \
                     pull request targets it. Push follow-up commits to it with git_push instead"
                )),
                (Publication::Push { force: false }, true) => match live.heads_open_pr {
                    Some(true) => refuses_lineage(branch, &live.lineage, false),
                    Some(false) => Some(format!(
                        "refusing to publish to '{branch}': it is the branch this work is \
                         reviewed against, and no open pull request has it as its head to \
                         review what you push. Push your own branch (e.g. fix/…) and open a pull \
                         request instead"
                    )),
                    None => Some(format!(
                        "refusing to publish to '{branch}': it is the branch this work is \
                         reviewed against, and GitHub could not confirm an open pull request \
                         reviews it; try again shortly"
                    )),
                },
            };
        }
        if publication != (Publication::Push { force: true }) {
            return None;
        }
        if !facts.owned {
            return Some(format!(
                "refusing to force-push '{branch}': force is limited to branches this workspace \
                 created on origin. Push without --force to update a branch it did not create"
            ));
        }
        match live.bases_open_pr {
            Some(false) => refuses_lineage(branch, &live.lineage, true),
            Some(true) => Some(format!(
                "refusing to force-push '{branch}': an open pull request is based on it, and \
                 rewriting it would rewrite that pull request's base. Push without --force"
            )),
            None => Some(format!(
                "refusing to force-push '{branch}': GitHub could not confirm that no open pull \
                 request is based on it; try again shortly"
            )),
        }
    }
}

/// Rule 3's and rule 4's lineage check: the branch on origin must still be the
/// one this workspace created. A force may also go to a branch recorded before
/// tips were (it could be forced before them) or one origin no longer holds
/// (nothing is overwritten); a stacked-base push needs the recorded lineage.
fn refuses_lineage(branch: &str, lineage: &Lineage, force: bool) -> Option<String> {
    match lineage {
        Lineage::Continuous { .. } => None,
        Lineage::Unrecorded | Lineage::Absent if force => None,
        Lineage::Unrecorded => Some(format!(
            "refusing to publish to '{branch}': it is the branch this work is reviewed against, \
             and this workspace has no record of what it last published there"
        )),
        Lineage::Absent => Some(format!(
            "refusing to publish to '{branch}': it is the branch this work is reviewed against, \
             and origin no longer has it"
        )),
        Lineage::Broken => Some(format!(
            "refusing to {} '{branch}': origin's '{branch}' no longer contains what this \
             workspace last pushed there — it was rewritten or recreated by someone else, so \
             it is not this workspace's branch any more. Push your work to a new branch instead",
            if force { "force-push" } else { "publish to" }
        )),
        Lineage::Unknown | Lineage::NotAsked => Some(format!(
            "refusing to {} '{branch}': could not confirm origin's '{branch}' is still the \
             branch this workspace created. Fetch it (git_fetch) and try again",
            if force { "force-push" } else { "publish to" }
        )),
    }
}

fn is_trunk(branch: &str) -> bool {
    PROTECTED_TRUNKS
        .iter()
        .any(|trunk| same_branch(branch, trunk))
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

    /// Facts with nothing live: what every request that needs no live fact
    /// is decided on.
    fn facts(base: &str, owned: bool) -> BranchFacts<'_> {
        BranchFacts {
            base,
            owned,
            live: LiveFacts::default(),
        }
    }

    fn continuous() -> Lineage {
        Lineage::Continuous { at: "abc".into() }
    }

    /// A stacked base as it is when everything holds: owned, still this
    /// workspace's branch, and the head of an open PR.
    fn stacked_base(base: &str) -> BranchFacts<'_> {
        BranchFacts {
            live: LiveFacts {
                heads_open_pr: Some(true),
                lineage: continuous(),
                ..LiveFacts::default()
            },
            ..facts(base, true)
        }
    }

    /// An owned branch that is nobody's base, as it is when everything holds.
    fn forceable(base: &str) -> BranchFacts<'_> {
        BranchFacts {
            live: LiveFacts {
                bases_open_pr: Some(false),
                lineage: continuous(),
                ..LiveFacts::default()
            },
            ..facts(base, true)
        }
    }

    #[test]
    fn an_ordinary_agent_may_not_publish_the_review_base() {
        let caps = AgentCaps::interactive();
        assert!(
            caps.refuses("git_push").is_none(),
            "the op itself is allowed"
        );
        assert!(caps
            .refuses_publish(PUSH, "fix/login", &facts("main", false))
            .is_none());

        for blocked in ["main", "master", "Main", " main "] {
            for publication in [PUSH, FORCE, OPEN] {
                assert!(
                    caps.refuses_publish(publication, blocked, &forceable("main"))
                        .is_some(),
                    "{blocked:?} must be refused for {publication:?}, owned or not"
                );
            }
        }
        assert!(caps
            .refuses_publish(PUSH, "release/24", &facts("release/24", false))
            .is_some());
        assert!(caps
            .refuses_publish(PUSH, "main", &facts("release/24", false))
            .is_some());
        assert!(caps
            .refuses_publish(PUSH, "fix/x", &facts("release/24", false))
            .is_none());
    }

    /// The lower branch of the workspace's own stack takes a plain push while
    /// everything holds, and never a force or `open_pr`.
    #[test]
    fn a_stacked_base_takes_a_plain_push_and_nothing_else() {
        let caps = AgentCaps::interactive();
        let base = stacked_base("feat/backend");
        assert!(caps.refuses_publish(PUSH, "feat/backend", &base).is_none());
        assert!(caps
            .refuses_publish(PUSH, " Feat/Backend ", &base)
            .is_none());
        let why = caps
            .refuses_publish(FORCE, "feat/backend", &base)
            .expect("force on the base of a stacked PR");
        assert!(why.contains("without --force"), "{why}");
        let why = caps
            .refuses_publish(OPEN, "feat/backend", &base)
            .expect("open_pr from the base of a stacked PR");
        assert!(why.contains("git_push"), "{why}");
        assert!(AgentCaps::run_owned()
            .refuses_publish(PUSH, "feat/backend", &base)
            .is_some());
    }

    /// Every condition of the stacked-base push is load-bearing: who made the
    /// branch, whether it is still that branch, and whether a PR reviews it.
    #[test]
    fn a_stacked_base_push_needs_every_fact() {
        let caps = AgentCaps::interactive();
        let refused = |f: BranchFacts<'_>, expect: &str| {
            let why = caps
                .refuses_publish(PUSH, "feat/backend", &f)
                .unwrap_or_else(|| panic!("must be refused: {f:?}"));
            assert!(why.contains(expect), "{why}");
        };
        let base = || stacked_base("feat/backend");
        refused(
            BranchFacts {
                owned: false,
                ..base()
            },
            "reviewed against",
        );
        // Never opened, merged or closed: no open PR has it as head.
        let mut f = base();
        f.live.heads_open_pr = Some(false);
        refused(f, "no open pull request");
        let mut f = base();
        f.live.heads_open_pr = None;
        refused(f, "could not confirm");
        for (lineage, expect) in [
            (Lineage::Broken, "recreated"),
            (Lineage::Unknown, "Fetch it"),
            (Lineage::NotAsked, "Fetch it"),
            (Lineage::Unrecorded, "no record"),
            (Lineage::Absent, "no longer has it"),
        ] {
            let mut f = base();
            f.live.lineage = lineage;
            refused(f, expect);
        }
    }

    /// Force is decided by GitHub's own view of which PRs a branch is the base
    /// of, not the one recorded base: in a stack A <- B <- C the recorded base
    /// is B, and A must stay unforceable while B is based on it.
    #[test]
    fn force_needs_an_owned_branch_that_no_open_pr_is_based_on() {
        let caps = AgentCaps::interactive();
        assert!(caps
            .refuses_publish(FORCE, "feat/a", &forceable("feat/b"))
            .is_none());
        assert!(caps
            .refuses_publish(FORCE, " Feat/A ", &forceable("feat/b"))
            .is_none());

        let mut a_under_b = forceable("feat/b");
        a_under_b.live.bases_open_pr = Some(true);
        let why = caps
            .refuses_publish(FORCE, "feat/a", &a_under_b)
            .expect("A is B's base");
        assert!(why.contains("based on it"), "{why}");

        let mut unanswered = forceable("feat/b");
        unanswered.live.bases_open_pr = None;
        let why = caps
            .refuses_publish(FORCE, "feat/a", &unanswered)
            .expect("no answer is never a yes");
        assert!(why.contains("could not confirm"), "{why}");

        for other in ["develop", "release/24", "teammate/wip"] {
            let why = caps
                .refuses_publish(
                    FORCE,
                    other,
                    &BranchFacts {
                        owned: false,
                        ..forceable("main")
                    },
                )
                .expect("force on a branch the workspace did not create");
            assert!(why.contains("force is limited"), "{why}");
            assert!(
                caps.refuses_publish(PUSH, other, &facts("main", false))
                    .is_none(),
                "{other:?}: a plain push is not refused"
            );
        }
    }

    /// The branch on origin must still be the one this workspace created. A
    /// branch recorded before tips were keeps the force it had; one origin
    /// no longer holds has nothing to overwrite.
    #[test]
    fn force_needs_the_branch_this_workspace_created() {
        let caps = AgentCaps::interactive();
        for (lineage, allowed) in [
            (continuous(), true),
            (Lineage::Unrecorded, true),
            (Lineage::Absent, true),
            (Lineage::Broken, false),
            (Lineage::Unknown, false),
            (Lineage::NotAsked, false),
        ] {
            let mut f = forceable("main");
            f.live.lineage = lineage.clone();
            assert_eq!(
                caps.refuses_publish(FORCE, "fix/login", &f).is_none(),
                allowed,
                "{lineage:?}"
            );
        }
    }

    /// The live facts are asked only where a rule reads them: an ordinary
    /// push, a non-owned branch and a refused request ask nothing.
    #[test]
    fn live_facts_are_asked_only_where_a_rule_reads_them() {
        let caps = AgentCaps::interactive();
        let none = LiveNeeds::default();
        assert_eq!(caps.live_needs(PUSH, "fix/x", "main", true), none);
        assert_eq!(caps.live_needs(FORCE, "fix/x", "main", false), none);
        assert_eq!(caps.live_needs(FORCE, "main", "main", true), none);
        assert_eq!(caps.live_needs(OPEN, "feat/a", "feat/a", true), none);
        assert_eq!(caps.live_needs(FORCE, "feat/a", "feat/a", true), none);
        assert_eq!(
            AgentCaps::run_owned().live_needs(FORCE, "fix/x", "main", true),
            none
        );
        assert_eq!(
            caps.live_needs(PUSH, "feat/a", "feat/a", true),
            LiveNeeds {
                heads_open_pr: true,
                lineage: true,
                ..none
            }
        );
        assert_eq!(
            caps.live_needs(FORCE, "fix/x", "main", true),
            LiveNeeds {
                bases_open_pr: true,
                lineage: true,
                ..none
            }
        );
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
        for publication in [PUSH, FORCE, OPEN] {
            assert!(caps
                .refuses_publish(publication, "wf/anything", &forceable("main"))
                .is_some());
        }
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
            .refuses_publish(PUSH, "pm/notes", &facts("main", false))
            .is_some());
    }
}

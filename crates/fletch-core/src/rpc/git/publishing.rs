use std::path::Path;

use serde_json::{json, Value};

use super::args::{arg_bool, arg_branch, arg_branch_named, refuse_option_like};
use super::ledger::PrSide;
use super::targets::{approval_repo, with_repo, Target};
use super::{GitDispatcher, EVENT_BRANCH_CREATED, EVENT_PR_OPENED};
use crate::rpc::caps::{same_branch, BranchFacts, Lineage, LiveFacts, Publication};
use crate::rpc::{approval, Response, RpcEvent};
use crate::workspace::OwnedBranch;

impl GitDispatcher {
    /// Fails closed when the gate is on but there is no window to ask through.
    pub(super) async fn refuse_unless_publish_approved(
        &self,
        op: &str,
        repo: Option<&str>,
        detail: &str,
    ) -> Option<String> {
        if !approval::enabled() {
            return None;
        }
        let Some((sink, agent_id)) = &self.approval else {
            return Some(format!(
                "not publishing ({detail}): publish confirmation is enabled but this \
                 session has no window to ask through"
            ));
        };
        // `None` for the primary: the frontend keys it unsuffixed, and a mismatched
        // key strands an unattended push on a prompt nobody answers.
        let repo = approval_repo(repo, self.default_subdir.as_deref());
        approval::refuse_unless_approved(sink.as_ref(), agent_id, op, repo, detail).await
    }

    pub(super) async fn open_pr(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return (resp, Vec::new()),
        };
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let body_arg = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
        // Live issue lookup, not a spawn-time snapshot: a later composer pick must
        // reach the trailer. Idempotent.
        let closes = self.live_closes(&t);
        let body_owned = crate::github::with_closes_trailer(body_arg, closes.as_deref());
        let body = body_owned.as_str();
        let requested = arg_branch(args);
        // Resolved before the head branch is materialized so a malformed base fails
        // without a stray branch. The effective base is reported on `EVENT_PR_OPENED`,
        // which rewrites the checkout's recorded base.
        let requested_base = arg_branch_named(args, "base");
        let base = requested_base
            .clone()
            .unwrap_or_else(|| t.base_branch.clone());
        if let Some(resp) = refuse_option_like(id, "open_pr", "base", &base) {
            return (resp, Vec::new());
        }
        // Only the requested base is probed, so a typo can't leave a pushed branch
        // with no PR. `None` means "couldn't ask" and must not block.
        if requested_base.is_some()
            && crate::git::remote_branch_exists(&t.cwd, &t.source_repo, &base).await == Some(false)
        {
            return (
                Response::err(
                    id,
                    format!(
                        "open_pr: base branch {base:?} does not exist on origin — \
                         pass an existing branch as `args.base`"
                    ),
                ),
                Vec::new(),
            );
        }

        let current = match crate::git::current_branch(&t.cwd).await {
            Ok(b) => b,
            Err(e) => return (Response::err(id, format!("open_pr: {e}")), Vec::new()),
        };

        let mut effects = Vec::new();
        // `args.head` names an existing local branch other than the checked-out
        // one — a stack's next PR is opened without checking each branch out.
        // Nothing is materialized for it: a head that isn't there fails the
        // identity read below, which names it.
        let head = arg_branch_named(args, "head");
        // An explicit head that names the checked-out branch is the agent's
        // own branch, spelled out; only a head that differs is another one.
        let publishes_other_head = head.is_some() && head != current;
        let branch = match (head, current, requested) {
            (Some(head), _, _) => head,
            (None, Some(cur), None) => cur,
            (None, Some(cur), Some(req)) if req == cur => cur,
            (None, Some(_), Some(req)) => match create_branch(&t, &req, &mut effects).await {
                Ok(name) => name,
                Err(e) => return (Response::err(id, format!("open_pr: {e}")), effects),
            },
            (None, None, req) => {
                let desired = req.unwrap_or_else(|| fallback_branch(title));
                match create_branch(&t, &desired, &mut effects).await {
                    Ok(name) => name,
                    Err(e) => return (Response::err(id, format!("open_pr: {e}")), effects),
                }
            }
        };

        if let Some(resp) = refuse_option_like(id, "open_pr", "branch", &branch) {
            return (resp, effects);
        }
        // Deliberately the *recorded* base, never `base`: reading it from the
        // request would let an agent name a decoy base and publish over the
        // real one.
        let cleared = match self.clear_publish(&t, Publication::OpenPr, &branch).await {
            Ok(cleared) => cleared,
            Err(why) => return (Response::err(id, why), effects),
        };
        // Before the prompt, so nobody approves a push the host then refuses…
        if let Err(resp) = checked_tip(id, &t.cwd, &branch).await {
            return (resp, effects);
        }
        if let Some(why) = self
            .refuse_unless_publish_approved(
                "open_pr",
                t.subdir.as_deref(),
                &format!("open a pull request from {branch} into {base}"),
            )
            .await
        {
            return (Response::err(id, why), effects);
        }
        // …and again after it, publishing the very commit it read: the branch
        // may move while the prompt is open, or between here and the push.
        let tip = match checked_tip(id, &t.cwd, &branch).await {
            Ok(tip) => tip,
            Err(resp) => return (resp, effects),
        };
        let born = cleared.owned.is_none() && creates_on_origin(&t, &branch).await;
        let published = match t.publish(&branch, &tip, None).await {
            Ok(published) => published,
            Err(e) => {
                return (
                    Response::err(id, format!("open_pr push failed: {e}")),
                    effects,
                )
            }
        };
        self.record_publish(&t, &branch, &cleared, false, born, &published.tip)
            .await;
        if born && !publishes_other_head {
            note_checkout_branch(&t, &branch, &mut effects);
        }
        // By the branch just published, not the checkout's HEAD: with
        // `args.head` the two differ. In the user's repository, like every
        // other GitHub op; the checkout only lends its last commit to an
        // empty title.
        match crate::github::pr_create_in(&t.source_repo, &t.cwd, &branch, title, body, &base).await
        {
            Ok(pr) => {
                crate::telemetry::track("pr_opened", json!({ "source": "agent_rpc" }));
                // Enough to log the PR into its checkout's set the moment it
                // binds, before any fetch has filled in its snapshot.
                effects.push(RpcEvent::named(
                    EVENT_PR_OPENED,
                    with_repo(
                        json!({
                            "number": pr.number,
                            "base": base,
                            "branch": branch,
                            "url": pr.url,
                            "title": pr.title,
                        }),
                        &t.subdir,
                    ),
                ));
                (Response::ok(id, 0, pr.url, String::new()), effects)
            }
            Err(e) => (Response::err(id, format!("open_pr: {e}")), effects),
        }
    }

    pub(super) async fn git_push(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return (resp, Vec::new()),
        };
        let current = match crate::git::current_branch(&t.cwd).await {
            Ok(b) => b,
            Err(e) => return (Response::err(id, format!("git_push: {e}")), Vec::new()),
        };

        let mut effects = Vec::new();
        let branch = match current {
            Some(cur) => cur,
            None => match arg_branch(args) {
                Some(req) => match create_branch(&t, &req, &mut effects).await {
                    Ok(name) => name,
                    Err(e) => return (Response::err(id, format!("git_push: {e}")), effects),
                },
                None => {
                    return (
                        Response::err(
                            id,
                            "git_push: HEAD is detached — pass `args.branch` (e.g. \"fix/…\") to create the branch",
                        ),
                        effects,
                    )
                }
            },
        };

        if let Some(resp) = refuse_option_like(id, "git_push", "branch", &branch) {
            return (resp, effects);
        }
        // Lease-based, so a stale local view can't clobber remote work it hasn't seen.
        let force = arg_bool(args, "force");
        let publication = Publication::Push { force };
        // Earliest point `branch` is known. The whole branch policy, force
        // included, runs here: every refusal the host can make on its own comes
        // before the prompt, so nobody approves a push the host then refuses, and
        // before anything below touches the checkout — publishing may rewrite the
        // unpublished commits (signing), and a refused request must leave no trace.
        let cleared = match self.clear_publish(&t, publication, &branch).await {
            Ok(cleared) => cleared,
            Err(why) => return (Response::err(id, why), effects),
        };
        if let Err(resp) = checked_tip(id, &t.cwd, &branch).await {
            return (resp, effects);
        }
        if let Some(why) = self
            .refuse_unless_publish_approved(
                "git_push",
                t.subdir.as_deref(),
                &format!("push {branch}"),
            )
            .await
        {
            return (Response::err(id, why), effects);
        }
        // Checked again after the prompt, and the push publishes the very commit
        // it read: the branch may move while the prompt is open, or between
        // here and the push.
        let tip = match checked_tip(id, &t.cwd, &branch).await {
            Ok(tip) => tip,
            Err(resp) => return (resp, effects),
        };
        // The live facts too: a PR can merge, and a branch be rewritten, while
        // the prompt is open. Only when the policy read any, so an ordinary
        // push asks nothing twice.
        let cleared = if cleared.live_checked {
            match self.clear_publish(&t, publication, &branch).await {
                Ok(cleared) => cleared,
                Err(why) => return (Response::err(id, why), effects),
            }
        } else {
            cleared
        };
        let born = cleared.owned.is_none() && creates_on_origin(&t, &branch).await;
        // A force leases against the tip its lineage was checked at, so what
        // lands on origin after the check fails the push instead of being lost.
        let lease = force.then_some(cleared.expect.as_deref());
        match t.publish(&branch, &tip, lease).await {
            Ok(published) => {
                self.record_publish(&t, &branch, &cleared, force, born, &published.tip)
                    .await;
                if born {
                    note_checkout_branch(&t, &branch, &mut effects);
                }
                (
                    Response::ok(id, 0, published.summary, String::new()),
                    effects,
                )
            }
            Err(e) => (Response::err(id, e.to_string()), effects),
        }
    }

    /// The publish policy (`caps::refuses_publish`) over facts read now: the
    /// recorded base and the branches this workspace created (the ledger),
    /// plus whatever GitHub and origin must answer for this request
    /// (`caps::live_needs`). Never a spawn-time copy: within a session a PR
    /// merges, a new PR moves the base, a force moves the tip.
    async fn clear_publish(
        &self,
        t: &Target,
        publication: Publication,
        branch: &str,
    ) -> std::result::Result<Cleared, String> {
        let subdir = t.subdir.as_deref();
        let base = self
            .ledger
            .base(subdir)
            .unwrap_or_else(|| t.base_branch.clone());
        let owned = self
            .ledger
            .owned(subdir)
            .into_iter()
            .find(|own| same_branch(&own.name, branch));
        let needs = self
            .caps
            .live_needs(publication, branch, &base, owned.is_some());
        let mut live = LiveFacts::default();
        if needs.heads_open_pr {
            live.heads_open_pr = self
                .pr_index
                .has_open_pr(&t.source_repo, PrSide::Head, branch)
                .await;
        }
        if needs.bases_open_pr {
            live.bases_open_pr = self
                .pr_index
                .has_open_pr(&t.source_repo, PrSide::Base, branch)
                .await;
        }
        if needs.lineage {
            let recorded = owned.as_ref().and_then(|own| own.tip.as_deref());
            live.lineage = lineage(t, branch, recorded).await;
        }
        let facts = BranchFacts {
            base: &base,
            owned: owned.is_some(),
            live,
        };
        if let Some(why) = self.caps.refuses_publish(publication, branch, &facts) {
            return Err(why);
        }
        let expect = match facts.live.lineage {
            Lineage::Continuous { at } => Some(at),
            _ => None,
        };
        Ok(Cleared {
            owned,
            expect,
            live_checked: needs.any(),
        })
    }

    /// Record a publish that landed `tip` on `branch` in the ledger. A push
    /// that created the branch makes it this workspace's own; on a branch
    /// already owned, the tip moves only when the push continued the branch
    /// this workspace made — a force, whose lineage was checked, or a plain
    /// push descending from the last recorded tip. A plain push onto a
    /// same-named branch someone else recreated (allowed, as to any branch)
    /// must not adopt it, or the next force would pass its lineage check.
    async fn record_publish(
        &self,
        t: &Target,
        branch: &str,
        cleared: &Cleared,
        force: bool,
        born: bool,
        tip: &str,
    ) {
        let subdir = t.subdir.as_deref();
        if born {
            self.ledger.record_push(subdir, branch, tip, true);
            return;
        }
        let Some(owned) = &cleared.owned else {
            return;
        };
        let continues = force
            || match owned.tip.as_deref() {
                None => true,
                Some(last) => crate::git::descends_from(&t.cwd, tip, last).await == Some(true),
            };
        if continues {
            self.ledger.record_push(subdir, branch, tip, false);
        }
    }
}

/// What [`GitDispatcher::clear_publish`] found for a request it allowed.
struct Cleared {
    /// The ledger's record of the branch, when this workspace created it.
    owned: Option<OwnedBranch>,
    /// For a force: the tip origin must still hold when the push lands — the
    /// one the lineage check read. `None` leases against whatever origin
    /// holds then (a branch recorded before tips were, or absent).
    expect: Option<String>,
    /// The policy read live facts, which can change while an approval prompt
    /// is open.
    live_checked: bool,
}

/// Whether origin's `branch` is still the one this workspace created: its tip
/// contains `recorded`, the commit this workspace last published there.
async fn lineage(t: &Target, branch: &str, recorded: Option<&str>) -> Lineage {
    let Some(recorded) = recorded else {
        return Lineage::Unrecorded;
    };
    match crate::git::remote_branch_tip(&t.cwd, &t.source_repo, branch).await {
        None => Lineage::Unknown,
        Some(None) => Lineage::Absent,
        Some(Some(at)) => match crate::git::descends_from(&t.cwd, &at, recorded).await {
            Some(true) => Lineage::Continuous { at },
            Some(false) => Lineage::Broken,
            None => Lineage::Unknown,
        },
    }
}

/// Whether the push about to happen is what creates `branch` on origin — the
/// one fact that makes a branch this workspace's own. Asked of origin itself,
/// through the host's transport, never read from the checkout: a name the host
/// materialized is only collision-free against the checkout's remote-tracking
/// refs, which the agent can edit, so materializing proves nothing. A probe
/// that can't answer records nothing, so a flaky network never widens what is
/// owned.
async fn creates_on_origin(t: &Target, branch: &str) -> bool {
    crate::git::remote_branch_exists(&t.cwd, &t.source_repo, branch).await == Some(false)
}

/// After a push created the checked-out `branch` on origin: unless the host
/// materialized it (and said so), it was made with plain `git checkout -b`,
/// and becomes the checkout's recorded branch now. A stack's other head
/// published through `args.head` is not where the agent works.
fn note_checkout_branch(t: &Target, branch: &str, effects: &mut Vec<RpcEvent>) {
    let materialized = effects.iter().any(|e| {
        let RpcEvent::Named { name, .. } = e;
        name == EVENT_BRANCH_CREATED
    });
    if !materialized {
        effects.push(RpcEvent::named(
            EVENT_BRANCH_CREATED,
            with_repo(json!({ "branch": branch }), &t.subdir),
        ));
    }
}

impl Target {
    /// Publish `tip` to `branch` through the host publication layer, which
    /// signs when the user's repository does (`git::publish`). `force` is
    /// `Some(expect)` for a lease-guarded force: `expect` the tip origin must
    /// still hold, or `None` for whatever it holds when the push runs.
    async fn publish(
        &self,
        branch: &str,
        tip: &str,
        force: Option<Option<&str>>,
    ) -> crate::error::Result<crate::git::Published> {
        crate::git::publish(
            &crate::git::Publication {
                checkout: &self.cwd,
                source_repo: &self.source_repo,
                branch,
                force: force.is_some(),
                expect: force.flatten(),
            },
            crate::git::Tip::Commit(tip),
        )
        .await
    }
}

/// The tip of `branch` once its new commits are known to be the checkout's own
/// (see `git::identity`) — the commit to push, so nothing that lands after the
/// check is published. A foreign committer, or an unreadable branch, is the
/// refusal to answer with: better than pushing blind.
async fn checked_tip(id: &str, cwd: &Path, branch: &str) -> std::result::Result<String, Response> {
    match crate::git::check_identity(cwd, branch).await {
        Ok(crate::git::Identity::Own { tip }) => Ok(tip),
        Ok(crate::git::Identity::Foreign(why)) => Err(Response::err(id, why)),
        Err(e) => Err(Response::err(
            id,
            format!("could not read the commits '{branch}' would publish: {e}"),
        )),
    }
}

async fn create_branch(
    t: &Target,
    desired: &str,
    effects: &mut Vec<RpcEvent>,
) -> std::result::Result<String, String> {
    let name = materialize_branch(&t.cwd, desired).await?;
    effects.push(RpcEvent::named(
        EVENT_BRANCH_CREATED,
        with_repo(json!({ "branch": name }), &t.subdir),
    ));
    Ok(name)
}

/// The one place an agent's branch is born, so the user's branch prefix is
/// applied here and nowhere else.
async fn materialize_branch(checkout: &Path, desired: &str) -> std::result::Result<String, String> {
    let desired = crate::publish_prefs::apply_branch_prefix(desired);
    crate::git::checkout_new_unique_branch(checkout, &desired)
        .await
        .map_err(|e| e.to_string())
}

fn fallback_branch(title: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "chore/update".to_string()
    } else {
        format!("chore/{slug}")
    }
}

#[cfg(test)]
#[path = "tests/open_pr.rs"]
mod open_pr_tests;

#[cfg(test)]
#[path = "tests/push.rs"]
mod push_tests;

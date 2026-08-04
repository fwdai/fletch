//! Code review on the board: the read behind an `in_review` card's merge gate,
//! and the two things the user can do about it from there.
//!
//! # Why this is not `gitSync`
//!
//! Every other PR-detail poll in the app is keyed by *checkout* — an agent's
//! workspace, `checkoutKey(agent_id, subdir)` — because that is what the Git
//! panel renders. A roadmap item has no checkout: the run that built it worked
//! in a disposable clone under `~/.fletch/runs/<id>`, and by the time the item
//! is `in_review` that clone may be gone. What survives is the pair
//! ([`merge_sweep`](super::merge_sweep)'s pair): the project's primary repo,
//! which shares the origin remote, and the item's `pr_number`. So the reads are
//! addressed the same way the sweep addresses its own, and the frontend keys the
//! answers by item id rather than by checkout.
//!
//! # What it adds over the sweep
//!
//! The sweep asks one question — did it merge — and it asks it host-side,
//! forever, because the queue must drain with the window shut. This asks the
//! *review* questions (is CI green, does it conflict, is anyone waiting on a
//! reply), which only matter while someone is looking at the board. So this half
//! is pulled by the frontend on a modest cadence and disappears when the board
//! unmounts; nothing here runs in the background.
//!
//! Both reads are thin wrappers over the fetchers the Git panel already uses
//! ([`crate::github::pr_checks_live`], [`crate::github::pr_threads_number`]),
//! and both degrade the way every GitHub read in this app degrades: a missing
//! token, an unresolvable remote, a rate-limit pause, a deleted PR all yield
//! `None` rather than an error. A board must never show an error bar because
//! GitHub was briefly unreachable.

use std::path::PathBuf;

use serde::Serialize;

use super::drainer;
use super::types::{ItemStatus, RoadmapItem};
use super::Db;
use crate::github::{PrChecks, PrComments};

/// One `in_review` item's live review state, as the card renders it.
///
/// Every field is optional and independently degradable: the checks read is
/// REST, the threads read is GraphQL, and the refs read is a third conditional
/// REST hit — one failing must not blank the other two. `None` means "nothing to
/// say this round", never "zero" (see [`crate::github::pr_checks_live`]).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ItemReview {
    pub checks: Option<PrChecks>,
    pub comments: Option<PrComments>,
    /// The PR's branch, so "Fix review feedback" can put an agent *on* the PR
    /// rather than on a fresh branch off the project's base.
    pub head_ref: Option<String>,
    /// The PR's base branch, so the gate chip can name what it is behind or
    /// conflicting with instead of saying "base".
    pub base_ref: Option<String>,
}

/// The PR number this item's card can poll, or `None`.
///
/// The same rule [`merge_sweep::pollable`](super::merge_sweep::pollable) applies,
/// for the same reason: only an `in_review` item is under review, and a URL
/// without a number is not something to guess a number out of. Narrowing to
/// `u32` here (not at the call site) keeps a nonsense stored number from
/// reaching GitHub as a wrapped one.
pub(crate) fn watchable(item: &RoadmapItem) -> Option<u32> {
    if item.status != ItemStatus::InReview {
        return None;
    }
    u32::try_from(item.pr_number?).ok()
}

/// The (repo, number) pair a card's reads are addressed by, or `None` when this
/// item has nothing to read: it is not under review, it has no PR number, or its
/// project has no repo to resolve `owner/repo` from.
///
/// The repo is the project's *primary* checkout, exactly as the merge sweep
/// resolves it (`drainer::primary_repo_path`) and for the same reason: the run's
/// own clone is scratch that may already be cleaned up, while the project's
/// checkout shares the origin remote and outlives every run in it.
pub(crate) fn target(db: &Db, item_id: &str) -> Option<(PathBuf, u32)> {
    let conn = db.lock();
    let item = super::store::get(&conn, item_id).ok().flatten()?;
    let number = watchable(&item)?;
    let repo = drainer::primary_repo_path(&conn, &item.project_id)?;
    Some((PathBuf::from(repo), number))
}

/// Fetch everything a card wants about one PR. Never fails: each half is
/// independently degraded to `None`, so a GraphQL point budget that ran out
/// still leaves the CI rollup on screen (and vice versa).
pub(crate) async fn fetch(repo: &std::path::Path, number: u32) -> ItemReview {
    let (checks, comments, refs) = tokio::join!(
        crate::github::pr_checks_live(repo, None, number),
        crate::github::pr_threads_number(repo, None, number),
        crate::github::pr_refs_live(repo, number),
    );
    let refs = degrade(refs, number, "PR refs");
    ItemReview {
        checks: degrade(checks, number, "PR checks"),
        comments: degrade(comments, number, "PR review threads"),
        head_ref: refs.as_ref().map(|r| r.head.clone()),
        base_ref: refs.map(|r| r.base),
    }
}

/// Collapse a GitHub read's `Result<Option<T>>` to `Option<T>`: a hard error is
/// logged at debug and treated exactly like the read's own "no answer" — the
/// board is a view, and a view has nothing useful to do with a transport error.
fn degrade<T>(read: crate::error::Result<Option<T>>, number: u32, what: &str) -> Option<T> {
    match read {
        Ok(value) => value,
        Err(e) => {
            tracing::debug!(pr = number, error = %e, "roadmap review: {what} read failed");
            None
        }
    }
}

/// The history line "Fix review feedback" writes. The count is the point: the
/// trail should say how much feedback went out, so a later reader can tell a
/// one-comment nit from a rewrite. Zero threads never reaches here (the card
/// only offers the action when there are some), but the singular/plural split
/// has to be right for the one-thread case, which is the common one.
pub(crate) fn feedback_detail(threads: usize) -> String {
    match threads {
        1 => "Sent 1 review thread to an agent".to_string(),
        n => format!("Sent {n} review threads to an agent"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roadmap::types::{Horizon, ItemSource};

    fn item(status: ItemStatus, pr_number: Option<i64>) -> RoadmapItem {
        RoadmapItem {
            id: "i1".into(),
            project_id: "p1".into(),
            code: "FLT-1".into(),
            parent_id: None,
            title: "t".into(),
            why: String::new(),
            horizon: Horizon::Now,
            status,
            rank: 1.0,
            area: None,
            source: ItemSource::User,
            accept: Vec::new(),
            deps: Vec::new(),
            agent_id: None,
            workflow_def_id: None,
            run_id: None,
            pr_url: pr_number.map(|n| format!("https://github.com/o/r/pull/{n}")),
            pr_number,
            created_at: 1,
            updated_at: 1,
        }
    }

    /// The card's reads are scoped exactly like the sweep's watch list: only an
    /// item under review, and only one whose PR number we actually have.
    #[test]
    fn only_in_review_items_with_a_number_are_watchable() {
        assert_eq!(watchable(&item(ItemStatus::InReview, Some(42))), Some(42));
        assert_eq!(watchable(&item(ItemStatus::InReview, None)), None);
        for status in [
            ItemStatus::Proposed,
            ItemStatus::Open,
            ItemStatus::Queued,
            ItemStatus::Active,
            ItemStatus::Done,
        ] {
            assert_eq!(
                watchable(&item(status, Some(42))),
                None,
                "{} is not under review",
                status.as_str()
            );
        }
    }

    /// A stored number that can't be a PR number is not sent to GitHub as a
    /// wrapped one — it reads as "nothing to poll", like a missing number.
    #[test]
    fn a_nonsense_number_is_not_watchable() {
        assert_eq!(watchable(&item(ItemStatus::InReview, Some(-1))), None);
        assert_eq!(
            watchable(&item(ItemStatus::InReview, Some(i64::from(u32::MAX) + 1))),
            None
        );
    }

    /// A transport failure is indistinguishable from the read's own "no answer":
    /// both leave the card with its previous value rather than an error.
    #[test]
    fn a_failed_read_degrades_to_no_answer() {
        let ok: crate::error::Result<Option<u8>> = Ok(Some(7));
        assert_eq!(degrade(ok, 1, "x"), Some(7));
        let empty: crate::error::Result<Option<u8>> = Ok(None);
        assert_eq!(degrade(empty, 1, "x"), None);
        let failed: crate::error::Result<Option<u8>> =
            Err(crate::error::Error::Gh("no token".into()));
        assert_eq!(degrade(failed, 1, "x"), None);
    }

    #[test]
    fn the_feedback_note_counts_threads() {
        assert_eq!(feedback_detail(1), "Sent 1 review thread to an agent");
        assert_eq!(feedback_detail(3), "Sent 3 review threads to an agent");
    }
}

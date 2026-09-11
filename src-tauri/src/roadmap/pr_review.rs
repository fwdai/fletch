//! Board-side PR review reads for `in_review` cards (CI, conflicts, threads).
//!
//! Drop the DB lock before network; merge still goes through hold gates on the command path.


use std::path::PathBuf;

use serde::Serialize;

use super::drainer;
use super::types::{ItemStatus, RoadmapItem};
use super::Db;
use crate::github::{PrChecks, PrComments};

#[derive(Debug, Clone, Default, Serialize)]
pub struct ItemReview {
    pub checks: Option<PrChecks>,
    pub comments: Option<PrComments>,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
}

pub(crate) fn watchable(item: &RoadmapItem) -> Option<u32> {
    if item.status != ItemStatus::InReview {
        return None;
    }
    u32::try_from(item.pr_number?).ok()
}

pub(crate) fn target(db: &Db, item_id: &str) -> Option<(PathBuf, u32)> {
    let conn = db.lock();
    let item = super::store::get(&conn, item_id).ok().flatten()?;
    let number = watchable(&item)?;
    let repo = drainer::primary_repo_path(&conn, &item.project_id)?;
    Some((PathBuf::from(repo), number))
}

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

fn degrade<T>(read: crate::error::Result<Option<T>>, number: u32, what: &str) -> Option<T> {
    match read {
        Ok(value) => value,
        Err(e) => {
            tracing::debug!(pr = number, error = %e, "roadmap review: {what} read failed");
            None
        }
    }
}

pub(crate) fn feedback_detail(threads: usize) -> String {
    match threads {
        1 => "Sent 1 review thread to an agent".to_string(),
        n => format!("Sent {n} review threads to an agent"),
    }
}

#[cfg(test)]
#[path = "tests/pr_review.rs"]
mod tests;

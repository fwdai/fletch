//! The Git panel's combined PR read: merge gate + per-check rollup + unresolved
//! review threads for one PR, in a single by-number GraphQL round-trip.
//!
//! Why this exists: the panel used to poll `pr_checks` and `pr_comments`
//! separately, both through the `first:30` branch scan. Because GraphQL bills a
//! nested connection *per parent node the parent connection declares*, the
//! review-thread selection cost ~30 points there — 30× what the same fields
//! cost hanging off a single `pullRequest(number:)` node. Fetching both field
//! sets off one by-number node makes the pair cost 1 point total and halves the
//! round-trips, so the panel can stay on a tight cadence.

use std::path::Path;

use crate::error::Result;

use super::checks::{pr_checks_from_node, PR_CHECKS_FIELDS};
use super::comments::{pr_comments_from_node, PR_COMMENTS_FIELDS};
use super::query::pr_node_by_number;
use super::types::*;

/// Fetch the merge gate, checks, and unresolved review threads for one PR by
/// number, in a single query. `Ok(None)` when the PR (or the GitHub slug) can't
/// be resolved, or while a rate-limit backoff is active — the panel then keeps
/// its last-known values, matching the per-field fetchers' contract.
pub async fn pr_detail_number(
    checkout: &Path,
    source_repo: Option<&Path>,
    number: u32,
) -> Result<Option<PrDetail>> {
    let fields = format!("{PR_CHECKS_FIELDS}\n{PR_COMMENTS_FIELDS}");
    let Some(node) = pr_node_by_number(checkout, source_repo, number, &fields).await? else {
        return Ok(None);
    };
    Ok(Some(PrDetail {
        checks: pr_checks_from_node(&node),
        comments: pr_comments_from_node(&node),
    }))
}

//! The end of a run: push the last step's pinned ref as the run branch and,
//! when the spec asks, open its PR.

use std::path::Path;

use serde_json::json;

use crate::error::Result;
use crate::workflow::gitops;
use crate::workflow::journal::journal_event;
use crate::workflow::spec::Spec;
use crate::workflow::types::event_type;

use super::store::{set_pr, RunEssentials};
use super::RunCtx;

/// Push the run's final ref and (optionally) open its PR, from the run repo the
/// steps committed in.
pub(crate) async fn finalize_run(
    ctx: &RunCtx,
    run_id: &str,
    run: &RunEssentials,
    spec: &Spec,
    run_repo: &Path,
    last_exec_id: Option<&str>,
) -> Result<()> {
    let Some(fin) = spec.finalize.as_ref() else {
        return Ok(());
    };
    if !fin.push {
        return Ok(());
    }
    let Some(exec_id) = last_exec_id else {
        // Nothing was committed; nothing to push.
        return Ok(());
    };
    let final_ref = gitops::step_ref(exec_id);
    // The spec's explicit `pr_base` wins; otherwise fall back to the branch the
    // run was launched from, so a run forked off `develop` opens its PR against
    // `develop` rather than `main`. Empty `base_branch` means none was selected.
    let base = fin
        .pr_base
        .clone()
        .filter(|b| !b.is_empty())
        .or_else(|| Some(run.base_branch.clone()).filter(|b| !b.is_empty()))
        .unwrap_or_else(|| "main".to_string());
    let title = format!("wf: {}", spec.name);
    // A run started from an issue carries its ref ("123" for GitHub,
    // "ENG-123" for Linear); append the matching closing trailer so merging
    // the finalized PR closes the issue. The run repo is inherently the
    // issue's repo (single-repo), so no subdir gating — unlike the multi-repo
    // agent path. Idempotent; `None` (a normal launch) leaves the empty body
    // untouched.
    let body = crate::github::with_closes_trailer("", run.issue_ref.as_deref());
    let outcome = gitops::finalize(&gitops::Finalize {
        run_repo,
        source_repo: std::path::Path::new(&run.repo_path),
        final_ref: &final_ref,
        branch: &run.branch,
        base: &base,
        title: &title,
        body: &body,
        open_pr: fin.open_pr,
    })
    .await?;
    let conn = ctx.db.lock();
    journal_event(
        &conn,
        ctx.engine.as_ref(),
        run_id,
        event_type::FINALIZE_PUSHED,
        None,
        &json!({ "branch": outcome.branch }),
    );
    if let Some(url) = outcome.pr_url {
        journal_event(
            &conn,
            ctx.engine.as_ref(),
            run_id,
            event_type::FINALIZE_PR,
            None,
            &json!({ "url": url }),
        );
        // …and onto the row itself (0029). The journal event is the run's
        // history; the columns are the queryable fact. The roadmap merge sweep
        // needs the latter — "every `in_review` item's PR number" has to be one
        // read, not a journal scan per run per tick.
        set_pr(&conn, ctx.engine.as_ref(), run_id, outcome.pr_number, &url);
    } else if let Some(err) = outcome.pr_error {
        journal_event(
            &conn,
            ctx.engine.as_ref(),
            run_id,
            event_type::FINALIZE_PR,
            None,
            &json!({ "error": err }),
        );
    }
    Ok(())
}

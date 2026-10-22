//! The local review loop's durable half: the reviewer's findings as review
//! threads, merged with GitHub's so every consumer — the Git panel's Review
//! row, the readiness ladder, autopilot's `resolve-comments` rung and the
//! coder's `pr_threads` / `reply_thread` / `resolve_thread` ops — handles a
//! local finding exactly as it handles a Greptile comment.
//!
//! A local thread is keyed by the coder's checkout, the same
//! `(agent_id, subdir)` key delegations and autopilot use (`None` = primary),
//! and exists with or without a PR: the reviewer runs on PR open by default,
//! but can be asked earlier.

pub mod engine;
pub mod model;
pub mod persist;
pub mod store;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

pub use engine::{Phase, ReviewState, ReviewerView, Trigger};
pub use model::{Finding, Reply, ReplyAuthor, ThreadRow};

use crate::error::Result;
use crate::github::{self as gh, PrComments};
use crate::host::EngineCtx;
use crate::supervisor::Supervisor;
use crate::workspace::AgentRecord;

/// One checkout: `(agent_id, subdir)`, `None` for the primary repo. The same
/// key as `supervisor::delegation::Key`.
pub type Key = crate::supervisor::delegation::Key;

/// Epoch milliseconds, the one clock every review table reads.
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The GitHub threads last seen per checkout, by whoever read them (the
/// panel's slow tick, the watcher), so a local mutation can announce the
/// merged set without a GraphQL read of its own.
fn last_github() -> &'static Mutex<HashMap<Key, Vec<gh::PrComment>>> {
    static LAST: OnceLock<Mutex<HashMap<Key, Vec<gh::PrComment>>>> = OnceLock::new();
    LAST.get_or_init(Default::default)
}

fn remember_github(key: &Key, github: &[gh::PrComment]) {
    last_github().lock().insert(key.clone(), github.to_vec());
}

/// The store key a request's `subdir` names: `None` for the primary (whether
/// named or not), the subdir for a tracked secondary, an error otherwise.
pub fn checkout_key(record: &AgentRecord, subdir: Option<&str>) -> Result<Key> {
    Ok((
        record.id.clone(),
        crate::supervisor::delegation::checkout_subdir(record, subdir)?,
    ))
}

/// The local threads of `key` put in front of `github`'s: the merged set every
/// consumer reads.
pub fn merge_local(conn: &rusqlite::Connection, key: &Key, github: PrComments) -> PrComments {
    remember_github(key, &github.unresolved);
    let mut unresolved = match store::unresolved(conn, key) {
        Ok(local) => local,
        Err(e) => {
            tracing::warn!(error = %e, agent_id = %key.0, "local review threads unreadable");
            Vec::new()
        }
    };
    unresolved.extend(github.unresolved);
    PrComments { unresolved }
}

/// Every unresolved review thread of a checkout: the local reviewer's, then
/// the focused open PR's GitHub threads. `None` only when there is neither a
/// local thread nor an open PR to read — so a checkout with findings and no
/// PR still answers with threads.
///
/// `subdir`: `None` for the primary repo, or a tracked secondary's subdir.
pub async fn threads_for(
    sup: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<PrComments>> {
    let Ok(record) = sup.workspace.agent(agent_id) else {
        return Ok(None);
    };
    let Ok(key) = checkout_key(&record, subdir) else {
        return Ok(None);
    };
    let local = {
        let conn = sup.workspace.db().lock();
        store::unresolved(&conn, &key)?
    };
    let github = github_threads(sup, agent_id, subdir).await?;
    remember_github(
        &key,
        github.as_ref().map_or(&[][..], |g| g.unresolved.as_slice()),
    );
    match (local.is_empty(), github) {
        (true, None) => Ok(None),
        (_, github) => {
            let mut unresolved = local;
            unresolved.extend(github.map(|g| g.unresolved).unwrap_or_default());
            Ok(Some(PrComments { unresolved }))
        }
    }
}

/// The focused open PR's unresolved GitHub review threads, flattened to each
/// thread's root comment; `None` for no open PR or a read that didn't resolve.
///
/// Stays on GraphQL because thread resolution (`isResolved`/`isOutdated`) has
/// no REST equivalent — so unlike the panel's fast tick this spends points, 1
/// per call by PR number. Polled well below the state/CI cadence to match.
async fn github_threads(
    sup: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<PrComments>> {
    // Same resolver as `get_pr_live_impl`, so this shares its adoption and
    // throttling rather than running a second branch scan of its own. Its live
    // lookup is a conditional REST read the fast tick has already warmed, so
    // reaching for the PR number here is free.
    let Some((state, _bound)) = crate::supervisor::resolve_pr_state(
        &sup.workspace,
        agent_id,
        subdir,
        crate::supervisor::Discovery::Throttled,
    )
    .await
    else {
        return Ok(None);
    };
    // Only an open PR has threads worth polling.
    if !matches!(state.state, gh::PrStatus::Open) {
        return Ok(None);
    }
    let Some((repo, checkout)) = crate::commands::agent_repo_checkout_opt(sup, agent_id, subdir)?
    else {
        return Ok(None);
    };
    Ok(
        gh::pr_threads_number(&checkout, Some(&repo.repo_path), state.number)
            .await
            .unwrap_or(None),
    )
}

/// Tell every client the checkout's threads changed: the local threads in
/// front of the GitHub threads last seen for it, with `new_ids` as the
/// threads to announce. Called after a local mutation (findings stored, a
/// reply, a resolution); no GitHub read — the watcher's next look refreshes
/// that half. A checkout with no PR is reached only this way.
pub async fn emit_threads_changed(
    ctx: &Arc<EngineCtx>,
    agent_id: &str,
    subdir: Option<&str>,
    new_ids: Vec<String>,
) {
    let Some(sup) = ctx.supervisor() else {
        return;
    };
    let Ok(record) = sup.workspace.agent(agent_id) else {
        return;
    };
    let Ok(key) = checkout_key(&record, subdir) else {
        return;
    };
    let github = PrComments {
        unresolved: last_github().lock().get(&key).cloned().unwrap_or_default(),
    };
    let comments = {
        let conn = sup.workspace.db().lock();
        merge_local(&conn, &key, github)
    };
    crate::supervisor::emit_pr_threads(ctx.sink.as_ref(), agent_id, subdir, comments, new_ids);
}

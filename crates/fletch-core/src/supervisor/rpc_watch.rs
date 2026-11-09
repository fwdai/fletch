//! Per-agent file-mailbox RPC watcher: each tick, execute pending requests
//! and apply the resulting git/PR events.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::host::EngineCtx;
use crate::rpc;

use super::events::{emit_branch, emit_git_action, emit_notify, emit_title, emit_tour_changed};
use super::Supervisor;

/// How often the per-agent RPC watcher scans its mailbox for new requests.
const RPC_TICK: Duration = Duration::from_millis(100);

/// Drive the agent's file-mailbox RPC for the life of this generation: each
/// tick, execute any pending requests and write responses. Gen-guarded like
/// `spawn_turn_watchdog`, so it exits cleanly when the agent is respawned or
/// torn down (no explicit handle to track). Polling (no `notify` crate) mirrors
/// the transcript-sync style already used elsewhere — and is load-bearing for
/// sandbox engines beyond seatbelt: guest-originated writes over VirtioFS
/// don't reliably produce FSEvents, so this watcher must never be converted to
/// an FS-event trigger without keeping an interval fallback (`process_pending`
/// is idempotent, so extra triggers are safe).
pub(super) fn spawn_rpc_watcher(
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
    agent_id: String,
    dispatcher: Arc<dyn rpc::RpcDispatcher>,
    rpc_dir: PathBuf,
    gen: u64,
) {
    crate::host::spawn(async move {
        loop {
            tokio::time::sleep(RPC_TICK).await;

            let current_gen = sup.generations.lock().get(&agent_id).copied().unwrap_or(0);
            if current_gen != gen {
                return;
            }

            process_agent_rpc_once(&sup, &ctx, &agent_id, dispatcher.as_ref(), &rpc_dir).await;
        }
    });
}

/// Process every request currently pending in `agent_id`'s mailbox once, applying
/// the resulting git/PR events. `process_pending` is idempotent and
/// in-flight-guarded, so a request is dispatched exactly once however the ticks
/// fall.
async fn process_agent_rpc_once(
    sup: &Arc<Supervisor>,
    ctx: &Arc<EngineCtx>,
    agent_id: &str,
    dispatcher: &dyn rpc::RpcDispatcher,
    rpc_dir: &std::path::Path,
) {
    let events = rpc::process_pending(rpc_dir, dispatcher).await;
    for event in events {
        handle_rpc_event(sup, ctx, agent_id, event);
    }
}

fn handle_rpc_event(sup: &Supervisor, ctx: &Arc<EngineCtx>, agent_id: &str, event: rpc::RpcEvent) {
    // The subdir of the checkout a branch/PR event belongs to: the dispatcher
    // stamps the targeted repo into the payload (`args.repo` routing), so a PR
    // opened in a sibling checkout lands on that repo's worktree row. Events
    // without the field (legacy dispatchers, tests) keep the old primary-repo
    // behavior — correct for single-repo agents, where primary is all there is.
    fn event_subdir<'a>(
        record: &'a crate::workspace::AgentRecord,
        payload: &serde_json::Value,
    ) -> Option<&'a str> {
        match payload.get("repo").and_then(|v| v.as_str()) {
            Some(requested) => {
                let known = record
                    .repos
                    .iter()
                    .find(|r| r.subdir == requested)
                    .map(|r| r.subdir.as_str());
                if known.is_none() {
                    tracing::warn!(
                        repo = %requested,
                        "git event names an untracked repo; dropping instead of misattributing"
                    );
                }
                known
            }
            None => record.repos.first().map(|r| r.subdir.as_str()),
        }
    }

    match event {
        rpc::RpcEvent::Named { name, payload } if name == rpc::git::EVENT_BRANCH_CREATED => {
            let Some(branch) = payload.get("branch").and_then(|v| v.as_str()) else {
                tracing::warn!(
                    event = %name,
                    payload = %payload,
                    "git dispatcher emitted branch event without branch"
                );
                return;
            };
            // A head published through `args.head` is the workspace's own but
            // not the branch its checkout is on; dispatchers that predate the
            // field only ever emitted for the checked-out branch.
            let current = payload
                .get("current")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if let Ok(record) = sup.workspace.agent(agent_id) {
                if let Some(subdir) = event_subdir(&record, &payload) {
                    // Two records: the set of branches the checkout has
                    // created, which the push guard reads
                    // (`rpc::caps::refuses_publish`) and which keeps an
                    // earlier branch the workspace's own after a later one
                    // replaces it as current; and the branch it is on now.
                    if let Err(e) = sup.workspace.record_owned_branch(agent_id, subdir, branch) {
                        tracing::warn!(
                            error = %e,
                            agent_id = %agent_id,
                            branch = %branch,
                            "git_push/open_pr: failed to record the branch as the workspace's own"
                        );
                    }
                    if !current {
                        return;
                    }
                    if let Err(e) = sup.workspace.set_repo_branch(agent_id, subdir, branch) {
                        tracing::warn!(
                            error = %e,
                            agent_id = %agent_id,
                            branch = %branch,
                            "git_push/open_pr: failed to persist branch name"
                        );
                    } else {
                        emit_branch(ctx.sink.as_ref(), agent_id, subdir, branch);
                    }
                }
            }
        }
        rpc::RpcEvent::Named { name, payload } if name == rpc::git::EVENT_PR_OPENED => {
            let Some(number) = payload.get("number").and_then(|v| v.as_u64()) else {
                tracing::warn!(
                    event = %name,
                    payload = %payload,
                    "git dispatcher emitted PR event without number"
                );
                return;
            };
            if let Ok(record) = sup.workspace.agent(agent_id) {
                if let Some(subdir) = event_subdir(&record, &payload) {
                    // Older dispatchers sent only the number; the row then
                    // fills in from the first fetch.
                    let field = |k: &str| payload.get(k).and_then(|v| v.as_str());
                    if let Err(e) = sup.workspace.set_repo_pr_number(
                        agent_id,
                        subdir,
                        number as i64,
                        field("url").unwrap_or_default(),
                        field("title").unwrap_or_default(),
                        field("branch"),
                    ) {
                        tracing::warn!(
                            error = %e,
                            agent_id = %agent_id,
                            pr = number,
                            "open_pr: failed to persist PR number"
                        );
                    }
                    // The base the PR was actually opened against, which
                    // `args.base` may have redirected away from the spawn-time
                    // default. Rewriting the record keeps every base-keyed
                    // reader (ahead/behind, "rebase onto", the `update-branch`
                    // fetch) pointed at the branch this PR is stacked on — most
                    // importantly `update-branch`, which would otherwise merge
                    // the wrong branch into the PR. A no-op write when the base
                    // wasn't redirected, so no need to compare first.
                    //
                    // The record only. The live `GitDispatcher` stamps its base
                    // at construction and is not rebuilt until the next
                    // resume/turn, so a *second* bare `open_pr` in this same
                    // session still defaults to the spawn base — same bounded
                    // staleness the dispatcher already documents for
                    // `own_branch` (see `supervisor::lifecycle`). The app path
                    // self-corrects, because the panel reads its trigger's
                    // `base` param from this row; a bare op call does not.
                    // Making the base live would mean interior mutability on a
                    // struct deliberately stamped once at spawn, which is a
                    // bigger change than it buys.
                    if let Some(base) = payload.get("base").and_then(|v| v.as_str()) {
                        if let Err(e) = sup.workspace.set_repo_parent_branch(agent_id, subdir, base)
                        {
                            tracing::warn!(
                                error = %e,
                                agent_id = %agent_id,
                                base = %base,
                                "open_pr: failed to persist PR base"
                            );
                        }
                    }
                }
            }
            sup.fetch_and_emit_pr_state(ctx.clone(), agent_id.to_string());
            // A PR just opened: the local reviewer's cue (`review::engine`),
            // in the background — the spawn it may start is not the op's.
            if let Some(sup) = ctx.supervisor() {
                let (ctx, id) = (ctx.clone(), agent_id.to_string());
                let subdir = payload
                    .get("repo")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let number = u32::try_from(number).ok();
                crate::host::spawn(async move {
                    crate::review::engine::on_pr_opened(
                        &ctx,
                        &sup,
                        &id,
                        subdir.as_deref(),
                        number,
                        None,
                    )
                    .await;
                });
            }
            // The agent may now hold PRs in two or more repos — cross-link the
            // set in each PR's body (best-effort, in the background).
            let workspace = sup.workspace.clone();
            let id = agent_id.to_string();
            crate::host::spawn(async move {
                super::sync_pr_set_links(&workspace, &id).await;
            });
        }
        rpc::RpcEvent::Named { name, payload } if name == rpc::git::EVENT_PR_EDITED => {
            // Only a retarget is relayed: the base the PR now sits on has to
            // reach the record for the same reasons as on `EVENT_PR_OPENED`
            // (ahead/behind, "rebase onto", the `update-branch` fetch). The
            // dispatcher emits it for the checkout's own PR only; when the
            // record already knows its PR, a mismatched number is dropped
            // rather than steering the checkout at another PR's base.
            if let (Ok(record), Some(base)) = (
                sup.workspace.agent(agent_id),
                payload.get("base").and_then(|v| v.as_str()),
            ) {
                if let Some(subdir) = event_subdir(&record, &payload) {
                    let edited = payload.get("number").and_then(|v| v.as_i64());
                    let bound = record
                        .repos
                        .iter()
                        .find(|r| r.subdir == subdir)
                        .and_then(|r| r.pr_number);
                    if let (Some(edited), Some(bound)) = (edited, bound) {
                        if edited != bound {
                            tracing::warn!(
                                agent_id = %agent_id,
                                edited,
                                bound,
                                "pr_edit: retarget of another PR; the checkout's base stays"
                            );
                            return;
                        }
                    }
                    if let Err(e) = sup.workspace.set_repo_parent_branch(agent_id, subdir, base) {
                        tracing::warn!(
                            error = %e,
                            agent_id = %agent_id,
                            base = %base,
                            "pr_edit: failed to persist PR base"
                        );
                    }
                }
            }
            sup.fetch_and_emit_pr_state(ctx.clone(), agent_id.to_string());
        }
        rpc::RpcEvent::Named { name, payload } if name == rpc::git::EVENT_ACTION_DONE => {
            // Authoritative "the agent performed a git mutation this
            // turn" signal. Forward it so the panel can attribute a
            // git/PR transition to the turn rather than guessing from
            // a polled snapshot.
            let op = payload
                .get("op")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            // The delegation driver hears it here rather than off the sink:
            // the same fact, at the same moment every client hears it.
            super::delegation::note_git_action(agent_id, &op);
            emit_git_action(ctx.sink.as_ref(), agent_id, op);
        }
        rpc::RpcEvent::Named { name, payload } if name == rpc::git::EVENT_TITLE_SET => {
            let Some(title) = payload.get("title").and_then(|v| v.as_str()) else {
                tracing::warn!(
                    event = %name,
                    payload = %payload,
                    "dispatcher emitted title event without title"
                );
                return;
            };
            match sup.workspace.set_agent_title(agent_id, title) {
                Ok(()) => emit_title(ctx.sink.as_ref(), agent_id, title.to_string()),
                Err(e) => tracing::warn!(
                    error = %e,
                    agent_id = %agent_id,
                    "set_title: failed to persist workspace title"
                ),
            }
        }
        rpc::RpcEvent::Named { name, payload } if name == rpc::git::EVENT_TOUR_SET => {
            let steps: Vec<crate::workspace::TourStep> =
                match serde_json::from_value(payload["steps"].clone()) {
                    Ok(steps) => steps,
                    Err(e) => {
                        tracing::warn!(error = %e, "dispatcher emitted tour event without steps");
                        return;
                    }
                };
            // Credited to the turn being explained when the host knows it; a
            // lookup failure costs only that link, not the tour.
            let turn_id = sup
                .workspace
                .latest_user_turn_id(agent_id)
                .unwrap_or_default();
            match sup.workspace.set_tour(agent_id, turn_id.as_deref(), &steps) {
                Ok(()) => emit_tour_changed(ctx.sink.as_ref(), agent_id),
                Err(e) => tracing::warn!(
                    error = %e,
                    agent_id = %agent_id,
                    "tour_set: failed to persist the walkthrough"
                ),
            }
        }
        rpc::RpcEvent::Named { name, payload } if name == rpc::run::EVENT_NOTIFY => {
            // The agent asked for the user's attention. Nothing to persist:
            // the clients show it (under their own mute) and that is all.
            let text = |key: &str| {
                payload
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            let title = text("title");
            if title.is_empty() {
                tracing::warn!(payload = %payload, "notify: event without a title");
                return;
            }
            emit_notify(ctx.sink.as_ref(), agent_id, title, text("body"));
        }
        rpc::RpcEvent::Named { name, payload } => {
            tracing::debug!(event = %name, payload = %payload, "rpc: unhandled event");
        }
    }
}

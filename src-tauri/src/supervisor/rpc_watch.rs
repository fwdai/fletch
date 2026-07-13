//! Per-agent file-mailbox RPC watcher: each tick, execute pending requests
//! and apply the resulting git/PR events.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;

use crate::rpc;

use super::events::{emit_branch, emit_git_action};
use super::Supervisor;

/// How often the per-agent RPC watcher scans its mailbox for new requests.
const RPC_TICK: Duration = Duration::from_millis(100);

/// Drive the agent's file-mailbox RPC for the life of this generation: each
/// tick, execute any pending requests and write responses. Gen-guarded like
/// `spawn_turn_watchdog`, so it exits cleanly when the agent is respawned or
/// torn down (no explicit handle to track). Polling (no `notify` crate) mirrors
/// the transcript-sync style already used elsewhere — and is load-bearing for
/// sandbox engines beyond seatbelt: container-originated writes over VirtioFS
/// don't reliably produce FSEvents, so this watcher must never be converted to
/// an FS-event trigger without keeping an interval fallback (`process_pending`
/// is idempotent, so extra triggers are safe).
pub(super) fn spawn_rpc_watcher(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    dispatcher: Arc<dyn rpc::RpcDispatcher>,
    rpc_dir: PathBuf,
    gen: u64,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(RPC_TICK).await;

            let current_gen = sup.generations.lock().get(&agent_id).copied().unwrap_or(0);
            if current_gen != gen {
                return;
            }

            process_agent_rpc_once(&sup, &app, &agent_id, dispatcher.as_ref(), &rpc_dir).await;
        }
    });
}

/// Process every request currently pending in `agent_id`'s mailbox once, applying
/// the resulting git/PR events. Shared by the polling watcher and the scheduler's
/// synchronous drain (`Supervisor::settle_agent_rpc`): `process_pending` is
/// idempotent and in-flight-guarded, so a concurrent tick and drain never
/// double-dispatch. A request the agent wrote during its turn (e.g. a `wf_ask`)
/// is therefore dispatched — and its message persisted — deterministically before
/// the scheduler acts on that turn's gate outcome (§10.4).
pub(super) async fn process_agent_rpc_once(
    sup: &Arc<Supervisor>,
    app: &AppHandle,
    agent_id: &str,
    dispatcher: &dyn rpc::RpcDispatcher,
    rpc_dir: &std::path::Path,
) {
    let events = rpc::process_pending(rpc_dir, dispatcher).await;
    for event in events {
        handle_rpc_event(sup, app, agent_id, event);
    }
}

fn handle_rpc_event(sup: &Supervisor, app: &AppHandle, agent_id: &str, event: rpc::RpcEvent) {
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
            if let Ok(record) = sup.workspace.agent(agent_id) {
                if let Some(repo) = record.repos.first() {
                    if let Err(e) = sup
                        .workspace
                        .set_repo_branch(agent_id, &repo.subdir, branch)
                    {
                        tracing::warn!(
                            error = %e,
                            agent_id = %agent_id,
                            branch = %branch,
                            "git_push/open_pr: failed to persist branch name"
                        );
                    } else {
                        emit_branch(app, agent_id, &repo.subdir, branch);
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
                if let Some(repo) = record.repos.first() {
                    if let Err(e) =
                        sup.workspace
                            .set_repo_pr_number(agent_id, &repo.subdir, number as i64)
                    {
                        tracing::warn!(
                            error = %e,
                            agent_id = %agent_id,
                            pr = number,
                            "open_pr: failed to persist PR number"
                        );
                    }
                }
            }
            sup.fetch_and_emit_pr_state(app.clone(), agent_id.to_string());
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
            emit_git_action(app, agent_id, op);
        }
        rpc::RpcEvent::Named { name, payload } => {
            tracing::debug!(event = %name, payload = %payload, "rpc: unhandled event");
        }
    }
}

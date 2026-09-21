//! The remote op allowlist.
//!
//! Every arm deserializes the same camelCase argument keys the matching Tauri
//! command receives from `invoke`, then calls the same supervisor/service
//! function that command calls — so a phone and the desktop webview cannot
//! drift apart in behaviour. Nothing outside [`OPS`] is reachable: `db_*`, the
//! file mutations, the shell ops, raw PTY input, editor/log/telemetry/provider
//! ops and the `run_*` surface are all off the wire by construction.
//!
//! The whole `wf_*` and `roadmap_*` surface is on it (multi-host plan §5.3,
//! item 2): a paired desktop or phone drives autopilot, workflows and the
//! roadmap board against a headless host. See [`WITHHELD_WF_ROADMAP_OPS`] for
//! what the surrounding *flows* still cannot do remotely and why.
//!
//! Adding an op is: a row in `docs/remote-protocol.md`, a name in [`OPS`], and
//! one match arm here — unless it needs to know *which device* is asking, in
//! which case it goes in [`SESSION_OPS`] and is answered by `server`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

use crate::commands::DiffBaseMode;
use crate::host::EngineCtx;
use crate::managed_session::ToolUseBehavior;
use crate::supervisor::Supervisor;
use crate::workspace::AgentView;

/// A dispatched op's answer: the command's result serialized as Tauri would, or
/// the `Display` of the Rust error.
pub type DispatchResult = std::result::Result<Value, String>;
pub type DispatchFuture<'a> = Pin<Box<dyn Future<Output = DispatchResult> + Send + 'a>>;

/// What the WebSocket server needs from the op layer. A trait so the socket
/// tests can drive the server with a stub while production runs the
/// supervisor-backed impl below.
pub trait Dispatch: Send + Sync + 'static {
    fn dispatch<'a>(&'a self, op: &'a str, args: Value) -> DispatchFuture<'a>;
}

/// The error text the protocol reserves for an op that is not on the allowlist.
pub const UNKNOWN_OP: &str = "unknown op";

/// The five ops a host can only answer with a local speech engine behind it.
/// They stay in [`OPS`] on every host — the protocol descriptor advertises them
/// and a phone may always ask — but a host with no engine answers
/// `dictation_status` with `available: false` and the four capture ops with
/// [`DICTATION_UNAVAILABLE`], which is the wire shape a non-macOS build has
/// always had.
const DICTATION_OPS: &[&str] = &[
    "dictation_status",
    "dictation_begin",
    "dictation_audio",
    "dictation_end",
    "dictation_cancel",
];

/// What a host with no local speech engine says. The same words the non-macOS
/// build has always answered with, so the phone's message does not change with
/// the host's shape.
pub const DICTATION_UNAVAILABLE: &str =
    "Dictation from a phone needs a Mac host: whisper.cpp is only built there.";

/// The error text a connection gets when it already has the per-connection
/// maximum of requests outstanding (`server::MAX_IN_FLIGHT`). The request is
/// answered without being dispatched; the client retries.
pub const TOO_MANY_IN_FLIGHT: &str = "too many in-flight requests";

/// The error a `wf_*` op gets on a host whose scheduler was never published —
/// a boot that stopped short, or a test ctx. The ops stay on [`OPS`] (the
/// scheduler is part of every real boot, desktop or headless), so this is a
/// failure, not a capability answer like [`DICTATION_UNAVAILABLE`].
pub const WORKFLOWS_UNAVAILABLE: &str = "this host is not running the workflow scheduler";

/// What the workflow and roadmap *flows* still cannot reach remotely, with the
/// reason each one stays off. All 48 `wf_*`/`roadmap_*` commands themselves are
/// exposed — none opens a dialog, touches desktop-only state or is a host
/// policy decision — so what is left here is the ops those surfaces call from
/// *other* command families, which keep the exclusions their own family has.
/// Named rather than merely absent so the withholding is a decision on the
/// record, as `delete_branch_agent` is. Absence is what makes them unreachable,
/// so this list drives nothing at runtime — it is the reason, asserted by
/// `tests::the_withheld_neighbours_of_the_wf_and_roadmap_surface_stay_off`.
#[cfg(test)]
pub const WITHHELD_WF_ROADMAP_OPS: &[(&str, &str)] = &[
    // The workflow composer's `@file` and `#PR` mention sources. Part of the
    // file/github read surface, which is off the wire as a family.
    ("list_repo_tree", "file-read surface, withheld as a family"),
    ("list_repo_prs", "github-read surface, withheld as a family"),
    // The desktop autopilot ladder's two rungs. `run_verification` runs this
    // machine's verify scripts and `fork_agent` is a documented follow-up, so
    // the ladder stays a local loop over the local engine (see
    // `src/store/autopilotSync.ts`).
    (
        "run_verification",
        "runs the local machine's verify scripts",
    ),
    ("fork_agent", "documented follow-up, not yet on the wire"),
];

/// The ops the generic dispatcher answers, in the order of the protocol doc's
/// table. Load-bearing: `dispatch` rejects anything absent here before the
/// `match` runs, so a name is only reachable when it appears both here and as
/// an arm.
pub const OPS: &[&str] = &[
    "get_workspace",
    "allocate_draft_name",
    "spawn_agent",
    "send_user_message",
    "answer_tool_use",
    "answer_publish_approval",
    "stop_agent",
    "resume_agent",
    "archive_agent",
    "restore_agent",
    "discard_agent",
    "set_agent_model",
    "set_agent_effort",
    "read_session_records",
    "read_user_turns",
    "sync_session",
    "read_live_turn",
    "get_git_state",
    "get_all_shortstats",
    "get_all_git_meta",
    "list_checkout_tree",
    "read_checkout_file",
    "get_file_diff",
    "commit_agent",
    "push_agent",
    "pull_agent",
    "rebase_agent",
    "stash_agent",
    "discard_agent_changes",
    "abort_merge_agent",
    "create_pr",
    "merge_pr",
    "get_pr_state",
    "get_pr_checks",
    "get_pr_live",
    "get_pr_threads",
    "list_repo_branches",
    "repo_default_branch",
    "discover_supported_models",
    "list_dir",
    "add_workspace_repo",
    "clone_repo",
    "create_repo",
    "gh_status",
    "gh_repo_list",
    "remove_workspace_repo",
    "attach_repo_to_project",
    "detach_repo_from_project",
    "set_repo_label",
    "rename_project",
    "project_has_running_agents",
    "delete_project",
    "relocate_repo",
    "dictation_status",
    "dictation_begin",
    "dictation_audio",
    "dictation_end",
    "dictation_cancel",
    "attachment_begin",
    "attachment_chunk",
    "attachment_end",
    "attachment_cancel",
    "list_project_chats",
    "list_custom_agents",
    "get_agent",
    "wf_list_runs",
    "wf_get_run",
    "wf_events",
    "wf_run_agents",
    "wf_launch",
    "wf_cancel",
    "wf_resume",
    "wf_retry",
    "wf_approve",
    "wf_reject",
    "wf_run_diff",
    "wf_resolve_conflict",
    "wf_delete_run",
    "wf_answer",
    "wf_def_save",
    "wf_def_list",
    "wf_def_delete",
    "wf_def_export_yaml",
    "wf_def_import_yaml",
    "roadmap_list_items",
    "roadmap_get_item",
    "roadmap_create_item",
    "roadmap_update_item",
    "roadmap_set_rank",
    "roadmap_hand_off_item",
    "roadmap_item_review",
    "roadmap_merge_item_pr",
    "roadmap_note_review_feedback",
    "roadmap_hold_item",
    "roadmap_release_item",
    "roadmap_hold_project",
    "roadmap_release_project",
    "roadmap_get_project_hold",
    "roadmap_reclaim_item",
    "roadmap_reject_item",
    "roadmap_reopen_item",
    "roadmap_delete_item",
    "roadmap_discard_proposal",
    "roadmap_list_item_events",
    "roadmap_latest_events",
    "roadmap_list_proposals",
    "roadmap_accept_proposal",
    "roadmap_reject_proposal",
    "roadmap_get_order_proposal",
    "roadmap_accept_order_proposal",
    "roadmap_reject_order_proposal",
    "roadmap_get_brief",
    "roadmap_get_brief_proposal",
    "roadmap_accept_brief_proposal",
    "roadmap_reject_brief_proposal",
];

pub const REGISTER_PUSH: &str = "register_push";

/// The ops the *session* layer answers itself, because they act on the calling
/// device's own record and [`Dispatch`] deliberately carries no notion of who
/// is calling: every op in [`OPS`] is device-agnostic, and widening the trait
/// to thread a device id through 26 arms that must not depend on one would put
/// an identity where the design keeps it out. `server::read_loop` handles these
/// next to `pair`/`hello`, the one place the key the Noise handshake proved has
/// already been resolved to a record.
///
/// They are still on [`is_allowed`], so the doc's op table and the code agree
/// about what a phone may name in a request frame; the generic dispatcher gates
/// on [`OPS`] alone, so one of these reaching it anyway fails closed as
/// [`UNKNOWN_OP`] instead of being silently answered without an identity.
pub const SESSION_OPS: &[&str] = &[REGISTER_PUSH];

/// The whole wire surface: every op name a phone may send. The union of what
/// the dispatcher answers and what the session layer answers for itself.
pub fn is_allowed(op: &str) -> bool {
    OPS.contains(&op) || SESSION_OPS.contains(&op)
}

/// The production dispatcher: the engine ctx and supervisor the Tauri commands
/// would have received through `State`.
pub struct SupervisorDispatch {
    ctx: Arc<EngineCtx>,
    sup: Arc<Supervisor>,
    /// The host's local speech engine, if it has one. Not an arm here because
    /// transcription is the one op group that needs a platform the engine does
    /// not: whisper.cpp is built on macOS only, and its module lives in the
    /// desktop shell beside the microphone code it shares. See
    /// [`DICTATION_OPS`].
    dictation: Option<Arc<dyn Dispatch>>,
}

impl SupervisorDispatch {
    pub fn new(ctx: Arc<EngineCtx>, sup: Arc<Supervisor>) -> Self {
        Self {
            ctx,
            sup,
            dictation: None,
        }
    }

    /// Answer the five [`DICTATION_OPS`] with `dictation` instead of the
    /// unavailable stub. The desktop passes its whisper-backed dispatcher
    /// through [`crate::host::BootConfig::dictation`].
    pub fn with_dictation(mut self, dictation: Arc<dyn Dispatch>) -> Self {
        self.dictation = Some(dictation);
        self
    }
}

impl Dispatch for SupervisorDispatch {
    fn dispatch<'a>(&'a self, op: &'a str, args: Value) -> DispatchFuture<'a> {
        Box::pin(async move {
            // `OPS`, not `is_allowed`: a session op belongs to the session
            // layer and has no identity here, so it fails closed.
            if !OPS.contains(&op) {
                return Err(UNKNOWN_OP.to_string());
            }
            if DICTATION_OPS.contains(&op) {
                return match &self.dictation {
                    Some(dictation) => dictation.dispatch(op, args).await,
                    // Same shape `dictation::remote::Status` serializes, so a
                    // phone talking to an engine-less host reads the reply it
                    // already knows how to render.
                    None if op == "dictation_status" => Ok(serde_json::json!({
                        "available": false,
                        "reason": DICTATION_UNAVAILABLE,
                    })),
                    None => Err(DICTATION_UNAVAILABLE.to_string()),
                };
            }
            let sup = &self.sup;
            let ctx = &self.ctx;
            match op {
                "get_workspace" => ok(sup.current_workspace()),

                "allocate_draft_name" => {
                    let a: DraftsArgs = parse(args)?;
                    res(crate::commands::allocate_draft_name_impl(sup, a.drafts))
                }

                // The structured chat view is forced: the phone has no UI for a
                // native PTY. A roadmap-PM `purpose` passes through so a phone
                // can open the same planning chat the Roadmap tab does (see
                // [`remote_purpose`]). The phone only *names* a preset in
                // `customAgentId`; this host resolves it by value out of its own
                // library — brief, skills, MCP servers — so MCP commands and
                // their secrets never cross the wire, and the phone's own
                // `skills` / `mcpServers` keys stay ignored. A dangling id
                // resolves to nothing and spawns a plain agent, as the desktop
                // does.
                "spawn_agent" => {
                    let a: SpawnArgs = parse(args)?;
                    let profile = a.custom_agent_id.as_deref().and_then(|id| {
                        let conn = ctx.db.lock();
                        crate::agent_profile::library::resolve_custom_agent(
                            &conn,
                            id,
                            // The same default `spawn_agent_impl` applies, so
                            // the MCP filter matches the provider that runs.
                            a.provider.as_deref().unwrap_or("claude"),
                        )
                    });
                    let (custom_agent_id, instructions, skills, mcp_servers) = match profile {
                        Some(p) => (
                            Some(p.custom_agent_id),
                            p.instructions.or(a.instructions),
                            Some(p.skills),
                            Some(p.mcp_servers),
                        ),
                        None => (None, a.instructions, None, None),
                    };
                    res(crate::commands::spawn_agent_impl(
                        sup.clone(),
                        ctx.clone(),
                        Some(AgentView::Custom),
                        a.repo_path,
                        a.provider,
                        a.name,
                        a.effort,
                        a.model,
                        instructions,
                        custom_agent_id,
                        skills,
                        mcp_servers,
                        a.fork_base,
                        a.issue_ref,
                        remote_purpose(a.purpose),
                    )
                    .await)
                }

                "send_user_message" => {
                    let a: SendMessageArgs = parse(args)?;
                    res(sup.clone().send_user_message(
                        ctx,
                        &a.agent_id,
                        &a.turn_id,
                        &a.text,
                        &a.attachments,
                    ))
                }

                "answer_tool_use" => {
                    let a: AnswerToolUseArgs = parse(args)?;
                    res(sup.answer_tool_use(
                        &a.agent_id,
                        &a.request_id,
                        a.updated_input,
                        a.behavior,
                        a.message,
                    ))
                }

                // The gated-publish prompt already reaches a phone as
                // `publish:approval-requested`; this is the answer. The same
                // function the `answer_publish_approval` command calls, so an
                // id that has already timed out is ignored here too and a late
                // answer can never publish anything.
                "answer_publish_approval" => {
                    let a: PublishApprovalArgs = parse(args)?;
                    crate::rpc::approval::answer(&a.id, a.approved);
                    Ok(Value::Null)
                }

                "stop_agent" => {
                    let a: AgentArgs = parse(args)?;
                    res(sup.clone().stop_agent(ctx.clone(), &a.agent_id).await)
                }

                "resume_agent" => {
                    let a: AgentArgs = parse(args)?;
                    res(sup.clone().resume_agent(ctx.clone(), &a.agent_id).await)
                }

                "archive_agent" => {
                    let a: AgentArgs = parse(args)?;
                    res(sup.clone().archive_agent(ctx.clone(), &a.agent_id).await)
                }

                "restore_agent" => {
                    let a: AgentArgs = parse(args)?;
                    res(sup.clone().restore_agent(ctx.clone(), &a.agent_id).await)
                }

                // The destructive twin of `archive_agent`: record, checkout and
                // transcript all go. Same supervisor method the command calls,
                // so there is no softer remote variant of it.
                "discard_agent" => {
                    let a: AgentArgs = parse(args)?;
                    res(sup.clone().discard_agent(&a.agent_id).await)
                }

                "set_agent_model" => {
                    let a: ModelArgs = parse(args)?;
                    res(sup
                        .set_agent_model(ctx, &a.agent_id, a.model.as_deref())
                        .await)
                }

                "set_agent_effort" => {
                    let a: EffortArgs = parse(args)?;
                    res(sup
                        .set_agent_effort(ctx, &a.agent_id, a.effort.as_deref())
                        .await)
                }

                "read_session_records" => {
                    let a: AgentArgs = parse(args)?;
                    res(sup.workspace.read_session_records(&a.agent_id))
                }

                "read_user_turns" => {
                    let a: AgentArgs = parse(args)?;
                    res(sup.workspace.read_user_turns(&a.agent_id))
                }

                // Lazy backfill, same as the `sync_session` command: the phone
                // calls it when `read_session_records` comes back empty for an
                // agent that plainly has a transcript on disk.
                "sync_session" => {
                    let a: AgentArgs = parse(args)?;
                    sup.sync_session(&a.agent_id);
                    res::<()>(Ok(()))
                }

                // The running turn's event stream, for a client that missed it
                // (a phone back from the background). The turn-end ingest is
                // the only other copy and it has not happened yet.
                "read_live_turn" => {
                    let a: AgentArgs = parse(args)?;
                    ok(sup.read_live_turn(&a.agent_id))
                }

                "get_git_state" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(
                        crate::commands::get_git_state_impl(sup, &a.agent_id, a.subdir.as_deref())
                            .await,
                    )
                }

                "get_all_shortstats" => res(crate::commands::get_all_shortstats_impl(sup).await),

                "get_all_git_meta" => res(crate::commands::get_all_git_meta_impl(sup).await),

                "list_checkout_tree" => {
                    let a: AgentArgs = parse(args)?;
                    res(crate::commands::list_checkout_tree_impl(sup, &a.agent_id).await)
                }

                "read_checkout_file" => {
                    let a: FileArgs = parse(args)?;
                    res(crate::commands::read_checkout_file_impl(
                        sup,
                        &a.agent_id,
                        &a.path,
                        a.base_mode,
                    )
                    .await)
                }

                "get_file_diff" => {
                    let a: FileArgs = parse(args)?;
                    res(
                        crate::commands::get_file_diff_impl(sup, &a.agent_id, &a.path, a.base_mode)
                            .await,
                    )
                }

                "commit_agent" => {
                    let a: CommitArgs = parse(args)?;
                    res(crate::commands::commit_agent_impl(
                        sup,
                        &a.agent_id,
                        &a.message,
                        a.subdir.as_deref(),
                    )
                    .await)
                }

                "push_agent" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(crate::commands::push_agent_impl(
                        sup,
                        ctx.clone(),
                        a.agent_id,
                        a.subdir.as_deref(),
                    )
                    .await)
                }

                // The Git panel's working-tree moves. Each one acts inside the
                // agent's own checkout and nowhere else — the reach
                // `commit_agent` above already has — and none of them takes a
                // path, branch or ref from the caller. `delete_branch_agent` is
                // the one panel action left off the wire: it force-deletes a ref
                // in the user's real repo, outside every checkout.
                "pull_agent" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(
                        crate::commands::pull_agent_impl(sup, &a.agent_id, a.subdir.as_deref())
                            .await,
                    )
                }

                "rebase_agent" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(
                        crate::commands::rebase_agent_impl(sup, &a.agent_id, a.subdir.as_deref())
                            .await,
                    )
                }

                "stash_agent" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(
                        crate::commands::stash_agent_impl(sup, &a.agent_id, a.subdir.as_deref())
                            .await,
                    )
                }

                // Destructive, like `discard_agent` above, but only of the
                // working tree: the checkout and the transcript stay.
                "discard_agent_changes" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(crate::commands::discard_agent_changes_impl(
                        sup,
                        &a.agent_id,
                        a.subdir.as_deref(),
                    )
                    .await)
                }

                "abort_merge_agent" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(crate::commands::abort_merge_agent_impl(
                        sup,
                        &a.agent_id,
                        a.subdir.as_deref(),
                    )
                    .await)
                }

                "create_pr" => {
                    let a: CreatePrArgs = parse(args)?;
                    res(crate::commands::create_pr_impl(
                        sup,
                        a.agent_id,
                        &a.title,
                        &a.body,
                        a.subdir.as_deref(),
                    )
                    .await)
                }

                "merge_pr" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(crate::commands::merge_pr_impl(sup, &a.agent_id, a.subdir.as_deref()).await)
                }

                "get_pr_state" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(
                        crate::commands::get_pr_state_impl(sup, &a.agent_id, a.subdir.as_deref())
                            .await,
                    )
                }

                "get_pr_checks" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(
                        crate::commands::get_pr_checks_impl(sup, &a.agent_id, a.subdir.as_deref())
                            .await,
                    )
                }

                "get_pr_live" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(
                        crate::commands::get_pr_live_impl(sup, &a.agent_id, a.subdir.as_deref())
                            .await,
                    )
                }

                "get_pr_threads" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(
                        crate::commands::get_pr_threads_impl(sup, &a.agent_id, a.subdir.as_deref())
                            .await,
                    )
                }

                "list_repo_branches" => {
                    let a: RepoPathArgs = parse(args)?;
                    res(crate::git::list_local_branches(Path::new(&a.repo_path)).await)
                }

                "repo_default_branch" => {
                    let a: RepoPathArgs = parse(args)?;
                    ok(crate::git::default_branch(Path::new(&a.repo_path)).await)
                }

                "discover_supported_models" => {
                    ok(crate::model_catalog::discover_supported_models().await)
                }

                "list_dir" | "gh_status" | "gh_repo_list" => add_project_op(sup, op, args).await,

                // The three that change the project list also tell every other
                // view about it, exactly as the Tauri commands do.
                "add_workspace_repo" | "clone_repo" | "create_repo" => {
                    crate::commands::announce_workspace(
                        ctx.sink.as_ref(),
                        add_project_op(sup, op, args).await,
                    )
                }

                // The project settings page (protocol doc, "Project settings
                // from a remote client"). Every one of these rewrites the
                // project list, so each tells the other views about it — the
                // desktop command answers the caller with the new `Workspace`
                // and no event, which is enough for one window but not for the
                // host's own window plus a phone.
                "remove_workspace_repo"
                | "attach_repo_to_project"
                | "detach_repo_from_project"
                | "set_repo_label"
                | "rename_project"
                | "relocate_repo" => crate::commands::announce_workspace(
                    ctx.sink.as_ref(),
                    project_settings_op(sup, op, args).await,
                ),

                // The read the Delete section polls while its confirm is up.
                "project_has_running_agents" => project_settings_op(sup, op, args).await,

                // Destructive, like `discard_agent` and `wf_delete_run`: the
                // project's agents, their checkouts and its workflow runs all
                // go. Needs the scheduler, so it is here rather than in
                // [`project_settings_op`], and the supervisor refuses while any
                // of the project's agents is running.
                "delete_project" => {
                    let a: ProjectArgs = parse(args)?;
                    let workflows = workflows(ctx)?;
                    crate::commands::announce_workspace(
                        ctx.sink.as_ref(),
                        res(sup.clone().delete_project(&workflows, &a.project_id).await),
                    )
                }

                // Remote-only: the phone streams a file's bytes, this Mac stages
                // it where a desktop paste lands (`attachments::remote`). The
                // path `attachment_end` answers with goes in `send_user_message`'s
                // `attachments`, and adoption at send time is the same code.
                "attachment_begin" => {
                    let a: AttachmentBeginArgs = parse(args)?;
                    res(crate::attachments::remote::begin(&a.name))
                }
                "attachment_chunk" => {
                    let a: AttachmentChunkArgs = parse(args)?;
                    res(crate::attachments::remote::append(&a.upload, &a.data))
                }
                "attachment_end" => {
                    let a: AttachmentUploadArgs = parse(args)?;
                    res(crate::attachments::remote::end(&a.upload))
                }
                "attachment_cancel" => {
                    let a: AttachmentUploadArgs = parse(args)?;
                    crate::attachments::remote::cancel(&a.upload);
                    Ok(Value::Null)
                }

                // The planning surface (protocol doc, "Planning chats from the
                // phone"). Purpose-scoped chats are absent from the workspace
                // snapshot, so this is the only listing that surfaces them.
                "list_project_chats" => {
                    let a: ProjectChatsArgs = parse(args)?;
                    ok(sup.project_chats(&a.project_id, &a.purpose))
                }

                // Read-only, and the one place the phone touches a table
                // directly: a custom agent is stored rows-only (no command
                // reads it back), and the phone needs the Project Manager
                // preset's id to spawn a planning chat against it. Same query
                // the desktop's `listCustomAgents` runs.
                "list_custom_agents" => {
                    let rows = {
                        let conn = ctx.db.lock();
                        crate::database::db_select(
                            &conn,
                            "custom_agents",
                            serde_json::json!({ "orderBy": "updated_at", "orderDirection": "desc" }),
                        )
                    };
                    res(rows)
                }

                // One agent by id, hidden records included — a phone opened
                // cold from a push notification knows only the id it carried,
                // and a planning chat is absent from `get_workspace`.
                "get_agent" => {
                    let a: AgentArgs = parse(args)?;
                    ok(sup.agent_record(&a.agent_id))
                }

                // ── Workflows (spec §13). Reads first, then run control, then
                // the stored-definition library. Every arm is the same `_impl`
                // the Tauri command calls, reached through the scheduler the
                // ctx published at boot.
                "wf_list_runs" => {
                    let a: RunsArgs = parse(args)?;
                    ok(crate::workflow::wf_list_runs_impl(a.project_id, &ctx.db).await?)
                }

                "wf_get_run" => {
                    let a: RunArgs = parse(args)?;
                    ok(crate::workflow::wf_get_run_impl(a.run_id, &ctx.db).await?)
                }

                "wf_events" => {
                    let a: WfEventsArgs = parse(args)?;
                    ok(
                        crate::workflow::wf_events_impl(a.run_id, a.after_seq, a.limit, &ctx.db)
                            .await?,
                    )
                }

                // A run's step agents are hidden from `get_workspace`, so the
                // monitor reads them here to render each attempt's chat.
                "wf_run_agents" => {
                    let a: RunArgs = parse(args)?;
                    ok(sup.run_agents(&a.run_id))
                }

                // `attachments` are host paths, exactly as `send_user_message`
                // takes them: a remote client uploads through `attachment_*`
                // first and passes what `attachment_end` answered.
                "wf_launch" => {
                    let a: WfLaunchArgs = parse(args)?;
                    ok(crate::workflow::scheduler::wf_launch_impl(
                        a.spec,
                        a.task,
                        a.project_id,
                        a.repo_path,
                        a.definition_id,
                        a.base_branch,
                        a.base_sha,
                        a.attachments,
                        a.issue_ref,
                        &workflows(ctx)?,
                        sup,
                    )
                    .await?)
                }

                "wf_cancel" => {
                    let a: RunArgs = parse(args)?;
                    ok(
                        crate::workflow::scheduler::wf_cancel_impl(a.run_id, &workflows(ctx)?)
                            .await?,
                    )
                }

                "wf_resume" => {
                    let a: WfResumeArgs = parse(args)?;
                    ok(crate::workflow::scheduler::wf_resume_impl(
                        a.run_id,
                        a.budget_patch,
                        &workflows(ctx)?,
                    )
                    .await?)
                }

                "wf_retry" => {
                    let a: RunArgs = parse(args)?;
                    ok(
                        crate::workflow::scheduler::wf_retry_impl(a.run_id, &workflows(ctx)?)
                            .await?,
                    )
                }

                "wf_approve" => {
                    let a: RunArgs = parse(args)?;
                    ok(
                        crate::workflow::scheduler::wf_approve_impl(a.run_id, &workflows(ctx)?)
                            .await?,
                    )
                }

                "wf_reject" => {
                    let a: WfRejectArgs = parse(args)?;
                    ok(crate::workflow::scheduler::wf_reject_impl(
                        a.run_id,
                        a.note,
                        &workflows(ctx)?,
                    )
                    .await?)
                }

                "wf_run_diff" => {
                    let a: WfRunDiffArgs = parse(args)?;
                    ok(crate::workflow::scheduler::wf_run_diff_impl(
                        a.run_id,
                        a.from_sha,
                        a.to_sha,
                        a.path,
                        &workflows(ctx)?,
                    )
                    .await?)
                }

                // `mode: "human"` says the conflict was resolved by hand in the
                // host's own integration worktree, so a remote client without a
                // shell there sends `"agent"`; the host rules on it either way.
                "wf_resolve_conflict" => {
                    let a: WfConflictArgs = parse(args)?;
                    ok(crate::workflow::scheduler::wf_resolve_conflict_impl(
                        a.run_id,
                        a.mode,
                        &workflows(ctx)?,
                    )
                    .await?)
                }

                // Destructive, like `discard_agent`: the run's step agents,
                // their chats, its directory and its rows all go. Same method
                // the command calls, so there is no softer remote variant.
                "wf_delete_run" => {
                    let a: RunArgs = parse(args)?;
                    ok(crate::workflow::scheduler::wf_delete_run_impl(
                        a.run_id,
                        &workflows(ctx)?,
                        sup,
                    )
                    .await?)
                }

                "wf_answer" => {
                    let a: WfAnswerArgs = parse(args)?;
                    ok(crate::workflow::comms::wf_answer_impl(
                        a.project_id,
                        a.run_id,
                        a.message_id,
                        a.body,
                        &workflows(ctx)?,
                    )
                    .await?)
                }

                "wf_def_save" => {
                    let a: WfDefSaveArgs = parse(args)?;
                    ok(
                        crate::workflow::definition::wf_def_save_impl(a.spec, a.id, a.hue, &ctx.db)
                            .await?,
                    )
                }

                "wf_def_list" => ok(crate::workflow::definition::wf_def_list_impl(&ctx.db).await?),

                "wf_def_delete" => {
                    let a: ItemIdArgs = parse(args)?;
                    ok(crate::workflow::definition::wf_def_delete_impl(a.id, &ctx.db).await?)
                }

                // Both YAML ops deal in *text*: picking the path and touching
                // the disk is the client's own business, on its own machine, so
                // neither needs a dialog on the host.
                "wf_def_export_yaml" => {
                    let a: ItemIdArgs = parse(args)?;
                    ok(crate::workflow::definition::wf_def_export_yaml_impl(a.id, &ctx.db).await?)
                }

                "wf_def_import_yaml" => {
                    let a: WfDefImportArgs = parse(args)?;
                    ok(
                        crate::workflow::definition::wf_def_import_yaml_impl(a.yaml_text, &ctx.db)
                            .await?,
                    )
                }

                // ── Roadmap. The board's whole surface: reads, item writes,
                // the brakes, and the PM's three proposal kinds.
                "roadmap_list_items" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_list_items_impl(a.project_id, &ctx.db)
                            .await?,
                    )
                }

                "roadmap_get_item" => {
                    let a: ItemArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_get_item_impl(a.item_id, &ctx.db).await?)
                }

                "roadmap_create_item" => {
                    let a: NewItemArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_create_item_impl(
                        a.project_id,
                        a.item,
                        ctx,
                        &ctx.db,
                    )
                    .await?)
                }

                // Accepting a proposal is an `open` patch guarded by
                // `expectStatus: "proposed"`, so two clients racing on the same
                // ghost row can't both accept it.
                "roadmap_update_item" => {
                    let a: RoadmapUpdateArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_update_item_impl(
                        a.id,
                        a.patch,
                        a.expect_status,
                        a.queue,
                        ctx,
                        &ctx.db,
                    )
                    .await?)
                }

                "roadmap_set_rank" => {
                    let a: RankArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_set_rank_impl(
                        a.item_id, a.rank, ctx, &ctx.db,
                    )
                    .await?)
                }

                "roadmap_hand_off_item" => {
                    let a: HandOffArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_hand_off_item_impl(
                        a.item_id, a.agent_id, ctx, &ctx.db,
                    )
                    .await?)
                }

                "roadmap_item_review" => {
                    let a: ItemArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_item_review_impl(a.item_id, &ctx.db)
                            .await?,
                    )
                }

                // The same credential `push_agent` and `create_pr` already
                // spend, through the same host path `merge_pr` uses.
                "roadmap_merge_item_pr" => {
                    let a: ItemArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_merge_item_pr_impl(a.item_id, &ctx.db)
                            .await?,
                    )
                }

                "roadmap_note_review_feedback" => {
                    let a: ReviewFeedbackArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_note_review_feedback_impl(
                        a.item_id, a.threads, ctx, &ctx.db,
                    )
                    .await?)
                }

                "roadmap_hold_item" => {
                    let a: ItemReasonArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_hold_item_impl(
                        a.item_id, a.reason, ctx, &ctx.db,
                    )
                    .await?)
                }

                // Releasing a hold is the user's alone — the PM has an op to
                // hold and none to release — and a paired client is the user.
                "roadmap_release_item" => {
                    let a: ItemArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_release_item_impl(
                            a.item_id, ctx, &ctx.db,
                        )
                        .await?,
                    )
                }

                "roadmap_hold_project" => {
                    let a: ProjectReasonArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_hold_project_impl(
                        a.project_id,
                        a.reason,
                        ctx,
                        &ctx.db,
                    )
                    .await?)
                }

                "roadmap_release_project" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_release_project_impl(
                        a.project_id,
                        ctx,
                        &ctx.db,
                    )
                    .await?)
                }

                "roadmap_get_project_hold" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_get_project_hold_impl(
                        a.project_id,
                        &ctx.db,
                    )
                    .await?)
                }

                "roadmap_reclaim_item" => {
                    let a: ItemArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_reclaim_item_impl(
                            a.item_id, ctx, &ctx.db,
                        )
                        .await?,
                    )
                }

                "roadmap_reject_item" => {
                    let a: ItemReasonArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_reject_item_impl(
                        a.item_id, a.reason, ctx, &ctx.db,
                    )
                    .await?)
                }

                "roadmap_reopen_item" => {
                    let a: ItemArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_reopen_item_impl(a.item_id, ctx, &ctx.db)
                            .await?,
                    )
                }

                // The board's own Remove: unconditional, and destructive like
                // `discard_agent` and `wf_delete_run`. It is on the wire so the
                // one `roadmap` capability gate covers every board control
                // rather than leaving this one to fail as `unknown op`.
                "roadmap_delete_item" => {
                    let a: ItemIdArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_delete_item_impl(a.id, ctx, &ctx.db)
                            .await?,
                    )
                }

                // Discard, not delete: the phone may have held the card since
                // before another client accepted it, and a stale discard must
                // not take a row that is already being worked. `applied: false`
                // comes back with the row as it now stands.
                "roadmap_discard_proposal" => {
                    let a: ItemIdArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_discard_proposal_impl(a.id, ctx, &ctx.db)
                            .await?,
                    )
                }

                "roadmap_list_item_events" => {
                    let a: ItemArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_list_item_events_impl(a.item_id, &ctx.db)
                            .await?,
                    )
                }

                "roadmap_latest_events" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_latest_events_impl(a.project_id, &ctx.db)
                            .await?,
                    )
                }

                "roadmap_list_proposals" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_list_proposals_impl(
                            a.project_id,
                            &ctx.db,
                        )
                        .await?,
                    )
                }

                "roadmap_accept_proposal" => {
                    let a: ProposalArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_accept_proposal_impl(
                        a.proposal_id,
                        ctx,
                        &ctx.db,
                    )
                    .await?)
                }

                "roadmap_reject_proposal" => {
                    let a: ProposalArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_reject_proposal_impl(
                        a.proposal_id,
                        ctx,
                        &ctx.db,
                    )
                    .await?)
                }

                "roadmap_get_order_proposal" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_get_order_proposal_impl(
                        a.project_id,
                        &ctx.db,
                    )
                    .await?)
                }

                "roadmap_accept_order_proposal" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_accept_order_proposal_impl(
                            a.project_id,
                            ctx,
                            &ctx.db,
                        )
                        .await?,
                    )
                }

                "roadmap_reject_order_proposal" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_reject_order_proposal_impl(
                            a.project_id,
                            ctx,
                            &ctx.db,
                        )
                        .await?,
                    )
                }

                "roadmap_get_brief" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_get_brief_impl(a.project_id, &ctx.db)
                            .await?,
                    )
                }

                "roadmap_get_brief_proposal" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(crate::roadmap::commands::roadmap_get_brief_proposal_impl(
                        a.project_id,
                        &ctx.db,
                    )
                    .await?)
                }

                "roadmap_accept_brief_proposal" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_accept_brief_proposal_impl(
                            a.project_id,
                            ctx,
                            &ctx.db,
                        )
                        .await?,
                    )
                }

                "roadmap_reject_brief_proposal" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_reject_brief_proposal_impl(
                            a.project_id,
                            ctx,
                            &ctx.db,
                        )
                        .await?,
                    )
                }

                // Unreachable while `OPS` and the arms above agree; kept so a
                // name added to one and not the other fails closed.
                _ => Err(UNKNOWN_OP.to_string()),
            }
        })
    }
}

/// The "add a project" ops (protocol doc, "Adding a project from the phone"):
/// browse the Mac's folders, pin one, or clone a GitHub repo into one. Split out
/// of the match above because none of them needs the engine ctx to do its work
/// — a bare `Supervisor` is the whole host state they touch, which is what lets
/// the remote tests dispatch them for real without a Tauri app. The
/// `workspace:changed` the two mutating ones owe everyone else is added by the
/// caller, which has the sink.
pub(super) async fn add_project_op(sup: &Supervisor, op: &str, args: Value) -> DispatchResult {
    match op {
        "list_dir" => {
            let a: PathArgs = parse(args)?;
            res(crate::commands::list_dir_impl(a.path).await)
        }

        "add_workspace_repo" => {
            let a: RepoPathArgs = parse(args)?;
            res(crate::commands::add_workspace_repo_impl(sup, a.repo_path).await)
        }

        "clone_repo" => {
            let a: CloneArgs = parse(args)?;
            res(crate::commands::clone_repo_impl(sup, &a.spec, &a.dest_parent).await)
        }

        // `publish` decides whether GitHub is involved at all: false creates a
        // local-only repo, which is what a host with no `gh` connection can
        // still do. Absent means publish, as the command's default does.
        "create_repo" => {
            let a: CreateRepoArgs = parse(args)?;
            res(crate::commands::create_repo_impl(
                sup,
                &a.name,
                &a.dest_parent,
                a.private,
                a.description.as_deref(),
                a.publish.unwrap_or(true),
            )
            .await)
        }

        // Both take no arguments; calling the commands themselves keeps the
        // repo-list cap in one place.
        "gh_status" => res(crate::commands::gh_status_impl().await),
        "gh_repo_list" => res(crate::commands::gh_repo_list_impl().await),

        _ => Err(UNKNOWN_OP.to_string()),
    }
}

/// The project settings page's ops (protocol doc, "Project settings from a
/// remote client"): rename and delete a project, attach/detach/relocate a repo,
/// label one, unpin one. Split out of the match above for the same reason
/// [`add_project_op`] is — none of them needs the engine ctx, so the remote
/// tests drive them against a bare `Supervisor`. `delete_project` is the one
/// that stays in the match: it needs the workflow scheduler. The
/// `workspace:changed` the mutating ones owe everyone else is added by the
/// caller, which has the sink.
pub(super) async fn project_settings_op(sup: &Supervisor, op: &str, args: Value) -> DispatchResult {
    match op {
        "remove_workspace_repo" => {
            let a: RepoPathArgs = parse(args)?;
            res(sup.remove_workspace_repo(PathBuf::from(a.repo_path)))
        }

        // Two-phase (DB, then the folder) with a rollback, exactly as the
        // desktop command runs it.
        "attach_repo_to_project" => {
            let a: ProjectRepoArgs = parse(args)?;
            res(crate::commands::attach_repo_to_project_impl(sup, &a.project_id, a.repo_path).await)
        }

        // Guarded supervisor-side: a project's last repo, and any repo an agent
        // checkout still references, are both refused.
        "detach_repo_from_project" => {
            let a: ProjectRepoArgs = parse(args)?;
            res(sup.detach_repo_from_project(&a.project_id, PathBuf::from(a.repo_path)))
        }

        "set_repo_label" => {
            let a: RepoLabelArgs = parse(args)?;
            res(sup.set_repo_label(PathBuf::from(a.repo_path), &a.label))
        }

        "rename_project" => {
            let a: RenameProjectArgs = parse(args)?;
            res(sup.rename_project(&a.project_id, &a.name))
        }

        // Repoints a pinned repo at a folder the user moved *on the host*; it
        // validates the destination is a git repo there and touches neither
        // folder.
        "relocate_repo" => {
            let a: RelocateArgs = parse(args)?;
            res(sup.relocate_repo(PathBuf::from(a.old_path), PathBuf::from(a.new_path)))
        }

        "project_has_running_agents" => {
            let a: ProjectArgs = parse(args)?;
            ok(sup.project_has_running_agents(&a.project_id))
        }

        _ => Err(UNKNOWN_OP.to_string()),
    }
}

/// The one `purpose` a phone may set on a spawn: a Roadmap PM chat, the single
/// purpose-scoped surface the remote protocol exposes. Every other value is
/// dropped rather than refused — `purpose` hides a workspace from the sidebar
/// and narrows its capability grant, so an unknown one from a future client
/// would strand an agent in a surface this host does not have.
fn remote_purpose(purpose: Option<String>) -> Option<String> {
    purpose.filter(|p| p == crate::workspace::PURPOSE_ROADMAP_PM)
}

/// The scheduler the `wf_*` run-control arms need, or [`WORKFLOWS_UNAVAILABLE`].
/// The Tauri commands read the same service out of `State`, where boot having
/// published it is equally assumed.
fn workflows(
    ctx: &EngineCtx,
) -> std::result::Result<Arc<crate::workflow::scheduler::WorkflowService>, String> {
    ctx.workflows()
        .ok_or_else(|| WORKFLOWS_UNAVAILABLE.to_string())
}

fn parse<T: DeserializeOwned>(args: Value) -> std::result::Result<T, String> {
    serde_json::from_value(args).map_err(|e| e.to_string())
}

fn ok<T: serde::Serialize>(value: T) -> DispatchResult {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// Commands answer with `crate::error::Result`; the wire carries the `Display`
/// of the error, per the protocol doc.
fn res<T: serde::Serialize>(result: crate::error::Result<T>) -> DispatchResult {
    ok(result.map_err(|e| e.to_string())?)
}

// ---------------------------------------------------------------------------
// Argument shapes. Keys match `src/api/domains/*.ts` exactly; unknown keys are
// ignored, which is what lets `spawn_agent` accept (and drop) the fields v1
// does not expose.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentArgs {
    agent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSubdirArgs {
    agent_id: String,
    #[serde(default)]
    subdir: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoPathArgs {
    repo_path: String,
}

#[derive(Deserialize)]
struct PathArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloneArgs {
    spec: String,
    dest_parent: String,
}

/// `private` is the wire key the desktop's `createRepo` already sends (the
/// command's own parameter name), so it is not renamed here.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRepoArgs {
    name: String,
    dest_parent: String,
    private: bool,
    #[serde(default)]
    description: Option<String>,
    /// Absent publishes, as the command's default does.
    #[serde(default)]
    publish: Option<bool>,
}

/// One repo of one project: attach and detach both address a repo *within* a
/// project, so both keys are required.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectRepoArgs {
    project_id: String,
    repo_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoLabelArgs {
    repo_path: String,
    /// Blank clears back to the folder-basename fallback.
    label: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameProjectArgs {
    project_id: String,
    name: String,
}

/// Both paths are on the *host*: the folder was moved there, not here.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelocateArgs {
    old_path: String,
    new_path: String,
}

#[derive(Deserialize)]
struct DraftsArgs {
    #[serde(default)]
    drafts: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnArgs {
    repo_path: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    fork_base: Option<String>,
    #[serde(default)]
    issue_ref: Option<String>,
    /// The custom-agent preset this chat runs as, from `list_custom_agents`.
    #[serde(default)]
    custom_agent_id: Option<String>,
    /// Honoured only for `PURPOSE_ROADMAP_PM`; see [`remote_purpose`].
    #[serde(default)]
    purpose: Option<String>,
}

/// One project's purpose-scoped chats (`workspace::PURPOSE_ROADMAP_PM`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectChatsArgs {
    project_id: String,
    purpose: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectArgs {
    project_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RoadmapUpdateArgs {
    id: String,
    patch: crate::roadmap::types::ItemPatch,
    /// The status the caller believes the item is in; the write is a no-op
    /// (`applied: false`) when it has moved on.
    #[serde(default)]
    expect_status: Option<crate::roadmap::types::ItemStatus>,
    #[serde(default)]
    queue: Option<bool>,
}

/// `{ id }`, shared by the two roadmap deletes and the three `wf_def_*` ops
/// that address a stored definition — all of which the frontend calls with a
/// bare `id`.
#[derive(Deserialize)]
struct ItemIdArgs {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemArgs {
    item_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemReasonArgs {
    item_id: String,
    reason: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectReasonArgs {
    project_id: String,
    reason: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProposalArgs {
    proposal_id: String,
}

/// The `code` is allocated host-side under the connection lock, which is why it
/// is absent from the payload.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewItemArgs {
    project_id: String,
    item: crate::roadmap::types::NewItem,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RankArgs {
    item_id: String,
    rank: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HandOffArgs {
    item_id: String,
    agent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewFeedbackArgs {
    item_id: String,
    threads: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunArgs {
    run_id: String,
}

/// `wf_list_runs` scopes to one project or lists every run; the frontend sends
/// the key present-and-null for the unscoped call.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunsArgs {
    #[serde(default)]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WfEventsArgs {
    run_id: String,
    after_seq: i64,
    limit: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WfLaunchArgs {
    spec: crate::workflow::spec::Spec,
    task: String,
    project_id: String,
    repo_path: String,
    #[serde(default)]
    definition_id: Option<String>,
    #[serde(default)]
    base_branch: Option<String>,
    #[serde(default)]
    base_sha: Option<String>,
    /// Host paths, as `send_user_message` takes them: from `attachment_end`.
    #[serde(default)]
    attachments: Vec<String>,
    #[serde(default)]
    issue_ref: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WfResumeArgs {
    run_id: String,
    /// Additively raises the run-level caps before re-driving; absent resumes
    /// on the budget the run already has.
    #[serde(default)]
    budget_patch: Option<crate::workflow::spec::Budgets>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WfRejectArgs {
    run_id: String,
    note: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WfRunDiffArgs {
    run_id: String,
    from_sha: String,
    to_sha: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WfConflictArgs {
    run_id: String,
    /// `"agent"` or `"human"`; the host rules on anything else.
    mode: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WfAnswerArgs {
    project_id: String,
    run_id: String,
    message_id: String,
    body: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WfDefSaveArgs {
    spec: crate::workflow::spec::Spec,
    /// Absent creates; an existing id edits in place.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    hue: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WfDefImportArgs {
    yaml_text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendMessageArgs {
    agent_id: String,
    turn_id: String,
    text: String,
    #[serde(default)]
    attachments: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnswerToolUseArgs {
    agent_id: String,
    request_id: String,
    updated_input: Value,
    behavior: ToolUseBehavior,
    #[serde(default)]
    message: Option<String>,
}

/// One answer to a `publish:approval-requested` prompt; `id` is the one the
/// event carried.
#[derive(Deserialize)]
struct PublishApprovalArgs {
    id: String,
    approved: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelArgs {
    agent_id: String,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EffortArgs {
    agent_id: String,
    #[serde(default)]
    effort: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileArgs {
    agent_id: String,
    path: String,
    #[serde(default)]
    base_mode: Option<DiffBaseMode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitArgs {
    agent_id: String,
    message: String,
    #[serde(default)]
    subdir: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatePrArgs {
    agent_id: String,
    title: String,
    body: String,
    #[serde(default)]
    subdir: Option<String>,
}

/// Opens a phone upload; `name` is the display filename (path components are
/// stripped on the host). See `attachments::remote`.
#[derive(Deserialize)]
struct AttachmentBeginArgs {
    name: String,
}

#[derive(Deserialize)]
struct AttachmentUploadArgs {
    upload: String,
}

/// One chunk of a phone upload: the file's next bytes, base64.
#[derive(Deserialize)]
struct AttachmentChunkArgs {
    upload: String,
    data: String,
}

#[cfg(test)]
mod arg_tests {
    use super::*;
    use serde_json::json;

    /// A phone's planning chat is the one spawn that names a purpose, and
    /// `roadmap-pm` is the only surface the protocol exposes: anything else —
    /// a purpose a later desktop adds, or a typo — is dropped, so the spawn
    /// lands in the sidebar rather than in a surface the phone cannot list.
    #[test]
    fn only_the_roadmap_pm_purpose_survives_the_wire() {
        assert_eq!(
            remote_purpose(Some(crate::workspace::PURPOSE_ROADMAP_PM.to_string())),
            Some("roadmap-pm".to_string())
        );
        assert_eq!(remote_purpose(Some("something-else".into())), None);
        assert_eq!(remote_purpose(None), None);
    }

    /// The mobile client sends the desktop's full `spawn_agent` payload, with
    /// the fields v1 does not expose present but null. They must deserialize
    /// away rather than fail the request.
    #[test]
    fn spawn_args_accept_the_full_desktop_payload() {
        let args: SpawnArgs = parse(json!({
            "view": "custom",
            "repoPath": "/Users/alex/code/thing",
            "provider": "claude",
            "name": "arabia",
            "effort": null,
            "model": null,
            "instructions": null,
            "customAgentId": null,
            "skills": null,
            "mcpServers": null,
            "forkBase": "main",
            "issueRef": null,
            "purpose": null,
        }))
        .expect("full desktop payload");
        assert_eq!(args.repo_path, "/Users/alex/code/thing");
        assert_eq!(args.provider.as_deref(), Some("claude"));
        assert_eq!(args.name.as_deref(), Some("arabia"));
        assert_eq!(args.fork_base.as_deref(), Some("main"));
        assert!(args.instructions.is_none());
        assert!(args.issue_ref.is_none());
        assert!(args.custom_agent_id.is_none());
        assert!(args.purpose.is_none());

        // The planning-chat payload: the preset and the purpose both ride
        // through to the spawn.
        let pm: SpawnArgs = parse(json!({
            "repoPath": "/Users/alex/code/thing",
            "customAgentId": "ca-1",
            "purpose": "roadmap-pm",
            "instructions": "You are the Project Manager.",
        }))
        .expect("planning-chat payload");
        assert_eq!(pm.custom_agent_id.as_deref(), Some("ca-1"));
        assert_eq!(remote_purpose(pm.purpose).as_deref(), Some("roadmap-pm"));
    }

    #[test]
    fn spawn_args_need_only_a_repo_path() {
        let args: SpawnArgs = parse(json!({ "repoPath": "/tmp/repo" })).expect("minimal payload");
        assert!(args.provider.is_none());
        assert!(
            parse::<SpawnArgs>(json!({})).is_err(),
            "repoPath is required"
        );
    }

    #[test]
    fn message_and_tool_use_args_use_the_command_keys() {
        let msg: SendMessageArgs = parse(json!({
            "agentId": "arabia", "turnId": "t-1", "text": "go", "attachments": []
        }))
        .unwrap();
        assert_eq!(msg.turn_id, "t-1");
        assert!(msg.attachments.is_empty());
        // `attachments` omitted is the same as empty — a phone on an older
        // build, from before it had a picker, still sends without the key.
        assert!(parse::<SendMessageArgs>(json!({
            "agentId": "arabia", "turnId": "t-1", "text": "go"
        }))
        .unwrap()
        .attachments
        .is_empty());
        let with: SendMessageArgs = parse(json!({
            "agentId": "arabia", "turnId": "t-2", "text": "",
            "attachments": ["/Users/alex/Library/Application Support/x/attachments/u1/shot.png"]
        }))
        .unwrap();
        assert_eq!(with.attachments.len(), 1);

        let tool: AnswerToolUseArgs = parse(json!({
            "agentId": "arabia",
            "requestId": "req-9",
            "updatedInput": { "command": "ls" },
            "behavior": "allow",
            "message": null,
        }))
        .unwrap();
        assert_eq!(tool.request_id, "req-9");
        assert_eq!(tool.behavior, ToolUseBehavior::Allow);
        assert!(tool.message.is_none());
    }

    #[test]
    fn publish_approval_args_need_both_the_id_and_the_verdict() {
        let a: PublishApprovalArgs = parse(json!({ "id": "r1", "approved": false })).unwrap();
        assert_eq!(a.id, "r1");
        assert!(!a.approved);
        assert!(parse::<PublishApprovalArgs>(json!({ "id": "r1" })).is_err());
        assert!(parse::<PublishApprovalArgs>(json!({ "approved": true })).is_err());
    }

    #[test]
    fn file_args_take_base_mode_or_leave_it_to_the_command_default() {
        let bare: FileArgs = parse(json!({ "agentId": "a", "path": "src/lib.rs" })).unwrap();
        assert!(bare.base_mode.is_none());
        let head: FileArgs =
            parse(json!({ "agentId": "a", "path": "src/lib.rs", "baseMode": "head" })).unwrap();
        assert!(matches!(head.base_mode, Some(DiffBaseMode::Head)));
    }

    #[test]
    fn attachment_args_use_the_upload_keys() {
        let begin: AttachmentBeginArgs = parse(json!({ "name": "IMG_0001.jpeg" })).unwrap();
        assert_eq!(begin.name, "IMG_0001.jpeg");
        assert!(
            parse::<AttachmentBeginArgs>(json!({})).is_err(),
            "name is required"
        );
        let chunk: AttachmentChunkArgs =
            parse(json!({ "upload": "u1", "data": "AAEA/w==" })).unwrap();
        assert_eq!(
            (chunk.upload.as_str(), chunk.data.as_str()),
            ("u1", "AAEA/w==")
        );
        let end: AttachmentUploadArgs = parse(json!({ "upload": "u1" })).unwrap();
        assert_eq!(end.upload, "u1");
    }

    #[test]
    fn subdir_is_optional_on_the_repo_scoped_ops() {
        let bare: AgentSubdirArgs = parse(json!({ "agentId": "a" })).unwrap();
        assert!(bare.subdir.is_none());
        let explicit: AgentSubdirArgs = parse(json!({ "agentId": "a", "subdir": null })).unwrap();
        assert!(explicit.subdir.is_none());
    }
}

#[cfg(test)]
mod dictation_tests {
    use super::*;
    use crate::host::ctx::test_ctx;
    use crate::supervisor::Supervisor;
    use crate::workspace::WorkspaceManager;
    use serde_json::json;

    fn host_without_a_speech_engine() -> (SupervisorDispatch, tempfile::TempDir) {
        let (ctx, _sink, dir) = test_ctx();
        let sup = Arc::new(Supervisor::new(Arc::new(WorkspaceManager::new(
            ctx.db.clone(),
        ))));
        (SupervisorDispatch::new(ctx, sup), dir)
    }

    /// The five names stay on the wire whatever the host is: the protocol
    /// descriptor advertises them, so a phone must be able to ask and get a
    /// real answer rather than `unknown op`.
    #[test]
    fn the_five_ops_are_on_the_allowlist_with_or_without_an_engine() {
        for op in DICTATION_OPS {
            assert!(OPS.contains(op), "{op} left the allowlist");
        }
    }

    /// A host with no local speech engine — the headless Linux case, and the
    /// non-macOS desktop build — answers the probe instead of erroring, so the
    /// phone hides its mic button the way it always has.
    #[tokio::test]
    async fn status_reports_unavailable_when_no_engine_is_attached() {
        let (dispatch, _dir) = host_without_a_speech_engine();
        let reply = dispatch
            .dispatch("dictation_status", json!({}))
            .await
            .expect("status is always answerable");
        assert_eq!(
            reply,
            json!({ "available": false, "reason": DICTATION_UNAVAILABLE })
        );
    }

    /// The four capture ops fail with the words the non-macOS build has always
    /// used, rather than `unknown op` (which would read as a protocol
    /// mismatch) or a panic.
    #[tokio::test]
    async fn the_capture_ops_fail_with_the_unavailable_message() {
        let (dispatch, _dir) = host_without_a_speech_engine();
        for op in ["dictation_begin", "dictation_audio", "dictation_end"] {
            assert_eq!(
                dispatch.dispatch(op, json!({ "session": "s" })).await,
                Err(DICTATION_UNAVAILABLE.to_string()),
                "{op}"
            );
        }
        assert_eq!(
            dispatch.dispatch("dictation_cancel", json!({})).await,
            Err(DICTATION_UNAVAILABLE.to_string())
        );
    }

    /// With an engine attached the five route straight to it — the desktop's
    /// whisper dispatcher, here a stub that just names itself.
    #[tokio::test]
    async fn an_attached_engine_answers_all_five() {
        struct Stub;
        impl Dispatch for Stub {
            fn dispatch<'a>(&'a self, op: &'a str, _args: Value) -> DispatchFuture<'a> {
                Box::pin(async move { Ok(Value::String(op.to_string())) })
            }
        }

        let (dispatch, _dir) = host_without_a_speech_engine();
        let dispatch = dispatch.with_dictation(Arc::new(Stub));
        for op in DICTATION_OPS {
            assert_eq!(
                dispatch.dispatch(op, json!({})).await,
                Ok(Value::String((*op).to_string())),
                "{op}"
            );
        }
    }
}

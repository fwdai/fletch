//! The remote op allowlist.
//!
//! Every arm deserializes the same camelCase argument keys the matching Tauri
//! command receives from `invoke`, then calls the same supervisor/service
//! function that command calls — so a phone and the desktop webview cannot
//! drift apart in behaviour. Nothing outside [`OPS`] is reachable: `db_*`, the
//! file mutations, the shell ops, raw PTY input, editor/log/telemetry/provider
//! ops and the workflow/run surfaces are all off the wire by construction — as
//! is every roadmap op but the three a phone's planning chat needs (read a
//! project's items, accept a proposal, discard one).
//!
//! Adding an op is: a row in `docs/remote-protocol.md`, a name in [`OPS`], and
//! one match arm here — unless it needs to know *which device* is asking, in
//! which case it goes in [`SESSION_OPS`] and is answered by `server`.

use std::future::Future;
use std::path::Path;
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
    "gh_status",
    "gh_repo_list",
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
    "roadmap_list_items",
    "roadmap_update_item",
    "roadmap_discard_proposal",
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

                // The two that change the project list also tell every other
                // view about it, exactly as the Tauri commands do.
                "add_workspace_repo" | "clone_repo" => crate::commands::announce_workspace(
                    ctx.sink.as_ref(),
                    add_project_op(sup, op, args).await,
                ),

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

                "roadmap_list_items" => {
                    let a: ProjectArgs = parse(args)?;
                    ok(
                        crate::roadmap::commands::roadmap_list_items_impl(a.project_id, &ctx.db)
                            .await?,
                    )
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

        // Both take no arguments; calling the commands themselves keeps the
        // repo-list cap in one place.
        "gh_status" => res(crate::commands::gh_status_impl().await),
        "gh_repo_list" => res(crate::commands::gh_repo_list_impl().await),

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

#[derive(Deserialize)]
struct ItemIdArgs {
    id: String,
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

//! The remote op allowlist.
//!
//! Every arm deserializes the same camelCase argument keys the matching Tauri
//! command receives from `invoke`, then calls the same supervisor/service
//! function that command calls — so a phone and the desktop webview cannot
//! drift apart in behaviour. Nothing outside [`OPS`] is reachable: `db_*`, the
//! file mutations, the shell ops, raw PTY input, editor/log/telemetry/provider
//! ops and the workflow/roadmap/run surfaces are all off the wire by
//! construction.
//!
//! Adding an op is: a row in `docs/remote-protocol.md`, a name in [`OPS`], and
//! one match arm here.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use tauri::AppHandle;

use crate::commands::DiffBaseMode;
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

/// The error text a connection gets when it already has the per-connection
/// maximum of requests outstanding (`server::MAX_IN_FLIGHT`). The request is
/// answered without being dispatched; the client retries.
pub const TOO_MANY_IN_FLIGHT: &str = "too many in-flight requests";

/// The v1 allowlist, in the order of the protocol doc's table. Load-bearing:
/// `dispatch` rejects anything absent here before the `match` runs, so a name
/// is only reachable when it appears both here and as an arm.
pub const OPS: &[&str] = &[
    "get_workspace",
    "allocate_draft_name",
    "spawn_agent",
    "send_user_message",
    "answer_tool_use",
    "stop_agent",
    "resume_agent",
    "archive_agent",
    "set_agent_model",
    "set_agent_effort",
    "read_session_records",
    "read_user_turns",
    "get_git_state",
    "get_agent_diff_stats",
    "list_checkout_tree",
    "read_checkout_file",
    "get_file_diff",
    "commit_agent",
    "push_agent",
    "create_pr",
    "get_pr_state",
    "get_pr_checks",
    "get_pr_live",
    "list_repo_branches",
    "repo_default_branch",
    "discover_supported_models",
];

pub fn is_allowed(op: &str) -> bool {
    OPS.contains(&op)
}

/// The production dispatcher: the supervisor and app handle the Tauri commands
/// would have received through `State`/`AppHandle`.
pub struct SupervisorDispatch {
    app: AppHandle,
    sup: Arc<Supervisor>,
}

impl SupervisorDispatch {
    pub fn new(app: AppHandle, sup: Arc<Supervisor>) -> Self {
        Self { app, sup }
    }
}

impl Dispatch for SupervisorDispatch {
    fn dispatch<'a>(&'a self, op: &'a str, args: Value) -> DispatchFuture<'a> {
        Box::pin(async move {
            if !is_allowed(op) {
                return Err(UNKNOWN_OP.to_string());
            }
            let sup = &self.sup;
            let app = &self.app;
            match op {
                "get_workspace" => ok(sup.current_workspace()),

                "allocate_draft_name" => {
                    let a: DraftsArgs = parse(args)?;
                    res(crate::commands::allocate_draft_name_impl(sup, a.drafts))
                }

                // v1 forces the structured chat view and drops the custom-agent
                // fields: the phone has no UI for a native PTY, and skills / MCP
                // servers / a custom agent id are resolved by value from desktop
                // storage the phone cannot see.
                "spawn_agent" => {
                    let a: SpawnArgs = parse(args)?;
                    res(crate::commands::spawn_agent_impl(
                        sup.clone(),
                        app.clone(),
                        Some(AgentView::Custom),
                        a.repo_path,
                        a.provider,
                        a.name,
                        a.effort,
                        a.model,
                        a.instructions,
                        None,
                        None,
                        None,
                        a.fork_base,
                        a.issue_ref,
                        None,
                    )
                    .await)
                }

                "send_user_message" => {
                    let a: SendMessageArgs = parse(args)?;
                    res(sup.clone().send_user_message(
                        app,
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

                "stop_agent" => {
                    let a: AgentArgs = parse(args)?;
                    res(sup.clone().stop_agent(app.clone(), &a.agent_id).await)
                }

                "resume_agent" => {
                    let a: AgentArgs = parse(args)?;
                    res(sup.clone().resume_agent(app.clone(), &a.agent_id).await)
                }

                "archive_agent" => {
                    let a: AgentArgs = parse(args)?;
                    res(sup.clone().archive_agent(app.clone(), &a.agent_id).await)
                }

                "set_agent_model" => {
                    let a: ModelArgs = parse(args)?;
                    res(sup
                        .set_agent_model(app, &a.agent_id, a.model.as_deref())
                        .await)
                }

                "set_agent_effort" => {
                    let a: EffortArgs = parse(args)?;
                    res(sup
                        .set_agent_effort(app, &a.agent_id, a.effort.as_deref())
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

                "get_git_state" => {
                    let a: AgentSubdirArgs = parse(args)?;
                    res(
                        crate::commands::get_git_state_impl(sup, &a.agent_id, a.subdir.as_deref())
                            .await,
                    )
                }

                "get_agent_diff_stats" => {
                    let a: AgentArgs = parse(args)?;
                    res(crate::commands::get_agent_diff_stats_impl(sup, a.agent_id).await)
                }

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
                        app.clone(),
                        a.agent_id,
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

                // Unreachable while `OPS` and the arms above agree; kept so a
                // name added to one and not the other fails closed.
                _ => Err(UNKNOWN_OP.to_string()),
            }
        })
    }
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

#[cfg(test)]
mod arg_tests {
    use super::*;
    use serde_json::json;

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
        // `attachments` omitted is the same as empty — the phone has no
        // attachment picker in v1.
        assert!(parse::<SendMessageArgs>(json!({
            "agentId": "arabia", "turnId": "t-1", "text": "go"
        }))
        .unwrap()
        .attachments
        .is_empty());

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
    fn file_args_take_base_mode_or_leave_it_to_the_command_default() {
        let bare: FileArgs = parse(json!({ "agentId": "a", "path": "src/lib.rs" })).unwrap();
        assert!(bare.base_mode.is_none());
        let head: FileArgs =
            parse(json!({ "agentId": "a", "path": "src/lib.rs", "baseMode": "head" })).unwrap();
        assert!(matches!(head.base_mode, Some(DiffBaseMode::Head)));
    }

    #[test]
    fn subdir_is_optional_on_the_repo_scoped_ops() {
        let bare: AgentSubdirArgs = parse(json!({ "agentId": "a" })).unwrap();
        assert!(bare.subdir.is_none());
        let explicit: AgentSubdirArgs = parse(json!({ "agentId": "a", "subdir": null })).unwrap();
        assert!(explicit.subdir.is_none());
    }
}

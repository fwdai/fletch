//! Shared, non-provider-specific CLI arg-building helpers.

use std::path::Path;

use crate::instructions;

use super::spawn::{SessionStart, SpawnSpec};

/// Claude's session-level effort flag (`--effort <level>`), shared by the
/// managed (custom-view) and PTY (native-view) arg builders. Empty when no
/// effort was selected for the session, so claude falls back to its own
/// default. Effort is a spawn-time flag for the whole session, not per-turn
/// (unlike the per-turn agents' `thinking` arg) — see `providerDetail.ts`.
pub(crate) fn effort_args(effort: Option<&str>) -> Vec<String> {
    match effort {
        Some(level) => vec!["--effort".into(), level.to_string()],
        None => Vec::new(),
    }
}

pub(crate) fn model_args(model: Option<&str>) -> Vec<String> {
    match model {
        Some(id) if !id.trim().is_empty() => vec!["--model".into(), id.to_string()],
        _ => Vec::new(),
    }
}

/// Append `flag` then `value` to `args` when `value` is `Some` — the
/// `<flag> <id>` resume/session pattern every per-turn arg builder repeats.
/// `flag` is a positional subcommand for codex (`resume`) or a `--…` option for
/// the others; either way it's two tokens.
pub(crate) fn push_opt(args: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(v) = value {
        args.push(flag.to_string());
        args.push(v.to_string());
    }
}

/// Claude's `--agents <json>` for the Fletch-defined `codegraph` subagent.
/// Empty unless codegraph actually landed — a subagent whose entire job is
/// calling `codegraph_explore` is worse than useless without the tool.
///
/// `--agents` *adds* here; it can override a built-in by name, but we
/// deliberately don't (see `agent_profile::codegraph_subagent_json`).
pub(crate) fn subagent_args(codegraph_available: bool) -> Vec<String> {
    if !codegraph_available {
        return Vec::new();
    }
    vec![
        "--agents".into(),
        crate::agent_profile::codegraph_subagent_json(),
    ]
}

/// Claude's session flags, shared by the managed and PTY builders.
fn session_args(start: SessionStart, session_id: &str) -> Vec<String> {
    let flag = match start {
        SessionStart::Fresh => "--session-id",
        SessionStart::Resume => "--resume",
    };
    vec![flag.into(), session_id.to_string()]
}

/// `mcp_args` comes from the provider's `McpDeliveryBuilder`, resolved and run
/// by the spawn path (see `agent::mcp_delivery`) — for claude that's
/// `--mcp-config <path> --strict-mcp-config`. Empty when no servers are
/// attached.
pub(crate) fn prepare_pty_args(spec: &SpawnSpec<'_>, mcp_args: &[String]) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--dangerously-skip-permissions".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
    ];
    args.extend(effort_args(spec.effort));
    args.extend(model_args(spec.model));
    // The text itself, not the file the custom view reads: a TUI that took
    // the file form for print mode only would drop it unseen (`instructions`).
    args.extend(instructions::append_system_prompt_args(spec.instructions));
    args.extend(crate::attribution::claude_settings_args());
    args.extend_from_slice(mcp_args);
    args.extend(subagent_args(spec.codegraph_available));
    args.extend(session_args(spec.start, spec.session_id));
    args
}

/// `thinking_summaries`: whether the claude this launch runs parses
/// `--thinking-display` (see below). `system_prompt`: the file the session's
/// instruction text was written to (`agent_profile::write_system_prompt`),
/// which claude reads with `--append-system-prompt-file`; `None` when there is
/// no text.
pub(crate) fn prepare_managed_args(
    spec: &SpawnSpec<'_>,
    mcp_args: &[String],
    thinking_summaries: bool,
    system_prompt: Option<&Path>,
) -> Vec<String> {
    // Stream-json input + output give us a structured back-and-forth
    // over stdio. --verbose is required when using stream-json output
    // so events keep flowing. --include-partial-messages emits
    // incremental assistant text deltas for a responsive UI.
    //
    // `--permission-mode default --permission-prompt-tool stdio` (instead of
    // `bypassPermissions`) routes every tool through a `can_use_tool` control
    // request on stdio. `ManagedSession` auto-approves all of them except the
    // question tools, which it holds open so the user actually answers — see
    // managed_session.rs. `bypassPermissions` can't do this: it auto-denies
    // AskUserQuestion before the client is consulted.
    //
    // `--thinking-display summarized` asks for a readable summary of each
    // thinking phase. Left unset, claude in print mode leaves the display to
    // the server, which redacts the reasoning to an empty block and shares a
    // one-line note for only some phases (observed: 55 of 234 in one session)
    // — so the chat had almost no thoughts to show. The flag is hidden from
    // `--help` but parsed (2.1.314: choices summarized, omitted, highlights),
    // and a claude that doesn't parse it refuses to launch, so it goes on the
    // line only when `thinking_summaries` says this launch's binary takes it
    // (`probe::claude_supports_thinking_display`). Whatever comes back, an
    // empty thinking block still shows nothing (src/adapters/claude/reduce.ts).
    let mut args: Vec<String> = vec![
        "--print".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
    ];
    if thinking_summaries {
        args.push("--thinking-display".into());
        args.push("summarized".into());
    }
    args.extend([
        "--permission-mode".into(),
        "default".into(),
        "--permission-prompt-tool".into(),
        "stdio".into(),
    ]);
    args.extend(effort_args(spec.effort));
    args.extend(model_args(spec.model));
    args.extend(instructions::append_system_prompt_file_args(system_prompt));
    args.extend(crate::attribution::claude_settings_args());
    args.extend_from_slice(mcp_args);
    args.extend(subagent_args(spec.codegraph_available));
    args.extend(session_args(spec.start, spec.session_id));
    args
}

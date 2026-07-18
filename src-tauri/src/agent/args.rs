//! Shared, non-provider-specific CLI arg-building helpers.

use std::path::Path;

use crate::instructions;

use super::spawn::SpawnSpec;

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

pub(crate) fn prepare_pty_args(spec: &SpawnSpec<'_>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--dangerously-skip-permissions".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
    ];
    args.extend(effort_args(spec.effort));
    args.extend(model_args(spec.model));
    args.extend(instructions::append_system_prompt_args(spec.instructions));
    args.extend(mcp_config_args(spec.mcp_config));

    if spec.fresh {
        args.push("--session-id".into());
        args.push(spec.session_id.to_string());
    } else {
        args.push("--resume".into());
        args.push(spec.session_id.to_string());
    }

    args
}

/// Claude's MCP flags for a generated config file: `--strict-mcp-config` makes
/// the snapshot-derived file the *only* MCP source, so on-disk user/project MCP
/// config never rides along with an agent Fletch spawns.
fn mcp_config_args(config: Option<&Path>) -> Vec<String> {
    match config {
        Some(path) => vec![
            "--mcp-config".into(),
            path.to_string_lossy().into_owned(),
            "--strict-mcp-config".into(),
        ],
        None => Vec::new(),
    }
}

pub(crate) fn prepare_managed_args(spec: &SpawnSpec<'_>) -> Vec<String> {
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
    let mut args: Vec<String> = vec![
        "--print".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        "--permission-mode".into(),
        "default".into(),
        "--permission-prompt-tool".into(),
        "stdio".into(),
    ];
    args.extend(effort_args(spec.effort));
    args.extend(model_args(spec.model));
    args.extend(instructions::append_system_prompt_args(spec.instructions));
    args.extend(mcp_config_args(spec.mcp_config));

    if spec.fresh {
        args.push("--session-id".into());
        args.push(spec.session_id.to_string());
    } else {
        args.push("--resume".into());
        args.push(spec.session_id.to_string());
    }

    args
}

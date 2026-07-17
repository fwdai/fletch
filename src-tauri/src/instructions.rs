//! Agent instruction injection.
//!
//! A single source of truth for the system-prompt-level instructions Fletch
//! injects into every agent — edit `instructions/system_prompt.md` and every
//! agent picks it up on its next spawn. There is no other copy.
//!
//! Fletch drives heterogeneous agent CLIs, and they expose different slots for
//! app-supplied guidance, so the *delivery* is per-agent while the *text* is
//! shared:
//!
//! - **Claude, Pi** — `--append-system-prompt <text>` (appends to the real
//!   system prompt; re-passed every spawn, one non-accumulating copy).
//! - **Codex** — `-c developer_instructions=<text>` (the developer-role layer
//!   on top of Codex's base prompt; re-passed every turn, non-accumulating).
//! - **Cursor, OpenCode, Antigravity** — no system-prompt slot, so the text is
//!   prepended to the *first* turn's prompt. It then lives in the resumed
//!   conversation, so later turns don't re-send it (no per-turn token tax, no
//!   accumulating copies).
//!
//! The injected text has three layers: editable general guidance
//! (`instructions/system_prompt.md`), a Fletch-managed protocol block
//! (`instructions/rpc_protocol.md`) that documents the file-RPC transport the
//! app exposes (see `rpc.rs`), and Fletch-managed feature playbooks (for
//! example `instructions/git_actions.md`) behind the panel's `[app-action]`
//! triggers. The managed blocks are code-managed because they must stay in
//! sync with the op allowlist / trigger names; the general layer is yours to
//! edit. Blank all files to disable injection entirely — every helper below
//! no-ops when the combined text is empty.

/// Editable general guidance. Edit the file, not this constant.
const SYSTEM_PROMPT: &str = include_str!("instructions/system_prompt.md");

/// Fletch-managed RPC protocol block, appended after the general guidance.
const RPC_INSTRUCTIONS: &str = include_str!("instructions/rpc_protocol.md");

/// Fletch-managed git-action playbooks. The panel sends a short
/// `[app-action] <name>` trigger; the full per-action instructions live here
/// so the chat transcript stays free of boilerplate. Code-managed: must stay
/// in sync with the trigger names the frontend sends (see
/// `components/RightPanel/delegation.ts`).
const GIT_ACTIONS: &str = include_str!("instructions/git_actions.md");

/// The combined instruction text, trimmed. Empty when every source is
/// blank/whitespace, which makes every injection helper a no-op.
pub fn text() -> String {
    let combined = [SYSTEM_PROMPT, RPC_INSTRUCTIONS, GIT_ACTIONS]
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    combined
}

/// Per-agent workspace-layout note for multi-repo projects, composed ahead of
/// any custom brief by the spawn path. `None` for single-repo agents, so the
/// common case injects nothing extra. Lists each sibling checkout by its
/// directory name (with the repo's project label when one is set) and points
/// at the `args.repo` selector the git RPC ops accept.
pub fn multi_repo_workspace_note(repos: &[crate::workspace::TrackedRepo]) -> Option<String> {
    if repos.len() < 2 {
        return None;
    }
    let mut lines = String::new();
    for (i, r) in repos.iter().enumerate() {
        let basename = r
            .repo_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let name = r.label.as_deref().unwrap_or(basename);
        let marker = if i == 0 {
            " (your starting checkout)"
        } else {
            ""
        };
        if name.is_empty() || name == r.subdir {
            lines.push_str(&format!("- `{}/`{}\n", r.subdir, marker));
        } else {
            lines.push_str(&format!("- `{}/` — {}{}\n", r.subdir, name, marker));
        }
    }
    Some(format!(
        "## Workspace layout: multiple repositories\n\n\
         This project spans {} repositories; this workspace holds a sibling checkout of each \
         under the workspace root:\n\n{lines}\n\
         Work across whichever checkouts the task requires (e.g. `cd ../{}`), committing per \
         repository with plain git. The host git ops (`git_push`, `open_pr`, `git_fetch`, \
         `git_status`) target your starting repository by default — pass `\"repo\": \
         \"<checkout dir name>\"` inside `args` to run one against a sibling checkout instead.",
        repos.len(),
        repos[1].subdir,
    ))
}

/// The global instruction text plus an optional per-session suffix (a custom
/// agent's standing brief). The suffix is appended *after* the global block so
/// project/global guidance composes with the agent's role rather than replacing
/// it. Empty (a no-op for every helper) only when both layers are blank.
fn combined(extra: Option<&str>) -> String {
    let base = text();
    match extra.map(str::trim).filter(|s| !s.is_empty()) {
        Some(custom) if base.is_empty() => custom.to_string(),
        Some(custom) => format!("{base}\n\n{custom}"),
        None => base,
    }
}

/// Args for agents that expose `--append-system-prompt` (Claude, Pi). `extra`
/// carries a custom agent's per-session instructions. Empty when there's
/// nothing to inject.
pub fn append_system_prompt_args(extra: Option<&str>) -> Vec<String> {
    let text = combined(extra);
    if text.is_empty() {
        return Vec::new();
    }
    vec!["--append-system-prompt".into(), text]
}

/// Args for Codex's developer-instructions config override
/// (`-c developer_instructions="…"`). `extra` carries a custom agent's
/// per-session instructions. Empty when there's nothing to inject.
///
/// The value is a TOML basic string passed as a single argv element (no shell
/// is involved — `Command`/`portable-pty` pass argv directly), so only TOML
/// string escaping matters, not shell quoting.
pub fn codex_config_args(extra: Option<&str>) -> Vec<String> {
    let text = combined(extra);
    if text.is_empty() {
        return Vec::new();
    }
    vec![
        "-c".into(),
        format!("developer_instructions={}", toml_basic_string(&text)),
    ]
}

/// For agents with no system-prompt slot (Cursor, OpenCode, Antigravity), fold
/// the instructions into the prompt — but only on the first turn of a session
/// (`session_id` is `None`). On later turns the text is already in the resumed
/// history, so the original prompt is returned unchanged. `extra` carries a
/// custom agent's per-session instructions.
pub fn prepend_to_prompt(prompt: &str, session_id: Option<&str>, extra: Option<&str>) -> String {
    let text = combined(extra);
    if text.is_empty() || session_id.is_some() {
        return prompt.to_string();
    }
    // Wrap in a namespaced tag so the UI can strip this block from the user
    // bubble (these agents echo the prompt back into the transcript). The tag
    // is Fletch-specific to avoid colliding with real user content.
    format!("<fletch-system>\n{text}\n</fletch-system>\n\n{prompt}")
}

/// Encode `s` as a TOML basic string (double-quoted, with escapes), so it can
/// be passed as the value half of a `-c key=value` config override. Also used
/// by `agent_profile` for codex `mcp_servers.*` overrides.
pub(crate) fn toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Other control chars are illegal raw in a TOML basic string.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_is_present_and_nonempty() {
        // The shipped default is non-empty; guards against an accidental blank.
        assert!(!text().is_empty());
    }

    #[test]
    fn git_action_playbooks_are_injected() {
        // The panel's `[app-action]` triggers only work if the playbook block
        // reaches the agent's instructions.
        let t = text();
        assert!(t.contains("[app-action]"), "playbook block missing");
        assert!(t.contains("### commit"), "commit playbook missing");
    }

    #[test]
    fn append_args_carry_the_text() {
        let args = append_system_prompt_args(None);
        assert_eq!(args[0], "--append-system-prompt");
        assert_eq!(args[1], text());
    }

    #[test]
    fn codex_args_are_a_toml_developer_instructions_override() {
        let args = codex_config_args(None);
        assert_eq!(args[0], "-c");
        assert!(args[1].starts_with("developer_instructions=\""));
        assert!(args[1].ends_with('"'));
    }

    #[test]
    fn prepend_only_on_first_turn() {
        let first = prepend_to_prompt("do the thing", None, None);
        assert!(first.starts_with("<fletch-system>"));
        assert!(first.contains(text().as_str()));
        assert!(first.contains("</fletch-system>"));
        assert!(first.ends_with("do the thing"));

        // Resumed turn: untouched (the text is already in history).
        assert_eq!(
            prepend_to_prompt("do the thing", Some("sess-1"), None),
            "do the thing"
        );
    }

    #[test]
    fn custom_instructions_append_after_global_block() {
        let custom = "You are the Reviewer. Be terse.";

        // Append-style: the global text and the custom brief both ride in the
        // single --append-system-prompt arg, global first.
        let args = append_system_prompt_args(Some(custom));
        let base = text();
        assert_eq!(args[1], format!("{base}\n\n{custom}"));

        // Codex developer_instructions carries the combined text too.
        let codex = codex_config_args(Some(custom));
        assert!(codex[1].contains(custom));

        // Prepend-style: custom brief lands in the first-turn block.
        let first = prepend_to_prompt("do it", None, Some(custom));
        assert!(first.contains(custom));
        assert!(first.contains(base.as_str()));
        // Still suppressed on resume (the text is already in history).
        assert_eq!(prepend_to_prompt("do it", Some("s"), Some(custom)), "do it");
    }

    #[test]
    fn blank_custom_instructions_are_a_noop() {
        assert_eq!(
            append_system_prompt_args(Some("   ")),
            append_system_prompt_args(None)
        );
    }

    #[test]
    fn multi_repo_note_lists_checkouts_and_labels() {
        use crate::workspace::TrackedRepo;
        fn repo(subdir: &str, path: &str, label: Option<&str>) -> TrackedRepo {
            TrackedRepo {
                repo_path: std::path::PathBuf::from(path),
                subdir: subdir.into(),
                branch: None,
                parent_branch: None,
                base_sha: None,
                pr_number: None,
                pr_url: None,
                pr_title: None,
                pr_state: None,
                label: label.map(str::to_string),
            }
        }

        // Single repo (the common case): no note at all.
        assert_eq!(
            multi_repo_workspace_note(&[repo("app", "/src/app", None)]),
            None
        );

        let note = multi_repo_workspace_note(&[
            repo("frontend", "/src/frontend", None),
            repo("backend", "/src/backend", Some("Gateway")),
        ])
        .unwrap();
        assert!(note.contains("`frontend/`"), "note: {note}");
        assert!(note.contains("(your starting checkout)"), "note: {note}");
        assert!(note.contains("`backend/` — Gateway"), "note: {note}");
        assert!(note.contains("args"), "must point at args.repo: {note}");
        // No redundant "frontend — frontend" suffix when label == subdir.
        assert!(!note.contains("frontend/` — frontend"), "note: {note}");
    }

    #[test]
    fn toml_escaping_handles_quotes_newlines_and_backslashes() {
        assert_eq!(toml_basic_string("a\"b"), r#""a\"b""#);
        assert_eq!(toml_basic_string("a\nb"), r#""a\nb""#);
        assert_eq!(toml_basic_string("a\\b"), r#""a\\b""#);
        assert_eq!(toml_basic_string("tab\there"), r#""tab\there""#);
    }
}

//! Agent instruction injection.
//!
//! A single source of truth for the system-prompt-level instructions Quorum
//! injects into every agent — edit `instructions/system_prompt.md` and every
//! agent picks it up on its next spawn. There is no other copy.
//!
//! Quorum drives heterogeneous agent CLIs, and they expose different slots for
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
//! (`instructions/system_prompt.md`), a Quorum-managed protocol block
//! (`instructions/rpc_protocol.md`) that documents the file-RPC the app
//! exposes (see `rpc.rs`), and the Quorum-managed git-action playbooks
//! (`instructions/git_actions.md`) behind the panel's `[app-action]`
//! triggers. The managed blocks are code-managed because they must stay in
//! sync with the op allowlist / trigger names; the general layer is yours to
//! edit. Blank all files to disable injection entirely — every helper below
//! no-ops when the combined text is empty.

/// Editable general guidance. Edit the file, not this constant.
const SYSTEM_PROMPT: &str = include_str!("instructions/system_prompt.md");

/// Quorum-managed RPC protocol block, appended after the general guidance.
const RPC_INSTRUCTIONS: &str = include_str!("instructions/rpc_protocol.md");

/// Quorum-managed git-action playbooks. The panel sends a short
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

/// Args for agents that expose `--append-system-prompt` (Claude, Pi).
/// Empty when there's nothing to inject.
pub fn append_system_prompt_args() -> Vec<String> {
    let text = text();
    if text.is_empty() {
        return Vec::new();
    }
    vec!["--append-system-prompt".into(), text.to_string()]
}

/// Args for Codex's developer-instructions config override
/// (`-c developer_instructions="…"`). Empty when there's nothing to inject.
///
/// The value is a TOML basic string passed as a single argv element (no shell
/// is involved — `Command`/`portable-pty` pass argv directly), so only TOML
/// string escaping matters, not shell quoting.
pub fn codex_config_args() -> Vec<String> {
    let text = text();
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
/// history, so the original prompt is returned unchanged.
pub fn prepend_to_prompt(prompt: &str, session_id: Option<&str>) -> String {
    let text = text();
    if text.is_empty() || session_id.is_some() {
        return prompt.to_string();
    }
    // Wrap in a namespaced tag so the UI can strip this block from the user
    // bubble (these agents echo the prompt back into the transcript). The tag
    // is Quorum-specific to avoid colliding with real user content.
    format!("<quorum-system>\n{text}\n</quorum-system>\n\n{prompt}")
}

/// Encode `s` as a TOML basic string (double-quoted, with escapes), so it can
/// be passed as the value half of a `-c key=value` config override.
fn toml_basic_string(s: &str) -> String {
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
        let args = append_system_prompt_args();
        assert_eq!(args[0], "--append-system-prompt");
        assert_eq!(args[1], text());
    }

    #[test]
    fn codex_args_are_a_toml_developer_instructions_override() {
        let args = codex_config_args();
        assert_eq!(args[0], "-c");
        assert!(args[1].starts_with("developer_instructions=\""));
        assert!(args[1].ends_with('"'));
    }

    #[test]
    fn prepend_only_on_first_turn() {
        let first = prepend_to_prompt("do the thing", None);
        assert!(first.starts_with("<quorum-system>"));
        assert!(first.contains(text().as_str()));
        assert!(first.contains("</quorum-system>"));
        assert!(first.ends_with("do the thing"));

        // Resumed turn: untouched (the text is already in history).
        assert_eq!(prepend_to_prompt("do the thing", Some("sess-1")), "do the thing");
    }

    #[test]
    fn toml_escaping_handles_quotes_newlines_and_backslashes() {
        assert_eq!(toml_basic_string("a\"b"), r#""a\"b""#);
        assert_eq!(toml_basic_string("a\nb"), r#""a\nb""#);
        assert_eq!(toml_basic_string("a\\b"), r#""a\\b""#);
        assert_eq!(toml_basic_string("tab\there"), r#""tab\there""#);
    }
}

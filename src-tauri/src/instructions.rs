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
//! The injected text has three unconditional layers: editable general guidance
//! (`instructions/system_prompt.md`), a Fletch-managed protocol block
//! (`instructions/rpc_protocol.md`) that documents the file-RPC transport the
//! app exposes (see `rpc.rs`), and Fletch-managed feature playbooks (for
//! example `instructions/git_actions.md`) behind the panel's `[app-action]`
//! triggers. The managed blocks are code-managed because they must stay in
//! sync with the op allowlist / trigger names; the general layer is yours to
//! edit. Blank all files to disable injection entirely — every helper below
//! no-ops when the combined text is empty.
//!
//! One layer is *conditional* and therefore not part of [`text`]:
//! [`codegraph_block`], which only makes sense for a session that actually got
//! the codegraph MCP server. It rides the per-session suffix instead (see
//! `agent_profile::effective_instructions`), which lands right after this
//! block in all three deliveries above.

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

/// Fletch-managed codegraph playbook: prefer the injected code-index MCP
/// server over a grep/read loop, and tell delegated subagents to do the same.
///
/// Deliberately *not* in [`text`]. The codegraph server reaches only some
/// sessions — indexing off, a Docker engine, a missing binary, a user-defined
/// `codegraph` server, or a provider with no MCP surface at all each suppress
/// it (see `codegraph::inject_mcp_server`) — and instructing an agent to call
/// a tool it was never given is worse than staying quiet.
const CODEGRAPH: &str = include_str!("instructions/codegraph.md");

/// Fletch-managed roadmap playbook: the `roadmap_list` / `roadmap_propose` RPC
/// ops, and the contract that a proposal is a ghost row until the user accepts
/// it.
///
/// Conditional for the same reason as [`CODEGRAPH`]: only a project-manager
/// chat is given the [`crate::rpc::roadmap::RoadmapDispatcher`], so only that
/// session may be told these ops exist. Code-managed — it must stay in sync
/// with the ops that dispatcher implements (pinned by a test there).
///
/// Per-session content joins it: the project's product context
/// ([`crate::roadmap::memory::product_context`] — the brief plus the board's
/// not-doing digest) is appended by [`roadmap_block`] when the project has any,
/// which is the read half of that seam.
const ROADMAP: &str = include_str!("instructions/roadmap.md");

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

/// The codegraph guidance block, for sessions where the server was actually
/// injected. `None` when the file is blank, so blanking every file under
/// `instructions/` still disables injection entirely.
pub fn codegraph_block() -> Option<String> {
    let block = CODEGRAPH.trim();
    (!block.is_empty()).then(|| block.to_string())
}

/// The roadmap-ops guidance block, for project-manager chats only. `None` when
/// the file is blank, like [`codegraph_block`].
///
/// `product_context` is the project's product memory, composed as named markdown
/// sections by `roadmap::memory::product_context` (the brief, and the board's
/// "Not doing" digest of rejected items) and threaded in from the spawn path —
/// the *read* half of that seam. Present, it is appended as its own fenced
/// section: a PM that has to be told the vision and the killed ideas every
/// session re-litigates decisions the user already made, and this context is the
/// only thing in this chat that survives the chat. Absent (nothing decided yet,
/// or not a project with a board), the block is exactly the playbook, so nothing
/// claims a memory that doesn't exist.
///
/// A parameter rather than a lookup in here: this module owns *text*, not
/// storage, and a global would make the block untestable and the injection
/// order implicit.
pub fn roadmap_block(product_context: Option<&str>) -> Option<String> {
    let block = ROADMAP.trim();
    if block.is_empty() {
        return None;
    }
    match product_context.map(str::trim).filter(|s| !s.is_empty()) {
        None => Some(block.to_string()),
        Some(context) => Some(format!("{block}\n\n{}", product_context_section(context))),
    }
}

/// The product context, fenced and framed: what its sections are, whose they
/// are, and how each one changes.
///
/// Fenced in a namespaced tag for the same reason [`prepend_to_prompt`] uses one
/// — the content is written by other parties (the PM drafted the brief, the user
/// ruled it in; the digest quotes the user's rejection reasons), so its headings
/// must not read as instructions from the app. The frame states the trust model
/// up front, because an agent that believes it owns this content will quietly
/// rewrite the user's position — or re-propose what the user already killed.
fn product_context_section(context: &str) -> String {
    format!(
        "## Product context (maintained by you, ruled by the user)\n\n\
         This is your memory of *this product* across sessions — the thing you would otherwise \
         have to ask the user to restate — in named sections. A section with nothing to say yet \
         is simply absent.\n\n\
         **Product brief** is the document you maintain and the user owns: vision, domains, \
         constraints, rejected directions. When a direction decision lands in this conversation, \
         propose the whole updated brief with `roadmap_propose_brief_update` and say so — the \
         user's acceptance is what changes it. **Not doing** is the board's decision log, newest \
         ruling first: one line per item the user ruled off the board, with the reason. You do \
         not write it; rulings do.\n\n\
         Read both before you propose anything: a direction they rule out has already been \
         argued, and re-proposing it silently is the failure mode this section exists to \
         prevent. When an idea matches a rejected item, surface the old decision and its reason \
         and ask whether the user wants to challenge it — only they can reopen it. Neither \
         section is the board: what is being built lives in items, and restating them here \
         would rot.\n\n\
         <product-context>\n{context}\n</product-context>"
    )
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

/// Per-agent env-awareness note: which of the project's env keys reach the
/// processes the app runs on the agent's behalf (the Run panel and the
/// verifier/tests gate), and which exist — in `.env` or declared by
/// `.env.example`/`.env.sample` — but were not shared. Composed ahead of any
/// custom brief by the spawn path, like [`multi_repo_workspace_note`].
///
/// Key NAMES only, never values: a value in the system prompt would defeat the
/// membrane (`run_env`), which never lets values into the agent's process or
/// its checkout. `None` when the project declares no env at all, so the common
/// case injects nothing extra.
pub fn env_awareness_note(shared: &[String], unshared: &[String]) -> Option<String> {
    if shared.is_empty() && unshared.is_empty() {
        return None;
    }
    let list = |keys: &[String]| {
        keys.iter()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let shared_line = if shared.is_empty() {
        "- Shared with app-run processes: none yet.\n".to_string()
    } else {
        format!("- Shared with app-run processes: {}.\n", list(shared))
    };
    let unshared_line = if unshared.is_empty() {
        String::new()
    } else {
        format!(
            "- Present in the project's env config but NOT shared: {}.\n",
            list(unshared)
        )
    };
    Some(format!(
        "## Project environment variables\n\n\
         This project's env values live outside your workspace (its `.env` is gitignored and \
         deliberately absent from this checkout). Fletch injects the shared ones into the \
         sandboxed processes it runs for you — the Run panel and the verifier/tests gate — \
         never into your own environment.\n\n\
         {shared_line}{unshared_line}\n\
         Do not fabricate `.env` files or hardcode values to compensate. Run env-dependent \
         commands through the app (Run panel / verification), or tell the user which key you \
         need shared."
    ))
}

/// Per-agent heads-up for a workspace whose base branch could not be fetched
/// from `origin` while it was being provisioned (offline, no credentials, the
/// branch gone from the remote). Composed ahead of any custom brief by the
/// spawn path, exactly like [`multi_repo_workspace_note`], and only for the
/// agents that actually degraded — see `provision::BaseFreshness`.
///
/// The agent, not the app, delivers this: it is the surface the user is already
/// talking to, and it knows whether the task even depends on being current. The
/// alternative — failing the spawn — would make an offline machine unusable,
/// and staying quiet would let the agent redo work that already landed on the
/// base branch.
pub fn stale_base_note(base: &str) -> String {
    format!(
        "## Heads-up: this workspace's base may be out of date\n\n\
         Fletch could not reach `origin` while creating this workspace, so it started from \
         the last copy of `{base}` present on this machine rather than the branch's current \
         tip on the remote. Your starting commit — and `origin/{base}` inside this checkout \
         — may be behind by an unknown amount.\n\n\
         Tell the user this early, before doing work that depends on being up to date \
         (anything touching recently-changed code, or a task that may already have landed \
         on `{base}`). Once the network or GitHub credentials are back, \
         `git fetch origin {base}` in this checkout brings it current."
    )
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
    fn codegraph_block_is_conditional_and_names_the_tool() {
        // Never in the unconditional text: a session without the server must
        // not be told to call it.
        assert!(!text().contains("codegraph_explore"));

        let block = codegraph_block().expect("shipped default is non-empty");
        assert!(block.contains("codegraph_explore"), "block: {block}");
        // The tool is namespaced differently per provider surface, so the
        // agent has to be able to recognize both renderings.
        assert!(block.contains("mcp__codegraph__codegraph_explore"));
        assert!(block.contains("codegraph.codegraph_explore"));
        // The whole point of the block: subagents don't inherit it, so the
        // main agent must be told to pass it down.
        assert!(block.to_lowercase().contains("subagent"), "block: {block}");
    }

    #[test]
    fn roadmap_block_is_conditional_and_states_the_accept_contract() {
        // Never unconditional: an agent without the roadmap dispatcher must not
        // be told it can put tickets on a board.
        let t = text();
        assert!(!t.contains("roadmap_propose"));
        assert!(!t.contains("roadmap_list"));

        let block = roadmap_block(None).expect("shipped default is non-empty");
        assert!(block.contains("roadmap_list"), "block: {block}");
        assert!(block.contains("roadmap_propose"), "block: {block}");
        // The safety property the whole feature rests on: a proposal is a ghost
        // row until the user accepts it. If the block stops saying so, the PM
        // starts talking as though it put things on the roadmap itself.
        let lower = block.to_lowercase();
        assert!(lower.contains("ghost row"), "block: {block}");
        assert!(lower.contains("accept"), "block: {block}");
        // Deps are codes, not titles — the dispatcher rejects anything else.
        assert!(lower.contains("deps"), "block: {block}");
    }

    #[test]
    fn the_product_context_rides_the_roadmap_block_only_when_there_is_one() {
        // No context (a fresh project): the block is exactly the playbook, and
        // in particular claims no memory. A PM told it has a brief it can't see
        // would quote an empty one at the user.
        let bare = roadmap_block(None).expect("shipped default is non-empty");
        assert!(!bare.contains("<product-context>"), "{bare}");
        assert!(!bare.contains("Product context (maintained"), "{bare}");
        // Blank and whitespace-only are the same as absent — an empty document
        // must not produce an empty fence.
        assert_eq!(
            roadmap_block(Some("   \n ")).as_deref(),
            Some(bare.as_str())
        );

        // With one: the playbook first, then the fenced section carrying the
        // composed context verbatim. Fenced because the content has headings of
        // its own, and the agent must be able to tell the memory from the
        // instructions.
        let context = "## Product brief\n\n# Fletch\n\n## Not doing\n\n- FLT-9 — Sprints — no";
        let block = roadmap_block(Some(context)).expect("shipped default is non-empty");
        assert!(
            block.starts_with(&bare),
            "the playbook still leads: {block}"
        );
        assert!(block.contains("## Product context (maintained by you, ruled by the user)"));
        assert!(
            block.contains(&format!("<product-context>\n{context}\n</product-context>")),
            "the context must be injected verbatim inside the fence: {block}"
        );
        // The trust model, stated where the content is: the PM maintains the
        // brief and the user rules it, via the one op that can change it.
        assert!(block.contains("roadmap_propose_brief_update"), "{block}");
        // The frame must say what the decision log is for — surfacing a killed
        // idea rather than silently re-proposing it — and who can undo it.
        assert!(block.contains("Not doing"), "{block}");
        assert!(block.contains("reopen"), "{block}");
        // And it must not invite the PM to restate the board here.
        assert!(block.contains("Neither section is the board"), "{block}");
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
    fn env_note_lists_key_names_only_and_is_conditional() {
        // No env at all (the common case): no note.
        assert_eq!(env_awareness_note(&[], &[]), None);

        let note = env_awareness_note(
            &["DATABASE_URL".into()],
            &["SECRET_KEY".into(), "API_KEY".into()],
        )
        .unwrap();
        // Both lists render by key NAME — the note carries names, never values.
        assert!(note.contains("`DATABASE_URL`"), "note: {note}");
        assert!(note.contains("`SECRET_KEY`"), "note: {note}");
        assert!(note.contains("`API_KEY`"), "note: {note}");
        assert!(note.contains("NOT shared"), "note: {note}");
        // The failure modes the note exists to prevent.
        assert!(note.contains("Do not fabricate"), "note: {note}");
        assert!(note.contains("Run panel"), "note: {note}");

        // Nothing shared yet, keys discovered: the note says so rather than
        // listing an empty set.
        let none_shared = env_awareness_note(&[], &["API_KEY".into()]).unwrap();
        assert!(none_shared.contains("none yet"), "note: {none_shared}");
    }

    #[test]
    fn stale_base_note_names_the_base_and_asks_the_agent_to_tell_the_user() {
        // The whole point of the note is that the *agent* relays the
        // degradation, so it must name the branch and say to speak up.
        let note = stale_base_note("develop");
        assert!(note.contains("`develop`"), "note: {note}");
        assert!(note.contains("Tell the user"), "note: {note}");
        assert!(
            note.contains("git fetch origin develop"),
            "note must give the recovery command: {note}"
        );
    }

    #[test]
    fn toml_escaping_handles_quotes_newlines_and_backslashes() {
        assert_eq!(toml_basic_string("a\"b"), r#""a\"b""#);
        assert_eq!(toml_basic_string("a\nb"), r#""a\nb""#);
        assert_eq!(toml_basic_string("a\\b"), r#""a\\b""#);
        assert_eq!(toml_basic_string("tab\there"), r#""tab\there""#);
    }
}

use std::path::Path;

use serde_json::json;

use super::args::effort_args;
use super::capabilities::{provider_bin_label, PER_TURN_AGENTS};
use super::providers::antigravity::{antigravity_conv_id_from_map, antigravity_read};
use super::providers::codex::{codex_build_args, codex_pty_args, codex_session_id};
use super::providers::cursor::{cursor_build_args, cursor_pty_args, cursor_session_id};
use super::providers::opencode::{opencode_build_args, opencode_pty_args, opencode_session_id};
use super::providers::pi::{pi_build_args, pi_pty_args, pi_session_id, pi_session_slug};
use super::transcript::{jsonl_files_ending, records_with_id};
use super::*;

// ── transcript readers ────────────────────────────────────────────────

#[test]
fn read_jsonl_tail_consumes_complete_lines_holds_partial_then_resumes() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");

    // Two complete lines, then a torn trailing line (no newline yet — the
    // writer is mid-append).
    {
        let mut f = std::fs::File::create(&path).unwrap();
        write!(
            f,
            "{{\"uuid\":\"a\",\"type\":\"user\"}}\n{{\"uuid\":\"b\",\"type\":\"assistant\"}}\n{{\"uuid\":\"c\","
        )
        .unwrap();
    }

    let (recs, off) = read_jsonl_tail(
        &path,
        0,
        0,
        Some("uuid"),
        false,
        &mut ReadDiagnostics::default(),
    );
    assert_eq!(
        recs.iter()
            .map(|r| r.native_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"],
        "only complete lines ingested; the torn line is held back",
    );

    // The writer finishes line c and appends d. Resume from the held offset
    // with start_index = 2 (we consumed 2 records).
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(
            f,
            "\"type\":\"assistant\"}}\n{{\"uuid\":\"d\",\"type\":\"user\"}}\n"
        )
        .unwrap();
    }

    let (recs2, _off2) = read_jsonl_tail(
        &path,
        off,
        2,
        Some("uuid"),
        false,
        &mut ReadDiagnostics::default(),
    );
    assert_eq!(
        recs2
            .iter()
            .map(|r| r.native_id.as_str())
            .collect::<Vec<_>>(),
        vec!["c", "d"],
        "resume picks up the now-complete line + the new one, no re-emit",
    );
}

#[test]
fn read_jsonl_tail_positional_ids_continue_from_start_index() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.jsonl");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        // No `uuid` field → positional `ln:{global_index}` fallback.
        write!(f, "{{\"type\":\"mode\"}}\n{{\"type\":\"summary\"}}\n").unwrap();
    }
    // Pretend 5 records were already ingested.
    let (recs, _off) = read_jsonl_tail(&path, 0, 5, None, false, &mut ReadDiagnostics::default());
    assert_eq!(
        recs.iter()
            .map(|r| r.native_id.as_str())
            .collect::<Vec<_>>(),
        vec!["ln:5", "ln:6"],
    );
}

#[test]
fn read_jsonl_tail_consume_trailing_reads_unterminated_final_line() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();

    // A complete line + a complete-but-unterminated final line — how cursor
    // and pi write their last (assistant) line: no trailing newline.
    let path = dir.path().join("c.jsonl");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        write!(
            f,
            "{{\"uuid\":\"a\",\"type\":\"user\"}}\n{{\"uuid\":\"b\",\"type\":\"assistant\"}}",
        )
        .unwrap();
    }

    // Exited writer: the unterminated final line is consumed, to EOF.
    let (recs, off) = read_jsonl_tail(
        &path,
        0,
        0,
        Some("uuid"),
        true,
        &mut ReadDiagnostics::default(),
    );
    assert_eq!(
        recs.iter()
            .map(|r| r.native_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"],
    );
    assert_eq!(
        off,
        std::fs::metadata(&path).unwrap().len(),
        "consumed to EOF"
    );

    // Live writer (same bytes): the unterminated final line is held back.
    let (held, _) = read_jsonl_tail(
        &path,
        0,
        0,
        Some("uuid"),
        false,
        &mut ReadDiagnostics::default(),
    );
    assert_eq!(
        held.iter()
            .map(|r| r.native_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a"],
    );

    // A genuinely torn final line (invalid JSON) is held even when consuming.
    let torn = dir.path().join("torn.jsonl");
    {
        let mut f = std::fs::File::create(&torn).unwrap();
        write!(f, "{{\"uuid\":\"a\",\"type\":\"user\"}}\n{{\"uuid\":\"x\",").unwrap();
    }
    let (recs_torn, _) = read_jsonl_tail(
        &torn,
        0,
        0,
        Some("uuid"),
        true,
        &mut ReadDiagnostics::default(),
    );
    assert_eq!(
        recs_torn
            .iter()
            .map(|r| r.native_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a"],
        "a mid-write torn line is held even with consume_trailing",
    );
}

#[test]
fn provider_bin_label_dispatch() {
    assert_eq!(
        provider_bin_label("claude"),
        Some(("claude", "Claude Code"))
    );
    assert!(provider_bin_label("codex").is_some());
    assert!(provider_bin_label("pi").is_some());
    assert_eq!(
        provider_bin_label("antigravity"),
        Some(("agy", "Antigravity"))
    );
    assert!(provider_bin_label("nope").is_none());
}

#[test]
fn transcript_reader_dispatch() {
    // Claude (persistent runner, not a per-turn agent) + every per-turn agent.
    assert!(transcript_reader("claude").is_some());
    assert!(transcript_reader("pi").is_some());
    assert!(transcript_reader("codex").is_some());
    assert!(transcript_reader("cursor").is_some());
    assert!(transcript_reader("opencode").is_some());
    assert!(transcript_reader("antigravity").is_some());
    // Unknown providers have none.
    assert!(transcript_reader("nope").is_none());
}

#[test]
fn antigravity_conv_id_from_map_reads_cwd_key() {
    let json = r#"{"/Users/alex/x":"conv-1","/Users/alex/y":"conv-2"}"#;
    assert_eq!(
        antigravity_conv_id_from_map(json, "/Users/alex/x").as_deref(),
        Some("conv-1")
    );
    assert_eq!(antigravity_conv_id_from_map(json, "/Users/alex/z"), None);
    assert_eq!(antigravity_conv_id_from_map("not json", "/x"), None);
}

#[test]
fn antigravity_read_keys_by_step_index() {
    let td = tempfile::tempdir().unwrap();
    let f = td.path().join("transcript_full.jsonl");
    std::fs::write(
        &f,
        "{\"step_index\":0,\"type\":\"USER_INPUT\"}\n{\"step_index\":2,\"type\":\"PLANNER_RESPONSE\"}\n{\"type\":\"X\"}\n",
    )
    .unwrap();
    let mut diag = ReadDiagnostics::default();
    let recs = antigravity_read(&[f], &mut diag);
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0].native_id, "step:0");
    assert_eq!(recs[1].native_id, "step:2");
    assert_eq!(recs[2].native_id, "ln:2"); // no step_index → positional
    assert_eq!(diag.lines_seen, 3);
    assert_eq!(diag.records_parsed, 3);
}

// agy's native (PTY/TUI) view is wired — it resumes the conversation the
// custom view established. Rollout test below pins the full matrix.
#[test]
fn antigravity_native_view_is_wired() {
    assert!(capabilities("antigravity").native_view);
}

#[test]
fn pi_slug_wraps_cwd_with_dashes() {
    assert_eq!(
        pi_session_slug(Path::new("/Users/alex/Code/amux")),
        "--Users-alex-Code-amux--"
    );
    // Dots are preserved (unlike Cursor) — only slashes are replaced.
    assert_eq!(
        pi_session_slug(Path::new("/Users/alex/.fletch/worktrees/balkhash/agent")),
        "--Users-alex-.fletch-worktrees-balkhash-agent--"
    );
}

#[test]
fn records_with_id_uses_id_field_when_present() {
    let values = vec![json!({"id": "abc", "v": 1}), json!({"id": "def", "v": 2})];
    let recs = records_with_id(values, Some("id"));
    assert_eq!(recs[0].native_id, "abc");
    assert_eq!(recs[1].native_id, "def");
    assert_eq!(recs[0].body, json!({"id": "abc", "v": 1}));
}

#[test]
fn records_with_id_positional_fallback_is_global() {
    // First line has an id, second doesn't; the positional index is the
    // global stream offset, not reset per missing line.
    let values = vec![json!({"id": "abc"}), json!({"no_id": true})];
    let recs = records_with_id(values, Some("id"));
    assert_eq!(recs[0].native_id, "abc");
    assert_eq!(recs[1].native_id, "ln:1");
}

#[test]
fn records_with_id_none_field_is_all_positional() {
    let values = vec![json!({"a": 1}), json!({"a": 2})];
    let recs = records_with_id(values, None);
    assert_eq!(recs[0].native_id, "ln:0");
    assert_eq!(recs[1].native_id, "ln:1");
}

#[test]
fn jsonl_files_ending_filters_and_sorts() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path();
    std::fs::write(dir.join("2026-06-04T19-10-20Z_sess-1.jsonl"), "{}").unwrap();
    std::fs::write(dir.join("2026-06-04T08-00-00Z_sess-1.jsonl"), "{}").unwrap();
    std::fs::write(dir.join("2026-06-04T09-00-00Z_other.jsonl"), "{}").unwrap();
    std::fs::write(dir.join("notes.txt"), "x").unwrap();

    let found = jsonl_files_ending(dir, "_sess-1.jsonl");
    let names: Vec<String> = found
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    // Only the two matching files, sorted lexically (== chronological here).
    assert_eq!(
        names,
        vec![
            "2026-06-04T08-00-00Z_sess-1.jsonl".to_string(),
            "2026-06-04T19-10-20Z_sess-1.jsonl".to_string(),
        ]
    );
}

#[test]
fn missing_dir_yields_no_files() {
    let td = tempfile::tempdir().unwrap();
    assert!(jsonl_files_ending(&td.path().join("nope"), "_x.jsonl").is_empty());
}

// ── build_args ────────────────────────────────────────────────────────

#[test]
fn opencode_args_request_thinking() {
    // Without --thinking, opencode emits no `reasoning` events at all.
    let args = opencode_build_args(&TurnArgs {
        prompt: "hi",
        ..Default::default()
    });
    assert!(args.contains(&"--thinking".to_string()));
    assert!(args.contains(&"--format".to_string()));
    // Prompt is positional and last (possibly prefixed with injected
    // instructions on a fresh turn, so match the tail rather than equality).
    assert!(
        args.last().unwrap().ends_with("hi"),
        "prompt is positional and last"
    );
}

// ── pty (native TUI) args ──────────────────────────────────────────────

#[test]
fn codex_pty_args_launch_tui_fresh_and_resume() {
    // Fresh: bypass approvals/sandbox so the TUI runs unattended; no
    // `exec`/`resume` subcommand means the interactive CLI.
    let fresh = codex_pty_args(None, None, None, &[]);
    assert!(fresh.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(!fresh.iter().any(|a| a == "resume"));
    // Resume: `resume <id>` continues the prior interactive session.
    let resume = codex_pty_args(Some("abc123"), None, None, &[]);
    assert!(resume.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    let pos = resume
        .iter()
        .position(|a| a == "resume")
        .expect("resume subcommand");
    assert_eq!(resume.get(pos + 1).map(String::as_str), Some("abc123"));
}

#[test]
fn cursor_pty_args_force_and_resume() {
    let fresh = cursor_pty_args(None, None, None, &[]);
    assert!(fresh.contains(&"--force".to_string()));
    assert!(!fresh.iter().any(|a| a == "--resume"));
    let resume = cursor_pty_args(Some("chat-1"), None, None, &[]);
    assert!(resume.contains(&"--force".to_string()));
    let pos = resume
        .iter()
        .position(|a| a == "--resume")
        .expect("--resume flag");
    assert_eq!(resume.get(pos + 1).map(String::as_str), Some("chat-1"));
}

#[test]
fn opencode_pty_args_launch_tui_and_session() {
    // Fresh: bare `opencode` launches the TUI. It must NOT carry
    // `--dangerously-skip-permissions` — that's a `run`-only flag, and
    // the default (TUI) command prints help and exits when given it.
    let fresh = opencode_pty_args(None, None, None, &[]);
    assert!(!fresh.iter().any(|a| a == "--dangerously-skip-permissions"));
    assert!(fresh.is_empty());
    // Resume: `--session <id>` continues the prior session.
    let resume = opencode_pty_args(Some("ses_9"), None, None, &[]);
    assert!(!resume.iter().any(|a| a == "--dangerously-skip-permissions"));
    let pos = resume
        .iter()
        .position(|a| a == "--session")
        .expect("--session flag");
    assert_eq!(resume.get(pos + 1).map(String::as_str), Some("ses_9"));
}

#[test]
fn pi_pty_args_bare_tui_and_session() {
    // Fresh: bare `pi` launches the interactive TUI; tools auto-run there.
    // (May carry injected --append-system-prompt args, but never a resume.)
    let fresh = pi_pty_args(None, None, None, &[]);
    assert!(
        !fresh.iter().any(|a| a == "--session"),
        "fresh TUI has no resume flag"
    );
    // Resume uses `--session <id>` (target pi 0.74.x lacks `--session-id`).
    let resume = pi_pty_args(Some("u-7"), None, None, &[]);
    let pos = resume
        .iter()
        .position(|a| a == "--session")
        .expect("--session flag");
    assert_eq!(resume.get(pos + 1).map(String::as_str), Some("u-7"));
}

#[test]
fn every_per_turn_agent_has_a_pty_arg_builder() {
    // Native view is wired for every per-turn agent, so each descriptor
    // must carry a TUI arg-builder. Fresh launch never references resume.
    for d in PER_TURN_AGENTS {
        let fresh = (d.pty_args)(None, None, None, &[]);
        assert!(
            !fresh
                .iter()
                .any(|a| a == "resume" || a == "--resume" || a == "--session"),
            "fresh {} args must not resume: {fresh:?}",
            d.id
        );
    }
}

#[test]
fn opencode_args_variant_when_thinking_set() {
    let args = opencode_build_args(&TurnArgs {
        prompt: "hi",
        thinking: Some("max"),
        ..Default::default()
    });
    assert!(args.contains(&"--variant".to_string()));
    assert!(args.contains(&"max".to_string()));
}

#[test]
fn codex_args_reasoning_effort_when_thinking_set() {
    let args = codex_build_args(&TurnArgs {
        prompt: "hi",
        thinking: Some("high"),
        ..Default::default()
    });
    assert!(args.contains(&"reasoning_effort=\"high\"".to_string()));
}

#[test]
fn codex_args_disable_codex_own_sandbox() {
    // Fletch now wraps codex in sandbox-exec, so codex's own confinement is
    // turned fully off — otherwise its workspace-write sandbox would block
    // the RPC mailbox, which lives outside the checkout.
    let args = codex_build_args(&TurnArgs {
        prompt: "hi",
        ..Default::default()
    });
    assert!(args.contains(&"sandbox_mode=\"danger-full-access\"".to_string()));
    assert!(!args.iter().any(|a| a.contains("workspace-write")));
    assert!(args.contains(&"approval_policy=\"never\"".to_string()));
}

#[test]
fn pi_args_thinking_when_set() {
    let args = pi_build_args(&TurnArgs {
        prompt: "hi",
        thinking: Some("xhigh"),
        ..Default::default()
    });
    assert!(args.contains(&"--thinking".to_string()));
    assert!(args.contains(&"xhigh".to_string()));
}

#[test]
fn effort_args_present_when_set() {
    assert_eq!(
        effort_args(Some("xhigh")),
        vec!["--effort".to_string(), "xhigh".to_string()]
    );
}

#[test]
fn effort_args_empty_when_unset() {
    assert!(effort_args(None).is_empty());
}

#[test]
fn cursor_args_ignores_thinking() {
    let with_none = cursor_build_args(&TurnArgs {
        prompt: "hi",
        ..Default::default()
    });
    let with_some = cursor_build_args(&TurnArgs {
        prompt: "hi",
        thinking: Some("high"),
        ..Default::default()
    });
    assert_eq!(with_none, with_some);
}

// ── session-id extraction ──────────────────────────────────────────────

/// Each `*_session_id` reads the id only when its event-type and (for
/// cursor) subtype gates pass — the shared `gated_session_id` shape with
/// per-provider gates.
#[test]
fn session_id_extractors_gate_then_read() {
    // Codex: gated on `thread.started`, reads `thread_id`.
    assert_eq!(
        codex_session_id(&json!({"type": "thread.started", "thread_id": "t-1"})),
        Some("t-1".into())
    );
    assert_eq!(
        codex_session_id(&json!({"type": "turn.delta", "thread_id": "t-1"})),
        None
    );

    // Cursor: gated on `system` + `subtype == init`, reads `session_id`.
    assert_eq!(
        cursor_session_id(&json!({"type": "system", "subtype": "init", "session_id": "s-1"})),
        Some("s-1".into())
    );
    assert_eq!(
        cursor_session_id(&json!({"type": "system", "subtype": "delta", "session_id": "s-1"})),
        None,
        "wrong subtype is gated out"
    );

    // OpenCode: no type gate, reads top-level `sessionID` off any event.
    assert_eq!(
        opencode_session_id(&json!({"type": "message", "sessionID": "ses_9"})),
        Some("ses_9".into())
    );

    // Pi: gated on `session`, reads `id`.
    assert_eq!(
        pi_session_id(&json!({"type": "session", "id": "u-7"})),
        Some("u-7".into())
    );
    assert_eq!(
        pi_session_id(&json!({"type": "assistant", "id": "u-7"})),
        None
    );
}

// ── descriptor table ──────────────────────────────────────────────────

#[test]
fn every_per_turn_agent_resolves_to_its_descriptor() {
    for d in PER_TURN_AGENTS {
        assert_eq!(per_turn_descriptor(d.id).map(|x| x.id), Some(d.id));
    }
    assert!(per_turn_descriptor("claude").is_none());
    assert!(per_turn_descriptor("nope").is_none());
}

/// Pins the current native-view capability rollout. When a follow-up wires
/// native view for an agent, it flips the descriptor flag and updates the
/// expectation here on purpose.
#[test]
fn capability_rollout_matches_what_is_wired_today() {
    let cases = [
        ("claude", true),
        ("codex", true),
        ("cursor", true),
        ("opencode", true),
        ("pi", true),
        ("antigravity", true),
        ("unknown", false),
    ];
    for (provider, native_view) in cases {
        assert_eq!(
            capabilities(provider).native_view,
            native_view,
            "native_view for {provider}"
        );
    }
}

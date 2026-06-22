//! Per-agent lifecycle.
//!
//! An agent is a git worktree + a coding-agent process running inside
//! it. There are three runner shapes:
//!
//! - **Pty** (claude native view): a sandboxed `claude` process in a PTY
//!   rendering its TUI; the app overlays its own input over the prompt.
//! - **Managed** (claude custom view): a sandboxed, persistent
//!   `claude --print` stream-json subprocess; the app renders structured
//!   chat. Both claude shapes attach to the same conversation via
//!   `--session-id <uuid>` on first spawn and `--resume <uuid>` after.
//! - **CodexManaged** (codex custom view): codex's `exec` runs one turn
//!   and exits, so there's no persistent process — each user message
//!   spawns a fresh `codex exec [resume <id>]` (see `codex_session`).
//!   Codex sandboxes itself rather than running under sandbox-exec.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::activity::{Activity, ManagedActivity};
use crate::error::{Error, Result};
use crate::exec_session::{ExecCallbacks, ExecSession, ExecSpawn};
use crate::instructions;
use crate::managed_session::{ManagedExit, ManagedSession, ManagedSpawn, ToolUseBehavior};
use crate::pty_session::{PtyExit, PtySession, PtySpawn};
use crate::sandbox;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub enum Agent {
    Pty(PtyAgent),
    Managed(ManagedAgent),
    /// A per-turn runner (codex, cursor): holds no live process between
    /// turns; each user message spawns a fresh process. The agent
    /// sandboxes itself, so there's no sandbox-exec profile.
    PerTurn(PerTurnAgent),
}


pub struct PtyAgent {
    pty: PtySession,
    /// `sandbox-exec` profile for claude's PTY run. `None` for per-turn
    /// agents in the native view: they launch their own binary directly and
    /// self-sandbox, so there's no profile to keep alive.
    _profile_file: Option<tempfile::NamedTempFile>,
}

pub struct ManagedAgent {
    session: ManagedSession,
    _profile_file: tempfile::NamedTempFile,
}

pub struct PerTurnAgent {
    session: ExecSession,
}

/// Parameters for spawning a per-turn runner. Unlike `SpawnSpec` there's
/// no sandbox profile (the agent sandboxes itself) and the session id is
/// optional — these agents assign one on the first turn.
pub struct PerTurnSpec {
    /// The agent's working directory — the primary repo's worktree.
    pub cwd: PathBuf,
    /// Sandbox writable root — the agent's parent dir (same role as
    /// `SpawnSpec::sandbox_root`). Per-turn agents now run under sandbox-exec
    /// too, so they need it to build the profile.
    pub sandbox_root: PathBuf,
    /// Session id to resume, if one has been captured already.
    pub session_id: Option<String>,
    /// Session-level model override. `None` keeps the provider CLI default.
    pub model: Option<String>,
    /// The agent's RPC mailbox dir, exposed to the child as `QUORUM_RPC_DIR`.
    pub rpc_dir: PathBuf,
}

/// Everything that varies between per-turn agents. The runner lifecycle —
/// one fresh process per turn via `ExecSession` — is identical for all of
/// them; only the binary, CLI args, session-id extraction, and turn-end
/// detector differ. Capturing those as a table entry means a new
/// per-turn agent is one `PER_TURN_AGENTS` row, with no new `spawn_*`
/// method, `resolve_*` helper, or `match provider` arm anywhere.
pub struct PerTurnDescriptor {
    /// Provider id (matches the frontend adapter / `AgentRecord.provider`).
    pub id: &'static str,
    /// Executable name resolved via `resolve_agent_bin`.
    bin: &'static str,
    /// Human-facing product name, used only in the not-found error.
    label: &'static str,
    /// Builds the CLI args for a turn: `(prompt, resume_session_id, thinking_effort, model)`.
    build_args: fn(&str, Option<&str>, Option<&str>, Option<&str>) -> Vec<String>,
    /// Builds the args to launch this agent's interactive TUI in the native
    /// (PTY) view: first arg is session (`None` = fresh), second is model.
    pty_args: fn(Option<&str>, Option<&str>) -> Vec<String>,
    /// Extracts the agent-assigned session id from a turn's events. No-op for
    /// `plaintext` agents (they emit no events — see `session_id_from_cwd`).
    session_id: fn(&Value) -> Option<String>,
    /// Constructs this agent's turn-end detector (custom-view `Activity`).
    pub activity: fn() -> Box<dyn Activity>,
    /// Whether this agent can render in the native PTY view (see
    /// `AgentCapabilities::native_view`).
    native_view: bool,
    /// True if the turn process emits **plaintext** on stdout rather than a
    /// newline-delimited JSON event stream. The runner then drains stdout
    /// without parsing (no events; history comes from `transcript`), and the
    /// session id is captured via `session_id_from_cwd` instead of events.
    plaintext: bool,
    /// For agents whose session id isn't in their event stream (e.g. agy), read
    /// it from the filesystem at turn-end given the worktree cwd. `None` =
    /// session id comes from events via `session_id`.
    pub session_id_from_cwd: Option<fn(&Path) -> Option<String>>,
    /// Reader for this agent's on-disk transcript, used by `sync_session` to
    /// ingest verbatim records into `session_records`. `None` = no readable
    /// transcript.
    pub transcript: Option<TranscriptReader>,
}

/// One verbatim durable record from an agent's transcript: the raw body in the
/// agent's own shape plus a stable per-record dedup key (`native_id`).
#[derive(Debug, Clone)]
pub struct RawRecord {
    pub native_id: String,
    pub body: Value,
}

/// Marks a reader whose transcript is a single append-only JSONL file, so it can
/// be read incrementally from a byte offset (`read_jsonl_tail`) instead of fully
/// re-parsed each turn. `id_field` is the per-line native-id field (matching the
/// reader's full `read`), or None for positional `ln:{i}` ids.
#[derive(Clone, Copy)]
pub struct JsonlTail {
    pub id_field: Option<&'static str>,
}

/// How to find and parse a provider's on-disk transcript into ordered records.
pub struct TranscriptReader {
    /// Ordered transcript artifact paths for a session (empty if none / not
    /// yet flushed). Multiple paths concatenate in order (resume can split).
    pub locate: fn(session_id: &str, cwd: &Path) -> Vec<PathBuf>,
    /// Parse located artifacts into ordered verbatim records.
    pub read: fn(paths: &[PathBuf]) -> Vec<RawRecord>,
    /// Set for single-file JSONL readers to enable incremental tail ingest.
    /// `None` for multi-file (codex) / blob-dir (opencode) readers, which fall
    /// back to a full read + idempotent batched insert.
    pub tail: Option<JsonlTail>,
}

/// What an agent can do *right now*. A rollout flag, not a fixed trait: native
/// (PTY/TUI) view support is being brought to every agent, and callers gate on
/// the capability, never on the provider id, so nothing else changes when
/// support lands.
pub struct AgentCapabilities {
    /// Can render in the native PTY view (its interactive TUI streamed into
    /// xterm), in addition to the structured custom view. Wired for claude
    /// and every per-turn agent (codex/cursor/opencode/pi); a per-turn agent
    /// can only switch *into* native once it has a session id to resume.
    pub native_view: bool,
}

/// Capabilities for a provider. Per-turn agents read theirs from the
/// descriptor table; claude (the lone persistent-runner agent) is the
/// fully-wired baseline. Unknown providers get nothing.
pub fn capabilities(provider: &str) -> AgentCapabilities {
    match per_turn_descriptor(provider) {
        Some(d) => AgentCapabilities {
            native_view: d.native_view,
        },
        None if provider == "claude" => AgentCapabilities {
            native_view: true,
        },
        None => AgentCapabilities {
            native_view: false,
        },
    }
}

const PER_TURN_AGENTS: &[PerTurnDescriptor] = &[
    PerTurnDescriptor {
        id: "codex",
        bin: "codex",
        label: "Codex",
        build_args: codex_build_args,
        pty_args: codex_pty_args,
        session_id: codex_session_id,
        activity: || Box::new(ManagedActivity::codex()),
        native_view: true,
        plaintext: false,
        session_id_from_cwd: None,
        transcript: Some(TranscriptReader {
            locate: codex_locate,
            read: codex_read,
            tail: None, // multiple rollout files
        }),
    },
    PerTurnDescriptor {
        id: "cursor",
        bin: "cursor-agent",
        label: "Cursor",
        build_args: cursor_build_args,
        pty_args: cursor_pty_args,
        session_id: cursor_session_id,
        // Cursor emits Claude-shaped stream-json incl. a `result` turn-end,
        // so it reuses the Claude managed detector.
        activity: || Box::new(ManagedActivity::claude()),
        native_view: true,
        plaintext: false,
        session_id_from_cwd: None,
        transcript: Some(TranscriptReader {
            locate: cursor_locate,
            read: cursor_read,
            tail: Some(JsonlTail { id_field: None }), // single jsonl, positional ids
        }),
    },
    PerTurnDescriptor {
        id: "opencode",
        bin: "opencode",
        label: "OpenCode",
        build_args: opencode_build_args,
        pty_args: opencode_pty_args,
        session_id: opencode_session_id,
        activity: || Box::new(ManagedActivity::opencode()),
        native_view: true,
        plaintext: false,
        session_id_from_cwd: None,
        transcript: Some(TranscriptReader {
            locate: opencode_locate,
            read: opencode_read,
            tail: None, // blob-store directory, not a single file
        }),
    },
    PerTurnDescriptor {
        id: "pi",
        bin: "pi",
        label: "Pi",
        build_args: pi_build_args,
        pty_args: pi_pty_args,
        session_id: pi_session_id,
        activity: || Box::new(ManagedActivity::pi()),
        native_view: true,
        plaintext: false,
        session_id_from_cwd: None,
        // Pi is the reference reader — its per-session JSONL feeds session_records.
        transcript: Some(TranscriptReader {
            locate: pi_locate,
            read: pi_read,
            tail: Some(JsonlTail { id_field: Some("id") }), // single jsonl when one file
        }),
    },
    PerTurnDescriptor {
        id: "antigravity",
        bin: "agy",
        label: "Antigravity",
        build_args: antigravity_build_args,
        pty_args: antigravity_pty_args,
        // agy emits no JSON events; its session id is read from the filesystem.
        session_id: |_| None,
        // No event stream to detect turn-end from — the turn's process exit ends
        // the turn (on_turn_exit). The detector is never fed, so any is fine.
        activity: || Box::new(ManagedActivity::claude()),
        // Native PTY view runs agy's interactive TUI, resuming the conversation
        // the custom view established (see antigravity_pty_args).
        native_view: true,
        plaintext: true,
        session_id_from_cwd: Some(antigravity_session_id_from_cwd),
        transcript: Some(TranscriptReader {
            locate: antigravity_locate,
            read: antigravity_read,
            tail: None, // per-turn agent; full read on exit is bounded
        }),
    },
];

/// Look up the descriptor for a per-turn provider id. `None` means the
/// provider isn't a per-turn agent (e.g. claude, which has its own
/// Pty/Managed runners) or isn't a known agent at all.
pub fn per_turn_descriptor(id: &str) -> Option<&'static PerTurnDescriptor> {
    PER_TURN_AGENTS.iter().find(|d| d.id == id)
}

// ── Transcript readers ──────────────────────────────────────────────────────

/// Build ordered `RawRecord`s from a parsed JSONL stream. `native_id` is the
/// value's `id_field` (a string) when present, else a positional `ln:{i}` key
/// over the global stream offset — stable across append-only multi-file reads.
/// Incrementally read an append-only JSONL transcript from byte `offset` to EOF,
/// returning the new records and the byte offset just past the last **complete**
/// line consumed. A torn trailing line (the writer is mid-append, no newline yet)
/// is left unconsumed so it's picked up — whole — on the next read; this is what
/// makes tailing safe against the flush race. Positional `ln:{i}` ids continue
/// from `start_index` (the count of records already ingested) so they match what
/// a full read would have assigned; `id_field` (e.g. claude's `uuid`) overrides
/// when present. Mirrors `records_with_id` parsing, just over the new tail only.
/// Parse one JSONL line into a `RawRecord`. Returns `None` for blank lines,
/// non-UTF-8, or incomplete/invalid JSON — so a half-written trailing line is
/// simply skipped rather than ingested.
fn parse_jsonl_record(line: &[u8], id_field: Option<&str>, idx: usize) -> Option<RawRecord> {
    let text = std::str::from_utf8(line).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(text).ok()?;
    let native_id = id_field
        .and_then(|f| v.get(f))
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("ln:{idx}"));
    Some(RawRecord { native_id, body: v })
}

/// Read new JSONL records from `path` starting at byte `offset`.
///
/// Newline-terminated lines are always safe to consume. The segment *after* the
/// last newline is an unterminated trailing line:
/// - For a **live writer** (`consume_trailing == false`, e.g. claude's open
///   transcript) it may be half-written, so we hold it and re-read once it's
///   terminated — the byte offset stays before it.
/// - For an **exited writer** (`consume_trailing == true`, i.e. a per-turn agent
///   in Custom view that has finished the turn) the trailing line is the final,
///   complete line. Cursor and Pi write their last line with no trailing
///   newline, so without this they'd never be ingested. We consume it *only if
///   it parses as JSON*, so a genuinely partial line (caught mid-write) is still
///   held rather than ingested.
pub fn read_jsonl_tail(
    path: &Path,
    offset: u64,
    start_index: usize,
    id_field: Option<&str>,
    consume_trailing: bool,
) -> (Vec<RawRecord>, u64) {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return (Vec::new(), offset);
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return (Vec::new(), offset);
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return (Vec::new(), offset);
    }

    // Bytes through the last newline are complete lines; the rest is the
    // unterminated trailing segment.
    let complete_end = buf
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|p| p + 1)
        .unwrap_or(0);

    let mut out = Vec::new();
    let mut idx = start_index;
    let mut consumed = complete_end;

    for line in buf[..complete_end].split(|&b| b == b'\n') {
        if let Some(rec) = parse_jsonl_record(line, id_field, idx) {
            out.push(rec);
            idx += 1;
        }
    }

    if consume_trailing {
        if let Some(rec) = parse_jsonl_record(&buf[complete_end..], id_field, idx) {
            out.push(rec);
            // The trailing line parsed cleanly, so it was complete — consume to
            // EOF so we don't re-read it next time.
            consumed = buf.len();
        }
    }

    (out, offset + consumed as u64)
}

pub fn records_with_id(values: Vec<Value>, id_field: Option<&str>) -> Vec<RawRecord> {
    values
        .into_iter()
        .enumerate()
        .map(|(i, body)| {
            let native_id = id_field
                .and_then(|f| body.get(f))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("ln:{i}"));
            RawRecord { native_id, body }
        })
        .collect()
}

/// JSONL files directly in `dir` whose filename ends with `suffix`, sorted
/// lexically (filenames are timestamp-prefixed, so lexical == chronological).
fn jsonl_files_ending(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix))
        })
        .collect();
    paths.sort();
    paths
}

/// Pi's session-dir slug: cwd with `/` → `-`, wrapped in `--…--`.
/// `/Users/alex/Code/amux` → `--Users-alex-Code-amux--`. Dots are preserved.
fn pi_session_slug(cwd: &Path) -> String {
    format!("-{}--", cwd.to_string_lossy().replace('/', "-"))
}

fn pi_locate(session_id: &str, cwd: &Path) -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let dir = home.join(".pi/agent/sessions").join(pi_session_slug(cwd));
    // Files are `<ts>_<session_id>.jsonl`.
    jsonl_files_ending(&dir, &format!("_{session_id}.jsonl"))
}

fn pi_read(paths: &[PathBuf]) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::supervisor::read_jsonl_values(p).unwrap_or_default())
        .collect();
    // Pi's JSONL lines carry a stable `id`.
    records_with_id(values, Some("id"))
}

// ── Claude ──
// Claude is the lone persistent-runner agent (not in PER_TURN_AGENTS), launched
// `--session-id <uuid>` / `--resume <uuid>`, so it writes
// `~/.claude/projects/<slug>/<uuid>.jsonl`. find_session_jsonl already locates
// it. Content lines carry a top-level
// `uuid`; metadata lines (mode/permission-mode/…) don't → positional fallback.

fn claude_locate(session_id: &str, _cwd: &Path) -> Vec<PathBuf> {
    crate::supervisor::find_session_jsonl(session_id)
        .into_iter()
        .collect()
}

fn claude_read(paths: &[PathBuf]) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::supervisor::read_jsonl_values(p).unwrap_or_default())
        .collect();
    records_with_id(values, Some("uuid"))
}

static CLAUDE_TRANSCRIPT: TranscriptReader = TranscriptReader {
    locate: claude_locate,
    read: claude_read,
    tail: Some(JsonlTail { id_field: Some("uuid") }), // single persistent jsonl
};

// ── Codex ──
// Codex writes `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`.
// Lines are `{timestamp,type,payload}` dual-channel with no stable per-line id,
// so records key positionally. The codex frontend adapter already normalizes.
fn codex_locate(session_id: &str, _cwd: &Path) -> Vec<PathBuf> {
    crate::supervisor::find_codex_rollouts(session_id)
}

fn codex_read(paths: &[PathBuf]) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::supervisor::read_jsonl_values(p).unwrap_or_default())
        .collect();
    records_with_id(values, None)
}

// ── Cursor ──
// cursor-agent writes `~/.cursor/projects/<slug>/agent-transcripts/<id>/<id>.jsonl`.
// The session-id dir is unique, so glob by it (like claude) rather than
// reverse-engineering the undocumented slug. Lines have no per-line id →
// positional keys.
fn cursor_locate(session_id: &str, _cwd: &Path) -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let rel = format!("agent-transcripts/{session_id}/{session_id}.jsonl");
    let projects = home.join(".cursor").join("projects");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&projects) {
        for entry in entries.flatten() {
            let path = entry.path().join(&rel);
            if path.exists() {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn cursor_read(paths: &[PathBuf]) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::supervisor::read_jsonl_values(p).unwrap_or_default())
        .collect();
    records_with_id(values, None)
}

// ── OpenCode ──
// OpenCode stores a blob store under `$XDG_DATA_HOME/opencode/storage` (defaults
// to `~/.local/share/opencode/storage`, even on macOS): message blobs at
// `message/<ses>/<msg>.json` (role + metadata, no content) and part blobs at
// `part/<msg>/<part>.json` (the content). We emit each message record then its
// parts, in id order (ids are time-sortable); the frontend reassembles. ids are
// globally unique, so they're the native dedup key.

fn opencode_storage_root() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))?;
    Some(base.join("opencode").join("storage"))
}

/// Sorted `*.json` files directly in `dir` (filenames are id-prefixed, so
/// lexical sort == creation order).
fn json_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();
    paths
}

fn read_json_value(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn opencode_locate(session_id: &str, _cwd: &Path) -> Vec<PathBuf> {
    let Some(root) = opencode_storage_root() else {
        return Vec::new();
    };
    json_files_in(&root.join("message").join(session_id))
}

fn opencode_read(message_paths: &[PathBuf]) -> Vec<RawRecord> {
    let mut out = Vec::new();
    for msg_path in message_paths {
        let Some(msg) = read_json_value(msg_path) else {
            continue;
        };
        let Some(msg_id) = msg.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
            continue;
        };
        out.push(RawRecord {
            native_id: msg_id.clone(),
            body: msg,
        });
        // Parts live at `<storage>/part/<msg_id>/`; derive <storage> from the
        // message path `<storage>/message/<ses>/<msg>.json` (three parents up).
        let Some(part_dir) = msg_path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|storage| storage.join("part").join(&msg_id))
        else {
            continue;
        };
        for pf in json_files_in(&part_dir) {
            if let Some(part) = read_json_value(&pf) {
                if let Some(pid) = part.get("id").and_then(|v| v.as_str()) {
                    out.push(RawRecord {
                        native_id: pid.to_string(),
                        body: part,
                    });
                }
            }
        }
    }
    out
}

// ── Antigravity (agy) ──
// agy has no JSON event stream (its `--print` output is plaintext), so it runs
// as a `plaintext` per-turn agent: the runner drains stdout, the turn's process
// exit ends the turn, and history comes entirely from its on-disk transcript.
// The conversation id (== session id) lives in agy's filesystem, not its output.

// `_model` is intentionally unused: agy's `--print` runner ignores model
// selection (the `--model` flag is inert in print mode), so the picker offers
// no selectable models for antigravity (see `model_catalog::discover_one`).
fn antigravity_build_args(prompt: &str, session_id: Option<&str>, _thinking: Option<&str>, _model: Option<&str>) -> Vec<String> {
    // `--print` takes the prompt as its *value* (i.e. `--print <prompt>`), so the
    // prompt must come last, directly after `--print`. Putting another flag
    // between them makes that flag the prompt (agy then "answers" the flag name).
    let mut args = vec!["--dangerously-skip-permissions".to_string()];
    if let Some(id) = session_id {
        args.push("--conversation".into());
        args.push(id.to_string());
    }
    args.push("--print".into());
    args.push(instructions::prepend_to_prompt(prompt, session_id));
    args
}

// `_model` unused: agy's TUI manages its own model selection (see
// `antigravity_build_args` and `model_catalog::discover_one`).
fn antigravity_pty_args(session_id: Option<&str>, _model: Option<&str>) -> Vec<String> {
    // Native view: launch agy's interactive TUI (NOT `--print`, the
    // non-interactive turn runner), resuming the conversation by id.
    let mut args = vec!["--dangerously-skip-permissions".to_string()];
    if let Some(id) = session_id {
        args.push("--conversation".into());
        args.push(id.to_string());
    }
    args
}

/// agy stores `cwd → conversationId` in
/// `~/.gemini/antigravity-cli/cache/last_conversations.json` (the worktree cwd
/// is the key). Read it at turn-end to capture the id for resume + transcript.
fn antigravity_session_id_from_cwd(cwd: &Path) -> Option<String> {
    let home = dirs::home_dir()?;
    let path = home.join(".gemini/antigravity-cli/cache/last_conversations.json");
    let text = std::fs::read_to_string(path).ok()?;
    antigravity_conv_id_from_map(&text, &cwd.to_string_lossy())
}

/// Pure: extract the conversation id for `cwd` from the last-conversations map.
fn antigravity_conv_id_from_map(json_text: &str, cwd: &str) -> Option<String> {
    let map: Value = serde_json::from_str(json_text).ok()?;
    map.get(cwd).and_then(|v| v.as_str()).map(str::to_string)
}

fn antigravity_locate(session_id: &str, cwd: &Path) -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    // Prefer the captured id; fall back to the cwd→id map (e.g. the first turn,
    // before the id has been persisted).
    let id = if session_id.is_empty() {
        match antigravity_session_id_from_cwd(cwd) {
            Some(i) => i,
            None => return Vec::new(),
        }
    } else {
        session_id.to_string()
    };
    let path = home
        .join(".gemini/antigravity-cli/brain")
        .join(&id)
        .join(".system_generated/logs/transcript_full.jsonl");
    if path.exists() {
        vec![path]
    } else {
        Vec::new()
    }
}

fn antigravity_read(paths: &[PathBuf]) -> Vec<RawRecord> {
    paths
        .iter()
        .flat_map(|p| crate::supervisor::read_jsonl_values(p).unwrap_or_default())
        .enumerate()
        .map(|(i, body)| {
            // `step_index` is a stable, monotonic per-conversation key.
            let native_id = body
                .get("step_index")
                .and_then(|v| v.as_i64())
                .map(|n| format!("step:{n}"))
                .unwrap_or_else(|| format!("ln:{i}"));
            RawRecord { native_id, body }
        })
        .collect()
}

/// The transcript reader for a provider, or `None` if it has no on-disk
/// transcript wired. Per-turn agents read theirs from the descriptor table;
/// claude (persistent runner) is special-cased here. Callers gate on this, not
/// on the provider id.
pub fn transcript_reader(provider: &str) -> Option<&'static TranscriptReader> {
    match per_turn_descriptor(provider) {
        Some(d) => d.transcript.as_ref(),
        None if provider == "claude" => Some(&CLAUDE_TRANSCRIPT),
        None => None,
    }
}

/// (binary, human label) for a provider, or `None` if unknown. Same dispatch
/// as `transcript_reader`: per-turn descriptors + the claude special case.
fn provider_bin_label(provider: &str) -> Option<(&'static str, &'static str)> {
    match per_turn_descriptor(provider) {
        Some(d) => Some((d.bin, d.label)),
        None if provider == "claude" => Some(("claude", "Claude Code")),
        None => None,
    }
}

/// The probed CLI version for a provider (`v1.2.3`), memoized per process so the
/// `--version` subprocess runs at most once per provider. Stamped onto
/// session_records at ingest so read-time normalizers can branch by version
/// when a vendor format changes. `None` if the binary is missing/unparseable.
pub fn cached_provider_version(provider: &str) -> Option<String> {
    static CACHE: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<String, Option<String>>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()));
    if let Some(v) = cache.lock().get(provider) {
        return v.clone();
    }
    let version = provider_bin_label(provider).and_then(|(bin, label)| {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        resolve_agent_bin(provider, bin, label, &home)
            .ok()
            .and_then(|p| probe_version(&p))
    });
    cache.lock().insert(provider.to_string(), version.clone());
    version
}

pub struct SpawnSpec<'a> {
    pub agent_id: &'a str,
    /// Claude's working directory — the primary repo's worktree.
    pub cwd: PathBuf,
    /// Sandbox writable root — the agent's parent dir, which may
    /// contain multiple per-repo worktrees as siblings of `cwd`. Writes
    /// are allowed anywhere under this path.
    pub sandbox_root: PathBuf,
    pub session_id: &'a str,
    /// True if this is the agent's first spawn (no prior conversation
    /// on disk for this session). False if we're respawning to switch
    /// views — claude should `--resume` instead of starting fresh.
    pub fresh: bool,
    /// Claude's session-level effort (`--effort <level>`), chosen at session
    /// creation and persisted on the `AgentRecord`. Applied on every spawn
    /// (fresh, view-switch, resume) so it sticks for the session. `None` =
    /// no selection; claude uses its own default. Ignored by per-turn agents,
    /// which take effort per-turn via their `thinking` build-args instead.
    pub effort: Option<&'a str>,
    /// Session-level model override. `None` keeps the provider CLI default.
    pub model: Option<&'a str>,
    /// The agent's RPC mailbox dir, exposed to the child as `QUORUM_RPC_DIR`.
    pub rpc_dir: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

/// The environment Quorum injects into every agent child: the absolute path to
/// its file-mailbox RPC dir. The agent posts requests there for the app to
/// execute (see `rpc.rs`). Layered on top of the inherited environment.
fn rpc_env(rpc_dir: &Path) -> Vec<(String, String)> {
    vec![(
        "QUORUM_RPC_DIR".to_string(),
        rpc_dir.to_string_lossy().into_owned(),
    )]
}

impl Agent {
    pub fn spawn_pty<F, G>(spec: SpawnSpec<'_>, on_output: F, on_exit: G) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
        G: Fn(PtyExit) + Send + 'static,
    {
        let (profile_file, args) = prepare_pty_args(&spec)?;
        let env = rpc_env(&spec.rpc_dir);

        tracing::info!(
            agent_id = %spec.agent_id,
            session = %spec.session_id,
            fresh = spec.fresh,
            cwd = %spec.cwd.display(),
            sandbox_root = %spec.sandbox_root.display(),
            profile = %profile_file.path().display(),
            argv = ?args,
            "spawning sandboxed pty agent"
        );

        let pty = PtySession::spawn(
            PtySpawn {
                program: Path::new(SANDBOX_EXEC),
                args: &args,
                cwd: &spec.cwd,
                env: &env,
                cols: spec.cols,
                rows: spec.rows,
            },
            on_output,
            on_exit,
        )?;

        Ok(Self::Pty(PtyAgent {
            pty,
            _profile_file: Some(profile_file),
        }))
    }

    /// Launch a per-turn agent's interactive TUI in a PTY — the native view
    /// for codex/cursor/opencode/pi. Unlike claude's `spawn_pty`, the agent
    /// binary runs directly (no `sandbox-exec`): these agents self-sandbox.
    /// The session is always resumed (`spec.fresh == false`); the supervisor
    /// only routes a per-turn agent here once it has an established session
    /// id, so the TUI continues the same conversation the Custom view built.
    pub fn spawn_pty_native<F, G>(
        spec: SpawnSpec<'_>,
        provider: &str,
        on_output: F,
        on_exit: G,
    ) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
        G: Fn(PtyExit) + Send + 'static,
    {
        let desc = per_turn_descriptor(provider)
            .ok_or_else(|| Error::Other(format!("no per-turn descriptor for `{provider}`")))?;
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let bin = resolve_agent_bin(desc.id, desc.bin, desc.label, &home)?;
        let session = if spec.fresh {
            None
        } else {
            Some(spec.session_id)
        };
        let agent_args = (desc.pty_args)(session, spec.model);
        let env = rpc_env(&spec.rpc_dir);

        // Unified sandbox: run the agent's TUI under sandbox-exec with Quorum's
        // profile (the agent's own sandbox is disabled in its arg builder), so
        // per-turn agents are confined exactly like claude. argv becomes
        // `sandbox-exec -f <profile> <agent-bin> <agent-args…>`.
        let profile_file = prepare_sandbox(&spec.sandbox_root, &spec.rpc_dir, &home)?;
        let profile_path = profile_file
            .path()
            .to_str()
            .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
            .to_string();
        let mut args: Vec<String> = vec!["-f".into(), profile_path, bin.clone()];
        args.extend(agent_args);

        tracing::info!(
            agent_id = %spec.agent_id,
            provider = %provider,
            session = %spec.session_id,
            fresh = spec.fresh,
            cwd = %spec.cwd.display(),
            sandbox_root = %spec.sandbox_root.display(),
            bin = %bin,
            argv = ?args,
            "spawning sandboxed native pty per-turn agent"
        );

        let pty = PtySession::spawn(
            PtySpawn {
                program: Path::new(SANDBOX_EXEC),
                args: &args,
                cwd: &spec.cwd,
                env: &env,
                cols: spec.cols,
                rows: spec.rows,
            },
            on_output,
            on_exit,
        )?;

        Ok(Self::Pty(PtyAgent {
            pty,
            _profile_file: Some(profile_file),
        }))
    }

    pub fn spawn_managed<F, G>(spec: SpawnSpec<'_>, on_event: F, on_exit: G) -> Result<Self>
    where
        F: Fn(Value) + Send + 'static,
        G: Fn(ManagedExit) + Send + 'static,
    {
        let (profile_file, args) = prepare_managed_args(&spec)?;
        let env = rpc_env(&spec.rpc_dir);

        tracing::info!(
            agent_id = %spec.agent_id,
            session = %spec.session_id,
            fresh = spec.fresh,
            cwd = %spec.cwd.display(),
            sandbox_root = %spec.sandbox_root.display(),
            profile = %profile_file.path().display(),
            argv = ?args,
            "spawning sandboxed managed agent"
        );

        let session = ManagedSession::spawn(
            ManagedSpawn {
                program: Path::new(SANDBOX_EXEC),
                args: &args,
                cwd: &spec.cwd,
                env: &env,
            },
            on_event,
            on_exit,
        )?;

        Ok(Self::Managed(ManagedAgent {
            session,
            _profile_file: profile_file,
        }))
    }

    /// Build a per-turn runner (codex, cursor, opencode, pi) from its
    /// `PerTurnDescriptor`. The binary, CLI args, and session-id extraction
    /// come from the descriptor; the lifecycle is the shared `spawn_exec`.
    /// Per-turn agents hold no live process between turns — each user
    /// message spawns a fresh process — and sandbox themselves, so there's
    /// no sandbox-exec profile.
    pub fn spawn_per_turn<F, G, H>(
        desc: &PerTurnDescriptor,
        spec: PerTurnSpec,
        on_event: F,
        on_session_id: G,
        on_turn_exit: H,
    ) -> Result<Self>
    where
        F: Fn(Value) + Send + Sync + 'static,
        G: Fn(String) + Send + Sync + 'static,
        H: Fn(bool) + Send + Sync + 'static,
    {
        let home = dirs::home_dir()
            .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let program = PathBuf::from(resolve_agent_bin(desc.id, desc.bin, desc.label, &home)?);
        Self::spawn_exec(
            program,
            spec,
            desc.build_args,
            desc.session_id,
            !desc.plaintext,
            ExecCallbacks {
                on_event,
                on_session_id,
                on_exit: on_turn_exit,
            },
        )
    }

    /// Shared per-turn exec lifecycle. Spawns no process yet — the first
    /// turn is launched when the first user message arrives. `on_exit(success)`
    /// fires when a turn's process exits (and that turn is still current)
    /// — the per-turn analogue of a turn-end signal, so an interrupted or
    /// failed turn that never emits an in-band turn-end still leaves the
    /// agent promptly.
    fn spawn_exec<A, I, F, G, H>(
        program: PathBuf,
        spec: PerTurnSpec,
        build_args: A,
        extract_session_id: I,
        stdout_is_json: bool,
        cb: ExecCallbacks<F, G, H>,
    ) -> Result<Self>
    where
        A: Fn(&str, Option<&str>, Option<&str>, Option<&str>) -> Vec<String> + Send + Sync + 'static,
        I: Fn(&Value) -> Option<String> + Send + Sync + 'static,
        F: Fn(Value) + Send + Sync + 'static,
        G: Fn(String) + Send + Sync + 'static,
        H: Fn(bool) + Send + Sync + 'static,
    {
        // Unified sandbox: wrap each turn's process in sandbox-exec with
        // Quorum's profile. The agent binary moves into `prefix_args`
        // (`sandbox-exec -f <profile> <agent-bin>`), and the profile tempfile
        // rides on the ExecSession so it outlives the per-turn respawns.
        let home = dirs::home_dir()
            .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let profile_file = prepare_sandbox(&spec.sandbox_root, &spec.rpc_dir, &home)?;
        let profile_path = profile_file
            .path()
            .to_str()
            .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
            .to_string();
        let agent_bin = program
            .to_str()
            .ok_or_else(|| Error::Other("agent bin path not utf-8".into()))?
            .to_string();
        let prefix_args = vec!["-f".to_string(), profile_path, agent_bin];

        tracing::info!(
            program = %SANDBOX_EXEC,
            agent_bin = %program.display(),
            cwd = %spec.cwd.display(),
            sandbox_root = %spec.sandbox_root.display(),
            resume = spec.session_id.is_some(),
            "preparing sandboxed per-turn runner"
        );
        let env = rpc_env(&spec.rpc_dir);
        let session = ExecSession::new(
            ExecSpawn {
                program: PathBuf::from(SANDBOX_EXEC),
                prefix_args,
                profile: Some(profile_file),
                cwd: spec.cwd,
                session_id: spec.session_id,
                model: spec.model,
                stdout_is_json,
                env,
            },
            build_args,
            extract_session_id,
            cb,
        );
        Ok(Self::PerTurn(PerTurnAgent { session }))
    }

    pub fn write_pty(&self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.write(bytes),
            Self::Managed(_) | Self::PerTurn(_) => Err(Error::Other(
                "write_pty called on a managed agent".into(),
            )),
        }
    }

    pub fn send_user_message(&self, text: &str, attachments: &[String], thinking: Option<&str>) -> Result<()> {
        match self {
            Self::Managed(a) => a.session.send_user_message(text, attachments),
            Self::PerTurn(a) => a.session.send_user_message(text, attachments, thinking),
            Self::Pty(_) => Err(Error::Other(
                "send_user_message called on pty agent".into(),
            )),
        }
    }

    /// Answer a held user-input prompt (`AskUserQuestion` / `ExitPlanMode`) by
    /// delivering the user's selection as a control response. Only the managed
    /// (Claude stream-json) transport pauses on tools this way; per-turn and
    /// PTY agents run fully auto-approved and never surface such a prompt.
    pub fn answer_tool_use(
        &self,
        request_id: &str,
        updated_input: serde_json::Value,
        behavior: ToolUseBehavior,
        message: Option<String>,
    ) -> Result<()> {
        match self {
            Self::Managed(a) => a
                .session
                .answer_tool_use(request_id, updated_input, behavior, message),
            Self::PerTurn(_) | Self::Pty(_) => Err(Error::Other(
                "answer_tool_use is only supported for managed agents".into(),
            )),
        }
    }

    /// Interrupt the agent's current turn without terminating the process.
    /// For PTY agents this writes Ctrl+C; for managed agents this sends SIGINT.
    pub fn interrupt(&self) {
        match self {
            Self::Pty(a) => {
                let _ = a.pty.interrupt();
            }
            Self::Managed(a) => {
                a.session.interrupt();
            }
            Self::PerTurn(a) => {
                a.session.interrupt();
            }
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.resize(cols, rows),
            Self::Managed(_) | Self::PerTurn(_) => Ok(()),
        }
    }

    pub fn shutdown(self) -> Result<()> {
        drop(self);
        Ok(())
    }
}

fn prepare_sandbox(
    writable_root: &Path,
    rpc_dir: &Path,
    home: &Path,
) -> Result<tempfile::NamedTempFile> {
    let profile_text = sandbox::build_profile(writable_root, rpc_dir, home)?;
    let mut profile_file = tempfile::Builder::new()
        .prefix("quorum-sandbox-")
        .suffix(".sb")
        .tempfile()
        .map_err(|e| Error::Other(format!("create sandbox profile tmp: {e}")))?;
    profile_file
        .write_all(profile_text.as_bytes())
        .map_err(|e| Error::Other(format!("write sandbox profile: {e}")))?;
    profile_file
        .flush()
        .map_err(|e| Error::Other(format!("flush sandbox profile: {e}")))?;
    Ok(profile_file)
}

/// Claude's session-level effort flag (`--effort <level>`), shared by the
/// managed (custom-view) and PTY (native-view) arg builders. Empty when no
/// effort was selected for the session, so claude falls back to its own
/// default. Effort is a spawn-time flag for the whole session, not per-turn
/// (unlike the per-turn agents' `thinking` arg) — see `providerDetail.ts`.
fn effort_args(effort: Option<&str>) -> Vec<String> {
    match effort {
        Some(level) => vec!["--effort".into(), level.to_string()],
        None => Vec::new(),
    }
}

fn model_args(model: Option<&str>) -> Vec<String> {
    match model {
        Some(id) if !id.trim().is_empty() => vec!["--model".into(), id.to_string()],
        _ => Vec::new(),
    }
}

fn prepare_pty_args(
    spec: &SpawnSpec<'_>,
) -> Result<(tempfile::NamedTempFile, Vec<String>)> {
    let home = dirs::home_dir()
        .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    let claude = resolve_claude(&home)?;
    let profile_file = prepare_sandbox(&spec.sandbox_root, &spec.rpc_dir, &home)?;

    let profile_path = profile_file
        .path()
        .to_str()
        .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
        .to_string();

    let mut args: Vec<String> = vec![
        "-f".into(),
        profile_path,
        claude,
        "--dangerously-skip-permissions".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
    ];
    args.extend(effort_args(spec.effort));
    args.extend(model_args(spec.model));
    args.extend(instructions::append_system_prompt_args());

    if spec.fresh {
        args.push("--session-id".into());
        args.push(spec.session_id.to_string());
    } else {
        args.push("--resume".into());
        args.push(spec.session_id.to_string());
    }

    Ok((profile_file, args))
}

fn prepare_managed_args(
    spec: &SpawnSpec<'_>,
) -> Result<(tempfile::NamedTempFile, Vec<String>)> {
    let home = dirs::home_dir()
        .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    let claude = resolve_claude(&home)?;
    let profile_file = prepare_sandbox(&spec.sandbox_root, &spec.rpc_dir, &home)?;

    let profile_path = profile_file
        .path()
        .to_str()
        .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
        .to_string();

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
        "-f".into(),
        profile_path,
        claude,
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
    args.extend(instructions::append_system_prompt_args());

    if spec.fresh {
        args.push("--session-id".into());
        args.push(spec.session_id.to_string());
    } else {
        args.push("--resume".into());
        args.push(spec.session_id.to_string());
    }

    Ok((profile_file, args))
}

fn resolve_claude(home: &Path) -> Result<String> {
    resolve_agent_bin("claude", "claude", "Claude Code", home)
}

// ── per-turn provider configs ─────────────────────────────────────────────

/// Codex: `codex exec [resume <id>] --json …`. Approvals off and codex's own
/// sandbox set to `danger-full-access` via `-c` (works on both `exec` and
/// `exec resume`, unlike the `-s`/`-a` flags). Quorum now runs codex under
/// sandbox-exec like every other agent, so codex's own confinement is disabled
/// to leave a single boundary — and so codex can reach its RPC mailbox, which
/// lives outside the worktree that `workspace-write` would have confined it to.
fn codex_build_args(prompt: &str, session_id: Option<&str>, thinking: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["exec".into()];
    if let Some(id) = session_id {
        args.push("resume".into());
        args.push(id.to_string());
    }
    args.push("--json".into());
    args.push("--skip-git-repo-check".into());
    args.push("-c".into());
    args.push("approval_policy=\"never\"".into());
    args.push("-c".into());
    args.push("sandbox_mode=\"danger-full-access\"".into());
    if let Some(effort) = thinking {
        args.push("-c".into());
        args.push(format!("reasoning_effort=\"{effort}\""));
    }
    args.extend(model_args(model));
    args.extend(instructions::codex_config_args());
    args.push(prompt.to_string());
    args
}

/// Codex assigns its thread id on the first turn via `thread.started`.
fn codex_session_id(event: &Value) -> Option<String> {
    if event.get("type").and_then(|t| t.as_str()) != Some("thread.started") {
        return None;
    }
    event
        .get("thread_id")
        .and_then(|t| t.as_str())
        .map(str::to_string)
}

/// Cursor: `cursor-agent -p --output-format stream-json --force [--resume <id>] <prompt>`.
/// `--force` runs commands without approval prompts; `--trust` trusts the
/// workspace in headless mode. Cursor's own sandbox applies; cwd comes from
/// the child process working directory.
fn cursor_build_args(prompt: &str, session_id: Option<&str>, _thinking: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--force".into(),
        "--trust".into(),
    ];
    if let Some(id) = session_id {
        args.push("--resume".into());
        args.push(id.to_string());
    }
    args.extend(model_args(model));
    // Prompt is positional and must come after options.
    args.push(instructions::prepend_to_prompt(prompt, session_id));
    args
}

/// Cursor assigns its session id on the first turn, reported on the
/// `system`/`init` event (and echoed on every later event).
fn cursor_session_id(event: &Value) -> Option<String> {
    if event.get("type").and_then(|t| t.as_str()) != Some("system") {
        return None;
    }
    if event.get("subtype").and_then(|s| s.as_str()) != Some("init") {
        return None;
    }
    event
        .get("session_id")
        .and_then(|s| s.as_str())
        .map(str::to_string)
}

/// OpenCode: `opencode run --format json --dangerously-skip-permissions [--session <id>] <prompt>`.
/// `--dangerously-skip-permissions` auto-approves tools (incl. shell + file
/// writes) so turns run unattended; verified end-to-end against opencode
/// 1.15.12. OpenCode runs in the child's cwd (no `--dir` needed) and assigns
/// its own session id on the first turn. The prompt is positional and must
/// come after the flags.
fn opencode_build_args(prompt: &str, session_id: Option<&str>, thinking: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--format".into(),
        "json".into(),
        "--dangerously-skip-permissions".into(),
        // Surface the model's reasoning as `reasoning` events (captured by the
        // opencode reducer and persisted via opencode_is_durable).
        "--thinking".into(),
    ];
    if let Some(variant) = thinking {
        args.push("--variant".into());
        args.push(variant.to_string());
    }
    args.extend(model_args(model));
    if let Some(id) = session_id {
        args.push("--session".into());
        args.push(id.to_string());
    }
    args.push(instructions::prepend_to_prompt(prompt, session_id));
    args
}

/// OpenCode stamps the session id (`ses_…`) on the top-level `sessionID`
/// field of every event, so the first event of the first turn carries it.
/// `maybe_capture_session_id` captures it once and ignores the later echoes.
fn opencode_session_id(event: &Value) -> Option<String> {
    event
        .get("sessionID")
        .and_then(|s| s.as_str())
        .map(str::to_string)
}

/// Pi: `pi -p --mode json [--session <id>] <prompt>`. `-p` runs one turn
/// non-interactively and exits; in that mode Pi auto-runs its tools (bash,
/// write, …) with no approval prompt. Pi assigns its own session id on the
/// first turn (captured from the `session` event), and `--session <id>`
/// resumes it. We deliberately use `--session` (not the newer `--session-id`):
/// it's the resume flag common to the versions we target — 0.74.x lacks
/// `--session-id` entirely. Verified end-to-end against pi 0.74.2. Pi runs in
/// the child's cwd; the prompt is positional and must come after the flags.
fn pi_build_args(prompt: &str, session_id: Option<&str>, thinking: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["-p".into(), "--mode".into(), "json".into()];
    args.extend(model_args(model));
    if let Some(level) = thinking {
        args.push("--thinking".into());
        args.push(level.to_string());
    }
    args.extend(instructions::append_system_prompt_args());
    if let Some(id) = session_id {
        args.push("--session".into());
        args.push(id.to_string());
    }
    args.push(prompt.to_string());
    args
}

/// Pi reports its session id on the first `{"type":"session","id":"…"}` event.
fn pi_session_id(event: &Value) -> Option<String> {
    if event.get("type").and_then(|t| t.as_str()) != Some("session") {
        return None;
    }
    event.get("id").and_then(|s| s.as_str()).map(str::to_string)
}

// ── native (PTY/TUI) arg builders ───────────────────────────────────────────
//
// These launch each agent's *interactive* TUI inside a PTY (the native view),
// as opposed to the one-shot JSON `*_build_args` used by the structured Custom
// view. `session_id == None` starts a fresh interactive session; `Some(id)`
// resumes the prior one. The PTY runs in the agent's cwd (set by `PtySession`),
// so none of these need a working-dir flag. Verified against codex-cli 0.135,
// cursor-agent 2026.06, opencode 1.15, pi 0.74+.

/// Codex: bare `codex` launches the interactive TUI;
/// `--dangerously-bypass-approvals-and-sandbox` runs it unattended (Quorum
/// already isolates the worktree). `resume <id>` continues a prior session.
fn codex_pty_args(session_id: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["--dangerously-bypass-approvals-and-sandbox".into()];
    args.extend(model_args(model));
    args.extend(instructions::codex_config_args());
    if let Some(id) = session_id {
        args.push("resume".into());
        args.push(id.to_string());
    }
    args
}

/// Cursor: bare `cursor-agent` launches the TUI; `--force` auto-allows
/// commands. `--resume <id>` continues a prior chat.
fn cursor_pty_args(session_id: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["--force".into()];
    args.extend(model_args(model));
    if let Some(id) = session_id {
        args.push("--resume".into());
        args.push(id.to_string());
    }
    args
}

/// OpenCode: bare `opencode` launches the interactive TUI; `--session <id>`
/// continues a prior session. Note: no auto-approve flag — that's
/// `--dangerously-skip-permissions`, which belongs to the `run` (headless)
/// subcommand and makes the *default* (TUI) command print help and exit. The
/// TUI prompts for tool permissions interactively, which the native view
/// handles like any other keystroke.
fn opencode_pty_args(session_id: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    args.extend(model_args(model));
    if let Some(id) = session_id {
        args.push("--session".into());
        args.push(id.to_string());
    }
    args
}

/// Pi: bare `pi` launches the interactive TUI (tools auto-run there).
/// `--session <id>` resumes — same flag the Custom-view runner uses, since the
/// versions we target (0.74.x) lack `--session-id`.
fn pi_pty_args(session_id: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = instructions::append_system_prompt_args();
    args.extend(model_args(model));
    if let Some(id) = session_id {
        args.push("--session".into());
        args.push(id.to_string());
    }
    args
}

/// Locate an agent CLI by name: PATH first, then the user's login shell
/// (catches nvm / fnm / volta / homebrew setups the GUI process's bare
/// PATH misses), then the usual install dirs. `label` is the
/// human-facing product name used only in the not-found error.
fn resolve_agent_bin(agent_id: &str, name: &str, label: &str, home: &Path) -> Result<String> {
    // A user-set custom path wins over PATH discovery. If it no longer points
    // at an executable we surface a clear error rather than silently falling
    // back to a different binary off PATH — the user chose this one explicitly.
    if let Some(result) = crate::bin_resolve::resolve_agent_override(agent_id, home) {
        return result.map_err(|path| {
            Error::Other(format!(
                "The custom binary path for {label} is not executable: {path}"
            ))
        });
    }
    crate::bin_resolve::resolve_bin(name, home).ok_or_else(|| {
        Error::Other(format!(
            "Could not find the `{name}` executable. Install {label} or make it available on PATH."
        ))
    })
}

// ── Version probing ───────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct ProviderProbe {
    pub id: String,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(serde::Serialize)]
pub struct BinValidation {
    /// The path is an executable regular file (after `~` expansion).
    pub executable: bool,
    /// The version `<path> --version` reported, if it ran and parsed.
    pub version: Option<String>,
}

/// Pre-flight a user-entered custom binary path before it's saved as an
/// override: expand a leading `~`, confirm it's an executable file, and probe
/// `--version` when it is. Powers the providers settings UI's inline feedback.
pub fn validate_bin(path: &str) -> BinValidation {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let expanded = crate::bin_resolve::expand_tilde(path, &home);
    let executable = crate::bin_resolve::is_executable_path(&expanded);
    let version = if executable {
        probe_version(&expanded.to_string_lossy())
    } else {
        None
    };
    BinValidation { executable, version }
}

/// Probe every known provider in parallel and return their resolved path +
/// version string. Missing/uninstalled providers return `None` for both fields;
/// the frontend falls back to the hardcoded defaults in that case.
pub async fn probe_all_providers() -> Vec<ProviderProbe> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

    // (id, bin_name, human_label)
    let mut targets: Vec<(&str, &str, &str)> = vec![("claude", "claude", "Claude Code")];
    for d in PER_TURN_AGENTS {
        targets.push((d.id, d.bin, d.label));
    }

    let mut handles = Vec::new();
    for (id, bin, label) in targets {
        let home = home.clone();
        let id = id.to_string();
        let bin = bin.to_string();
        let label = label.to_string();
        handles.push(tokio::task::spawn_blocking(move || {
            let path = resolve_agent_bin(&id, &bin, &label, &home).ok();
            let version = path.as_deref().and_then(probe_version);
            ProviderProbe { id, version, path }
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        if let Ok(probe) = handle.await {
            results.push(probe);
        }
    }
    results
}

/// Run `<bin> --version` and extract the first semver-like token from stdout
/// (or stderr as fallback). Returns `None` if the binary errors or emits no
/// recognisable version.
fn probe_version(bin: &str) -> Option<String> {
    let out = Command::new(bin).arg("--version").output().ok()?;
    let text = if !out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    parse_semver(&text)
}

/// Extract the first `N.N[.N[.N]]` token from arbitrary version output.
/// Strips a leading `v` from each word before testing so `v1.0.42` and
/// `1.0.42` both match. Returns the token with a `v` prefix.
fn parse_semver(s: &str) -> Option<String> {
    for word in s.split_whitespace() {
        let word = word.trim_start_matches('v');
        // Accept anything that is purely digit-and-dot with at least one dot.
        if word.contains('.') && word.chars().all(|c| c.is_ascii_digit() || c == '.') && !word.starts_with('.') && !word.ends_with('.') {
            return Some(format!("v{word}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
                "{}\n{}\n{}",
                r#"{"uuid":"a","type":"user"}"#,
                r#"{"uuid":"b","type":"assistant"}"#,
                r#"{"uuid":"c","#
            )
            .unwrap();
        }

        let (recs, off) = read_jsonl_tail(&path, 0, 0, Some("uuid"), false);
        assert_eq!(
            recs.iter().map(|r| r.native_id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"],
            "only complete lines ingested; the torn line is held back",
        );

        // The writer finishes line c and appends d. Resume from the held offset
        // with start_index = 2 (we consumed 2 records).
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            write!(f, "{}\n{}\n", r#""type":"assistant"}"#, r#"{"uuid":"d","type":"user"}"#).unwrap();
        }

        let (recs2, _off2) = read_jsonl_tail(&path, off, 2, Some("uuid"), false);
        assert_eq!(
            recs2.iter().map(|r| r.native_id.as_str()).collect::<Vec<_>>(),
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
            write!(f, "{}\n{}\n", r#"{"type":"mode"}"#, r#"{"type":"summary"}"#).unwrap();
        }
        // Pretend 5 records were already ingested.
        let (recs, _off) = read_jsonl_tail(&path, 0, 5, None, false);
        assert_eq!(
            recs.iter().map(|r| r.native_id.as_str()).collect::<Vec<_>>(),
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
                "{}\n{}",
                r#"{"uuid":"a","type":"user"}"#,
                r#"{"uuid":"b","type":"assistant"}"#,
            )
            .unwrap();
        }

        // Exited writer: the unterminated final line is consumed, to EOF.
        let (recs, off) = read_jsonl_tail(&path, 0, 0, Some("uuid"), true);
        assert_eq!(
            recs.iter().map(|r| r.native_id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"],
        );
        assert_eq!(off, std::fs::metadata(&path).unwrap().len(), "consumed to EOF");

        // Live writer (same bytes): the unterminated final line is held back.
        let (held, _) = read_jsonl_tail(&path, 0, 0, Some("uuid"), false);
        assert_eq!(
            held.iter().map(|r| r.native_id.as_str()).collect::<Vec<_>>(),
            vec!["a"],
        );

        // A genuinely torn final line (invalid JSON) is held even when consuming.
        let torn = dir.path().join("torn.jsonl");
        {
            let mut f = std::fs::File::create(&torn).unwrap();
            write!(f, "{}\n{}", r#"{"uuid":"a","type":"user"}"#, r#"{"uuid":"x","#).unwrap();
        }
        let (recs_torn, _) = read_jsonl_tail(&torn, 0, 0, Some("uuid"), true);
        assert_eq!(
            recs_torn.iter().map(|r| r.native_id.as_str()).collect::<Vec<_>>(),
            vec!["a"],
            "a mid-write torn line is held even with consume_trailing",
        );
    }

    #[test]
    fn provider_bin_label_dispatch() {
        assert_eq!(provider_bin_label("claude"), Some(("claude", "Claude Code")));
        assert!(provider_bin_label("codex").is_some());
        assert!(provider_bin_label("pi").is_some());
        assert_eq!(provider_bin_label("antigravity"), Some(("agy", "Antigravity")));
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
        let recs = antigravity_read(&[f]);
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0].native_id, "step:0");
        assert_eq!(recs[1].native_id, "step:2");
        assert_eq!(recs[2].native_id, "ln:2"); // no step_index → positional
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
            pi_session_slug(Path::new("/Users/alex/.quorum/worktrees/balkhash/agent")),
            "--Users-alex-.quorum-worktrees-balkhash-agent--"
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
        let args = opencode_build_args("hi", None, None, None);
        assert!(args.contains(&"--thinking".to_string()));
        assert!(args.contains(&"--format".to_string()));
        // Prompt is positional and last (possibly prefixed with injected
        // instructions on a fresh turn, so match the tail rather than equality).
        assert!(args.last().unwrap().ends_with("hi"), "prompt is positional and last");
    }

    // ── pty (native TUI) args ──────────────────────────────────────────────

    #[test]
    fn codex_pty_args_launch_tui_fresh_and_resume() {
        // Fresh: bypass approvals/sandbox so the TUI runs unattended; no
        // `exec`/`resume` subcommand means the interactive CLI.
        let fresh = codex_pty_args(None, None);
        assert!(fresh.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(!fresh.iter().any(|a| a == "resume"));
        // Resume: `resume <id>` continues the prior interactive session.
        let resume = codex_pty_args(Some("abc123"), None);
        assert!(resume.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        let pos = resume
            .iter()
            .position(|a| a == "resume")
            .expect("resume subcommand");
        assert_eq!(resume.get(pos + 1).map(String::as_str), Some("abc123"));
    }

    #[test]
    fn cursor_pty_args_force_and_resume() {
        let fresh = cursor_pty_args(None, None);
        assert!(fresh.contains(&"--force".to_string()));
        assert!(!fresh.iter().any(|a| a == "--resume"));
        let resume = cursor_pty_args(Some("chat-1"), None);
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
        let fresh = opencode_pty_args(None, None);
        assert!(!fresh.iter().any(|a| a == "--dangerously-skip-permissions"));
        assert!(fresh.is_empty());
        // Resume: `--session <id>` continues the prior session.
        let resume = opencode_pty_args(Some("ses_9"), None);
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
        let fresh = pi_pty_args(None, None);
        assert!(!fresh.iter().any(|a| a == "--session"), "fresh TUI has no resume flag");
        // Resume uses `--session <id>` (target pi 0.74.x lacks `--session-id`).
        let resume = pi_pty_args(Some("u-7"), None);
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
            let fresh = (d.pty_args)(None, None);
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
        let args = opencode_build_args("hi", None, Some("max"), None);
        assert!(args.contains(&"--variant".to_string()));
        assert!(args.contains(&"max".to_string()));
    }

    #[test]
    fn codex_args_reasoning_effort_when_thinking_set() {
        let args = codex_build_args("hi", None, Some("high"), None);
        assert!(args.contains(&"reasoning_effort=\"high\"".to_string()));
    }

    #[test]
    fn codex_args_disable_codex_own_sandbox() {
        // Quorum now wraps codex in sandbox-exec, so codex's own confinement is
        // turned fully off — otherwise its workspace-write sandbox would block
        // the RPC mailbox, which lives outside the worktree.
        let args = codex_build_args("hi", None, None, None);
        assert!(args.contains(&"sandbox_mode=\"danger-full-access\"".to_string()));
        assert!(!args.iter().any(|a| a.contains("workspace-write")));
        assert!(args.contains(&"approval_policy=\"never\"".to_string()));
    }

    #[test]
    fn pi_args_thinking_when_set() {
        let args = pi_build_args("hi", None, Some("xhigh"), None);
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
        let with_none = cursor_build_args("hi", None, None, None);
        let with_some = cursor_build_args("hi", None, Some("high"), None);
        assert_eq!(with_none, with_some);
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
}

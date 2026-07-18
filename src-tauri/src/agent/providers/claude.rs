//! Claude transcript reader.
//!
//! Claude is the lone persistent-runner agent (not in PER_TURN_AGENTS), launched
//! `--session-id <uuid>` / `--resume <uuid>`, so it writes
//! `<config-dir>/projects/<slug>/<uuid>.jsonl` (config-dir = CLAUDE_CONFIG_DIR or
//! `~/.claude`). find_session_jsonl locates it, honoring the Docker sandbox's
//! per-agent transcript dir (derived from `cwd`). Content lines carry a top-level
//! `uuid`; metadata lines (mode/permission-mode/…) don't → positional fallback.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::transcript::{records_with_id, JsonlTail, RawRecord, ReadDiagnostics, TranscriptReader};

fn claude_locate(session_id: &str, cwd: &Path, diag: &mut ReadDiagnostics) -> Vec<PathBuf> {
    crate::transcripts::find_session_jsonl(session_id, cwd, diag)
        .into_iter()
        .collect()
}

fn claude_read(paths: &[PathBuf], diag: &mut ReadDiagnostics) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::transcripts::read_jsonl_values(p, diag))
        .collect();
    records_with_id(values, Some("uuid"))
}

pub(crate) static CLAUDE_TRANSCRIPT: TranscriptReader = TranscriptReader {
    locate: claude_locate,
    read: claude_read,
    tail: Some(JsonlTail {
        id_field: Some("uuid"),
    }), // single persistent jsonl
};

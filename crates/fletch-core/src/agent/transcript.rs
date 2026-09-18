//! Transcript record types and generic JSONL reading.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// One verbatim durable record from an agent's transcript: the raw body in the
/// agent's own shape plus a stable per-record dedup key (`native_id`).
#[derive(Debug, Clone)]
pub struct RawRecord {
    pub native_id: String,
    pub body: Value,
}

/// Counters a `locate`/`read` pass fills in as it walks a provider's transcript
/// on disk. Purely observational — never an error, and a reader must still
/// return every record that DID parse (partial drift must not lose good
/// records). The turn-end sync classifies these into a health signal to tell a
/// benign "nothing flushed yet" apart from a vendor moving/reshaping its files
/// (see `supervisor::session_sync`).
#[derive(Default, Debug, Clone)]
pub struct ReadDiagnostics {
    /// The provider's transcript root dir was found (for Claude, EITHER of its
    /// two candidate roots counts). False means the CLI's home dir is gone —
    /// a strong drift signal when the CLI just ran a turn.
    pub root_exists: bool,
    /// Located transcript artifacts (locate glob/suffix hits). For OpenCode this
    /// also includes the part-blob files discovered during `read`.
    pub files_matched: usize,
    /// Non-blank JSONL lines (or JSON files) actually read. Blank lines are
    /// normal and never counted, so `lines_seen > records_parsed` means real
    /// parse failures — the fingerprint of a format change.
    pub lines_seen: usize,
    /// Records that parsed cleanly out of `lines_seen`.
    pub records_parsed: usize,
    /// I/O failures opening/reading a located artifact.
    pub io_errors: usize,
}

impl ReadDiagnostics {
    /// Fold another pass's counters into this one. The turn-end sync polls
    /// repeatedly and classifies the turn as a whole, so counters accumulate
    /// across passes — a clean settle read must not erase an earlier pass's
    /// errors.
    pub fn absorb(&mut self, other: &ReadDiagnostics) {
        self.root_exists |= other.root_exists;
        self.files_matched += other.files_matched;
        self.lines_seen += other.lines_seen;
        self.records_parsed += other.records_parsed;
        self.io_errors += other.io_errors;
    }
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
    /// Records `root_exists` / `files_matched` into `diag` as it scans.
    pub locate: fn(session_id: &str, cwd: &Path, diag: &mut ReadDiagnostics) -> Vec<PathBuf>,
    /// Parse located artifacts into ordered verbatim records, recording
    /// `lines_seen` / `records_parsed` / `io_errors` into `diag`.
    pub read: fn(paths: &[PathBuf], diag: &mut ReadDiagnostics) -> Vec<RawRecord>,
    /// Set for single-file JSONL readers to enable incremental tail ingest.
    /// `None` for multi-file (codex) / blob-dir (opencode) readers, which fall
    /// back to a full read + idempotent batched insert.
    pub tail: Option<JsonlTail>,
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
    diag: &mut ReadDiagnostics,
) -> (Vec<RawRecord>, u64) {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        diag.io_errors += 1;
        return (Vec::new(), offset);
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        diag.io_errors += 1;
        return (Vec::new(), offset);
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        diag.io_errors += 1;
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

    // A blank line is normal padding, not content — only non-blank lines count
    // toward `lines_seen`, so a non-blank line that fails to parse is what marks
    // a format drift (`lines_seen > records_parsed`).
    let count_line = |line: &[u8], diag: &mut ReadDiagnostics| {
        let nonblank = std::str::from_utf8(line).map_or(true, |t| !t.trim().is_empty());
        if nonblank {
            diag.lines_seen += 1;
        }
    };

    for line in buf[..complete_end].split(|&b| b == b'\n') {
        count_line(line, diag);
        if let Some(rec) = parse_jsonl_record(line, id_field, idx) {
            diag.records_parsed += 1;
            out.push(rec);
            idx += 1;
        }
    }

    if consume_trailing {
        count_line(&buf[complete_end..], diag);
        if let Some(rec) = parse_jsonl_record(&buf[complete_end..], id_field, idx) {
            diag.records_parsed += 1;
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
pub(crate) fn jsonl_files_ending(dir: &Path, suffix: &str) -> Vec<PathBuf> {
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

/// Sorted `*.json` files directly in `dir` (filenames are id-prefixed, so
/// lexical sort == creation order).
pub(crate) fn json_files_in(dir: &Path) -> Vec<PathBuf> {
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

pub(crate) fn read_json_value(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

//! Incremental, per-file parse cache.
//!
//! Parsing yields per-file [`UsageRecord`]s; aggregation (windowing, the global
//! Claude dedupe, bucketing) runs afterwards over those records. That split is
//! what makes caching possible: records are window-independent, so a cached
//! file can answer any later window without being re-read.
//!
//! A [`ScanCache`] holds, per file path, the records parsed so far plus the
//! `(len, mtime, offset)` triple that says how much of the file they cover. On
//! the next scan each known file is `stat`-ed: unchanged → records reused with
//! no I/O; grown → only the bytes after `offset` are read and parsed (for Codex
//! the state machine resumes from its saved state); shrunk or rewritten → the
//! entry is dropped and the file re-read from 0. A trailing partial line (no
//! `\n` yet) is never consumed, so a record still being written is picked up
//! whole on the following scan. [`scan_all`](super::scan_all) uses one
//! process-lifetime cache; tests and callers that want isolation use
//! [`scan_dirs_with`](super::scan_dirs_with).
//!
//! The `(len, mtime)` fingerprint is what says "this file is done", so it is
//! only ever written after a read that reached the end of the file. An I/O
//! error part-way through keeps the records and the offset it did reach — the
//! progress is real — but leaves the fingerprint unmatchable, so the next scan
//! picks the file back up instead of trusting a half-read entry forever. See
//! [`ReadOutcome`].

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::parse::{parse_claude_line, parse_codex_line, CodexState, UsageRecord};
use super::Provider;

/// What one file contributed, plus enough metadata to resume it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FileEntry {
    /// File length when `records`/`offset` were last brought up to date — i.e.
    /// when a read last reached the end of the file. Left at the
    /// [`UNREAD_LEN`]/[`UNREAD_MTIME_MS`] sentinels while the entry is only
    /// part-read, which no real `stat` can match, so the next scan resumes it.
    pub(super) len: u64,
    pub(super) mtime_ms: i64,
    /// Bytes consumed, always just past a `\n`: a trailing partial line is left
    /// for the next scan to read whole.
    pub(super) offset: u64,
    pub(super) records: Vec<UsageRecord>,
    /// Codex only: the state machine's position, so appended lines parse as if
    /// the whole file had been read in one pass.
    pub(super) codex: Option<CodexState>,
}

/// Fingerprint of an entry no read has finished yet. A real file of length 0
/// has nothing to parse, and no `stat` reports [`UNREAD_MTIME_MS`], so an entry
/// carrying these can never be mistaken for up to date.
const UNREAD_LEN: u64 = 0;
const UNREAD_MTIME_MS: i64 = i64::MIN;

impl FileEntry {
    fn new(provider: Provider) -> Self {
        Self {
            len: UNREAD_LEN,
            mtime_ms: UNREAD_MTIME_MS,
            offset: 0,
            records: Vec::new(),
            // Also picks what per-file state has to survive between scans:
            // Claude lines are self-describing, Codex lines are not.
            codex: match provider {
                Provider::Claude => None,
                Provider::Codex => Some(CodexState::default()),
            },
        }
    }

    /// Codex session id for this file, falling back to its path so a rollout
    /// with no `session_meta` still counts as one distinct session.
    pub(super) fn codex_session_id(&self, path: &Path) -> String {
        self.codex
            .as_ref()
            .and_then(|s| s.session_id.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| path.to_string_lossy().into_owned())
    }
}

/// A transcript opened for reading. Boxed rather than generic so the cache can
/// be handed a different source in tests without infecting every caller with a
/// type parameter.
pub(super) trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// How a scan gets at a transcript's bytes. Only tests ever set one; production
/// opens the real file.
#[cfg(test)]
pub(super) type Opener = Box<dyn Fn(&Path) -> std::io::Result<Box<dyn ReadSeek>> + Send + Sync>;

/// Per-path parse cache. Lives for the process in
/// [`scan_all`](super::scan_all); tests own theirs. Change detection is
/// `(len, mtime-in-ms)`, which cannot see an in-place rewrite that preserves
/// the exact length within the same millisecond — transcripts are append-only,
/// so that doesn't happen.
#[derive(Default)]
pub struct ScanCache {
    pub(super) files: HashMap<PathBuf, FileEntry>,
    /// Test-only override for [`ScanCache::open`], so a scan can be made to
    /// fail at open, at seek, or part-way through a file.
    #[cfg(test)]
    pub(super) opener: Option<Opener>,
}

#[derive(Default)]
pub(super) struct IoStats {
    pub(super) files_read: u32,
    pub(super) bytes_read: u64,
}

impl ScanCache {
    fn open(&self, path: &Path) -> std::io::Result<Box<dyn ReadSeek>> {
        #[cfg(test)]
        if let Some(opener) = &self.opener {
            return opener(path);
        }
        Ok(Box::new(std::fs::File::open(path)?))
    }

    /// Bring `path`'s entry up to date, reading as little as possible. Returns
    /// whether the file has an entry to aggregate (`false` = prefiltered away,
    /// or unreadable with nothing cached).
    pub(super) fn refresh(
        &mut self,
        path: &Path,
        provider: Provider,
        since_ms: i64,
        io: &mut IoStats,
    ) -> bool {
        let Some(stat) = file_stat(path) else {
            // Size/mtime unknown (racing deletion, odd permissions): read the
            // whole thing, as the pre-cache scanner did, and leave the entry
            // unfingerprinted so the next scan does it again.
            return self.read_fully(path, provider, None, io);
        };
        let (len, mtime_ms) = stat;

        match self.files.get(path) {
            // Same bytes as last time: reuse the records, touch no I/O.
            Some(e) if e.len == len && e.mtime_ms == mtime_ms => true,
            // Appended to, with the cached offset still inside the file: read
            // from where the last scan stopped.
            Some(e) if len > e.len && e.offset <= len => {
                self.read_tail(path, len, mtime_ms, io);
                true
            }
            // Truncated, or rewritten in place: nothing cached can be trusted.
            Some(_) => self.read_fully(path, provider, Some(stat), io),
            // The mtime prefilter only applies to files never read: one last
            // written before the window opened cannot hold an in-window record.
            // A file with an entry is always cheap enough to stat and reuse.
            None if mtime_ms < since_ms => false,
            None => self.read_fully(path, provider, Some(stat), io),
        }
    }

    /// Parse the bytes appended since the last scan into `path`'s entry.
    fn read_tail(&mut self, path: &Path, len: u64, mtime_ms: i64, io: &mut IoStats) {
        // Opened before the entry is borrowed; a failure here leaves the entry
        // exactly as it was, fingerprint included, so the next scan retries.
        let Ok(reader) = self.open(path) else { return };
        let entry = self.files.get_mut(path).expect("caller matched an entry");
        let from = entry.offset;
        if let Ok(ReadOutcome::Complete { .. }) = read_into(entry, reader, from, io) {
            entry.len = len;
            entry.mtime_ms = mtime_ms;
        }
    }

    /// Replace `path`'s entry with one parsed from the first byte. `stat` is
    /// the fingerprint to record once the read reaches the end of the file, or
    /// `None` when the file couldn't be stat-ed at all. Returns whether `path`
    /// has an entry to aggregate afterwards.
    fn read_fully(
        &mut self,
        path: &Path,
        provider: Provider,
        stat: Option<(u64, i64)>,
        io: &mut IoStats,
    ) -> bool {
        let Ok(reader) = self.open(path) else {
            // Nothing was read, so nothing is known: keep whatever earlier
            // scans cached (it still describes bytes that are still on disk)
            // rather than replacing it with an empty entry, and don't invent
            // one where there was none.
            return self.files.contains_key(path);
        };
        let mut entry = FileEntry::new(provider);
        // Only a read that reached the end of the file may claim the
        // fingerprint; an interrupted one keeps its records and offset under
        // the unread sentinels, so the next scan resumes from there.
        if let (Ok(ReadOutcome::Complete { .. }), Some((len, mtime_ms))) =
            (read_into(&mut entry, reader, 0, io), stat)
        {
            entry.len = len;
            entry.mtime_ms = mtime_ms;
        }
        self.files.insert(path.to_path_buf(), entry);
        true
    }
}

/// Parse `reader` from `start` into `entry`, advancing its consumed offset.
fn read_into(
    entry: &mut FileEntry,
    reader: impl Read + Seek,
    start: u64,
    io: &mut IoStats,
) -> std::io::Result<ReadOutcome> {
    let FileEntry { records, codex, .. } = entry;
    let outcome = match codex {
        Some(state) => read_lines_from(reader, start, io, |line| {
            parse_codex_line(line, state, records)
        }),
        None => read_lines_from(reader, start, io, |line| {
            if let Some(record) = parse_claude_line(line) {
                records.push(record);
            }
        }),
    }?;
    entry.offset = outcome.offset();
    Ok(outcome)
}

/// Size and mtime in one syscall — the cache's change detector, and the input
/// to the prefilter that keeps a full-disk scan of a short window cheap.
/// `None` when the file can't be stat-ed at all.
fn file_stat(path: &Path) -> Option<(u64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let mtime_ms = match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        // Pre-epoch mtime: ancient, so the prefilter always drops it.
        Err(e) => -(e.duration().as_millis() as i64),
    };
    Some((meta.len(), mtime_ms))
}

/// Why a streaming read stopped, and how far it got. The distinction is the
/// cache's correctness hinge: `Complete` means the bytes up to `offset` are
/// everything the file had, so the file's `(len, mtime)` may be recorded as
/// processed; `Interrupted` means an I/O error cut the stream short, so the
/// records parsed so far are kept but the fingerprint must not advance, or the
/// missing usage would be invisible until the transcript changed again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadOutcome {
    Complete { offset: u64 },
    Interrupted { offset: u64 },
}

impl ReadOutcome {
    /// Bytes consumed, always just past a `\n` — the last *complete* line.
    fn offset(self) -> u64 {
        match self {
            ReadOutcome::Complete { offset } | ReadOutcome::Interrupted { offset } => offset,
        }
    }
}

/// Stream `reader` line by line from `start`, reusing one buffer. Never loads
/// the whole file: real transcripts reach hundreds of megabytes. A half-written
/// trailing line (no `\n` yet) is not consumed and is not an interruption — the
/// file simply ends there for now, and the next scan reads it whole.
///
/// `Err` only for a seek that fails outright, i.e. nothing was read at all;
/// a failure part-way through is [`ReadOutcome::Interrupted`], which keeps the
/// lines that did parse.
fn read_lines_from(
    mut reader: impl Read + Seek,
    start: u64,
    io: &mut IoStats,
    mut f: impl FnMut(&str),
) -> std::io::Result<ReadOutcome> {
    if start > 0 {
        reader.seek(SeekFrom::Start(start))?;
    }
    io.files_read += 1;

    let mut reader = BufReader::with_capacity(64 * 1024, reader);
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut consumed = start;
    loop {
        buf.clear();
        let read = match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            // Mid-file I/O error: whatever this call half-read is unusable, but
            // every line before it is real.
            Err(_) => return Ok(ReadOutcome::Interrupted { offset: consumed }),
        };
        io.bytes_read += read as u64;
        if buf.last() != Some(&b'\n') {
            // Partial line, still being appended to: don't consume it.
            break;
        }
        consumed += read as u64;
        // Lossy rather than strict: one mis-encoded byte shouldn't drop the
        // rest of a transcript.
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if !line.is_empty() {
            f(line);
        }
    }
    Ok(ReadOutcome::Complete { offset: consumed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_scan::test_support::*;
    use crate::usage_scan::{scan_dirs, scan_dirs_with, UsageScan};

    #[test]
    fn a_second_scan_of_unchanged_files_reads_nothing_and_answers_the_same() {
        let td = tempfile::tempdir().unwrap();
        let projects = claude_file(
            td.path(),
            "slug",
            "s1",
            &[claude_line(
                "s1",
                "m1",
                "r1",
                "2026-01-02T10:00:00Z",
                "claude-opus-4",
                claude_usage(10, 5, 0, 0),
            )],
        );
        let sessions = codex_file(
            td.path(),
            "a",
            &[
                serde_json::json!({
                    "timestamp": "2026-01-02T10:00:00Z",
                    "type": "session_meta",
                    "payload": { "id": "codex-1" },
                })
                .to_string(),
                codex_turn_context("2026-01-02T10:00:00Z", "gpt-5"),
                codex_token_count("2026-01-02T10:00:01Z", codex_usage(100, 60, 10, 20)),
            ],
        );

        let (since, until) = wide_window();
        let mut cache = ScanCache::default();
        let cold = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            std::slice::from_ref(&sessions),
            since,
            until,
        );
        assert_eq!(cold.files_read, 2);
        assert!(cold.bytes_read > 0);

        let warm = scan_dirs_with(&mut cache, &[projects], &[sessions], since, until);
        assert_eq!(
            (warm.files_read, warm.bytes_read),
            (0, 0),
            "nothing changed on disk, so nothing may be opened"
        );
        assert_eq!(answer(&warm), answer(&cold));
        assert_eq!(warm.scanned_files, 2, "reused files still count as scanned");
    }

    #[test]
    fn appending_a_claude_record_reads_only_the_appended_bytes() {
        let td = tempfile::tempdir().unwrap();
        let projects = claude_file(
            td.path(),
            "slug",
            "s1",
            &[claude_line(
                "s1",
                "m1",
                "r1",
                "2026-01-02T10:00:00Z",
                "claude-opus-4",
                claude_usage(10, 5, 0, 0),
            )],
        );
        let file = claude_path(&projects, "slug", "s1");

        let (since, until) = wide_window();
        let mut cache = ScanCache::default();
        let cold = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        assert_eq!(cold.buckets[0].requests, 1);

        let added = append(
            &file,
            &format!(
                "{}\n",
                claude_line(
                    "s1",
                    "m2",
                    "r2",
                    "2026-01-02T10:10:00Z",
                    "claude-opus-4",
                    claude_usage(1, 2, 0, 0),
                )
            ),
        );
        let warm = scan_dirs_with(&mut cache, &[projects], &[], since, until);
        assert_eq!((warm.files_read, warm.bytes_read), (1, added));
        assert_eq!(warm.buckets.len(), 1);
        assert_eq!(warm.buckets[0].requests, 2);
        assert_eq!(warm.buckets[0].tokens.input, 11);
        assert_eq!(warm.buckets[0].tokens.output, 7);
    }

    #[test]
    fn appending_to_a_codex_rollout_resumes_the_state_machine() {
        // Model, session id and last-usage all live in the parser's state, so a
        // tail-read has to pick up exactly where the previous scan stopped.
        let td = tempfile::tempdir().unwrap();
        let sessions = codex_file(
            td.path(),
            "a",
            &[
                serde_json::json!({
                    "timestamp": "2026-01-02T09:00:00Z",
                    "type": "session_meta",
                    "payload": { "id": "codex-1" },
                })
                .to_string(),
                codex_turn_context("2026-01-02T10:00:00Z", "gpt-5"),
                codex_token_count("2026-01-02T10:00:01Z", codex_usage(100, 60, 10, 20)),
            ],
        );
        let file = codex_path(&sessions, "a");

        let (since, until) = wide_window();
        let mut cache = ScanCache::default();
        let cold = scan_dirs_with(
            &mut cache,
            &[],
            std::slice::from_ref(&sessions),
            since,
            until,
        );
        assert_eq!(cold.buckets.len(), 1);
        assert_eq!(cold.buckets[0].model, "gpt-5");

        let added = append(
            &file,
            &format!(
                "{}\n{}\n{}\n",
                // Byte-identical repeat of the last cached usage: still a
                // duplicate, even though it lands in a different read.
                codex_token_count("2026-01-02T10:00:02Z", codex_usage(100, 60, 10, 20)),
                // A later turn switches model; the session id stays the one
                // from the `session_meta` line the cold scan consumed.
                codex_turn_context("2026-01-02T10:00:03Z", "gpt-5-codex"),
                codex_token_count("2026-01-02T10:00:04Z", codex_usage(200, 150, 0, 40)),
            ),
        );
        let warm = scan_dirs_with(
            &mut cache,
            &[],
            std::slice::from_ref(&sessions),
            since,
            until,
        );
        assert_eq!((warm.files_read, warm.bytes_read), (1, added));
        assert_eq!(
            warm.buckets
                .iter()
                .map(|b| (b.model.as_str(), b.requests))
                .collect::<Vec<_>>(),
            vec![("gpt-5", 1), ("gpt-5-codex", 1)],
            "the repeat is suppressed and the appended turn uses the new model"
        );
        assert_eq!(session_ids(&warm, "codex"), vec!["codex-1"]);
        assert_eq!(
            span(&warm, "codex", "codex-1"),
            (ms("2026-01-02T10:00:01Z"), ms("2026-01-02T10:00:04Z"))
        );
        // A resumed parse must agree with reading the finished file in one go.
        assert_eq!(
            answer(&warm),
            answer(&scan_dirs(&[], &[sessions], since, until))
        );
    }

    #[test]
    fn a_trailing_partial_line_is_left_for_the_next_scan() {
        // A transcript is appended to while we read it: the last line may be
        // half-written. Consuming it would lose the record for good.
        let td = tempfile::tempdir().unwrap();
        let projects = claude_file(
            td.path(),
            "slug",
            "s1",
            &[claude_line(
                "s1",
                "m1",
                "r1",
                "2026-01-02T10:00:00Z",
                "claude-opus-4",
                claude_usage(10, 5, 0, 0),
            )],
        );
        let file = claude_path(&projects, "slug", "s1");
        let pending = claude_line(
            "s1",
            "m2",
            "r2",
            "2026-01-02T10:10:00Z",
            "claude-opus-4",
            claude_usage(1, 2, 0, 0),
        );
        let (head, tail) = pending.split_at(20);
        append(&file, head);

        let (since, until) = wide_window();
        let mut cache = ScanCache::default();
        let cold = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        assert_eq!(cold.buckets[0].requests, 1, "the half line is not a record");

        append(&file, &format!("{tail}\n"));
        let warm = scan_dirs_with(&mut cache, &[projects], &[], since, until);
        assert_eq!(
            warm.buckets[0].requests, 2,
            "and is picked up once complete"
        );
        assert_eq!(
            warm.bytes_read,
            (pending.len() + 1) as u64,
            "the re-read starts at the unconsumed head, not before it"
        );
    }

    #[test]
    fn a_shrunken_file_is_re_read_from_the_start() {
        let td = tempfile::tempdir().unwrap();
        let projects = claude_file(
            td.path(),
            "slug",
            "s1",
            &[
                claude_line(
                    "s1",
                    "m1",
                    "r1",
                    "2026-01-02T10:00:00Z",
                    "claude-opus-4",
                    claude_usage(10, 5, 0, 0),
                ),
                claude_line(
                    "s1",
                    "m2",
                    "r2",
                    "2026-01-02T10:05:00Z",
                    "claude-opus-4",
                    claude_usage(10, 5, 0, 0),
                ),
            ],
        );
        let file = claude_path(&projects, "slug", "s1");

        let (since, until) = wide_window();
        let mut cache = ScanCache::default();
        let cold = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        assert_eq!(cold.buckets[0].requests, 2);

        // Rewritten shorter — the cached records describe bytes that are gone.
        let replaced = claude_line(
            "s1",
            "m3",
            "r3",
            "2026-01-02T10:00:00Z",
            "claude-opus-4",
            claude_usage(7, 0, 0, 0),
        );
        std::fs::write(&file, format!("{replaced}\n")).unwrap();

        let warm = scan_dirs_with(&mut cache, &[projects], &[], since, until);
        assert_eq!(warm.files_read, 1);
        assert_eq!(warm.bytes_read, (replaced.len() + 1) as u64);
        assert_eq!(warm.buckets.len(), 1);
        assert_eq!(warm.buckets[0].requests, 1);
        assert_eq!(warm.buckets[0].tokens.input, 7, "the old records are gone");
    }

    #[test]
    fn widening_the_window_uses_cached_out_of_window_records() {
        // Records are cached unfiltered, so a wider window is answered from
        // memory. This is what lets the frontend re-slice without re-reading.
        let td = tempfile::tempdir().unwrap();
        let projects = claude_file(
            td.path(),
            "slug",
            "s1",
            &[
                claude_line(
                    "s1",
                    "m1",
                    "r1",
                    "2026-01-02T10:00:00Z",
                    "claude-opus-4",
                    claude_usage(3, 0, 0, 0),
                ),
                claude_line(
                    "s1",
                    "m2",
                    "r2",
                    "2026-01-02T11:30:00Z",
                    "claude-opus-4",
                    claude_usage(5, 0, 0, 0),
                ),
            ],
        );

        let mut cache = ScanCache::default();
        let narrow = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            ms("2026-01-02T11:00:00Z"),
            ms("2026-01-02T12:00:00Z"),
        );
        assert_eq!(narrow.buckets.len(), 1);
        assert_eq!(narrow.buckets[0].tokens.input, 5);

        let (since, until) = wide_window();
        let wide = scan_dirs_with(&mut cache, &[projects], &[], since, until);
        assert_eq!((wide.files_read, wide.bytes_read), (0, 0));
        assert_eq!(
            wide.buckets.iter().map(|b| b.tokens.input).sum::<u64>(),
            8,
            "the record outside the first window was cached, not dropped"
        );
    }

    #[test]
    fn the_global_claude_dedupe_still_keeps_the_first_file_when_both_are_cached() {
        let td = tempfile::tempdir().unwrap();
        let line = claude_line(
            "s1",
            "msg_1",
            "req_1",
            "2026-01-02T10:00:00Z",
            "claude-opus-4",
            claude_usage(10, 5, 0, 0),
        );
        let projects = claude_file(td.path(), "slug-a", "s1", std::slice::from_ref(&line));
        let copied = claude_line(
            "s2",
            "msg_1",
            "req_1",
            "2026-01-02T10:00:00Z",
            "claude-opus-4",
            claude_usage(10, 5, 0, 0),
        );
        claude_file(td.path(), "slug-b", "s2", &[copied]);

        let (since, until) = wide_window();
        let mut cache = ScanCache::default();
        let cold = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        let warm = scan_dirs_with(&mut cache, &[projects], &[], since, until);
        assert_eq!((warm.files_read, warm.bytes_read), (0, 0));
        assert_eq!(warm.buckets.len(), 1);
        assert_eq!(warm.buckets[0].requests, 1);
        // Order over cached files is the sorted path order, so slug-a's copy
        // still wins and slug-b's session contributes nothing.
        assert_eq!(session_ids(&warm, "claude"), vec!["s1"]);
        assert_eq!(answer(&warm), answer(&cold));
    }

    // ── recovery from a failed read ─────────────────────────────────────────
    //
    // A transcript that couldn't be read now is not a transcript with no usage.
    // The fingerprint that says "this file is done" is therefore only written
    // after a read that reached the end of the file — otherwise the next scan
    // would see an unchanged `(len, mtime)`, skip the file, and lose the
    // missing spend until the transcript happened to change again (and, since
    // the entry is persisted, across restarts too).

    /// Three lines, so an interrupted read can land in the middle, plus the
    /// clean cold answer every recovery has to converge on.
    fn three_record_projects(td: &tempfile::TempDir) -> (PathBuf, Vec<String>) {
        let lines: Vec<String> = [
            ("m1", "2026-01-02T10:00:00Z", 10u64),
            ("m2", "2026-01-02T10:05:00Z", 20),
            ("m3", "2026-01-02T10:10:00Z", 30),
        ]
        .iter()
        .map(|(id, ts, input)| {
            claude_line(
                "s1",
                id,
                id,
                ts,
                "claude-opus-4",
                claude_usage(*input, 1, 0, 0),
            )
        })
        .collect();
        (claude_file(td.path(), "slug", "s1", &lines), lines)
    }

    /// The scan a healthy machine would produce for [`three_record_projects`].
    fn clean_scan(projects: &PathBuf) -> UsageScan {
        let (since, until) = wide_window();
        scan_dirs(std::slice::from_ref(projects), &[], since, until)
    }

    #[test]
    fn a_file_that_cannot_be_opened_is_read_in_full_by_the_next_scan() {
        let td = tempfile::tempdir().unwrap();
        let (projects, _) = three_record_projects(&td);
        let (since, until) = wide_window();

        let mut cache = cache_failing_once(Fail::Open);
        let failed = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        assert_eq!(
            (failed.files_read, failed.bytes_read),
            (0, 0),
            "nothing was read, so nothing may be counted as read"
        );
        assert!(failed.buckets.is_empty());
        assert_eq!(
            failed.scanned_files, 0,
            "a file we learned nothing about did not contribute"
        );
        assert!(
            cache.files.is_empty(),
            "no entry may be invented for a file that never opened"
        );

        let retry = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        assert_eq!(retry.files_read, 1);
        assert_eq!(answer(&retry), answer(&clean_scan(&projects)));
    }

    #[test]
    fn a_failed_seek_leaves_the_entry_alone_and_the_next_scan_resumes_it() {
        // Seeking only happens on a resumed read, so this is the append path:
        // the cached records are good, the appended ones were never reached.
        let td = tempfile::tempdir().unwrap();
        let (projects, _) = three_record_projects(&td);
        let file = claude_path(&projects, "slug", "s1");
        let (since, until) = wide_window();

        let mut cache = ScanCache::default();
        let cold = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        assert_eq!(cold.buckets[0].requests, 3);
        let before = cache
            .files
            .get(&file)
            .map(|e| (e.len, e.mtime_ms, e.offset));

        let added = append(
            &file,
            &format!(
                "{}\n",
                claude_line(
                    "s1",
                    "m4",
                    "m4",
                    "2026-01-02T10:15:00Z",
                    "claude-opus-4",
                    claude_usage(40, 1, 0, 0),
                )
            ),
        );

        fail_once(&mut cache, Fail::Seek);
        let failed = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        assert_eq!(
            (failed.files_read, failed.bytes_read),
            (0, 0),
            "a read that never started is not a read"
        );
        assert_eq!(failed.buckets[0].requests, 3, "the cached records survive");
        assert_eq!(
            cache
                .files
                .get(&file)
                .map(|e| (e.len, e.mtime_ms, e.offset)),
            before,
            "and the entry is untouched, so the grown file is still pending"
        );

        let retry = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        assert_eq!(
            (retry.files_read, retry.bytes_read),
            (1, added),
            "the retry reads exactly the bytes the failed scan missed"
        );
        assert_eq!(retry.buckets[0].requests, 4);
        assert_eq!(answer(&retry), answer(&clean_scan(&projects)));
    }

    #[test]
    fn a_read_that_dies_mid_file_keeps_its_progress_and_the_next_scan_finishes_it() {
        let td = tempfile::tempdir().unwrap();
        let (projects, lines) = three_record_projects(&td);
        let file = claude_path(&projects, "slug", "s1");
        let total = std::fs::metadata(&file).unwrap().len();
        let first = (lines[0].len() + 1) as u64;
        let (since, until) = wide_window();

        // The stream dies just after the first line's newline.
        let mut cache = cache_failing_once(Fail::AfterBytes(first));
        let partial = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        assert_eq!(
            (partial.files_read, partial.bytes_read),
            (1, first),
            "the bytes that did arrive were read"
        );
        assert_eq!(
            partial.buckets[0].requests, 1,
            "and the line they held is kept — progress is not thrown away"
        );
        let entry = cache.files.get(&file).expect("an entry");
        assert_eq!(entry.offset, first);
        assert_eq!(
            (entry.len, entry.mtime_ms),
            (UNREAD_LEN, UNREAD_MTIME_MS),
            "a part-read file must not look processed"
        );

        let retry = scan_dirs_with(
            &mut cache,
            std::slice::from_ref(&projects),
            &[],
            since,
            until,
        );
        assert_eq!(
            (retry.files_read, retry.bytes_read),
            (1, total - first),
            "only the bytes the failed read never got to"
        );
        assert_eq!(retry.buckets[0].requests, 3);
        assert_eq!(answer(&retry), answer(&clean_scan(&projects)));

        // And the finished entry is now fingerprinted, so a third scan is free.
        let warm = scan_dirs_with(&mut cache, &[projects], &[], since, until);
        assert_eq!((warm.files_read, warm.bytes_read), (0, 0));
    }
}

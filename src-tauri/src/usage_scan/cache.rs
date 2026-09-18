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

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::parse::{parse_claude_line, parse_codex_line, CodexState, UsageRecord};
use super::Provider;

/// What one file contributed, plus enough metadata to resume it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FileEntry {
    /// File length when `records`/`offset` were last brought up to date.
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

impl FileEntry {
    fn new(provider: Provider) -> Self {
        Self {
            len: 0,
            mtime_ms: i64::MIN,
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

/// Per-path parse cache. Lives for the process in
/// [`scan_all`](super::scan_all); tests own theirs. Change detection is
/// `(len, mtime-in-ms)`, which cannot see an in-place rewrite that preserves
/// the exact length within the same millisecond — transcripts are append-only,
/// so that doesn't happen.
#[derive(Default)]
pub struct ScanCache {
    pub(super) files: HashMap<PathBuf, FileEntry>,
}

#[derive(Default)]
pub(super) struct IoStats {
    pub(super) files_read: u32,
    pub(super) bytes_read: u64,
}

impl ScanCache {
    /// Bring `path`'s entry up to date, reading as little as possible. Returns
    /// whether the file has an entry to aggregate (`false` = prefiltered away).
    pub(super) fn refresh(
        &mut self,
        path: &Path,
        provider: Provider,
        since_ms: i64,
        io: &mut IoStats,
    ) -> bool {
        let Some((len, mtime_ms)) = file_stat(path) else {
            // Size/mtime unknown (racing deletion, odd permissions): read the
            // whole thing, as the pre-cache scanner did. The sentinel len/mtime
            // make the next scan resume from the offset or start over.
            self.read_fully(path, provider, 0, i64::MIN, io);
            return true;
        };

        match self.files.get(path) {
            // Same bytes as last time: reuse the records, touch no I/O.
            Some(e) if e.len == len && e.mtime_ms == mtime_ms => {}
            // Appended to: read from where the last scan stopped.
            Some(e) if len > e.len => {
                let entry = self.files.get_mut(path).expect("just matched");
                let from = entry.offset;
                read_into(entry, path, from, io);
                entry.len = len;
                entry.mtime_ms = mtime_ms;
            }
            // Truncated, or rewritten in place: nothing cached can be trusted.
            Some(_) => self.read_fully(path, provider, len, mtime_ms, io),
            // The mtime prefilter only applies to files never read: one last
            // written before the window opened cannot hold an in-window record.
            // A file with an entry is always cheap enough to stat and reuse.
            None if mtime_ms < since_ms => return false,
            None => self.read_fully(path, provider, len, mtime_ms, io),
        }
        true
    }

    /// Replace `path`'s entry with one parsed from the first byte.
    fn read_fully(
        &mut self,
        path: &Path,
        provider: Provider,
        len: u64,
        mtime_ms: i64,
        io: &mut IoStats,
    ) {
        let mut entry = FileEntry::new(provider);
        read_into(&mut entry, path, 0, io);
        entry.len = len;
        entry.mtime_ms = mtime_ms;
        self.files.insert(path.to_path_buf(), entry);
    }
}

/// Parse `path` from `start` into `entry`, advancing its consumed offset.
fn read_into(entry: &mut FileEntry, path: &Path, start: u64, io: &mut IoStats) {
    let FileEntry { records, codex, .. } = entry;
    let consumed = match codex {
        Some(state) => read_lines_from(path, start, io, |line| {
            parse_codex_line(line, state, records)
        }),
        None => read_lines_from(path, start, io, |line| {
            if let Some(record) = parse_claude_line(line) {
                records.push(record);
            }
        }),
    };
    entry.offset = consumed;
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

/// Stream a file line by line from `start`, reusing one buffer, and return the
/// offset consumed — always just past a `\n`, so a half-written trailing line
/// is left for the next scan to read whole. Never loads the whole file: real
/// transcripts reach hundreds of megabytes. Unreadable files are skipped
/// silently — a scan is best-effort reporting, not a correctness boundary.
fn read_lines_from(path: &Path, start: u64, io: &mut IoStats, mut f: impl FnMut(&str)) -> u64 {
    let Ok(mut file) = std::fs::File::open(path) else {
        return start;
    };
    if start > 0 && file.seek(SeekFrom::Start(start)).is_err() {
        return start;
    }
    io.files_read += 1;

    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut consumed = start;
    loop {
        buf.clear();
        let read = match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
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
    consumed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_scan::test_support::*;
    use crate::usage_scan::{scan_dirs, scan_dirs_with};

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
}

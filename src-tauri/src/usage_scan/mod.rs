//! Whole-disk token-usage scan over Claude Code and Codex CLI transcripts.
//!
//! Unlike `transcripts.rs` (which locates the transcript for one known
//! session), this walks *every* transcript on disk — including sessions this
//! app never spawned — and buckets token counts by local hour, provider and
//! model. Pricing is deliberately not done here: the frontend prices the
//! buckets against models.dev, so this stays a pure counting pass.
//!
//! Buckets are hourly (not daily) and sessions carry their first/last
//! timestamp so the frontend can scan once for the widest window it offers and
//! slice every shorter range client-side, instead of re-walking the disk on
//! every range switch.
//!
//! Files are streamed line by line (some transcripts are 100+ MB) behind two
//! cheap gates — an mtime prefilter that drops whole files that cannot contain
//! in-window records, and a substring test that skips JSON parsing for lines
//! that obviously carry no usage.
//!
//! The work is split across three submodules: [`parse`] turns lines into
//! window-independent records, [`cache`] decides how little of each file has to
//! be read to have those records, and [`persist`] keeps that cache across
//! restarts. What's left here is the walk, the aggregation and the wire types.

mod cache;
mod parse;
mod persist;
#[cfg(test)]
mod test_support;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use cache::IoStats;
pub use cache::ScanCache;
use parse::local_hour_start_ms;
use persist::cache_path;

/// Which CLI a record came from. The lowercase serialisation is the wire shape
/// the frontend and the on-disk cache both speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }
}

/// Token counts for one (hour, provider, model) bucket. `input` is *fresh*
/// (uncached) input only — cache hits/writes are reported separately because
/// they are priced differently.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenCounts {
    fn add(&mut self, other: &TokenCounts) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// One hour/provider/model cell of the usage table.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    /// Epoch ms of the *local* hour containing the record timestamp. Hourly
    /// rather than daily so the frontend can re-slice one scan into any range
    /// (and any day boundary) without touching disk again.
    pub hour_start_ms: i64,
    /// `"claude"` or `"codex"`.
    pub provider: String,
    pub model: String,
    pub tokens: TokenCounts,
    /// Number of usage records folded into this bucket (post-dedupe).
    pub requests: u32,
}

/// One distinct session that contributed at least one in-window record, with
/// the span of those records. The frontend counts sessions for a sub-range by
/// keeping the spans that overlap it, so the span must cover the in-window
/// records only — never the session's full lifetime on disk.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSessionSpan {
    /// `"claude"` or `"codex"`.
    pub provider: String,
    pub id: String,
    pub first_ms: i64,
    pub last_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageScan {
    /// Sorted by (`hour_start_ms`, provider, model).
    pub buckets: Vec<UsageBucket>,
    /// Sorted by (provider, `first_ms`, id).
    pub sessions: Vec<UsageSessionSpan>,
    /// Transcript files whose records went into this answer — read now or
    /// reused from the cache. Files dropped by the mtime prefilter don't count.
    pub scanned_files: u32,
    /// Of those, the files this scan actually opened (fully or from the cached
    /// offset). A warm repeat scan of an idle machine reports 0.
    pub files_read: u32,
    /// Bytes pulled off disk by this scan. Observability only — the frontend
    /// doesn't use it yet.
    pub bytes_read: u64,
    /// Echo of the requested window, so a cached scan can be checked against
    /// the range the caller wants to slice out of it.
    pub since_ms: i64,
    pub until_ms: i64,
}

/// Scan the real on-disk transcript roots for `[since_ms, until_ms)`, reusing
/// the process-wide cache — restored from disk on first use, so even the first
/// scan of a fresh process only reads what was appended since the last run.
/// Blocking; call from `spawn_blocking`.
pub fn scan_all(since_ms: i64, until_ms: i64) -> UsageScan {
    let claude = crate::transcripts::claude_projects_dirs();
    let codex: Vec<PathBuf> = crate::transcripts::codex_sessions_dir()
        .into_iter()
        .collect();
    static CACHE: OnceLock<Mutex<ScanCache>> = OnceLock::new();
    let path = cache_path();
    // Reading the cache is the first scan's I/O budget well spent: it replaces
    // a full re-parse of every transcript. Held under the same mutex as the
    // scan, so two concurrent scans can't both load (or both write) it.
    let mut cache = CACHE
        .get_or_init(|| {
            let started = Instant::now();
            let cache = ScanCache::load(&path);
            tracing::debug!(
                entries = cache.files.len(),
                elapsed_ms = started.elapsed().as_millis(),
                "usage scan cache loaded"
            );
            Mutex::new(cache)
        })
        .lock();

    let before = cache.files.len();
    let out = scan_dirs_with(&mut cache, &claude, &codex, since_ms, until_ms);
    // Nothing read and nothing pruned means the file on disk still describes
    // this cache exactly — rewriting it would be pure I/O for no new state.
    if out.files_read > 0 || cache.files.len() != before {
        let started = Instant::now();
        match cache.save(&path) {
            Ok(bytes) => tracing::debug!(
                entries = cache.files.len(),
                bytes,
                elapsed_ms = started.elapsed().as_millis(),
                "usage scan cache saved"
            ),
            // A cache we couldn't write is only a slower next start.
            Err(e) => tracing::warn!("usage scan cache not saved ({}): {e}", path.display()),
        }
    }
    out
}

/// [`scan_dirs_with`] against a throwaway cache, i.e. a cold scan that reads
/// every eligible file.
#[cfg(test)]
pub fn scan_dirs(
    claude_projects_dirs: &[PathBuf],
    codex_sessions_dirs: &[PathBuf],
    since_ms: i64,
    until_ms: i64,
) -> UsageScan {
    let mut cache = ScanCache::default();
    scan_dirs_with(
        &mut cache,
        claude_projects_dirs,
        codex_sessions_dirs,
        since_ms,
        until_ms,
    )
}

/// Core scan against a caller-owned cache, with the roots injected so tests
/// never touch the real home dir: files unchanged since the last scan through
/// the same cache are not re-read, and grown files are read from where that
/// scan stopped. `claude_projects_dirs` are `.../projects` dirs (children are
/// per-project slugs holding `<session>.jsonl`); `codex_sessions_dirs` are
/// `.../sessions` dirs holding a `YYYY/MM/DD` tree of `rollout-*.jsonl`.
pub fn scan_dirs_with(
    cache: &mut ScanCache,
    claude_projects_dirs: &[PathBuf],
    codex_sessions_dirs: &[PathBuf],
    since_ms: i64,
    until_ms: i64,
) -> UsageScan {
    let mut io = IoStats::default();
    // Aggregation order decides which copy of a duplicated Claude record wins,
    // so the files are collected in one deterministic (sorted, per root) order.
    let mut files: Vec<PathBuf> = Vec::new();

    for dir in claude_projects_dirs {
        for file in collect_jsonl_recursive(dir) {
            if cache.refresh(&file, Provider::Claude, since_ms, &mut io) {
                files.push(file);
            }
        }
    }

    for dir in codex_sessions_dirs {
        for year in read_subdirs(dir) {
            for month in read_subdirs(&year) {
                for day in read_subdirs(&month) {
                    for file in read_jsonl_files(&day) {
                        if cache.refresh(&file, Provider::Codex, since_ms, &mut io) {
                            files.push(file);
                        }
                    }
                }
            }
        }
    }

    // Every walked file that has an entry is in `files` (only a file with no
    // entry at all can be dropped by the prefilter), so anything else the cache
    // holds describes a transcript that is gone. Forgetting it here keeps a
    // long-lived — and persisted — cache the size of the disk it mirrors.
    let walked: HashSet<&PathBuf> = files.iter().collect();
    if walked.len() != cache.files.len() {
        cache.files.retain(|path, _| walked.contains(path));
    }

    aggregate(cache, &files, &io, since_ms, until_ms)
}

// ── filesystem walk ─────────────────────────────────────────────────────────

fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

/// Every `*.jsonl` under `root`, at any depth. Claude's layout is not flat:
/// besides `projects/<slug>/<session>.jsonl` it nests a subagent's transcript
/// at `projects/<slug>/<session>/subagents/agent-<id>.jsonl`, and those carry
/// real spend (they outnumber the top-level files several to one). Scanning
/// only the first level silently halved the reported usage.
fn collect_jsonl_recursive(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            // `file_type` doesn't follow symlinks, so a link loop can't make
            // this walk diverge.
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t)
                    if t.is_file()
                        && path.extension().and_then(|e| e.to_str()) == Some("jsonl") =>
                {
                    out.push(path)
                }
                _ => {}
            }
        }
    }
    // Deterministic order so the global dedupe's "keep the first occurrence"
    // resolves the same way on every run.
    out.sort();
    out
}

fn read_jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    out.sort();
    out
}

// ── aggregation ─────────────────────────────────────────────────────────────

/// Fold every cached record of `files` (in file order) into the answer for
/// `[since_ms, until_ms)`. Windowing, the global Claude dedupe and the
/// zero-token drop are applied here, in that order, so the result is identical
/// to a single-pass scan whatever the cache did.
fn aggregate(
    cache: &ScanCache,
    files: &[PathBuf],
    io: &IoStats,
    since_ms: i64,
    until_ms: i64,
) -> UsageScan {
    let mut acc = Accumulator::default();
    // A record is copied forward into the transcript of a resumed or forked
    // session, so the seen-set spans every file, not just the current one.
    let mut seen_claude: HashSet<&str> = HashSet::new();

    for path in files {
        let Some(entry) = cache.files.get(path) else {
            continue;
        };
        // Codex records get their session id from the file, Claude records
        // carry their own (subagent transcripts report the parent's).
        let codex_session_id = entry.codex.as_ref().map(|_| entry.codex_session_id(path));
        for record in &entry.records {
            if record.ts_ms < since_ms || record.ts_ms >= until_ms {
                continue;
            }
            if let Some(key) = record.dedupe_key.as_deref() {
                if !seen_claude.insert(key) {
                    continue;
                }
            }
            if record.tokens.total() == 0 {
                continue;
            }
            acc.record(record.ts_ms, record.provider, &record.model, &record.tokens);
            let session_id = match record.provider {
                Provider::Codex => codex_session_id.as_deref(),
                Provider::Claude => record.session_id.as_deref(),
            };
            if let Some(id) = session_id {
                acc.touch_session(record.provider, id, record.ts_ms);
            }
        }
    }

    acc.finish(files.len() as u32, io, since_ms, until_ms)
}

#[derive(Default)]
struct Accumulator {
    /// Keyed by (local hour start, provider, model) — a BTreeMap so the output
    /// is sorted by exactly that tuple with no extra sort pass. `Provider`
    /// orders `Claude` before `Codex`, i.e. the same way their wire names do.
    buckets: BTreeMap<(i64, Provider, String), (TokenCounts, u32)>,
    /// (provider, session id) → (first ms, last ms) over in-window records.
    sessions: HashMap<(Provider, String), (i64, i64)>,
}

impl Accumulator {
    fn record(&mut self, ts_ms: i64, provider: Provider, model: &str, tokens: &TokenCounts) {
        let entry = self
            .buckets
            .entry((local_hour_start_ms(ts_ms), provider, model.to_string()))
            .or_default();
        entry.0.add(tokens);
        entry.1 += 1;
    }

    /// Widen a session's span to include `ts_ms`, creating it on first sight.
    fn touch_session(&mut self, provider: Provider, id: &str, ts_ms: i64) {
        let entry = self
            .sessions
            .entry((provider, id.to_string()))
            .or_insert((ts_ms, ts_ms));
        entry.0 = entry.0.min(ts_ms);
        entry.1 = entry.1.max(ts_ms);
    }

    fn finish(self, scanned_files: u32, io: &IoStats, since_ms: i64, until_ms: i64) -> UsageScan {
        let mut sessions: Vec<UsageSessionSpan> = self
            .sessions
            .into_iter()
            .map(|((provider, id), (first_ms, last_ms))| UsageSessionSpan {
                provider: provider.as_str().to_string(),
                id,
                first_ms,
                last_ms,
            })
            .collect();
        sessions.sort_by(|a, b| {
            (&a.provider, a.first_ms, &a.id).cmp(&(&b.provider, b.first_ms, &b.id))
        });

        UsageScan {
            buckets: self
                .buckets
                .into_iter()
                .map(
                    |((hour_start_ms, provider, model), (tokens, requests))| UsageBucket {
                        hour_start_ms,
                        provider: provider.as_str().to_string(),
                        model,
                        tokens,
                        requests,
                    },
                )
                .collect(),
            sessions,
            scanned_files,
            files_read: io.files_read,
            bytes_read: io.bytes_read,
            since_ms,
            until_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::persist::CACHE_FILE;
    use super::test_support::*;
    use super::*;
    use serde_json::Value;
    use std::time::UNIX_EPOCH;

    // ── claude ──────────────────────────────────────────────────────────────

    #[test]
    fn claude_dedupes_the_same_message_across_two_files() {
        // Resuming a session copies earlier assistant records verbatim into the
        // new transcript; counting both would double the spend. The dedupe set
        // is global, so the second file's copy is dropped.
        let td = tempfile::tempdir().unwrap();
        let line = claude_line(
            "s1",
            "msg_1",
            "req_1",
            "2026-01-02T10:00:00Z",
            "claude-opus-4",
            claude_usage(10, 5, 100, 20),
        );
        let projects = claude_file(td.path(), "slug-a", "s1", std::slice::from_ref(&line));
        // Same message, different session file (the resumed one).
        let copied = claude_line(
            "s2",
            "msg_1",
            "req_1",
            "2026-01-02T10:00:00Z",
            "claude-opus-4",
            claude_usage(10, 5, 100, 20),
        );
        claude_file(td.path(), "slug-b", "s2", &[copied]);

        let (since, until) = wide_window();
        let out = scan_dirs(&[projects], &[], since, until);
        assert_eq!(out.scanned_files, 2);
        assert_eq!(out.buckets.len(), 1);
        assert_eq!(out.buckets[0].requests, 1);
        assert_eq!(
            out.buckets[0].tokens,
            TokenCounts {
                input: 10,
                output: 5,
                cache_read: 100,
                cache_write: 20
            }
        );
        // Only the first occurrence's session contributed.
        assert_eq!(session_ids(&out, "claude"), vec!["s1"]);
    }

    #[test]
    fn claude_dedupes_repeated_content_blocks_within_one_file() {
        // Claude writes one record per content block, each carrying the same
        // usage object for the single underlying request.
        let td = tempfile::tempdir().unwrap();
        let mk = || {
            claude_line(
                "s1",
                "msg_1",
                "req_1",
                "2026-01-02T10:00:00Z",
                "claude-opus-4",
                claude_usage(10, 5, 0, 0),
            )
        };
        let projects = claude_file(td.path(), "slug", "s1", &[mk(), mk(), mk()]);

        let (since, until) = wide_window();
        let out = scan_dirs(&[projects], &[], since, until);
        assert_eq!(out.buckets.len(), 1);
        assert_eq!(out.buckets[0].requests, 1);
        assert_eq!(out.buckets[0].tokens.output, 5);
    }

    #[test]
    fn claude_scans_nested_subagent_transcripts_and_dedupes_them_against_the_parent() {
        // Claude writes a subagent's transcript to
        // `projects/<slug>/<session>/subagents/agent-<id>.jsonl` — a level
        // deeper than the main session file. A first-level-only walk missed
        // those, which on a real machine dropped ~half the tokens. The nested
        // records carry the parent's `sessionId`, so they must fold into the
        // same session, and a record present in both files is still one
        // request.
        let td = tempfile::tempdir().unwrap();
        let ts = "2026-01-02T10:00:00Z";
        let parent = claude_line(
            "s1",
            "msg_p",
            "req_p",
            ts,
            "claude-opus-4",
            claude_usage(10, 1, 0, 0),
        );
        let projects = claude_file(td.path(), "slug", "s1", std::slice::from_ref(&parent));

        let nested = projects.join("slug").join("s1").join("subagents");
        std::fs::create_dir_all(&nested).unwrap();
        let sub = claude_line(
            "s1",
            "msg_sub",
            "req_sub",
            ts,
            "claude-opus-4",
            claude_usage(20, 2, 0, 0),
        );
        std::fs::write(nested.join("agent-x.jsonl"), format!("{sub}\n{parent}\n")).unwrap();

        let (since, until) = wide_window();
        let out = scan_dirs(&[projects], &[], since, until);
        assert_eq!(out.scanned_files, 2, "the nested file must be read");
        assert_eq!(out.buckets.len(), 1);
        assert_eq!(out.buckets[0].requests, 2, "parent copy must be deduped");
        assert_eq!(out.buckets[0].tokens.input, 30);
        assert_eq!(out.buckets[0].tokens.output, 3);
        // Subagent records carry the parent sessionId — one session, not two.
        assert_eq!(session_ids(&out, "claude"), vec!["s1"]);
    }

    #[test]
    fn claude_skips_records_outside_the_window() {
        let td = tempfile::tempdir().unwrap();
        let before = claude_line(
            "s1",
            "m0",
            "r0",
            "2026-01-01T10:00:00Z",
            "claude-opus-4",
            claude_usage(1, 1, 0, 0),
        );
        let inside = claude_line(
            "s1",
            "m1",
            "r1",
            "2026-01-02T10:00:00Z",
            "claude-opus-4",
            claude_usage(7, 3, 0, 0),
        );
        // `until` is exclusive: a record exactly at the bound is out.
        let at_until = claude_line(
            "s1",
            "m2",
            "r2",
            "2026-01-03T00:00:00Z",
            "claude-opus-4",
            claude_usage(9, 9, 0, 0),
        );
        let projects = claude_file(td.path(), "slug", "s1", &[before, inside, at_until]);

        let out = scan_dirs(
            &[projects],
            &[],
            ms("2026-01-02T00:00:00Z"),
            ms("2026-01-03T00:00:00Z"),
        );
        assert_eq!(out.buckets.len(), 1);
        assert_eq!(out.buckets[0].requests, 1);
        assert_eq!(out.buckets[0].tokens.input, 7);
    }

    #[test]
    fn claude_skips_api_errors_synthetic_and_zero_token_records() {
        let td = tempfile::tempdir().unwrap();
        let mut err: Value = serde_json::from_str(&claude_line(
            "s1",
            "m0",
            "r0",
            "2026-01-02T10:00:00Z",
            "claude-opus-4",
            claude_usage(5, 5, 0, 0),
        ))
        .unwrap();
        err["isApiErrorMessage"] = Value::Bool(true);
        let zero = claude_line(
            "s1",
            "m1",
            "r1",
            "2026-01-02T10:00:00Z",
            "claude-opus-4",
            claude_usage(0, 0, 0, 0),
        );
        // The CLI talking to itself, not a billed call.
        let synthetic = claude_line(
            "s1",
            "m2",
            "r2",
            "2026-01-02T10:00:00Z",
            "<synthetic>",
            claude_usage(5, 5, 0, 0),
        );
        let projects = claude_file(td.path(), "slug", "s1", &[err.to_string(), zero, synthetic]);

        let (since, until) = wide_window();
        let out = scan_dirs(&[projects], &[], since, until);
        assert!(out.buckets.is_empty());
        assert!(out.sessions.is_empty());
    }

    #[test]
    fn claude_keeps_a_record_whose_transcript_never_named_a_model() {
        // The TS adapter keeps the call with `model` undefined and the fold
        // keys it as ""; dropping it here would lose spend that happened.
        let td = tempfile::tempdir().unwrap();
        let no_model = claude_line(
            "s1",
            "m1",
            "r1",
            "2026-01-02T10:00:00Z",
            "",
            claude_usage(5, 5, 0, 0),
        );
        let projects = claude_file(td.path(), "slug", "s1", &[no_model]);

        let (since, until) = wide_window();
        let out = scan_dirs(&[projects], &[], since, until);
        assert_eq!(out.buckets.len(), 1);
        assert_eq!(out.buckets[0].model, "");
        assert_eq!(out.buckets[0].tokens.input, 5);
        assert_eq!(session_ids(&out, "claude"), vec!["s1"]);
    }

    // ── codex ───────────────────────────────────────────────────────────────

    #[test]
    fn codex_takes_model_from_turn_context_and_drops_repeated_usage() {
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
                // No model yet → this one can't be attributed and is dropped.
                codex_token_count("2026-01-02T09:00:01Z", codex_usage(50, 0, 0, 1)),
                codex_turn_context("2026-01-02T10:00:00Z", "gpt-5"),
                codex_token_count("2026-01-02T10:00:01Z", codex_usage(100, 60, 10, 20)),
                // Byte-identical repeat of the previous usage → same request.
                codex_token_count("2026-01-02T10:00:02Z", codex_usage(100, 60, 10, 20)),
                codex_token_count("2026-01-02T10:01:00Z", codex_usage(200, 150, 0, 40)),
            ],
        );

        let (since, until) = wide_window();
        let out = scan_dirs(&[], &[sessions], since, until);
        assert_eq!(out.buckets.len(), 1);
        let b = &out.buckets[0];
        assert_eq!(b.provider, "codex");
        assert_eq!(b.model, "gpt-5");
        assert_eq!(b.requests, 2);
        assert_eq!(
            b.tokens,
            TokenCounts {
                // Fresh input is the total minus the cached prefix only —
                // cache writes are reported beside it, not carved out of it.
                // (100-60) + (200-150)
                input: 40 + 50,
                output: 60,
                cache_read: 210,
                cache_write: 10,
            }
        );
        assert_eq!(session_ids(&out, "codex"), vec!["codex-1"]);
        // Span covers only the two counted records, not the session_meta line.
        assert_eq!(
            span(&out, "codex", "codex-1"),
            (ms("2026-01-02T10:00:01Z"), ms("2026-01-02T10:01:00Z"))
        );
    }

    #[test]
    fn codex_forked_session_drops_the_copied_burst_but_keeps_later_spend() {
        let td = tempfile::tempdir().unwrap();
        let sessions = codex_file(
            td.path(),
            "fork",
            &[
                serde_json::json!({
                    "timestamp": "2026-01-02T10:00:00Z",
                    "type": "session_meta",
                    "payload": { "id": "codex-fork", "forked_from_id": "codex-parent" },
                })
                .to_string(),
                codex_turn_context("2026-01-02T10:00:00.100Z", "gpt-5"),
                // Burst copied from the parent: each within 1s of the last.
                codex_token_count("2026-01-02T10:00:00.200Z", codex_usage(100, 0, 0, 10)),
                codex_token_count("2026-01-02T10:00:00.700Z", codex_usage(200, 0, 0, 20)),
                codex_token_count("2026-01-02T10:00:01.100Z", codex_usage(300, 0, 0, 30)),
                // Genuine turn, well after the burst.
                codex_token_count("2026-01-02T10:05:00Z", codex_usage(400, 100, 0, 40)),
            ],
        );

        let (since, until) = wide_window();
        let out = scan_dirs(&[], &[sessions], since, until);
        assert_eq!(out.buckets.len(), 1);
        assert_eq!(out.buckets[0].requests, 1);
        assert_eq!(
            out.buckets[0].tokens,
            TokenCounts {
                input: 300,
                output: 40,
                cache_read: 100,
                cache_write: 0
            }
        );
        // The dropped burst must not stretch the span back to fork time.
        assert_eq!(
            span(&out, "codex", "codex-fork"),
            (ms("2026-01-02T10:05:00Z"), ms("2026-01-02T10:05:00Z"))
        );
    }

    #[test]
    fn codex_subagent_thread_spawn_is_treated_as_a_fork() {
        let td = tempfile::tempdir().unwrap();
        let sessions = codex_file(
            td.path(),
            "sub",
            &[
                serde_json::json!({
                    "timestamp": "2026-01-02T10:00:00Z",
                    "type": "session_meta",
                    "payload": {
                        "id": "codex-sub",
                        "source": { "subagent": { "thread_spawn": { "parent_thread_id": "p" } } },
                    },
                })
                .to_string(),
                codex_turn_context("2026-01-02T10:00:00.100Z", "gpt-5"),
                codex_token_count("2026-01-02T10:00:00.200Z", codex_usage(100, 0, 0, 10)),
            ],
        );

        let (since, until) = wide_window();
        let out = scan_dirs(&[], &[sessions], since, until);
        assert!(out.buckets.is_empty());
        assert!(out.sessions.is_empty());
    }

    // ── cross-cutting ───────────────────────────────────────────────────────

    #[test]
    fn mtime_prefilter_skips_files_last_written_before_the_window() {
        // The file's *records* are in the window, but its mtime says it was
        // last touched long before it opened — an impossible combination in
        // practice, used here to prove the file is never opened.
        let td = tempfile::tempdir().unwrap();
        let line = claude_line(
            "s1",
            "m1",
            "r1",
            "2026-01-02T10:00:00Z",
            "claude-opus-4",
            claude_usage(10, 10, 0, 0),
        );
        let projects = claude_file(td.path(), "slug", "s1", &[line]);
        set_mtime_ms(
            &projects.join("slug").join("s1.jsonl"),
            ms("2020-01-01T00:00:00Z"),
        );

        let out = scan_dirs(
            std::slice::from_ref(&projects),
            &[],
            ms("2026-01-01T00:00:00Z"),
            ms("2026-02-01T00:00:00Z"),
        );
        assert_eq!(out.scanned_files, 0);
        assert!(out.buckets.is_empty());

        // Same file, a window that starts before its mtime → it is read.
        let out = scan_dirs(
            &[projects],
            &[],
            ms("2019-01-01T00:00:00Z"),
            ms("2026-02-01T00:00:00Z"),
        );
        assert_eq!(out.scanned_files, 1);
        assert_eq!(out.buckets.len(), 1);
    }

    #[test]
    fn records_in_the_same_local_hour_merge_and_adjacent_hours_split() {
        let td = tempfile::tempdir().unwrap();
        // Every real UTC offset is a whole number of quarter-hours, so a pair
        // of instants at :05 and :10 past share a local hour wherever the test
        // runs, and one exactly an hour later is always the next local hour.
        let projects = claude_file(
            td.path(),
            "slug",
            "s1",
            &[
                claude_line(
                    "s1",
                    "m1",
                    "r1",
                    "2026-01-02T10:05:00Z",
                    "claude-opus-4",
                    claude_usage(1, 1, 0, 0),
                ),
                claude_line(
                    "s1",
                    "m2",
                    "r2",
                    "2026-01-02T10:10:00Z",
                    "claude-opus-4",
                    claude_usage(2, 2, 0, 0),
                ),
                claude_line(
                    "s1",
                    "m3",
                    "r3",
                    "2026-01-02T11:05:00Z",
                    "claude-opus-4",
                    claude_usage(4, 4, 0, 0),
                ),
            ],
        );

        let (since, until) = wide_window();
        let out = scan_dirs(&[projects], &[], since, until);
        assert_eq!(out.buckets.len(), 2, "two local hours");
        assert_eq!(out.buckets[0].requests, 2);
        assert_eq!(out.buckets[0].tokens.input, 3);
        assert_eq!(out.buckets[1].requests, 1);
        assert_eq!(out.buckets[1].tokens.input, 4);
        assert_eq!(
            out.buckets[0].hour_start_ms,
            local_hour_start_ms(ms("2026-01-02T10:05:00Z"))
        );
        assert_eq!(
            out.buckets[1].hour_start_ms - out.buckets[0].hour_start_ms,
            3_600_000
        );
    }

    #[test]
    fn buckets_are_sorted_by_hour_provider_model() {
        let td = tempfile::tempdir().unwrap();
        // Two instants 48h apart are two distinct local hours in every
        // timezone, so the assertion holds wherever the test runs.
        let early = "2026-01-02T12:00:00Z";
        let late = "2026-01-04T12:00:00Z";
        let projects = claude_file(
            td.path(),
            "slug",
            "s1",
            &[
                claude_line(
                    "s1",
                    "m2",
                    "r2",
                    late,
                    "claude-sonnet-4",
                    claude_usage(1, 1, 0, 0),
                ),
                claude_line(
                    "s1",
                    "m1",
                    "r1",
                    early,
                    "claude-opus-4",
                    claude_usage(2, 2, 0, 0),
                ),
                claude_line(
                    "s1",
                    "m3",
                    "r3",
                    early,
                    "claude-sonnet-4",
                    claude_usage(3, 3, 0, 0),
                ),
            ],
        );
        let sessions = codex_file(
            td.path(),
            "a",
            &[
                codex_turn_context(early, "gpt-5"),
                codex_token_count(early, codex_usage(4, 0, 0, 4)),
            ],
        );

        let (since, until) = wide_window();
        let out = scan_dirs(&[projects], &[sessions], since, until);
        let keys: Vec<(i64, String, String)> = out
            .buckets
            .iter()
            .map(|b| (b.hour_start_ms, b.provider.clone(), b.model.clone()))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "buckets must come out sorted");
        let (h0, h1) = (
            local_hour_start_ms(ms(early)),
            local_hour_start_ms(ms(late)),
        );
        assert_eq!(
            keys,
            vec![
                (h0, "claude".into(), "claude-opus-4".into()),
                (h0, "claude".into(), "claude-sonnet-4".into()),
                (h0, "codex".into(), "gpt-5".into()),
                (h1, "claude".into(), "claude-sonnet-4".into()),
            ]
        );
        assert_ne!(h0, h1);
    }

    #[test]
    fn sessions_are_reported_as_distinct_spans_per_provider() {
        let td = tempfile::tempdir().unwrap();
        // sess-a spans two records; sess-b is a single point an hour later.
        let projects = claude_file(
            td.path(),
            "slug",
            "s1",
            &[
                claude_line(
                    "sess-a",
                    "m1",
                    "r1",
                    "2026-01-02T10:00:00Z",
                    "claude-opus-4",
                    claude_usage(1, 1, 0, 0),
                ),
                claude_line(
                    "sess-a",
                    "m2",
                    "r2",
                    "2026-01-02T12:00:00Z",
                    "claude-opus-4",
                    claude_usage(1, 1, 0, 0),
                ),
                claude_line(
                    "sess-b",
                    "m3",
                    "r3",
                    "2026-01-02T11:00:00Z",
                    "claude-opus-4",
                    claude_usage(1, 1, 0, 0),
                ),
            ],
        );
        // cx-2 starts *before* cx-1, so file order and span order disagree.
        codex_file(
            td.path(),
            "a",
            &[
                serde_json::json!({ "timestamp": "2026-01-02T10:00:00Z", "type": "session_meta", "payload": { "session_id": "cx-1" } })
                    .to_string(),
                codex_turn_context("2026-01-02T10:00:00Z", "gpt-5"),
                codex_token_count("2026-01-02T10:00:00Z", codex_usage(5, 0, 0, 5)),
            ],
        );
        let sessions = codex_file(
            td.path(),
            "b",
            &[
                serde_json::json!({ "timestamp": "2026-01-02T09:00:00Z", "type": "session_meta", "payload": { "session_id": "cx-2" } })
                    .to_string(),
                codex_turn_context("2026-01-02T09:00:00Z", "gpt-5"),
                codex_token_count("2026-01-02T09:00:00Z", codex_usage(5, 0, 0, 5)),
            ],
        );

        let (since, until) = wide_window();
        let out = scan_dirs(&[projects], &[sessions], since, until);
        // Sorted by (provider, first_ms, id) — claude before codex, and within
        // codex the later-started cx-1 comes second.
        assert_eq!(
            out.sessions
                .iter()
                .map(|s| (s.provider.as_str(), s.id.as_str(), s.first_ms, s.last_ms))
                .collect::<Vec<_>>(),
            vec![
                (
                    "claude",
                    "sess-a",
                    ms("2026-01-02T10:00:00Z"),
                    ms("2026-01-02T12:00:00Z")
                ),
                (
                    "claude",
                    "sess-b",
                    ms("2026-01-02T11:00:00Z"),
                    ms("2026-01-02T11:00:00Z")
                ),
                (
                    "codex",
                    "cx-2",
                    ms("2026-01-02T09:00:00Z"),
                    ms("2026-01-02T09:00:00Z")
                ),
                (
                    "codex",
                    "cx-1",
                    ms("2026-01-02T10:00:00Z"),
                    ms("2026-01-02T10:00:00Z")
                ),
            ]
        );
    }

    #[test]
    fn a_session_span_covers_only_its_in_window_records() {
        // The same session has records on both sides of the window. Its span
        // must clamp to what was counted, or the frontend's "sessions
        // overlapping this range" slice would over-report.
        let td = tempfile::tempdir().unwrap();
        let projects = claude_file(
            td.path(),
            "slug",
            "s1",
            &[
                claude_line(
                    "sess-a",
                    "m0",
                    "r0",
                    "2026-01-01T10:00:00Z",
                    "claude-opus-4",
                    claude_usage(1, 1, 0, 0),
                ),
                claude_line(
                    "sess-a",
                    "m1",
                    "r1",
                    "2026-01-02T08:00:00Z",
                    "claude-opus-4",
                    claude_usage(1, 1, 0, 0),
                ),
                claude_line(
                    "sess-a",
                    "m2",
                    "r2",
                    "2026-01-02T20:00:00Z",
                    "claude-opus-4",
                    claude_usage(1, 1, 0, 0),
                ),
                claude_line(
                    "sess-a",
                    "m3",
                    "r3",
                    "2026-01-04T10:00:00Z",
                    "claude-opus-4",
                    claude_usage(1, 1, 0, 0),
                ),
            ],
        );

        let out = scan_dirs(
            &[projects],
            &[],
            ms("2026-01-02T00:00:00Z"),
            ms("2026-01-03T00:00:00Z"),
        );
        assert_eq!(out.sessions.len(), 1);
        assert_eq!(
            span(&out, "claude", "sess-a"),
            (ms("2026-01-02T08:00:00Z"), ms("2026-01-02T20:00:00Z"))
        );
    }

    #[test]
    fn a_codex_rollout_without_session_meta_falls_back_to_its_path() {
        let td = tempfile::tempdir().unwrap();
        let ts = "2026-01-02T10:00:00Z";
        let sessions = codex_file(
            td.path(),
            "orphan",
            &[
                codex_turn_context(ts, "gpt-5"),
                codex_token_count(ts, codex_usage(5, 0, 0, 5)),
            ],
        );

        let (since, until) = wide_window();
        let out = scan_dirs(&[], &[sessions], since, until);
        assert_eq!(out.sessions.len(), 1);
        assert!(
            out.sessions[0].id.ends_with("rollout-orphan.jsonl"),
            "got {}",
            out.sessions[0].id
        );
    }

    #[test]
    fn missing_roots_scan_to_an_empty_result() {
        let out = scan_dirs(
            &[PathBuf::from("/nope/projects")],
            &[PathBuf::from("/nope/sessions")],
            0,
            i64::MAX,
        );
        assert!(out.buckets.is_empty());
        assert_eq!(out.scanned_files, 0);
        assert!(out.sessions.is_empty());
        // The window is echoed back even when nothing matched.
        assert_eq!((out.since_ms, out.until_ms), (0, i64::MAX));
    }

    // ── the shared corpus ───────────────────────────────────────────────────

    /// The corpus is read by two parsers — this one and the TS adapters under
    /// `src/adapters/{claude,codex}/usage.ts`, whose test is
    /// `tests/adapters/usageCorpus.test.ts` — so `expected.json` is the one
    /// place their semantics are pinned to each other. Change a rule in either
    /// parser and one of the two tests fails until all three agree.
    #[test]
    fn the_shared_corpus_folds_to_the_expected_totals() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Expected {
            input: u64,
            output: u64,
            cache_read: u64,
            cache_write: u64,
            calls: u32,
        }

        let expected: HashMap<String, Expected> =
            serde_json::from_str(include_str!("../../../tests/fixtures/usage/expected.json"))
                .expect("expected.json");

        let td = tempfile::tempdir().unwrap();
        let projects = td.path().join("projects").join("corpus");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(
            projects.join("7f3c9d21-0b64-4f0a-9c2e-1d5b7a4e8c30.jsonl"),
            include_str!("../../../tests/fixtures/usage/claude.jsonl"),
        )
        .unwrap();
        let day = td
            .path()
            .join("sessions")
            .join("2026")
            .join("03")
            .join("04");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-2026-03-04T09-00-00-019e4448-78a8-7203-a078-981c1d34c547.jsonl"),
            include_str!("../../../tests/fixtures/usage/codex.jsonl"),
        )
        .unwrap();

        let (since, until) = wide_window();
        let out = scan_dirs(
            &[td.path().join("projects")],
            &[td.path().join("sessions")],
            since,
            until,
        );

        for (provider, want) in &expected {
            let mut tokens = TokenCounts::default();
            let mut calls = 0;
            for b in out.buckets.iter().filter(|b| &b.provider == provider) {
                tokens.add(&b.tokens);
                calls += b.requests;
            }
            assert_eq!(
                (
                    tokens.input,
                    tokens.output,
                    tokens.cache_read,
                    tokens.cache_write,
                    calls
                ),
                (
                    want.input,
                    want.output,
                    want.cache_read,
                    want.cache_write,
                    want.calls
                ),
                "{provider} totals"
            );
        }
    }

    /// Sanity check against this machine's real transcripts — not part of the
    /// normal suite (slow, and depends on local data). Run with:
    /// `cargo test -- --ignored usage_scan::tests::real_scan --nocapture`
    /// Asserts nothing about volume so it passes on a machine with no
    /// transcripts at all.
    #[test]
    #[ignore]
    fn real_scan() {
        let now = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        // 90 days: the widest window the usage page offers, and the one scan
        // the frontend slices every shorter range out of.
        let since = now - 90 * 24 * 60 * 60 * 1000;
        // The real roots, but never the real cache file: this must measure a
        // genuinely cold scan, and must not overwrite the installed app's
        // cache from a test run.
        let claude = crate::transcripts::claude_projects_dirs();
        let codex: Vec<PathBuf> = crate::transcripts::codex_sessions_dir()
            .into_iter()
            .collect();
        let td = tempfile::tempdir().unwrap();
        let cache_file = td.path().join(CACHE_FILE);

        let mut cache = ScanCache::default();
        let started = std::time::Instant::now();
        let out = scan_dirs_with(&mut cache, &claude, &codex, since, now);
        let elapsed = started.elapsed();
        println!(
            "cold: elapsed={:?} files_read={} bytes_read={}",
            elapsed, out.files_read, out.bytes_read
        );

        let started_save = Instant::now();
        let bytes = cache.save(&cache_file).unwrap();
        println!(
            "save: elapsed={:?} cache_bytes={bytes} ({:.1} MiB)",
            started_save.elapsed(),
            bytes as f64 / (1024.0 * 1024.0)
        );

        // What the first scan after an app restart does: load the file, then
        // scan — only what was appended since the save may be read.
        let started_load = Instant::now();
        let mut restored = ScanCache::load(&cache_file);
        println!(
            "load: elapsed={:?} entries={}",
            started_load.elapsed(),
            restored.files.len()
        );
        let started_warm = std::time::Instant::now();
        let warm = scan_dirs_with(&mut restored, &claude, &codex, since, now);
        println!(
            "warm: elapsed={:?} files_read={} bytes_read={}",
            started_warm.elapsed(),
            warm.files_read,
            warm.bytes_read
        );
        // Not an equality check on the answer: a live CLI may well append to a
        // transcript between the two scans, which is exactly what the warm pass
        // is supposed to pick up.
        assert_eq!(warm.scanned_files, out.scanned_files);

        let mut totals = TokenCounts::default();
        let mut requests: u64 = 0;
        for b in &out.buckets {
            totals.add(&b.tokens);
            requests += u64::from(b.requests);
        }
        println!("elapsed: {:?}", elapsed);
        println!("scanned_files: {}", out.scanned_files);
        println!("buckets: {}", out.buckets.len());
        println!("requests: {requests}");
        println!(
            "tokens: total={} input={} output={} cache_read={} cache_write={}",
            totals.total(),
            totals.input,
            totals.output,
            totals.cache_read,
            totals.cache_write
        );
        println!("sessions: {}", out.sessions.len());
        let mut by_provider_sessions: BTreeMap<&str, u32> = BTreeMap::new();
        for s in &out.sessions {
            *by_provider_sessions.entry(s.provider.as_str()).or_default() += 1;
        }
        for (p, n) in by_provider_sessions {
            println!("sessions[{p}]: {n}");
        }
        let mut by_provider: BTreeMap<&str, u64> = BTreeMap::new();
        for b in &out.buckets {
            *by_provider.entry(b.provider.as_str()).or_default() += b.tokens.total();
        }
        for (p, t) in by_provider {
            println!("provider {p}: {t} tokens");
        }
    }
}

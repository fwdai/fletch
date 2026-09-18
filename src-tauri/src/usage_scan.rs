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
//! # Incremental cache
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
//! whole on the following scan. [`scan_all`] uses one process-lifetime cache;
//! tests and callers that want isolation use [`scan_dirs_with`].
//!
//! # On-disk cache
//!
//! That cache is also persisted, as JSON under the app's data dir, so the first
//! scan after a restart is warm too: a cold walk of a real machine reads over a
//! gigabyte, a loaded one reads nothing. Nothing about the file is trusted —
//! every entry is re-validated against the file's current `(len, mtime)` at
//! scan time, exactly as an in-memory entry is, so a stale or corrupt cache
//! costs at most a re-read. A missing file, a parse error or a version bump all
//! degrade to "start empty"; persistence never surfaces an error to the user.
//! Entries whose file has disappeared are pruned by each scan, so the file
//! tracks the transcripts on disk instead of growing forever.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Instant, UNIX_EPOCH};

use chrono::{Local, TimeZone, Timelike};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROVIDER_CLAUDE: &str = "claude";
pub const PROVIDER_CODEX: &str = "codex";

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
            if cache.refresh(&file, Source::Claude, since_ms, &mut io) {
                files.push(file);
            }
        }
    }

    for dir in codex_sessions_dirs {
        for year in read_subdirs(dir) {
            for month in read_subdirs(&year) {
                for day in read_subdirs(&month) {
                    for file in read_jsonl_files(&day) {
                        if cache.refresh(&file, Source::Codex, since_ms, &mut io) {
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

// ── cache ───────────────────────────────────────────────────────────────────

/// Which parser a file is fed to. Also picks what per-file state has to survive
/// between scans: Claude lines are self-describing, Codex lines are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Claude,
    Codex,
}

/// One of the two provider constants. Spelled as an alias, not as a bare
/// `&'static str`, because serde's derive infers lifetime bounds from the
/// *syntax* of a field's type: written out, it would saddle every struct
/// holding a record with a `'de: 'static` bound, which no borrowing
/// deserializer can satisfy. The value is always `PROVIDER_CLAUDE` or
/// `PROVIDER_CODEX`, so it costs no allocation and compares by pointer.
type ProviderName = &'static str;

/// One parsed usage record, before windowing/dedupe/bucketing. Deliberately
/// window-independent so a cached record answers any later window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct UsageRecord {
    ts_ms: i64,
    #[serde(deserialize_with = "provider_from_str")]
    provider: ProviderName,
    model: String,
    /// `None` for Codex, whose session id belongs to the file (see
    /// [`FileEntry::codex_session_id`]), and for Claude records with no
    /// `sessionId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    tokens: TokenCounts,
    /// `{message.id}:{requestId}` for Claude, deduped globally at aggregation
    /// time. `None` when neither id is present, or for Codex (whose duplicate
    /// suppression is positional and happens while parsing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dedupe_key: Option<String>,
}

/// Map a persisted provider name back onto the `&'static str` the rest of the
/// module compares by value — an unknown one is a cache from another version
/// and fails the whole load back to an empty cache.
fn provider_from_str<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ProviderName, D::Error> {
    match String::deserialize(d)?.as_str() {
        PROVIDER_CLAUDE => Ok(PROVIDER_CLAUDE),
        PROVIDER_CODEX => Ok(PROVIDER_CODEX),
        other => Err(serde::de::Error::custom(format!(
            "unknown provider {other:?}"
        ))),
    }
}

/// What one file contributed, plus enough metadata to resume it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FileEntry {
    /// File length when `records`/`offset` were last brought up to date.
    len: u64,
    mtime_ms: i64,
    /// Bytes consumed, always just past a `\n`: a trailing partial line is left
    /// for the next scan to read whole.
    offset: u64,
    records: Vec<UsageRecord>,
    /// Codex only: the state machine's position, so appended lines parse as if
    /// the whole file had been read in one pass.
    codex: Option<CodexState>,
}

impl FileEntry {
    fn new(source: Source) -> Self {
        Self {
            len: 0,
            mtime_ms: i64::MIN,
            offset: 0,
            records: Vec::new(),
            codex: match source {
                Source::Claude => None,
                Source::Codex => Some(CodexState::default()),
            },
        }
    }

    /// Codex session id for this file, falling back to its path so a rollout
    /// with no `session_meta` still counts as one distinct session.
    fn codex_session_id(&self, path: &Path) -> String {
        self.codex
            .as_ref()
            .and_then(|s| s.session_id.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| path.to_string_lossy().into_owned())
    }
}

/// Per-path parse cache. Lives for the process in [`scan_all`]; tests own
/// theirs. Change detection is `(len, mtime-in-ms)`, which cannot see an
/// in-place rewrite that preserves the exact length within the same
/// millisecond — transcripts are append-only, so that doesn't happen.
#[derive(Default)]
pub struct ScanCache {
    files: HashMap<PathBuf, FileEntry>,
}

#[derive(Default)]
struct IoStats {
    files_read: u32,
    bytes_read: u64,
}

impl ScanCache {
    /// Bring `path`'s entry up to date, reading as little as possible. Returns
    /// whether the file has an entry to aggregate (`false` = prefiltered away).
    fn refresh(&mut self, path: &Path, source: Source, since_ms: i64, io: &mut IoStats) -> bool {
        let Some((len, mtime_ms)) = file_stat(path) else {
            // Size/mtime unknown (racing deletion, odd permissions): read the
            // whole thing, as the pre-cache scanner did. The sentinel len/mtime
            // make the next scan resume from the offset or start over.
            self.read_fully(path, source, 0, i64::MIN, io);
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
            Some(_) => self.read_fully(path, source, len, mtime_ms, io),
            // The mtime prefilter only applies to files never read: one last
            // written before the window opened cannot hold an in-window record.
            // A file with an entry is always cheap enough to stat and reuse.
            None if mtime_ms < since_ms => return false,
            None => self.read_fully(path, source, len, mtime_ms, io),
        }
        true
    }

    /// Replace `path`'s entry with one parsed from the first byte.
    fn read_fully(
        &mut self,
        path: &Path,
        source: Source,
        len: u64,
        mtime_ms: i64,
        io: &mut IoStats,
    ) {
        let mut entry = FileEntry::new(source);
        read_into(&mut entry, path, 0, io);
        entry.len = len;
        entry.mtime_ms = mtime_ms;
        self.files.insert(path.to_path_buf(), entry);
    }
}

// ── persistence ─────────────────────────────────────────────────────────────

/// Basename of the cache under the app data dir.
const CACHE_FILE: &str = "usage-scan-cache.json";

/// Bump whenever the persisted shape or the parsing semantics change: an older
/// file is then dropped rather than reinterpreted, and the next scan rebuilds
/// it. (A changed *parser* matters as much as a changed struct — cached records
/// were produced by the parser of the build that wrote them.)
const CACHE_VERSION: u32 = 1;

/// Where [`scan_all`] keeps its cache: the app data dir, which is already split
/// per build (`…/<bundle id>/dev` in debug), so a debug run can't hand a release
/// install its cache.
fn cache_path() -> PathBuf {
    crate::data_dir().join(CACHE_FILE)
}

#[derive(Deserialize)]
struct PersistedCache {
    version: u32,
    entries: Vec<PersistedEntry>,
}

/// One [`FileEntry`] plus the path it belongs to. Nested rather than flattened:
/// `serde(flatten)` would buffer every record through an intermediate value on
/// load, and this file holds every record on the machine.
#[derive(Deserialize)]
struct PersistedEntry {
    path: PathBuf,
    entry: FileEntry,
}

/// Write side of [`PersistedCache`], borrowing the live cache instead of
/// cloning a copy of every record on the machine just to serialise it.
#[derive(Serialize)]
struct PersistedCacheRef<'a> {
    version: u32,
    entries: Vec<PersistedEntryRef<'a>>,
}

#[derive(Serialize)]
struct PersistedEntryRef<'a> {
    path: &'a Path,
    entry: &'a FileEntry,
}

impl ScanCache {
    /// Restore a cache written by [`save`](Self::save). Anything unexpected —
    /// no file, truncated JSON, a version this build doesn't speak — yields an
    /// empty cache, which only costs the next scan a full read. Touches no
    /// transcript: entries are validated against `(len, mtime)` when the scan
    /// reaches them, so a stale entry here is harmless.
    fn load(path: &Path) -> Self {
        let Ok(bytes) = std::fs::read(path) else {
            return Self::default();
        };
        let parsed = match serde_json::from_slice::<PersistedCache>(&bytes) {
            Ok(parsed) if parsed.version == CACHE_VERSION => parsed,
            Ok(parsed) => {
                tracing::debug!(
                    version = parsed.version,
                    "usage scan cache from another version, ignored"
                );
                return Self::default();
            }
            Err(e) => {
                tracing::warn!("usage scan cache unreadable ({}): {e}", path.display());
                return Self::default();
            }
        };
        Self {
            files: parsed
                .entries
                .into_iter()
                .map(|e| (e.path, e.entry))
                .collect(),
        }
    }

    /// Write the cache to `path`, returning the bytes written. The write goes
    /// to a sibling `.tmp` and is renamed into place, so a crash (or a second
    /// process) can never leave a half-written cache to be loaded — the rename
    /// is atomic, and the loser of a race simply overwrites.
    fn save(&self, path: &Path) -> std::io::Result<u64> {
        let mut entries: Vec<PersistedEntryRef<'_>> = self
            .files
            .iter()
            .map(|(path, entry)| PersistedEntryRef { path, entry })
            .collect();
        // HashMap order is arbitrary and salted per process; sorting makes the
        // file reproducible (and diffable) for the same cache contents.
        entries.sort_by_key(|e| e.path);
        let bytes = serde_json::to_vec(&PersistedCacheRef {
            version: CACHE_VERSION,
            entries,
        })?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let written = (|| -> std::io::Result<u64> {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&tmp, path)?;
            Ok(bytes.len() as u64)
        })();
        if written.is_err() {
            // Don't leave the failed attempt behind for the disk to keep.
            let _ = std::fs::remove_file(&tmp);
        }
        written
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
                PROVIDER_CODEX => codex_session_id.as_deref(),
                _ => record.session_id.as_deref(),
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
    /// is sorted by exactly that tuple with no extra sort pass.
    buckets: BTreeMap<(i64, String, String), (TokenCounts, u32)>,
    /// (provider, session id) → (first ms, last ms) over in-window records.
    sessions: HashMap<(&'static str, String), (i64, i64)>,
}

impl Accumulator {
    fn record(&mut self, ts_ms: i64, provider: &'static str, model: &str, tokens: &TokenCounts) {
        let entry = self
            .buckets
            .entry((
                local_hour_start_ms(ts_ms),
                provider.to_string(),
                model.to_string(),
            ))
            .or_default();
        entry.0.add(tokens);
        entry.1 += 1;
    }

    /// Widen a session's span to include `ts_ms`, creating it on first sight.
    fn touch_session(&mut self, provider: &'static str, id: &str, ts_ms: i64) {
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
                provider: provider.to_string(),
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
                        provider,
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

// ── claude ──────────────────────────────────────────────────────────────────

fn parse_claude_line(line: &str) -> Option<UsageRecord> {
    // Cheap gate: only assistant records carry a `usage` object, and they are a
    // small minority of lines in a transcript.
    if !line.contains("\"usage\"") {
        return None;
    }
    let v = serde_json::from_str::<Value>(line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    if v.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let message = v.get("message")?;
    let usage = message.get("usage").filter(|u| u.is_object())?;
    let model = message
        .get("model")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())?;
    let ts_ms = parse_ts_ms(&v)?;

    // Claude emits the *same* usage object once per content block of a reply,
    // and copies prior records verbatim into the transcript of a resumed or
    // forked session — so the dedupe key has to be global, not per-file. Either
    // half of the key may be absent on odd records; only dedupe when at least
    // one is present.
    let msg_id = message.get("id").and_then(Value::as_str).unwrap_or("");
    let req_id = v.get("requestId").and_then(Value::as_str).unwrap_or("");
    let dedupe_key =
        (!(msg_id.is_empty() && req_id.is_empty())).then(|| format!("{msg_id}:{req_id}"));

    Some(UsageRecord {
        ts_ms,
        provider: PROVIDER_CLAUDE,
        model: model.to_string(),
        session_id: v
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        tokens: TokenCounts {
            input: num(usage, "input_tokens"),
            output: num(usage, "output_tokens"),
            cache_read: num(usage, "cache_read_input_tokens"),
            cache_write: num(usage, "cache_creation_input_tokens"),
        },
        dedupe_key,
    })
}

// ── codex ───────────────────────────────────────────────────────────────────

/// Codex rollouts are a stream, not self-describing records: the model lives in
/// a `turn_context` line that applies to every later `token_count`, so the file
/// has to be read as a small state machine. Cached between scans so appended
/// lines parse exactly as they would have in one pass over the whole file.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct CodexState {
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    saw_session_meta: bool,
    /// Timestamp anchoring the burst of records a fork copied from its parent.
    #[serde(skip_serializing_if = "Option::is_none")]
    fork_anchor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prev_usage: Option<String>,
}

fn parse_codex_line(line: &str, state: &mut CodexState, out: &mut Vec<UsageRecord>) {
    if !(line.contains("token_count")
        || line.contains("turn_context")
        || line.contains("session_meta"))
    {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = v.get("payload");

    if kind == "session_meta" && !state.saw_session_meta {
        state.saw_session_meta = true;
        if let Some(p) = payload {
            state.session_id = p
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| p.get("session_id").and_then(Value::as_str))
                .map(str::to_string);
            // A fork (explicit `codex resume --fork`, or a subagent thread
            // spawned off a parent) starts its rollout with a verbatim copy
            // of the parent's history, token_count events included. Those
            // are replays, not new spend: anchor here and drop the burst.
            let forked = p.get("forked_from_id").and_then(Value::as_str).is_some()
                || p.pointer("/source/subagent/thread_spawn/parent_thread_id")
                    .and_then(Value::as_str)
                    .is_some();
            if forked {
                state.fork_anchor = parse_ts_ms(&v);
            }
        }
        return;
    }

    if kind == "turn_context" {
        if let Some(m) = payload
            .and_then(|p| p.get("model"))
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
        {
            state.model = Some(m.to_string());
        }
        return;
    }

    let Some(payload) = payload else { return };
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return;
    }
    let Some(usage) = payload
        .pointer("/info/last_token_usage")
        .filter(|u| u.is_object())
    else {
        return;
    };
    let Some(model) = state.model.as_deref() else {
        return;
    };
    let Some(ts_ms) = parse_ts_ms(&v) else {
        return;
    };

    // Codex re-emits the previous turn's `last_token_usage` unchanged on
    // some events; only a *changed* payload is a new request.
    let usage_json = usage.to_string();
    if state.prev_usage.as_deref() == Some(usage_json.as_str()) {
        return;
    }
    state.prev_usage = Some(usage_json);

    if let Some(anchor) = state.fork_anchor {
        // The copied burst is written back-to-back at fork time; a real
        // turn lands at least a second later. Walk the anchor forward
        // through the burst, then stop suppressing for good.
        if ts_ms - anchor < 1000 {
            state.fork_anchor = Some(ts_ms);
            return;
        }
        state.fork_anchor = None;
    }

    let input = num(usage, "input_tokens");
    let cached = num(usage, "cached_input_tokens");
    let cache_write = num(usage, "cache_write_input_tokens");
    out.push(UsageRecord {
        ts_ms,
        provider: PROVIDER_CODEX,
        model: model.to_string(),
        // The id belongs to the file, not the record: it may be learned from a
        // `session_meta` line and falls back to the path.
        session_id: None,
        tokens: TokenCounts {
            // Codex reports total input including the cached/written parts;
            // the fresh remainder is what a non-cached input rate applies to.
            input: input.saturating_sub(cached).saturating_sub(cache_write),
            output: num(usage, "output_tokens"),
            cache_read: cached,
            cache_write,
        },
        dedupe_key: None,
    });
}

// ── filesystem + parsing helpers ────────────────────────────────────────────

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

fn parse_ts_ms(v: &Value) -> Option<i64> {
    let s = v.get("timestamp")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

/// Epoch ms of the *local* hour containing an instant. Bucket boundaries sit on
/// the user's wall-clock hour, so summing whole hours into a day reproduces
/// their midnight-to-midnight "today". Falls back to the UTC hour if the local
/// time is ambiguous or nonexistent (the one DST-transition hour per year).
fn local_hour_start_ms(ms: i64) -> i64 {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .and_then(|d| {
            d.with_minute(0)
                .and_then(|d| d.with_second(0))
                .and_then(|d| d.with_nanosecond(0))
        })
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|| ms - ms.rem_euclid(3_600_000))
}

/// A token field, clamped at 0 — absent, null, non-numeric and negative all
/// mean "nothing to count".
fn num(v: &Value, key: &str) -> u64 {
    v.get(key)
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0)
        .try_into()
        .unwrap_or(0)
}

/// Backdate a file's mtime so the prefilter treats it as old (tests, and the
/// `SystemTime` plumbing the prefilter reads).
#[cfg(test)]
fn set_mtime_ms(path: &Path, ms: i64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    let t = UNIX_EPOCH + std::time::Duration::from_millis(ms as u64);
    file.set_times(std::fs::FileTimes::new().set_modified(t))
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(iso: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .timestamp_millis()
    }

    /// Write `<root>/projects/<slug>/<name>.jsonl`, returning the projects dir.
    fn claude_file(root: &Path, slug: &str, name: &str, lines: &[String]) -> PathBuf {
        let projects = root.join("projects");
        let dir = projects.join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.jsonl")), lines.join("\n") + "\n").unwrap();
        projects
    }

    /// Write `<root>/sessions/2026/01/02/rollout-<name>.jsonl`, returning the
    /// sessions dir.
    fn codex_file(root: &Path, name: &str, lines: &[String]) -> PathBuf {
        let sessions = root.join("sessions");
        let dir = sessions.join("2026").join("01").join("02");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("rollout-{name}.jsonl")),
            lines.join("\n") + "\n",
        )
        .unwrap();
        sessions
    }

    fn claude_line(
        session: &str,
        msg_id: &str,
        req_id: &str,
        ts: &str,
        model: &str,
        usage: Value,
    ) -> String {
        serde_json::json!({
            "type": "assistant",
            "sessionId": session,
            "requestId": req_id,
            "timestamp": ts,
            "message": { "id": msg_id, "model": model, "usage": usage },
        })
        .to_string()
    }

    fn claude_usage(input: u64, output: u64, read: u64, write: u64) -> Value {
        serde_json::json!({
            "input_tokens": input,
            "output_tokens": output,
            "cache_read_input_tokens": read,
            "cache_creation_input_tokens": write,
        })
    }

    fn codex_token_count(ts: &str, usage: Value) -> String {
        serde_json::json!({
            "timestamp": ts,
            "type": "event_msg",
            "payload": { "type": "token_count", "info": { "last_token_usage": usage } },
        })
        .to_string()
    }

    fn codex_usage(input: u64, cached: u64, cache_write: u64, output: u64) -> Value {
        serde_json::json!({
            "input_tokens": input,
            "cached_input_tokens": cached,
            "cache_write_input_tokens": cache_write,
            "output_tokens": output,
        })
    }

    fn codex_turn_context(ts: &str, model: &str) -> String {
        serde_json::json!({
            "timestamp": ts,
            "type": "turn_context",
            "payload": { "model": model },
        })
        .to_string()
    }

    /// Path of a file written by [`claude_file`].
    fn claude_path(projects: &Path, slug: &str, name: &str) -> PathBuf {
        projects.join(slug).join(format!("{name}.jsonl"))
    }

    /// Path of a file written by [`codex_file`].
    fn codex_path(sessions: &Path, name: &str) -> PathBuf {
        sessions
            .join("2026")
            .join("01")
            .join("02")
            .join(format!("rollout-{name}.jsonl"))
    }

    /// Append raw bytes, as the CLIs do while a session runs. Returns how many.
    fn append(path: &Path, text: &str) -> u64 {
        use std::io::Write;
        std::fs::File::options()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        text.len() as u64
    }

    fn wide_window() -> (i64, i64) {
        (ms("2000-01-01T00:00:00Z"), ms("2100-01-01T00:00:00Z"))
    }

    fn session_ids(out: &UsageScan, provider: &str) -> Vec<String> {
        out.sessions
            .iter()
            .filter(|s| s.provider == provider)
            .map(|s| s.id.clone())
            .collect()
    }

    fn span(out: &UsageScan, provider: &str, id: &str) -> (i64, i64) {
        let s = out
            .sessions
            .iter()
            .find(|s| s.provider == provider && s.id == id)
            .unwrap_or_else(|| panic!("no {provider} session {id}"));
        (s.first_ms, s.last_ms)
    }

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
    fn claude_skips_api_errors_zero_token_and_modelless_records() {
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
        let no_model = claude_line(
            "s1",
            "m2",
            "r2",
            "2026-01-02T10:00:00Z",
            "",
            claude_usage(5, 5, 0, 0),
        );
        let projects = claude_file(td.path(), "slug", "s1", &[err.to_string(), zero, no_model]);

        let (since, until) = wide_window();
        let out = scan_dirs(&[projects], &[], since, until);
        assert!(out.buckets.is_empty());
        assert!(out.sessions.is_empty());
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
                // (100-60-10) + (200-150-0)
                input: 30 + 50,
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
    fn hour_starts_are_aligned_to_the_local_wall_clock() {
        // Whatever the local offset, the bucket start must be an exact hour
        // boundary in local time — i.e. zero minutes/seconds/millis.
        let t = ms("2026-01-02T10:37:42.123Z");
        let start = local_hour_start_ms(t);
        assert!(start <= t && t - start < 3_600_000);
        let local = Local.timestamp_millis_opt(start).single().unwrap();
        assert_eq!(
            (local.minute(), local.second(), local.nanosecond()),
            (0, 0, 0)
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

    // ── incremental cache ───────────────────────────────────────────────────

    /// Everything about an answer that must not depend on what the cache did.
    fn answer(out: &UsageScan) -> Value {
        serde_json::json!({
            "buckets": out.buckets,
            "sessions": out.sessions,
            "scannedFiles": out.scanned_files,
        })
    }

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

    // ── persisted cache ─────────────────────────────────────────────────────

    /// Cache contents in a comparable, order-independent form.
    fn entries(cache: &ScanCache) -> Vec<(&PathBuf, &FileEntry)> {
        let mut out: Vec<(&PathBuf, &FileEntry)> = cache.files.iter().collect();
        out.sort_by_key(|(path, _)| *path);
        out
    }

    /// A tempdir holding one Claude transcript and one Codex rollout, plus the
    /// two roots and a path to keep a cache file at.
    struct Fixture {
        _td: tempfile::TempDir,
        projects: PathBuf,
        sessions: PathBuf,
        cache_file: PathBuf,
    }

    fn fixture() -> Fixture {
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
                    claude_usage(10, 5, 7, 3),
                ),
                claude_line(
                    "s1",
                    "m2",
                    "r2",
                    "2026-01-02T11:00:00Z",
                    "claude-opus-4",
                    claude_usage(1, 2, 0, 0),
                ),
            ],
        );
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
        let cache_file = td.path().join("state").join("usage-scan-cache.json");
        Fixture {
            _td: td,
            projects,
            sessions,
            cache_file,
        }
    }

    impl Fixture {
        fn scan_with(&self, cache: &mut ScanCache) -> UsageScan {
            let (since, until) = wide_window();
            scan_dirs_with(
                cache,
                std::slice::from_ref(&self.projects),
                std::slice::from_ref(&self.sessions),
                since,
                until,
            )
        }
    }

    #[test]
    fn saving_and_loading_round_trips_every_entry() {
        let fx = fixture();
        let mut cache = ScanCache::default();
        fx.scan_with(&mut cache);

        let bytes = cache.save(&fx.cache_file).unwrap();
        assert_eq!(
            bytes,
            std::fs::metadata(&fx.cache_file).unwrap().len(),
            "the reported size is what landed on disk"
        );

        let loaded = ScanCache::load(&fx.cache_file);
        // Records, offsets and the Codex state machine all come back identical
        // — the entry is what a resumed parse continues from.
        assert_eq!(entries(&loaded), entries(&cache));
        let codex = loaded
            .files
            .get(&codex_path(&fx.sessions, "a"))
            .expect("codex entry");
        assert_eq!(
            codex.codex.as_ref().unwrap().session_id.as_deref(),
            Some("codex-1")
        );
        assert_eq!(
            codex.codex.as_ref().unwrap().model.as_deref(),
            Some("gpt-5")
        );
        assert!(codex.offset > 0 && codex.offset == codex.len);
        // Re-saving the loaded cache reproduces the file byte for byte.
        let again = fx.cache_file.with_extension("again");
        loaded.save(&again).unwrap();
        assert_eq!(
            std::fs::read(&fx.cache_file).unwrap(),
            std::fs::read(&again).unwrap()
        );
    }

    #[test]
    fn loading_a_missing_garbage_or_future_cache_starts_empty() {
        let td = tempfile::tempdir().unwrap();
        let missing = td.path().join("nope").join("usage-scan-cache.json");
        assert!(ScanCache::load(&missing).files.is_empty());

        let garbage = td.path().join("garbage.json");
        std::fs::write(&garbage, b"{not json at all").unwrap();
        assert!(ScanCache::load(&garbage).files.is_empty());

        // Right shape, wrong version: a future build's records may have come
        // out of a different parser, so they are dropped, not reinterpreted.
        let future = td.path().join("future.json");
        std::fs::write(
            &future,
            serde_json::json!({
                "version": CACHE_VERSION + 1,
                "entries": [{
                    "path": "/tmp/whatever.jsonl",
                    "entry": { "len": 1, "mtime_ms": 2, "offset": 1, "records": [], "codex": null },
                }],
            })
            .to_string(),
        )
        .unwrap();
        assert!(ScanCache::load(&future).files.is_empty());

        // An entry serde can't make sense of fails the load as a whole rather
        // than half-restoring the cache.
        let bad_entry = td.path().join("bad-entry.json");
        std::fs::write(
            &bad_entry,
            serde_json::json!({
                "version": CACHE_VERSION,
                "entries": [{ "path": "/tmp/x.jsonl", "entry": { "len": "huge" } }],
            })
            .to_string(),
        )
        .unwrap();
        assert!(ScanCache::load(&bad_entry).files.is_empty());
    }

    #[test]
    fn a_scan_through_a_loaded_cache_reads_nothing_and_answers_the_same() {
        // The whole point of persisting: a fresh process' first scan is warm.
        let fx = fixture();
        let mut cold_cache = ScanCache::default();
        let cold = fx.scan_with(&mut cold_cache);
        assert_eq!(cold.files_read, 2);
        cold_cache.save(&fx.cache_file).unwrap();

        let mut restored = ScanCache::load(&fx.cache_file);
        let warm = fx.scan_with(&mut restored);
        assert_eq!(
            (warm.files_read, warm.bytes_read),
            (0, 0),
            "a restored cache must not re-open an unchanged transcript"
        );
        assert_eq!(answer(&warm), answer(&cold));
    }

    #[test]
    fn a_restored_cache_still_resumes_an_appended_file_and_re_reads_a_rewritten_one() {
        let fx = fixture();
        let mut cache = ScanCache::default();
        fx.scan_with(&mut cache);
        cache.save(&fx.cache_file).unwrap();

        // Append to the Codex rollout: the restored state machine has to supply
        // the model and the previous usage, exactly as the in-memory one did.
        let added = append(
            &codex_path(&fx.sessions, "a"),
            &format!(
                "{}\n{}\n",
                codex_token_count("2026-01-02T10:00:02Z", codex_usage(100, 60, 10, 20)),
                codex_token_count("2026-01-02T10:01:00Z", codex_usage(200, 150, 0, 40)),
            ),
        );

        let mut restored = ScanCache::load(&fx.cache_file);
        let warm = fx.scan_with(&mut restored);
        assert_eq!((warm.files_read, warm.bytes_read), (1, added));
        // Same answer as re-reading both files from scratch.
        let mut fresh = ScanCache::default();
        assert_eq!(answer(&warm), answer(&fx.scan_with(&mut fresh)));
    }

    #[test]
    fn a_deleted_transcript_is_pruned_from_the_saved_cache() {
        // Nothing else bounds the file: without this it would accumulate every
        // transcript that ever existed on the machine.
        let fx = fixture();
        let extra = claude_path(&fx.projects, "slug", "s1")
            .parent()
            .unwrap()
            .join("s2.jsonl");
        std::fs::write(
            &extra,
            format!(
                "{}\n",
                claude_line(
                    "s2",
                    "m9",
                    "r9",
                    "2026-01-02T12:00:00Z",
                    "claude-opus-4",
                    claude_usage(4, 4, 0, 0),
                )
            ),
        )
        .unwrap();

        let mut cache = ScanCache::default();
        fx.scan_with(&mut cache);
        assert!(cache.files.contains_key(&extra));
        cache.save(&fx.cache_file).unwrap();
        assert!(ScanCache::load(&fx.cache_file).files.contains_key(&extra));

        std::fs::remove_file(&extra).unwrap();
        let mut restored = ScanCache::load(&fx.cache_file);
        let out = fx.scan_with(&mut restored);
        assert_eq!(out.scanned_files, 2, "the deleted file is not aggregated");
        assert!(!restored.files.contains_key(&extra), "nor kept in memory");
        restored.save(&fx.cache_file).unwrap();
        assert!(!ScanCache::load(&fx.cache_file).files.contains_key(&extra));
    }

    #[test]
    fn saving_is_atomic_and_leaves_no_temporary_behind() {
        let fx = fixture();
        let mut cache = ScanCache::default();
        fx.scan_with(&mut cache);

        // The parent dir doesn't exist yet on the first save.
        cache.save(&fx.cache_file).unwrap();
        // ...and a second save overwrites in place, tmp included.
        cache.save(&fx.cache_file).unwrap();

        let dir = fx.cache_file.parent().unwrap();
        let names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["usage-scan-cache.json".to_string()]);
    }

    #[test]
    fn the_cache_lives_in_the_apps_data_dir() {
        let path = cache_path();
        assert!(path.starts_with(crate::data_dir()));
        assert_eq!(path.file_name().unwrap(), CACHE_FILE);
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

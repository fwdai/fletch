//! Fixture builders shared by the tests of every submodule: writers for the
//! two on-disk transcript layouts, line templates for both formats, a few
//! accessors over a finished [`UsageScan`], and an I/O source that can be made
//! to fail so the cache's recovery paths are reachable.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde_json::Value;

use super::cache::{ReadSeek, ScanCache};
use super::UsageScan;

pub(super) fn ms(iso: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(iso)
        .unwrap()
        .timestamp_millis()
}

/// Write `<root>/projects/<slug>/<name>.jsonl`, returning the projects dir.
pub(super) fn claude_file(root: &Path, slug: &str, name: &str, lines: &[String]) -> PathBuf {
    let projects = root.join("projects");
    let dir = projects.join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.jsonl")), lines.join("\n") + "\n").unwrap();
    projects
}

/// Write `<root>/sessions/2026/01/02/rollout-<name>.jsonl`, returning the
/// sessions dir.
pub(super) fn codex_file(root: &Path, name: &str, lines: &[String]) -> PathBuf {
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

pub(super) fn claude_line(
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

pub(super) fn claude_usage(input: u64, output: u64, read: u64, write: u64) -> Value {
    serde_json::json!({
        "input_tokens": input,
        "output_tokens": output,
        "cache_read_input_tokens": read,
        "cache_creation_input_tokens": write,
    })
}

pub(super) fn codex_token_count(ts: &str, usage: Value) -> String {
    serde_json::json!({
        "timestamp": ts,
        "type": "event_msg",
        "payload": { "type": "token_count", "info": { "last_token_usage": usage } },
    })
    .to_string()
}

pub(super) fn codex_usage(input: u64, cached: u64, cache_write: u64, output: u64) -> Value {
    serde_json::json!({
        "input_tokens": input,
        "cached_input_tokens": cached,
        "cache_write_input_tokens": cache_write,
        "output_tokens": output,
    })
}

pub(super) fn codex_turn_context(ts: &str, model: &str) -> String {
    serde_json::json!({
        "timestamp": ts,
        "type": "turn_context",
        "payload": { "model": model },
    })
    .to_string()
}

/// Path of a file written by [`claude_file`].
pub(super) fn claude_path(projects: &Path, slug: &str, name: &str) -> PathBuf {
    projects.join(slug).join(format!("{name}.jsonl"))
}

/// Path of a file written by [`codex_file`].
pub(super) fn codex_path(sessions: &Path, name: &str) -> PathBuf {
    sessions
        .join("2026")
        .join("01")
        .join("02")
        .join(format!("rollout-{name}.jsonl"))
}

/// Append raw bytes, as the CLIs do while a session runs. Returns how many.
pub(super) fn append(path: &Path, text: &str) -> u64 {
    use std::io::Write;
    std::fs::File::options()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    text.len() as u64
}

/// Backdate a file's mtime so the prefilter treats it as old (tests, and the
/// `SystemTime` plumbing the prefilter reads).
pub(super) fn set_mtime_ms(path: &Path, ms: i64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    let t = UNIX_EPOCH + std::time::Duration::from_millis(ms as u64);
    file.set_times(std::fs::FileTimes::new().set_modified(t))
        .unwrap();
}

pub(super) fn wide_window() -> (i64, i64) {
    (ms("2000-01-01T00:00:00Z"), ms("2100-01-01T00:00:00Z"))
}

pub(super) fn session_ids(out: &UsageScan, provider: &str) -> Vec<String> {
    out.sessions
        .iter()
        .filter(|s| s.provider == provider)
        .map(|s| s.id.clone())
        .collect()
}

pub(super) fn span(out: &UsageScan, provider: &str, id: &str) -> (i64, i64) {
    let s = out
        .sessions
        .iter()
        .find(|s| s.provider == provider && s.id == id)
        .unwrap_or_else(|| panic!("no {provider} session {id}"));
    (s.first_ms, s.last_ms)
}

// ── injected I/O failures ───────────────────────────────────────────────────

/// One way a transcript read can go wrong. Real causes are boring and
/// transient — a network volume blinking, a file swapped under us, an fd limit
/// — and all of them used to poison the cache permanently, so each is
/// reproduced here exactly once.
#[derive(Debug, Clone, Copy)]
pub(super) enum Fail {
    /// `open` fails: the scan learns nothing at all about the file.
    Open,
    /// The file opens but the seek to the cached offset fails, so a resumed
    /// read can't even start.
    Seek,
    /// The file opens and streams, then errors after `n` bytes.
    AfterBytes(u64),
}

/// Make `cache`'s next read fail the given way; every read after it is an
/// ordinary file read — i.e. a transient fault, which is what the recovery has
/// to survive.
pub(super) fn fail_once(cache: &mut ScanCache, fail: Fail) {
    let pending = Mutex::new(Some(fail));
    cache.opener = Some(Box::new(move |path: &Path| {
        let fail = pending.lock().unwrap().take();
        if matches!(fail, Some(Fail::Open)) {
            return Err(std::io::Error::other("injected open failure"));
        }
        let file = std::fs::File::open(path)?;
        Ok(Box::new(FailingReader {
            inner: file,
            fail_seek: matches!(fail, Some(Fail::Seek)),
            budget: match fail {
                Some(Fail::AfterBytes(n)) => Some(n),
                _ => None,
            },
        }) as Box<dyn ReadSeek>)
    }));
}

/// A fresh cache whose first read fails, per [`fail_once`].
pub(super) fn cache_failing_once(fail: Fail) -> ScanCache {
    let mut cache = ScanCache::default();
    fail_once(&mut cache, fail);
    cache
}

/// A file that refuses to seek, or stops delivering bytes part-way through.
struct FailingReader {
    inner: std::fs::File,
    fail_seek: bool,
    /// Bytes still allowed through before the stream errors. `None` = no limit.
    budget: Option<u64>,
}

impl Read for FailingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Some(budget) = self.budget else {
            return self.inner.read(buf);
        };
        if budget == 0 {
            return Err(std::io::Error::other("injected read failure"));
        }
        let take = buf.len().min(budget as usize);
        let read = self.inner.read(&mut buf[..take])?;
        self.budget = Some(budget - read as u64);
        Ok(read)
    }
}

impl Seek for FailingReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        if self.fail_seek {
            return Err(std::io::Error::other("injected seek failure"));
        }
        self.inner.seek(pos)
    }
}

/// Everything about an answer that must not depend on what the cache did.
pub(super) fn answer(out: &UsageScan) -> Value {
    serde_json::json!({
        "buckets": out.buckets,
        "sessions": out.sessions,
        "scannedFiles": out.scanned_files,
    })
}

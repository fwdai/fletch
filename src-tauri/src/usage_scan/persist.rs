//! On-disk form of the [`ScanCache`].
//!
//! The cache is persisted, as JSON under the app's data dir, so the first scan
//! after a restart is warm too: a cold walk of a real machine reads over a
//! gigabyte, a loaded one reads nothing. Nothing about the file is trusted —
//! every entry is re-validated against the file's current `(len, mtime)` at
//! scan time, exactly as an in-memory entry is, so a stale or corrupt cache
//! costs at most a re-read. A missing file, a parse error or a version bump all
//! degrade to "start empty"; persistence never surfaces an error to the user.
//! Entries whose file has disappeared are pruned by each scan, so the file
//! tracks the transcripts on disk instead of growing forever.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::cache::{FileEntry, ScanCache};

/// Basename of the cache under the app data dir.
pub(super) const CACHE_FILE: &str = "usage-scan-cache.json";

/// Bump whenever the persisted shape or the parsing semantics change: an older
/// file is then dropped rather than reinterpreted, and the next scan rebuilds
/// it. (A changed *parser* matters as much as a changed struct — cached records
/// were produced by the parser of the build that wrote them.)
/// 2: Claude records now keep a model-less call, skip `<synthetic>`, prefer the
/// `cache_creation` TTL breakdown and only dedupe on a `message.id`; Codex no
/// longer subtracts cache writes from fresh input.
/// 3: Claude entries carry a `ClaudeState`, and a record naming no model is
/// attributed to the last model its transcript named instead of `""`.
const CACHE_VERSION: u32 = 3;

/// Where [`scan_all`](super::scan_all) keeps its cache: the app data dir, which
/// is already split per build (`…/<bundle id>/dev` in debug), so a debug run
/// can't hand a release install its cache.
pub(super) fn cache_path() -> PathBuf {
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
    pub(super) fn load(path: &Path) -> Self {
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
        // `..default()` is redundant in a release build and required in a test
        // one, where `ScanCache` also carries an injectable I/O source.
        #[allow(clippy::needless_update)]
        Self {
            files: parsed
                .entries
                .into_iter()
                .map(|e| (e.path, e.entry))
                .collect(),
            ..Self::default()
        }
    }

    /// Write the cache to `path`, returning the bytes written. The write goes
    /// to a sibling temporary file and is renamed into place, so a crash can
    /// never leave a half-written cache to be loaded — the rename is atomic.
    /// The temporary name is unique per write (pid + nanos), so two processes
    /// saving at once each rename their own complete file and the later rename
    /// simply wins; a shared `.tmp` would let them truncate each other's bytes
    /// before either renamed.
    pub(super) fn save(&self, path: &Path) -> std::io::Result<u64> {
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
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(format!(".{}-{nanos}.tmp", std::process::id()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_scan::test_support::*;
    use crate::usage_scan::{scan_dirs_with, UsageScan};

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
        // Claude's carry-forward model comes back the same way, so a record
        // appended later without one is still attributed.
        let claude = loaded
            .files
            .get(&claude_path(&fx.projects, "slug", "s1"))
            .expect("claude entry");
        assert_eq!(
            claude.claude.as_ref().unwrap().last_model.as_deref(),
            Some("claude-opus-4")
        );
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
                    "entry": {
                        "len": 1, "mtime_ms": 2, "offset": 1, "records": [],
                        "claude": {}, "codex": null,
                    },
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
        // ...and a second save overwrites in place; its own temporary is gone
        // too, even though every write uses a fresh name.
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
}

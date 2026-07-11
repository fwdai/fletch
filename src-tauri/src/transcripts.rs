//! Locating and reading provider transcript files on disk.
//!
//! Pure filesystem mechanism — no supervisor or workspace state. The
//! per-provider reader tables in `agent.rs` point their `locate`/`read`
//! hooks here.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::ReadDiagnostics;

/// Directory (under an agent's writable root) that the Docker sandbox binds
/// over the container's read-only `<config-dir>/projects`, so claude's session
/// transcripts persist on a per-agent host dir instead of the shared, read-only
/// `~/.claude` (see `sandbox::docker`). Shared with the mount side so the writer
/// and this reader agree on one name.
pub(crate) const DOCKER_CLAUDE_PROJECTS_DIRNAME: &str = ".fletch-claude-projects";

/// Locate the claude session JSONL by scanning the candidate `projects/*/`
/// dirs (see [`claude_projects_dirs`]) for `<session-id>.jsonl`. Claude's
/// path-encoding scheme isn't part of its public API, so we glob instead of
/// recomputing the encoded directory name from the checkout path.
///
/// A Docker-sandboxed agent writes its transcript to a per-agent host dir
/// ([`DOCKER_CLAUDE_PROJECTS_DIRNAME`]) rather than the shared `~/.claude`, so
/// that dir is scanned first. The agent's cwd is `<writable_root>/<repo>`, so
/// its parent is the writable root that holds it. Seatbelt agents have no such
/// dir (it won't exist and is skipped), so the scan falls through to the
/// standard `~/.claude` / `CLAUDE_CONFIG_DIR` candidates.
pub(crate) fn find_session_jsonl(
    session_id: &str,
    cwd: &Path,
    diag: &mut ReadDiagnostics,
) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(parent) = cwd.parent() {
        dirs.push(parent.join(DOCKER_CLAUDE_PROJECTS_DIRNAME));
    }
    dirs.extend(claude_projects_dirs());
    find_session_jsonl_in(&dirs, session_id, diag)
}

/// Candidate `projects` directories Claude may have written transcripts to.
/// Honors `CLAUDE_CONFIG_DIR` (Claude CLI's own config-dir override) and always
/// also includes the default `~/.claude`, so a transcript is located regardless
/// of which config dir was active when the agent was spawned. Mirrors the way
/// `find_codex_rollouts` honors `CODEX_HOME` — without this, an agent spawned
/// with `CLAUDE_CONFIG_DIR` set wrote its transcript somewhere we never scanned,
/// so it was never ingested into `session_records` and was lost when that dir
/// moved.
fn claude_projects_dirs() -> Vec<PathBuf> {
    projects_dirs_from(
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        dirs::home_dir(),
    )
}

/// Pure core of [`claude_projects_dirs`]: configured dir first (if any), then
/// the default `~/.claude`, deduped so an explicit `CLAUDE_CONFIG_DIR=~/.claude`
/// doesn't double-scan.
fn projects_dirs_from(config_dir: Option<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |base: PathBuf| {
        let p = base.join("projects");
        if !out.contains(&p) {
            out.push(p);
        }
    };
    if let Some(cfg) = config_dir {
        push(cfg);
    }
    if let Some(home) = home {
        push(home.join(".claude"));
    }
    out
}

/// Scan the given `projects` dirs for `<session-id>.jsonl`, returning the first
/// match. A missing/unreadable candidate dir is skipped, not fatal, so a later
/// candidate is still searched.
fn find_session_jsonl_in(
    projects_dirs: &[PathBuf],
    session_id: &str,
    diag: &mut ReadDiagnostics,
) -> Option<PathBuf> {
    let filename = format!("{session_id}.jsonl");
    for projects in projects_dirs {
        let Ok(entries) = std::fs::read_dir(projects) else {
            continue;
        };
        // A readable `projects` candidate is a live Claude root — Claude's home
        // hasn't moved out from under us (either of its two roots counts).
        diag.root_exists = true;
        for entry in entries.flatten() {
            let path = entry.path().join(&filename);
            if path.exists() {
                diag.files_matched += 1;
                return Some(path);
            }
        }
    }
    None
}

/// All of codex's rollout files for a thread id, ordered (filenames are
/// timestamp-prefixed, so lexical sort == chronological). Codex stores sessions
/// at `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl` (CODEX_HOME
/// defaults to `~/.codex`); the id suffix is the thread id we captured. Resume
/// normally keeps one file per session, but returning all is correct if it splits.
pub(crate) fn find_codex_rollouts(session_id: &str, diag: &mut ReadDiagnostics) -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
    else {
        return Vec::new();
    };
    // Anchor on the `-<id>.jsonl` boundary (filenames are
    // `rollout-<ts>-<id>.jsonl`) so one thread id can't match another whose
    // name merely ends with the same characters.
    let suffix = format!("-{session_id}.jsonl");
    // Walk the YYYY/MM/DD tree (three dir levels) and match the suffix.
    fn dirs_in(p: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect()
    }
    let sessions = home.join("sessions");
    diag.root_exists = sessions.exists();
    let mut out = Vec::new();
    for year in dirs_in(&sessions) {
        for month in dirs_in(&year) {
            for day in dirs_in(&month) {
                for entry in std::fs::read_dir(&day).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with(&suffix))
                    {
                        out.push(path);
                    }
                }
            }
        }
    }
    out.sort();
    diag.files_matched += out.len();
    out
}

/// Read a JSONL file into a vec of parsed values, skipping blank lines. A
/// non-blank line that fails to parse is counted (`lines_seen`) but not
/// returned, so `lines_seen > records_parsed` surfaces a format drift instead
/// of silently vanishing; an unreadable file — or a read that fails partway
/// through, leaving an unread tail — bumps `io_errors`. Infallible by design —
/// every value that DID parse is still returned.
pub(crate) fn read_jsonl_values(path: &Path, diag: &mut ReadDiagnostics) -> Vec<Value> {
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(path) else {
        diag.io_errors += 1;
        return Vec::new();
    };
    let reader = std::io::BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                // Mid-stream read failure: the rest of the file is unreadable,
                // so record it rather than letting the tail look ingested.
                diag.io_errors += 1;
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        diag.lines_seen += 1;
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            diag.records_parsed += 1;
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── claude transcript location (CLAUDE_CONFIG_DIR) ────────────────────────

    #[test]
    fn projects_dirs_prefers_config_dir_then_default_home() {
        let dirs = projects_dirs_from(
            Some(PathBuf::from("/home/u/.claude-eve")),
            Some(PathBuf::from("/home/u")),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.claude-eve/projects"),
                PathBuf::from("/home/u/.claude/projects"),
            ],
        );
    }

    #[test]
    fn projects_dirs_dedups_when_config_is_default() {
        // CLAUDE_CONFIG_DIR explicitly set to ~/.claude must not double-scan.
        let dirs = projects_dirs_from(
            Some(PathBuf::from("/home/u/.claude")),
            Some(PathBuf::from("/home/u")),
        );
        assert_eq!(dirs, vec![PathBuf::from("/home/u/.claude/projects")]);
    }

    #[test]
    fn projects_dirs_falls_back_to_home_when_config_unset() {
        let dirs = projects_dirs_from(None, Some(PathBuf::from("/home/u")));
        assert_eq!(dirs, vec![PathBuf::from("/home/u/.claude/projects")]);
    }

    #[test]
    fn find_session_jsonl_locates_transcript_in_relocated_config_dir() {
        // Regression: the locator used to hardcode `~/.claude/projects`, so an
        // agent spawned with CLAUDE_CONFIG_DIR pointing elsewhere wrote its
        // transcript to a dir we never scanned — it was never ingested and was
        // lost when that dir moved. The transcript must be found wherever the
        // configured projects dir is.
        let cfg = tempfile::tempdir().unwrap();
        let projects = cfg.path().join("projects");
        let slug = projects.join("-Users-alex--fletch-worktrees-transylvania-fletch");
        std::fs::create_dir_all(&slug).unwrap();
        let sid = "f90f9c57-6dd1-45a0-9b69-5b5963979d5b";
        let jsonl = slug.join(format!("{sid}.jsonl"));
        std::fs::write(&jsonl, b"{}\n").unwrap();

        let found = find_session_jsonl_in(&[projects], sid, &mut ReadDiagnostics::default());
        assert_eq!(found.as_deref(), Some(jsonl.as_path()));
    }

    #[test]
    fn find_session_jsonl_skips_missing_dir_and_scans_the_next() {
        // A non-existent candidate dir (e.g. the default ~/.claude when only the
        // relocated config dir has the file) must not short-circuit the scan.
        let cfg = tempfile::tempdir().unwrap();
        let projects = cfg.path().join("projects");
        let slug = projects.join("slug");
        std::fs::create_dir_all(&slug).unwrap();
        let sid = "abc";
        let jsonl = slug.join(format!("{sid}.jsonl"));
        std::fs::write(&jsonl, b"{}\n").unwrap();

        let missing = cfg.path().join("does-not-exist");
        let found =
            find_session_jsonl_in(&[missing, projects], sid, &mut ReadDiagnostics::default());
        assert_eq!(found.as_deref(), Some(jsonl.as_path()));
    }

    #[test]
    fn find_session_jsonl_prefers_docker_per_agent_dir() {
        // A Docker-sandboxed agent writes its transcript to
        // `<writable_root>/<DOCKER_CLAUDE_PROJECTS_DIRNAME>/<slug>/<sid>.jsonl`,
        // where the agent's cwd is `<writable_root>/<repo>`. The locator derives
        // that dir from `cwd.parent()` and scans it first — no dependency on the
        // host `~/.claude`, which the container never wrote (the match early-
        // returns before any real-home candidate is read).
        let td = tempfile::tempdir().unwrap();
        let writable_root = td.path().join("orkney");
        let cwd = writable_root.join("repo");
        let slug = writable_root
            .join(DOCKER_CLAUDE_PROJECTS_DIRNAME)
            .join("-Users-u-orkney-repo");
        std::fs::create_dir_all(&slug).unwrap();
        let sid = "11111111-2222-3333-4444-555555555555";
        let jsonl = slug.join(format!("{sid}.jsonl"));
        std::fs::write(&jsonl, b"{}\n").unwrap();

        let found = find_session_jsonl(sid, &cwd, &mut ReadDiagnostics::default());
        assert_eq!(found.as_deref(), Some(jsonl.as_path()));
    }

    // ── read_jsonl_values: I/O failure accounting ──────────────────────────────

    #[test]
    fn read_jsonl_values_counts_read_failure_after_successful_open() {
        // A directory opens fine on Unix but the first read fails (EISDIR) —
        // the mid-stream `Err` arm must bump `io_errors` instead of the old
        // `map_while(Result::ok)` silently stopping.
        let td = tempfile::tempdir().unwrap();
        let mut diag = ReadDiagnostics::default();
        let out = read_jsonl_values(td.path(), &mut diag);
        assert!(out.is_empty());
        assert_eq!(diag.io_errors, 1);
        assert_eq!(diag.lines_seen, 0);
        assert_eq!(diag.records_parsed, 0);
    }
}

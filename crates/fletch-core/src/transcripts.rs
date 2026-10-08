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

/// The `projects` directory claude keeps its sessions in when it runs in
/// `cwd`: the per-agent dir a container sandbox mounts over it (`container`),
/// else the active default config dir, where every agent runs whatever its
/// account (a claude account is a token source only). The first place
/// [`find_session_jsonl`] looks for each kind of agent.
pub(crate) fn claude_projects_dir(cwd: &Path, container: bool) -> Option<PathBuf> {
    if container {
        return Some(cwd.parent()?.join(DOCKER_CLAUDE_PROJECTS_DIRNAME));
    }
    claude_projects_dirs().into_iter().next()
}

/// Move a session that only exists under a managed claude account's dir —
/// written before accounts became token sources, when claude ran with
/// `CLAUDE_CONFIG_DIR` there — into the default projects dir, transcript and
/// subagent dir both, so `--resume` (which now runs in the default dir) finds
/// it. `Ok(Some(new path))` when something moved. A session already in the
/// default dir (or a container's per-agent dir) is left alone, and nothing at
/// the destination is overwritten.
pub(crate) fn adopt_account_session(
    session_id: &str,
    cwd: &Path,
) -> std::io::Result<Option<PathBuf>> {
    let Some(default) = claude_projects_dir(cwd, false) else {
        return Ok(None);
    };
    let Some(found) = find_session_jsonl(session_id, cwd, &mut ReadDiagnostics::default()) else {
        return Ok(None);
    };
    let account_projects: Vec<PathBuf> = crate::agent::accounts::list_account_dirs("claude")
        .into_iter()
        .map(|dir| dir.join("projects"))
        .collect();
    adopt_session_from(&found, &account_projects, &default)
}

/// Pure-path core of [`adopt_account_session`]: `found` is where the locator
/// found the transcript, which lies under an account dir only when no copy
/// exists in the default or container dirs it searches first.
fn adopt_session_from(
    found: &Path,
    account_projects: &[PathBuf],
    default_projects: &Path,
) -> std::io::Result<Option<PathBuf>> {
    if !account_projects.iter().any(|p| found.starts_with(p)) {
        return Ok(None);
    }
    let (Some(slug_dir), Some(slug), Some(file), Some(stem)) = (
        found.parent(),
        found.parent().and_then(Path::file_name),
        found.file_name(),
        found.file_stem(),
    ) else {
        return Ok(None);
    };
    let target_dir = default_projects.join(slug);
    let target = target_dir.join(file);
    if target.exists() {
        return Ok(None);
    }
    std::fs::create_dir_all(&target_dir)?;
    std::fs::rename(found, &target)?;
    let subagents = slug_dir.join(stem);
    let subagents_target = target_dir.join(stem);
    if subagents.is_dir() && !subagents_target.exists() {
        std::fs::rename(&subagents, &subagents_target)?;
    }
    Ok(Some(target))
}

/// The directory under `projects` claude files the sessions of `cwd` in, as
/// claude names it (2.1.287): every character that isn't an ASCII letter or
/// digit becomes `-`. A name over 200 characters gets a hash suffix computed
/// in a way claude doesn't document, so such a path is refused rather than
/// guessed.
pub(crate) fn claude_project_dirname(cwd: &Path) -> Option<String> {
    let name: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    (name.len() <= 200).then_some(name)
}

/// Candidate `projects` directories Claude may have written transcripts to.
/// Honors `CLAUDE_CONFIG_DIR` (Claude CLI's own config-dir override) and always
/// also includes the default `~/.claude`, so a transcript is located regardless
/// of which config dir was active when the agent was spawned. Mirrors the way
/// `find_codex_rollouts` honors `CODEX_HOME` — without this, an agent spawned
/// with `CLAUDE_CONFIG_DIR` set wrote its transcript somewhere we never scanned,
/// so it was never ingested into `session_records` and was lost when that dir
/// moved.
///
/// Also the root list for the whole-disk usage scan (`usage_scan`), which walks
/// every `<projects dir>/*/*.jsonl` rather than one known session id.
///
/// Every managed account dir (`agent::accounts`) is a root too: agents
/// stamped with an account used to run claude with that dir as
/// `CLAUDE_CONFIG_DIR`, so their history lives there. New sessions of every
/// account land in the default dir.
pub(crate) fn claude_projects_dirs() -> Vec<PathBuf> {
    projects_dirs_from(
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        dirs::home_dir(),
        crate::agent::accounts::list_account_dirs("claude"),
    )
}

/// Pure core of [`claude_projects_dirs`]: configured dir first (if any), then
/// the default `~/.claude`, then each managed account dir, deduped so an
/// explicit `CLAUDE_CONFIG_DIR=~/.claude` (or an account dir) doesn't
/// double-scan.
fn projects_dirs_from(
    config_dir: Option<PathBuf>,
    home: Option<PathBuf>,
    account_dirs: Vec<PathBuf>,
) -> Vec<PathBuf> {
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
    for dir in account_dirs {
        push(dir);
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

/// Codex's default session root: `$CODEX_HOME/sessions` (CODEX_HOME defaults
/// to `~/.codex`), holding a `YYYY/MM/DD` tree of `rollout-<ts>-<id>.jsonl`.
/// `None` only when neither CODEX_HOME nor a home dir can be resolved. Where a
/// default-account session is written; [`codex_sessions_dirs`] is every root.
pub(crate) fn codex_sessions_dir() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
        .map(|home| home.join("sessions"))
}

/// The session roots codex wrote to before each agent ran in its own
/// `CODEX_HOME`: the default one, then each managed account's
/// (`agent::accounts`). Agents created since write only to their overlay
/// ([`codex_overlay_sessions_dirs`]); these still hold older threads.
pub(crate) fn legacy_codex_sessions_dirs() -> Vec<PathBuf> {
    sessions_dirs_from(
        codex_sessions_dir(),
        crate::agent::accounts::list_account_dirs("codex"),
    )
}

/// Pure core of [`legacy_codex_sessions_dirs`], deduped in first-seen order.
fn sessions_dirs_from(default: Option<PathBuf>, account_dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in default
        .into_iter()
        .chain(account_dirs.into_iter().map(|d| d.join("sessions")))
    {
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    out
}

/// Every agent's own codex session root, as `(agent id, sessions dir)`: the
/// `sessions` dir of each agent's `CODEX_HOME` overlay
/// (`agent::codex_login::overlay_for_agent`). Only overlays that exist.
pub(crate) fn codex_overlay_sessions_dirs() -> Vec<(String, PathBuf)> {
    crate::workspace::checkouts_root()
        .map(|root| overlay_sessions_in(&root))
        .unwrap_or_default()
}

/// Pure core of [`codex_overlay_sessions_dirs`] over a checkouts root, sorted
/// by agent id.
fn overlay_sessions_in(checkouts_root: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(checkouts_root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().into_string().ok()?;
            let sessions = crate::agent::codex_login::overlay_in(&entry.path()).join("sessions");
            sessions.is_dir().then_some((id, sessions))
        })
        .collect();
    out.sort();
    out
}

/// Every codex session root on this host: each agent's own, then the legacy
/// ones. The whole-disk usage scan's list.
pub(crate) fn codex_sessions_dirs() -> Vec<PathBuf> {
    codex_overlay_sessions_dirs()
        .into_iter()
        .map(|(_, dir)| dir)
        .chain(legacy_codex_sessions_dirs())
        .collect()
}

/// All of codex's rollout files for a thread id, ordered (filenames are
/// timestamp-prefixed, so lexical sort == chronological). Codex stores sessions
/// at `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`; the id suffix
/// is the thread id we captured. Resume normally keeps one file per session,
/// but returning all is correct if it splits.
///
/// A thread lives in agent `agent_id`'s overlay or, for a thread from before
/// overlays, in a legacy root. The first with a match wins: a legacy thread
/// copied into an overlay at its next launch
/// (`providers::codex::adopt_legacy_rollouts`) is read from the copy codex
/// appends to, never from both.
pub(crate) fn find_codex_rollouts(
    session_id: &str,
    agent_id: &str,
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    let own = crate::agent::codex_login::overlay_for_agent(agent_id)
        .map(|o| o.join("sessions"))
        .ok();
    let tiers = [
        own.into_iter().collect::<Vec<_>>(),
        legacy_codex_sessions_dirs(),
    ];
    find_codex_rollouts_tiered(session_id, &tiers, diag)
}

/// Pure core of [`find_codex_rollouts`]: the matches of the first tier of
/// roots that has any.
fn find_codex_rollouts_tiered(
    session_id: &str,
    tiers: &[Vec<PathBuf>],
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    for roots in tiers {
        let found: Vec<PathBuf> = roots
            .iter()
            .flat_map(|sessions| find_codex_rollouts_in(sessions, session_id, diag))
            .collect();
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

/// [`find_codex_rollouts`] under the `sessions` root given.
pub(crate) fn find_codex_rollouts_in(
    sessions: &Path,
    session_id: &str,
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    // Anchor on the `-<id>.jsonl` boundary (filenames are
    // `rollout-<ts>-<id>.jsonl`) so one thread id can't match another whose
    // name merely ends with the same characters.
    let suffix = format!("-{session_id}.jsonl");
    // Or-ed, not set: a scan over several roots has a live one if any is.
    diag.root_exists |= sessions.exists();
    let out: Vec<PathBuf> = codex_rollout_files(sessions)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&suffix))
        })
        .collect();
    diag.files_matched += out.len();
    out
}

/// Every file in a codex `sessions` root's `YYYY/MM/DD` tree (three dir
/// levels, nothing deeper), sorted — and since every path component is
/// zero-padded and the filenames timestamp-prefixed, path order is creation
/// order. Directory listing only; no file is opened.
pub(crate) fn codex_rollout_files(sessions: &Path) -> Vec<PathBuf> {
    fn dirs_in(p: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect()
    }
    let mut out = Vec::new();
    for year in dirs_in(sessions) {
        for month in dirs_in(&year) {
            for day in dirs_in(&month) {
                out.extend(
                    std::fs::read_dir(&day)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|e| e.path()),
                );
            }
        }
    }
    out.sort();
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
            Vec::new(),
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
            Vec::new(),
        );
        assert_eq!(dirs, vec![PathBuf::from("/home/u/.claude/projects")]);
    }

    #[test]
    fn projects_dirs_falls_back_to_home_when_config_unset() {
        let dirs = projects_dirs_from(None, Some(PathBuf::from("/home/u")), Vec::new());
        assert_eq!(dirs, vec![PathBuf::from("/home/u/.claude/projects")]);
    }

    #[test]
    fn projects_dirs_union_the_account_dirs_after_the_default() {
        let dirs = projects_dirs_from(
            Some(PathBuf::from("/home/u/.fletch/accounts/claude/work")),
            Some(PathBuf::from("/home/u")),
            vec![
                PathBuf::from("/home/u/.fletch/accounts/claude/personal"),
                PathBuf::from("/home/u/.fletch/accounts/claude/work"),
            ],
        );
        // The configured dir keeps its lead; an account it duplicates isn't
        // scanned twice.
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.fletch/accounts/claude/work/projects"),
                PathBuf::from("/home/u/.claude/projects"),
                PathBuf::from("/home/u/.fletch/accounts/claude/personal/projects"),
            ],
        );
    }

    #[test]
    fn codex_sessions_dirs_union_the_account_homes_after_the_default() {
        let dirs = sessions_dirs_from(
            Some(PathBuf::from("/home/u/.codex/sessions")),
            vec![
                PathBuf::from("/a/codex/work"),
                PathBuf::from("/a/codex/work"),
            ],
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.codex/sessions"),
                PathBuf::from("/a/codex/work/sessions"),
            ],
        );
        assert_eq!(
            sessions_dirs_from(None, vec![PathBuf::from("/a/codex/work")]),
            vec![PathBuf::from("/a/codex/work/sessions")],
        );
    }

    fn legacy_session(td: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let account = td.join("accounts/claude/work/projects");
        let default = td.join("home/.claude/projects");
        let slug = account.join("-repo");
        std::fs::create_dir_all(slug.join("sid/subagents")).unwrap();
        std::fs::write(slug.join("sid.jsonl"), "{}\n").unwrap();
        std::fs::write(slug.join("sid/subagents/agent-1.jsonl"), "{}\n").unwrap();
        (account, default, slug.join("sid.jsonl"))
    }

    #[test]
    fn a_session_left_in_an_account_dir_moves_to_the_default_with_its_subagents() {
        let td = tempfile::tempdir().unwrap();
        let (account, default, found) = legacy_session(td.path());
        let moved = adopt_session_from(&found, &[account], &default).unwrap();
        assert_eq!(moved, Some(default.join("-repo/sid.jsonl")));
        assert!(default.join("-repo/sid/subagents/agent-1.jsonl").is_file());
        assert!(!found.exists());
    }

    #[test]
    fn a_session_outside_the_account_dirs_stays_put() {
        let td = tempfile::tempdir().unwrap();
        let (_account, default, found) = legacy_session(td.path());
        let elsewhere = td.path().join("other/projects");
        assert_eq!(
            adopt_session_from(&found, &[elsewhere], &default).unwrap(),
            None
        );
        assert!(found.exists());
    }

    #[test]
    fn adopting_never_overwrites_a_session_already_in_the_default_dir() {
        let td = tempfile::tempdir().unwrap();
        let (account, default, found) = legacy_session(td.path());
        std::fs::create_dir_all(default.join("-repo")).unwrap();
        std::fs::write(default.join("-repo/sid.jsonl"), "kept\n").unwrap();
        assert_eq!(
            adopt_session_from(&found, &[account], &default).unwrap(),
            None
        );
        let kept = std::fs::read_to_string(default.join("-repo/sid.jsonl")).unwrap();
        assert_eq!(kept, "kept\n");
        assert!(found.exists());
    }

    #[test]
    fn overlay_session_dirs_are_listed_per_agent_and_only_when_present() {
        let td = tempfile::tempdir().unwrap();
        let overlay = |id: &str| {
            td.path()
                .join(id)
                .join(crate::agent::codex_login::OVERLAY_DIRNAME)
        };
        std::fs::create_dir_all(overlay("fuji").join("sessions")).unwrap();
        std::fs::create_dir_all(overlay("etna").join("sessions")).unwrap();
        std::fs::create_dir_all(td.path().join("rainier").join("repo")).unwrap();

        assert_eq!(
            overlay_sessions_in(td.path()),
            vec![
                ("etna".to_string(), overlay("etna").join("sessions")),
                ("fuji".to_string(), overlay("fuji").join("sessions")),
            ]
        );
    }

    fn rollout_in(sessions: &Path, thread: &str) -> PathBuf {
        let day = sessions.join("2026").join("10").join("08");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join(format!("rollout-2026-10-08T09-00-00-{thread}.jsonl"));
        std::fs::write(&path, b"{}\n").unwrap();
        path
    }

    #[test]
    fn an_agents_own_overlay_wins_over_a_legacy_copy_of_the_thread() {
        let td = tempfile::tempdir().unwrap();
        let (own, legacy) = (td.path().join("own"), td.path().join("legacy"));
        let thread = "019a-thread";
        let copy = rollout_in(&own, thread);
        rollout_in(&legacy, thread);
        let mut diag = ReadDiagnostics::default();

        let found = find_codex_rollouts_tiered(thread, &[vec![own], vec![legacy]], &mut diag);

        assert_eq!(found, vec![copy]);
    }

    #[test]
    fn a_thread_only_in_a_legacy_root_is_still_found() {
        let td = tempfile::tempdir().unwrap();
        let (own, legacy) = (td.path().join("own"), td.path().join("legacy"));
        std::fs::create_dir_all(&own).unwrap();
        let thread = "019a-old-thread";
        let old = rollout_in(&legacy, thread);
        let mut diag = ReadDiagnostics::default();

        let found = find_codex_rollouts_tiered(thread, &[vec![own], vec![legacy]], &mut diag);

        assert_eq!(found, vec![old]);
        assert!(diag.root_exists);
    }

    /// End to end over a real accounts root: a transcript written under a
    /// managed account is found by the same lookups History and the reader
    /// use, and the account roots join the usage scan's lists.
    #[test]
    fn transcripts_under_a_managed_account_are_found() {
        crate::agent::accounts::with_test_root(|root| {
            let claude = root.join("claude").join("work");
            let slug = claude.join("projects").join("-repo");
            std::fs::create_dir_all(&slug).unwrap();
            let sid = "0f0e0d0c-aaaa-bbbb-cccc-121212121212";
            let jsonl = slug.join(format!("{sid}.jsonl"));
            std::fs::write(&jsonl, b"{}\n").unwrap();
            assert!(claude_projects_dirs().contains(&claude.join("projects")));
            let found = find_session_jsonl(
                sid,
                Path::new("/nonexistent/agent/repo"),
                &mut ReadDiagnostics::default(),
            );
            assert_eq!(found.as_deref(), Some(jsonl.as_path()));
            assert_ne!(
                claude_projects_dir(Path::new("/w/a/repo"), false),
                Some(claude.join("projects")),
                "new sessions land in the default dir, not the account's"
            );

            let codex = root.join("codex").join("work");
            let day = codex.join("sessions").join("2026").join("10").join("07");
            std::fs::create_dir_all(&day).unwrap();
            let thread = "019a-account-thread";
            let rollout = day.join(format!("rollout-2026-10-07T10-00-00-{thread}.jsonl"));
            std::fs::write(&rollout, b"{}\n").unwrap();
            assert!(codex_sessions_dirs().contains(&codex.join("sessions")));
            let mut diag = ReadDiagnostics::default();
            assert_eq!(
                find_codex_rollouts(thread, "no-such-agent", &mut diag),
                vec![rollout]
            );
            assert!(diag.root_exists);
        });
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

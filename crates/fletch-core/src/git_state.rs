//! Read-only git state queries for a checkout.
//!
//! Companion to `git.rs` which handles mutations. This module only reads.

use std::collections::HashMap;
use std::path::Path;

use crate::error::Result;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct GitState {
    pub branch: String,
    pub parent_branch: String,
    pub ahead: u32,
    pub behind: u32,
    /// Commits a push would actually send: those on HEAD not yet on the
    /// branch's upstream, or — with no upstream configured (a detached agent
    /// clone, a branch never pushed) — those no `origin` branch contains.
    /// Distinct from `ahead`, which is measured against the base branch and so
    /// counts commits `origin` may already have.
    pub unpushed: u32,
    pub files: Vec<FileStatus>,
    pub additions: u32,
    pub deletions: u32,
    /// GitHub web base for the `origin` remote (`https://github.com/owner/repo`),
    /// or `None` when origin is missing or isn't a github.com remote. Lets the
    /// UI link out to a commit / compare view. Stable across a branch's life.
    pub remote_url: Option<String>,
    /// Whether an `origin` remote exists at all (GitHub or not). `false` means
    /// a local-only repo — push/PR affordances are replaced by "publish".
    pub has_origin: bool,
    /// HEAD commit SHA, used to build a single-commit link when exactly one
    /// commit is ahead. `None` on an empty repo / detached read failure.
    pub head_sha: Option<String>,
}

/// Compact projection of GitState used by the app-wide bulk poll —
/// enough to render per-agent shortstats and the right-rail tab badge
/// without shipping every agent's full file list over the IPC channel.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShortStats {
    pub additions: u32,
    pub deletions: u32,
    pub file_count: u32,
}

/// Advisory fleet-wide git metadata for one checkout: how far its base has
/// moved ahead of it (staleness) and which files it touches (cross-agent
/// overlap hints). Distinct from `ShortStats` — which is deliberately just the
/// three badge numbers — so the always-current 5s shortstats path stays a flat
/// number-only payload while this slower, advisory signal evolves on its own.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GitMeta {
    /// The base branch this checkout is measured against (its parent branch).
    pub base: String,
    /// Commits the base has moved ahead of this checkout's HEAD, or `None` when
    /// the base tip can't be resolved (no origin ref / never fetched). `None`
    /// and `Some(0)` both render no staleness chip — a moved base is only shown
    /// when it's genuinely ahead.
    pub behind: Option<u32>,
    /// Working-tree file paths (same set `git status` reports), for the
    /// frontend's pairwise overlap selector. Empty when the tree is clean or
    /// unreadable.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FileStatus {
    pub path: String,
    pub kind: StatusKind,
    pub staged: bool,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Query the read-only git state for a checkout, measured against an
/// already-resolved base (`git::ResolvedBase` — see `TrackedRepo::resolve_base`).
pub async fn query(checkout_path: &Path, base: &crate::git::ResolvedBase) -> Result<GitState> {
    // These reads build commands directly rather than through `git::cmd`, so they
    // don't inherit its config guard — `git diff`/`git status` here would run a
    // planted textconv or clean filter. Guarded at the module's public surface, so
    // the internal helpers below stay plain.
    crate::git::hardening::refuse_steerable_config(checkout_path).await?;
    // 1. Branch name
    let branch = match crate::git::current_branch(checkout_path).await {
        Ok(Some(b)) => b,
        Ok(None) => String::new(),
        // Empty repo / no HEAD — return a zero-state
        Err(_) => {
            return Ok(GitState {
                branch: String::new(),
                parent_branch: base.name.clone(),
                ahead: 0,
                behind: 0,
                unpushed: 0,
                files: vec![],
                additions: 0,
                deletions: 0,
                remote_url: None,
                has_origin: false,
                head_sha: None,
            });
        }
    };

    // 2. Ahead / behind (vs the base's tip), and unpushed (vs origin). An
    // unresolvable tip counts nothing rather than guessing at a ref: the panel
    // already renders 0/0 as "nothing to show".
    let (ahead, behind) = match &base.tip {
        Some(tip) => rev_list_counts(checkout_path, tip).await.unwrap_or((0, 0)),
        None => (0, 0),
    };
    let unpushed = query_unpushed(checkout_path).await;

    // 3. File list from `git status --porcelain=v1`
    let mut files = match run_status(checkout_path).await {
        Ok(output) => parse_porcelain(&output),
        Err(_) => vec![],
    };

    // 4. Per-file diff stats from `git diff --numstat HEAD`
    let numstat = match run_numstat(checkout_path).await {
        Ok(output) => parse_numstat(&output),
        Err(_) => HashMap::new(),
    };

    // Merge numstat into file list. `git diff --numstat HEAD` only covers
    // tracked files, so untracked (agent-created, never-added) ones fall back
    // to a direct line count of their on-disk contents — under the same read
    // budget the bulk poll uses (MAX_UNTRACKED_READ_BYTES). This path needs the
    // bound at least as much: the focused Git panel polls it at 1 Hz, five times
    // the shortstats cadence.
    let mut untracked_at: Vec<usize> = Vec::new();
    for (i, f) in files.iter_mut().enumerate() {
        if let Some(&(adds, dels)) = numstat.get(&f.path) {
            f.additions = adds;
            f.deletions = dels;
        } else if matches!(f.kind, StatusKind::Untracked) {
            untracked_at.push(i);
        }
    }
    if !untracked_at.is_empty() {
        let rels: Vec<String> = untracked_at
            .iter()
            .map(|&i| files[i].path.clone())
            .collect();
        let (counts, _truncated) = untracked_additions(checkout_path, rels).await;
        for (&i, adds) in untracked_at.iter().zip(counts) {
            files[i].additions = adds;
        }
    }

    let additions: u32 = files.iter().map(|f| f.additions).sum();
    let deletions: u32 = files.iter().map(|f| f.deletions).sum();

    // 5. Link targets — the origin web base (for commit / compare links) and the
    //    HEAD sha (for a single-commit link). Both are cheap reads; failures are
    //    non-fatal (the UI just omits the link).
    let (has_origin, remote_url) = query_origin(checkout_path).await;
    let head_sha = query_head_sha(checkout_path).await;

    Ok(GitState {
        branch,
        parent_branch: base.name.clone(),
        ahead,
        behind,
        unpushed,
        files,
        additions,
        deletions,
        remote_url,
        has_origin,
        head_sha,
    })
}

/// Budget for how much untracked file *content* one git-state pass will read to
/// count additions.
///
/// Untracked lines can only be counted by reading the file — `git diff
/// --numstat` doesn't see them — which makes this the one part of the git-state
/// path whose cost scales with the *tree* rather than with the diff. Unbounded,
/// a checkout holding a large untracked directory (a package-manager cache that
/// landed inside it) turns every tick into a full-tree read: one real workspace
/// sat at 7.5k untracked files / 57 MB, re-read every 5s per agent.
///
/// Budgeting **bytes rather than file count** is what keeps the numbers honest,
/// and is the whole reason this isn't a simple `.take(n)`. A legitimate large
/// change is many *small* files — a framework scaffold, a codegen run, a
/// vendored client — and 8 MB counts thousands of them exactly. The pathological
/// case is bulk *content*, which exhausts the budget almost immediately and
/// stops. A flat file-count cap can't tell those apart: it would have truncated
/// a 900-file scaffold (fully countable in well under a megabyte) exactly as
/// eagerly as a package cache, reporting a number that silently disagreed with
/// the file count beside it.
///
/// When the budget does bind, `additions` is a floor. The file *count* stays
/// exact either way — it comes from `git status`, not from these reads.
const MAX_UNTRACKED_READ_BYTES: u64 = 8 * 1024 * 1024;

/// Slim projection for the app-wide bulk poll. Where `query` spawns ~7 git
/// subprocesses to assemble a full `GitState`, this reads only what the
/// shortstats badge needs: `git status` for the file count and `git diff
/// --numstat` for the line totals. The two run concurrently, so latency is a
/// single git invocation. Failures degrade to zeroes rather than dropping the
/// agent, matching the badge's "no news is zero" contract.
pub async fn shortstats(checkout_path: &Path) -> ShortStats {
    // `git status`/`--numstat` run clean filters and textconv, so a poisoned
    // checkout reports zeroes rather than executing them (see `query`).
    if !crate::git::hardening::config_is_safe(checkout_path).await {
        return ShortStats {
            additions: 0,
            deletions: 0,
            file_count: 0,
        };
    }
    let (status, numstat) = tokio::join!(run_status(checkout_path), run_numstat(checkout_path));
    let files = status.map(|o| parse_porcelain(&o)).unwrap_or_default();
    let file_count = files.len() as u32;
    let (mut additions, deletions) = numstat
        .map(|o| {
            parse_numstat(&o)
                .values()
                .fold((0, 0), |(a, d), &(adds, dels)| (a + adds, d + dels))
        })
        .unwrap_or((0, 0));
    // Numstat misses untracked files (see `query`); count their lines too,
    // under the shared read budget so a pathological tree can't turn this poll
    // into a full-tree read. See MAX_UNTRACKED_READ_BYTES.
    let rels: Vec<String> = files
        .iter()
        .filter(|f| matches!(f.kind, StatusKind::Untracked))
        .map(|f| f.path.clone())
        .collect();
    if !rels.is_empty() {
        let (counts, truncated) = untracked_additions(checkout_path, rels).await;
        additions += counts.iter().sum::<u32>();
        if truncated {
            tracing::debug!(
                checkout = %checkout_path.display(),
                budget_bytes = MAX_UNTRACKED_READ_BYTES,
                "shortstats: untracked content exceeded the read budget; additions are a floor"
            );
        }
    }
    ShortStats {
        additions,
        deletions,
        file_count,
    }
}

/// Ceiling on how many working-tree paths `git_meta` ships per checkout.
///
/// These paths serve one advisory consumer — the frontend's pairwise overlap
/// hint, which builds a `Set` per checkout and intersects every pair sharing a
/// source repo. So an unbounded list costs twice: the IPC payload, and an
/// O(n·m) pass across the fleet, both scaling with whatever happens to be
/// untracked rather than with what the agent actually changed. Truncating can
/// only cost a hint that was never promised to be exhaustive, and a real
/// changed set is orders of magnitude under this.
const MAX_META_FILES: usize = 500;

/// Base-staleness + changed-file paths for one checkout — the advisory bulk
/// poll behind the "base moved" chips and cross-agent overlap hints.
///
/// `base.tip` is the freshest base commit this checkout can actually read (see
/// `git::resolve_base`): the SOURCE repo's `refs/remotes/origin/<base>` when the
/// clone shares its objects — one background fetch there makes a moved base
/// measurable from every checkout, with no per-clone network round-trip — else
/// the clone's own refs. `None` → the base couldn't be resolved (no origin / no
/// fetch yet), so staleness degrades to unknown rather than a fake zero. File
/// paths always come straight from the checkout's `git status`.
pub async fn git_meta(checkout_path: &Path, base: &crate::git::ResolvedBase) -> GitMeta {
    // Same reason as `shortstats`. `files` is already documented as empty when the
    // tree is unreadable, so this needs no new state to express itself.
    if !crate::git::hardening::config_is_safe(checkout_path).await {
        return GitMeta {
            base: base.name.clone(),
            behind: None,
            files: Vec::new(),
        };
    }
    let files = run_status(checkout_path)
        .await
        .map(|o| parse_porcelain(&o))
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.path)
        .take(MAX_META_FILES)
        .collect();
    let behind = match &base.tip {
        Some(tip) => rev_list_count(checkout_path, &[&format!("HEAD..{tip}")]).await,
        None => None,
    };
    GitMeta {
        base: base.name.clone(),
        behind,
        files,
    }
}

/// `git rev-list --count <args>`, or `None` when the revisions don't resolve
/// (e.g. an object the shared store doesn't hold, or a branch with no
/// upstream). One number — unlike `rev_list_counts`, which reads both sides of
/// a symmetric range.
async fn rev_list_count(checkout_path: &Path, args: &[&str]) -> Option<u32> {
    let out = read_command(checkout_path)
        .args(["rev-list", "--count"])
        .args(args)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Additions for a batch of untracked files (paths relative to `checkout_path`):
/// each one's on-disk line count, i.e. what `git diff --numstat` would report
/// once it was added. Returns counts parallel to `rels` — 0 for anything binary,
/// oversized, unreadable, or past the budget — plus whether the budget cut the
/// pass short.
///
/// Reads in the given (git status) order until [`MAX_UNTRACKED_READ_BYTES`] of
/// content has been consumed, so the numbers are exact for any realistic change
/// set and bounded for a pathological tree.
///
/// Batched and sequential on one blocking thread, rather than a task per file as
/// this used to be. Three reasons: the budget already caps the work at a few MB,
/// which reads faster straight through than fanned out over thousands of spawned
/// tasks; a shared budget across racing tasks would make the total depend on
/// completion order, so the badge would flicker between ticks; and a per-file
/// fan-out was itself unbounded — 7.5k concurrent reads in the case that
/// prompted all this.
async fn untracked_additions(checkout_path: &Path, rels: Vec<String>) -> (Vec<u32>, bool) {
    /// Beyond this a single file isn't a plausible text diff; numstat reports
    /// `-` for those, so 0 matches it. Checked from metadata, so an oversized
    /// file costs a stat, never a read, and never touches the budget.
    const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

    let root = checkout_path.to_path_buf();
    let empty = rels.len();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;

        let mut out = vec![0u32; rels.len()];
        let mut spent: u64 = 0;
        let mut buf: Vec<u8> = Vec::new();
        for (i, rel) in rels.iter().enumerate() {
            let remaining = MAX_UNTRACKED_READ_BYTES.saturating_sub(spent);
            if remaining == 0 {
                return (out, true);
            }
            let abs = root.join(rel);
            // Cheap pre-filter: skip an oversized file (or a non-file) without
            // opening it. This is an optimization, not the bound — the read
            // below enforces that, because the file can change underneath us.
            let Ok(meta) = std::fs::metadata(&abs) else {
                continue;
            };
            if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
                continue;
            }
            // Read through a hard ceiling rather than trusting the length we
            // just stat'd. The agent is actively working in this tree, so a
            // file can grow or be replaced between the `metadata` and the read;
            // `fs::read` pre-allocates from the *current* size and would then
            // consume the whole replacement, overshooting both this file's
            // limit and the pass budget. `take` caps the allocation at what we
            // are willing to read no matter what happened in between.
            let ceiling = MAX_FILE_BYTES.min(remaining);
            let Ok(file) = std::fs::File::open(&abs) else {
                continue;
            };
            buf.clear();
            // +1 so a file sitting exactly at the ceiling is distinguishable
            // from one that overruns it.
            if file.take(ceiling + 1).read_to_end(&mut buf).is_err() {
                continue;
            }
            // Charge what was actually read, not what was stat'd.
            spent += buf.len() as u64;
            if buf.len() as u64 > ceiling {
                // Either the file outgrew MAX_FILE_BYTES mid-read, or the
                // budget ran out inside it. Counting a truncated prefix would
                // report a wrong line count, so contribute nothing. Only a
                // budget overrun ends the pass; an oversized file is skipped
                // the way numstat's `-` marker is.
                if ceiling == remaining {
                    return (out, true);
                }
                continue;
            }
            if buf.is_empty() || buf.contains(&0) {
                continue;
            }
            let newlines = buf.iter().filter(|&&b| b == b'\n').count() as u32;
            // A final line without a trailing newline still counts as a line.
            out[i] = if buf.ends_with(b"\n") {
                newlines
            } else {
                newlines + 1
            };
        }
        (out, false)
    })
    .await
    .unwrap_or_else(|_| (vec![0; empty], false))
}

/// Build a git command for this module's state queries with `GIT_OPTIONAL_LOCKS`
/// disabled. Every invocation in `git_state` is read-only, but `git status` /
/// `git diff HEAD` still take `.git/index.lock` for an *opportunistic* stat-cache
/// refresh — which races a concurrent Fletch write on the same checkout (e.g. a
/// fork's carry-code `reset`, or a user git-action) and fails with "Unable to
/// create '.git/index.lock': File exists". `GIT_OPTIONAL_LOCKS=0` tells git to
/// skip that refresh (the same knob editors/IDEs use); the only cost is a
/// slightly staler stat cache, rebuilt by the next writing command.
fn read_command(checkout_path: &Path) -> tokio::process::Command {
    let mut cmd = crate::git_dist::command(checkout_path);
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    cmd
}

/// HEAD commit SHA, or `None` when it can't be read.
async fn query_head_sha(checkout_path: &Path) -> Option<String> {
    let out = read_command(checkout_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// The `origin` remote: whether one exists at all, and its GitHub web base
/// (`None` when missing or not a github.com remote).
async fn query_origin(checkout_path: &Path) -> (bool, Option<String>) {
    let out = match read_command(checkout_path)
        .args(["remote", "get-url", "origin"])
        .output()
        .await
    {
        Ok(out) if out.status.success() => out,
        _ => return (false, None),
    };
    (
        true,
        github_web_url(String::from_utf8_lossy(&out.stdout).trim()),
    )
}

/// Normalize a git remote URL to its GitHub web base (`https://github.com/
/// owner/repo`). Handles `https://`, `http://`, `git://`, `ssh://git@`, and
/// `git@github.com:` forms, with optional `.git` suffix / trailing slash.
/// Returns `None` for non-github.com remotes or anything not of the shape
/// `owner/repo`, so callers only ever build valid GitHub links.
pub(crate) fn github_web_url(remote: &str) -> Option<String> {
    let r = remote.trim();
    let owner_repo = r
        .strip_prefix("git@github.com:")
        .or_else(|| r.strip_prefix("ssh://git@github.com/"))
        .or_else(|| r.strip_prefix("https://github.com/"))
        .or_else(|| r.strip_prefix("http://github.com/"))
        .or_else(|| r.strip_prefix("git://github.com/"))?;
    let owner_repo = owner_repo.trim_end_matches('/');
    let owner_repo = owner_repo.strip_suffix(".git").unwrap_or(owner_repo);
    let owner_repo = owner_repo.trim_end_matches('/');
    let mut parts = owner_repo.split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().filter(|s| !s.is_empty())?;
    // Reject anything deeper than owner/repo.
    if parts.next().is_some() {
        return None;
    }
    Some(format!("https://github.com/{owner}/{repo}"))
}

// ---------------------------------------------------------------------------
// Private helpers — subprocess runners
// ---------------------------------------------------------------------------

/// Count the commits a push would send. With an upstream configured that's
/// `@{upstream}..HEAD`. Without one — an agent workspace is a detached clone,
/// and a branch that was never pushed has no upstream either — it's every
/// commit no `origin` branch already contains, which is what a push would have
/// to carry. That's 0 for a fresh clone sitting on `origin/main`, where the
/// base-relative `ahead` count can be stale and non-zero.
async fn query_unpushed(checkout_path: &Path) -> u32 {
    if let Some(n) = rev_list_count(checkout_path, &["@{upstream}..HEAD"]).await {
        return n;
    }
    rev_list_count(checkout_path, &["HEAD", "--not", "--remotes=origin"])
        .await
        .unwrap_or(0)
}

/// `git rev-list --left-right --count HEAD...<base>`, or `None` when the base
/// commit isn't in this checkout's object store.
async fn rev_list_counts(checkout_path: &Path, base: &str) -> Option<(u32, u32)> {
    let out = read_command(checkout_path)
        .args([
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{base}"),
        ])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    Some(parse_ahead_behind(s.trim()))
}

async fn run_status(checkout_path: &Path) -> Result<String> {
    let out = read_command(checkout_path)
        // `-uall` lists each file inside an untracked directory individually
        // (the default collapses them into one `dir/` entry, which can't be
        // diffed or counted per-file).
        .args(["status", "--porcelain=v1", "-uall"])
        .output()
        .await?;
    if !out.status.success() {
        tracing::warn!(stderr = %String::from_utf8_lossy(&out.stderr).trim(), "git status --porcelain=v1 failed");
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn run_numstat(checkout_path: &Path) -> Result<String> {
    let out = read_command(checkout_path)
        .args(["diff", "--numstat", "HEAD"])
        .output()
        .await?;
    if !out.status.success() {
        tracing::warn!(stderr = %String::from_utf8_lossy(&out.stderr).trim(), "git diff --numstat failed");
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// Private helpers — pure parsers (also visible to tests)
// ---------------------------------------------------------------------------

fn parse_ahead_behind(s: &str) -> (u32, u32) {
    let mut parts = s.splitn(2, '\t');
    let ahead: u32 = parts
        .next()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0);
    let behind: u32 = parts
        .next()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0);
    (ahead, behind)
}

fn parse_porcelain(output: &str) -> Vec<FileStatus> {
    let mut files = Vec::new();
    for line in output.lines() {
        // Each line is at least 4 chars: `XY PATH`
        // `XY ` is the 3-char prefix, followed by the path.
        if line.len() < 4 {
            continue;
        }
        let x = line.chars().next().unwrap_or(' ');
        let y = line.chars().nth(1).unwrap_or(' ');
        // Space between status and path
        let path_part = &line[3..];

        let kind = status_kind(x, y);
        let staged = x != ' ' && x != '?';

        // For renamed files, git porcelain v1 formats the path as
        // `old_name -> new_name` (arrow) or `old_name\tnew_name` (tab).
        // We want the new name (destination), which is the part after the separator.
        let raw = if matches!(kind, StatusKind::Renamed) {
            // Try tab separator first, then " -> "
            if let Some(pos) = path_part.find('\t') {
                &path_part[pos + 1..]
            } else if let Some(pos) = path_part.find(" -> ") {
                &path_part[pos + 4..]
            } else {
                path_part
            }
        } else {
            path_part
        };
        // Paths with spaces / non-ASCII come back C-quoted (e.g. `"a b.rs"`);
        // decode them back to the real on-disk path. Without this, the quotes
        // leak into the tree and break path-based operations like delete.
        let path = unquote_path(raw);

        files.push(FileStatus {
            path,
            kind,
            staged,
            additions: 0,
            deletions: 0,
        });
    }
    files
}

/// Decode a path as printed by `git status --porcelain` (without `-z`). Git
/// wraps paths containing spaces, quotes, or non-ASCII bytes in double quotes
/// and C-style escapes them (`\"`, `\\`, `\t`, octal `\NNN`, …). Plain,
/// unquoted paths pass through unchanged.
fn unquote_path(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'"' || bytes[bytes.len() - 1] != b'"' {
        return s.to_string();
    }
    let inner = &bytes[1..bytes.len() - 1];
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        if inner[i] == b'\\' && i + 1 < inner.len() {
            match inner[i + 1] {
                b'a' => {
                    out.push(0x07);
                    i += 2;
                }
                b'b' => {
                    out.push(0x08);
                    i += 2;
                }
                b't' => {
                    out.push(b'\t');
                    i += 2;
                }
                b'n' => {
                    out.push(b'\n');
                    i += 2;
                }
                b'v' => {
                    out.push(0x0b);
                    i += 2;
                }
                b'f' => {
                    out.push(0x0c);
                    i += 2;
                }
                b'r' => {
                    out.push(b'\r');
                    i += 2;
                }
                b'"' => {
                    out.push(b'"');
                    i += 2;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 2;
                }
                d @ b'0'..=b'7' => {
                    // Up to three octal digits encode one byte (UTF-8 sequences
                    // arrive as several such escapes).
                    let mut val = (d - b'0') as u32;
                    let mut j = i + 2;
                    let mut n = 1;
                    while j < inner.len() && n < 3 && inner[j].is_ascii_digit() && inner[j] < b'8' {
                        val = val * 8 + (inner[j] - b'0') as u32;
                        j += 1;
                        n += 1;
                    }
                    out.push(val as u8);
                    i = j;
                }
                _ => {
                    out.push(inner[i]);
                    i += 1;
                }
            }
        } else {
            out.push(inner[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn status_kind(x: char, y: char) -> StatusKind {
    // Conflict codes: UU, AA, DD, AU, UA, DU, UD
    if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
        return StatusKind::Conflicted;
    }
    // Untracked
    if x == '?' && y == '?' {
        return StatusKind::Untracked;
    }
    // Use whichever of X/Y is not space or '?'
    let effective = if x != ' ' && x != '?' { x } else { y };
    match effective {
        'M' => StatusKind::Modified,
        'A' => StatusKind::Added,
        'D' => StatusKind::Deleted,
        'R' => StatusKind::Renamed,
        _ => StatusKind::Modified,
    }
}

fn parse_numstat(output: &str) -> HashMap<String, (u32, u32)> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let mut parts = line.splitn(3, '\t');
        let adds_str = parts.next().unwrap_or("-");
        let dels_str = parts.next().unwrap_or("-");
        let path = match parts.next() {
            Some(p) => p.to_string(),
            None => continue,
        };
        // For renames, numstat emits `OLD => NEW` as the path field.
        // Index by the new name so lookups by current filename succeed.
        let path = if let Some(pos) = path.find(" => ") {
            &path[pos + 4..]
        } else {
            &path
        };
        // numstat quotes non-ASCII paths just like `git status`; decode so the
        // key matches the (unquoted) path in the file list it's merged into.
        let path = unquote_path(path);
        // Binary files show `-\t-\t<path>`
        let adds: u32 = adds_str.parse().unwrap_or(0);
        let dels: u32 = dels_str.parse().unwrap_or(0);
        map.insert(path, (adds, dels));
    }
    map
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- untracked_additions (read budget) ---

    /// The property the byte budget exists to protect: a large *count* of small
    /// files — a scaffold, a codegen run — is counted exactly. The earlier
    /// file-count cap failed here, truncating a perfectly cheap change set and
    /// reporting additions that disagreed with the file count beside them.
    #[tokio::test]
    async fn many_small_untracked_files_are_counted_exactly() {
        let td = tempfile::tempdir().unwrap();
        let rels: Vec<String> = (0..900)
            .map(|i| {
                let rel = format!("gen/file{i}.ts");
                let abs = td.path().join(&rel);
                std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
                // Three lines each, well under the budget in aggregate.
                std::fs::write(&abs, "a\nb\nc\n").unwrap();
                rel
            })
            .collect();

        let (counts, truncated) = untracked_additions(td.path(), rels).await;
        assert!(
            !truncated,
            "900 tiny files must not exhaust the read budget"
        );
        assert_eq!(counts.len(), 900);
        assert_eq!(counts.iter().sum::<u32>(), 2_700);
    }

    /// The other half: bulk *content* is bounded. Reads stop once the budget is
    /// spent, the pass reports itself truncated, and files already read still
    /// carry their real counts (a floor, never a wrong total).
    #[tokio::test]
    async fn bulk_untracked_content_stops_at_the_read_budget() {
        let td = tempfile::tempdir().unwrap();
        // 3 MB each: individually under MAX_FILE_BYTES, collectively over the
        // 8 MB budget on the third.
        let line = "x".repeat(1023);
        let body: String = std::iter::repeat_n(line.as_str(), 3 * 1024)
            .map(|l| format!("{l}\n"))
            .collect();
        let rels: Vec<String> = (0..4)
            .map(|i| {
                let rel = format!("blob{i}.bin");
                std::fs::write(td.path().join(&rel), &body).unwrap();
                rel
            })
            .collect();

        let (counts, truncated) = untracked_additions(td.path(), rels).await;
        assert!(truncated, "9+ MB of content must exhaust the 8 MB budget");
        // Two whole files fit (6 MB); the third would cross the budget.
        assert_eq!(counts.iter().filter(|&&c| c > 0).count(), 2);
        assert_eq!(counts[0], 3 * 1024, "a file that was read is counted fully");
        assert_eq!(counts[3], 0, "a file past the budget contributes nothing");
    }

    // --- github_web_url ---

    #[test]
    fn web_url_https() {
        assert_eq!(
            github_web_url("https://github.com/octocat/Hello-World"),
            Some("https://github.com/octocat/Hello-World".into())
        );
    }

    #[test]
    fn web_url_https_dot_git() {
        assert_eq!(
            github_web_url("https://github.com/octocat/Hello-World.git"),
            Some("https://github.com/octocat/Hello-World".into())
        );
    }

    #[test]
    fn web_url_https_trailing_slash() {
        assert_eq!(
            github_web_url("https://github.com/octocat/Hello-World.git/"),
            Some("https://github.com/octocat/Hello-World".into())
        );
    }

    #[test]
    fn web_url_ssh_scp_form() {
        assert_eq!(
            github_web_url("git@github.com:octocat/Hello-World.git"),
            Some("https://github.com/octocat/Hello-World".into())
        );
    }

    #[test]
    fn web_url_ssh_scheme_form() {
        assert_eq!(
            github_web_url("ssh://git@github.com/octocat/Hello-World.git"),
            Some("https://github.com/octocat/Hello-World".into())
        );
    }

    #[test]
    fn web_url_non_github_is_none() {
        assert_eq!(
            github_web_url("git@gitlab.com:octocat/Hello-World.git"),
            None
        );
        assert_eq!(
            github_web_url("https://bitbucket.org/octocat/Hello-World"),
            None
        );
    }

    #[test]
    fn web_url_malformed_is_none() {
        assert_eq!(github_web_url("https://github.com/octocat"), None); // no repo
        assert_eq!(github_web_url("https://github.com/a/b/c"), None); // too deep
        assert_eq!(github_web_url("not a url"), None);
    }

    // --- parse_ahead_behind ---

    #[test]
    fn ahead_behind_typical() {
        assert_eq!(parse_ahead_behind("3\t5"), (3, 5));
    }

    #[test]
    fn ahead_behind_zero() {
        assert_eq!(parse_ahead_behind("0\t0"), (0, 0));
    }

    #[test]
    fn ahead_behind_only_ahead() {
        assert_eq!(parse_ahead_behind("4\t0"), (4, 0));
    }

    #[test]
    fn ahead_behind_empty() {
        assert_eq!(parse_ahead_behind(""), (0, 0));
    }

    #[test]
    fn ahead_behind_malformed() {
        assert_eq!(parse_ahead_behind("abc\txyz"), (0, 0));
    }

    // --- ahead / behind ---

    #[tokio::test]
    async fn ahead_behind_ignores_a_stale_local_base_branch() {
        // The phantom-commit bug: an agent checkout sits exactly on the fetched
        // `origin/main`, while the `refs/heads/main` it inherited from the clone
        // is a snapshot from clone time. Measuring against the local ref would
        // report the checkout as ahead of a base it is identical to.
        let td = tempfile::tempdir().unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        let capture = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        let source = td.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        run(&source, &["init", "-q", "-b", "main"]);
        run(&source, &["config", "user.email", "t@example.com"]);
        run(&source, &["config", "user.name", "Tester"]);
        std::fs::write(source.join("a.txt"), b"one").unwrap();
        run(&source, &["add", "-A"]);
        run(&source, &["commit", "-q", "-m", "first"]);

        let clone = td.path().join("clone");
        run(
            td.path(),
            &[
                "clone",
                "-q",
                source.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );

        // The base moves; the checkout fetches and detaches onto the new tip.
        // Its local `main` never moves again.
        std::fs::write(source.join("b.txt"), b"two").unwrap();
        run(&source, &["add", "-A"]);
        run(&source, &["commit", "-q", "-m", "second"]);
        run(&clone, &["fetch", "-q", "origin", "main"]);
        let moved = capture(&clone, &["rev-parse", "refs/remotes/origin/main"]);
        run(&clone, &["checkout", "-q", "--detach", &moved]);
        assert_ne!(moved, capture(&clone, &["rev-parse", "refs/heads/main"]));

        let base = crate::git::resolve_base(&clone, &source, "main", None).await;
        let state = query(&clone, &base).await.unwrap();
        assert_eq!(state.parent_branch, "main");
        assert_eq!((state.ahead, state.behind), (0, 0));
    }

    // --- query_unpushed ---

    fn git_run(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), name.as_bytes()).unwrap();
        git_run(dir, &["add", "-A"]);
        git_run(dir, &["commit", "-q", "-m", name]);
    }

    /// A one-commit source repo plus a clone of it — the shape of an agent
    /// workspace. Returns the clone.
    fn clone_fixture(root: &Path) -> std::path::PathBuf {
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        git_run(&source, &["init", "-q", "-b", "main"]);
        git_run(&source, &["config", "user.email", "t@example.com"]);
        git_run(&source, &["config", "user.name", "Tester"]);
        commit(&source, "a.txt");

        let clone = root.join("clone");
        git_run(
            root,
            &[
                "clone",
                "-q",
                source.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        git_run(&clone, &["config", "user.email", "t@example.com"]);
        git_run(&clone, &["config", "user.name", "Tester"]);
        clone
    }

    /// The bug this replaced: a detached agent clone has no upstream, and the
    /// old fallback reported `ahead` — commits `origin` already has — so a
    /// fresh workspace that changed nothing claimed a pushable backlog.
    #[tokio::test]
    async fn unpushed_is_zero_on_a_fresh_detached_clone() {
        let td = tempfile::tempdir().unwrap();
        let clone = clone_fixture(td.path());
        git_run(&clone, &["checkout", "-q", "--detach", "origin/main"]);

        assert_eq!(query_unpushed(&clone).await, 0);
    }

    #[tokio::test]
    async fn unpushed_counts_detached_commits_origin_lacks() {
        let td = tempfile::tempdir().unwrap();
        let clone = clone_fixture(td.path());
        git_run(&clone, &["checkout", "-q", "--detach", "origin/main"]);
        commit(&clone, "work.txt");

        assert_eq!(query_unpushed(&clone).await, 1);
    }

    #[tokio::test]
    async fn unpushed_still_counts_against_the_upstream_when_there_is_one() {
        let td = tempfile::tempdir().unwrap();
        let clone = clone_fixture(td.path());
        // The clone's `main` tracks `origin/main`.
        assert_eq!(query_unpushed(&clone).await, 0);

        commit(&clone, "work.txt");
        assert_eq!(query_unpushed(&clone).await, 1);
    }

    // --- git_meta ---

    #[tokio::test]
    async fn git_meta_counts_behind_against_the_source_tracking_ref() {
        // Mirror the live topology: a `--shared` clone borrows the source's
        // objects, so a base SHA that only exists in the source (a moved
        // base the clone never fetched) is still countable in the clone.
        let td = tempfile::tempdir().unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        let capture = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        let source = td.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        run(&source, &["init", "-q", "-b", "main"]);
        run(&source, &["config", "user.email", "t@example.com"]);
        run(&source, &["config", "user.name", "Tester"]);
        std::fs::write(source.join("a.txt"), b"one").unwrap();
        run(&source, &["add", "-A"]);
        run(&source, &["commit", "-q", "-m", "first"]);

        // Shared clone, then branch off and commit — one ahead, zero behind.
        let clone = td.path().join("clone");
        run(
            td.path(),
            &[
                "clone",
                "-q",
                "--shared",
                source.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        run(&clone, &["config", "user.email", "t@example.com"]);
        run(&clone, &["config", "user.name", "Tester"]);
        run(&clone, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(clone.join("f.txt"), b"work").unwrap();
        run(&clone, &["add", "-A"]);
        run(&clone, &["commit", "-q", "-m", "feature work"]);

        // The base advances by two commits in the source only (the clone never
        // fetches) — but the objects are shared via alternates.
        std::fs::write(source.join("b.txt"), b"two").unwrap();
        run(&source, &["add", "-A"]);
        run(&source, &["commit", "-q", "-m", "second"]);
        std::fs::write(source.join("c.txt"), b"three").unwrap();
        run(&source, &["add", "-A"]);
        run(&source, &["commit", "-q", "-m", "third"]);
        let moved_base = capture(&source, &["rev-parse", "main"]);
        // What the background freshness fetch leaves behind on the source.
        run(
            &source,
            &["update-ref", "refs/remotes/origin/main", &moved_base],
        );

        // A staged-but-uncommitted edit in the clone shows up as a changed file.
        std::fs::write(clone.join("dirty.txt"), b"x").unwrap();

        let base = crate::git::resolve_base(&clone, &source, "main", None).await;
        assert_eq!(base.tip.as_deref(), Some(moved_base.as_str()));
        let meta = git_meta(&clone, &base).await;
        assert_eq!(meta.base, "main");
        assert_eq!(meta.behind, Some(2), "base moved two commits ahead");
        assert!(meta.files.iter().any(|p| p == "dirty.txt"));

        // Unresolvable base → unknown behind, files still resolve.
        let unknown = crate::git::resolve_base(&clone, &source, "no-such-base", None).await;
        assert_eq!(unknown.tip, None);
        let meta_none = git_meta(&clone, &unknown).await;
        assert_eq!(meta_none.behind, None);
        assert!(meta_none.files.iter().any(|p| p == "dirty.txt"));
    }

    // --- parse_porcelain ---

    #[test]
    fn porcelain_modified_unstaged() {
        let input = " M src/main.rs\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/main.rs");
        assert!(matches!(files[0].kind, StatusKind::Modified));
        assert!(!files[0].staged);
    }

    #[test]
    fn porcelain_added_staged() {
        let input = "A  src/new.rs\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/new.rs");
        assert!(matches!(files[0].kind, StatusKind::Added));
        assert!(files[0].staged);
    }

    #[test]
    fn porcelain_deleted_staged() {
        let input = "D  old.rs\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert!(matches!(files[0].kind, StatusKind::Deleted));
        assert!(files[0].staged);
    }

    #[test]
    fn porcelain_untracked() {
        let input = "?? untracked.rs\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert!(matches!(files[0].kind, StatusKind::Untracked));
        assert!(!files[0].staged);
    }

    #[test]
    fn porcelain_conflicted_uu() {
        let input = "UU conflict.rs\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert!(matches!(files[0].kind, StatusKind::Conflicted));
    }

    #[test]
    fn porcelain_conflicted_aa() {
        let input = "AA conflict.rs\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert!(matches!(files[0].kind, StatusKind::Conflicted));
    }

    #[test]
    fn porcelain_renamed_arrow() {
        // git porcelain v1 format: `R  <old-name> -> <new-name>`
        let input = "R  old_name.rs -> new_name.rs\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new_name.rs");
        assert!(matches!(files[0].kind, StatusKind::Renamed));
    }

    #[test]
    fn porcelain_renamed_tab() {
        // Some git versions use a tab: `R  <old-name>\t<new-name>`
        let input = "R  old_name.rs\tnew_name.rs\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new_name.rs");
        assert!(matches!(files[0].kind, StatusKind::Renamed));
    }

    #[test]
    fn porcelain_multiple_files() {
        let input = " M src/a.rs\nA  src/b.rs\n?? src/c.rs\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn porcelain_empty() {
        assert!(parse_porcelain("").is_empty());
    }

    #[test]
    fn porcelain_unquotes_path_with_space() {
        // `git status` C-quotes paths containing spaces.
        let input = "?? \"src/foo copy.ts\"\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/foo copy.ts");
        assert!(matches!(files[0].kind, StatusKind::Untracked));
    }

    #[test]
    fn porcelain_unquotes_non_ascii_octal_escapes() {
        // "café.ts" → octal-escaped UTF-8 bytes inside quotes.
        let input = " M \"caf\\303\\251.ts\"\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "café.ts");
    }

    #[test]
    fn porcelain_unquotes_renamed_destination() {
        let input = "R  \"old name.rs\" -> \"new name.rs\"\n";
        let files = parse_porcelain(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new name.rs");
        assert!(matches!(files[0].kind, StatusKind::Renamed));
    }

    #[test]
    fn unquote_passes_through_plain_paths() {
        assert_eq!(unquote_path("src/main.rs"), "src/main.rs");
        assert_eq!(unquote_path("\"a b.rs\""), "a b.rs");
        assert_eq!(unquote_path("\"a\\\"b.rs\""), "a\"b.rs");
    }

    // --- parse_numstat ---

    #[test]
    fn numstat_typical() {
        let input = "10\t3\tsrc/main.rs\n5\t0\tsrc/lib.rs\n";
        let map = parse_numstat(input);
        assert_eq!(map.get("src/main.rs"), Some(&(10, 3)));
        assert_eq!(map.get("src/lib.rs"), Some(&(5, 0)));
    }

    #[test]
    fn numstat_binary_file() {
        let input = "-\t-\timage.png\n";
        let map = parse_numstat(input);
        assert_eq!(map.get("image.png"), Some(&(0, 0)));
    }

    #[test]
    fn numstat_empty() {
        assert!(parse_numstat("").is_empty());
    }

    #[test]
    fn numstat_renamed_file() {
        // git diff --numstat HEAD emits renames as `<add>\t<del>\tOLD => NEW`
        let input = "5\t2\told_name.rs => new_name.rs\n";
        let map = parse_numstat(input);
        assert_eq!(map.get("new_name.rs"), Some(&(5, 2)));
        // Old name should not be present
        assert!(!map.contains_key("old_name.rs => new_name.rs"));
    }

    #[test]
    fn numstat_mixed() {
        let input = "42\t7\tsrc/foo.rs\n-\t-\tassets/logo.png\n";
        let map = parse_numstat(input);
        assert_eq!(map.get("src/foo.rs"), Some(&(42, 7)));
        assert_eq!(map.get("assets/logo.png"), Some(&(0, 0)));
    }

    #[test]
    fn numstat_unquotes_non_ascii_path() {
        // numstat C-quotes non-ASCII paths; the key must match the unquoted
        // path used in the file list, or line counts silently drop to 0.
        let input = "4\t0\t\"caf\\303\\251.ts\"\n";
        let map = parse_numstat(input);
        assert_eq!(map.get("café.ts"), Some(&(4, 0)));
    }
}

/// Add `entry` to a checkout's `.git/info/exclude` if it isn't already listed,
/// so a host-generated file inside the worktree never shows up in `git status`.
///
/// `.git/info/exclude` (not `.gitignore`) because it's local to the checkout
/// and untracked — Fletch never edits a file the repo ships. Idempotent: the
/// entry is written once and re-checked on every later spawn.
///
/// Only hides files git isn't already tracking; a *tracked* path stays visible
/// as a modification, which is the honest outcome — see `cursor_mcp_delivery`.
pub fn ensure_git_exclude(checkout: &Path, entry: &str) -> Result<()> {
    use crate::error::Error;

    let info_dir = checkout.join(".git").join("info");
    std::fs::create_dir_all(&info_dir)
        .map_err(|e| Error::Other(format!("create .git/info: {e}")))?;
    let exclude = info_dir.join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == entry) {
        return Ok(());
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(entry);
    next.push('\n');
    std::fs::write(&exclude, next)
        .map_err(|e| Error::Other(format!("write .git/info/exclude: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod exclude_tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join(".git").join("info")).unwrap();
        td
    }

    #[test]
    fn entry_is_appended_once_and_preserves_existing_lines() {
        let td = repo();
        let exclude = td.path().join(".git/info/exclude");
        std::fs::write(&exclude, "# user's own\nbuild/\n").unwrap();

        ensure_git_exclude(td.path(), ".cursor/mcp.json").unwrap();
        ensure_git_exclude(td.path(), ".cursor/mcp.json").unwrap();

        let body = std::fs::read_to_string(&exclude).unwrap();
        assert_eq!(body.matches(".cursor/mcp.json").count(), 1, "{body}");
        assert!(
            body.contains("# user's own"),
            "clobbered user lines: {body}"
        );
        assert!(body.contains("build/"), "clobbered user lines: {body}");
    }

    #[test]
    fn missing_exclude_file_is_created_with_a_trailing_newline() {
        let td = repo();
        ensure_git_exclude(td.path(), ".cursor/mcp.json").unwrap();
        let body = std::fs::read_to_string(td.path().join(".git/info/exclude")).unwrap();
        assert_eq!(body, ".cursor/mcp.json\n");
    }

    #[test]
    fn a_file_without_a_trailing_newline_does_not_glue_entries_together() {
        let td = repo();
        let exclude = td.path().join(".git/info/exclude");
        std::fs::write(&exclude, "build/").unwrap();
        ensure_git_exclude(td.path(), ".codegraph").unwrap();
        assert_eq!(
            std::fs::read_to_string(&exclude).unwrap(),
            "build/\n.codegraph\n"
        );
    }
}

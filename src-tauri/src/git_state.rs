//! Read-only git state queries for a worktree.
//!
//! Companion to `git.rs` which handles mutations. This module only reads.

use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;

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
    pub files: Vec<FileStatus>,
    pub additions: u32,
    pub deletions: u32,
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

/// Query the read-only git state for a worktree.
pub async fn query(worktree_path: &Path, parent_branch: &str) -> Result<GitState> {
    // 1. Branch name
    let branch = match crate::git::current_branch(worktree_path).await {
        Ok(Some(b)) => b,
        Ok(None) => String::new(),
        // Empty repo / no HEAD — return a zero-state
        Err(_) => {
            return Ok(GitState {
                branch: String::new(),
                parent_branch: parent_branch.to_string(),
                ahead: 0,
                behind: 0,
                files: vec![],
                additions: 0,
                deletions: 0,
            });
        }
    };

    // 2. Ahead / behind
    let (ahead, behind) = query_ahead_behind(worktree_path, parent_branch).await;

    // 3. File list from `git status --porcelain=v1`
    let mut files = match run_status(worktree_path).await {
        Ok(output) => parse_porcelain(&output),
        Err(_) => vec![],
    };

    // 4. Per-file diff stats from `git diff --numstat HEAD`
    let numstat = match run_numstat(worktree_path).await {
        Ok(output) => parse_numstat(&output),
        Err(_) => HashMap::new(),
    };

    // Merge numstat into file list
    for f in &mut files {
        if let Some(&(adds, dels)) = numstat.get(&f.path) {
            f.additions = adds;
            f.deletions = dels;
        }
    }

    let additions: u32 = files.iter().map(|f| f.additions).sum();
    let deletions: u32 = files.iter().map(|f| f.deletions).sum();

    Ok(GitState {
        branch,
        parent_branch: parent_branch.to_string(),
        ahead,
        behind,
        files,
        additions,
        deletions,
    })
}

// ---------------------------------------------------------------------------
// Private helpers — subprocess runners
// ---------------------------------------------------------------------------

async fn query_ahead_behind(worktree_path: &Path, parent_branch: &str) -> (u32, u32) {
    let out = Command::new("git")
        .current_dir(worktree_path)
        .args(["rev-list", "--left-right", "--count", &format!("HEAD...{parent_branch}")])
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            parse_ahead_behind(s.trim())
        }
        _ => (0, 0),
    }
}

async fn run_status(worktree_path: &Path) -> Result<String> {
    let out = Command::new("git")
        .current_dir(worktree_path)
        .args(["status", "--porcelain=v1"])
        .output()
        .await?;
    if !out.status.success() {
        tracing::warn!("git status --porcelain=v1 failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn run_numstat(worktree_path: &Path) -> Result<String> {
    let out = Command::new("git")
        .current_dir(worktree_path)
        .args(["diff", "--numstat", "HEAD"])
        .output()
        .await?;
    if !out.status.success() {
        tracing::warn!("git diff --numstat failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// Private helpers — pure parsers (also visible to tests)
// ---------------------------------------------------------------------------

fn parse_ahead_behind(s: &str) -> (u32, u32) {
    let mut parts = s.splitn(2, '\t');
    let ahead: u32 = parts.next().and_then(|t| t.trim().parse().ok()).unwrap_or(0);
    let behind: u32 = parts.next().and_then(|t| t.trim().parse().ok()).unwrap_or(0);
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
                b'a' => { out.push(0x07); i += 2; }
                b'b' => { out.push(0x08); i += 2; }
                b't' => { out.push(b'\t'); i += 2; }
                b'n' => { out.push(b'\n'); i += 2; }
                b'v' => { out.push(0x0b); i += 2; }
                b'f' => { out.push(0x0c); i += 2; }
                b'r' => { out.push(b'\r'); i += 2; }
                b'"' => { out.push(b'"'); i += 2; }
                b'\\' => { out.push(b'\\'); i += 2; }
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
                _ => { out.push(inner[i]); i += 1; }
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
        assert!(map.get("old_name.rs => new_name.rs").is_none());
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

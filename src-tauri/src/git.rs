//! Thin wrapper around `git worktree`.
//!
//! Kept deliberately minimal — the v1 supervisor only needs to add a
//! worktree on a fresh branch and remove it later.

use std::path::Path;
use tokio::process::Command;

use crate::error::{Error, Result};

/// Create a worktree on detached HEAD (no branch yet). Used by the
/// instant-spawn flow so we don't pollute `git branch` for agents
/// that may never receive a user message. The branch is created
/// later via `checkout_new_branch` when we have a slug from the
/// first user message.
pub async fn worktree_add_detached(repo: &Path, worktree_path: &Path) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "worktree",
            "add",
            "--detach",
            worktree_path.to_str().ok_or_else(|| {
                Error::InvalidPath(worktree_path.display().to_string())
            })?,
        ])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "worktree add --detach failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Inside an existing worktree, create a new branch at the current
/// commit and check it out (`git checkout -b <branch>`). Used to
/// promote a detached-HEAD worktree onto a named branch once the
/// first user message gives us a slug.
pub async fn checkout_new_branch(worktree: &Path, branch: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["checkout", "-b", branch])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "checkout -b {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

pub async fn worktree_remove(repo: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let path_str = worktree_path
        .to_str()
        .ok_or_else(|| Error::InvalidPath(worktree_path.display().to_string()))?;
    args.push(path_str);
    let out = Command::new("git")
        .current_dir(repo)
        .args(&args)
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "worktree remove failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Drop any internal `.git/worktrees/<id>` refs whose linked working tree
/// no longer exists. Safe to run unconditionally — git just no-ops when
/// there's nothing to prune.
pub async fn worktree_prune(repo: &Path) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["worktree", "prune"])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "worktree prune failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Return the name of the currently-checked-out branch in the repo,
/// or `None` if HEAD is detached. Used by the supervisor to record
/// the parent branch when spawning an agent worktree.
pub async fn current_branch(repo: &Path) -> Result<Option<String>> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["symbolic-ref", "--short", "-q", "HEAD"])
        .output()
        .await?;
    match out.status.code() {
        Some(0) => {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if name.is_empty() {
                Ok(None)
            } else {
                Ok(Some(name))
            }
        }
        // `symbolic-ref -q` exits 1 in detached-HEAD state. Treat that
        // as "no branch", not an error.
        Some(1) => Ok(None),
        _ => Err(Error::Git(format!(
            "symbolic-ref failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// Whether a local branch with this name exists in the repo. Used by
/// the supervisor to disambiguate auto-generated branch names before
/// spawning a worktree — on collision it falls back to a name that
/// includes the agent's place id.
pub async fn branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .await?;
    // Exit 0 = ref exists, exit 1 = not found, anything else = real error.
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::Git(format!(
            "show-ref failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// Resolve a ref to its full SHA. Returns the bare 40-char hex string.
/// Errors if the ref is unknown or git is unhappy.
pub async fn rev_parse(repo: &Path, refname: &str) -> Result<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--verify", refname])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "rev-parse {refname} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run `git diff --shortstat <a>..<b>` and parse the additions /
/// deletions counts. Returns zero counts if both refs resolve to the
/// same commit (git prints nothing in that case).
pub async fn diff_shortstat(
    repo: &Path,
    from_sha: &str,
    to_sha: &str,
) -> Result<(u32, u32)> {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "diff",
            "--shortstat",
            &format!("{from_sha}..{to_sha}"),
        ])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "diff --shortstat failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(parse_shortstat(&line))
}

/// Run `git diff --shortstat <base>` from a live worktree. This compares the
/// current working tree, including uncommitted changes, against the base ref.
pub async fn worktree_diff_shortstat(
    worktree: &Path,
    base_ref: &str,
) -> Result<(u32, u32)> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["diff", "--shortstat", base_ref])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "diff --shortstat {base_ref} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(parse_shortstat(&line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shortstat_typical() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 82 insertions(+), 12 deletions(-)"),
            (82, 12)
        );
    }

    #[test]
    fn parse_shortstat_only_additions() {
        assert_eq!(
            parse_shortstat(" 1 file changed, 5 insertions(+)"),
            (5, 0)
        );
    }

    #[test]
    fn parse_shortstat_only_deletions() {
        assert_eq!(
            parse_shortstat(" 2 files changed, 9 deletions(-)"),
            (0, 9)
        );
    }

    #[test]
    fn parse_shortstat_empty() {
        assert_eq!(parse_shortstat(""), (0, 0));
    }
}

fn parse_shortstat(s: &str) -> (u32, u32) {
    let mut adds = 0u32;
    let mut dels = 0u32;
    for chunk in s.split(',').map(|c| c.trim()) {
        let mut parts = chunk.splitn(2, ' ');
        let n: u32 = match parts.next().and_then(|t| t.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let label = parts.next().unwrap_or("");
        if label.starts_with("insertion") {
            adds = n;
        } else if label.starts_with("deletion") {
            dels = n;
        }
    }
    (adds, dels)
}

/// Create a branch at a specific commit. Errors if the branch already
/// exists or the SHA isn't reachable.
pub async fn branch_create_at(repo: &Path, name: &str, sha: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["branch", name, sha])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "branch {name} {sha} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Create a worktree at `worktree_path` checked out on an existing
/// branch. Counterpart to `worktree_add_detached` — used by restore.
pub async fn worktree_add_branch(
    repo: &Path,
    worktree_path: &Path,
    branch: &str,
) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "worktree",
            "add",
            worktree_path.to_str().ok_or_else(|| {
                Error::InvalidPath(worktree_path.display().to_string())
            })?,
            branch,
        ])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "worktree add {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Force-delete a local branch. Returns Ok even if the branch never
/// existed in the first place — that's exactly the state the caller
/// usually wants to converge on. Errors only for genuine git failures
/// (e.g. branch checked out in another live worktree).
pub async fn branch_delete(repo: &Path, branch: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["branch", "-D", branch])
        .output()
        .await?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // git emits "branch '<x>' not found." with exit 1 when the branch is
    // already gone. Treat that as success — the caller's goal is satisfied.
    if stderr.contains("not found") {
        return Ok(());
    }
    Err(Error::Git(format!(
        "branch -D {branch} failed: {}",
        stderr.trim()
    )))
}

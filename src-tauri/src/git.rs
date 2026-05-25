//! Thin wrapper around `git worktree`.
//!
//! Kept deliberately minimal — the v1 supervisor only needs to add a
//! worktree on a fresh branch and remove it later.

use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::error::{Error, Result};

pub async fn worktree_add(repo: &Path, worktree_path: &Path, branch: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "worktree",
            "add",
            "-b",
            branch,
            worktree_path.to_str().ok_or_else(|| {
                Error::InvalidPath(worktree_path.display().to_string())
            })?,
        ])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "worktree add failed: {}",
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

pub fn worktrees_dir(repo: &Path) -> PathBuf {
    repo.join(".worktrees")
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

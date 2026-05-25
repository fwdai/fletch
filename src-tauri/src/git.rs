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

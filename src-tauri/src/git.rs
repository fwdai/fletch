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

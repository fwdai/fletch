//! Thin wrapper around the `gh` CLI for GitHub PR operations.
//!
//! Follows the same subprocess pattern as `git.rs` — each function
//! shells out to `gh` and maps exit-code / stderr to typed errors.

use std::path::Path;
use tokio::process::Command;

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrStatus {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PrState {
    pub number: u32,
    pub url: String,
    pub state: PrStatus,
    pub title: String,
    pub mergeable: bool,
}

// ---------------------------------------------------------------------------
// Internal deserialization helpers
// ---------------------------------------------------------------------------

/// Raw shape returned by `gh pr view --json ...`. `gh` uses uppercase
/// strings for both `state` and `mergeable`.
#[derive(serde::Deserialize)]
struct GhPrRaw {
    number: u32,
    url: String,
    state: String,     // "OPEN" | "MERGED" | "CLOSED"
    title: String,
    mergeable: String, // "MERGEABLE" | "CONFLICTING" | "UNKNOWN"
}

impl From<GhPrRaw> for PrState {
    fn from(raw: GhPrRaw) -> Self {
        PrState {
            number: raw.number,
            url: raw.url,
            state: match raw.state.as_str() {
                "MERGED" => PrStatus::Merged,
                "CLOSED" => PrStatus::Closed,
                _ => PrStatus::Open,
            },
            title: raw.title,
            mergeable: raw.mergeable == "MERGEABLE",
        }
    }
}

// ---------------------------------------------------------------------------
// Public async functions
// ---------------------------------------------------------------------------

/// Fetch the current PR state for the branch checked out in `worktree`.
///
/// Returns `Ok(None)` when `gh` exits non-zero with "no pull requests found"
/// in stderr (i.e. the branch simply has no open PR yet).
pub async fn pr_view(worktree: &Path) -> Result<Option<PrState>> {
    let out = Command::new("gh")
        .current_dir(worktree)
        .args(["pr", "view", "--json", "number,url,state,title,mergeable"])
        .output()
        .await?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.to_lowercase().contains("no pull requests found") {
            return Ok(None);
        }
        return Err(Error::Git(stderr.trim().to_string()));
    }

    let raw: GhPrRaw = serde_json::from_slice(&out.stdout)?;
    Ok(Some(raw.into()))
}

/// Create a PR for the branch checked out in `worktree`.
///
/// If `title` is empty the `--fill` flag is used so `gh` auto-fills the
/// title and body from the commit log. Otherwise `--title` / `--body` are
/// passed explicitly.
pub async fn pr_create(worktree: &Path, title: &str, body: &str, base: &str) -> Result<PrState> {
    let mut args = vec!["pr", "create", "--json", "number,url,state,title,mergeable"];
    if title.is_empty() {
        args.push("--fill");
    } else {
        args.extend_from_slice(&["--title", title, "--body", body]);
    }
    args.extend_from_slice(&["--base", base]);

    let out = Command::new("gh")
        .current_dir(worktree)
        .args(&args)
        .output()
        .await?;

    if !out.status.success() {
        return Err(Error::Git(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    let raw: GhPrRaw = serde_json::from_slice(&out.stdout)?;
    Ok(raw.into())
}

/// Merge the open PR for the branch checked out in `worktree` using a merge
/// commit and the `--auto` flag (merges as soon as all checks pass).
pub async fn pr_merge(worktree: &Path) -> Result<()> {
    let out = Command::new("gh")
        .current_dir(worktree)
        .args(["pr", "merge", "--merge", "--auto"])
        .output()
        .await?;

    if !out.status.success() {
        return Err(Error::Git(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    Ok(())
}

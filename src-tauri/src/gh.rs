//! Thin wrapper around the `gh` CLI for GitHub PR operations.
//!
//! Follows the same subprocess pattern as `git.rs` — each function
//! shells out to `gh` and maps exit-code / stderr to typed errors.

use std::io::ErrorKind;
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
        return Err(Error::Gh(stderr.trim().to_string()));
    }

    let raw: GhPrRaw = serde_json::from_slice(&out.stdout)?;
    Ok(Some(raw.into()))
}

/// Create a PR for the branch checked out in `worktree`.
///
/// If `title` is empty the `--fill` flag is used so `gh` auto-fills the
/// title and body from the commit log. Otherwise `--title` / `--body` are
/// passed explicitly.
///
/// `gh pr create` does not support `--json`, so we run it for its side-effect
/// (creating the PR) and then call `pr_view` to fetch the full `PrState`.
/// Whether a `gh pr create` failure means a PR for this branch already exists
/// (gh's message is "a pull request for branch ... already exists"). Used to
/// make `pr_create` idempotent across retries.
fn pr_already_exists(stderr: &str) -> bool {
    stderr.to_lowercase().contains("already exists")
}

pub async fn pr_create(worktree: &Path, title: &str, body: &str, base: &str) -> Result<PrState> {
    let mut args = vec!["pr", "create", "--base", base];
    if title.is_empty() {
        args.push("--fill");
    } else {
        args.extend_from_slice(&["--title", title, "--body", body]);
    }

    let out = Command::new("gh")
        .current_dir(worktree)
        .args(&args)
        .output()
        .await?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // Idempotency: a prior attempt may have created the PR but failed
        // before we could fetch it (a transient `pr_view` error returns
        // `Err` even though the PR exists). On retry `gh pr create` reports
        // the branch already has a PR — treat that as success by returning the
        // existing one, so the caller isn't stuck erroring forever over a PR
        // that's actually there.
        if pr_already_exists(&stderr) {
            if let Some(pr) = pr_view(worktree).await? {
                return Ok(pr);
            }
        }
        return Err(Error::Gh(stderr.trim().to_string()));
    }

    // `gh pr create` only prints the PR URL on success; fetch full state.
    pr_view(worktree).await?.ok_or_else(|| {
        Error::Gh("PR was created but could not be fetched".into())
    })
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
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Account / discovery (used by the New Project flow)
// ---------------------------------------------------------------------------

/// Whether `gh` is installed and authenticated. Drives the New Project UI:
/// clone and create both go through `gh`, so we surface a clear prompt up
/// front instead of letting an operation fail half-way.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GhStatus {
    pub installed: bool,
    pub authenticated: bool,
    pub login: Option<String>,
}

/// One repo as returned by `gh repo list --json`. `gh` emits camelCase keys;
/// we re-expose the snake_case shape the rest of the IPC surface uses.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GhRepoSummary {
    pub name_with_owner: String,
    pub description: Option<String>,
    pub is_private: bool,
    pub updated_at: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRepoRaw {
    name_with_owner: String,
    description: Option<String>,
    is_private: bool,
    updated_at: String,
}

impl From<GhRepoRaw> for GhRepoSummary {
    fn from(r: GhRepoRaw) -> Self {
        GhRepoSummary {
            name_with_owner: r.name_with_owner,
            // `gh` returns "" rather than null for an empty description.
            description: r.description.filter(|d| !d.is_empty()),
            is_private: r.is_private,
            updated_at: r.updated_at,
        }
    }
}

/// Probe `gh` availability and auth. Never errors — a missing binary or a
/// logged-out state are reported as fields, not failures.
pub async fn auth_status() -> Result<GhStatus> {
    let out = match Command::new("gh").args(["auth", "status"]).output().await {
        Ok(out) => out,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(GhStatus { installed: false, authenticated: false, login: None });
        }
        Err(e) => return Err(e.into()),
    };

    if !out.status.success() {
        // `gh` is installed but no account is logged in.
        return Ok(GhStatus { installed: true, authenticated: false, login: None });
    }

    // Best-effort login name; never fail the whole probe on it.
    let login = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    Ok(GhStatus { installed: true, authenticated: true, login })
}

/// List the authenticated user's repos (most-recently-updated first). The
/// New Project picker filters this list client-side.
pub async fn repo_list(limit: u32) -> Result<Vec<GhRepoSummary>> {
    let out = Command::new("gh")
        .args([
            "repo",
            "list",
            "--json",
            "nameWithOwner,description,isPrivate,updatedAt",
            "--limit",
            &limit.to_string(),
        ])
        .output()
        .await?;

    if !out.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    let raw: Vec<GhRepoRaw> = serde_json::from_slice(&out.stdout)?;
    Ok(raw.into_iter().map(Into::into).collect())
}

/// Clone `spec` (an `owner/repo`, an https URL, or an ssh URL — `gh` accepts
/// all three) into `target`.
pub async fn repo_clone(spec: &str, target: &Path) -> Result<()> {
    let target = target.to_str().ok_or_else(|| {
        Error::InvalidPath(target.display().to_string())
    })?;
    let out = Command::new("gh")
        .args(["repo", "clone", spec, target])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Create a GitHub repo from the existing git repo at `target` and push the
/// initial commit. `target` must already be a git repo with at least one
/// commit (see `new_project::create`).
pub async fn repo_create_and_push(
    target: &Path,
    name: &str,
    private: bool,
    description: Option<&str>,
) -> Result<()> {
    let mut args = vec![
        "repo".to_string(),
        "create".to_string(),
        name.to_string(),
        if private { "--private".to_string() } else { "--public".to_string() },
        "--source=.".to_string(),
        "--remote=origin".to_string(),
        "--push".to_string(),
    ];
    if let Some(desc) = description.filter(|d| !d.is_empty()) {
        args.push("--description".to_string());
        args.push(desc.to_string());
    }

    let out = Command::new("gh")
        .current_dir(target)
        .args(&args)
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_raw_open_mergeable() {
        let raw = GhPrRaw {
            number: 42,
            url: "https://github.com/owner/repo/pull/42".into(),
            state: "OPEN".into(),
            title: "My PR".into(),
            mergeable: "MERGEABLE".into(),
        };
        let pr = PrState::from(raw);
        assert!(matches!(pr.state, PrStatus::Open));
        assert!(pr.mergeable);
        assert_eq!(pr.number, 42);
    }

    #[test]
    fn detects_already_exists_failure() {
        // gh's real message for a duplicate PR.
        assert!(pr_already_exists(
            "a pull request for branch \"feat\" into branch \"main\" already exists:\nhttps://github.com/o/r/pull/7"
        ));
        // An unrelated failure must not be mistaken for it.
        assert!(!pr_already_exists("fatal: could not read from remote repository"));
    }

    #[test]
    fn pr_raw_merged_conflicting() {
        let raw = GhPrRaw {
            number: 1,
            url: "u".into(),
            state: "MERGED".into(),
            title: "t".into(),
            mergeable: "CONFLICTING".into(),
        };
        let pr = PrState::from(raw);
        assert!(matches!(pr.state, PrStatus::Merged));
        assert!(!pr.mergeable);
    }

    #[test]
    fn pr_raw_closed_unknown() {
        let raw = GhPrRaw {
            number: 2,
            url: "u".into(),
            state: "CLOSED".into(),
            title: "t".into(),
            mergeable: "UNKNOWN".into(),
        };
        let pr = PrState::from(raw);
        assert!(matches!(pr.state, PrStatus::Closed));
        assert!(!pr.mergeable);
    }

    #[test]
    fn pr_raw_unknown_state_defaults_to_open() {
        let raw = GhPrRaw {
            number: 3,
            url: "u".into(),
            state: "SOMETHING_NEW".into(),
            title: "t".into(),
            mergeable: "MERGEABLE".into(),
        };
        let pr = PrState::from(raw);
        assert!(matches!(pr.state, PrStatus::Open));
    }
}

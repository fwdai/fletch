//! Git transport for a run (spec §12). The load-bearing fact (§12.1): step
//! workspaces are `--shared` clones of the user's repo, so a commit created in
//! one step's clone is unreachable from every other clone and from the source.
//! v1 makes transport explicit through a **run repository** — a host-owned
//! `--shared` clone under `~/.fletch/runs/<id>/repo/` that is the run's durable
//! git home:
//!
//! - every boundary commit is pinned as `refs/wf/steps/<step-exec-id>` in the
//!   step workspace and **ferried** into the run repo by an explicit-ref fetch;
//!   only after the ferry succeeds is the attempt `done` (§6.3 step 8);
//! - the next step forks from that ref *in the run repo* (the provisioning
//!   extension in `sandbox/provision.rs`);
//! - finalize pushes the run branch from the run repo, so it works even after
//!   every step workspace is gone.
//!
//! The v0 hardenings are kept: `wf/`-namespace enforcement on push, and the
//! path-validated artifact probe (now in `workflow::attempt`).

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::git;

/// The run repository lives beside the blackboard, under the run directory.
pub fn run_repo_path(run_dir: &Path) -> PathBuf {
    run_dir.join("repo")
}

/// The ref a step's boundary commit is pinned as, in both the step workspace and
/// (after the ferry) the run repo. Explicit refs mean no `allowAnySHA1InWant`.
pub fn step_ref(step_exec_id: &str) -> String {
    format!("refs/wf/steps/{step_exec_id}")
}

/// A run only ever pushes to its own generated `wf/<slug>-<suffix>` branch.
/// Enforce that namespace (and a safe ref charset) so a tampered run row or a
/// direct invoke can't redirect the push onto `main` or another branch. Moved
/// verbatim from v0 `workflows.rs`.
pub fn is_run_branch(b: &str) -> bool {
    b.starts_with("wf/")
        && b.len() <= 200
        && !b.contains("..")
        && b.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'-' | b'_' | b'.'))
}

/// Provision the run repository: a host-owned `--shared` clone of the source
/// repo, with `origin` pointed at the source's real remote so finalize's push/PR
/// hit it (not the local source path). Idempotent — reused on resume.
pub async fn provision_run_repo(source_repo: &Path, run_dir: &Path) -> Result<PathBuf> {
    let dest = run_repo_path(run_dir);
    if dest.join(".git").exists() {
        return Ok(dest);
    }
    tokio::fs::create_dir_all(run_dir).await?;
    let source = path_str(source_repo)?;
    let dest_s = path_str(&dest)?;
    git::run_git(
        run_dir,
        &["clone", "--shared", &source, &dest_s],
        "clone run repo",
    )
    .await?;
    rewrite_origin(source_repo, &dest).await?;
    Ok(dest)
}

/// Point the run repo's `origin` at the source repo's real remote (so a finalize
/// push reaches the user's remote, not the local source path). Removes the
/// clone's local-path `origin` when the source has none — the same discipline as
/// `sandbox/provision.rs::rewrite_origin`.
async fn rewrite_origin(source_repo: &Path, dest: &Path) -> Result<()> {
    let out = git::git_output(source_repo, &["remote", "get-url", "origin"]).await?;
    if out.status.success() {
        let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
        git::run_git(dest, &["remote", "set-url", "origin", &url], "set-url origin").await?;
    } else {
        // No source remote → drop the local-path origin so a push can't write
        // into the user's source repo.
        let _ = git::run_git(dest, &["remote", "remove", "origin"], "remove origin").await;
    }
    Ok(())
}

/// Current HEAD of a checkout (the `head_start` snapshot and the `commit` gate).
pub async fn head_sha(checkout: &Path) -> Result<String> {
    git::rev_parse(checkout, "HEAD").await
}

/// The result of a boundary commit.
pub struct BoundaryCommit {
    pub head: String,
    pub committed: bool,
}

/// Stage everything and commit if the tree is dirty; always return the resulting
/// HEAD. Runs host-side on the step's checkout (the workspace has a seeded
/// identity from provisioning). Mirrors v0 `workflow_boundary_commit`.
pub async fn boundary_commit(checkout: &Path, message: &str) -> Result<BoundaryCommit> {
    git::run_git(checkout, &["add", "-A"], "git add").await?;
    // `diff --cached --quiet` exits 1 when something is staged.
    let staged = git::git_output(checkout, &["diff", "--cached", "--quiet"]).await?;
    let committed = !staged.status.success();
    if committed {
        git::run_git(checkout, &["commit", "-m", message], "boundary commit").await?;
    }
    Ok(BoundaryCommit {
        head: head_sha(checkout).await?,
        committed,
    })
}

/// Pin the step workspace's current HEAD as `refs/wf/steps/<step-exec-id>` so it
/// survives ferrying and workspace teardown. Returns the ref name.
pub async fn pin_step_ref(checkout: &Path, step_exec_id: &str) -> Result<String> {
    let refname = step_ref(step_exec_id);
    git::run_git(checkout, &["update-ref", &refname, "HEAD"], "pin step ref").await?;
    Ok(refname)
}

/// Ferry a pinned step ref from its workspace into the run repo by an explicit-
/// ref fetch — the durability step and the `done` precondition (§6.3 step 8).
/// Both repos are `--shared` clones of the same source, so only the step's new
/// objects transfer.
pub async fn ferry(step_checkout: &Path, run_repo: &Path, refname: &str) -> Result<()> {
    let src = path_str(step_checkout)?;
    let refspec = format!("{refname}:{refname}");
    git::run_git(run_repo, &["fetch", &src, &refspec], "ferry step ref into run repo").await?;
    Ok(())
}

/// The result of finalizing a run.
pub struct FinalizeOutcome {
    pub pushed: bool,
    pub branch: String,
    pub pr_url: Option<String>,
    pub pr_error: Option<String>,
}

/// Push the run branch to the origin from the **run repo** (§12.2) — HEAD is
/// moved to the final ferried ref first, so this works even after every step
/// workspace is deleted. The PR is best-effort (`pr_base` threaded through from
/// the spec's `finalize`). Push is guarded to the `wf/` namespace.
pub async fn finalize(
    run_repo: &Path,
    final_ref: &str,
    branch: &str,
    base: &str,
    title: &str,
    body: &str,
    open_pr: bool,
) -> Result<FinalizeOutcome> {
    if !is_run_branch(branch) {
        return Err(Error::Other(format!(
            "refusing to push: '{branch}' is not a wf/ run branch"
        )));
    }
    git::run_git(
        run_repo,
        &["checkout", "--detach", final_ref],
        "checkout final ref",
    )
    .await?;
    git::push_head_to_branch(run_repo, branch).await?;

    let (pr_url, pr_error) = if open_pr {
        match crate::github::pr_create_head(run_repo, branch, title, body, base).await {
            Ok(pr) => (Some(pr.url), None),
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, None)
    };
    Ok(FinalizeOutcome {
        pushed: true,
        branch: branch.to_string(),
        pr_url,
        pr_error,
    })
}

fn path_str(p: &Path) -> Result<String> {
    p.to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Other(format!("path is not valid UTF-8: {}", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["config", "user.email", "t@t.t"]);
        git(dir, &["config", "user.name", "t"]);
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn head(dir: &Path) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// §16 gitops transport: a commit made in workspace A is fetchable into the
    /// run repo by ref, and resolves there — the core ferry invariant.
    #[tokio::test]
    async fn ferry_makes_a_workspace_commit_reachable_in_the_run_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        init_repo(&source);
        write(&source, "README", "base");
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "base"]);

        let run_dir = tmp.path().join("run");
        let run_repo = provision_run_repo(&source, &run_dir).await.unwrap();

        // Workspace A: a --shared clone of the source that makes a new commit.
        let ws_a = tmp.path().join("ws-a");
        git(tmp.path(), &["clone", "-q", "--shared", source.to_str().unwrap(), ws_a.to_str().unwrap()]);
        git(&ws_a, &["config", "user.email", "t@t.t"]);
        git(&ws_a, &["config", "user.name", "t"]);
        write(&ws_a, "step1.txt", "work");
        let bc = boundary_commit(&ws_a, "wf(x): step1").await.unwrap();
        assert!(bc.committed);

        let refname = pin_step_ref(&ws_a, "exec-1").await.unwrap();
        ferry(&ws_a, &run_repo, &refname).await.unwrap();

        // The run repo now resolves the ferried ref to A's commit.
        let in_run = git::rev_parse(&run_repo, &refname).await.unwrap();
        assert_eq!(in_run, bc.head);
        assert_eq!(in_run, head(&ws_a));
    }

    /// §16 gitops transport: finalize pushes the run branch from the run repo
    /// with the step workspace deleted, proving durability doesn't depend on the
    /// disposable workspaces.
    #[tokio::test]
    async fn finalize_pushes_from_run_repo_after_workspace_deletion() {
        let tmp = tempfile::tempdir().unwrap();

        // A bare "remote" the source points `origin` at, so the run repo's
        // rewritten origin targets it and finalize's push has somewhere to land.
        let bare = tmp.path().join("origin.git");
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare", "-b", "main"]);

        let source = tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        init_repo(&source);
        write(&source, "README", "base");
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "base"]);
        git(&source, &["remote", "add", "origin", bare.to_str().unwrap()]);

        let run_dir = tmp.path().join("run");
        let run_repo = provision_run_repo(&source, &run_dir).await.unwrap();

        let ws_a = tmp.path().join("ws-a");
        git(tmp.path(), &["clone", "-q", "--shared", source.to_str().unwrap(), ws_a.to_str().unwrap()]);
        git(&ws_a, &["config", "user.email", "t@t.t"]);
        git(&ws_a, &["config", "user.name", "t"]);
        write(&ws_a, "step1.txt", "work");
        boundary_commit(&ws_a, "wf(x): step1").await.unwrap();
        let refname = pin_step_ref(&ws_a, "exec-1").await.unwrap();
        ferry(&ws_a, &run_repo, &refname).await.unwrap();
        let final_sha = head(&ws_a);

        // The workspace is now disposable.
        std::fs::remove_dir_all(&ws_a).unwrap();

        let outcome = finalize(&run_repo, &refname, "wf/test-abc", "main", "t", "b", false)
            .await
            .unwrap();
        assert!(outcome.pushed);

        // The branch landed on the bare remote at the ferried commit.
        let out = Command::new("git")
            .current_dir(&bare)
            .args(["rev-parse", "refs/heads/wf/test-abc"])
            .output()
            .unwrap();
        assert!(out.status.success(), "branch not pushed: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), final_sha);
    }

    #[test]
    fn run_branch_namespace_is_enforced() {
        assert!(is_run_branch("wf/feature-abc123"));
        assert!(!is_run_branch("main"));
        assert!(!is_run_branch("wf/../escape"));
        assert!(!is_run_branch("feature/wf-lookalike"));
    }
}

//! Git for a run (spec §12). Every run has one **run repository** — a
//! host-owned `--shared` clone of the user's repo under
//! `~/.fletch/runs/<id>/repo/` whose working tree is the run's single workspace:
//!
//! - each step's agent adopts that tree and works in it directly;
//! - each step's boundary commit is pinned there as `refs/wf/steps/<exec-id>`,
//!   the durable record of the step and the next step's starting point;
//! - finalize pushes the run branch from it, guarded to the `wf/` namespace.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::git;
use crate::sandbox::provision;

/// The run repository lives beside the blackboard, under the run directory.
pub fn run_repo_path(run_dir: &Path) -> PathBuf {
    run_dir.join("repo")
}

/// The ref a step's boundary commit is pinned as in the run repo.
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
/// hit it (not the local source path). Idempotent — launch provisions it and the
/// kernel provisions it again when it starts driving, relinked first to where the
/// source is now in case the user moved it since.
pub async fn provision_run_repo(source_repo: &Path, run_dir: &Path) -> Result<PathBuf> {
    let dest = run_repo_path(run_dir);
    if dest.join(".git").exists() {
        if let Err(e) = provision::relink_alternates(&dest, source_repo).await {
            tracing::warn!(run_repo = %dest.display(), error = %e, "relinking a run repo's objects failed");
        }
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
    // The pieces of the checkout pipeline (`sandbox::provision::fixups`) a run
    // repo needs: origin and identity, so finalize's push and the host-side
    // boundary commits work on a machine with no global gitconfig. Not the
    // rest, which would change what a run repo has always been: no delegation
    // hooks (no agent mailbox listens there), no signing freeze (the host's own
    // boundary commits keep the user's signing config), and its refs stay as
    // cloned.
    provision::rewrite_origin(source_repo, &dest).await?;
    provision::seed_identity(source_repo, &dest).await?;
    // The step agents adopt this tree and commit in it directly, so it needs
    // the same attribution hook as an agent workspace.
    crate::attribution::install_hook(&dest).await?;
    Ok(dest)
}

/// Current HEAD of a checkout.
pub async fn head_sha(checkout: &Path) -> Result<String> {
    git::rev_parse(checkout, "HEAD").await
}

/// The result of a boundary commit.
pub struct BoundaryCommit {
    pub head: String,
    /// Whether the boundary actually produced a commit (the tree was dirty).
    /// Asserted in tests; production only cares about `head`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub committed: bool,
}

/// Stage everything and commit if the tree is dirty; always return the resulting
/// HEAD. Runs host-side on the run workspace (it has a seeded identity from
/// provisioning).
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

/// Pin the workspace's current HEAD as `refs/wf/steps/<step-exec-id>`, so the
/// step's result stays addressable after later steps move HEAD on. Returns the
/// ref name.
pub async fn pin_step_ref(checkout: &Path, step_exec_id: &str) -> Result<String> {
    let refname = step_ref(step_exec_id);
    git::run_git(checkout, &["update-ref", &refname, "HEAD"], "pin step ref").await?;
    Ok(refname)
}

/// The result of finalizing a run.
pub struct FinalizeOutcome {
    /// Whether the run branch was pushed. Asserted in tests; production reads
    /// `branch`/`pr_url`/`pr_error`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub pushed: bool,
    pub branch: String,
    pub pr_url: Option<String>,
    /// The PR's number, from the same `PrState` the URL came off. Carried
    /// beside the URL (rather than re-parsed from it downstream) so the run row
    /// can record the PR's identity as GitHub reported it — see migration 0029.
    pub pr_number: Option<i64>,
    pub pr_error: Option<String>,
}

/// What [`finalize`] publishes.
#[derive(Clone, Copy)]
pub struct Finalize<'a> {
    pub run_repo: &'a Path,
    /// The user's repo the run was cloned from; decides and does the signing.
    pub source_repo: &'a Path,
    /// The pinned ref whose commit is pushed.
    pub final_ref: &'a str,
    /// The `wf/` run branch on origin.
    pub branch: &'a str,
    pub base: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub open_pr: bool,
}

/// Push the run branch to the origin from the **run repo** (§12.2) — HEAD is
/// moved to the final pinned ref first. The PR is best-effort (`pr_base`
/// threaded through from the spec's `finalize`). Push is guarded to the `wf/` namespace and goes
/// through the publication layer like every push: when `source_repo` (the
/// user's repo the run was cloned from) signs its commits, the run's commits
/// are published as signed copies, and `final_ref` is re-pinned to the copy
/// that went out so the run repo agrees with origin.
pub async fn finalize(f: &Finalize<'_>) -> Result<FinalizeOutcome> {
    let Finalize {
        run_repo,
        source_repo,
        final_ref,
        branch,
        base,
        title,
        body,
        open_pr,
    } = *f;
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
    let head = head_sha(run_repo).await?;
    let published = git::publish(
        &git::Publication {
            checkout: run_repo,
            source_repo,
            branch,
            force: false,
        },
        git::Tip::Commit(&head),
    )
    .await?;
    if published.tip != head {
        git::run_git(
            run_repo,
            &["update-ref", final_ref, &published.tip, &head],
            "re-pin final ref to its signed copy",
        )
        .await?;
        git::run_git(
            run_repo,
            &["checkout", "--detach", &published.tip],
            "checkout signed final ref",
        )
        .await?;
    }

    let (pr_url, pr_number, pr_error) = if open_pr {
        match crate::github::pr_create_head(run_repo, branch, title, body, base).await {
            Ok(pr) => (Some(pr.url), Some(i64::from(pr.number)), None),
            Err(e) => (None, None, Some(e.to_string())),
        }
    } else {
        (None, None, None)
    };
    Ok(FinalizeOutcome {
        pushed: true,
        branch: branch.to_string(),
        pr_url,
        pr_number,
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
        crate::git::testing::git(dir, args);
    }

    /// The user's repo of these runs. It does not sign: finalize publishes
    /// through `git::publish`, which reads this repo's policy, and a global
    /// `commit.gpgsign=true` with a key the test process cannot reach would
    /// otherwise fail the push. Signing is `git::publish`'s own tests' job.
    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["config", "user.email", "t@t.t"]);
        git(dir, &["config", "user.name", "t"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// Step agents adopt the run repo inside the sandbox, so an SSH GitHub
    /// origin becomes HTTPS there, as in a sandbox clone.
    #[tokio::test]
    async fn run_repo_rewrites_an_ssh_github_origin_to_https() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        init_repo(&source);
        write(&source, "README", "base");
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "base"]);
        git(
            &source,
            &["remote", "add", "origin", "git@github.com:acme/widget.git"],
        );

        let run_repo = provision_run_repo(&source, &tmp.path().join("run"))
            .await
            .unwrap();
        let out = git::git_output(&run_repo, &["remote", "get-url", "origin"])
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "https://github.com/acme/widget.git"
        );
    }

    /// §16 gitops: a step's boundary commit, pinned in the run repo, is what
    /// finalize pushes as the run branch.
    #[tokio::test]
    async fn finalize_pushes_the_pinned_step_ref_from_the_run_repo() {
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
        git(
            &source,
            &["remote", "add", "origin", bare.to_str().unwrap()],
        );

        let run_repo = provision_run_repo(&source, &tmp.path().join("run"))
            .await
            .unwrap();
        write(&run_repo, "step1.txt", "work");
        let bc = boundary_commit(&run_repo, "wf(x): step1").await.unwrap();
        assert!(bc.committed);
        let refname = pin_step_ref(&run_repo, "exec-1").await.unwrap();
        // A later step moves HEAD on; finalize still pushes the pinned ref.
        git(&run_repo, &["checkout", "-q", "--detach", "HEAD~1"]);

        let outcome = finalize(&Finalize {
            run_repo: &run_repo,
            source_repo: &source,
            final_ref: &refname,
            branch: "wf/test-abc",
            base: "main",
            title: "t",
            body: "b",
            open_pr: false,
        })
        .await
        .unwrap();
        assert!(outcome.pushed);

        let out = Command::new("git")
            .current_dir(&bare)
            .args(["rev-parse", "refs/heads/wf/test-abc"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "branch not pushed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), bc.head);
    }

    #[test]
    fn run_branch_namespace_is_enforced() {
        assert!(is_run_branch("wf/feature-abc123"));
        assert!(!is_run_branch("main"));
        assert!(!is_run_branch("wf/../escape"));
        assert!(!is_run_branch("feature/wf-lookalike"));
    }
}

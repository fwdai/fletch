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
        git::run_git(
            dest,
            &["remote", "set-url", "origin", &url],
            "set-url origin",
        )
        .await?;
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
    git::run_git(
        run_repo,
        &["fetch", &src, &refspec],
        "ferry step ref into run repo",
    )
    .await?;
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

// ─────────────────────────── parallel merge (§12.3) ─────────────────────────
//
// A code-producing parallel stage (`integrate: merge`) merges each successful
// child's *ferried* ref into a stage accumulator **in the run repo** — the
// children's objects are already there, so no cross-workspace transport is
// needed. Merges run in a linked integration worktree of the run repo (kept in
// place while a conflict is paused so a human can resolve it in an editor, §12.3
// mode c). A conflicted merge is committed as a snapshot (a real merge commit
// whose tree still carries the conflict markers), so it is a single ref a
// resolution step can fork from (§12.3 mode a).

/// The integration worktree for the merge stage at `block_index`. It lives beside
/// the run repo (never inside it) so it can be opened directly for human
/// resolution, and torn down once the stage finalizes.
pub fn integration_worktree_path(run_dir: &Path, block_index: usize) -> PathBuf {
    run_dir.join(format!("integrate-{block_index}"))
}

/// The accumulator ref, pinned after every clean merge so stage progress survives
/// worktree teardown and an app restart.
pub fn merge_acc_ref(block_index: usize) -> String {
    format!("refs/wf/merge/{block_index}/acc")
}

/// The conflicted-merge snapshot ref (§12.3 mode a): a merge commit whose tree
/// still carries conflict markers, forkable by a resolution step.
pub fn merge_conflict_ref(block_index: usize) -> String {
    format!("refs/wf/merge/{block_index}/conflict")
}

/// Set up (or reset) the integration worktree at `base` — a detached linked
/// worktree of the run repo. Idempotent across resumes: an existing worktree is
/// cleared of any in-progress merge and hard-reset to `base`; a stale
/// registration whose directory is gone is pruned before a fresh add.
pub async fn setup_integration_worktree(run_repo: &Path, wt: &Path, base: &str) -> Result<()> {
    if wt.join(".git").exists() {
        let _ = git::merge_abort(wt).await;
        git::run_git(wt, &["reset", "--hard", base], "reset integration worktree").await?;
        git::run_git(wt, &["clean", "-fdq"], "clean integration worktree").await?;
        return Ok(());
    }
    // A previous run of this stage may have left a registration behind.
    git::worktree_prune(run_repo).await?;
    let wt_s = path_str(wt)?;
    git::run_git(
        run_repo,
        &["worktree", "add", "--detach", &wt_s, base],
        "add integration worktree",
    )
    .await?;
    Ok(())
}

/// The result of merging one child ref into the integration worktree.
pub enum MergeResult {
    /// Merged without conflicts; `head` is the new accumulator HEAD.
    Clean { head: String },
    /// Conflicted. The conflicted merge is committed as a snapshot (`head`) and
    /// `files` lists the paths git left with conflict markers.
    Conflict { head: String, files: Vec<String> },
}

/// Merge `child_ref` into the integration worktree (a `--no-ff` merge, so every
/// child yields one observable merge commit). A clean merge returns the new HEAD.
/// A conflict is committed as a snapshot commit (markers preserved in the tree)
/// and its file list returned; a non-conflict merge failure is surfaced as an
/// error rather than a fake conflict.
pub async fn merge_child(wt: &Path, child_ref: &str, message: &str) -> Result<MergeResult> {
    let out = git::git_output(wt, &["merge", "--no-ff", "-m", message, child_ref]).await?;
    if out.status.success() {
        return Ok(MergeResult::Clean {
            head: head_sha(wt).await?,
        });
    }
    let files = conflicted_files(wt).await?;
    if files.is_empty() {
        let _ = git::merge_abort(wt).await;
        return Err(Error::Other(format!(
            "merge of {child_ref} failed without conflicts: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    // Staging resolves the unmerged index entries (keeping the marker text), so
    // the in-progress merge can be committed as a single forkable snapshot.
    git::run_git(wt, &["add", "-A"], "stage conflicted merge").await?;
    git::run_git(
        wt,
        &["commit", "-m", &format!("{message} [conflict]")],
        "commit conflict snapshot",
    )
    .await?;
    Ok(MergeResult::Conflict {
        head: head_sha(wt).await?,
        files,
    })
}

/// Paths git left with conflict markers (unmerged index entries).
async fn conflicted_files(wt: &Path) -> Result<Vec<String>> {
    let out = git::git_output(wt, &["diff", "--name-only", "--diff-filter=U"]).await?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Whether a checkout has no uncommitted changes (staged, unstaged, or
/// untracked). Guards human conflict resolution (§12.3 mode c): an uncommitted
/// tree must not be silently reset away when the merge continues.
pub async fn is_worktree_clean(wt: &Path) -> Result<bool> {
    let out = git::git_output(wt, &["status", "--porcelain"]).await?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().is_empty())
}

/// Pin a checkout's current HEAD as `refname` in the shared ref store (linked
/// worktrees share the run repo's ref db and object store).
pub async fn pin_ref(wt: &Path, refname: &str) -> Result<()> {
    git::run_git(wt, &["update-ref", refname, "HEAD"], "pin ref").await?;
    Ok(())
}

/// Remove the integration worktree once the stage has finalized (its durable
/// state now lives in the run repo's refs). Best-effort — a leftover worktree is
/// harmless and pruned on the next stage.
pub async fn remove_integration_worktree(run_repo: &Path, wt: &Path) {
    let _ = git::worktree_remove(run_repo, wt, true).await;
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
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                "--shared",
                source.to_str().unwrap(),
                ws_a.to_str().unwrap(),
            ],
        );
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
        git(
            &source,
            &["remote", "add", "origin", bare.to_str().unwrap()],
        );

        let run_dir = tmp.path().join("run");
        let run_repo = provision_run_repo(&source, &run_dir).await.unwrap();

        let ws_a = tmp.path().join("ws-a");
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                "--shared",
                source.to_str().unwrap(),
                ws_a.to_str().unwrap(),
            ],
        );
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
        assert!(
            out.status.success(),
            "branch not pushed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), final_sha);
    }

    #[test]
    fn run_branch_namespace_is_enforced() {
        assert!(is_run_branch("wf/feature-abc123"));
        assert!(!is_run_branch("main"));
        assert!(!is_run_branch("wf/../escape"));
        assert!(!is_run_branch("feature/wf-lookalike"));
    }

    /// Ferry two children editing disjoint files into the run repo, then merge
    /// both into an integration worktree: clean merges, both files present.
    #[tokio::test]
    async fn merge_children_cleanly_integrates_disjoint_work() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        init_repo(&source);
        write(&source, "README", "base\n");
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "base"]);

        let run_dir = tmp.path().join("run");
        let run_repo = provision_run_repo(&source, &run_dir).await.unwrap();
        let base = git::rev_parse(&run_repo, "HEAD").await.unwrap();

        // Two children forking the same base, each adding its own file.
        let mut child_refs = Vec::new();
        for c in ["a", "b"] {
            let ws = tmp.path().join(format!("ws-{c}"));
            git(
                tmp.path(),
                &[
                    "clone",
                    "-q",
                    "--shared",
                    source.to_str().unwrap(),
                    ws.to_str().unwrap(),
                ],
            );
            git(&ws, &["config", "user.email", "t@t.t"]);
            git(&ws, &["config", "user.name", "t"]);
            write(&ws, &format!("{c}.txt"), c);
            boundary_commit(&ws, &format!("child {c}")).await.unwrap();
            let refname = pin_step_ref(&ws, &format!("exec-{c}")).await.unwrap();
            ferry(&ws, &run_repo, &refname).await.unwrap();
            child_refs.push(refname);
        }

        let wt = integration_worktree_path(&run_dir, 0);
        setup_integration_worktree(&run_repo, &wt, &base)
            .await
            .unwrap();
        for r in &child_refs {
            match merge_child(&wt, r, "merge child").await.unwrap() {
                MergeResult::Clean { .. } => {}
                MergeResult::Conflict { .. } => panic!("unexpected conflict"),
            }
        }
        assert!(wt.join("a.txt").exists());
        assert!(wt.join("b.txt").exists());
    }

    /// Two children editing the *same* line conflict on the second merge; the
    /// snapshot is committed, markers survive in the tree, and the file list is
    /// reported.
    #[tokio::test]
    async fn merge_children_conflict_is_committed_and_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        init_repo(&source);
        write(&source, "f.txt", "base\n");
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "base"]);

        let run_dir = tmp.path().join("run");
        let run_repo = provision_run_repo(&source, &run_dir).await.unwrap();
        let base = git::rev_parse(&run_repo, "HEAD").await.unwrap();

        let mut child_refs = Vec::new();
        for c in ["a", "b"] {
            let ws = tmp.path().join(format!("ws-{c}"));
            git(
                tmp.path(),
                &[
                    "clone",
                    "-q",
                    "--shared",
                    source.to_str().unwrap(),
                    ws.to_str().unwrap(),
                ],
            );
            git(&ws, &["config", "user.email", "t@t.t"]);
            git(&ws, &["config", "user.name", "t"]);
            write(&ws, "f.txt", &format!("{c}-change\n"));
            boundary_commit(&ws, &format!("child {c}")).await.unwrap();
            let refname = pin_step_ref(&ws, &format!("exec-{c}")).await.unwrap();
            ferry(&ws, &run_repo, &refname).await.unwrap();
            child_refs.push(refname);
        }

        let wt = integration_worktree_path(&run_dir, 0);
        setup_integration_worktree(&run_repo, &wt, &base)
            .await
            .unwrap();
        // First merge is clean (fast-forward-free --no-ff commit).
        assert!(matches!(
            merge_child(&wt, &child_refs[0], "merge a").await.unwrap(),
            MergeResult::Clean { .. }
        ));
        // Second conflicts on the same line.
        match merge_child(&wt, &child_refs[1], "merge b").await.unwrap() {
            MergeResult::Conflict { files, .. } => {
                assert_eq!(files, vec!["f.txt".to_string()]);
                let body = std::fs::read_to_string(wt.join("f.txt")).unwrap();
                assert!(
                    body.contains("<<<<<<<"),
                    "markers preserved in the snapshot"
                );
                // The snapshot is a committed, forkable state.
                pin_ref(&wt, &merge_conflict_ref(0)).await.unwrap();
                assert!(git::rev_parse(&run_repo, &merge_conflict_ref(0))
                    .await
                    .is_ok());
            }
            MergeResult::Clean { .. } => panic!("expected a conflict"),
        }
    }
}

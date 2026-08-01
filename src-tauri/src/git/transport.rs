//! Network and mutation transport: push/pull/fetch, rebase, commit, stash,
//! merge-abort, remote add, branch delete, and last-commit-message.

use std::path::Path;

use crate::error::{Error, Result};

use super::cmd::{git_output, git_output_env, identity_env, merge_git_env, output_timed, run_git};

/// Push the detached `HEAD` to `refs/heads/<branch>` on `origin`, creating the
/// remote branch without needing a local one (so it never collides with a
/// linked worktree's checkout). Same auth as [`push`], so the https transport
/// authenticates with the app's token; a workspace `pre-push` hook cannot fire
/// because hooks are neutralised at the spawn seam (`git::hardening`).
pub async fn push_head_to_branch(checkout: &Path, branch: &str) -> Result<()> {
    // A push over ssh or a local path runs the transport-executing config keys
    // `remote.<origin>.receivepack`, `core.sshCommand` and `core.gitProxy` — which
    // `config_overrides` deliberately omits (they carry a user's real auth), so
    // neither hardening layer neutralises them for push. Refuse a steerable
    // checkout here, before the transport runs a planted one. Scoped to
    // `checkouts_root`, so a push against a non-agent target is a no-op.
    super::hardening::refuse_steerable_config(checkout).await?;
    let mut cmd = crate::git_dist::command(checkout);
    cmd.args(["push", "origin", &format!("HEAD:refs/heads/{branch}")]);
    for (k, v) in crate::github::git_auth_env() {
        cmd.env(k, v);
    }
    let out = output_timed(&mut cmd, "git push").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "push failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Push the current branch to `origin`. Uses `-u` to set the upstream
/// tracking ref on the first push.
/// When `force` is set, pushes with `--force-with-lease --force-if-includes`:
/// the safe force that rewrites the remote branch (e.g. after a local rebase)
/// but refuses if the remote moved in a way we haven't seen, so we never
/// silently clobber someone else's work. `--force-if-includes` is essential
/// here, not redundant: bare `--force-with-lease` only compares the *local*
/// `refs/remotes/origin/<branch>` tracking ref, which git also opportunistically
/// refreshes from the push's own ref advertisement — so a stale or never-fetched
/// tracking ref (the clone-workspace case, where only the base branch is
/// refreshed) would pass the lease and overwrite unseen commits. `--force-if-
/// includes` additionally requires the remote tip to be reachable from the local
/// branch's reflog — proof we actually integrated it — and fails safe otherwise.
/// A plain `--force` is intentionally not offered.
/// Returns `"up-to-date"` when the remote already had everything (a no-op
/// push), otherwise `"pushed"`. Lets the UI confirm the outcome instead of
/// silently doing nothing when there was nothing to send.
///
/// `branch` is resolved host-side from the agent-writable `.git/HEAD`, so it is
/// pushed as a fully-qualified `refs/heads/<branch>:refs/heads/<branch>`
/// refspec, NEVER as a bare `origin <branch>` positional. As a bare positional
/// an option-named branch (`--mirror`, `--all`, `--force`) is reparsed by git as
/// a push *option* — mirror-force-pushing every ref and clobbering protected
/// `main`/`master`/base. Inside a refspec the string can only ever be a
/// source/destination branch name, so such a name becomes a harmless same-named
/// remote branch (or git rejects it) and can never trigger option behaviour or
/// redirect to a trunk. This primitive is the single choke point behind every
/// push caller (agent broker, PR broker, Git panel, publish), so the defence
/// lives here.
pub async fn push(checkout: &Path, branch: &str, force: bool) -> Result<String> {
    // See `push_head_to_branch`: push executes `remote.<origin>.receivepack` /
    // `core.sshCommand` / `core.gitProxy` over ssh/local transport, transport keys
    // that neither hardening layer neutralises, so the refusal runs before we spawn.
    super::hardening::refuse_steerable_config(checkout).await?;
    let mut cmd = crate::git_dist::command(checkout);
    cmd.args(["push", "-u"]);
    if force {
        cmd.args(["--force-with-lease", "--force-if-includes"]);
    }
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    cmd.args(["origin", refspec.as_str()]);
    // Auth for the https transport. `pre-push` would fire on the host, but
    // hooks are already neutralised at the spawn seam (`git::hardening`).
    for (k, v) in crate::github::git_auth_env() {
        cmd.env(k, v);
    }
    let out = output_timed(&mut cmd, "git push").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "push failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    // git reports "Everything up-to-date" on stderr when there was nothing new.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    if combined.contains("Everything up-to-date") {
        Ok("up-to-date".to_string())
    } else {
        Ok("pushed".to_string())
    }
}

/// Pull latest from the tracking remote branch.
/// Requires `push -u` to have been called first to establish an upstream.
pub async fn pull(checkout: &Path) -> Result<()> {
    // A pull merges, so it runs `merge.<name>.driver` — a wildcard key the `-c`
    // overrides cannot neutralise by name. This builds its command directly, so it
    // needs the config guard explicitly, the same as `push` above (which runs the
    // transport-executing keys) and every other direct-build path on an agent
    // checkout.
    super::hardening::refuse_steerable_config(checkout).await?;
    let mut cmd = crate::git_dist::command(checkout);
    cmd.args(["pull"]);
    // Auth for the https transport; identity because a pull may create a merge
    // commit. Merge so the auth `GIT_CONFIG_*` set and the identity vars don't
    // clobber each other. Merge hooks are neutralised at the spawn seam.
    for (k, v) in merge_git_env(&[
        &crate::github::git_auth_env(),
        &identity_env(checkout).await,
    ]) {
        cmd.env(k, v);
    }
    let out = output_timed(&mut cmd, "git pull").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "pull failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Fetch a single base branch on a project's SOURCE repo from its `origin`
/// (GitHub), authenticating the https transport with the app token. Updates the
/// source's `refs/remotes/origin/<base>` and lands the new base commits in the
/// object store every `--shared` agent clone borrows — so one fetch here makes
/// a moved base measurable from every checkout without each clone fetching.
///
/// Background, best-effort: hook-disabling env is applied (the source repo may
/// carry its own hooks), the token is never logged, and any failure is returned
/// for the caller to log-and-skip rather than surface. No-op-ish when the source
/// has no `origin` — git simply fails, which the caller swallows.
pub async fn fetch_base(source_repo: &Path, base: &str) -> Result<()> {
    // No `refuse_steerable_config` here: this fetch runs on the user-owned *source*
    // repo, which sits outside `checkouts_root` and is not agent-writable, so the
    // (scoped) refusal would be a no-op. The transport-key guard belongs on the
    // agent-checkout fetch paths (`fetch_fork_point`, `provision::fetch`), not here.
    let mut cmd = crate::git_dist::command(source_repo);
    cmd.args(["fetch", "--quiet", "origin", base]);
    for (k, v) in crate::github::git_auth_env() {
        cmd.env(k, v);
    }
    let out = output_timed(&mut cmd, "git fetch base").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "fetch base failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Resolve a source repo's remote-tracking base tip
/// (`refs/remotes/origin/<base>`) — the freshest base a `--shared` clone can
/// measure staleness against, since the clone shares this repo's objects but
/// not its refs. `None` when the ref is absent (no origin, or never fetched).
pub async fn remote_base_sha(source_repo: &Path, base: &str) -> Option<String> {
    let out = git_output(
        source_repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/origin/{base}"),
        ],
    )
    .await
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Rebase the current branch onto `base` (e.g. "main"). Used by the clean-state
/// panel action to bring the checkout up to date with its base branch when the
/// base has moved ahead. Aborts the rebase on conflict so the checkout is never
/// left mid-rebase — the caller surfaces the error.
pub async fn rebase_onto(checkout: &Path, base: &str) -> Result<()> {
    // Rebasing rewrites commits, which needs a committer identity. Its
    // `pre-rebase`/`post-rewrite` hooks are neutralised at the spawn seam.
    let env = identity_env(checkout).await;
    let out = git_output_env(checkout, &["rebase", base], &env).await?;
    if !out.status.success() {
        let conflict = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // Don't leave the checkout mid-rebase. `rebase --abort` checks out the
        // original HEAD, firing `post-checkout` — neutralised at the spawn seam
        // like every other invocation. If the abort *itself* fails or times out,
        // the checkout is stuck mid-rebase and needs manual recovery — surface
        // that alongside the original conflict rather than silently swallowing it
        // and reporting only the conflict.
        if let Err(abort_err) = run_git(checkout, &["rebase", "--abort"], "rebase --abort").await {
            return Err(Error::Git(format!(
                "rebase onto {base} failed: {conflict}; the checkout is left \
                 mid-rebase because cleanup also failed ({abort_err}) — run \
                 `git rebase --abort` manually"
            )));
        }
        return Err(Error::Git(format!("rebase onto {base} failed: {conflict}")));
    }
    Ok(())
}

/// Stage all working-tree changes (including untracked) and create a commit.
/// Errors if there is nothing to commit or if git is unhappy.
pub async fn commit(checkout: &Path, message: &str) -> Result<()> {
    run_git(checkout, &["add", "-A"], "add -A").await?;
    let env = identity_env(checkout).await;
    let out = git_output_env(checkout, &["commit", "-m", message], &env).await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "commit failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Discard every uncommitted change in the working tree, including
/// untracked files and directories. Equivalent to a hard reset plus a
/// `clean -fd`. Destructive — caller is responsible for confirming.
pub async fn discard_all(checkout: &Path) -> Result<()> {
    run_git(checkout, &["reset", "--hard", "HEAD"], "reset --hard").await?;
    run_git(checkout, &["clean", "-fd"], "clean -fd").await?;
    Ok(())
}

/// Stash all working-tree changes including untracked files. No message —
/// git generates the default "WIP on <branch>" label.
pub async fn stash_push(checkout: &Path) -> Result<()> {
    run_git(
        checkout,
        &["stash", "push", "--include-untracked"],
        "stash push",
    )
    .await?;
    Ok(())
}

/// Abort an in-progress merge, restoring the pre-merge working tree.
pub async fn merge_abort(checkout: &Path) -> Result<()> {
    run_git(checkout, &["merge", "--abort"], "merge --abort").await?;
    Ok(())
}

/// Add a remote. Used when publishing a fresh local repo to GitHub.
pub async fn remote_add(checkout: &Path, name: &str, url: &str) -> Result<()> {
    run_git(
        checkout,
        &["remote", "add", name, url],
        &format!("remote add {name}"),
    )
    .await?;
    Ok(())
}

/// Subject and body of the checkout's last commit — the source for a PR's
/// title/body when the caller didn't supply one (what `gh pr create --fill`
/// did).
pub async fn last_commit_message(checkout: &Path) -> Result<(String, String)> {
    let out = run_git(checkout, &["log", "-1", "--format=%s%n%b"], "log -1").await?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let subject = lines.next().unwrap_or("").trim().to_string();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    Ok((subject, body))
}

/// Force-delete a local branch. Returns Ok even if the branch never
/// existed in the first place — that's exactly the state the caller
/// usually wants to converge on. Errors only for genuine git failures
/// (e.g. branch checked out in another live checkout).
pub async fn branch_delete(repo: &Path, branch: &str) -> Result<()> {
    let out = git_output(repo, &["branch", "-D", branch]).await?;
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

#[cfg(test)]
mod tests {
    use super::super::branch::{checkout_new_branch, current_branch};
    use super::super::worktree::{commit_all, init_repo};
    use super::*;
    use tokio::process::Command;

    async fn config(repo: &Path, key: &str, val: &str) {
        let out = Command::new("git")
            .current_dir(repo)
            .args(["config", key, val])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
    }

    async fn git_ok(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    async fn rev(dir: &Path, refname: &str) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", refname])
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "rev-parse {refname}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The agent owns its `.git/HEAD`/`refs`, so the `branch` string reaching
    /// `push` can be an option-looking name (`--mirror`, `--all`). Pushing a
    /// fully-qualified refspec must make such a name only ever a destination
    /// branch — never a git-push OPTION that mirror-force-pushes every ref over
    /// protected `main`. Reproduces the real injection: the ref + HEAD are
    /// written directly, and `push` is driven with the resolved option string.
    #[tokio::test]
    async fn push_refspec_neutralizes_option_named_branch_injection() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        let remote = root.join("remote.git");
        let attacker = root.join("attacker");

        // Bare remote seeded with a protected `main`.
        git_ok(root, &["init", "-q", "--bare", "remote.git"]).await;
        let seed = root.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        git_ok(&seed, &["init", "-q", "-b", "main"]).await;
        config(&seed, "user.email", "v@example.com").await;
        config(&seed, "user.name", "V").await;
        std::fs::write(seed.join("main.txt"), b"legit main\n").unwrap();
        git_ok(&seed, &["add", "-A"]).await;
        git_ok(&seed, &["commit", "-q", "-m", "protected main"]).await;
        git_ok(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .await;
        git_ok(&seed, &["push", "-q", "-u", "origin", "main"]).await;
        let protected = rev(&remote, "refs/heads/main").await;

        // Attacker checkout: an agent clone that fully controls its own `.git`.
        git_ok(
            root,
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                attacker.to_str().unwrap(),
            ],
        )
        .await;
        config(&attacker, "user.email", "a@example.com").await;
        config(&attacker, "user.name", "A").await;
        std::fs::write(attacker.join("main.txt"), b"PWNED by attacker\n").unwrap();
        git_ok(&attacker, &["add", "-A"]).await;
        git_ok(&attacker, &["commit", "-q", "-m", "attacker payload"]).await;
        let payload = rev(&attacker, "HEAD").await;

        for opt in ["--mirror", "--all"] {
            // Plant the option-named branch and point HEAD at it directly —
            // `.git/HEAD` and `.git/refs` are outside the config deny list.
            std::fs::write(
                attacker.join(format!(".git/refs/heads/{opt}")),
                format!("{payload}\n"),
            )
            .unwrap();
            std::fs::write(
                attacker.join(".git/HEAD"),
                format!("ref: refs/heads/{opt}\n"),
            )
            .unwrap();
            // `current_branch` resolves to the injected option string, exactly
            // as the brokers would hand it to `push`.
            assert_eq!(
                current_branch(&attacker).await.unwrap().as_deref(),
                Some(opt),
                "the injected option name must be what a broker would push"
            );

            // Drive the real primitive both ways. Neither may mirror or redirect:
            // as a refspec `{opt}` is only ever a plain destination branch.
            push(&attacker, opt, false).await.unwrap();
            push(&attacker, opt, true).await.unwrap();

            // Protected main is untouched; the attack achieves at most a harmless
            // same-named remote branch pointing at the attacker's own commit.
            assert_eq!(
                rev(&remote, "refs/heads/main").await,
                protected,
                "protected main must be untouched by an option-named push of {opt:?}"
            );
            assert_eq!(
                rev(&remote, &format!("refs/heads/{opt}")).await,
                payload,
                "{opt:?} must land only as a literal same-named branch"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn conflicting_rebase_runs_no_workspace_hooks_and_leaves_no_rebase_state() {
        use std::os::unix::fs::PermissionsExt;

        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;

        // Base commit; remember the starting branch to rebase onto.
        std::fs::write(repo.join("a.txt"), b"base\n").unwrap();
        commit_all(repo, "base").await.unwrap();
        let base = current_branch(repo).await.unwrap().unwrap();

        // Feature branch edits a.txt one way…
        checkout_new_branch(repo, "feature").await.unwrap();
        std::fs::write(repo.join("a.txt"), b"feature\n").unwrap();
        commit_all(repo, "feature edit").await.unwrap();

        // …while base advances with a conflicting edit to the same line.
        config(repo, "checkout.quiet", "true").await; // keep output tidy
        let out = Command::new("git")
            .current_dir(repo)
            .args(["checkout", &base])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        std::fs::write(repo.join("a.txt"), b"base advanced\n").unwrap();
        commit_all(repo, "base edit").await.unwrap();
        let out = Command::new("git")
            .current_dir(repo)
            .args(["checkout", "feature"])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());

        // Plant a hostile post-checkout hook. The rebase checks out the base to
        // start replaying (fires post-checkout) and, on conflict, aborts — both
        // host-side and both must run with workspace hooks disabled.
        let sentinel = td.path().join("hook-ran");
        let hook = repo.join(".git/hooks/post-checkout");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\n", sentinel.display()),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Rebase conflicts on a.txt → rebase_onto aborts internally and errs.
        let err = rebase_onto(repo, &base).await.unwrap_err();
        assert!(err.to_string().contains("rebase onto"), "got: {err}");

        // No workspace hook ran during any host-side step of the rebase.
        assert!(
            !sentinel.exists(),
            "workspace post-checkout hook must not run"
        );
        // And the abort left the checkout clean, not mid-rebase.
        assert!(!repo.join(".git/rebase-merge").exists());
        assert!(!repo.join(".git/rebase-apply").exists());
    }
}

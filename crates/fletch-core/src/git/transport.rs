//! Network and mutation transport: push/pull/fetch, rebase, commit, stash,
//! merge-abort, remote add, branch delete, and last-commit-message.

use std::path::Path;

use crate::error::{Error, Result};

use super::cmd::{git_output, git_output_env, identity_env, merge_git_env, output_timed, run_git};

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
#[cfg(test)]
pub(super) async fn push(
    checkout: &Path,
    source_repo: &Path,
    branch: &str,
    force: bool,
) -> Result<String> {
    let source = format!("refs/heads/{branch}");
    push_source(checkout, source_repo, &source, branch, force).await
}

/// [`push`] of one exact commit to `refs/heads/<branch>`: the commit a check
/// just read, so nothing that lands on the branch between the check and the
/// push is published. The lease applies as it does for `push`. The push names
/// a URL, not a remote, so git sets no upstream; the tracking ref is set
/// afterwards. Private to `git`: [`super::publish`] is the one caller, so no
/// push can skip what publishing involves.
pub(super) async fn push_commit(
    checkout: &Path,
    source_repo: &Path,
    commit: &str,
    branch: &str,
    force: bool,
) -> Result<String> {
    let summary = push_source(checkout, source_repo, commit, branch, force).await?;
    // Best effort, as `-u` would be: a local branch that no longer exists has
    // nothing to track, and the push already happened.
    let upstream = format!("origin/{branch}");
    let _ = super::cmd::git_output(
        checkout,
        &["branch", "--set-upstream-to", &upstream, branch],
    )
    .await;
    Ok(summary)
}

/// The push names the remote as a **URL** (`git::remote`), never as `origin`,
/// so the checkout's config cannot redirect it. Git then keeps no
/// remote-tracking ref for it, so what `push origin` would have done through
/// `remote.origin.*` is done here explicitly, with the same safety:
///
/// - the lease carries the push destination's last known tip
///   (`--force-with-lease=<branch>:<expected>`, "absent" when there is none),
///   which is what a bare `--force-with-lease` reads for a named remote — and
///   `--force-if-includes`, a no-op once the expected value is explicit, is
///   [`require_integrated`];
/// - `refs/remotes/origin/<branch>` is moved to the pushed commit afterwards,
///   as a named-remote push would move it.
///
/// That last known tip is `refs/remotes/origin/<branch>` when pushes go where
/// fetches come from. With a `pushurl` (a fork) it is not: `origin/*` is the
/// fetch remote's view, refreshed and pruned from there before a signed
/// publish, and never holds a branch that exists on the fork alone. The fork's
/// tip is kept apart, under `refs/fletch/pushed/<branch>`, and before a force
/// it is refreshed from the fork itself — the lease then holds exactly what
/// the fork has, and [`require_integrated`] is what refuses a tip nobody here
/// has looked at.
async fn push_source(
    checkout: &Path,
    source_repo: &Path,
    source: &str,
    branch: &str,
    force: bool,
) -> Result<String> {
    // See `push_head_to_branch`: push executes `remote.<origin>.receivepack` /
    // `core.sshCommand` / `core.gitProxy` over ssh/local transport, transport keys
    // that neither hardening layer neutralises, so the refusal runs before we spawn.
    super::hardening::refuse_steerable_config(checkout).await?;
    let url = super::remote::trusted_remote(checkout, source_repo).await?;
    let tracking = format!("refs/remotes/origin/{branch}");
    let triangular = url.push != url.fetch;
    let lease_ref = if triangular {
        format!("refs/fletch/pushed/{branch}")
    } else {
        tracking.clone()
    };
    if force && triangular {
        refresh_push_tip(checkout, &url.push, branch, &lease_ref).await?;
    }
    let expected = resolve(checkout, &lease_ref).await?;
    let pushed = resolve(checkout, source).await?;
    let mut cmd = crate::git_dist::command(checkout);
    cmd.args(["push"]);
    if force {
        // As git does, a push that changes nothing is up to date, not forced.
        if let Some(expected) = expected
            .as_deref()
            .filter(|e| Some(*e) != pushed.as_deref())
        {
            require_integrated(checkout, branch, expected).await?;
        }
        cmd.arg(format!(
            "--force-with-lease=refs/heads/{branch}:{}",
            expected.as_deref().unwrap_or_default()
        ));
    }
    let refspec = format!("{source}:refs/heads/{branch}");
    cmd.args([url.push.as_str(), refspec.as_str()]);
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
    if let Some(pushed) = pushed {
        for refname in std::iter::once(&tracking).chain(triangular.then_some(&lease_ref)) {
            run_git(
                checkout,
                &["update-ref", refname, &pushed],
                "update remote-tracking ref",
            )
            .await?;
        }
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

/// `lease_ref` set to `branch` as the push remote at `push_url` has it right
/// now, or deleted when the remote has no such branch — so a lease built on
/// it expects exactly the remote's current state. The config refusal has
/// already run for this checkout.
async fn refresh_push_tip(
    checkout: &Path,
    push_url: &str,
    branch: &str,
    lease_ref: &str,
) -> Result<()> {
    let mut cmd = crate::git_dist::command(checkout);
    cmd.args([
        "fetch",
        "--quiet",
        push_url,
        &format!("+refs/heads/{branch}:{lease_ref}"),
    ]);
    super::cmd::apply_github_auth(&mut cmd);
    let out = output_timed(&mut cmd, "git fetch push tip").await?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("couldn't find remote ref") {
        let _ = git_output(checkout, &["update-ref", "-d", lease_ref]).await;
        return Ok(());
    }
    Err(Error::Git(format!(
        "could not read '{branch}' from the push remote before forcing: {}",
        stderr.trim()
    )))
}

/// `rev` as a commit, or `None` when it names nothing.
async fn resolve(checkout: &Path, rev: &str) -> Result<Option<String>> {
    let spec = format!("{rev}^{{commit}}");
    let out = git_output(checkout, &["rev-parse", "--verify", "--quiet", &spec]).await?;
    Ok(out
        .status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

/// What `--force-if-includes` checks, for a lease with an explicit value
/// (where git skips it): the remote tip the lease expects, `expected`, must be
/// reachable from one of the local branch's reflog entries — proof it was
/// integrated at some point, so a force cannot drop commits that a fetch
/// brought in but nobody looked at. A rewrite (an amend, a rebase) keeps the
/// pre-rewrite tip in the reflog, so it passes; a branch with no reflog has
/// integrated nothing and is refused.
async fn require_integrated(checkout: &Path, branch: &str, expected: &str) -> Result<()> {
    let refname = format!("refs/heads/{branch}");
    let out = git_output(checkout, &["log", "-g", "--format=%H", &refname]).await?;
    let entries: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let integrated = if entries.is_empty() {
        false
    } else {
        let mut args = vec!["rev-list", "-n", "1", expected, "--not"];
        args.extend(entries.iter().map(String::as_str));
        let out = git_output(checkout, &args).await?;
        out.status.success() && out.stdout.iter().all(u8::is_ascii_whitespace)
    };
    if integrated {
        return Ok(());
    }
    Err(Error::Git(format!(
        "push failed: ! [rejected] {branch} (stale info): origin/{branch} is at {expected}, \
         which this checkout has never integrated; fetch and rebase onto it first"
    )))
}

/// Pull the current branch's remote counterpart into it, from the user's
/// repository's remote (`git::remote`). A detached HEAD has no branch to pull.
pub async fn pull(checkout: &Path, source_repo: &Path) -> Result<()> {
    // A pull merges, so it runs `merge.<name>.driver` — a wildcard key the `-c`
    // overrides cannot neutralise by name. This builds its command directly, so it
    // needs the config guard explicitly, the same as `push` above (which runs the
    // transport-executing keys) and every other direct-build path on an agent
    // checkout.
    super::hardening::refuse_steerable_config(checkout).await?;
    let Some(branch) = super::branch::current_branch(checkout).await? else {
        return Err(Error::Git(
            "pull failed: HEAD is detached — no branch to pull into".into(),
        ));
    };
    let url = super::remote::trusted_remote(checkout, source_repo).await?;
    let mut cmd = crate::git_dist::command(checkout);
    cmd.args([
        "pull",
        &url.fetch,
        &super::remote::tracking_refspec(&branch),
    ]);
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

/// Make commit `sha` readable in a project's SOURCE repo: nothing to do when
/// its object store already has it, else one fetch of that commit from
/// `origin` as [`fetch_base`] fetches a branch (a merge made on GitHub is not
/// local until something fetches it).
pub async fn fetch_commit(source_repo: &Path, sha: &str) -> Result<()> {
    let object = format!("{sha}^{{commit}}");
    let have = git_output(source_repo, &["cat-file", "-e", &object])
        .await
        .is_ok_and(|out| out.status.success());
    if have {
        return Ok(());
    }
    fetch_base(source_repo, sha).await
}

/// Whether a branch named exactly `branch` exists on `origin` *right now*, asked
/// over the wire with the app's token. `Some(false)` when no such branch is
/// there; `None` when the question couldn't be answered at all (no remote,
/// transport failure, timeout), so a caller can tell "absent" from "don't know"
/// and a flaky network never becomes a false refusal.
///
/// Exact, not pattern: a glob (`feat/*`) is reported absent even though git
/// happily matches it, because the caller is validating a literal branch name.
///
/// Deliberately not a ref lookup: reading `refs/remotes/origin/<branch>` (as
/// [`super::resolve_base`] does) reports any branch created since the last fetch
/// as missing — fine for measuring staleness, wrong for a pre-flight check on a
/// branch name a request just supplied.
pub async fn remote_branch_exists(
    checkout: &Path,
    source_repo: &Path,
    branch: &str,
) -> Option<bool> {
    // Same reasoning as `push_head_to_branch`: this runs the transport, so a
    // steerable config in an agent-writable checkout is refused before git does.
    super::hardening::refuse_steerable_config(checkout)
        .await
        .ok()?;
    let url = super::remote::trusted_remote(checkout, source_repo)
        .await
        .ok()?;
    let mut cmd = crate::git_dist::command(checkout);
    // The pattern is always prefixed `refs/heads/`, so `branch` can never reach
    // git in an option position however the request spelled it.
    cmd.args([
        "ls-remote",
        "--exit-code",
        "--heads",
        &url.fetch,
        &format!("refs/heads/{branch}"),
    ]);
    for (k, v) in crate::github::git_auth_env() {
        cmd.env(k, v);
    }
    let out = output_timed(&mut cmd, "git ls-remote").await.ok()?;
    match (out.status.success(), out.status.code()) {
        // Exit status alone is not the answer: `ls-remote`'s ref argument is a
        // *pattern*, so `refs/heads/feat/*` matches `refs/heads/feat/x` and exits
        // 0 while naming something GitHub will reject as a literal base. Require
        // the ref back verbatim, so only an exact branch counts as present and a
        // glob resolves to "absent" like any other name that isn't a branch.
        (true, _) => {
            let want = format!("refs/heads/{branch}");
            Some(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .any(|line| line.split('\t').nth(1).map(str::trim) == Some(want.as_str())),
            )
        }
        // `--exit-code` reserves 2 for "matched nothing". Every other non-zero
        // code is a failure to ask (128 for a missing remote or a rejected
        // credential), which must not read as absence.
        (false, Some(2)) => Some(false),
        _ => None,
    }
}

/// Rebase the current branch onto `base` (e.g. "main"). Used by the clean-state
/// panel action to bring the checkout up to date with its base branch when the
/// base has moved ahead. Aborts the rebase on conflict so the checkout is never
/// left mid-rebase — the caller surfaces the error.
pub async fn rebase_onto(checkout: &Path, base: &super::ResolvedBase) -> Result<()> {
    // Onto the resolved tip commit, never the branch name: a clone's
    // `refs/heads/<base>` is a stale snapshot, and rebasing onto it would replay
    // the agent's work onto a commit the base left behind long ago.
    let Some(tip) = base.tip.as_deref() else {
        return Err(Error::Git(format!(
            "base branch `{}` could not be resolved in this checkout — \
             nothing to rebase onto",
            base.name
        )));
    };
    // Rebasing rewrites commits, which needs a committer identity. Its
    // `pre-rebase`/`post-rewrite` hooks are neutralised at the spawn seam.
    let env = identity_env(checkout).await;
    let out = git_output_env(checkout, &["rebase", tip], &env).await?;
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
                "rebase onto {} failed: {conflict}; the checkout is left \
                 mid-rebase because cleanup also failed ({abort_err}) — run \
                 `git rebase --abort` manually",
                base.name
            )));
        }
        return Err(Error::Git(format!(
            "rebase onto {} failed: {conflict}",
            base.name
        )));
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
        super::super::testing::git(dir, args);
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
            push(&attacker, &attacker, opt, false).await.unwrap();
            push(&attacker, &attacker, opt, true).await.unwrap();

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
        let resolved = super::super::ResolvedBase {
            name: base.clone(),
            tip: super::super::rev_parse(repo, &base).await.ok(),
            fork_point: None,
        };
        let err = rebase_onto(repo, &resolved).await.unwrap_err();
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

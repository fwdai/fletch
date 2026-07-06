//! Thin wrapper around `git worktree`.
//!
//! Kept deliberately minimal — the v1 supervisor only needs to add a
//! worktree on a fresh branch and remove it later.

use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

use crate::error::{Error, Result};

/// Hard cap on the spawn-time `git fetch`. A fetch over a hung SSH/TCP
/// connection can otherwise block for the OS keep-alive window (75–120s), far
/// past the supervisor's 15s spawn watchdog — which would mark the agent
/// `Error` while the background task later still runs `start_process`, leaving
/// a live process under a failed-looking agent. Bounding it keeps the fetch
/// inside the spawn budget; on timeout we fall back to local HEAD.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Hard cap on quick network-bound ops (push/pull and the gh PR actions) so a
/// dropped connection surfaces as a finite error the UI can show, instead of
/// hanging a spinner for the OS keep-alive window. Deliberately generous —
/// these transfer little data, so 60s only ever trips on a stalled connection,
/// not on slow-but-progressing work. NOT used for `clone` (a large repo can
/// legitimately take minutes); clone's retry wedge is handled by self-heal in
/// `new_project::clone` instead.
const NET_TIMEOUT: Duration = Duration::from_secs(60);

/// Run a network-bound command under `NET_TIMEOUT`, killing the process (and
/// its SSH/HTTP child) on expiry via `kill_on_drop` rather than orphaning it.
/// `what` names the op for the timeout message. Mirrors `fetch_fork_point`'s
/// inline pattern; shared so push/pull and the gh PR ops stay consistent.
pub(crate) async fn output_timed(cmd: &mut Command, what: &str) -> Result<std::process::Output> {
    cmd.kill_on_drop(true);
    tokio::time::timeout(NET_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            // `Error::Other` (not `Error::Git`) — `what` already names the op
            // (e.g. "gh pr create"), so the "git command failed:" prefix would
            // mislabel non-git callers.
            Error::Other(format!(
                "{what} timed out after {}s — check your network connection",
                NET_TIMEOUT.as_secs()
            ))
        })?
        .map_err(Error::from)
}

/// Hard cap on *local* git operations (commit, rebase, diff, branch, …). Local
/// git is fast, so this only ever trips on a genuine hang — a blocking hook, an
/// `index.lock` held by another process, a wedged filesystem — bounding the UI
/// spinner instead of letting it spin indefinitely. These run during panel
/// actions, outside the spawn watchdog, so without a bound here a hung commit
/// or rebase has nothing to stop it. Network ops use `NET_TIMEOUT` via
/// `output_timed` instead, which carries a network-specific hint.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Spawn `git <args>` in `dir` under `LOCAL_TIMEOUT`, killing a hung process
/// (via `kill_on_drop`) on expiry rather than orphaning it. Returns the raw
/// `Output`; callers that inspect exit codes themselves (e.g. `show-ref`'s
/// 0/1) use this and check `status`, while `run_git` layers the
/// success-or-`Error::Git` check on top.
pub(crate) async fn git_output(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    git_output_env(dir, args, &[]).await
}

/// `git_output` with extra env vars — the commit-creating ops pass the
/// fallback identity through here (see `identity_env`).
async fn git_output_env(
    dir: &Path,
    args: &[&str],
    env: &[(String, String)],
) -> Result<std::process::Output> {
    let mut cmd = crate::git_dist::command(dir);
    cmd.args(args).kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    tokio::time::timeout(LOCAL_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            Error::Git(format!(
                "git {} timed out after {}s",
                args.first().copied().unwrap_or(""),
                LOCAL_TIMEOUT.as_secs()
            ))
        })?
        .map_err(Error::from)
}

/// Run `git <args>` in `dir` and require a zero exit, returning the `Output` so
/// callers can read stdout. On a non-zero exit returns
/// `Error::Git("<label> failed: <stderr>")` — `label` names the op for the
/// message. Collapses the "spawn, check status, format stderr" trio repeated
/// across this module into one call.
pub(crate) async fn run_git(dir: &Path, args: &[&str], label: &str) -> Result<std::process::Output> {
    let out = git_output(dir, args).await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "{label} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out)
}

/// GitHub token auth for git's https transport, applied to every network op
/// (push/pull/fetch). No-op without a token; scoped to github.com https, so
/// SSH remotes and other hosts are untouched (see `github::git_auth_env`).
fn apply_github_auth(cmd: &mut Command) {
    for (k, v) in crate::github::git_auth_env() {
        cmd.env(k, v);
    }
}

/// Env that makes a host-side git invocation ignore the workspace's own
/// hooks. Agent workspaces are agent-writable, so a hostile `.git/hooks/*`
/// would otherwise execute on the host when Fletch runs commit/push/merge.
/// `/dev/null` is not a directory, so git finds no hooks and runs none.
pub(crate) fn no_hooks_env() -> Vec<(String, String)> {
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), "core.hooksPath".into()),
        ("GIT_CONFIG_VALUE_0".into(), "/dev/null".into()),
    ]
}

/// Combine several env-var sets into one, correctly re-indexing any
/// `GIT_CONFIG_*` entries. Each set uses git's env-config convention
/// (`GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_<n>` / `GIT_CONFIG_VALUE_<n>`), so
/// setting two such sets blindly would let the second `GIT_CONFIG_COUNT`
/// shadow the first and silently drop a config entry — exactly the trap when
/// `git_auth_env()` and `no_hooks_env()` are both needed. This walks each
/// set's declared count, re-emits its key/value pairs under fresh contiguous
/// indices, and writes a single `GIT_CONFIG_COUNT` equal to the total.
/// Non-`GIT_CONFIG_*` vars (e.g. the identity `GIT_AUTHOR_*`) pass through
/// unchanged.
pub(crate) fn merge_git_env(sets: &[&[(String, String)]]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut idx: u32 = 0;
    for set in sets {
        let lookup = |name: &str| set.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        // Read only the entries the set declared it populated.
        let count: u32 = lookup("GIT_CONFIG_COUNT")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        for i in 0..count {
            if let (Some(key), Some(val)) = (
                lookup(&format!("GIT_CONFIG_KEY_{i}")),
                lookup(&format!("GIT_CONFIG_VALUE_{i}")),
            ) {
                out.push((format!("GIT_CONFIG_KEY_{idx}"), key));
                out.push((format!("GIT_CONFIG_VALUE_{idx}"), val));
                idx += 1;
            }
        }
        // Anything outside the GIT_CONFIG_* protocol (identity vars, etc.)
        // carries over verbatim.
        for (k, v) in set.iter() {
            if k == "GIT_CONFIG_COUNT"
                || k.starts_with("GIT_CONFIG_KEY_")
                || k.starts_with("GIT_CONFIG_VALUE_")
            {
                continue;
            }
            out.push((k.clone(), v.clone()));
        }
    }
    out.push(("GIT_CONFIG_COUNT".into(), idx.to_string()));
    out
}

/// Env vars that guarantee commit-creating commands (commit, merge via pull,
/// rebase) an author/committer identity. Empty when the repo already resolves
/// both `user.name` and `user.email` — the env would otherwise *override*
/// config, and a user's own identity must always win. Only the missing half
/// is filled, from the signed-in profile (or a neutral default), so a
/// non-engineer's first commit never dies with "Please tell me who you are".
pub(crate) async fn identity_env(dir: &Path) -> Vec<(String, String)> {
    let (mut has_name, mut has_email) = (false, false);
    // Exits 1 with empty stdout when nothing matches — not an error here.
    if let Ok(out) = git_output(dir, &["config", "--get-regexp", r"^user\.(name|email)$"]).await {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            match line.split_once(' ') {
                Some(("user.name", v)) if !v.trim().is_empty() => has_name = true,
                Some(("user.email", v)) if !v.trim().is_empty() => has_email = true,
                _ => {}
            }
        }
    }
    if has_name && has_email {
        return Vec::new();
    }
    let (name, email) = crate::git_dist::fallback_identity();
    let mut env = Vec::new();
    if !has_name {
        env.push(("GIT_AUTHOR_NAME".to_string(), name.clone()));
        env.push(("GIT_COMMITTER_NAME".to_string(), name));
    }
    if !has_email {
        env.push(("GIT_AUTHOR_EMAIL".to_string(), email.clone()));
        env.push(("GIT_COMMITTER_EMAIL".to_string(), email));
    }
    env
}

/// Create a worktree on detached HEAD (no branch yet). The worktree
/// stays detached for its whole working life; a branch is materialized
/// only at the first push (via `checkout_new_unique_branch`), named by
/// the agent. This keeps `git branch` clean for agents that never push
/// and lets the branch carry a meaningful, conventional name chosen with
/// full task context rather than a placeholder allocated up front.
///
/// `base` is the commit-ish the worktree starts from (e.g. `origin/main`
/// after a fresh fetch). When `None`, it starts from the repo's current
/// HEAD — the legacy behavior.
pub async fn worktree_add_detached(
    repo: &Path,
    worktree_path: &Path,
    base: Option<&str>,
) -> Result<()> {
    let path = worktree_path
        .to_str()
        .ok_or_else(|| Error::InvalidPath(worktree_path.display().to_string()))?;
    let mut args = vec!["worktree", "add", "--detach", path];
    if let Some(base) = base {
        args.push(base);
    }
    // `worktree add` checks out files in the new worktree, firing
    // `post-checkout` — a hook in the *source* repo would run on the host, so
    // disable workspace hooks for defense-in-depth.
    let hooks = no_hooks_env();
    let out = git_output_env(repo, &args, &hooks).await?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();

    // Recoverable case: the target path already exists but git doesn't track it
    // as a worktree — an orphan left by a crashed spawn or another instance
    // that shares this worktrees root. Prune stale registrations, and if the
    // path still isn't a live worktree, clear the orphan and retry once. We
    // never delete a *registered* worktree: that would be someone else's live
    // checkout, so in that case we fall through to the original error.
    //
    // We gate on `worktree_path.exists()` rather than on the git error text:
    // the stderr phrasing ("already exists") varies by git version and is
    // translated under non-C locales, so matching it would silently skip the
    // recovery on, say, a French or Japanese machine. The on-disk check is the
    // actual condition this branch handles and is locale-independent.
    if worktree_path.exists() && !is_registered_worktree(repo, worktree_path).await {
        let _ = worktree_prune(repo).await;
        if !is_registered_worktree(repo, worktree_path).await && worktree_path.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(worktree_path).await {
                tracing::warn!(path = %worktree_path.display(), error = %e, "orphan worktree dir cleanup failed");
            }
        }
        if !worktree_path.exists() {
            let retry = git_output_env(repo, &args, &hooks).await?;
            if retry.status.success() {
                tracing::info!(path = %worktree_path.display(), "recovered orphan worktree path on spawn");
                return Ok(());
            }
            return Err(Error::Git(format!(
                "worktree add --detach failed: {}",
                String::from_utf8_lossy(&retry.stderr).trim()
            )));
        }
    }

    Err(Error::Git(format!("worktree add --detach failed: {stderr}")))
}

/// Is `path` currently registered as a worktree of `repo`? Used by spawn to
/// tell an orphan directory (safe to clear) from a live checkout (must not
/// touch). A `false` here authorizes the caller to `remove_dir_all` the path,
/// so when the listing itself fails (transient lock contention, a process-limit
/// spike) we return `true`: "can't confirm it's an orphan, so don't touch it."
/// The caller then falls through to the original error rather than risking the
/// deletion of a live checkout.
async fn is_registered_worktree(repo: &Path, path: &Path) -> bool {
    let out = match git_output(repo, &["worktree", "list", "--porcelain"]).await {
        Ok(out) if out.status.success() => out,
        _ => return true,
    };
    // `tokio::fs::canonicalize` keeps these path-resolution syscalls off the
    // executor thread — `std::fs` would block it if the filesystem is slow
    // (network mount, loaded disk).
    let canonical = tokio::fs::canonicalize(path).await.ok();
    let stdout = String::from_utf8_lossy(&out.stdout);
    for listed in stdout.lines().filter_map(|l| l.strip_prefix("worktree ")) {
        let listed = Path::new(listed);
        if listed == path {
            return true;
        }
        if let Some(a) = &canonical {
            if let Ok(b) = tokio::fs::canonicalize(listed).await {
                if a == &b {
                    return true;
                }
            }
        }
    }
    false
}

/// Best-effort fetch of `branch` from `origin` so a freshly-spawned worktree
/// can fork from the latest remote state rather than a stale local ref.
/// Returns the commit-ish a worktree should be based on — `origin/<branch>`
/// when the fetch succeeded and the remote branch resolves — otherwise `None`,
/// signalling the caller to fall back to local HEAD.
///
/// Never errors: a missing `origin`, an offline machine, or a purely local
/// branch are all expected and simply mean "use local state".
pub async fn fetch_fork_point(repo: &Path, branch: &str) -> Option<String> {
    // `kill_on_drop` so a timeout actually tears down the hung git process
    // (and its SSH child) rather than orphaning it to keep blocking on the
    // dead connection.
    let mut fetch_cmd = crate::git_dist::command(repo);
    fetch_cmd
        .args(["fetch", "origin", branch])
        .kill_on_drop(true);
    apply_github_auth(&mut fetch_cmd);
    let fetched = tokio::time::timeout(FETCH_TIMEOUT, fetch_cmd.output()).await;
    // Timed out, failed to spawn, or non-zero exit → fall back to local HEAD.
    match fetched {
        Ok(Ok(out)) if out.status.success() => {}
        _ => return None,
    }
    // Confirm the remote-tracking ref resolves before handing it back — a
    // refspec that doesn't map this branch into refs/remotes would otherwise
    // leave us pointing at a ref that `worktree add` can't use.
    let remote_ref = format!("origin/{branch}");
    rev_parse(repo, &remote_ref).await.ok().map(|_| remote_ref)
}

/// Inside an existing worktree, create a new branch at the current
/// commit and check it out (`git checkout -b <branch>`). Used to
/// promote a detached-HEAD worktree onto a named branch once the
/// first user message gives us a slug.
pub async fn checkout_new_branch(worktree: &Path, branch: &str) -> Result<()> {
    // `checkout` fires `post-checkout`, which would run on the host against an
    // agent-writable workspace — disable workspace hooks for this invocation.
    let out = git_output_env(worktree, &["checkout", "-b", branch], &no_hooks_env()).await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "checkout -b {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Most same-named branches we'll step over before giving up when
/// materializing an agent's branch. A modest cap so a pathological
/// pile-up surfaces as an error rather than an unbounded probe loop.
const MAX_BRANCH_SUFFIX: u32 = 1000;

/// Whether `branch` is already claimed — either a local head or a known
/// remote-tracking ref (`origin/<branch>`). Used to pick a collision-free
/// name when materializing an agent's branch at push time.
///
/// The remote check reads `refs/remotes/origin/<branch>`, which reflects the
/// last fetch rather than a live `ls-remote` — a branch created on the remote
/// since then isn't seen, and `git push` would update it. That race is rare
/// and acceptable; avoiding a network round-trip on every push isn't.
pub async fn branch_name_taken(worktree: &Path, branch: &str) -> Result<bool> {
    if branch_exists(worktree, branch).await? {
        return Ok(true);
    }
    let refname = format!("refs/remotes/origin/{branch}");
    let out = git_output(worktree, &["show-ref", "--verify", "--quiet", &refname]).await?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Ok(false),
    }
}

/// Materialize a branch on a (typically detached) worktree at its current
/// HEAD, picking the first collision-free name from `desired`, `desired-2`,
/// `desired-3`, … and checking it out. Returns the name actually used.
///
/// This is the single point where an agent's branch is born — at the first
/// push, named from the agent's conventional choice (`fix/…`, `feat/…`,
/// `chore/…`) rather than a placeholder allocated at spawn.
pub async fn checkout_new_unique_branch(worktree: &Path, desired: &str) -> Result<String> {
    for n in 1..=MAX_BRANCH_SUFFIX {
        let candidate = if n == 1 {
            desired.to_string()
        } else {
            format!("{desired}-{n}")
        };
        // Propagate a probe error rather than masking it as "free": treating a
        // transient show-ref failure as an open name would attempt a checkout
        // that fails confusingly. Surfacing it lets the caller report honestly.
        if !branch_name_taken(worktree, &candidate).await? {
            checkout_new_branch(worktree, &candidate).await?;
            return Ok(candidate);
        }
    }
    Err(Error::Git(format!(
        "no free branch name for `{desired}` within {MAX_BRANCH_SUFFIX} tries"
    )))
}

/// `git init` a fresh repository at `path` (created if absent). Used by the
/// New Project "create" flow before seeding an initial commit.
pub async fn init_repo(path: &Path) -> Result<()> {
    // No `current_dir` — the target may not exist yet (`git init` creates it),
    // and spawning with a missing cwd fails before git ever runs.
    let out = crate::git_dist::bare_command()
        .args([
            "init",
            path.to_str().ok_or_else(|| {
                Error::InvalidPath(path.display().to_string())
            })?,
        ])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "init failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Stage everything and create a commit in `repo`. Uses the user's git
/// identity when configured; otherwise falls back to the signed-in profile
/// (see `identity_env`) so a machine with no `.gitconfig` can still commit.
pub async fn commit_all(repo: &Path, message: &str) -> Result<()> {
    run_git(repo, &["add", "-A"], "add -A").await?;

    let env = merge_git_env(&[&identity_env(repo).await, &no_hooks_env()]);
    let out = git_output_env(repo, &["commit", "-m", message], &env).await?;
    if !out.status.success() {
        // `git commit` writes the common "nothing to commit, working tree
        // clean" diagnostic to *stdout*, not stderr — so report both, else a
        // clean-tree failure surfaces as an empty, undebuggable message.
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        return Err(Error::Git(format!("commit failed: {}", detail.trim())));
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
    run_git(repo, &args, "worktree remove").await?;
    Ok(())
}

/// Drop any internal `.git/worktrees/<id>` refs whose linked working tree
/// no longer exists. Safe to run unconditionally — git just no-ops when
/// there's nothing to prune.
pub async fn worktree_prune(repo: &Path) -> Result<()> {
    run_git(repo, &["worktree", "prune"], "worktree prune").await?;
    Ok(())
}

/// Return the name of the currently-checked-out branch in the repo,
/// or `None` if HEAD is detached. Used by the supervisor to record
/// the parent branch when spawning an agent worktree.
pub async fn current_branch(repo: &Path) -> Result<Option<String>> {
    let out = git_output(repo, &["symbolic-ref", "--short", "-q", "HEAD"]).await?;
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
    let refname = format!("refs/heads/{branch}");
    let out = git_output(repo, &["show-ref", "--verify", "--quiet", &refname]).await?;
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

/// Resolve a ref to its full SHA. Returns the bare 40-char hex string.
/// Errors if the ref is unknown or git is unhappy.
pub async fn rev_parse(repo: &Path, refname: &str) -> Result<String> {
    let out = run_git(repo, &["rev-parse", "--verify", refname], &format!("rev-parse {refname}")).await?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run `git diff --shortstat <a>..<b>` and parse the additions /
/// deletions counts. Returns zero counts if both refs resolve to the
/// same commit (git prints nothing in that case).
pub async fn diff_shortstat(
    repo: &Path,
    from_sha: &str,
    to_sha: &str,
) -> Result<(u32, u32)> {
    let range = format!("{from_sha}..{to_sha}");
    let out = run_git(repo, &["diff", "--shortstat", &range], "diff --shortstat").await?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(parse_shortstat(&line))
}

/// Run `git diff --shortstat <base>` from a live worktree. This compares the
/// current working tree, including uncommitted changes, against the base ref.
pub async fn worktree_diff_shortstat(
    worktree: &Path,
    base_ref: &str,
) -> Result<(u32, u32)> {
    let out = run_git(worktree, &["diff", "--shortstat", base_ref], &format!("diff --shortstat {base_ref}")).await?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(parse_shortstat(&line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shortstat_typical() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 82 insertions(+), 12 deletions(-)"),
            (82, 12)
        );
    }

    #[test]
    fn parse_shortstat_only_additions() {
        assert_eq!(
            parse_shortstat(" 1 file changed, 5 insertions(+)"),
            (5, 0)
        );
    }

    #[test]
    fn parse_shortstat_only_deletions() {
        assert_eq!(
            parse_shortstat(" 2 files changed, 9 deletions(-)"),
            (0, 9)
        );
    }

    #[test]
    fn parse_shortstat_empty() {
        assert_eq!(parse_shortstat(""), (0, 0));
    }

    async fn config(repo: &Path, key: &str, val: &str) {
        let out = Command::new("git")
            .current_dir(repo)
            .args(["config", key, val])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
    }

    /// Look up a `GIT_CONFIG_KEY_<n>`'s paired value in a merged env set.
    fn config_value_for(env: &[(String, String)], key: &str) -> Option<String> {
        for i in 0.. {
            let k = format!("GIT_CONFIG_KEY_{i}");
            let entry = env.iter().find(|(name, _)| *name == k)?;
            if entry.1 == key {
                return env
                    .iter()
                    .find(|(name, _)| *name == format!("GIT_CONFIG_VALUE_{i}"))
                    .map(|(_, v)| v.clone());
            }
        }
        None
    }

    #[test]
    fn no_hooks_env_points_hookspath_at_dev_null() {
        let env = no_hooks_env();
        assert_eq!(config_value_for(&env, "core.hooksPath").as_deref(), Some("/dev/null"));
    }

    #[test]
    fn merge_git_env_reindexes_both_sets_without_collision() {
        // Two GIT_CONFIG_* sets that both declare COUNT=1 with KEY_0/VALUE_0.
        // Merged blindly one would clobber the other; the merge must keep both
        // under distinct indices and report the total count.
        let auth = vec![
            ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
            (
                "GIT_CONFIG_KEY_0".to_string(),
                "http.https://github.com/.extraheader".to_string(),
            ),
            (
                "GIT_CONFIG_VALUE_0".to_string(),
                "AUTHORIZATION: basic abc123".to_string(),
            ),
        ];
        let merged = merge_git_env(&[&auth, &no_hooks_env()]);

        // COUNT equals the total across both sets.
        assert_eq!(
            merged
                .iter()
                .find(|(k, _)| k == "GIT_CONFIG_COUNT")
                .map(|(_, v)| v.as_str()),
            Some("2")
        );
        // There is exactly one COUNT entry (the second set's didn't survive).
        assert_eq!(
            merged.iter().filter(|(k, _)| k == "GIT_CONFIG_COUNT").count(),
            1
        );
        // Both config entries are present with distinct indices.
        assert_eq!(
            config_value_for(&merged, "core.hooksPath").as_deref(),
            Some("/dev/null")
        );
        assert_eq!(
            config_value_for(&merged, "http.https://github.com/.extraheader").as_deref(),
            Some("AUTHORIZATION: basic abc123")
        );
    }

    #[test]
    fn merge_git_env_passes_plain_env_through() {
        // identity-style vars aren't part of the GIT_CONFIG_* protocol and must
        // survive the merge untouched.
        let identity = vec![
            ("GIT_AUTHOR_NAME".to_string(), "Tester".to_string()),
            ("GIT_AUTHOR_EMAIL".to_string(), "t@example.com".to_string()),
        ];
        let merged = merge_git_env(&[&identity, &no_hooks_env()]);
        assert!(merged
            .iter()
            .any(|(k, v)| k == "GIT_AUTHOR_NAME" && v == "Tester"));
        assert!(merged
            .iter()
            .any(|(k, v)| k == "GIT_AUTHOR_EMAIL" && v == "t@example.com"));
        assert_eq!(
            config_value_for(&merged, "core.hooksPath").as_deref(),
            Some("/dev/null")
        );
    }

    /// Write an executable `.git/hooks/<name>` that drops `sentinel` and fails.
    /// If git ran it, the sentinel would exist (and, for pre-* hooks, the op
    /// would be aborted). Unix-only: hooks must be executable to run.
    #[cfg(unix)]
    fn write_failing_hook(repo: &Path, name: &str, sentinel: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let hooks = repo.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let path = hooks.join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", sentinel.display()),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn commit_all_ignores_workspace_pre_commit_hook() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;

        let sentinel = td.path().join("hook-ran");
        write_failing_hook(repo, "pre-commit", &sentinel);

        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        // With hooks honored, the `exit 1` pre-commit would abort this commit.
        commit_all(repo, "first").await.unwrap();

        assert!(!sentinel.exists(), "workspace pre-commit hook must not run");
        // And the commit actually landed.
        let log = run_git(repo, &["log", "--oneline"], "log").await.unwrap();
        assert!(String::from_utf8_lossy(&log.stdout).contains("first"));
    }

    #[test]
    fn push_env_disables_hooks() {
        // A live remote is impractical here; assert instead that push's env
        // carries `core.hooksPath=/dev/null` (merged alongside any auth).
        let merged = merge_git_env(&[&crate::github::git_auth_env(), &no_hooks_env()]);
        assert_eq!(
            config_value_for(&merged, "core.hooksPath").as_deref(),
            Some("/dev/null")
        );
    }

    #[tokio::test]
    async fn commit_all_clean_tree_reports_nothing_to_commit() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;

        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        commit_all(repo, "first").await.unwrap();

        // Tree is now clean: git writes "nothing to commit" to stdout and exits
        // non-zero. The error must surface that, not an empty string.
        let err = commit_all(repo, "second").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nothing to commit"), "got: {msg}");
    }

    #[tokio::test]
    async fn fetch_fork_point_without_remote_is_none() {
        // No `origin` configured → best-effort fetch fails and we fall back to
        // local HEAD (None), never an error.
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        commit_all(repo, "first").await.unwrap();

        assert_eq!(fetch_fork_point(repo, "main").await, None);
    }

    #[tokio::test]
    async fn worktree_add_detached_uses_base_commit_when_given() {
        // A worktree forked from an explicit commit-ish starts at that commit,
        // not at the repo's current HEAD.
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;

        std::fs::write(repo.join("a.txt"), b"one").unwrap();
        commit_all(repo, "first").await.unwrap();
        let first = rev_parse(repo, "HEAD").await.unwrap();
        std::fs::write(repo.join("b.txt"), b"two").unwrap();
        commit_all(repo, "second").await.unwrap();

        // Base the worktree on the first commit even though HEAD is now `second`.
        let wt = td.path().join("wt");
        worktree_add_detached(repo, &wt, Some(&first)).await.unwrap();
        assert_eq!(rev_parse(&wt, "HEAD").await.unwrap(), first);

        // With no base it tracks current HEAD (`second`).
        let head = rev_parse(repo, "HEAD").await.unwrap();
        let wt2 = td.path().join("wt2");
        worktree_add_detached(repo, &wt2, None).await.unwrap();
        assert_eq!(rev_parse(&wt2, "HEAD").await.unwrap(), head);
    }

    #[tokio::test]
    async fn worktree_add_detached_recovers_orphan_dir() {
        // An orphan directory at the target path (left by a crashed spawn or a
        // foreign instance) is not a registered worktree, so add should clear
        // it and succeed rather than failing with "already exists".
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo).await.unwrap();
        config(&repo, "user.email", "t@example.com").await;
        config(&repo, "user.name", "Tester").await;
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        commit_all(&repo, "first").await.unwrap();

        // Pre-create the target as a plain, untracked directory with a file.
        let wt = td.path().join("orphan");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("leftover.txt"), b"stale").unwrap();

        worktree_add_detached(&repo, &wt, None).await.unwrap();
        assert!(is_registered_worktree(&repo, &wt).await);
        // The stale contents were cleared and replaced by the checkout.
        assert!(!wt.join("leftover.txt").exists());
        assert!(wt.join("a.txt").exists());
    }

    #[tokio::test]
    async fn worktree_add_detached_refuses_to_clobber_live_worktree() {
        // If the path is a *registered* worktree (someone's live checkout), add
        // must fail rather than delete it.
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo).await.unwrap();
        config(&repo, "user.email", "t@example.com").await;
        config(&repo, "user.name", "Tester").await;
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        commit_all(&repo, "first").await.unwrap();

        let wt = td.path().join("live");
        worktree_add_detached(&repo, &wt, None).await.unwrap();
        std::fs::write(wt.join("precious.txt"), b"keep me").unwrap();

        // A second add at the same live path must error and leave it untouched.
        assert!(worktree_add_detached(&repo, &wt, None).await.is_err());
        assert!(wt.join("precious.txt").exists());
    }
}

fn parse_shortstat(s: &str) -> (u32, u32) {
    let mut adds = 0u32;
    let mut dels = 0u32;
    for chunk in s.split(',').map(|c| c.trim()) {
        let mut parts = chunk.splitn(2, ' ');
        let n: u32 = match parts.next().and_then(|t| t.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let label = parts.next().unwrap_or("");
        if label.starts_with("insertion") {
            adds = n;
        } else if label.starts_with("deletion") {
            dels = n;
        }
    }
    (adds, dels)
}

/// Create a branch at a specific commit. Errors if the branch already
/// exists or the SHA isn't reachable.
pub async fn branch_create_at(repo: &Path, name: &str, sha: &str) -> Result<()> {
    run_git(repo, &["branch", name, sha], &format!("branch {name} {sha}")).await?;
    Ok(())
}

/// Create a worktree at `worktree_path` checked out on an existing
/// branch. Counterpart to `worktree_add_detached` — used by restore.
pub async fn worktree_add_branch(
    repo: &Path,
    worktree_path: &Path,
    branch: &str,
) -> Result<()> {
    let path = worktree_path
        .to_str()
        .ok_or_else(|| Error::InvalidPath(worktree_path.display().to_string()))?;
    // Checks out `branch` in the new worktree, firing `post-checkout` — disable
    // workspace hooks so a source-repo hook can't run on the host.
    let out =
        git_output_env(repo, &["worktree", "add", path, branch], &no_hooks_env()).await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "worktree add {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Push the current branch to `origin`. Uses `-u` to set the upstream
/// tracking ref on the first push.
/// Returns `"up-to-date"` when the remote already had everything (a no-op
/// push), otherwise `"pushed"`. Lets the UI confirm the outcome instead of
/// silently doing nothing when there was nothing to send.
pub async fn push(worktree: &Path, branch: &str) -> Result<String> {
    let mut cmd = crate::git_dist::command(worktree);
    cmd.args(["push", "-u", "origin", branch]);
    // Auth for the https transport *and* hook-disabling — `pre-push` fires on
    // the host, so a workspace-planted hook must not run. Merge so neither
    // set's `GIT_CONFIG_COUNT` clobbers the other.
    for (k, v) in merge_git_env(&[&crate::github::git_auth_env(), &no_hooks_env()]) {
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
pub async fn pull(worktree: &Path) -> Result<()> {
    let mut cmd = crate::git_dist::command(worktree);
    cmd.args(["pull"]);
    // Auth for the https transport; identity because a pull may create a merge
    // commit; no-hooks because the merge fires `post-merge`/`prepare-commit-msg`
    // on the host. Merge so the auth and no-hooks `GIT_CONFIG_*` sets don't
    // clobber each other (identity uses plain env vars and passes through).
    for (k, v) in merge_git_env(&[
        &crate::github::git_auth_env(),
        &no_hooks_env(),
        &identity_env(worktree).await,
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

/// Rebase the current branch onto `base` (e.g. "main"). Used by the clean-state
/// panel action to bring the worktree up to date with its base branch when the
/// base has moved ahead. Aborts the rebase on conflict so the worktree is never
/// left mid-rebase — the caller surfaces the error.
pub async fn rebase_onto(worktree: &Path, base: &str) -> Result<()> {
    // Rebasing rewrites commits, which needs a committer identity; it also
    // fires `pre-rebase`/`post-rewrite`, so disable workspace hooks too.
    let env = merge_git_env(&[&identity_env(worktree).await, &no_hooks_env()]);
    let out = git_output_env(worktree, &["rebase", base], &env).await?;
    if !out.status.success() {
        let conflict = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // Don't leave the worktree mid-rebase. If the abort *itself* fails or
        // times out, the worktree is stuck mid-rebase and needs manual
        // recovery — surface that alongside the original conflict rather than
        // silently swallowing it and reporting only the conflict.
        if let Err(abort_err) =
            run_git(worktree, &["rebase", "--abort"], "rebase --abort").await
        {
            return Err(Error::Git(format!(
                "rebase onto {base} failed: {conflict}; the worktree is left \
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
pub async fn commit(worktree: &Path, message: &str) -> Result<()> {
    run_git(worktree, &["add", "-A"], "add -A").await?;
    let env = merge_git_env(&[&identity_env(worktree).await, &no_hooks_env()]);
    let out = git_output_env(worktree, &["commit", "-m", message], &env).await?;
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
pub async fn discard_all(worktree: &Path) -> Result<()> {
    run_git(worktree, &["reset", "--hard", "HEAD"], "reset --hard").await?;
    run_git(worktree, &["clean", "-fd"], "clean -fd").await?;
    Ok(())
}

/// Stash all working-tree changes including untracked files. No message —
/// git generates the default "WIP on <branch>" label.
pub async fn stash_push(worktree: &Path) -> Result<()> {
    run_git(worktree, &["stash", "push", "--include-untracked"], "stash push").await?;
    Ok(())
}

/// Abort an in-progress merge, restoring the pre-merge working tree.
pub async fn merge_abort(worktree: &Path) -> Result<()> {
    run_git(worktree, &["merge", "--abort"], "merge --abort").await?;
    Ok(())
}

/// Stage everything and create the repo's first commit. `--allow-empty` so an
/// empty folder still gets a HEAD — worktrees can't fork without one. Uses the
/// identity fallback like every other commit-creating op.
pub async fn commit_initial(repo: &Path) -> Result<()> {
    run_git(repo, &["add", "-A"], "add -A").await?;
    let env = merge_git_env(&[&identity_env(repo).await, &no_hooks_env()]);
    let out = git_output_env(
        repo,
        &["commit", "--allow-empty", "-m", "Initial commit"],
        &env,
    )
    .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "initial commit failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Add a remote. Used when publishing a fresh local repo to GitHub.
pub async fn remote_add(worktree: &Path, name: &str, url: &str) -> Result<()> {
    run_git(worktree, &["remote", "add", name, url], &format!("remote add {name}")).await?;
    Ok(())
}

/// Subject and body of the worktree's last commit — the source for a PR's
/// title/body when the caller didn't supply one (what `gh pr create --fill`
/// did).
pub async fn last_commit_message(worktree: &Path) -> Result<(String, String)> {
    let out = run_git(worktree, &["log", "-1", "--format=%s%n%b"], "log -1").await?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let subject = lines.next().unwrap_or("").trim().to_string();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    Ok((subject, body))
}

/// Force-delete a local branch. Returns Ok even if the branch never
/// existed in the first place — that's exactly the state the caller
/// usually wants to converge on. Errors only for genuine git failures
/// (e.g. branch checked out in another live worktree).
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

/// List all local branches in the repo, sorted alphabetically.
pub async fn list_local_branches(repo: &Path) -> Result<Vec<String>> {
    let out = run_git(
        repo,
        &["for-each-ref", "refs/heads", "--format=%(refname:short)", "--sort=refname"],
        "for-each-ref",
    )
    .await?;
    let branches = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    Ok(branches)
}

/// List the worktree's relevant files: everything tracked plus untracked
/// files that aren't gitignored. Paths are repo-relative with forward
/// slashes (git's native form). This is what the File panel browses — it
/// naturally excludes `node_modules`, build output, etc.
pub async fn list_files(worktree: &Path) -> Result<Vec<String>> {
    let out = run_git(
        worktree,
        &["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        "ls-files",
    )
    .await?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect())
}

/// Read a single file's contents at a given ref (e.g. the parent branch),
/// used to show the prior contents of a file the agent deleted.
pub async fn show_file(worktree: &Path, base_ref: &str, path: &str) -> Result<String> {
    let spec = format!("{base_ref}:{path}");
    let out = run_git(worktree, &["show", &spec], &format!("show {base_ref}:{path}")).await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Return the 1-indexed line numbers (in the current working-tree file) the
/// agent changed versus `base_ref`, split into purely-added lines and
/// modified lines. Drives the File panel's VS Code-style change gutter.
pub async fn file_changed_lines(
    worktree: &Path,
    base_ref: &str,
    path: &str,
) -> Result<(Vec<u32>, Vec<u32>)> {
    let out = run_git(
        worktree,
        &["diff", "--no-color", "-U0", base_ref, "--", path],
        &format!("diff -U0 {base_ref} -- {path}"),
    )
    .await?;
    Ok(parse_changed_lines(&String::from_utf8_lossy(&out.stdout)))
}

/// Return the full unified diff of `path` versus `base_ref`, for the Code
/// panel's live view. `-U3` gives three lines of surrounding context per hunk.
pub async fn file_diff(worktree: &Path, base_ref: &str, path: &str) -> Result<String> {
    let out = run_git(
        worktree,
        &["diff", "--no-color", "-U3", base_ref, "--", path],
        &format!("diff -U3 {base_ref} -- {path}"),
    )
    .await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse `git diff -U0` output into (added, modified) new-file line numbers.
/// A hunk that only inserts lines marks them "added"; a hunk that also
/// removes lines marks its inserted lines "modified" (a replacement).
fn parse_changed_lines(diff: &str) -> (Vec<u32>, Vec<u32>) {
    let mut added: Vec<u32> = Vec::new();
    let mut modified: Vec<u32> = Vec::new();
    let mut new_line: u32 = 0;
    let mut hunk_added: Vec<u32> = Vec::new();
    let mut hunk_has_del = false;

    let flush = |hunk_added: &mut Vec<u32>,
                 hunk_has_del: &mut bool,
                 added: &mut Vec<u32>,
                 modified: &mut Vec<u32>| {
        if *hunk_has_del {
            modified.append(hunk_added);
        } else {
            added.append(hunk_added);
        }
        hunk_added.clear();
        *hunk_has_del = false;
    };

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            flush(&mut hunk_added, &mut hunk_has_del, &mut added, &mut modified);
            // rest looks like " -a,b +c,d @@ ..."; take the "+c[,d]" token.
            if let Some(plus) = rest.split_whitespace().find(|t| t.starts_with('+')) {
                new_line = plus
                    .trim_start_matches('+')
                    .split(',')
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .unwrap_or(0);
            }
            continue;
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        match line.chars().next() {
            Some('+') => {
                hunk_added.push(new_line);
                new_line = new_line.saturating_add(1);
            }
            Some('-') => hunk_has_del = true,
            Some('\\') => {} // "\ No newline at end of file" — ignore
            _ => new_line = new_line.saturating_add(1), // context line
        }
    }
    flush(&mut hunk_added, &mut hunk_has_del, &mut added, &mut modified);
    (added, modified)
}

#[cfg(test)]
mod changed_lines_tests {
    use super::parse_changed_lines;

    #[test]
    fn pure_additions_are_added() {
        let diff = "\
diff --git a/f b/f
--- a/f
+++ b/f
@@ -0,0 +1,3 @@
+one
+two
+three";
        assert_eq!(parse_changed_lines(diff), (vec![1, 2, 3], vec![]));
    }

    #[test]
    fn replacement_lines_are_modified() {
        // two old lines replaced by two new ones at line 3
        let diff = "\
@@ -3,2 +3,2 @@
-old a
-old b
+new a
+new b";
        assert_eq!(parse_changed_lines(diff), (vec![], vec![3, 4]));
    }

    #[test]
    fn mixed_hunks() {
        let diff = "\
@@ -3,1 +3,2 @@
-was
+now
+extra
@@ -10,0 +11,1 @@
+appended";
        // first hunk has a deletion → its '+' lines are modified (3,4);
        // second hunk is pure addition → added (11).
        assert_eq!(parse_changed_lines(diff), (vec![11], vec![3, 4]));
    }

    #[test]
    fn no_newline_marker_ignored() {
        let diff = "\
@@ -1 +1 @@
-a
+b
\\ No newline at end of file";
        assert_eq!(parse_changed_lines(diff), (vec![], vec![1]));
    }
}

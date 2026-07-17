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
pub(crate) async fn run_git(
    dir: &Path,
    args: &[&str],
    label: &str,
) -> Result<std::process::Output> {
    run_git_env(dir, args, &[], label).await
}

/// `run_git` with extra child env — the require-zero-exit wrapper for the
/// mutation helpers that must carry the no-hooks env (and, where relevant, an
/// identity/auth merge). Keeps the "spawn, check status, format stderr" trio in
/// one place so each hook-disabling call site stays a one-liner.
async fn run_git_env(
    dir: &Path,
    args: &[&str],
    env: &[(String, String)],
    label: &str,
) -> Result<std::process::Output> {
    let out = git_output_env(dir, args, env).await?;
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

/// Best-effort fetch of `branch` from `origin` so a freshly-spawned checkout
/// can fork from the latest remote state rather than a stale local ref.
/// Returns the commit-ish a checkout should be based on — the SHA that
/// `origin/<branch>` resolves to **in this repo** when the fetch succeeded —
/// otherwise `None`, signalling the caller to fall back to local HEAD.
///
/// The SHA (not the symbolic `origin/<branch>`) is essential for Clone-mode
/// provisioning: the checkout is a `git clone --shared` of this repo, so
/// inside the clone `origin/<branch>` resolves to this repo's *local*
/// `refs/heads/<branch>` — potentially stale — not the remote-tracking ref
/// the fetch just updated. A SHA resolves identically everywhere, and its
/// objects are reachable from the clone via alternates (shared clones) or
/// the copied object store (self-contained clones).
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
    // Resolve the remote-tracking ref to a SHA here, in the repo the fetch
    // updated. This both confirms the refspec mapped the branch into
    // refs/remotes and pins the base to the fetched tip regardless of which
    // repo (source or clone) later checks it out.
    let remote_ref = format!("origin/{branch}");
    rev_parse(repo, &remote_ref).await.ok()
}

/// Inside an existing checkout, create a new branch at the current
/// commit and check it out (`git checkout -b <branch>`). Used to
/// promote a detached-HEAD checkout onto a named branch once the
/// first user message gives us a slug.
pub async fn checkout_new_branch(checkout: &Path, branch: &str) -> Result<()> {
    // `checkout` fires `post-checkout`, which would run on the host against an
    // agent-writable workspace — disable workspace hooks for this invocation.
    run_git_env(
        checkout,
        &["checkout", "-b", branch],
        &no_hooks_env(),
        &format!("checkout -b {branch}"),
    )
    .await?;
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
pub async fn branch_name_taken(checkout: &Path, branch: &str) -> Result<bool> {
    if branch_exists(checkout, branch).await? {
        return Ok(true);
    }
    let refname = format!("refs/remotes/origin/{branch}");
    let out = git_output(checkout, &["show-ref", "--verify", "--quiet", &refname]).await?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Ok(false),
    }
}

/// Materialize a branch on a (typically detached) checkout at its current
/// HEAD, picking the first collision-free name from `desired`, `desired-2`,
/// `desired-3`, … and checking it out. Returns the name actually used.
///
/// This is the single point where an agent's branch is born — at the first
/// push, named from the agent's conventional choice (`fix/…`, `feat/…`,
/// `chore/…`) rather than a placeholder allocated at spawn.
pub async fn checkout_new_unique_branch(checkout: &Path, desired: &str) -> Result<String> {
    for n in 1..=MAX_BRANCH_SUFFIX {
        let candidate = if n == 1 {
            desired.to_string()
        } else {
            format!("{desired}-{n}")
        };
        // Propagate a probe error rather than masking it as "free": treating a
        // transient show-ref failure as an open name would attempt a checkout
        // that fails confusingly. Surfacing it lets the caller report honestly.
        if !branch_name_taken(checkout, &candidate).await? {
            checkout_new_branch(checkout, &candidate).await?;
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
            path.to_str()
                .ok_or_else(|| Error::InvalidPath(path.display().to_string()))?,
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

/// Capture `checkout`'s current working tree — tracked modifications plus
/// untracked, non-ignored files, and deletions — into a commit object WITHOUT
/// touching the checkout's real index, HEAD, or working tree, and return its
/// sha. Used by fork "carry code": the snapshot is created in the *source*
/// checkout's object store (so a live agent is left undisturbed) and later
/// fetched into the fork by [`carry_worktree`]. The snapshot's parent is the
/// source's HEAD, so it records the full state (committed + uncommitted).
pub async fn snapshot_worktree(checkout: &Path) -> Result<String> {
    // A throwaway index so `add -A` never stages into the live agent's index.
    let tmp = tempfile::Builder::new()
        .prefix("fletch-fork-index-")
        .tempfile()
        .map_err(Error::from)?;
    let index_env = vec![(
        "GIT_INDEX_FILE".to_string(),
        tmp.path().display().to_string(),
    )];

    // Seed the temp index from HEAD, then stage every working-tree change
    // (adds/mods/dels, honoring .gitignore) into it — the snapshot tree.
    let stage_env = merge_git_env(&[&index_env, &no_hooks_env()]);
    run_git_env(
        checkout,
        &["read-tree", "HEAD"],
        &stage_env,
        "fork snapshot read-tree",
    )
    .await?;
    run_git_env(checkout, &["add", "-A"], &stage_env, "fork snapshot add").await?;
    let tree = run_git_env(
        checkout,
        &["write-tree"],
        &stage_env,
        "fork snapshot write-tree",
    )
    .await?;
    let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();

    // commit-tree writes no hooks but needs an identity, same fallback as commit.
    let commit_env = merge_git_env(&[&index_env, &identity_env(checkout).await, &no_hooks_env()]);
    let commit = run_git_env(
        checkout,
        &[
            "commit-tree",
            &tree,
            "-p",
            "HEAD",
            "-m",
            "fletch: fork snapshot",
        ],
        &commit_env,
        "fork snapshot commit-tree",
    )
    .await?;
    Ok(String::from_utf8_lossy(&commit.stdout).trim().to_string())
}

/// Point `dest`'s working tree at `snapshot` (fetched from the `source`
/// checkout) while keeping `dest`'s HEAD on `base` — so the carried work shows
/// as uncommitted changes against the fork's base, mirroring the parent's own
/// diff. Used by fork "carry code" after `dest` is provisioned clean at `base`.
pub async fn carry_worktree(dest: &Path, source: &Path, snapshot: &str, base: &str) -> Result<()> {
    let source_str = source
        .to_str()
        .ok_or_else(|| Error::InvalidPath(source.display().to_string()))?;
    // Bring the snapshot commit + its reachable objects into the fork's store.
    run_git(
        dest,
        &["fetch", "--no-tags", source_str, snapshot],
        "carry fetch",
    )
    .await?;
    // Materialize the snapshot exactly (adds/mods/dels), then move HEAD + index
    // back to base, leaving the working tree — so the delta reads as unstaged
    // working-tree changes, exactly like the parent's uncommitted state.
    let env = no_hooks_env();
    run_git_env(
        dest,
        &["reset", "--hard", snapshot],
        &env,
        "carry reset --hard",
    )
    .await?;
    run_git_env(
        dest,
        &["reset", "--mixed", base],
        &env,
        "carry reset --mixed",
    )
    .await?;
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
/// the parent branch when spawning an agent checkout.
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
/// spawning a checkout — on collision it falls back to a name that
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
    let out = run_git(
        repo,
        &["rev-parse", "--verify", refname],
        &format!("rev-parse {refname}"),
    )
    .await?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run `git diff --shortstat <a>..<b>` and parse the additions /
/// deletions counts. Returns zero counts if both refs resolve to the
/// same commit (git prints nothing in that case).
pub async fn diff_shortstat(repo: &Path, from_sha: &str, to_sha: &str) -> Result<(u32, u32)> {
    let range = format!("{from_sha}..{to_sha}");
    let out = run_git(repo, &["diff", "--shortstat", &range], "diff --shortstat").await?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(parse_shortstat(&line))
}

/// Run `git diff --shortstat <base>` from a live checkout. This compares the
/// current working tree, including uncommitted changes, against the base ref.
pub async fn checkout_diff_shortstat(checkout: &Path, base_ref: &str) -> Result<(u32, u32)> {
    let out = run_git(
        checkout,
        &["diff", "--shortstat", base_ref],
        &format!("diff --shortstat {base_ref}"),
    )
    .await?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(parse_shortstat(&line))
}

/// Per-file additions/deletions for `git diff --numstat <from>..<to>` in `repo`.
/// Binary files (numstat prints `-`/`-`) report zero counts. Lists the files a
/// ferried ref changed versus the run base for the review surface's file list.
pub async fn diff_numstat(
    repo: &Path,
    from_sha: &str,
    to_sha: &str,
) -> Result<Vec<(String, u32, u32)>> {
    let range = format!("{from_sha}..{to_sha}");
    let out = run_git(repo, &["diff", "--numstat", &range], "diff --numstat").await?;
    Ok(parse_numstat_lines(&String::from_utf8_lossy(&out.stdout)))
}

/// Parse `git diff --numstat` output into `(path, additions, deletions)` rows.
/// Binary files print `-` for both counts; those become 0.
fn parse_numstat_lines(text: &str) -> Vec<(String, u32, u32)> {
    let mut files = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        if let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) {
            files.push((
                path.to_string(),
                a.parse::<u32>().unwrap_or(0),
                d.parse::<u32>().unwrap_or(0),
            ));
        }
    }
    files
}

/// The unified diff of `from_sha..to_sha` in `repo`, optionally scoped to one
/// `path` (`-U3`, matching the file-diff view's context). Both refs are objects
/// in the same repo — for the review surface, the run repo where the ferried step
/// ref and the run base both live — so no checkout is needed.
pub async fn diff_refs(
    repo: &Path,
    from_sha: &str,
    to_sha: &str,
    path: Option<&str>,
) -> Result<String> {
    let range = format!("{from_sha}..{to_sha}");
    let mut args = vec!["diff", "--no-color", "-U3", &range];
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let out = run_git(repo, &args, "diff -U3 range").await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numstat_lines_counts_and_binaries() {
        let text = "12\t3\tsrc/a.rs\n-\t-\tassets/logo.png\n0\t5\tsrc/b.rs\n";
        assert_eq!(
            parse_numstat_lines(text),
            vec![
                ("src/a.rs".to_string(), 12, 3),
                ("assets/logo.png".to_string(), 0, 0),
                ("src/b.rs".to_string(), 0, 5),
            ]
        );
    }

    #[test]
    fn parse_shortstat_typical() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 82 insertions(+), 12 deletions(-)"),
            (82, 12)
        );
    }

    #[test]
    fn parse_shortstat_only_additions() {
        assert_eq!(parse_shortstat(" 1 file changed, 5 insertions(+)"), (5, 0));
    }

    #[test]
    fn parse_shortstat_only_deletions() {
        assert_eq!(parse_shortstat(" 2 files changed, 9 deletions(-)"), (0, 9));
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
        assert_eq!(
            config_value_for(&env, "core.hooksPath").as_deref(),
            Some("/dev/null")
        );
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
            merged
                .iter()
                .filter(|(k, _)| k == "GIT_CONFIG_COUNT")
                .count(),
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

    #[tokio::test]
    async fn snapshot_and_carry_reproduce_working_tree_at_base() {
        // Parent checkout: a base commit (with a .gitignore), then uncommitted
        // work covering every case — modify, delete, add, and an ignored file.
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("src");
        std::fs::create_dir(&src).unwrap();
        init_repo(&src).await.unwrap();
        config(&src, "user.email", "t@example.com").await;
        config(&src, "user.name", "Tester").await;
        std::fs::write(src.join(".gitignore"), b"ignored.txt\n").unwrap();
        std::fs::write(src.join("keep.txt"), b"base").unwrap();
        std::fs::write(src.join("drop.txt"), b"remove me").unwrap();
        commit_all(&src, "base").await.unwrap();
        let base = rev_parse(&src, "HEAD").await.unwrap();

        std::fs::write(src.join("keep.txt"), b"modified").unwrap();
        std::fs::remove_file(src.join("drop.txt")).unwrap();
        std::fs::write(src.join("new.txt"), b"added").unwrap();
        std::fs::write(src.join("ignored.txt"), b"secret").unwrap();

        let snap = snapshot_worktree(&src).await.unwrap();
        // No side effects on the live checkout: HEAD is untouched.
        assert_eq!(rev_parse(&src, "HEAD").await.unwrap(), base);

        // Fork checkout: a clone sitting at base, like a freshly-provisioned one.
        let dst = td.path().join("dst");
        let clone = Command::new("git")
            .args(["clone", "-q", src.to_str().unwrap(), dst.to_str().unwrap()])
            .output()
            .await
            .unwrap();
        assert!(clone.status.success());

        carry_worktree(&dst, &src, &snap, &base).await.unwrap();

        // Working tree now mirrors the parent's: mod/add applied, deletion gone,
        // ignored file never carried.
        assert_eq!(std::fs::read(dst.join("keep.txt")).unwrap(), b"modified");
        assert_eq!(std::fs::read(dst.join("new.txt")).unwrap(), b"added");
        assert!(!dst.join("drop.txt").exists());
        assert!(!dst.join("ignored.txt").exists());
        // HEAD stays at base — the carried work reads as uncommitted changes.
        assert_eq!(rev_parse(&dst, "HEAD").await.unwrap(), base);
        let status = run_git(&dst, &["status", "--porcelain"], "status")
            .await
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "carried changes should show as uncommitted"
        );
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
    async fn ensure_head_commit_seeds_unborn_head_and_is_idempotent() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;

        // Unborn HEAD: no commit yet.
        assert!(rev_parse(repo, "HEAD").await.is_err());

        ensure_head_commit(repo).await.unwrap();
        let first = rev_parse(repo, "HEAD").await.unwrap();

        // No-op on a repo that already has a HEAD — same commit, no error.
        ensure_head_commit(repo).await.unwrap();
        assert_eq!(first, rev_parse(repo, "HEAD").await.unwrap());
    }

    /// Overlapping spawns against the same commit-less repo must seed exactly one
    /// commit and none may fail. Without the internal lock the check-then-act
    /// race lets multiple callers each commit (a stray empty "Initial commit") or
    /// fail one on `.git/index.lock` contention.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ensure_head_commit_concurrent_seeds_exactly_one_commit() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().to_path_buf();
        init_repo(&repo).await.unwrap();
        config(&repo, "user.email", "t@example.com").await;
        config(&repo, "user.name", "Tester").await;
        assert!(rev_parse(&repo, "HEAD").await.is_err());

        // Fire many concurrent calls at the unborn-HEAD repo at once.
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let r = repo.clone();
            set.spawn(async move { ensure_head_commit(&r).await });
        }
        while let Some(joined) = set.join_next().await {
            // No task panicked, and no call returned Err (no lock-contention
            // failure surfaced to a spawn).
            joined.unwrap().unwrap();
        }

        // Exactly one commit — no duplicate from a lost race.
        let out = run_git(&repo, &["rev-list", "--count", "HEAD"], "rev-list")
            .await
            .unwrap();
        let count = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(count, "1", "expected exactly one commit, got {count}");
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
    async fn fetch_fork_point_returns_fetched_tip_sha_not_stale_local_head() {
        // Clone-mode workspaces are `git clone --shared` of the source repo,
        // where `origin/<branch>` resolves to the source's *local* head —
        // stale when the user hasn't pulled. The fork point must therefore be
        // the SHA of the freshly-fetched remote tip, resolved in the source
        // repo, never the symbolic `origin/<branch>`.
        let td = tempfile::tempdir().unwrap();

        // `upstream` plays the true remote.
        let upstream = td.path().join("upstream");
        init_repo(&upstream).await.unwrap();
        config(&upstream, "user.email", "t@example.com").await;
        config(&upstream, "user.name", "Tester").await;
        std::fs::write(upstream.join("a.txt"), b"one").unwrap();
        commit_all(&upstream, "first").await.unwrap();
        run_git(&upstream, &["checkout", "-B", "main"], "checkout -B main")
            .await
            .unwrap();

        // `source` is the user's repo, cloned before upstream advanced.
        let source = td.path().join("source");
        let out = Command::new("git")
            .current_dir(td.path())
            .args(["clone", upstream.to_str().unwrap(), "source"])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());

        // Upstream advances; the source's local `main` is now stale.
        std::fs::write(upstream.join("b.txt"), b"two").unwrap();
        commit_all(&upstream, "second").await.unwrap();
        let upstream_tip = rev_parse(&upstream, "main").await.unwrap();
        let stale_local = rev_parse(&source, "main").await.unwrap();
        assert_ne!(stale_local, upstream_tip);

        let base = fetch_fork_point(&source, "main").await.unwrap();
        assert_eq!(base, upstream_tip);
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

/// Push the detached `HEAD` to `refs/heads/<branch>` on `origin`, creating the
/// remote branch without needing a local one (so it never collides with a
/// linked worktree's checkout). Same auth + hook-disabling env as [`push`], so
/// the https transport authenticates with the app's token and a workspace
/// `pre-push` hook can't fire on the host.
pub async fn push_head_to_branch(checkout: &Path, branch: &str) -> Result<()> {
    let mut cmd = crate::git_dist::command(checkout);
    cmd.args(["push", "origin", &format!("HEAD:refs/heads/{branch}")]);
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
pub async fn push(checkout: &Path, branch: &str, force: bool) -> Result<String> {
    let mut cmd = crate::git_dist::command(checkout);
    cmd.args(["push", "-u"]);
    if force {
        cmd.args(["--force-with-lease", "--force-if-includes"]);
    }
    cmd.args(["origin", branch]);
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
pub async fn pull(checkout: &Path) -> Result<()> {
    let mut cmd = crate::git_dist::command(checkout);
    cmd.args(["pull"]);
    // Auth for the https transport; identity because a pull may create a merge
    // commit; no-hooks because the merge fires `post-merge`/`prepare-commit-msg`
    // on the host. Merge so the auth and no-hooks `GIT_CONFIG_*` sets don't
    // clobber each other (identity uses plain env vars and passes through).
    for (k, v) in merge_git_env(&[
        &crate::github::git_auth_env(),
        &no_hooks_env(),
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
    let mut cmd = crate::git_dist::command(source_repo);
    cmd.args(["fetch", "--quiet", "origin", base]);
    for (k, v) in merge_git_env(&[&crate::github::git_auth_env(), &no_hooks_env()]) {
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
    // Rebasing rewrites commits, which needs a committer identity; it also
    // fires `pre-rebase`/`post-rewrite`, so disable workspace hooks too.
    let env = merge_git_env(&[&identity_env(checkout).await, &no_hooks_env()]);
    let out = git_output_env(checkout, &["rebase", base], &env).await?;
    if !out.status.success() {
        let conflict = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // Don't leave the checkout mid-rebase. `rebase --abort` checks out the
        // original HEAD, firing `post-checkout` — so it too must run with hooks
        // disabled, else the failure path reopens the very host-execution hole
        // the success path closes. If the abort *itself* fails or times out, the
        // checkout is stuck mid-rebase and needs manual recovery — surface that
        // alongside the original conflict rather than silently swallowing it and
        // reporting only the conflict.
        if let Err(abort_err) = run_git_env(
            checkout,
            &["rebase", "--abort"],
            &no_hooks_env(),
            "rebase --abort",
        )
        .await
        {
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
    let env = merge_git_env(&[&identity_env(checkout).await, &no_hooks_env()]);
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

/// Process-wide lock serializing the unborn-HEAD seed in `ensure_head_commit`.
/// Contended only when a repo has no commits yet and two spawns race to seed it;
/// the common HEAD-already-exists path never acquires it (see the double-check).
static HEAD_SEED_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Guarantee `repo` has a resolvable `HEAD` so a workspace can fork from it.
/// A repo that's been `git init`'d but never committed has an unborn HEAD, and
/// neither `git clone --shared` (the default workspace mode) nor `worktree add`
/// can fork a repo without one — the fork fails and the agent never launches.
/// Seed the repo's first commit in that case; no-op when HEAD already resolves.
/// Idempotent, so it's safe to call on every provisioning.
///
/// Concurrency: the check (`rev_parse`) and the act (`commit_initial`) are
/// guarded by a process-wide lock, with a double-check inside it. Without the
/// lock two overlapping spawns against the same commit-less repo could both pass
/// the check and each seed — leaving a stray empty "Initial commit", or failing
/// one spawn on `.git/index.lock` contention. The lock is taken only once HEAD
/// is confirmed unborn, so the common case stays lock-free. This serializes
/// within one Fletch process; two separate processes seeding the same brand-new
/// repo at the same instant still fall back to git's own index locking — a
/// vanishingly small, self-limiting window that closes the moment a commit lands.
pub async fn ensure_head_commit(repo: &Path) -> Result<()> {
    if rev_parse(repo, "HEAD").await.is_ok() {
        return Ok(());
    }
    // Unborn HEAD — serialize the seed so racing spawns don't double-commit.
    let _guard = HEAD_SEED_LOCK.lock().await;
    // A racer may have seeded HEAD while we waited for the lock; re-check.
    if rev_parse(repo, "HEAD").await.is_ok() {
        return Ok(());
    }
    commit_initial(repo).await
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

/// List all local branches in the repo, sorted alphabetically.
pub async fn list_local_branches(repo: &Path) -> Result<Vec<String>> {
    let out = run_git(
        repo,
        &[
            "for-each-ref",
            "refs/heads",
            "--format=%(refname:short)",
            "--sort=refname",
        ],
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

/// List the checkout's relevant files: everything tracked plus untracked
/// files that aren't gitignored. Paths are repo-relative with forward
/// slashes (git's native form). This is what the File panel browses — it
/// naturally excludes `node_modules`, build output, etc.
pub async fn list_files(checkout: &Path) -> Result<Vec<String>> {
    let out = run_git(
        checkout,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
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
pub async fn show_file(checkout: &Path, base_ref: &str, path: &str) -> Result<String> {
    let spec = format!("{base_ref}:{path}");
    let out = run_git(
        checkout,
        &["show", &spec],
        &format!("show {base_ref}:{path}"),
    )
    .await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Return the 1-indexed line numbers (in the current working-tree file) the
/// agent changed versus `base_ref`, split into purely-added lines and
/// modified lines. Drives the File panel's VS Code-style change gutter.
pub async fn file_changed_lines(
    checkout: &Path,
    base_ref: &str,
    path: &str,
) -> Result<(Vec<u32>, Vec<u32>)> {
    let diff = file_diff_unified(checkout, base_ref, path, "-U0").await?;
    Ok(parse_changed_lines(&diff))
}

/// Return the full unified diff of `path` versus `base_ref`, for the Code
/// panel's live view. `-U3` gives three lines of surrounding context per hunk.
pub async fn file_diff(checkout: &Path, base_ref: &str, path: &str) -> Result<String> {
    file_diff_unified(checkout, base_ref, path, "-U3").await
}

/// Unified diff of `path` versus `base_ref` with the given `-U<n>` context
/// flag. `git diff <ref>` only covers files in the index, so an untracked
/// (agent-created, never `git add`ed) file diffs as empty; when that happens,
/// re-diff it against /dev/null with `--no-index`, which renders the whole
/// file as one added hunk.
async fn file_diff_unified(
    checkout: &Path,
    base_ref: &str,
    path: &str,
    unified: &str,
) -> Result<String> {
    let out = run_git(
        checkout,
        &["diff", "--no-color", unified, base_ref, "--", path],
        &format!("diff {unified} {base_ref} -- {path}"),
    )
    .await?;
    if !out.stdout.is_empty() || is_tracked(checkout, path).await {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    // `--no-index` exits 1 whenever the two sides differ, so accept 0 and 1.
    let out = git_output(
        checkout,
        &[
            "diff",
            "--no-color",
            unified,
            "--no-index",
            "--",
            NULL_DEVICE,
            path,
        ],
    )
    .await?;
    match out.status.code() {
        Some(0) | Some(1) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        _ => Err(Error::Git(format!(
            "diff --no-index -- {path} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// The null device handed to `diff --no-index` as the "before" side. The app
/// ships macOS-only today, but name the Windows device explicitly rather than
/// relying on Git-for-Windows' msys `/dev/null` emulation if that changes.
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// Whether `path` is in the index. `ls-files --error-unmatch` exits 0 for
/// tracked paths; a spawn failure counts as tracked so callers fall back to
/// the plain-diff result rather than a second diff that would also fail.
async fn is_tracked(checkout: &Path, path: &str) -> bool {
    git_output(checkout, &["ls-files", "--error-unmatch", "--", path])
        .await
        .map(|o| o.status.success())
        .unwrap_or(true)
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
            flush(
                &mut hunk_added,
                &mut hunk_has_del,
                &mut added,
                &mut modified,
            );
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
    flush(
        &mut hunk_added,
        &mut hunk_has_del,
        &mut added,
        &mut modified,
    );
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

//! Shared core for the git CLI wrappers: timeout-bounded process spawning and
//! the auth / hook-disabling / identity env builders. Everything here is
//! `pub(crate)` — the topic modules and a few crate-internal callers build on it.

use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

use crate::error::{Error, Result};

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
pub(crate) async fn git_output_env(
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
pub(crate) async fn run_git_env(
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
pub(crate) fn apply_github_auth(cmd: &mut Command) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

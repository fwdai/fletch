//! Signing on publish. An agent's clone commits unsigned — the sandbox holds no
//! key, and provisioning freezes `commit.gpgsign=false` into the clone
//! (`sandbox::provision::seed_identity`) — so when the host's own git config
//! asks for signed commits, the host signs what a push publishes, at the push.
//! That is the moment the user takes responsibility for the commits (the
//! approval prompt sits right before it), so it is the honest moment for the
//! user's key to vouch for them.
//!
//! Each unpublished commit is replaced by a signed copy with the same tree,
//! parents, author, committer and message, parents mapped to their own signed
//! copies. Only the commits origin lacks are rewritten, so published history
//! keeps its hashes and the push stays a fast-forward. The branch is moved to
//! the signed tip, so the agent's next commit builds on it.
//!
//! Residual: a commit's `encoding` and `mergetag` headers are not carried over
//! (neither has a `commit-tree` spelling). Fletch never produces either.

use std::collections::HashMap;
use std::path::Path;

use super::cmd::{git_output, git_output_stdin};
use crate::error::{Error, Result};

/// Whether the host's git config asks for signed commits — in the scopes the
/// user owns (system, global), never the clone's own, where provisioning froze
/// `commit.gpgsign=false`.
pub async fn host_signs_commits(repo: &Path) -> Result<bool> {
    let out = git_output(
        repo,
        &[
            "config",
            "--type=bool",
            "--show-scope",
            "--get-all",
            "commit.gpgsign",
        ],
    )
    .await?;
    // Exit 1: the key is set in no scope.
    Ok(out.status.success() && signing_requested(&String::from_utf8_lossy(&out.stdout)))
}

/// `--show-scope` output (`<scope>\t<value>` per line, in precedence order):
/// the last system/global value decides.
pub(crate) fn signing_requested(show_scope: &str) -> bool {
    show_scope
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(scope, _)| matches!(*scope, "system" | "global"))
        .next_back()
        .is_some_and(|(_, value)| value.trim() == "true")
}

/// `tip` as the host will publish it: `tip` itself when the host does not
/// sign, else with its unpublished commits replaced by signed copies
/// ([`sign_unpublished`]).
pub async fn sign_for_publish(repo: &Path, branch: &str, tip: &str) -> Result<String> {
    if !host_signs_commits(repo).await? {
        return Ok(tip.to_string());
    }
    sign_unpublished(repo, branch, tip).await
}

/// Replace the commits `tip` reaches that origin lacks with signed copies and
/// move `refs/heads/<branch>` from `tip` to the new tip, which is returned.
/// `tip` comes back unchanged when every such commit already carries a
/// signature. The branch move is compare-and-swap on `tip`, so a commit that
/// lands on the branch meanwhile fails the move rather than being dropped.
pub async fn sign_unpublished(repo: &Path, branch: &str, tip: &str) -> Result<String> {
    let out = git_output(
        repo,
        &[
            "rev-list",
            "--reverse",
            "--topo-order",
            tip,
            "--not",
            "--remotes=origin",
            "--",
        ],
    )
    .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "rev-list {tip} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let unpublished: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    // Parents come before children (`--reverse` of topo order), so a parent's
    // signed copy exists by the time its child is rewritten.
    let mut signed: HashMap<String, String> = HashMap::new();
    for sha in &unpublished {
        let raw = git_output(repo, &["cat-file", "commit", sha]).await?;
        if !raw.status.success() {
            return Err(Error::Git(format!(
                "cat-file {sha} failed: {}",
                String::from_utf8_lossy(&raw.stderr).trim()
            )));
        }
        let commit = RawCommit::parse(&raw.stdout)
            .ok_or_else(|| Error::Git(format!("commit {sha} is malformed")))?;
        let parents: Vec<String> = commit
            .parents
            .iter()
            .map(|p| signed.get(p).cloned().unwrap_or_else(|| p.clone()))
            .collect();
        // A commit that is already signed keeps its signature — unless a parent
        // was rewritten, which changes its content and so voids it.
        if commit.signed && parents == commit.parents {
            continue;
        }
        let copy = commit_signed(repo, &commit, &parents).await?;
        signed.insert(sha.clone(), copy);
    }

    let Some(new_tip) = signed.get(tip) else {
        return Ok(tip.to_string());
    };
    let refname = format!("refs/heads/{branch}");
    let out = git_output(
        repo,
        &[
            "update-ref",
            "-m",
            "fletch: signed on publish",
            &refname,
            new_tip,
            tip,
        ],
    )
    .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "moving {branch} to its signed tip failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(new_tip.clone())
}

/// A signed copy of `commit` over `parents`: `commit-tree -S` with the
/// original identities and dates in the env and the message on stdin, the one
/// form that keeps it byte for byte. The signing setup (`gpg.format`,
/// `user.signingkey`, the key itself) is the host's own; `-S` signs regardless
/// of the clone's `commit.gpgsign=false`.
async fn commit_signed(repo: &Path, commit: &RawCommit, parents: &[String]) -> Result<String> {
    let mut args = vec!["commit-tree", "-S", commit.tree.as_str()];
    for parent in parents {
        args.push("-p");
        args.push(parent);
    }
    let out = git_output_stdin(repo, &args, &commit.identity_env(), &commit.message).await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "signing failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// One `author`/`committer` header: `Name <email> <unix-timestamp> <tz>`.
struct Ident {
    name: String,
    email: String,
    /// Git's internal date form, which `GIT_*_DATE` accepts verbatim.
    date: String,
}

impl Ident {
    fn parse(line: &str) -> Option<Self> {
        let (name, rest) = line.split_once('<')?;
        let (email, date) = rest.split_once('>')?;
        Some(Self {
            name: name.trim().to_string(),
            email: email.to_string(),
            date: date.trim().to_string(),
        })
    }
}

/// A commit object as `cat-file commit` prints it, reduced to what a copy
/// needs.
struct RawCommit {
    tree: String,
    parents: Vec<String>,
    author: Ident,
    committer: Ident,
    /// Carries a `gpgsig` header.
    signed: bool,
    /// Everything after the header block, untouched.
    message: Vec<u8>,
}

impl RawCommit {
    fn parse(raw: &[u8]) -> Option<Self> {
        let split = raw.windows(2).position(|w| w == b"\n\n")?;
        let headers = String::from_utf8_lossy(&raw[..split]);
        let message = raw[split + 2..].to_vec();
        let mut tree = None;
        let mut parents = Vec::new();
        let mut author = None;
        let mut committer = None;
        let mut signed = false;
        for line in headers.lines() {
            // A continuation line of a multi-line header (the signature body).
            if line.starts_with(' ') {
                continue;
            }
            match line.split_once(' ') {
                Some(("tree", v)) => tree = Some(v.to_string()),
                Some(("parent", v)) => parents.push(v.to_string()),
                Some(("author", v)) => author = Ident::parse(v),
                Some(("committer", v)) => committer = Ident::parse(v),
                Some(("gpgsig" | "gpgsig-sha256", _)) => signed = true,
                _ => {}
            }
        }
        Some(Self {
            tree: tree?,
            parents,
            author: author?,
            committer: committer?,
            signed,
            message,
        })
    }

    fn identity_env(&self) -> Vec<(String, String)> {
        [("AUTHOR", &self.author), ("COMMITTER", &self.committer)]
            .into_iter()
            .flat_map(|(who, ident)| {
                [
                    (format!("GIT_{who}_NAME"), ident.name.clone()),
                    (format!("GIT_{who}_EMAIL"), ident.email.clone()),
                    (format!("GIT_{who}_DATE"), ident.date.clone()),
                ]
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn only_the_scopes_the_user_owns_decide() {
        assert!(signing_requested("global\ttrue\n"));
        assert!(signing_requested("system\ttrue\n"));
        // The clone's own scope is where provisioning wrote `false`.
        assert!(signing_requested("global\ttrue\nlocal\tfalse\n"));
        assert!(!signing_requested("local\ttrue\n"));
        assert!(!signing_requested("worktree\ttrue\n"));
        assert!(!signing_requested("command\ttrue\n"));
        // Later scopes override earlier ones.
        assert!(!signing_requested("system\ttrue\nglobal\tfalse\n"));
        assert!(!signing_requested(""));
    }

    fn run(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit_file(repo: &Path, name: &str, extra: &[&str]) -> String {
        std::fs::write(repo.join(name), name).unwrap();
        run(repo, &["add", "-A"]);
        let mut args = vec!["commit", "-q", "-m", name];
        args.extend_from_slice(extra);
        run(repo, &args);
        run(repo, &["rev-parse", "HEAD"])
    }

    /// A repo on `main` with one commit pushed to a bare origin, signing set
    /// up with a throwaway ssh key the test generates. Returns (repo, base).
    fn signing_repo(dir: &Path) -> (PathBuf, String) {
        let origin = dir.join("origin.git");
        run(dir, &["init", "-q", "--bare", origin.to_str().unwrap()]);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["config", "user.email", "me@example.com"]);
        run(&repo, &["config", "user.name", "Me"]);
        run(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        let base = commit_file(&repo, "base.txt", &[]);
        run(&repo, &["push", "-q", "-u", "origin", "main"]);

        let key = dir.join("signing_key");
        let out = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f", key.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let public = std::fs::read_to_string(dir.join("signing_key.pub")).unwrap();
        let mut parts = public.split_whitespace();
        let (kind, blob) = (parts.next().unwrap(), parts.next().unwrap());
        let signers = dir.join("allowed_signers");
        std::fs::write(&signers, format!("me@example.com {kind} {blob}\n")).unwrap();
        run(&repo, &["config", "gpg.format", "ssh"]);
        run(&repo, &["config", "user.signingkey", key.to_str().unwrap()]);
        run(
            &repo,
            &[
                "config",
                "gpg.ssh.allowedSignersFile",
                signers.to_str().unwrap(),
            ],
        );
        (repo, base)
    }

    /// Everything a copy must preserve, per commit, oldest first.
    fn shape(repo: &Path, range: &str) -> Vec<String> {
        run(
            repo,
            &[
                "log",
                "--reverse",
                "--date=raw",
                "--format=%T|%an|%ae|%ad|%cn|%ce|%cd|%B",
                range,
            ],
        )
        .lines()
        .map(str::to_string)
        .collect()
    }

    fn verified(repo: &Path, rev: &str) -> bool {
        std::process::Command::new("git")
            .current_dir(repo)
            .args(["verify-commit", rev])
            .output()
            .unwrap()
            .status
            .success()
    }

    #[tokio::test]
    async fn unpublished_commits_become_signed_copies_and_published_history_is_kept() {
        let td = tempfile::tempdir().unwrap();
        let (repo, base) = signing_repo(td.path());
        let one = commit_file(&repo, "one.txt", &["--author=Them <them@example.com>"]);
        let tip = commit_file(&repo, "two.txt", &[]);
        assert!(!verified(&repo, &tip), "fixture commits are unsigned");
        let before = shape(&repo, "origin/main..main");

        let signed = sign_unpublished(&repo, "main", &tip).await.unwrap();

        assert_ne!(signed, tip);
        assert_eq!(run(&repo, &["rev-parse", "refs/heads/main"]), signed);
        assert_eq!(
            run(&repo, &["rev-list", "--count", "origin/main..main"]),
            "2"
        );
        assert!(
            verified(&repo, &signed),
            "the tip must carry a valid signature"
        );
        assert!(verified(&repo, "main~1"), "so must the commit under it");
        assert_eq!(
            shape(&repo, "origin/main..main"),
            before,
            "a copy changes nothing else"
        );
        assert_eq!(
            run(&repo, &["rev-parse", "main~2"]),
            base,
            "the published commit keeps its hash"
        );
        assert_ne!(run(&repo, &["rev-parse", "main~1"]), one);
        assert_eq!(
            run(&repo, &["status", "--porcelain"]),
            "",
            "the tree is untouched"
        );

        // Already signed: nothing to do, and the tip stays.
        assert_eq!(
            sign_unpublished(&repo, "main", &signed).await.unwrap(),
            signed
        );
    }

    #[tokio::test]
    async fn a_merge_in_the_range_keeps_both_parents() {
        let td = tempfile::tempdir().unwrap();
        let (repo, base) = signing_repo(td.path());
        commit_file(&repo, "one.txt", &[]);
        run(&repo, &["checkout", "-q", "-b", "side", &base]);
        commit_file(&repo, "side.txt", &[]);
        run(&repo, &["checkout", "-q", "main"]);
        run(
            &repo,
            &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
        );
        let tip = run(&repo, &["rev-parse", "HEAD"]);

        let signed = sign_unpublished(&repo, "main", &tip).await.unwrap();

        assert!(verified(&repo, &signed));
        let parents = run(&repo, &["log", "-1", "--format=%P", &signed]);
        assert_eq!(parents.split_whitespace().count(), 2, "{parents}");
        for parent in parents.split_whitespace() {
            assert!(verified(&repo, parent), "each side was signed too");
        }
        assert_eq!(run(&repo, &["rev-parse", &format!("{signed}^1~1")]), base);
        assert_eq!(run(&repo, &["rev-parse", &format!("{signed}^2~1")]), base);
    }

    #[tokio::test]
    async fn nothing_unpublished_leaves_the_tip_alone() {
        let td = tempfile::tempdir().unwrap();
        let (repo, base) = signing_repo(td.path());
        assert_eq!(sign_unpublished(&repo, "main", &base).await.unwrap(), base);
        assert_eq!(run(&repo, &["rev-parse", "refs/heads/main"]), base);
    }

    #[tokio::test]
    async fn a_branch_that_moved_since_the_tip_was_read_is_not_overwritten() {
        let td = tempfile::tempdir().unwrap();
        let (repo, _base) = signing_repo(td.path());
        let tip = commit_file(&repo, "one.txt", &[]);
        // Something lands after the caller read `tip`.
        commit_file(&repo, "two.txt", &[]);
        let err = sign_unpublished(&repo, "main", &tip).await.unwrap_err();
        assert!(err.to_string().contains("signed tip"), "{err}");
    }

    #[tokio::test]
    async fn an_unusable_key_is_an_error_not_an_unsigned_push() {
        let td = tempfile::tempdir().unwrap();
        let (repo, _base) = signing_repo(td.path());
        let tip = commit_file(&repo, "one.txt", &[]);
        run(&repo, &["config", "user.signingkey", "/nonexistent/key"]);
        let err = sign_unpublished(&repo, "main", &tip).await.unwrap_err();
        assert!(err.to_string().contains("signing failed"), "{err}");
        assert_eq!(run(&repo, &["rev-parse", "refs/heads/main"]), tip);
    }
}

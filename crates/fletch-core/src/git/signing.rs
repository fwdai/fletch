//! Signing on publish. An agent's clone commits unsigned — the sandbox holds no
//! key, provisioning freezes `commit.gpgsign=false` into the clone
//! (`sandbox::provision::seed_identity`), and host-side git never signs
//! implicitly in any checkout (`hardening::NEUTRALISED`) — so when the user's
//! own repository signs its commits, the host signs what a push publishes, at
//! the push. That is the moment the user takes responsibility for the commits
//! (the approval prompt sits right before it), so it is the honest moment for
//! the user's key to vouch for them. [`super::publish`] is the one caller.
//!
//! Each unpublished commit is replaced by a signed copy with the same tree,
//! parents, author, committer and message, parents mapped to their own signed
//! copies. Only the commits origin lacks are rewritten, so published history
//! keeps its hashes and the push stays a fast-forward.
//!
//! **Where the signer runs is the security boundary.** `commit-tree -S` reads
//! `user.signingKey`, `gpg.format`, `gpg.<format>.program` and
//! `gpg.ssh.defaultKeyCommand` — a key, and programs — from the repository it
//! runs in. Run in the agent's checkout, an agent-written `.git/config` would
//! choose the host's signing key or run a command on the host. So the signer
//! runs in the **user's own repository** (the clone's source), which no agent
//! can write, with `GIT_OBJECT_DIRECTORY` pointed at the checkout's object
//! store: the config is the user's, the objects are the checkout's. Nothing
//! from the checkout but trees, commits and messages reaches it.
//!
//! Two more things are deliberately not trusted. The clone's `origin/*` refs:
//! they are what the last fetch left, and a clone whose spawn-time fetch
//! failed carries the source repo's local heads under that name, so they are
//! refreshed from origin first ([`refresh_remote_refs`]). And a signature
//! already on an unpublished commit: the clone's `.git` is agent-writable, so
//! a `gpgsig` header there is just bytes — every unpublished commit is
//! re-signed, whatever it claims.
//!
//! Residual: a commit's `encoding` and `mergetag` headers are not carried over
//! (neither has a `commit-tree` spelling). Fletch never produces either.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::cmd::{git_output, git_output_stdin, output_timed};
use crate::error::{Error, Result};

/// Whether commits in `source_repo` — the user's own repository — are signed:
/// `commit.gpgsign` as the user's own `git commit` there would see it. Read
/// through `--show-scope` so the `-c commit.gpgsign=false` every host-side
/// invocation carries (`hardening::NEUTRALISED`, the `command` scope) is not
/// mistaken for the user's setting.
///
/// Never asked of an agent checkout: its config is the agent's to write.
pub async fn host_signs_commits(source_repo: &Path) -> Result<bool> {
    let out = git_output(
        source_repo,
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
/// the last value from any scope but `command` decides.
pub(crate) fn signing_requested(show_scope: &str) -> bool {
    show_scope
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .rfind(|(scope, _)| *scope != "command")
        .is_some_and(|(_, value)| value.trim() == "true")
}

/// `git fetch --prune` of every branch: the checkout's `origin/*` refs as the
/// remote has them right now, so "what origin lacks" is answered by the remote
/// rather than by whatever the last fetch left — the source repo's own local
/// heads, when the spawn-time fetch failed (`provision`), or a branch since
/// deleted. The remote is the user's repository's (`git::remote`), never the
/// checkout's `origin`; runs with the host's token, like the push that
/// follows it.
pub(crate) async fn refresh_remote_refs(checkout: &Path, source_repo: &Path) -> Result<()> {
    // A fetch runs the transport-executing keys (`core.sshCommand`,
    // `remote.<name>.uploadpack`) the `-c` overrides cannot neutralise.
    super::hardening::refuse_steerable_config(checkout).await?;
    let url = super::remote::trusted_remote(checkout, source_repo).await?;
    let mut cmd = crate::git_dist::command(checkout);
    cmd.args([
        "fetch",
        "--quiet",
        "--prune",
        &url.fetch,
        super::remote::ALL_TRACKING_REFSPEC,
    ]);
    for (k, v) in crate::github::git_auth_env() {
        cmd.env(k, v);
    }
    let out = output_timed(&mut cmd, "git fetch").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "refreshing origin before signing failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Replace the commits `tip` reaches that origin lacks with signed copies and
/// return the new tip — `tip` itself when origin already has everything. No
/// ref is moved: the caller owns the branch. "What origin lacks" is read from
/// the checkout's `origin/*` refs as they stand, so refresh them first. Every
/// commit in the range is signed afresh, including one that already carries a
/// `gpgsig` header: the checkout is agent-writable, so an existing header
/// proves nothing. The signatures are made in `source_repo` (see the module
/// doc), which is also where a signing failure comes from: a key the host
/// cannot use is an error, never an unsigned result.
pub async fn sign_unpublished(checkout: &Path, source_repo: &Path, tip: &str) -> Result<String> {
    let out = git_output(
        checkout,
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
    if unpublished.is_empty() {
        return Ok(tip.to_string());
    }
    let objects = object_directory(checkout).await?;

    // Parents come before children (`--reverse` of topo order), so a parent's
    // signed copy exists by the time its child is rewritten.
    let mut signed: HashMap<String, String> = HashMap::new();
    for sha in &unpublished {
        let raw = git_output(checkout, &["cat-file", "commit", sha]).await?;
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
        let copy = commit_signed(source_repo, &objects, &commit, &parents).await?;
        signed.insert(sha.clone(), copy);
    }
    Ok(signed.get(tip).cloned().unwrap_or_else(|| tip.to_string()))
}

/// The checkout's object store — the common dir's, so a linked worktree
/// resolves to the store it shares.
async fn object_directory(checkout: &Path) -> Result<PathBuf> {
    let out = git_output(
        checkout,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "rev-parse --git-common-dir failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()).join("objects"))
}

/// A signed copy of `commit` over `parents`, written into `objects`:
/// `commit-tree -S` run in `source_repo` so the signing config is the user's,
/// with the original identities and dates in the env and the message on
/// stdin, the one form that keeps it byte for byte. `-S` is explicit, so the
/// `commit.gpgsign=false` every host-side invocation carries does not apply.
async fn commit_signed(
    source_repo: &Path,
    objects: &Path,
    commit: &RawCommit,
    parents: &[String],
) -> Result<String> {
    let mut args = vec!["commit-tree", "-S", commit.tree.as_str()];
    for parent in parents {
        args.push("-p");
        args.push(parent);
    }
    let mut env = commit.identity_env();
    env.push((
        "GIT_OBJECT_DIRECTORY".to_string(),
        objects.to_string_lossy().into_owned(),
    ));
    let out = git_output_stdin(source_repo, &args, &env, &commit.message).await?;
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
        for line in headers.lines() {
            // A continuation line of a multi-line header (a signature body).
            // Any `gpgsig` header is dropped with it: the copy gets its own.
            if line.starts_with(' ') {
                continue;
            }
            match line.split_once(' ') {
                Some(("tree", v)) => tree = Some(v.to_string()),
                Some(("parent", v)) => parents.push(v.to_string()),
                Some(("author", v)) => author = Ident::parse(v),
                Some(("committer", v)) => committer = Ident::parse(v),
                _ => {}
            }
        }
        Some(Self {
            tree: tree?,
            parents,
            author: author?,
            committer: committer?,
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
pub(crate) mod fixtures {
    //! A user's repo with signing set up and a `--shared` agent clone of it,
    //! for this module's and `publish`'s tests.

    use std::path::{Path, PathBuf};

    use super::super::testing::{commit_file, git, init_repo, ssh_signing};

    /// `source` (the user's repo, origin = `origin.git`, one commit pushed,
    /// signing with a throwaway ssh key and verification wired to it),
    /// `checkout` (a `--shared` clone of it with the same origin) and the
    /// pushed base commit.
    pub(crate) struct Signing {
        pub source: PathBuf,
        pub checkout: PathBuf,
        pub base: String,
    }

    pub(crate) fn signing_repos(dir: &Path) -> Signing {
        let origin = dir.join("origin.git");
        git(dir, &["init", "-q", "--bare", origin.to_str().unwrap()]);
        let source = dir.join("source");
        init_repo(&source);
        git(
            &source,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        let base = commit_file(&source, "base.txt");
        git(&source, &["push", "-q", "-u", "origin", "main"]);

        ssh_signing(dir, &source);

        let checkout = dir.join("checkout");
        git(
            dir,
            &[
                "clone",
                "-q",
                "--shared",
                source.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ],
        );
        git(
            &checkout,
            &["remote", "set-url", "origin", origin.to_str().unwrap()],
        );
        git(&checkout, &["fetch", "-q", "--prune", "origin"]);
        git(&checkout, &["config", "user.email", "me@example.com"]);
        git(&checkout, &["config", "user.name", "Me"]);
        // As provisioning leaves a clone.
        git(&checkout, &["config", "commit.gpgsign", "false"]);
        Signing {
            source,
            checkout,
            base,
        }
    }

    /// `git verify-commit` in `source`, where the allowed signers live, of a
    /// commit in `checkout`'s store.
    pub(crate) fn verified(fx: &Signing, rev: &str) -> bool {
        std::process::Command::new("git")
            .current_dir(&fx.source)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                fx.checkout.join(".git").join("objects"),
            )
            .args(["verify-commit", rev])
            .output()
            .unwrap()
            .status
            .success()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::Command;

    use super::super::testing::{commit_file, git};
    use super::fixtures::{signing_repos, verified, Signing};
    use super::*;

    #[test]
    fn every_scope_but_the_command_line_decides() {
        assert!(signing_requested("global\ttrue\n"));
        assert!(signing_requested("system\ttrue\n"));
        assert!(signing_requested("local\ttrue\n"));
        // The user's repo may switch signing off for itself.
        assert!(!signing_requested("global\ttrue\nlocal\tfalse\n"));
        // The `-c commit.gpgsign=false` every host-side invocation carries.
        assert!(signing_requested("global\ttrue\ncommand\tfalse\n"));
        assert!(!signing_requested("command\ttrue\n"));
        assert!(!signing_requested("system\ttrue\nglobal\tfalse\n"));
        assert!(!signing_requested(""));
    }

    /// Everything a copy must preserve, per commit, oldest first.
    fn shape(repo: &Path, range: &str) -> Vec<String> {
        git(
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

    #[tokio::test]
    async fn the_users_repo_decides_whether_the_host_signs() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        assert!(host_signs_commits(&fx.source).await.unwrap());
        git(&fx.source, &["config", "commit.gpgsign", "false"]);
        assert!(!host_signs_commits(&fx.source).await.unwrap());
        git(&fx.source, &["config", "--unset", "commit.gpgsign"]);
        // Unset locally: whatever the developer's global config says, which
        // this test cannot know — only that the read does not fail.
        host_signs_commits(&fx.source).await.unwrap();
    }

    #[tokio::test]
    async fn unpublished_commits_become_signed_copies_and_published_history_is_kept() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let repo = fx.checkout.as_path();
        std::fs::write(repo.join("one.txt"), b"one").unwrap();
        git(repo, &["add", "-A"]);
        git(
            repo,
            &[
                "commit",
                "-q",
                "-m",
                "one",
                "--author=Them <them@example.com>",
            ],
        );
        let one = git(repo, &["rev-parse", "HEAD"]);
        let tip = commit_file(repo, "two.txt");
        assert!(!verified(&fx, &tip), "fixture commits are unsigned");
        let before = shape(repo, "origin/main..main");

        let signed = sign_unpublished(repo, &fx.source, &tip).await.unwrap();

        assert_ne!(signed, tip);
        assert_eq!(
            git(
                repo,
                &["rev-list", "--count", &format!("origin/main..{signed}")]
            ),
            "2"
        );
        assert!(
            verified(&fx, &signed),
            "the tip must carry a valid signature"
        );
        assert!(
            verified(&fx, &format!("{signed}~1")),
            "so must the commit under it"
        );
        assert_eq!(
            shape(repo, &format!("origin/main..{signed}")),
            before,
            "a copy changes nothing else"
        );
        assert_eq!(
            git(repo, &["rev-parse", &format!("{signed}~2")]),
            fx.base,
            "the published commit keeps its hash"
        );
        assert_ne!(git(repo, &["rev-parse", &format!("{signed}~1")]), one);
        // No ref moved: that is the publication layer's call.
        assert_eq!(git(repo, &["rev-parse", "refs/heads/main"]), tip);
        assert_eq!(git(repo, &["status", "--porcelain"]), "");
        // The copies live in the checkout's store, not the user's repo.
        assert!(repo
            .join(".git/objects")
            .join(&signed[..2])
            .join(&signed[2..])
            .exists());
        assert!(!fx
            .source
            .join(".git/objects")
            .join(&signed[..2])
            .join(&signed[2..])
            .exists());
    }

    /// Append `config` to the checkout's `.git/config` directly, as an agent
    /// would — not through Fletch's git.
    fn plant(checkout: &Path, config: &str) {
        std::fs::OpenOptions::new()
            .append(true)
            .open(checkout.join(".git/config"))
            .unwrap()
            .write_all(config.as_bytes())
            .unwrap();
    }

    /// The checkout's own signing config is the agent's to write, so none of
    /// it may reach the signer. Two plants, each alone so the other cannot
    /// mask it (git never runs the key command once a key is named): a key of
    /// the agent's own, which must not be what signs; and a key command,
    /// which must not run on the host.
    #[tokio::test]
    async fn the_checkouts_signing_config_never_reaches_the_signer() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let repo = fx.checkout.as_path();

        let planted = td.path().join("planted_key");
        let out = std::process::Command::new("ssh-keygen")
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-f",
                planted.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(out.status.success());
        let tip = commit_file(repo, "one.txt");
        plant(
            repo,
            &format!(
                "[user]\n\tsigningkey = {}\n[gpg]\n\tformat = ssh\n[commit]\n\tgpgsign = true\n",
                planted.display()
            ),
        );
        let signed = sign_unpublished(repo, &fx.source, &tip).await.unwrap();
        // Only the user's key is in the allowed signers, so this is the proof.
        assert!(
            verified(&fx, &signed),
            "the signature must be the user's key, not the planted one"
        );

        git(repo, &["update-ref", "refs/heads/main", &signed]);
        let tip = commit_file(repo, "two.txt");
        git(repo, &["config", "--unset", "user.signingkey"]);
        let marker = td.path().join("ran-on-host");
        plant(
            repo,
            &format!(
                "[gpg \"ssh\"]\n\tdefaultKeyCommand = touch {}\n",
                marker.display()
            ),
        );
        let signed = sign_unpublished(repo, &fx.source, &tip).await.unwrap();
        assert!(
            !marker.exists(),
            "the checkout's key command ran on the host"
        );
        assert!(verified(&fx, &signed));
    }

    /// The checkout's `.git` is agent-writable, so a `gpgsig` header on an
    /// unpublished commit is only a claim. A commit forged with one is
    /// re-signed like any other, never published on the strength of it.
    #[tokio::test]
    async fn a_forged_signature_header_is_replaced_not_trusted() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let repo = fx.checkout.as_path();
        let honest = commit_file(repo, "one.txt");
        let raw = git(repo, &["cat-file", "commit", &honest]);
        let (headers, message) = raw.split_once("\n\n").unwrap();
        let forged = format!(
            "{headers}\ngpgsig -----BEGIN SSH SIGNATURE-----\n not a signature\n -----END SSH SIGNATURE-----\n\n{message}\n"
        );
        let out = std::process::Command::new("git")
            .current_dir(repo)
            .args(["hash-object", "-t", "commit", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                child.stdin.take().unwrap().write_all(forged.as_bytes())?;
                child.wait_with_output()
            })
            .unwrap();
        let fake = String::from_utf8_lossy(&out.stdout).trim().to_string();
        git(repo, &["update-ref", "refs/heads/main", &fake]);
        assert!(!verified(&fx, &fake), "the forgery must not verify");

        let signed = sign_unpublished(repo, &fx.source, &fake).await.unwrap();

        assert_ne!(signed, fake);
        assert!(verified(&fx, &signed));
    }

    #[tokio::test]
    async fn a_merge_in_the_range_keeps_both_parents() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let repo = fx.checkout.as_path();
        commit_file(repo, "one.txt");
        git(repo, &["checkout", "-q", "-b", "side", &fx.base]);
        commit_file(repo, "side.txt");
        git(repo, &["checkout", "-q", "main"]);
        git(
            repo,
            &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
        );
        let tip = git(repo, &["rev-parse", "HEAD"]);

        let signed = sign_unpublished(repo, &fx.source, &tip).await.unwrap();

        assert!(verified(&fx, &signed));
        let parents = git(repo, &["log", "-1", "--format=%P", &signed]);
        assert_eq!(parents.split_whitespace().count(), 2, "{parents}");
        for parent in parents.split_whitespace() {
            assert!(verified(&fx, parent), "each side was signed too");
        }
        assert_eq!(git(repo, &["rev-parse", &format!("{signed}^1~1")]), fx.base);
        assert_eq!(git(repo, &["rev-parse", &format!("{signed}^2~1")]), fx.base);
    }

    #[tokio::test]
    async fn nothing_unpublished_leaves_the_tip_alone() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        assert_eq!(
            sign_unpublished(&fx.checkout, &fx.source, &fx.base)
                .await
                .unwrap(),
            fx.base
        );
    }

    #[tokio::test]
    async fn an_unusable_key_is_an_error_not_an_unsigned_result() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let tip = commit_file(&fx.checkout, "one.txt");
        git(
            &fx.source,
            &["config", "user.signingkey", "/nonexistent/key"],
        );
        let err = sign_unpublished(&fx.checkout, &fx.source, &tip)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("signing failed"), "{err}");
    }

    // ── The environment the signer's helpers read ──────────────────────────
    //
    // A Dock-launched app inherits launchd's sparse environment, not the
    // user's shell's: no Homebrew on PATH, no ssh-agent socket a shell rc
    // exports, no GNUPGHOME. Host-side git carries those from the login shell
    // (`git_dist::helper_env`), so the signer finds the user's gpg, agent and
    // keyring the way their terminal does. Each test below stands a login
    // shell in (`bin_resolve::with_login_shell_env`, thread-local) and signs
    // on that thread, first with a shell that exports nothing — the process's
    // own environment, which cannot reach the key — then with the shell that
    // can.

    /// `sign_unpublished` on this thread with `shell` as the login shell's
    /// exported environment.
    fn sign_with_shell_env(fx: &Signing, tip: &str, shell: &[(&str, &str)]) -> Result<String> {
        crate::bin_resolve::with_login_shell_env(shell, || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(sign_unpublished(&fx.checkout, &fx.source, tip))
        })
    }

    fn signing_failed(err: Error) {
        assert!(err.to_string().contains("signing failed"), "{err}");
    }

    fn on_process_path(name: &str) -> PathBuf {
        std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(name))
                    .find(|candidate| candidate.is_file())
            })
            .unwrap_or_else(|| panic!("{name} on PATH"))
    }

    /// A signing program that exists under its configured name only on the
    /// login shell's PATH — the Homebrew `gpg` a GUI launch cannot see.
    #[test]
    fn the_signing_program_is_found_on_the_login_shells_path() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let bin = td.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        std::os::unix::fs::symlink(
            on_process_path("ssh-keygen"),
            bin.join("fletch-test-ssh-keygen"),
        )
        .unwrap();
        git(
            &fx.source,
            &["config", "gpg.ssh.program", "fletch-test-ssh-keygen"],
        );
        let tip = commit_file(&fx.checkout, "one.txt");

        signing_failed(sign_with_shell_env(&fx, &tip, &[]).unwrap_err());

        let shell_path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let signed =
            sign_with_shell_env(&fx, &tip, &[("PATH", shell_path.to_str().unwrap())]).unwrap();
        // Verification runs outside the stub, so it names the program by path.
        git(&fx.source, &["config", "--unset", "gpg.ssh.program"]);
        assert!(verified(&fx, &signed));
    }

    /// An ssh-agent for one test, holding `key`, on a socket beside it (the
    /// default under `~/.ssh/agent` is not writable everywhere tests run);
    /// killed on drop.
    struct Agent {
        sock: String,
        pid: String,
    }

    impl Agent {
        fn start(key: &Path) -> Self {
            let sock = key.with_file_name("agent.sock");
            let out = Command::new("ssh-agent")
                .args(["-s", "-a"])
                .arg(&sock)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "ssh-agent: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let text = String::from_utf8_lossy(&out.stdout);
            let pid = text
                .lines()
                .find_map(|line| line.strip_prefix("SSH_AGENT_PID="))
                .and_then(|rest| rest.split(';').next())
                .unwrap_or_else(|| panic!("SSH_AGENT_PID in ssh-agent output: {text}"))
                .to_string();
            let agent = Self {
                sock: sock.to_string_lossy().into_owned(),
                pid,
            };
            let out = Command::new("ssh-add")
                .env("SSH_AUTH_SOCK", &agent.sock)
                .arg(key)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "ssh-add: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            agent
        }
    }

    impl Drop for Agent {
        fn drop(&mut self) {
            let _ = Command::new("ssh-agent")
                .arg("-k")
                .env("SSH_AGENT_PID", &self.pid)
                .output();
        }
    }

    /// A key the user names by its public half (`key::…`) and keeps in an
    /// agent — 1Password's, or one a shell rc starts — is reached through the
    /// socket that shell exports.
    #[test]
    fn an_agent_held_key_signs_through_the_login_shells_socket() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let key = td.path().join("signing_key");
        let agent = Agent::start(&key);
        let public = std::fs::read_to_string(td.path().join("signing_key.pub")).unwrap();
        git(
            &fx.source,
            &[
                "config",
                "user.signingkey",
                &format!("key::{}", public.trim()),
            ],
        );
        // The private half lives in the agent only.
        std::fs::remove_file(&key).unwrap();
        let tip = commit_file(&fx.checkout, "one.txt");

        signing_failed(sign_with_shell_env(&fx, &tip, &[]).unwrap_err());

        let signed = sign_with_shell_env(&fx, &tip, &[("SSH_AUTH_SOCK", &agent.sock)]).unwrap();
        assert!(verified(&fx, &signed));
    }

    /// A gpg keyring for one test under its own `GNUPGHOME`, with one
    /// passphrase-less signing key; the agent gpg starts there is killed on
    /// drop. `None` when the machine has no gpg.
    struct Gpg {
        home: PathBuf,
        fingerprint: String,
    }

    impl Gpg {
        fn create(dir: &Path) -> Option<Self> {
            let present = Command::new("gpg")
                .arg("--version")
                .output()
                .is_ok_and(|out| out.status.success());
            if !present {
                return None;
            }
            let home = Self::home(dir, "gnupg");
            let out = Command::new("gpg")
                .env("GNUPGHOME", &home)
                .args([
                    "--batch",
                    "--pinentry-mode",
                    "loopback",
                    "--passphrase",
                    "",
                    "--quick-generate-key",
                    "Me <me@example.com>",
                    "ed25519",
                    "sign",
                    "never",
                ])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "gpg --quick-generate-key: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let out = Command::new("gpg")
                .env("GNUPGHOME", &home)
                .args(["--list-secret-keys", "--with-colons"])
                .output()
                .unwrap();
            let fingerprint = String::from_utf8_lossy(&out.stdout)
                .lines()
                .find(|line| line.starts_with("fpr:"))
                .and_then(|line| line.split(':').nth(9))
                .expect("a fingerprint")
                .to_string();
            Some(Self { home, fingerprint })
        }

        /// An empty keyring directory with the permissions gpg insists on.
        fn home(dir: &Path, name: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let home = dir.join(name);
            std::fs::create_dir(&home).unwrap();
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
            home
        }

        /// `git verify-commit` against this keyring, of a commit in the
        /// checkout's store.
        fn verified(&self, fx: &Signing, rev: &str) -> bool {
            Command::new("git")
                .current_dir(&fx.source)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GNUPGHOME", &self.home)
                .env(
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                    fx.checkout.join(".git").join("objects"),
                )
                .args(["verify-commit", rev])
                .output()
                .unwrap()
                .status
                .success()
        }
    }

    impl Drop for Gpg {
        fn drop(&mut self) {
            let _ = Command::new("gpgconf")
                .env("GNUPGHOME", &self.home)
                .args(["--kill", "gpg-agent"])
                .output();
        }
    }

    /// The other signing format: a gpg key, in the keyring the login shell's
    /// `GNUPGHOME` names.
    #[test]
    fn a_gpg_key_signs_through_the_login_shells_gnupghome() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let Some(gpg) = Gpg::create(td.path()) else {
            eprintln!("skipped: no gpg on this machine");
            return;
        };
        git(&fx.source, &["config", "gpg.format", "openpgp"]);
        git(&fx.source, &["config", "user.signingkey", &gpg.fingerprint]);
        let tip = commit_file(&fx.checkout, "one.txt");

        // A keyring without the key — not the developer's own, which the
        // test must not touch.
        let empty = Gpg::home(td.path(), "empty-gnupg");
        signing_failed(
            sign_with_shell_env(&fx, &tip, &[("GNUPGHOME", empty.to_str().unwrap())]).unwrap_err(),
        );

        let signed =
            sign_with_shell_env(&fx, &tip, &[("GNUPGHOME", gpg.home.to_str().unwrap())]).unwrap();
        assert!(gpg.verified(&fx, &signed));
    }
}

//! Git for tests, isolated from the developer's own configuration.
//!
//! [`isolate_git_config`] runs before any test and points the whole test
//! process away from `~/.gitconfig` and the system config: a machine whose
//! global config signs every commit with a key the test process cannot reach
//! (a sandboxed agent on that machine, exactly the case `git::signing` is for)
//! would otherwise fail every commit — a fixture's and the code under test's —
//! before the assertion they were made for. What a test wants git to believe,
//! it writes into the repo's own config ([`ssh_signing`] for a signing
//! policy); nothing else reaches git.
//!
//! The helpers below set the same two variables themselves, so a fixture
//! reads the same with or without the constructor.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// No global or system git config for this process. A constructor, not a
/// per-test call: the code under test spawns git with the process env, and a
/// variable set from one test would race every other.
#[ctor::ctor]
fn isolate_git_config() {
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
}

/// `git <args>` in `dir` with no global or system config, as raw output.
pub(crate) fn git_output(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .unwrap()
}

/// `git <args>` in `dir`, asserting success; trimmed stdout.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_output(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `git init -b main` with a fixed identity.
pub(crate) fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "me@example.com"]);
    git(dir, &["config", "user.name", "Me"]);
}

/// Write `name` with `name` as content, stage everything and commit it as
/// `name`; the new HEAD.
pub(crate) fn commit_file(dir: &Path, name: &str) -> String {
    std::fs::write(dir.join(name), name).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", name]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A signing policy in `repo`'s own config, the way a user's repo signs: a
/// throwaway ssh key generated under `dir`, `gpg.format=ssh`,
/// `commit.gpgsign=true`, and an allowed-signers file naming
/// `me@example.com` so `git verify-commit` can confirm the key.
pub(crate) fn ssh_signing(dir: &Path, repo: &Path) {
    let key = dir.join("signing_key");
    let out = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f", key.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "ssh-keygen: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let public = std::fs::read_to_string(dir.join("signing_key.pub")).unwrap();
    let mut parts = public.split_whitespace();
    let (kind, blob) = (parts.next().unwrap(), parts.next().unwrap());
    let signers = dir.join("allowed_signers");
    std::fs::write(&signers, format!("me@example.com {kind} {blob}\n")).unwrap();
    git(repo, &["config", "gpg.format", "ssh"]);
    git(repo, &["config", "user.signingkey", key.to_str().unwrap()]);
    git(
        repo,
        &[
            "config",
            "gpg.ssh.allowedSignersFile",
            signers.to_str().unwrap(),
        ],
    );
    git(repo, &["config", "commit.gpgsign", "true"]);
}

/// Whether `rev` in `repo` carries a signature `repo`'s allowed signers
/// confirm ([`ssh_signing`]).
pub(crate) fn verified(repo: &Path, rev: &str) -> bool {
    git_output(repo, &["verify-commit", rev]).status.success()
}

/// A bare remote under `dir` whose default branch is `main`, so a clone of
/// it is born on `main` once `main` is pushed.
pub(crate) fn bare(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    git(dir, &["init", "-q", "--bare", path.to_str().unwrap()]);
    git(&path, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    path
}

/// A plain clone of `from` at `dir/name` with its own identity and no
/// signing: someone else moving a remote from outside.
pub(crate) fn clone_elsewhere(dir: &Path, from: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    git(
        dir,
        &[
            "clone",
            "-q",
            from.to_str().unwrap(),
            path.to_str().unwrap(),
        ],
    );
    git(&path, &["config", "user.email", "other@example.com"]);
    git(&path, &["config", "user.name", "Other"]);
    git(&path, &["config", "commit.gpgsign", "false"]);
    path
}

/// A user's repository and an agent checkout of it, as provisioning leaves
/// them.
pub(crate) struct Repos {
    /// The bare remote, default branch `main`.
    pub origin: PathBuf,
    /// The user's repository: `origin` set, `base` pushed to `main`.
    pub source: PathBuf,
    /// A `--shared` clone of `source` with `origin` pointed at the bare
    /// remote, an identity of its own and implicit signing off.
    pub checkout: PathBuf,
    /// The one commit pushed.
    pub base: String,
}

pub(crate) fn repos(dir: &Path) -> Repos {
    let origin = bare(dir, "origin.git");
    let source = dir.join("source");
    init_repo(&source);
    git(
        &source,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    let base = commit_file(&source, "base.txt");
    git(&source, &["push", "-q", "origin", "main"]);
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
    git(&checkout, &["config", "user.email", "me@example.com"]);
    git(&checkout, &["config", "user.name", "Me"]);
    git(&checkout, &["config", "commit.gpgsign", "false"]);
    Repos {
        origin,
        source,
        checkout,
        base,
    }
}

/// [`repos`] with `source` signing its commits ([`ssh_signing`], the key
/// under `dir`).
pub(crate) fn signing_repos(dir: &Path) -> Repos {
    let fx = repos(dir);
    ssh_signing(dir, &fx.source);
    fx
}

impl Repos {
    /// `git verify-commit` in `source`, where the allowed signers live, of a
    /// commit in the checkout's store.
    pub(crate) fn verified(&self, rev: &str) -> bool {
        Command::new("git")
            .current_dir(&self.source)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                self.checkout.join(".git").join("objects"),
            )
            .args(["verify-commit", rev])
            .output()
            .unwrap()
            .status
            .success()
    }
}

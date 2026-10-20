//! Git for test fixtures, isolated from the developer's own configuration.
//!
//! A fixture's `git commit` must not read `~/.gitconfig`: a machine whose
//! global config signs every commit with a key the test process cannot reach
//! (a sandboxed agent on that machine, exactly the case `git::signing` is for)
//! would otherwise fail every fixture before the test under it even ran. Only
//! the fixture is isolated — the code under test still runs git as the host
//! does, so what it reads from the user's config is tested, not hidden; a
//! fixture that wants a signing policy writes it into the repo's own config.

use std::path::Path;
use std::process::{Command, Output};

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

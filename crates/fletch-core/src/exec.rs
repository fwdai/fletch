//! Where work rooted at a path runs: on this host, or inside a sandbox that
//! holds the path's tree.
//!
//! Dozens of call sites resolve an agent's checkout and then run git in it or
//! read and write files under it. Under the box engine that tree exists only
//! inside the guest — at the *same absolute paths* (path identity, spec §1), so
//! nothing here translates a path. Instead every such operation asks
//! [`for_path`] once and gets back an [`Executor`] that does the work locally or
//! through the engine's channel. With no remote root registered, which is every
//! current engine, [`for_path`] is always [`Executor::Local`] and each method is
//! exactly the `std` call it replaced.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

/// A sandbox that owns a tree the host cannot reach directly. Implemented in a
/// later phase by the box engine over its SSH channel; a trait here so
/// `fletch-core` never depends on the runtime crate.
///
/// Every method takes the host-side absolute path, which by path identity is
/// also the guest's.
pub trait RemoteExecutor: Send + Sync + fmt::Debug {
    /// A host-side command that runs `program` in `cwd` inside the remote,
    /// with `env` set *for that process* (an `ssh … fletch-guest exec --cwd
    /// <cwd> -- <program>` line, say). The caller appends `program`'s own
    /// arguments as it would to any `Command`; it must not call `.env()`,
    /// which would reach only the ssh client — that is why `env` is an input.
    fn command(&self, program: &str, cwd: &Path, env: &[(String, String)])
        -> std::process::Command;

    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Write `bytes` to `path`, creating missing parent directories: the host
    /// does not prepare a remote tree the way it `create_dir_all`s its own.
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;

    /// Metadata of what `path` resolves to (symlinks followed), `None` when
    /// nothing is there.
    fn stat(&self, path: &Path) -> io::Result<Option<Stat>>;

    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>>;
}

/// What [`Executor::stat`] reports about a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub is_dir: bool,
    pub is_file: bool,
    pub len: u64,
}

/// One entry of [`Executor::list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub len: u64,
}

/// Where work rooted at a path runs.
#[derive(Debug, Clone)]
pub enum Executor {
    Local,
    Remote(Arc<dyn RemoteExecutor>),
}

/// Roots a remote executor owns, each with the executor. A box registers its
/// writable root when it boots and removes it when it stops; a handful of
/// entries at most, so a linear scan.
static ROOTS: RwLock<Vec<(PathBuf, Arc<dyn RemoteExecutor>)>> = RwLock::new(Vec::new());

/// Where work rooted at `dir` runs: [`Executor::Local`] unless `dir` is under a
/// root some remote executor [registered](register_root), in which case that
/// executor — the longest registered prefix wins. The one question every git
/// invocation ([`crate::git_dist`]) and checkout file access asks.
pub fn for_path(dir: &Path) -> Executor {
    ROOTS
        .read()
        .iter()
        .filter(|(root, _)| dir.starts_with(root))
        .max_by_key(|(root, _)| root.as_os_str().len())
        .map_or(Executor::Local, |(_, remote)| {
            Executor::Remote(remote.clone())
        })
}

/// Route everything under `root` to `remote` until [`unregister_root`]. Called
/// by the box engine when a box is running; re-registering a root replaces its
/// executor.
pub fn register_root(root: PathBuf, remote: Arc<dyn RemoteExecutor>) {
    let mut roots = ROOTS.write();
    roots.retain(|(r, _)| *r != root);
    roots.push((root, remote));
}

/// Stop routing `root` remotely; a no-op for a root never registered.
pub fn unregister_root(root: &Path) {
    ROOTS.write().retain(|(r, _)| r != root);
}

impl Executor {
    /// A command that runs `program` in `cwd` with `env` set for it. Locally
    /// the plain `Command` a caller would build; remotely the executor's
    /// wrapper. For git use [`crate::git_dist`], which adds the resolved
    /// binary and hardening on top; this is for other programs run in a
    /// checkout (a future search or shell).
    pub fn command(
        &self,
        program: &str,
        cwd: &Path,
        env: &[(String, String)],
    ) -> std::process::Command {
        match self {
            Self::Local => {
                let mut cmd = std::process::Command::new(program);
                cmd.current_dir(cwd).envs(env.iter().map(|(k, v)| (k, v)));
                cmd
            }
            Self::Remote(remote) => remote.command(program, cwd, env),
        }
    }

    /// `std::fs::read`, where the file lives.
    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        match self {
            Self::Local => std::fs::read(path),
            Self::Remote(remote) => remote.read(path),
        }
    }

    /// `std::fs::write`, where the file lives. Locally the parent must exist,
    /// as for `std::fs::write`; see [`RemoteExecutor::write`] for the remote.
    pub fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Local => std::fs::write(path, bytes),
            Self::Remote(remote) => remote.write(path, bytes),
        }
    }

    /// `std::fs::metadata` (symlinks followed), `Ok(None)` when nothing is at
    /// `path`.
    pub fn stat(&self, path: &Path) -> io::Result<Option<Stat>> {
        match self {
            Self::Local => match std::fs::metadata(path) {
                Ok(meta) => Ok(Some(Stat {
                    is_dir: meta.is_dir(),
                    is_file: meta.is_file(),
                    len: meta.len(),
                })),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            },
            Self::Remote(remote) => remote.stat(path),
        }
    }

    /// Whether anything is at `path`, as `Path::exists` answers it: an
    /// unreadable path counts as absent locally.
    pub fn exists(&self, path: &Path) -> io::Result<bool> {
        match self {
            Self::Local => Ok(path.exists()),
            Self::Remote(remote) => remote.stat(path).map(|s| s.is_some()),
        }
    }

    /// `std::fs::read_dir`, where the directory lives. For a tree the host
    /// cannot `read_dir` itself (the box engine's file-panel reads).
    pub fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        match self {
            Self::Local => std::fs::read_dir(dir)?
                .map(|entry| {
                    let entry = entry?;
                    let meta = entry.metadata()?;
                    Ok(DirEntry {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        is_dir: meta.is_dir(),
                        len: meta.len(),
                    })
                })
                .collect(),
            Self::Remote(remote) => remote.list(dir),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Fake(&'static str);

    impl RemoteExecutor for Fake {
        fn command(&self, _: &str, _: &Path, _: &[(String, String)]) -> std::process::Command {
            std::process::Command::new(self.0)
        }
        fn read(&self, _: &Path) -> io::Result<Vec<u8>> {
            Ok(self.0.as_bytes().to_vec())
        }
        fn write(&self, _: &Path, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn stat(&self, _: &Path) -> io::Result<Option<Stat>> {
            Ok(None)
        }
        fn list(&self, _: &Path) -> io::Result<Vec<DirEntry>> {
            Ok(Vec::new())
        }
    }

    /// Whether `exec` is the fake registered as `name` (compared by `Debug`
    /// form, the one view a trait object gives).
    fn routed_to(exec: &Executor, name: &'static str) -> bool {
        matches!(exec, Executor::Remote(r) if format!("{r:?}") == format!("{:?}", Fake(name)))
    }

    /// Paths under no registered root are local; under nested roots the
    /// longest prefix wins; an unregistered root is local again.
    #[test]
    fn for_path_routes_by_longest_registered_prefix() {
        // Roots no other test registers, so parallel tests cannot collide.
        let outer = PathBuf::from("/exec-test/outer");
        let inner = outer.join("inner");
        assert!(matches!(for_path(&outer.join("x")), Executor::Local));

        register_root(outer.clone(), Arc::new(Fake("outer")));
        register_root(inner.clone(), Arc::new(Fake("inner")));
        assert!(routed_to(&for_path(&outer.join("repo")), "outer"));
        assert!(routed_to(&for_path(&inner.join("repo")), "inner"));
        assert!(routed_to(&for_path(&inner), "inner"));
        // A sibling that merely shares the prefix string is not under the root.
        assert!(matches!(
            for_path(Path::new("/exec-test/outer-other")),
            Executor::Local
        ));

        unregister_root(&inner);
        assert!(routed_to(&for_path(&inner.join("repo")), "outer"));
        unregister_root(&outer);
        assert!(matches!(for_path(&inner.join("repo")), Executor::Local));
    }

    #[test]
    fn local_stat_distinguishes_missing_from_present() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("f");
        std::fs::write(&file, b"abc").unwrap();
        let exec = Executor::Local;
        assert_eq!(exec.stat(&td.path().join("missing")).unwrap(), None);
        let stat = exec.stat(&file).unwrap().unwrap();
        assert!(stat.is_file && !stat.is_dir);
        assert_eq!(stat.len, 3);
        assert!(exec.stat(td.path()).unwrap().unwrap().is_dir);
        assert_eq!(exec.read(&file).unwrap(), b"abc");
        assert!(exec.exists(&file).unwrap());
        assert!(!exec.exists(&td.path().join("missing")).unwrap());
        let listed = exec.list(td.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "f");
        assert_eq!(listed[0].len, 3);
    }

    /// Files whose git invocations are the seam itself.
    const GIT_SEAMS: &[&str] = &["git_dist.rs", "exec.rs"];

    /// Files that may set a cwd on a process of their own. None of them runs
    /// git in a checkout; each is listed with what it does run.
    const CWD_ALLOWED: &[&str] = &[
        // The seams.
        "git_dist.rs",
        "exec.rs",
        // The agent launcher itself: the cwd is the sandbox engine's program's
        // (`sandbox-exec`, `docker`, and in phase 4 `ssh`), not a host git.
        "managed_session.rs",
        "exec_session.rs",
        // A provider CLI run for a one-shot handoff summary; a box runs it
        // through its engine in phase 4.
        "handoff/run.rs",
        // The project's verify command; the box path is phase 5a.
        "verify.rs",
        // Provider CLIs run in the user's home on the host (sign-out, codex
        // app-server), never in a checkout.
        "commands/accounts.rs",
        "agent/limits/app_server.rs",
        // The codegraph CLI in its host-side mirror of the checkout, never the
        // checkout itself.
        "codegraph/mirror.rs",
    ];

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    fn is_test_file(rel: &str) -> bool {
        rel.contains("/tests/")
            || rel.ends_with("/tests.rs")
            || rel.ends_with("_tests.rs")
            || rel.ends_with("testing.rs")
    }

    /// The source up to the file's test module, where one exists.
    fn production_half(text: &str) -> &str {
        [
            "#[cfg(test)]\nmod ",
            "#[cfg(test)]\npub(crate) mod ",
            "#[cfg(test)]\npub mod ",
        ]
        .iter()
        .filter_map(|marker| text.find(marker))
        .min()
        .map_or(text, |at| &text[..at])
    }

    /// Every git invocation in a checkout goes through `git_dist` (and so
    /// through [`for_path`]); nothing else spawns `git` or sets a cwd on a
    /// process without being accounted for above. A new direct call would run
    /// on the host against a tree a box keeps in its guest.
    #[test]
    fn no_direct_git_on_checkouts() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        let mut offenders = Vec::new();
        for file in files {
            let rel = file
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if is_test_file(&rel) {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap();
            for (i, line) in production_half(&text).lines().enumerate() {
                let spawns_git =
                    line.contains("Command::new(\"git\")") || line.contains("Command::new(git");
                let sets_cwd = line.contains(".current_dir(");
                if (spawns_git && !GIT_SEAMS.contains(&rel.as_str()))
                    || (sets_cwd && !CWD_ALLOWED.contains(&rel.as_str()))
                {
                    offenders.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "git must run through git_dist (exec::for_path) and a cwd must be accounted for in \
             exec::tests::CWD_ALLOWED:\n{}",
            offenders.join("\n")
        );
    }
}

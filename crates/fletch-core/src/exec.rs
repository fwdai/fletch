//! Where work rooted at a path runs: on this host, or inside a sandbox that
//! holds the path's tree.
//!
//! Dozens of call sites resolve an agent's checkout and then run git in it or
//! read and write files under it. Under the box engine that tree exists only
//! inside the guest — at the *same absolute paths* (path identity: a guest
//! path is the host path, nothing is remapped), so
//! nothing here translates a path. Instead every such operation asks
//! [`for_path`] once and gets back an [`Executor`] that does the work locally or
//! through the engine's channel. With no remote root registered, which is every
//! current engine, [`for_path`] is always [`Executor::Local`] and each method is
//! exactly the `std` call it replaced.
//!
//! Fletch's *own* files inside an agent's tree (its `.git/info/exclude`, the
//! `.codegraph` index) are the exception: they go through
//! [`Executor::write_beneath`] and friends, which on this host never follow a
//! link the agent planted, since the host writes with the user's full rights.

use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::RwLock;

/// What the async methods of [`RemoteExecutor`] return. Boxed by hand: the
/// trait is used as `Arc<dyn RemoteExecutor>`, which an `async fn` in it
/// would rule out.
pub type ExecFuture<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

/// A sandbox that owns a tree the host cannot reach directly. Implemented by
/// the box engine over its SSH channel; a trait here so `fletch-core` never
/// depends on the runtime crate.
///
/// Every method takes the host-side absolute path, which by path identity is
/// also the guest's. Each file operation comes twice: sync, for callers
/// reached through sync code, and `…_async`, for a caller on the runtime,
/// which must not hold its thread on a round trip to the sandbox or on the
/// sandbox starting. The async ones default to their sync twin run inline,
/// which suits an in-memory double; an executor whose sync methods block
/// overrides every one.
pub trait RemoteExecutor: Send + Sync + fmt::Debug {
    /// A host-side command that runs `program` in `cwd` inside the remote,
    /// with `env` set *for that process* (an `ssh … fletch-guest exec --cwd
    /// <cwd> -- <program>` line, say). The caller appends `program`'s own
    /// arguments as it would to any `Command`; it must not call `.env()`,
    /// which would reach only the ssh client — that is why `env` is an input.
    fn command(&self, program: &str, cwd: &Path, env: &[(String, String)])
        -> std::process::Command;

    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Write `bytes` to `path` with permission bits `mode`, creating missing
    /// parent directories: the host does not prepare a remote tree the way it
    /// `create_dir_all`s its own.
    fn write(&self, path: &Path, bytes: &[u8], mode: u32) -> io::Result<()>;

    /// Metadata of what `path` resolves to (symlinks followed), `None` when
    /// nothing is there.
    fn stat(&self, path: &Path) -> io::Result<Option<Stat>>;

    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>>;

    /// `mkdir -p`.
    fn create_dir_all(&self, dir: &Path) -> io::Result<()>;

    /// Remove a file, or a directory with everything in it; nothing there is
    /// fine. The file panel's delete.
    fn remove(&self, path: &Path) -> io::Result<()>;

    /// `std::fs::rename`: the file panel's move.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;

    /// [`read`](Self::read) for an async caller.
    fn read_async<'a>(&'a self, path: &'a Path) -> ExecFuture<'a, Vec<u8>> {
        Box::pin(async move { self.read(path) })
    }

    /// [`write`](Self::write) for an async caller.
    fn write_async<'a>(
        &'a self,
        path: &'a Path,
        contents: &'a [u8],
        mode: u32,
    ) -> ExecFuture<'a, ()> {
        Box::pin(async move { self.write(path, contents, mode) })
    }

    /// [`stat`](Self::stat) for an async caller.
    fn stat_async<'a>(&'a self, path: &'a Path) -> ExecFuture<'a, Option<Stat>> {
        Box::pin(async move { self.stat(path) })
    }

    /// [`list`](Self::list) for an async caller.
    fn list_async<'a>(&'a self, dir: &'a Path) -> ExecFuture<'a, Vec<DirEntry>> {
        Box::pin(async move { self.list(dir) })
    }

    /// [`create_dir_all`](Self::create_dir_all) for an async caller.
    fn create_dir_all_async<'a>(&'a self, dir: &'a Path) -> ExecFuture<'a, ()> {
        Box::pin(async move { self.create_dir_all(dir) })
    }

    /// [`remove`](Self::remove) for an async caller.
    fn remove_async<'a>(&'a self, path: &'a Path) -> ExecFuture<'a, ()> {
        Box::pin(async move { self.remove(path) })
    }

    /// [`rename`](Self::rename) for an async caller.
    fn rename_async<'a>(&'a self, from: &'a Path, to: &'a Path) -> ExecFuture<'a, ()> {
        Box::pin(async move { self.rename(from, to) })
    }
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

tokio::task_local! {
    /// Set for the duration of a [`passive`] future.
    static PASSIVE: ();
}

/// Run `fut` as a poll: a stopped remote it reaches fails the operation
/// instead of being woken, so background polling never boots a box. A scope
/// rather than an executor because routing is by path, deep in every git
/// helper. It covers this task only, not work `fut` spawns elsewhere.
pub async fn passive<F: std::future::Future>(fut: F) -> F::Output {
    PASSIVE.scope((), fut).await
}

/// Whether the caller runs inside [`passive`].
pub(crate) fn is_passive() -> bool {
    PASSIVE.try_with(|()| ()).is_ok()
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
            Self::Remote(remote) => remote.write(path, bytes, 0o644),
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

    /// `std::fs::create_dir_all`, where the directory lives.
    pub fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        match self {
            Self::Local => std::fs::create_dir_all(dir),
            Self::Remote(remote) => remote.create_dir_all(dir),
        }
    }

    /// Remove a file or a whole directory; already gone is success.
    pub fn remove(&self, path: &Path) -> io::Result<()> {
        match self {
            Self::Local => {
                let removed = if path.is_dir() {
                    std::fs::remove_dir_all(path)
                } else {
                    std::fs::remove_file(path)
                };
                match removed {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                    other => other,
                }
            }
            Self::Remote(remote) => remote.remove(path),
        }
    }

    /// `std::fs::rename`, where both paths live.
    pub fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        match self {
            Self::Local => std::fs::rename(from, to),
            Self::Remote(remote) => remote.rename(from, to),
        }
    }

    // The async twins of the file operations above, for a caller on the
    // runtime. Remotely the executor's async half, so a sandbox's round trip
    // (or its start) holds no thread. Locally the same `std` call on a
    // blocking thread ([`off_runtime`]): a local file operation can stall
    // too (a large read, a tree's removal, a slow volume), and the async
    // caller must not stall the runtime on it either.

    /// [`read`](Self::read) for an async caller.
    pub async fn read_async(&self, path: &Path) -> io::Result<Vec<u8>> {
        match self {
            Self::Local => {
                let path = path.to_path_buf();
                off_runtime(move || Self::Local.read(&path)).await
            }
            Self::Remote(remote) => remote.read_async(path).await,
        }
    }

    /// [`write`](Self::write) for an async caller.
    pub async fn write_async(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Local => {
                let (path, bytes) = (path.to_path_buf(), bytes.to_vec());
                off_runtime(move || Self::Local.write(&path, &bytes)).await
            }
            Self::Remote(remote) => remote.write_async(path, bytes, 0o644).await,
        }
    }

    /// [`stat`](Self::stat) for an async caller.
    pub async fn stat_async(&self, path: &Path) -> io::Result<Option<Stat>> {
        match self {
            Self::Local => {
                let path = path.to_path_buf();
                off_runtime(move || Self::Local.stat(&path)).await
            }
            Self::Remote(remote) => remote.stat_async(path).await,
        }
    }

    /// [`exists`](Self::exists) for an async caller.
    pub async fn exists_async(&self, path: &Path) -> io::Result<bool> {
        match self {
            Self::Local => {
                let path = path.to_path_buf();
                off_runtime(move || Self::Local.exists(&path)).await
            }
            Self::Remote(remote) => remote.stat_async(path).await.map(|s| s.is_some()),
        }
    }

    /// [`list`](Self::list) for an async caller.
    pub async fn list_async(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        match self {
            Self::Local => {
                let dir = dir.to_path_buf();
                off_runtime(move || Self::Local.list(&dir)).await
            }
            Self::Remote(remote) => remote.list_async(dir).await,
        }
    }

    /// [`create_dir_all`](Self::create_dir_all) for an async caller.
    pub async fn create_dir_all_async(&self, dir: &Path) -> io::Result<()> {
        match self {
            Self::Local => {
                let dir = dir.to_path_buf();
                off_runtime(move || Self::Local.create_dir_all(&dir)).await
            }
            Self::Remote(remote) => remote.create_dir_all_async(dir).await,
        }
    }

    /// [`remove`](Self::remove) for an async caller.
    pub async fn remove_async(&self, path: &Path) -> io::Result<()> {
        match self {
            Self::Local => {
                let path = path.to_path_buf();
                off_runtime(move || Self::Local.remove(&path)).await
            }
            Self::Remote(remote) => remote.remove_async(path).await,
        }
    }

    /// [`rename`](Self::rename) for an async caller.
    pub async fn rename_async(&self, from: &Path, to: &Path) -> io::Result<()> {
        match self {
            Self::Local => {
                let (from, to) = (from.to_path_buf(), to.to_path_buf());
                off_runtime(move || Self::Local.rename(&from, &to)).await
            }
            Self::Remote(remote) => remote.rename_async(from, to).await,
        }
    }

    /// Write one of Fletch's own files into a tree an agent can also write
    /// (`.git/info/exclude`, the `.codegraph` index): `bytes` at `rel` beneath
    /// `anchor`, missing directories created. The one way host code puts such
    /// a file there, because where it lands must not be the agent's to choose.
    ///
    /// Locally the agent shares the host's filesystem, so a link it planted
    /// anywhere under `anchor` would carry a plain write out of its tree:
    /// every component of `rel` is opened by handle without following a link
    /// (a link is refused), and the file is replaced in one rename, so a link
    /// at the name itself is replaced rather than written through
    /// (`agent::credential_file::PrivateDir`). `anchor` must not be a link and
    /// what is above it must be the host's own, like the checkout root. In a
    /// box the tree is the guest's own, where a link leads nowhere else, so
    /// the remote write is the plain one.
    pub fn write_beneath(
        &self,
        anchor: &Path,
        rel: &Path,
        bytes: &[u8],
        mode: u32,
    ) -> io::Result<()> {
        match self {
            Self::Local => {
                let (dir, name) = local_parent(anchor, rel, true)?;
                dir.write_file(name, bytes, mode)
            }
            Self::Remote(remote) => remote.write(&anchor.join(rel), bytes, mode),
        }
    }

    /// [`write_beneath`](Self::write_beneath) with the bytes of the host file
    /// `from`. Locally a copy-on-write clone where the volume allows.
    pub fn copy_beneath(
        &self,
        from: &Path,
        anchor: &Path,
        rel: &Path,
        mode: u32,
    ) -> io::Result<()> {
        match self {
            Self::Local => {
                let (dir, name) = local_parent(anchor, rel, true)?;
                dir.copy_file(name, from, mode)
            }
            Self::Remote(remote) => remote.write(&anchor.join(rel), &std::fs::read(from)?, mode),
        }
    }

    /// Read back one of Fletch's own files written with
    /// [`write_beneath`](Self::write_beneath), refusing (locally) a link
    /// anywhere under `anchor`, the file itself included.
    pub fn read_beneath(&self, anchor: &Path, rel: &Path) -> io::Result<Vec<u8>> {
        match self {
            Self::Local => {
                let (dir, name) = local_parent(anchor, rel, false)?;
                dir.read_file(name)
            }
            Self::Remote(remote) => remote.read(&anchor.join(rel)),
        }
    }
}

/// `op` on the runtime's blocking pool: the local half of [`Executor`]'s async
/// methods. A task that cannot be joined (it panicked) is an error.
async fn off_runtime<T: Send + 'static>(
    op: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(op)
        .await
        .unwrap_or_else(|e| Err(io::Error::other(e)))
}

/// The directory holding `rel` beneath `anchor`, opened by handle without
/// following a link (created when `create`), and `rel`'s file name.
fn local_parent<'a>(
    anchor: &Path,
    rel: &'a Path,
    create: bool,
) -> io::Result<(crate::agent::credential_file::PrivateDir, &'a str)> {
    let name = rel
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let root = crate::agent::credential_file::PrivateDir::open(anchor)?;
    let dir = match rel.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(parent) => root.walk(parent, create)?,
        None => root,
    };
    Ok((dir, name))
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
        fn write(&self, _: &Path, _: &[u8], _: u32) -> io::Result<()> {
            Ok(())
        }
        fn stat(&self, _: &Path) -> io::Result<Option<Stat>> {
            Ok(None)
        }
        fn list(&self, _: &Path) -> io::Result<Vec<DirEntry>> {
            Ok(Vec::new())
        }
        fn create_dir_all(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn remove(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn rename(&self, _: &Path, _: &Path) -> io::Result<()> {
            Ok(())
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

    /// Each async method answers as its sync twin: locally, and through a
    /// remote's async half (here the trait's defaults).
    #[tokio::test]
    async fn async_methods_agree_with_the_sync_ones() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("f");
        let missing = td.path().join("missing");
        let local = Executor::Local;
        local.write_async(&file, b"abc").await.unwrap();
        assert_eq!(local.read(&file).unwrap(), b"abc");
        let read = local.read_async(&file).await.unwrap();
        assert_eq!(read, local.read(&file).unwrap());
        let stat = local.stat_async(&file).await.unwrap();
        assert_eq!(stat, local.stat(&file).unwrap());
        assert_eq!(local.stat_async(&missing).await.unwrap(), None);
        assert!(local.exists_async(&file).await.unwrap());
        assert!(!local.exists_async(&missing).await.unwrap());
        let listed = local.list_async(td.path()).await.unwrap();
        assert_eq!(listed, local.list(td.path()).unwrap());
        assert!(local.list_async(&missing).await.is_err());

        let nested = td.path().join("a").join("b");
        local.create_dir_all_async(&nested).await.unwrap();
        assert!(local.stat(&nested).unwrap().is_some_and(|s| s.is_dir));
        let moved = td.path().join("g");
        local.rename_async(&file, &moved).await.unwrap();
        assert!(!local.exists(&file).unwrap());
        assert_eq!(local.read(&moved).unwrap(), b"abc");
        local.remove_async(&td.path().join("a")).await.unwrap();
        assert!(!local.exists(&nested).unwrap());
        // Already gone is success, as for the sync remove.
        local.remove_async(&td.path().join("a")).await.unwrap();

        let remote = Executor::Remote(Arc::new(Fake("remote")));
        let path = Path::new("/exec-test/async/file");
        let read = remote.read_async(path).await.unwrap();
        assert_eq!(read, remote.read(path).unwrap());
        let stat = remote.stat_async(path).await.unwrap();
        assert_eq!(stat, remote.stat(path).unwrap());
        let exists = remote.exists_async(path).await.unwrap();
        assert_eq!(exists, remote.exists(path).unwrap());
        let listed = remote.list_async(path).await.unwrap();
        assert_eq!(listed, remote.list(path).unwrap());
        remote.write_async(path, b"x").await.unwrap();
        remote.create_dir_all_async(path).await.unwrap();
        remote.rename_async(path, path).await.unwrap();
        remote.remove_async(path).await.unwrap();
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
        // (`sandbox-exec`, `docker`, or `ssh` for a box), not a host git.
        "managed_session.rs",
        "exec_session.rs",
        // A provider CLI run for a one-shot handoff summary; a box would run
        // it through its engine.
        "handoff/run.rs",
        // The project's verify command; boxes do not run it yet.
        "verify.rs",
        // Provider CLIs run in the user's home on the host (sign-out, codex
        // app-server), never in a checkout.
        "commands/accounts.rs",
        "agent/limits/app_server.rs",
        // The codegraph CLI in its host-side mirror of the checkout, never the
        // checkout itself.
        "codegraph/mirror.rs",
        // The box executor's ssh client: its cwd is `/` on the host, the
        // guest's cwd travels in the remote command.
        "sandbox/boxes/executor.rs",
    ];

    /// Every git invocation in a checkout goes through `git_dist` (and so
    /// through [`for_path`]); nothing else spawns `git` or sets a cwd on a
    /// process without being accounted for above. A new direct call would run
    /// on the host against a tree a box keeps in its guest.
    #[test]
    fn no_direct_git_on_checkouts() {
        let mut offenders = Vec::new();
        for (rel, text) in crate::source_scan::production_sources() {
            for (i, line) in text.lines().enumerate() {
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

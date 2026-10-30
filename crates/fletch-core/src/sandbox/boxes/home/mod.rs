//! A provider's home in the guest: the state its CLI keeps outside the
//! checkout — first-run state, shared config, the login, its session
//! transcripts — and what the box engine does about each. The engine itself
//! is provider-agnostic; what differs between CLIs sits behind
//! [`ProviderHome`], one adapter per provider ([`claude`], [`codex`]), and this
//! module is the shared mechanism over the adapters:
//!
//! - **Seeding** ([`seed`]): what the guest home must hold before the CLI
//!   runs, applied before every launch so a change to the user's own config
//!   reaches the next launch, as seatbelt's links into the shared config do.
//!   Entries are declared ([`GuestEntry`]) and applied in as few round trips
//!   as they allow: one shell pass for removals and directories, one tar
//!   stream for the files, one probe per file that must not clobber what the
//!   CLI wrote.
//! - **The credential** ([`Credential`]): resolved on the host for the launch,
//!   never the stored login. Either env, which travels in the launch's env
//!   file, or a host file the host rewrites before every launch and turn
//!   (`agent::codex_home`), which [`ship_credential`] copies to the same guest
//!   path each time — the engine's `before_turn` hook on the launch plan.
//! - **Transcripts**: the guest dir the CLI writes its sessions to
//!   ([`ProviderHome::sessions_dir`]) is tailed by the relay into a mirror
//!   under the box dir ([`mirror_for_root`]), where the host reads them. A
//!   session the host writes itself (a fork, a rewind) is created in the
//!   guest, where the CLI resumes it, and in the mirror, where the next
//!   launch's locator finds it at once ([`create_session_file`]).
//!
//! Adding a provider is one adapter plus its tools-disk script
//! (`fletch_box::tools`); nothing in the engine names a provider.

pub mod claude;
pub mod codex;
pub mod cursor;

use std::io;
use std::path::{Path, PathBuf};

use fletch_box::Channel;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::sandbox::engine::AgentLaunchCtx;

use super::executor;

/// What the box engine needs to know about one provider's state outside the
/// checkout. `home` is the user's home, which by path identity is the guest's
/// (`HOME` there, `guest_env`); `writable_root` is the launching agent's own
/// root, under which per-agent state lives — a tenant's root is inside its
/// host's guest, so the same answers hold for a tenant.
pub trait ProviderHome: Send + Sync {
    /// The provider id (`AgentRecord.provider`).
    fn id(&self) -> &'static str;

    /// The guest directory the CLI keeps its session transcripts in: the
    /// relay tails every `*.jsonl` under it, recursively.
    fn sessions_dir(&self, home: &Path, writable_root: &Path) -> PathBuf;

    /// The directory under the box dir the host mirrors
    /// [`sessions_dir`](Self::sessions_dir) into.
    fn mirror_dirname(&self) -> &'static str;

    /// What the guest home must hold before the CLI runs. Applied by
    /// [`seed`] before every launch; must be idempotent.
    fn seed(&self, home: &Path, writable_root: &Path) -> Result<Vec<GuestEntry>>;

    /// The launch's credential, resolved on the host. An error refuses the
    /// launch: a CLI that could only fail on its first turn fails here, with
    /// what to do about it.
    fn credential(&self, ctx: &AgentLaunchCtx) -> Result<Credential>;
}

/// One thing the guest home holds after seeding. Removals and directories
/// are applied first, in order, then the files, so a stale copy of a host
/// item is cleared before its fresh contents land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestEntry {
    /// `mkdir -p`.
    Dir(PathBuf),
    /// A file with these contents and mode; with `keep_existing`, only when
    /// nothing is there yet (state the CLI goes on to change).
    File {
        path: PathBuf,
        bytes: Vec<u8>,
        mode: u32,
        keep_existing: bool,
    },
    /// Nothing at this path: whatever is there is removed (a copy of a host
    /// item that is gone, or that is replaced whole by the files that follow).
    Absent(PathBuf),
}

/// How a launch authenticates, as the host resolved it. Empty when the CLI
/// finds its login some other way, or has none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Credential {
    /// Variables for the launch's env file, good for the whole launch.
    pub env: Vec<(String, String)>,
    /// A host file the host rewrites fresh before every launch and turn;
    /// shipped to the same guest path each time, and removed there when the
    /// host has none to ship.
    pub file: Option<PathBuf>,
}

/// The adapters, one per provider boxes run.
const ALL: &[&dyn ProviderHome] = &[&claude::Claude, &codex::Codex, &cursor::Cursor];

/// `name` as the login shell exports it (over the app's own env,
/// `bin_resolve::effective_env_var`), trimmed, when non-blank: how a
/// credential a user keeps in their shell reaches a box launch, since the
/// guest inherits none of the host's env.
pub(crate) fn login_shell_var(name: &str) -> Option<String> {
    let value = crate::bin_resolve::effective_env_var(name)?;
    let value = value.to_string_lossy().trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// The adapter of `provider`, when boxes run it.
pub fn for_provider(provider: &str) -> Option<&'static dyn ProviderHome> {
    ALL.iter().copied().find(|h| h.id() == provider)
}

/// Whether boxes run `provider`: it has an adapter here.
pub fn supported(provider: &str) -> bool {
    for_provider(provider).is_some()
}

/// The adapter of `provider`, or the error a launch reports.
pub fn require(provider: &str) -> Result<&'static dyn ProviderHome> {
    for_provider(provider)
        .ok_or_else(|| Error::Other(format!("the box engine does not run `{provider}`")))
}

/// Under the box dir of the agent whose writable root is `writable_root`,
/// the host's mirror of the provider's sessions dir.
pub(crate) fn mirror_for_root(home: &dyn ProviderHome, writable_root: &Path) -> Option<PathBuf> {
    super::box_dir_for_root(writable_root).map(|dir| dir.join(home.mirror_dirname()))
}

/// The mirror of `provider`'s sessions for agent `agent_id`'s box. `None`
/// when boxes do not run the provider or the host has no boxes root; a path
/// whether or not the agent has a box.
pub(crate) fn mirror_for_agent(provider: &str, agent_id: &str) -> Option<PathBuf> {
    let home = for_provider(provider)?;
    super::box_dir(agent_id)
        .ok()
        .map(|dir| dir.join(home.mirror_dirname()))
}

/// Apply `home`'s seed to the guest reached over `channel`, for an agent
/// with this home and writable root. Synchronous; the caller is on a thread
/// that may block.
pub(crate) fn seed(
    channel: &Channel,
    home: &dyn ProviderHome,
    home_dir: &Path,
    writable_root: &Path,
) -> Result<()> {
    apply(channel, &home.seed(home_dir, writable_root)?)
}

/// Apply `entries`: removals and directories in one shell pass, the files
/// as one tar stream, each `keep_existing` file behind its own probe.
fn apply(channel: &Channel, entries: &[GuestEntry]) -> Result<()> {
    let mut script = String::from("umask 077");
    let mut tar: Vec<&GuestEntry> = Vec::new();
    let mut guarded: Vec<(&Path, &[u8], u32)> = Vec::new();
    for entry in entries {
        match entry {
            GuestEntry::Dir(path) => {
                script.push_str(" && mkdir -p -- ");
                script.push_str(&fletch_box::channel::shell_quote(&path.to_string_lossy()));
            }
            GuestEntry::Absent(path) => {
                script.push_str(" && rm -rf -- ");
                script.push_str(&fletch_box::channel::shell_quote(&path.to_string_lossy()));
            }
            GuestEntry::File {
                path,
                bytes,
                mode,
                keep_existing: true,
            } => guarded.push((path, bytes, *mode)),
            GuestEntry::File { .. } => tar.push(entry),
        }
    }
    if script != "umask 077" {
        let out = executor::run(channel, &["sh", "-c", &script], None)
            .map_err(|e| Error::Other(format!("seeding the guest home: {e}")))?;
        if !out.status.success() {
            return Err(Error::Other(format!(
                "seeding the guest home failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    if !tar.is_empty() {
        let archive = tar_of(&tar)?;
        let out = executor::run(
            channel,
            &["sh", "-c", "umask 077 && tar -xpf - -C /"],
            Some(&archive),
        )
        .map_err(|e| Error::Other(format!("seeding the guest home: {e}")))?;
        if !out.status.success() {
            return Err(Error::Other(format!(
                "unpacking the guest home seed failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    for (path, bytes, mode) in guarded {
        let exists = executor::run(channel, &["test", "-e", &path.to_string_lossy()], None)
            .map_err(|e| Error::Other(format!("probing {}: {e}", path.display())))?
            .status
            .success();
        if !exists {
            executor::put(channel, bytes, path, mode)?;
        }
    }
    Ok(())
}

/// A tar archive of the `File` entries among `entries`, rooted at `/`, for
/// `tar -xpf - -C /` in the guest: one round trip for a whole config tree.
fn tar_of(entries: &[&GuestEntry]) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    let mtime = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for entry in entries {
        let GuestEntry::File {
            path, bytes, mode, ..
        } = entry
        else {
            continue;
        };
        let rel = path
            .strip_prefix("/")
            .map_err(|_| Error::InvalidPath(path.display().to_string()))?;
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(*mode);
        header.set_mtime(mtime);
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, rel, bytes.as_slice())
            .map_err(|e| Error::Other(format!("tar {}: {e}", path.display())))?;
    }
    builder
        .into_inner()
        .map_err(|e| Error::Other(format!("tar: {e}")))
}

/// How deep a host tree is copied: deep enough for any config layout, and a
/// bound on a symlink loop, which the walk follows rather than resolves.
const COPY_DEPTH: usize = 16;

/// Entries that make `guest` a copy of the host's `host`: nothing when the
/// host has nothing there, one file for a file, the tree for a directory
/// (symlinks followed, as a link into a config dir means its target). Files
/// land mode 0644, in private directories (the seed's umask). Unreadable
/// items are skipped: a config file the host cannot read would not reach a
/// seatbelt launch either.
pub(crate) fn copy_tree(host: &Path, guest: &Path) -> Result<Vec<GuestEntry>> {
    let mut entries = Vec::new();
    copy_into(host, guest, 0, &mut entries)?;
    Ok(entries)
}

fn copy_into(host: &Path, guest: &Path, depth: usize, into: &mut Vec<GuestEntry>) -> Result<()> {
    let meta = match std::fs::metadata(host) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Ok(()),
        Err(e) => return Err(Error::Io(e)),
    };
    if meta.is_file() {
        if let Ok(bytes) = std::fs::read(host) {
            into.push(GuestEntry::File {
                path: guest.to_path_buf(),
                bytes,
                mode: 0o644,
                keep_existing: false,
            });
        }
        return Ok(());
    }
    if !meta.is_dir() || depth >= COPY_DEPTH {
        return Ok(());
    }
    into.push(GuestEntry::Dir(guest.to_path_buf()));
    let Ok(read) = std::fs::read_dir(host) else {
        return Ok(());
    };
    let mut names: Vec<_> = read.filter_map(|e| e.ok().map(|e| e.file_name())).collect();
    names.sort();
    for name in names {
        copy_into(&host.join(&name), &guest.join(&name), depth + 1, into)?;
    }
    Ok(())
}

/// Ship the credential file at host `path` into the guest at the same path,
/// 0600; remove the guest's copy when the host has none (the login was
/// revoked or signed out since).
pub(crate) fn ship_credential(channel: &Channel, path: &Path) -> Result<()> {
    match std::fs::read(path) {
        Ok(bytes) => executor::put(channel, &bytes, path, 0o600),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let out = executor::run(channel, &["rm", "-f", "--", &path.to_string_lossy()], None)
                .map_err(|e| {
                    Error::Other(format!("removing {} in the box: {e}", path.display()))
                })?;
            if !out.status.success() {
                return Err(Error::Other(format!(
                    "removing {} in the box failed: {}",
                    path.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            Ok(())
        }
        Err(e) => Err(Error::Io(e)),
    }
}

/// Create the session file `rel` (relative to `home`'s sessions dir) with
/// `lines`, for an agent working in `tree` — a session the host wrote: a
/// fork, a rewind. In the guest, where the CLI that resumes it reads its own
/// sessions dir; and in the host's mirror of that dir, which the relay fills
/// guest → host only, so the launch that follows sees a resumable session at
/// once rather than after the tail relays it back. Fails when something is
/// already there on either side. Returns the mirror path, the one the host's
/// locators read.
pub(crate) fn create_session_file(
    home: &dyn ProviderHome,
    tree: &Path,
    rel: &Path,
    lines: &[Value],
) -> Result<PathBuf> {
    let identity = super::box_holding(tree)
        .ok_or_else(|| Error::Other(format!("{} is in no box", tree.display())))?;
    let root = tree
        .parent()
        .ok_or_else(|| Error::InvalidPath(tree.display().to_string()))?;
    let mirror = mirror_for_root(home, root)
        .ok_or_else(|| Error::Other("the box's mirror directory can't be resolved".into()))?
        .join(rel);
    let guest_path = home.sessions_dir(&identity.home, root).join(rel);
    let mut bytes = Vec::new();
    for line in lines {
        serde_json::to_writer(&mut bytes, line)?;
        bytes.push(b'\n');
    }
    super::write_new_guest_file(tree, &guest_path, &bytes)?;
    crate::agent::write_new_jsonl(&mirror, lines)?;
    Ok(mirror)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_and_codex_have_adapters_and_nothing_else_does() {
        for supported in ["claude", "codex", "cursor"] {
            assert_eq!(for_provider(supported).map(|h| h.id()), Some(supported));
        }
        for other in ["opencode", "pi", "antigravity", ""] {
            assert!(!supported(other), "{other}");
            assert!(require(other).is_err(), "{other}");
        }
    }

    /// Every adapter's mirror has a name of its own, so two providers'
    /// sessions never land in one dir of a box.
    #[test]
    fn mirror_names_are_distinct() {
        let mut names: Vec<&str> = ALL.iter().map(|h| h.mirror_dirname()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }

    #[test]
    fn mirror_for_root_is_under_the_box_dir_of_the_roots_agent() {
        let root = Path::new("/w/workspaces/agent-7");
        let mirror = mirror_for_root(&codex::Codex, root).unwrap();
        assert!(mirror.ends_with("agent-7/codex-sessions"), "{mirror:?}");
        assert_eq!(
            mirror,
            mirror_for_agent("codex", "agent-7").unwrap(),
            "the two spellings of one agent's mirror agree"
        );
        assert_eq!(mirror_for_agent("cursor", "agent-7"), None);
    }

    /// A host tree is copied file by file under the guest path, directories
    /// first so empty ones exist, links followed; nothing when the host has
    /// nothing there.
    #[test]
    fn copy_tree_mirrors_a_host_tree_and_follows_links() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("skills");
        std::fs::create_dir_all(src.join("review")).unwrap();
        std::fs::write(src.join("review/SKILL.md"), b"# review").unwrap();
        std::fs::create_dir_all(src.join("empty")).unwrap();
        let elsewhere = td.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("linked.md"), b"# linked").unwrap();
        std::os::unix::fs::symlink(&elsewhere, src.join("linked")).unwrap();

        let guest = Path::new("/g/.fletch-codex-home/skills");
        let entries = copy_tree(&src, guest).unwrap();
        assert_eq!(
            entries,
            vec![
                GuestEntry::Dir(guest.to_path_buf()),
                GuestEntry::Dir(guest.join("empty")),
                GuestEntry::Dir(guest.join("linked")),
                GuestEntry::File {
                    path: guest.join("linked/linked.md"),
                    bytes: b"# linked".to_vec(),
                    mode: 0o644,
                    keep_existing: false,
                },
                GuestEntry::Dir(guest.join("review")),
                GuestEntry::File {
                    path: guest.join("review/SKILL.md"),
                    bytes: b"# review".to_vec(),
                    mode: 0o644,
                    keep_existing: false,
                },
            ]
        );

        let file = td.path().join("config.toml");
        std::fs::write(&file, b"model = \"x\"").unwrap();
        let entries = copy_tree(&file, Path::new("/g/config.toml")).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(
            matches!(&entries[0], GuestEntry::File { path, .. } if path == Path::new("/g/config.toml"))
        );

        assert!(copy_tree(&td.path().join("missing"), guest)
            .unwrap()
            .is_empty());
    }

    /// A symlink loop ends at the depth bound instead of never.
    #[test]
    fn copy_tree_is_bounded_on_a_link_loop() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("loop");
        std::fs::create_dir_all(&src).unwrap();
        std::os::unix::fs::symlink(&src, src.join("again")).unwrap();
        let entries = copy_tree(&src, Path::new("/g/loop")).unwrap();
        assert_eq!(entries.len(), COPY_DEPTH);
        assert!(entries.iter().all(|e| matches!(e, GuestEntry::Dir(_))));
    }

    /// The archive holds exactly the files, at their guest paths relative to
    /// `/`, with the modes the entries say.
    #[test]
    fn the_seed_archive_carries_the_files_at_their_guest_paths() {
        let entries = [
            GuestEntry::File {
                path: PathBuf::from("/Users/u/.fletch-codex-home/config.toml"),
                bytes: b"a = 1\n".to_vec(),
                mode: 0o644,
                keep_existing: false,
            },
            GuestEntry::File {
                path: PathBuf::from("/Users/u/.fletch-codex-home/prompts/p.md"),
                bytes: b"hi".to_vec(),
                mode: 0o600,
                keep_existing: false,
            },
        ];
        let refs: Vec<&GuestEntry> = entries.iter().collect();
        let archive = tar_of(&refs).unwrap();
        let mut read = tar::Archive::new(archive.as_slice());
        let mut seen = Vec::new();
        for entry in read.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_path_buf();
            let mode = entry.header().mode().unwrap();
            let mut body = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut body).unwrap();
            seen.push((path, mode, body));
        }
        assert_eq!(
            seen,
            vec![
                (
                    PathBuf::from("Users/u/.fletch-codex-home/config.toml"),
                    0o644,
                    b"a = 1\n".to_vec()
                ),
                (
                    PathBuf::from("Users/u/.fletch-codex-home/prompts/p.md"),
                    0o600,
                    b"hi".to_vec()
                ),
            ]
        );
    }

    #[test]
    fn a_relative_guest_path_is_refused() {
        let entry = GuestEntry::File {
            path: PathBuf::from("relative/auth.json"),
            bytes: Vec::new(),
            mode: 0o600,
            keep_existing: false,
        };
        assert!(tar_of(&[&entry]).is_err());
    }
}

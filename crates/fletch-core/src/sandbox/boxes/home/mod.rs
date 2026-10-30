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
//!   never the stored login. Env, which travels in the launch's env file,
//!   and/or host files the host keeps current (`agent::codex_home` rewrites
//!   codex's before every launch and turn), which [`ship_credential`] copies
//!   into the guest with the env file and again before every turn — the
//!   engine's `before_turn` hook on the launch plan.
//! - **Transcripts**: the guest dir the CLI writes its sessions to
//!   ([`ProviderHome::sessions_dir`]) reaches a mirror under the box dir
//!   ([`mirror_for_root`]), where the host reads them, one of two ways
//!   ([`Transport`]): tailed by the relay as it grows (append-only JSONL),
//!   or pulled whole by the host when it reads and before the box stops
//!   (a blob store the CLI rewrites in place). A session the host writes
//!   itself (a fork, a rewind) is created in the guest, where the CLI resumes
//!   it, and in the mirror, where the next launch's locator finds it at once
//!   ([`create_session_file`]).
//!
//! Adding a provider is one adapter plus its tools-disk script
//! (`fletch_box::tools`); nothing in the engine names a provider.

pub mod claude;
pub mod codex;
pub mod cursor;
pub mod opencode;
pub mod pi;

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

    /// How the sessions dir reaches its mirror. Tailed unless the CLI
    /// rewrites its files in place.
    fn transport(&self) -> Transport {
        Transport::Tail
    }

    /// What the guest home must hold before the CLI runs. Applied by
    /// [`seed`] before every launch; must be idempotent.
    fn seed(&self, home: &Path, writable_root: &Path) -> Result<Vec<GuestEntry>>;

    /// The launch's credential, resolved on the host. An error refuses the
    /// launch: a CLI that could only fail on its first turn fails here, with
    /// what to do about it.
    fn credential(&self, ctx: &AgentLaunchCtx) -> Result<Credential>;
}

/// How a provider's sessions dir reaches the host's mirror of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// The relay follows every `*.jsonl` under it as it grows, from the
    /// moment the box boots: for a CLI that only appends.
    Tail,
    /// The host copies the whole dir over when it reads the transcript
    /// ([`sessions_mirror`]) and before the box stops ([`pull_before_stop`]):
    /// for a CLI that rewrites its files in place, which a tail would
    /// garble.
    Pull,
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
    /// Host files the host keeps current, shipped into the guest with the
    /// env file and again before every turn, and removed there when the
    /// host no longer has one.
    pub files: Vec<ShippedFile>,
}

/// A host file and where it lands in the guest — the same path for
/// per-agent state under the writable root (path identity), a different
/// one for state the user keeps where an XDG variable put it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippedFile {
    pub host: PathBuf,
    pub guest: PathBuf,
}

impl ShippedFile {
    /// A file at the same path on both sides.
    pub fn at(path: PathBuf) -> Self {
        Self {
            host: path.clone(),
            guest: path,
        }
    }
}

/// The adapters, one per provider boxes run.
const ALL: &[&dyn ProviderHome] = &[
    &claude::Claude,
    &codex::Codex,
    &cursor::Cursor,
    &opencode::OpenCode,
    &pi::Pi,
];

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

/// Ship each credential file into the guest, 0600; remove the guest's copy
/// of one the host no longer has (the login was revoked or signed out since).
pub(crate) fn ship_credential(channel: &Channel, files: &[ShippedFile]) -> Result<()> {
    for ShippedFile { host, guest } in files {
        match std::fs::read(host) {
            Ok(bytes) => executor::put(channel, &bytes, guest, 0o600)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let out =
                    executor::run(channel, &["rm", "-f", "--", &guest.to_string_lossy()], None)
                        .map_err(|e| {
                            Error::Other(format!("removing {} in the box: {e}", guest.display()))
                        })?;
                if !out.status.success() {
                    return Err(Error::Other(format!(
                        "removing {} in the box failed: {}",
                        guest.display(),
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
            }
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}

/// Copy the guest's `guest_root` into `mirror` on the host, whole: one tar
/// over the channel, unpacked file by file. Only regular files land —
/// never a link the agent planted, which the host would then follow — and
/// only inside `mirror`. Nothing there is removed: a CLI's session store
/// only grows. A `guest_root` that does not exist yet copies nothing.
pub(crate) fn pull_sessions(channel: &Channel, guest_root: &Path, mirror: &Path) -> Result<()> {
    let root = guest_root.to_string_lossy();
    let out = executor::run(
        channel,
        &[
            "sh",
            "-c",
            "[ -d \"$1\" ] && exec tar -cf - -C \"$1\" . ; exit 0",
            "sh",
            &root,
        ],
        None,
    )
    .map_err(|e| {
        Error::Other(format!(
            "pulling {} from the box: {e}",
            guest_root.display()
        ))
    })?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "pulling {} from the box failed: {}",
            guest_root.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    unpack_files(&out.stdout, mirror)
}

/// Unpack the regular files of a tar stream under `dir`, skipping every
/// other entry and anything that would land outside `dir`.
fn unpack_files(archive: &[u8], dir: &Path) -> Result<()> {
    if archive.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let mut archive = tar::Archive::new(archive);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        // `unpack_in` refuses a path that escapes `dir` (`..`), reporting
        // `false`; it creates the parents it needs.
        entry.unpack_in(dir)?;
    }
    Ok(())
}

/// The mirror of `provider`'s sessions for agent `agent_id`, as current as
/// it can be made: for a provider the host pulls, refreshed from the box
/// first when it is running (a stopped box was pulled before it stopped;
/// it is never booted for a read). `None` when boxes do not run the
/// provider.
pub(crate) fn sessions_mirror(provider: &str, agent_id: &str) -> Option<PathBuf> {
    let home = for_provider(provider)?;
    let mirror = mirror_for_agent(provider, agent_id)?;
    if home.transport() == Transport::Pull {
        if let Some(running) = super::BoxRegistry::global().running(agent_id) {
            let root = home.sessions_dir(&running.identity.home, &running.identity.writable_root);
            if let Err(e) = pull_sessions(&running.channel, &root, &mirror) {
                tracing::warn!(agent_id, provider, error = %e, "pulling the sessions mirror failed");
            }
        }
    }
    Some(mirror)
}

/// Bring a pulled provider's mirror up to date before its box stops, so
/// what the agent wrote since the last read is not lost with the VM's
/// pause; nothing to do for a tailed one. Logged, never fatal: the stop
/// goes on either way.
pub(crate) async fn pull_before_stop(running: &super::RunningBox) {
    let identity = &running.identity;
    let Some(home) = for_provider(&identity.provider) else {
        return;
    };
    if home.transport() != Transport::Pull {
        return;
    }
    let channel = running.channel.clone();
    let root = home.sessions_dir(&identity.home, &identity.writable_root);
    let mirror = running.dir.join(home.mirror_dirname());
    let pulled = tokio::task::spawn_blocking(move || pull_sessions(&channel, &root, &mirror))
        .await
        .map_err(|e| Error::Other(e.to_string()))
        .and_then(|r| r);
    if let Err(e) = pulled {
        tracing::warn!(agent_id = %identity.agent_id, error = %e, "sessions pull before stop failed");
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
        for supported in ["claude", "codex", "cursor", "opencode", "pi"] {
            assert_eq!(for_provider(supported).map(|h| h.id()), Some(supported));
        }
        for other in ["antigravity", "gemini", ""] {
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
        assert_eq!(mirror_for_agent("antigravity", "agent-7"), None);
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

    /// A pulled archive lands its regular files under the mirror and nothing
    /// else: a link (which the host would follow on read) and an entry that
    /// climbs out are dropped; an empty pull is nothing.
    #[test]
    fn unpack_keeps_regular_files_inside_the_mirror_only() {
        let td = tempfile::tempdir().unwrap();
        let mirror = td.path().join("mirror");
        let mut builder = tar::Builder::new(Vec::new());
        let mut file = |path: &str, body: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            builder.append_data(&mut header, path, body).unwrap();
        };
        file("./message/ses_1/msg_1.json", b"{\"id\":\"msg_1\"}");
        // The builder refuses to spell `..` itself, as a guest's `tar` would
        // not: the name is written into the header by hand.
        let mut escaped = tar::Header::new_gnu();
        let name = b"../escaped.json";
        escaped.as_old_mut().name[..name.len()].copy_from_slice(name);
        escaped.set_size(2);
        escaped.set_mode(0o644);
        escaped.set_entry_type(tar::EntryType::Regular);
        escaped.set_cksum();
        builder.append(&escaped, &b"{}"[..]).unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_size(0);
        link.set_mode(0o777);
        link.set_entry_type(tar::EntryType::Symlink);
        builder
            .append_link(&mut link, "./message/ses_1/link.json", "/etc/passwd")
            .unwrap();
        let archive = builder.into_inner().unwrap();

        unpack_files(&archive, &mirror).unwrap();
        assert_eq!(
            std::fs::read(mirror.join("message/ses_1/msg_1.json")).unwrap(),
            b"{\"id\":\"msg_1\"}"
        );
        assert!(!mirror.join("message/ses_1/link.json").exists());
        assert!(!td.path().join("escaped.json").exists());

        unpack_files(&[], &td.path().join("none")).unwrap();
        assert!(!td.path().join("none").exists());
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

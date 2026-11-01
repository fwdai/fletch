//! A provider's home in the guest: what the box engine does with the answers
//! of the provider adapters (`sandbox::provider`, one [`ProviderHome`] per
//! CLI). The engine itself is provider-agnostic; this module is the shared
//! mechanism over the adapters, over the box's channel:
//!
//! - **Seeding** ([`seed`]): what the guest home must hold before the CLI
//!   runs, applied before every launch so a change to the user's own config
//!   reaches the next launch, as seatbelt's links into the shared config do.
//!   Entries are declared ([`GuestEntry`]) and applied in as few round trips
//!   as they allow: one shell pass for removals and directories, one tar
//!   stream for the files, one probe per file that must not clobber what the
//!   CLI wrote.
//! - **The credential** ([`Credential`](crate::sandbox::provider::Credential)):
//!   resolved on the host for the launch, never the stored login. Env, which
//!   travels in the launch's env file, and/or host files the host keeps
//!   current (`agent::codex_home` rewrites codex's before every launch and
//!   turn), which [`CredentialSync`] copies into the guest with the env file
//!   and again before every turn — the engine's `before_turn` hook on the
//!   launch plan.
//! - **Transcripts**: the guest dir the CLI writes its sessions to
//!   ([`ProviderHome::sessions_dir`]) reaches a mirror under the box dir
//!   ([`mirror_for_root`]), where the host reads them, one of two ways
//!   ([`Transport`]): tailed by the relay as it grows (append-only JSONL),
//!   or pulled whole by the host when it reads and before the box stops
//!   (a blob store the CLI rewrites in place). A session the host writes
//!   itself (a fork, a rewind) is created in the guest, where the CLI resumes
//!   it, and in the mirror, where the next launch's locator finds it at once
//!   ([`create_session_file`]).

use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fletch_box::Channel;
use parking_lot::Mutex;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::sandbox::provider::{
    for_provider, CredentialFile, FileOwner, GuestEntry, ProviderHome, Transport,
};

use super::executor;

/// The credential files of one launch, kept in step for as long as it lives:
/// [`launch`](Self::launch) when the env file is delivered,
/// [`turn`](Self::turn) before every turn of a per-turn session,
/// [`after_exit`](Self::after_exit) once each process of the launch has
/// exited. Remembers what each side last held, so a change on either side
/// is told from the other's.
#[derive(Debug, Default)]
pub struct CredentialSync {
    files: Vec<CredentialFile>,
    /// What both sides held after the last sync, by guest path: a digest of
    /// the contents, or `None` for no file.
    synced: std::collections::HashMap<PathBuf, Option<[u8; 32]>>,
}

impl CredentialSync {
    pub fn new(files: Vec<CredentialFile>) -> Self {
        Self {
            files,
            synced: Default::default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// At launch the host is authoritative for every file: what a previous
    /// launch left in the guest is replaced, since the host's copy is what
    /// the user signed in to (or back out of) since.
    pub fn launch(&mut self, channel: &Channel) -> Result<()> {
        for i in 0..self.files.len() {
            self.ship(channel, i)?;
        }
        Ok(())
    }

    /// Before a turn: a host-owned file is shipped afresh (the host rewrote
    /// it for the turn); a CLI-owned one is reconciled.
    pub fn turn(&mut self, channel: &Channel) -> Result<()> {
        for i in 0..self.files.len() {
            match self.files[i].owner {
                FileOwner::Host => self.ship(channel, i)?,
                FileOwner::Cli => self.reconcile(channel, i)?,
            }
        }
        Ok(())
    }

    /// Once a process of the launch has exited — each turn's, or the one
    /// long-lived one: what the CLI changed goes back to the host, now that
    /// nothing is writing it and before anything else could be lost.
    pub fn after_exit(&mut self, channel: &Channel) -> Result<()> {
        for i in 0..self.files.len() {
            if self.files[i].owner == FileOwner::Cli {
                self.reconcile(channel, i)?;
            }
        }
        Ok(())
    }

    /// Host → guest: put the host's copy, or remove the guest's when the host
    /// has none (the login was revoked or signed out).
    fn ship(&mut self, channel: &Channel, i: usize) -> Result<()> {
        let CredentialFile { host, guest, .. } = &self.files[i];
        let bytes = read_optional(host)?;
        put_optional(channel, guest, bytes.as_deref())?;
        self.synced
            .insert(guest.clone(), bytes.as_deref().map(digest));
        Ok(())
    }

    /// Both ways, by what changed since the last sync: the guest's change
    /// goes to the host (the guest wins a race); otherwise the host's change
    /// goes to the guest; otherwise nothing moves.
    fn reconcile(&mut self, channel: &Channel, i: usize) -> Result<()> {
        let CredentialFile { host, guest, .. } = &self.files[i];
        let last = self.synced.get(guest).copied().flatten();
        let in_guest = get_optional(channel, guest)?;
        let on_host = read_optional(host)?;
        let guest_digest = in_guest.as_deref().map(digest);
        let host_digest = on_host.as_deref().map(digest);
        match reconcile_direction(last, guest_digest, host_digest) {
            Move::ToHost => {
                write_host_file(host, in_guest.as_deref().unwrap_or_default())?;
                self.synced.insert(guest.clone(), guest_digest);
            }
            Move::ToGuest => {
                put_optional(channel, guest, on_host.as_deref())?;
                self.synced.insert(guest.clone(), host_digest);
            }
            Move::Nothing => {}
        }
        Ok(())
    }
}

/// Which way a CLI-owned file moves at a reconcile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    ToHost,
    ToGuest,
    Nothing,
}

/// The rule of [`CredentialSync::reconcile`], over digests: what the guest
/// changed goes to the host (the guest wins a race, and a file the guest
/// removed is left to the host's next sign-in rather than removed there);
/// otherwise what the host changed goes to the guest; otherwise nothing.
fn reconcile_direction(
    last: Option<[u8; 32]>,
    guest: Option<[u8; 32]>,
    host: Option<[u8; 32]>,
) -> Move {
    if guest.is_some() && guest != last {
        Move::ToHost
    } else if host != last {
        Move::ToGuest
    } else {
        Move::Nothing
    }
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).into()
}

/// The host file's contents, `None` when there is none.
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(e)),
    }
}

/// The guest file's contents, `None` when there is none.
fn get_optional(channel: &Channel, path: &Path) -> Result<Option<Vec<u8>>> {
    match executor::get(channel, path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Other(format!(
            "reading {} from the box: {e}",
            path.display()
        ))),
    }
}

/// Put `bytes` at the guest path, 0600, or remove it for `None`.
fn put_optional(channel: &Channel, path: &Path, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => executor::put(channel, bytes, path, 0o600),
        None => {
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
    }
}

/// Write a credential the CLI refreshed back to its host file: the user's
/// own login store, which a box agent cannot otherwise reach. Whole and in
/// one rename, 0600, so a reader never sees a half-written login.
fn write_host_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let dir = path
        .parent()
        .ok_or_else(|| Error::InvalidPath(path.display().to_string()))?;
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::Builder::new()
        .prefix(".fletch-credential-")
        .tempfile_in(dir)?;
    file.write_all(bytes)?;
    file.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.persist(path).map_err(|e| Error::Io(e.error))?;
    Ok(())
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

/// How far back a pull reaches past the last one: the host's clock against
/// the guest's, and a file still being written when the last pull looked.
/// A file sent twice only overwrites its own copy.
const PULL_OVERLAP: Duration = Duration::from_secs(30);

/// When each agent's sessions were last pulled, so the next pull takes
/// only what changed since (`tar --newer-mtime`). Per process: a restart
/// pulls whole once.
static LAST_PULL: Mutex<Option<std::collections::HashMap<String, SystemTime>>> = Mutex::new(None);

/// Copy what changed in the guest's `guest_root` since `agent_id`'s last
/// pull into `mirror` on the host — everything, the first time: one tar over
/// the channel, unpacked file by file. Only regular files land — never a
/// link the agent planted, which the host would then follow — and only
/// inside `mirror`. Nothing there is removed: a CLI's session store only
/// grows. A `guest_root` that does not exist yet copies nothing.
pub(crate) fn pull_sessions(
    channel: &Channel,
    agent_id: &str,
    guest_root: &Path,
    mirror: &Path,
) -> Result<()> {
    let started = SystemTime::now();
    let since = LAST_PULL
        .lock()
        .as_ref()
        .and_then(|m| m.get(agent_id).copied())
        .map(|t| t.checked_sub(PULL_OVERLAP).unwrap_or(t));
    let root = guest_root.to_string_lossy();
    let newer = match since.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()) {
        Some(epoch) => format!("--newer-mtime=@{}", epoch.as_secs()),
        None => String::new(),
    };
    let out = executor::run(
        channel,
        &[
            "sh",
            "-c",
            "[ -d \"$1\" ] && exec tar -cf - -C \"$1\" $2 . ; exit 0",
            "sh",
            &root,
            &newer,
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
    unpack_files(&out.stdout, mirror)?;
    LAST_PULL
        .lock()
        .get_or_insert_with(Default::default)
        .insert(agent_id.to_string(), started);
    Ok(())
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

/// Where agent `agent_id`'s sessions are while its box (its own, or its
/// host's when it is a tenant) runs: the channel, the guest sessions dir of
/// the agent's own writable root, and the host mirror. `None` when nothing
/// of the agent's runs now.
fn live_sessions(
    home: &dyn ProviderHome,
    agent_id: &str,
) -> Option<(Arc<Channel>, PathBuf, PathBuf)> {
    let registry = super::BoxRegistry::global();
    let (running, root) = match registry.running(agent_id) {
        Some(running) => {
            let root = running.identity.writable_root.clone();
            (running, root)
        }
        None => {
            let tenant = registry.tenant_of(agent_id)?;
            (
                registry.running(&tenant.host_agent_id)?,
                tenant.writable_root,
            )
        }
    };
    let sessions = home.sessions_dir(&running.identity.home, &root);
    let mirror = mirror_for_agent(home.id(), agent_id)?;
    Some((running.channel.clone(), sessions, mirror))
}

/// The mirror of `provider`'s sessions for agent `agent_id`, as current as
/// it can be made: for a provider the host pulls, refreshed from the box
/// first when the agent runs now (a stopped box was pulled before it
/// stopped; it is never booted for a read). `None` when boxes do not run
/// the provider.
pub(crate) fn sessions_mirror(provider: &str, agent_id: &str) -> Option<PathBuf> {
    let home = for_provider(provider)?;
    if home.transport() == Transport::Pull {
        if let Some((channel, sessions, mirror)) = live_sessions(home, agent_id) {
            if let Err(e) = pull_sessions(&channel, agent_id, &sessions, &mirror) {
                tracing::warn!(agent_id, provider, error = %e, "pulling the sessions mirror failed");
            }
        }
    }
    mirror_for_agent(provider, agent_id)
}

/// Bring a pulled provider's mirrors up to date before its box stops — the
/// owner's and every tenant's, which share the owner's provider — so what
/// the agents wrote since the last read is not lost with the VM's pause;
/// nothing to do for a tailed one. Logged, never fatal: the stop goes on
/// either way.
pub(crate) async fn pull_before_stop(running: &super::RunningBox) {
    let identity = &running.identity;
    let Some(home) = for_provider(&identity.provider) else {
        return;
    };
    if home.transport() != Transport::Pull {
        return;
    }
    let mut agents = vec![(identity.agent_id.clone(), identity.writable_root.clone())];
    agents.extend(
        super::BoxRegistry::global()
            .tenants_of(&identity.agent_id)
            .into_iter()
            .map(|(id, tenant)| (id, tenant.writable_root)),
    );
    for (agent_id, root) in agents {
        let Some(mirror) = mirror_for_agent(home.id(), &agent_id) else {
            continue;
        };
        let channel = running.channel.clone();
        let sessions = home.sessions_dir(&identity.home, &root);
        let id = agent_id.clone();
        let pulled =
            tokio::task::spawn_blocking(move || pull_sessions(&channel, &id, &sessions, &mirror))
                .await
                .map_err(|e| Error::Other(e.to_string()))
                .and_then(|r| r);
        if let Err(e) = pulled {
            tracing::warn!(agent_id, error = %e, "sessions pull before stop failed");
        }
    }
}

/// Create the session file `rel` (relative to `home`'s sessions dir) with
/// `lines`, for agent `agent_id` working in `tree` — a session the host
/// wrote: a fork, a rewind. In the guest that holds `tree` (the agent's own
/// box, or its host's when it adopted the tree as a tenant), under the
/// agent's *own* sessions dir, where the CLI that resumes it looks; and in
/// the agent's mirror of that dir, which the relay fills guest → host only,
/// so the launch that follows sees a resumable session at once rather than
/// after the tail relays it back. Fails when something is already there on
/// either side. Returns the mirror path, the one the host's locators read.
pub(crate) fn create_session_file(
    home: &dyn ProviderHome,
    agent_id: &str,
    tree: &Path,
    rel: &Path,
    lines: &[Value],
) -> Result<PathBuf> {
    let identity = super::box_holding(tree)
        .ok_or_else(|| Error::Other(format!("{} is in no box", tree.display())))?;
    let root = crate::workspace::agent_parent_dir(agent_id)?;
    let mirror = mirror_for_agent(home.id(), agent_id)
        .ok_or_else(|| Error::Other("the box's mirror directory can't be resolved".into()))?
        .join(rel);
    let guest_path = home.sessions_dir(&identity.home, &root).join(rel);
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
    fn mirror_for_root_is_under_the_box_dir_of_the_roots_agent() {
        let root = Path::new("/w/workspaces/agent-7");
        let mirror = mirror_for_root(&crate::sandbox::provider::codex::Codex, root).unwrap();
        assert!(mirror.ends_with("agent-7/codex-sessions"), "{mirror:?}");
        assert_eq!(
            mirror,
            mirror_for_agent("codex", "agent-7").unwrap(),
            "the two spellings of one agent's mirror agree"
        );
        assert_eq!(mirror_for_agent("antigravity", "agent-7"), None);
    }

    /// The guest's change goes to the host, the host's to the guest, a
    /// change on both sides goes the guest's way, and a file the guest
    /// removed is not removed on the host.
    #[test]
    fn a_cli_owned_file_moves_toward_whoever_did_not_change_it() {
        let (a, b, c) = (Some([1; 32]), Some([2; 32]), Some([3; 32]));
        assert_eq!(reconcile_direction(a, a, a), Move::Nothing);
        assert_eq!(reconcile_direction(a, b, a), Move::ToHost);
        assert_eq!(reconcile_direction(a, a, b), Move::ToGuest);
        assert_eq!(reconcile_direction(a, b, c), Move::ToHost);
        assert_eq!(reconcile_direction(a, None, a), Move::Nothing);
        assert_eq!(reconcile_direction(a, None, None), Move::ToGuest);
        assert_eq!(reconcile_direction(None, None, a), Move::ToGuest);
        assert_eq!(reconcile_direction(None, a, None), Move::ToHost);
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

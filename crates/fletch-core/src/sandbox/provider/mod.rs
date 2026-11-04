//! Provider knowledge, once for every engine (docs/sandbox-foundation.md rule
//! 5): what a provider CLI keeps outside the checkout — first-run state,
//! shared config, the login, its session transcripts — how its credential
//! reaches a launch, and which engines can run it ([`supports`]).
//!
//! One adapter per provider behind [`ProviderHome`] ([`claude`], [`codex`],
//! [`cursor`], [`opencode`], [`pi`]) answers as data: the guest entries a
//! home must hold ([`GuestEntry`]), the launch's credential as the host
//! resolved it ([`Credential`]: env, and files with the side that writes
//! them), the sessions dir and how it reaches the host ([`Transport`]). The
//! box engine applies those answers over its channel (`sandbox::boxes::home`);
//! nothing here touches a guest. What the adapters share to resolve a
//! credential is [`auth`].
//!
//! Adding a provider to the box is one adapter plus its tools-disk script
//! (`fletch_box::tools`); nothing in the engine names a provider.

pub mod auth;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod opencode;
pub mod pi;

use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::sandbox::engine::{AgentLaunchCtx, EngineKind};

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

    /// What the guest home must hold before the CLI runs. Applied by the
    /// box engine (`boxes::home::seed`) before every launch; must be
    /// idempotent.
    fn seed(&self, home: &Path, writable_root: &Path) -> Result<Vec<GuestEntry>>;

    /// The launch's credential, resolved on the host. An error refuses the
    /// launch: a CLI that could only fail on its first turn fails here, with
    /// what to do about it.
    fn credential(&self, ctx: &AgentLaunchCtx) -> Result<Credential>;

    /// What the CLI writes in the guest outside the writable root, which
    /// every launch in the box shares, at the guest's own paths (the guest
    /// sets no XDG variable, wherever the host's put the user's copies).
    /// Every path [`seed`](Self::seed) names and every guest credential file
    /// lies in it or under the writable root, so a launch that sees it as a
    /// private copy (`boxes::confined_view`) is seeded and signed in there.
    fn shared_state(&self, home: &Path) -> SharedState;
}

/// What a CLI shares with every other launch in its box and writes
/// ([`ProviderHome::shared_state`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedState {
    /// Directories, whole.
    pub dirs: Vec<PathBuf>,
    /// Single files outside every one of [`dirs`](Self::dirs).
    pub files: Vec<PathBuf>,
}

/// How a provider's sessions dir reaches the host's mirror of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// The relay follows every `*.jsonl` under it as it grows, from the
    /// moment the box boots: for a CLI that only appends.
    Tail,
    /// The host copies the whole dir over when it reads the transcript
    /// (`boxes::home::sessions_mirror`) and before the box stops
    /// (`boxes::home::pull_before_stop`):
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
    /// Files the launch runs with, each kept in step between host and guest
    /// by its owner ([`FileOwner`]) for as long as the launch lives
    /// (`boxes::home::CredentialSync`).
    pub files: Vec<CredentialFile>,
}

/// A credential file on the host and where it lives in the guest — the same
/// path for per-agent state under the writable root (path identity), a
/// different one for state the user keeps where an XDG variable put it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialFile {
    pub host: PathBuf,
    pub guest: PathBuf,
    pub owner: FileOwner,
}

/// Who writes a credential file while a launch lives, and so which way it
/// is copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOwner {
    /// The host derives it from its stored login before every launch and
    /// turn (`agent::codex_home`: an access token with the refresh token
    /// blanked). The guest's copy is replaced each time and never read back:
    /// the CLI cannot refresh it, so it has nothing to say.
    Host,
    /// The CLI's own login, which it refreshes itself (an OAuth token it
    /// rotates mid-turn; a map of provider credentials). Shipped at launch,
    /// then reconciled as a read-write mount would keep both sides: what the
    /// guest changed is copied back to the host after every turn and when
    /// the launch ends, and what the host changed meanwhile (a new sign-in)
    /// is shipped when the guest did not touch its copy. The guest wins when
    /// both changed: its session is the live one.
    Cli,
}

impl CredentialFile {
    /// A host-owned file at the same path on both sides.
    pub fn host_owned(path: PathBuf) -> Self {
        Self {
            host: path.clone(),
            guest: path,
            owner: FileOwner::Host,
        }
    }

    /// A CLI-owned file at the same path on both sides.
    pub fn cli_owned(path: PathBuf) -> Self {
        Self {
            host: path.clone(),
            guest: path,
            owner: FileOwner::Cli,
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

/// The adapter of `provider`, or the error a launch reports.
pub fn require(provider: &str) -> Result<&'static dyn ProviderHome> {
    for_provider(provider)
        .ok_or_else(|| Error::Other(format!("the box engine does not run `{provider}`")))
}

/// Whether `engine` can launch `provider`, or the refusal the user sees: the
/// one provider support gate, checked before every spawn (fresh spawns and
/// resume/view-switch, `sandbox::ensure_supports_provider`) and behind the
/// box's automatic engine choice (`boxes::supports_provider`).
///
/// - Seatbelt runs every provider: the CLI runs on this host as it would
///   without Fletch.
/// - A box runs a provider with an adapter here and a tools-disk install
///   script (`fletch_box::tools`).
pub fn supports(engine: EngineKind, provider: &str) -> std::result::Result<(), String> {
    match engine {
        EngineKind::SandboxExec => Ok(()),
        EngineKind::Box => {
            if for_provider(provider).is_some()
                && fletch_box::tools::install_script(provider).is_some()
            {
                return Ok(());
            }
            let label = crate::agent::provider_bin_label(provider).map_or(provider, |(_, l)| l);
            Err(format!("{label} isn't available in Linux VMs yet"))
        }
    }
}

/// What a copied config tree may amount to: a file past [`MAX_FILE_BYTES`]
/// is left out, and the whole copy stops — nothing more is read or even
/// listed — at [`MAX_TREE_BYTES`] of file contents or [`MAX_TREE_ENTRIES`]
/// items looked at, files and directories alike. A config dir is small; a
/// link out of it into a build tree or a cache is not, and must not make a
/// launch walk it, let alone ship it.
const MAX_FILE_BYTES: u64 = 4 << 20;
const MAX_TREE_BYTES: u64 = 64 << 20;
const MAX_TREE_ENTRIES: usize = 4096;
/// How deep a config layout can reasonably go.
const COPY_DEPTH: usize = 16;

/// Entries that make `guest` a copy of the host's `host`: nothing when the
/// host has nothing there, one file for a file, the tree for a directory.
/// Symlinks are followed, as a link into a config dir means its target —
/// but a directory already on the path down is a cycle and is not entered
/// again, whatever the link that led there. Files land mode 0644, in
/// private directories (the seed's umask). What cannot be read or
/// resolved, and what is too big ([`MAX_FILE_BYTES`] and the tree limits),
/// is left out with a log line: a config oddity must not refuse a launch,
/// and a config file the host cannot read would not reach a seatbelt launch
/// either.
pub(crate) fn copy_tree(host: &Path, guest: &Path) -> Vec<GuestEntry> {
    let mut walk = TreeCopy {
        entries: Vec::new(),
        bytes: 0,
        seen: 0,
        on_path: Vec::new(),
    };
    walk.descend(host, guest);
    walk.entries
}

/// One copy in progress.
struct TreeCopy {
    entries: Vec<GuestEntry>,
    /// File bytes gathered so far, against [`MAX_TREE_BYTES`].
    bytes: u64,
    /// Items looked at so far, files and directories alike, against
    /// [`MAX_TREE_ENTRIES`]: the bound on the walk itself.
    seen: usize,
    /// The directories entered on the way down, by identity: a cycle is a
    /// directory that is already here.
    on_path: Vec<(u64, u64)>,
}

impl TreeCopy {
    /// Whether the copy has hit a tree limit; once it has, nothing more is
    /// looked at.
    fn full(&self) -> bool {
        self.seen >= MAX_TREE_ENTRIES || self.bytes >= MAX_TREE_BYTES
    }

    fn descend(&mut self, host: &Path, guest: &Path) {
        use std::os::unix::fs::MetadataExt as _;
        if self.full() {
            return;
        }
        self.seen += 1;
        let meta = match std::fs::metadata(host) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::debug!(path = %host.display(), error = %e, "config item skipped");
                return;
            }
        };
        if meta.is_file() {
            if meta.len() > MAX_FILE_BYTES {
                tracing::info!(path = %host.display(), len = meta.len(), "config file too big to copy into a box");
                return;
            }
            if self.bytes + meta.len() > MAX_TREE_BYTES {
                tracing::info!(path = %host.display(), "config tree too big to copy into a box; the rest is left out");
                self.bytes = MAX_TREE_BYTES;
                return;
            }
            if let Ok(bytes) = std::fs::read(host) {
                self.bytes += bytes.len() as u64;
                self.entries.push(GuestEntry::File {
                    path: guest.to_path_buf(),
                    bytes,
                    mode: 0o644,
                    keep_existing: false,
                });
            }
            return;
        }
        if !meta.is_dir() {
            return;
        }
        let identity = (meta.dev(), meta.ino());
        if self.on_path.contains(&identity) || self.on_path.len() >= COPY_DEPTH {
            tracing::debug!(path = %host.display(), "config directory cycle or depth; not entered");
            return;
        }
        self.entries.push(GuestEntry::Dir(guest.to_path_buf()));
        let Ok(read) = std::fs::read_dir(host) else {
            return;
        };
        // Listed only as far as the entry limit allows: a link into a
        // directory of a million children costs at most the limit.
        let remaining = MAX_TREE_ENTRIES.saturating_sub(self.seen);
        let mut names: Vec<_> = read
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .take(remaining + 1)
            .collect();
        if names.len() > remaining {
            tracing::info!(path = %host.display(), "config directory too wide to copy into a box; the rest is left out");
            names.truncate(remaining);
        }
        names.sort();
        self.on_path.push(identity);
        for name in names {
            if self.full() {
                tracing::info!(path = %host.display(), "config tree too big to copy into a box; the rest is left out");
                break;
            }
            self.descend(&host.join(&name), &guest.join(&name));
        }
        self.on_path.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every provider with an adapter runs under every engine; antigravity
    /// runs under seatbelt only, and a box refuses it with the provider's
    /// product name.
    #[test]
    fn the_gate_refuses_with_each_engines_copy() {
        for provider in ["claude", "codex", "cursor", "opencode", "pi"] {
            for &engine in EngineKind::ALL {
                assert_eq!(supports(engine, provider), Ok(()), "{provider} {engine:?}");
            }
        }
        assert_eq!(supports(EngineKind::SandboxExec, "antigravity"), Ok(()));
        let refusal = |engine| supports(engine, "antigravity").unwrap_err();
        assert_eq!(
            refusal(EngineKind::Box),
            "Antigravity isn't available in Linux VMs yet"
        );
    }

    #[test]
    fn claude_and_codex_have_adapters_and_nothing_else_does() {
        for supported in ["claude", "codex", "cursor", "opencode", "pi"] {
            assert_eq!(for_provider(supported).map(|h| h.id()), Some(supported));
        }
        for other in ["antigravity", "gemini", ""] {
            assert!(for_provider(other).is_none(), "{other}");
            assert!(require(other).is_err(), "{other}");
        }
    }

    /// Every adapter's mirror has a name of its own, so two providers'
    /// sessions never land in one dir of a box.
    /// Everything a seed writes outside the writable root lies in what the
    /// CLI shares ([`ProviderHome::shared_state`]), where a confined launch's
    /// view gives it a private copy; each adapter's own tests hold its guest
    /// credential files to the same.
    #[test]
    fn what_a_seed_writes_lies_in_the_shared_state_or_the_writable_root() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path().join("home");
        let root = home.join(".fletch/workspaces/a");
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        std::fs::write(home.join(".pi/agent/settings.json"), b"{}").unwrap();
        for adapter in ALL {
            let state = adapter.shared_state(&home);
            let covered = |path: &Path| {
                path.starts_with(&root)
                    || state.files.iter().any(|file| file == path)
                    || state.dirs.iter().any(|dir| path.starts_with(dir))
            };
            for entry in adapter.seed(&home, &root).unwrap() {
                let path = match &entry {
                    GuestEntry::Dir(path) | GuestEntry::Absent(path) => path,
                    GuestEntry::File { path, .. } => path,
                };
                assert!(covered(path), "{}: {}", adapter.id(), path.display());
            }
        }
    }

    #[test]
    fn mirror_names_are_distinct() {
        let mut names: Vec<&str> = ALL.iter().map(|h| h.mirror_dirname()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
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
        let entries = copy_tree(&src, guest);
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
        let entries = copy_tree(&file, Path::new("/g/config.toml"));
        assert_eq!(entries.len(), 1);
        assert!(
            matches!(&entries[0], GuestEntry::File { path, .. } if path == Path::new("/g/config.toml"))
        );

        assert!(copy_tree(&td.path().join("missing"), guest).is_empty());
    }

    /// A link back into a directory on the way down is a cycle: the
    /// directory is not entered again, whatever the link's spelling, and
    /// the rest of the tree still copies.
    #[test]
    fn copy_tree_does_not_enter_a_directory_twice() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("loop");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/a.md"), b"a").unwrap();
        std::os::unix::fs::symlink(&src, src.join("again")).unwrap();
        std::os::unix::fs::symlink("..", src.join("sub/up")).unwrap();
        let guest = Path::new("/g/loop");
        assert_eq!(
            copy_tree(&src, guest),
            vec![
                GuestEntry::Dir(guest.to_path_buf()),
                GuestEntry::Dir(guest.join("sub")),
                GuestEntry::File {
                    path: guest.join("sub/a.md"),
                    bytes: b"a".to_vec(),
                    mode: 0o644,
                    keep_existing: false,
                },
            ]
        );
    }

    /// A file past the per-file limit is left out; the rest still copies.
    #[test]
    fn copy_tree_leaves_out_a_file_too_big_to_ship() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("cfg");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("small.json"), b"{}").unwrap();
        let big = std::fs::File::create(src.join("big.bin")).unwrap();
        big.set_len(MAX_FILE_BYTES + 1).unwrap();
        let guest = Path::new("/g/cfg");
        assert_eq!(
            copy_tree(&src, guest),
            vec![
                GuestEntry::Dir(guest.to_path_buf()),
                GuestEntry::File {
                    path: guest.join("small.json"),
                    bytes: b"{}".to_vec(),
                    mode: 0o644,
                    keep_existing: false,
                },
            ]
        );
    }

    /// The walk itself is bounded: past the entry limit nothing more is
    /// listed, directories included, so a link into a vast tree costs at
    /// most the limit.
    #[test]
    fn copy_tree_stops_walking_at_the_entry_limit() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("wide");
        // More directories than the limit, each empty: nothing to ship, but
        // a walk to bound.
        for i in 0..MAX_TREE_ENTRIES + 50 {
            std::fs::create_dir_all(src.join(format!("d{i:05}"))).unwrap();
        }
        let entries = copy_tree(&src, Path::new("/g/wide"));
        assert_eq!(entries.len(), MAX_TREE_ENTRIES);
        assert!(entries.iter().all(|e| matches!(e, GuestEntry::Dir(_))));
    }
}

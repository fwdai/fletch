//! Tools disks: one read-only ext4 disk per `<provider>-<version>` holding that
//! provider's CLI, built by booting a throwaway box on the base image with a
//! fresh sparse disk as `/data` and running the provider's install script from
//! `crates/fletch-box/tools/` through the egress proxy. Init then mounts the
//! disk's `tools/` directory at `/opt/fletch/tools`, so the CLI is at
//! `/opt/fletch/tools/<provider>/bin/<cli>`.
//!
//! A disk may also carry codegraph, the code index Fletch gives every agent,
//! at `/opt/fletch/tools/codegraph`: the guest's counterpart of the host's
//! `~/.fletch/tools/codegraph`. Its bundle comes from the caller, already
//! verified; nothing about it is fetched in the build box. Whether a disk
//! carries it is part of the disk's name ([`disk_path`]), so a disk built
//! without it is never mistaken for one built with it.
//!
//! The scripts are embedded so the runtime does not depend on the repository
//! layout; the `fletch-box tools build` subcommand and the engine share
//! [`build`].

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::channel::{self, Channel};
use crate::disks;
use crate::egress::{EgressEvent, EgressPolicy, EgressProxy};
use crate::error::{Error, Result};
use crate::fsutil;
use crate::guest;
use crate::image_store;
use crate::runtime::{RuntimePaths, StopOutcome, Vm, VmSpec};
use crate::version::version_key;

/// The providers with an install script, by id.
pub const PROVIDERS: &[&str] = &["claude", "codex", "cursor", "opencode", "pi"];

/// How big the sparse disk is. Only what the install writes is allocated.
/// Every tools disk is created this size, so a marker-less disk of another
/// is truncated ([`DiskState::Damaged`]).
pub const DISK_BYTES: u64 = 4 << 30;

/// How long an install may take end to end: npm over the proxy, a cold cache.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// The provider's install script, as `crates/fletch-box/tools/<provider>.sh`.
pub fn install_script(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "claude" => include_str!("../tools/claude.sh"),
        "codex" => include_str!("../tools/codex.sh"),
        "cursor" => include_str!("../tools/cursor.sh"),
        "opencode" => include_str!("../tools/opencode.sh"),
        "pi" => include_str!("../tools/pi.sh"),
        _ => return None,
    })
}

/// The CLI's file name under `/opt/fletch/tools/<provider>/bin`.
pub fn cli_bin(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "claude" => "claude",
        "codex" => "codex",
        "cursor" => "cursor-agent",
        "opencode" => "opencode",
        "pi" => "pi",
        _ => return None,
    })
}

/// The CLI's absolute path in a guest with the provider's tools disk attached.
pub fn bin_path(provider: &str) -> Option<String> {
    cli_bin(provider).map(|bin| format!("{}/{provider}/bin/{bin}", guest::TOOLS_MOUNT))
}

/// The `bin` directory a launch puts first on `PATH`.
pub fn bin_dir(provider: &str) -> String {
    format!("{}/{provider}/bin", guest::TOOLS_MOUNT)
}

/// The codegraph install script, as `crates/fletch-box/tools/codegraph.sh`.
const CODEGRAPH_SCRIPT: &str = include_str!("../tools/codegraph.sh");

/// What a disk's file name carries, before the codegraph version, when the
/// disk holds codegraph.
const CODEGRAPH_MARK: &str = "+codegraph-";

/// A disk's marker is the disk's file name plus this ([`marker_path`]).
pub const MARKER_SUFFIX: &str = ".ok";
/// A file being written, a build's disk or a marker, is its final name plus
/// this until a rename puts it in place.
pub const STAGING_SUFFIX: &str = ".building";
/// The version a disk is named for when the host CLI's is unknown.
pub const LATEST_VERSION: &str = "latest";

/// A codegraph release bundle (`codegraph-linux-<arch>.tar.gz`) for [`build`]
/// to install next to the provider CLI. The caller has checked it against its
/// pinned digest; the build only pushes it into the build box.
#[derive(Debug, Clone, Copy)]
pub struct Codegraph<'a> {
    /// The release tag, as the disk's name records it.
    pub version: &'a str,
    pub bundle: &'a Path,
}

/// The codegraph launcher's absolute path in a guest whose tools disk carries
/// it ([`carries_codegraph`]).
pub fn codegraph_bin_path() -> String {
    format!("{}/codegraph/bin/codegraph", guest::TOOLS_MOUNT)
}

/// The disk file for `provider` at `version` under `root` (the engine's
/// `<data dir>/box/tools`), with codegraph at version `codegraph` on it or
/// without.
pub fn disk_path(root: &Path, provider: &str, version: &str, codegraph: Option<&str>) -> PathBuf {
    match codegraph {
        Some(codegraph) => root.join(format!(
            "{provider}-{version}{CODEGRAPH_MARK}{codegraph}.img"
        )),
        None => root.join(format!("{provider}-{version}.img")),
    }
}

/// The CLI version and codegraph version (if any) a disk of `provider` was
/// named with by [`disk_path`], read back from its file name; `None` for any
/// other file, a build's staging file included.
pub fn parse_disk_name<'a>(name: &'a str, provider: &str) -> Option<(&'a str, Option<&'a str>)> {
    let rest = name
        .strip_prefix(provider)?
        .strip_prefix('-')?
        .strip_suffix(".img")?;
    let (version, codegraph) = match rest.split_once(CODEGRAPH_MARK) {
        Some((version, codegraph)) => (version, Some(codegraph)),
        None => (rest, None),
    };
    valid_version(version).then_some((version, codegraph))
}

/// Whether the tools disk at `disk` carries codegraph, as its name
/// ([`disk_path`]) records.
pub fn carries_codegraph(disk: &Path) -> bool {
    disk.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains(CODEGRAPH_MARK))
}

/// What [`install_disk`] writes beside a disk once the disk is durably in
/// place. Its size is checked on every look ([`disk_state`]); its hash only
/// in the background, as hashing 4 GB takes seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskMarker {
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// What the file at a disk's path is, by the cheap checks: no read of the
/// disk itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskState {
    Missing,
    /// No marker, and [`DISK_BYTES`] long: a disk from an app version that
    /// wrote no markers, or a build stopped between installing the disk and
    /// its marker. A fallback only. The size check catches truncation, not
    /// garbage; only a marker's hash would.
    Unmarked,
    /// Its marker records its size.
    Built(DiskMarker),
    /// Truncated or replaced since it was built: its marker does not parse
    /// or records another size, or it has none and is not [`DISK_BYTES`]
    /// long. Never booted.
    Damaged,
    /// Its marker is there but could not be read (EACCES, EIO). Never
    /// booted, and never swept: the disk may well be fine.
    Unreadable(io::ErrorKind),
}

/// Where the marker of the disk at `disk` is: `<disk>.ok`.
pub fn marker_path(disk: &Path) -> PathBuf {
    with_suffix(disk, MARKER_SUFFIX)
}

fn staging_path(path: &Path) -> PathBuf {
    with_suffix(path, STAGING_SUFFIX)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

/// A marker is a few dozen bytes; a file longer than this is not one.
const MARKER_MAX_BYTES: u64 = 4096;

/// What the file at `disk` is ([`DiskState`]): a stat and a read of its
/// small marker, cheap enough for every look at the disk.
pub fn disk_state(disk: &Path) -> DiskState {
    let len = match fs::metadata(disk) {
        Ok(meta) if meta.is_file() => meta.len(),
        _ => return DiskState::Missing,
    };
    let mut bytes = Vec::new();
    let read = File::open(marker_path(disk))
        .and_then(|file| file.take(MARKER_MAX_BYTES).read_to_end(&mut bytes));
    match read {
        Err(e) if e.kind() == io::ErrorKind::NotFound && len == DISK_BYTES => DiskState::Unmarked,
        Err(e) if e.kind() == io::ErrorKind::NotFound => DiskState::Damaged,
        Err(e) => DiskState::Unreadable(e.kind()),
        Ok(_) => match serde_json::from_slice::<DiskMarker>(&bytes) {
            Ok(marker) if marker.size == len && image_store::well_formed_sha256(&marker.sha256) => {
                DiskState::Built(marker)
            }
            _ => DiskState::Damaged,
        },
    }
}

/// Delete the disk at `disk` and then its marker; either already gone is
/// fine. In that order, so a crash in between never leaves a disk without
/// the marker that would have kept it from passing as a legacy one.
pub fn remove_disk(disk: &Path) -> io::Result<()> {
    fsutil::remove_if_exists(disk)?;
    fsutil::remove_if_exists(&marker_path(disk))
}

/// Put the finished disk at `staging` in place as `out`, crash-safe: the disk
/// is hashed, synced and made read-only, renamed over `out` with the rename
/// synced, and only then its marker written the same way. A crash leaves at
/// `out` the previous file, nothing, or the whole disk with or without its
/// marker, never a partial disk the marker vouches for. Any failure after
/// the rename removes the disk again. Answers the marker written.
pub fn install_disk(staging: &Path, out: &Path) -> Result<DiskMarker> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(staging)
        .map_err(Error::io(format!("open {}", staging.display())))?;
    let size = file
        .metadata()
        .map_err(Error::io(format!("stat {}", staging.display())))?
        .len();
    let sha256 = image_store::sha256_of(&mut file)
        .map_err(Error::io(format!("hash {}", staging.display())))?;
    file.sync_all()
        .map_err(Error::io(format!("sync {}", staging.display())))?;
    drop(file);
    let mut perms = fs::metadata(staging)
        .map_err(Error::io(format!("stat {}", staging.display())))?
        .permissions();
    perms.set_readonly(true);
    fs::set_permissions(staging, perms)
        .map_err(Error::io(format!("chmod {}", staging.display())))?;
    // A marker left by an earlier disk at `out` must not vouch for this one.
    let marker_file = marker_path(out);
    fsutil::remove_if_exists(&marker_file)
        .map_err(Error::io(format!("remove {}", marker_file.display())))?;
    fs::rename(staging, out).map_err(Error::io(format!("install {}", out.display())))?;
    let marker = DiskMarker { size, sha256 };
    let installed = fsutil::sync_dir(out)
        .map_err(Error::io(format!("install {}", out.display())))
        .and_then(|()| write_marker(out, &marker));
    if let Err(e) = installed {
        let _ = remove_disk(out);
        return Err(e);
    }
    Ok(marker)
}

fn write_marker(disk: &Path, marker: &DiskMarker) -> Result<()> {
    let path = marker_path(disk);
    let json = serde_json::to_vec(marker).map_err(|e| Error::Other(e.to_string()))?;
    fsutil::write_atomic(&path, &staging_path(&path), &json)
        .map_err(Error::io(format!("write {}", path.display())))
}

/// A disk a box can boot with, as [`built_disk_in`] or [`bootable_disk_in`]
/// found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundDisk {
    pub path: PathBuf,
    /// `None` for a disk with no marker ([`DiskState::Unmarked`]).
    pub marker: Option<DiskMarker>,
}

/// `provider`'s built disk for `version` in `root` ([`DiskState::Built`]).
/// The one carrying codegraph at `codegraph` goes first whether indexing is
/// on or not: it holds everything the plain one does, and with indexing off
/// its server is simply never injected.
pub fn built_disk_in(
    root: &Path,
    provider: &str,
    version: &str,
    codegraph: &str,
) -> Option<FoundDisk> {
    [Some(codegraph), None].into_iter().find_map(|codegraph| {
        let path = disk_path(root, provider, version, codegraph);
        match disk_state(&path) {
            DiskState::Built(marker) => Some(FoundDisk {
                path,
                marker: Some(marker),
            }),
            _ => None,
        }
    })
}

/// The disk a box of `provider` can boot with now: [`built_disk_in`] for
/// `version`, else the newest of any version ([`newest_disk_in`]), so a box
/// boots on another CLI version rather than wait minutes for a build.
pub fn bootable_disk_in(
    root: &Path,
    provider: &str,
    version: &str,
    codegraph: &str,
) -> Option<FoundDisk> {
    built_disk_in(root, provider, version, codegraph)
        .or_else(|| newest_disk_in(root, provider, codegraph))
}

/// The newest disk of `provider` in `root` by CLI version that is built or
/// unmarked, plain or carrying codegraph at `codegraph` (another release's
/// server may not read the index this host builds). At one version a marked
/// disk wins, then the one carrying codegraph.
pub fn newest_disk_in(root: &Path, provider: &str, codegraph: &str) -> Option<FoundDisk> {
    fs::read_dir(root)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let (version, carried) = parse_disk_name(name.to_str()?, provider)?;
            if carried.is_some_and(|tag| tag != codegraph) {
                return None;
            }
            let path = entry.path();
            let marker = match disk_state(&path) {
                DiskState::Built(marker) => Some(marker),
                DiskState::Unmarked => None,
                _ => return None,
            };
            let rank = (version_key(version), marker.is_some(), carried.is_some());
            Some((rank, FoundDisk { path, marker }))
        })
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, disk)| disk)
}

/// What a sweep of one provider's disks removes ([`superseded_files`]).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Superseded {
    /// Disks, each to go with its marker ([`remove_disk`]).
    pub disks: Vec<PathBuf>,
    /// Markers whose disk is gone, and staging files.
    pub files: Vec<PathBuf>,
    /// Disks left alone because their marker could not be read.
    pub unreadable: Vec<(PathBuf, io::ErrorKind)>,
}

/// The files of `provider` in `root` a sweep removes, `version` being the
/// current CLI version and `codegraph` the current codegraph release:
/// - a disk that is neither the one the next boot takes
///   ([`bootable_disk_in`]) nor `in_use`. None when no disk is bootable (a
///   sweep never empties a provider), or when `version` or that disk's is
///   [`LATEST_VERSION`] (an unknown CLI version is no reason to delete a
///   known one's disk). A damaged disk not in use goes regardless: it never
///   boots;
/// - a marker whose disk is gone;
/// - a staging file (`<disk>.building`, `<disk>.ok.building`) of a build
///   that crashed or was killed mid-write.
///
/// Nothing of a version in `building` goes: its build renames its disk into
/// place before it writes the marker. Nor does another provider's file.
pub fn superseded_files(
    root: &Path,
    provider: &str,
    version: &str,
    codegraph: &str,
    in_use: &HashSet<PathBuf>,
    building: &HashSet<String>,
) -> Superseded {
    let mut swept = Superseded::default();
    let Ok(entries) = fs::read_dir(root) else {
        return swept;
    };
    let is_latest = |disk: &Path| {
        let name = disk.file_name().and_then(|name| name.to_str());
        name.and_then(|name| parse_disk_name(name, provider))
            .is_some_and(|(version, _)| version == LATEST_VERSION)
    };
    let next_boot = bootable_disk_in(root, provider, version, codegraph)
        .map(|disk| disk.path)
        .filter(|next| version != LATEST_VERSION && !is_latest(next));
    let settled = |disk: &str| {
        parse_disk_name(disk, provider).is_some_and(|(version, _)| !building.contains(version))
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(staged) = name.strip_suffix(STAGING_SUFFIX) {
            if settled(staged.strip_suffix(MARKER_SUFFIX).unwrap_or(staged)) {
                swept.files.push(path);
            }
        } else if let Some(disk) = name.strip_suffix(MARKER_SUFFIX) {
            if settled(disk) && !root.join(disk).is_file() {
                swept.files.push(path);
            }
        } else if settled(name) && !in_use.contains(&path) {
            match disk_state(&path) {
                DiskState::Unreadable(kind) => swept.unreadable.push((path, kind)),
                DiskState::Damaged => swept.disks.push(path),
                _ if next_boot.as_ref().is_some_and(|next| *next != path) => swept.disks.push(path),
                _ => {}
            }
        }
    }
    swept
}

/// A version that can stand in a disk's file name and a script's argument.
fn valid_version(version: &str) -> bool {
    !version.is_empty() && !version.contains(|c: char| c.is_whitespace() || c == '/')
}

/// Builds the tools disk for `provider` at `version` into `out`, with
/// `codegraph` installed next to the CLI when given, booting a throwaway box
/// on `image` under `paths.run_root`. `log` sees the install's output lines.
/// A failure leaves nothing at `out`; success leaves the disk there with its
/// marker, crash-safe ([`install_disk`]), and answers the marker.
pub async fn build(
    paths: &RuntimePaths,
    image: &Path,
    provider: &str,
    version: &str,
    codegraph: Option<Codegraph<'_>>,
    out: &Path,
    mut log: impl FnMut(&str),
) -> Result<DiskMarker> {
    let script = install_script(provider)
        .ok_or_else(|| Error::Other(format!("no install script for provider `{provider}`")))?;
    if !valid_version(version) {
        return Err(Error::Other(format!("bad version {version:?}")));
    }
    // Read before anything boots: a missing bundle fails the build up front.
    let codegraph_bundle = match codegraph {
        Some(codegraph) => {
            if !valid_version(codegraph.version) {
                return Err(Error::Other(format!(
                    "bad codegraph version {:?}",
                    codegraph.version
                )));
            }
            Some(
                tokio::fs::read(codegraph.bundle)
                    .await
                    .map_err(Error::io(format!("read {}", codegraph.bundle.display())))?,
            )
        }
        None => None,
    };
    let probe = paths.probe();
    if !probe.ok() {
        return Err(Error::Unsupported(probe.problems().join("; ")));
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(Error::io(format!("create {}", parent.display())))?;
    }

    // A box dir of its own, under the run root like every box's, so socket
    // paths stay short; removed on every exit path below.
    let id = format!("tools-{}", std::process::id());
    let dir = paths.run_root.join(&id);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(Error::io(format!("create {}", dir.display())))?;
    let staging = staging_path(out);
    let _ = std::fs::remove_file(&staging);
    let result = build_in(
        paths,
        image,
        provider,
        version,
        script,
        codegraph_bundle.as_deref(),
        &dir,
        &staging,
        &mut log,
    )
    .await;
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&staging);
        return Err(e);
    }
    // Read-only once built: every box attaches it `ro`, and nothing may change
    // what a version's name promises. Hashing and syncing 4 GB takes
    // seconds: off the async runtime.
    let (from, to) = (staging.clone(), out.to_path_buf());
    let installed = tokio::task::spawn_blocking(move || install_disk(&from, &to))
        .await
        .map_err(|e| Error::Other(format!("install {}: {e}", out.display())))
        .and_then(|installed| installed);
    if installed.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    installed
}

#[allow(clippy::too_many_arguments)]
async fn build_in(
    paths: &RuntimePaths,
    image: &Path,
    provider: &str,
    version: &str,
    script: &str,
    codegraph_bundle: Option<&[u8]>,
    dir: &Path,
    disk: &Path,
    log: &mut impl FnMut(&str),
) -> Result<()> {
    disks::create_sparse(disk, DISK_BYTES)
        .map_err(Error::io(format!("create {}", disk.display())))?;
    let key = channel::keygen(dir)?;
    let authorized = channel::authorized_key(&key)?;

    // The identity paths are a contract with init, not something the install
    // uses; a build has no agent, so they point at a scratch tree under the
    // user's home (whose top directory the image has).
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Error::Other("HOME is not set".into()))?;
    let scratch = home.join(".fletch").join("tools-build");
    let guest_home = scratch.join("home");

    // Denials are logged, not fatal: the CLIs probe telemetry hosts the
    // default allow-list leaves out and carry on.
    let proxy = EgressProxy::start(&dir.join("egress.sock"), EgressPolicy::default(), |event| {
        if let EgressEvent::Denied { host, port, reason } = event {
            tracing::info!(host, port, %reason, "tools build: egress denied");
        }
    })
    .map_err(Error::io("start the egress proxy"))?;

    let mut spec = VmSpec::new(
        &format!("tools-{}", std::process::id()),
        image,
        disk,
        &dir.join("console.log"),
    );
    spec.listen_ports = vec![
        (guest::SSH_PORT, dir.join("ssh.sock")),
        (guest::CONTROL_PORT, dir.join("ctl.sock")),
    ];
    spec.connect_ports = vec![(guest::EGRESS_PORT, proxy.socket().to_path_buf())];
    spec.env = BTreeMap::from([
        (
            guest::ENV_WRITABLE_ROOT.to_owned(),
            path_string(&scratch.join("ws")),
        ),
        (
            guest::ENV_RPC_DIR.to_owned(),
            path_string(&scratch.join("rpc")),
        ),
        (guest::ENV_HOME.to_owned(), path_string(&guest_home)),
        (guest::ENV_AUTHORIZED_KEY.to_owned(), authorized),
    ]);

    let vm = Vm::start(paths, &spec)?;
    let host_key = match vm.wait_ready(READY_TIMEOUT).await {
        Ok(key) => key,
        Err(e) => {
            vm.kill();
            return Err(e);
        }
    };
    let known_hosts = dir.join(channel::KNOWN_HOSTS_FILE);
    channel::write_known_hosts(&known_hosts, &host_key)?;
    let chan = Channel::new(
        dir,
        &dir.join("ssh.sock"),
        &key,
        &known_hosts,
        &paths.helper,
    )?;

    let result = install(
        &chan,
        provider,
        version,
        script,
        codegraph_bundle,
        &guest_home,
        log,
    )
    .await;
    // Clean stop whatever happened: a killed VM loses recent writes, and a
    // failed build's disk is removed anyway.
    let outcome = vm.stop(Duration::from_secs(15)).await;
    proxy.stop();
    result?;
    if outcome == StopOutcome::Killed {
        return Err(Error::Other(
            "the build box had to be killed; the tools disk may be incomplete".into(),
        ));
    }
    Ok(())
}

async fn install(
    chan: &Channel,
    provider: &str,
    version: &str,
    script: &str,
    codegraph_bundle: Option<&[u8]>,
    guest_home: &Path,
    log: &mut impl FnMut(&str),
) -> Result<()> {
    let env_file = PathBuf::from(guest::ENV_FILE_DIR).join("tools.env");
    let env = format!(
        "HOME={home}\nPATH=/usr/local/bin:/usr/bin:/bin\nSHELL=/bin/bash\nTERM=dumb\n\
         HTTPS_PROXY={proxy}\nHTTP_PROXY={proxy}\nhttps_proxy={proxy}\nhttp_proxy={proxy}\n\
         NO_PROXY=127.0.0.1,localhost\nno_proxy=127.0.0.1,localhost\n\
         GIT_TERMINAL_PROMPT=0\nnpm_config_update_notifier=false\n",
        home = guest_home.display(),
        proxy = guest::LOOPBACK_PROXY,
    );
    chan.put(env.as_bytes(), &env_file, 0o600).await?;
    run_script(
        chan,
        &env_file,
        provider,
        script,
        &[version],
        &format!("{provider}@{version}"),
        log,
    )
    .await?;
    if let Some(bundle) = codegraph_bundle {
        // tmpfs, so the bundle never lands on the disk being built.
        let bundle_path = Path::new("/tmp/codegraph.tar.gz");
        chan.put(bundle, bundle_path, 0o644).await?;
        run_script(
            chan,
            &env_file,
            "codegraph",
            CODEGRAPH_SCRIPT,
            &[&bundle_path.to_string_lossy()],
            "codegraph",
            log,
        )
        .await?;
    }
    // Each script's last line is `<cli> --version`; `sync` so the stop finds
    // nothing left to flush.
    chan.exec(None, None, &["sync"]).await?;
    Ok(())
}

/// Puts `script` into the guest as `/tmp/install-<name>.sh` and runs it with
/// `args` under the build's env file, its output going to `log`. `what` names
/// the install in errors.
async fn run_script(
    chan: &Channel,
    env_file: &Path,
    name: &str,
    script: &str,
    args: &[&str],
    what: &str,
    log: &mut impl FnMut(&str),
) -> Result<()> {
    let script_path = PathBuf::from(format!("/tmp/install-{name}.sh"));
    chan.put(script.as_bytes(), &script_path, 0o755).await?;

    let script_arg = script_path.to_string_lossy();
    let mut cmd = vec!["sh", script_arg.as_ref()];
    cmd.extend_from_slice(args);
    let run = chan.exec(Some(Path::new("/tmp")), Some(env_file), &cmd);
    let out = tokio::time::timeout(INSTALL_TIMEOUT, run)
        .await
        .map_err(|_| Error::Other(format!("installing {what} timed out")))??;
    for line in String::from_utf8_lossy(&out.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&out.stderr).lines())
    {
        log(line);
    }
    if !out.status.success() {
        return Err(Error::Other(format!(
            "installing {what} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_has_a_script_and_a_bin() {
        for provider in PROVIDERS {
            let script = install_script(provider).unwrap();
            assert!(script.starts_with("#!/bin/sh"), "{provider}");
            assert!(
                script.contains(&format!("/data/tools/{provider}")),
                "{provider}'s script must install under /data/tools/{provider}"
            );
            let bin = bin_path(provider).unwrap();
            assert!(bin.starts_with(&format!("{}/{provider}/bin/", guest::TOOLS_MOUNT)));
            assert!(bin.ends_with(cli_bin(provider).unwrap()));
        }
        assert!(install_script("antigravity").is_none());
        assert!(bin_path("antigravity").is_none());
        assert_eq!(
            disk_path(Path::new("/d/tools"), "claude", "2.1.0", None),
            PathBuf::from("/d/tools/claude-2.1.0.img")
        );
    }

    /// `parse_disk_name` reads back what `disk_path` wrote, for that provider
    /// only, and nothing from a staging file or a stray name.
    #[test]
    fn a_disk_name_reads_back_its_versions() {
        let root = Path::new("/d/tools");
        let name = |path: PathBuf| path.file_name().unwrap().to_string_lossy().into_owned();
        let plain = name(disk_path(root, "claude", "2.1.0", None));
        let with = name(disk_path(root, "claude", "v2.1.0", Some("v1.4.1")));
        assert_eq!(parse_disk_name(&plain, "claude"), Some(("2.1.0", None)));
        assert_eq!(
            parse_disk_name(&with, "claude"),
            Some(("v2.1.0", Some("v1.4.1")))
        );
        assert_eq!(parse_disk_name(&plain, "codex"), None);
        assert_eq!(parse_disk_name(&plain, "claud"), None);
        assert_eq!(parse_disk_name("claude-2.1.0.img.building", "claude"), None);
        assert_eq!(parse_disk_name("claude-.img", "claude"), None);
        assert_eq!(parse_disk_name("claude.img", "claude"), None);
    }

    /// The engine deletes disks by what this reads back, so no provider's
    /// disk may parse as another's: no name is another's plus `-…`. Nor is a
    /// marker or a staging file ever read as a disk, of any provider.
    #[test]
    fn no_providers_disks_read_as_anothers() {
        for a in PROVIDERS {
            for b in PROVIDERS {
                let name = format!("{b}-v1.0.0.img");
                assert_eq!(parse_disk_name(&name, a).is_some(), a == b, "{a} / {b}");
                let marker = marker_path(Path::new(&name));
                let marker = marker.to_str().unwrap();
                assert_eq!(parse_disk_name(marker, a), None, "{a} / {marker}");
                let staged = format!("{marker}{STAGING_SUFFIX}");
                assert_eq!(parse_disk_name(&staged, a), None, "{a} / {staged}");
            }
        }
    }

    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    /// The codegraph release the classification tests take as current.
    const CODEGRAPH: &str = "v1.4.1";

    /// Files named in `root` as an app version from before markers left
    /// them: a disk (`*.img`) with no marker and [`DISK_BYTES`] long (sparse),
    /// anything else empty.
    fn legacy(root: &Path, names: &[&str]) {
        for name in names {
            let file = File::create(root.join(name)).unwrap();
            if name.ends_with(".img") {
                file.set_len(DISK_BYTES).unwrap();
            }
        }
    }

    /// A disk at `disk` installed the way a build installs one.
    fn built(disk: &Path, content: &[u8]) -> DiskMarker {
        let staging = staging_path(disk);
        std::fs::write(&staging, content).unwrap();
        install_disk(&staging, disk).unwrap()
    }

    fn names(paths: &[PathBuf]) -> Vec<String> {
        let mut names: Vec<String> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// An installed disk is in place with a marker recording its size and
    /// hash, read-only, the staging files gone; the marker is what makes it
    /// `Built`.
    #[test]
    fn an_installed_disk_carries_its_marker() {
        let dir = tempfile::tempdir().unwrap();
        let out = disk_path(dir.path(), "claude", "2.1.0", None);
        assert_eq!(disk_state(&out), DiskState::Missing);
        let marker = built(&out, b"hello");
        assert_eq!(
            marker,
            DiskMarker {
                size: 5,
                sha256: HELLO_SHA256.into(),
            }
        );
        assert_eq!(disk_state(&out), DiskState::Built(marker));
        assert!(std::fs::metadata(&out).unwrap().permissions().readonly());
        let mut names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["claude-2.1.0.img", "claude-2.1.0.img.ok"]);
    }

    /// A marker that cannot be written takes the disk just renamed into place
    /// with it: a failed install leaves nothing at `out`.
    #[test]
    fn a_failed_install_leaves_no_disk() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("claude-2.1.0.img");
        std::fs::create_dir(staging_path(&marker_path(&out))).unwrap();
        let staging = staging_path(&out);
        std::fs::write(&staging, b"hello").unwrap();
        assert!(install_disk(&staging, &out).is_err());
        assert_eq!(disk_state(&out), DiskState::Missing);
        assert!(!marker_path(&out).exists());
    }

    /// With no marker a disk is `Unmarked` at the size every disk is created
    /// with and `Damaged` at any other (truncated); with one, `Damaged` when
    /// it records another size or does not parse, `Unreadable` when it cannot
    /// be read. Same size, other bytes passes: only the hash tells. Removing
    /// a disk takes its marker.
    #[test]
    fn a_disk_its_marker_does_not_describe_is_not_built() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("claude-2.1.0.img");
        legacy(dir.path(), &["claude-2.1.0.img"]);
        assert_eq!(disk_state(&disk), DiskState::Unmarked);
        std::fs::write(&disk, b"hello").unwrap();
        assert_eq!(disk_state(&disk), DiskState::Damaged);
        let marker = |size: u64| DiskMarker {
            size,
            sha256: HELLO_SHA256.into(),
        };
        write_marker(&disk, &marker(DISK_BYTES)).unwrap();
        assert_eq!(disk_state(&disk), DiskState::Damaged);
        write_marker(&disk, &marker(5)).unwrap();
        assert_eq!(disk_state(&disk), DiskState::Built(marker(5)));
        std::fs::write(&disk, b"jello").unwrap();
        assert_eq!(disk_state(&disk), DiskState::Built(marker(5)));
        assert!(!image_store::file_matches(&disk, Some(5), HELLO_SHA256).unwrap());
        std::fs::write(marker_path(&disk), b"{not json").unwrap();
        assert_eq!(disk_state(&disk), DiskState::Damaged);
        remove_disk(&disk).unwrap();
        assert!(!disk.exists() && !marker_path(&disk).exists());
        assert_eq!(disk_state(&disk), DiskState::Missing);
        remove_disk(&disk).unwrap();
        std::fs::write(&disk, b"hello").unwrap();
        std::fs::create_dir(marker_path(&disk)).unwrap();
        assert!(matches!(disk_state(&disk), DiskState::Unreadable(_)));
    }

    /// Installing over a disk already there (one an earlier app version left
    /// with no marker) replaces it, and an old marker never survives to vouch
    /// for the new disk.
    #[test]
    fn installing_over_a_disk_replaces_it_and_its_marker() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("claude-2.1.0.img");
        std::fs::write(&out, b"legacy").unwrap();
        std::fs::write(marker_path(&out), b"stale").unwrap();
        let marker = built(&out, b"hello");
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        assert_eq!(disk_state(&out), DiskState::Built(marker.clone()));
        assert_eq!(marker.sha256, HELLO_SHA256);
    }

    fn path_of(disk: Option<FoundDisk>) -> Option<PathBuf> {
        disk.map(|disk| disk.path)
    }

    /// A box boots with the disk carrying codegraph once it is built, the
    /// plain one until then, and nothing before either is.
    #[test]
    fn the_disk_carrying_codegraph_is_preferred_once_built() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        let with = disk_path(root, "claude", "2.1.0", Some(CODEGRAPH));
        let plain = disk_path(root, "claude", "2.1.0", None);
        let current = || path_of(built_disk_in(root, "claude", "2.1.0", CODEGRAPH));
        assert_eq!(current(), None);
        built(&plain, b"");
        assert_eq!(current(), Some(plain.clone()));
        built(&with, b"");
        assert_eq!(current(), Some(with.clone()));
        std::fs::remove_file(&plain).unwrap();
        assert_eq!(current(), Some(with));
    }

    /// After a host CLI update the newest disk stands in: ordered by version
    /// as numbers, never another provider's, a staging file or a disk with
    /// another codegraph release.
    #[test]
    fn the_newest_disk_stands_in_for_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let newest = |provider| path_of(newest_disk_in(root, provider, CODEGRAPH));
        assert_eq!(newest("claude"), None);
        assert_eq!(
            newest_disk_in(&root.join("absent"), "claude", CODEGRAPH),
            None
        );
        legacy(
            root,
            &[
                "claude-latest.img",
                "claude-v2.1.9.img",
                "claude-v2.1.312.img",
                "claude-v2.1.400.img.building",
                "claude-v2.1.500+codegraph-v0.0.1.img",
                "codex-v9.9.9.img",
            ],
        );
        assert_eq!(newest("claude"), Some(root.join("claude-v2.1.312.img")));
        assert_eq!(newest("codex"), Some(root.join("codex-v9.9.9.img")));
        assert_eq!(newest("cursor"), None);
        // At one version the disk carrying this codegraph release wins.
        let with = format!("claude-v2.1.312+codegraph-{CODEGRAPH}.img");
        legacy(root, &[with.as_str()]);
        assert_eq!(newest("claude"), Some(root.join(with)));
    }

    /// The upgrade: a disk built before markers (here the 2.1.314 one, an
    /// APFS clone of 2.1.312's content) is never this version's built disk,
    /// so the version is built again, but it is the fallback a launch boots
    /// meanwhile. At one version a marked disk wins.
    #[test]
    fn a_disk_without_a_marker_is_a_fallback_but_never_built() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        legacy(root, &["claude-v2.1.312.img", "claude-v2.1.314.img"]);
        let current = || path_of(built_disk_in(root, "claude", "v2.1.314", CODEGRAPH));
        assert_eq!(current(), None);
        let bootable = bootable_disk_in(root, "claude", "v2.1.314", CODEGRAPH).unwrap();
        assert_eq!(bootable.path, root.join("claude-v2.1.314.img"));
        assert_eq!(bootable.marker, None);
        // Rebuilt properly, it is the built disk and the one booted.
        std::fs::remove_file(root.join("claude-v2.1.314.img")).unwrap();
        built(&root.join("claude-v2.1.314.img"), b"tools");
        assert_eq!(current(), Some(root.join("claude-v2.1.314.img")));
        // At one version, the marked disk over an unmarked codegraph one.
        let with = format!("claude-v2.1.314+codegraph-{CODEGRAPH}.img");
        legacy(root, &[with.as_str()]);
        assert_eq!(
            path_of(newest_disk_in(root, "claude", CODEGRAPH)),
            Some(root.join("claude-v2.1.314.img"))
        );
    }

    /// A disk whose marker does not match its size, or a truncated legacy
    /// one, is neither built nor a fallback: an older disk boots instead.
    #[test]
    fn a_damaged_disk_is_neither_built_nor_a_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        legacy(root, &["claude-v2.1.312.img"]);
        std::fs::write(root.join("claude-v2.1.313.img"), b"truncated").unwrap();
        let disk = root.join("claude-v2.1.314.img");
        built(&disk, b"tools");
        let newest = || path_of(newest_disk_in(root, "claude", CODEGRAPH));
        assert_eq!(newest(), Some(disk.clone()));
        std::fs::write(
            marker_path(&disk),
            format!(r#"{{"size":{DISK_BYTES},"sha256":"{HELLO_SHA256}"}}"#),
        )
        .unwrap();
        assert_eq!(built_disk_in(root, "claude", "v2.1.314", CODEGRAPH), None);
        assert_eq!(newest(), Some(root.join("claude-v2.1.312.img")));
    }

    /// Every file a sweep removes, disks and loose files together.
    fn swept(
        root: &Path,
        provider: &str,
        version: &str,
        in_use: &[&str],
        building: &[&str],
    ) -> Vec<String> {
        let swept = sweep(root, provider, version, in_use, building);
        let mut all = names(&swept.disks);
        all.extend(names(&swept.files));
        all.sort();
        all
    }

    fn sweep(
        root: &Path,
        provider: &str,
        version: &str,
        in_use: &[&str],
        building: &[&str],
    ) -> Superseded {
        let in_use = in_use.iter().map(|n| root.join(n)).collect();
        let building = building.iter().map(|v| v.to_string()).collect();
        superseded_files(root, provider, version, CODEGRAPH, &in_use, &building)
    }

    /// A sweep removes every disk of the provider but the one the next boot
    /// takes and those in use, and staging files no build is writing; never
    /// another provider's files or a name it does not parse.
    #[test]
    fn a_sweep_removes_only_superseded_disks_of_its_provider() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let codegraph = format!("claude-v2.1.314+codegraph-{CODEGRAPH}.img");
        legacy(
            root,
            &[
                "claude-latest.img",
                "claude-v2.1.311.img",
                "claude-v2.1.312.img",
                "claude-v2.1.314.img",
                &codegraph,
                "claude-v2.1.314+codegraph-v0.0.1.img",
                "claude-v2.1.300.img.building",
                "codex-v1.0.0.img",
                "codex-v1.0.1.img.building",
                "claude-notes.txt",
            ],
        );
        std::fs::create_dir(root.join("claude-v9.9.9.img")).unwrap();
        // The new version's disk is built; 2.1.312 still runs in a box.
        assert_eq!(
            swept(root, "claude", "v2.1.314", &["claude-v2.1.312.img"], &[]),
            [
                "claude-latest.img",
                "claude-v2.1.300.img.building",
                "claude-v2.1.311.img",
                "claude-v2.1.314+codegraph-v0.0.1.img",
                "claude-v2.1.314.img",
            ]
        );
        // Once nothing uses it, 2.1.312 goes too.
        assert!(swept(root, "claude", "v2.1.314", &[], &[])
            .contains(&"claude-v2.1.312.img".to_string()));
        // The other provider: its own disks only.
        assert_eq!(
            swept(root, "codex", "v1.0.0", &[], &[]),
            ["codex-v1.0.1.img.building"]
        );
    }

    /// While a version builds, its staging file stays, and the fallback the
    /// next boot takes stays; with no disk to boot, no disk is removed.
    #[test]
    fn a_sweep_spares_a_running_build_and_never_empties_a_provider() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        legacy(
            root,
            &[
                "claude-latest.img",
                "claude-v2.1.312.img",
                "claude-v2.1.314.img.building",
            ],
        );
        assert_eq!(
            swept(root, "claude", "v2.1.314", &[], &["v2.1.314"]),
            ["claude-latest.img"]
        );
        // A build that is gone left its staging file behind.
        assert_eq!(
            swept(root, "claude", "v2.1.314", &[], &[]),
            ["claude-latest.img", "claude-v2.1.314.img.building"]
        );
        // Only a disk with another codegraph release: nothing bootable.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        legacy(
            root,
            &[
                "claude-v2.1.312+codegraph-v0.0.1.img",
                "claude-v2.1.314.img.building",
            ],
        );
        assert_eq!(
            swept(root, "claude", "v2.1.314", &[], &[]),
            ["claude-v2.1.314.img.building"]
        );
        assert!(swept(&root.join("absent"), "claude", "v2.1.314", &[], &[]).is_empty());
    }

    /// While the CLI version is unknown (`latest`) no disk is swept, whether
    /// a `latest` disk is built or a versioned one stands in; and a `latest`
    /// disk standing in for a known version displaces no versioned one.
    #[test]
    fn a_latest_version_sweeps_no_versioned_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        legacy(
            root,
            &[
                "claude-latest.img",
                "claude-v2.1.311.img",
                "claude-v2.1.312.img",
                "claude-latest.img.building",
            ],
        );
        assert_eq!(
            swept(root, "claude", LATEST_VERSION, &[], &[]),
            ["claude-latest.img.building"]
        );
        std::fs::remove_file(root.join("claude-latest.img")).unwrap();
        assert_eq!(
            swept(root, "claude", LATEST_VERSION, &[], &[]),
            ["claude-latest.img.building"]
        );
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        legacy(
            root,
            &["claude-latest.img", "claude-v2.1.312+codegraph-v0.0.1.img"],
        );
        assert!(swept(root, "claude", "v2.1.314", &[], &[]).is_empty());
    }

    /// A disk goes with its marker ([`remove_disk`]), so the marker is not
    /// listed apart; a marker goes alone once its disk is gone; a marker's
    /// staging file is a staging file. Nothing of a version being built
    /// goes, its marker not yet written included.
    #[test]
    fn a_sweep_takes_markers_with_their_disks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        built(&root.join("claude-v2.1.311.img"), b"old");
        built(&root.join("claude-v2.1.312.img"), b"in use");
        built(&root.join("claude-v2.1.314.img"), b"next");
        legacy(
            root,
            &[
                "claude-v2.1.300.img.ok",
                "claude-v2.1.313.img.ok.building",
                "claude-v2.1.315.img",
                "claude-v2.1.315.img.ok.building",
                "codex-v1.0.0.img.ok",
            ],
        );
        let swept = sweep(
            root,
            "claude",
            "v2.1.314",
            &["claude-v2.1.312.img"],
            &["v2.1.315"],
        );
        assert_eq!(names(&swept.disks), ["claude-v2.1.311.img"]);
        assert_eq!(
            names(&swept.files),
            ["claude-v2.1.300.img.ok", "claude-v2.1.313.img.ok.building"]
        );
    }

    /// A damaged disk goes even when no other disk is swept (an unknown CLI
    /// version), a legacy one does not, and one in use stays. A disk whose
    /// marker cannot be read is left alone and reported.
    #[test]
    fn a_sweep_takes_damaged_disks_and_leaves_unreadable_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        built(&root.join("claude-v2.1.314.img"), b"tools");
        std::fs::write(root.join("claude-v2.1.314.img.ok"), b"{}").unwrap();
        legacy(root, &["claude-v2.1.312.img"]);
        std::fs::write(root.join("claude-v2.1.313.img"), b"truncated").unwrap();
        assert_eq!(
            swept(root, "claude", LATEST_VERSION, &[], &[]),
            ["claude-v2.1.313.img", "claude-v2.1.314.img"]
        );
        assert_eq!(
            swept(
                root,
                "claude",
                LATEST_VERSION,
                &["claude-v2.1.313.img", "claude-v2.1.314.img"],
                &[]
            ),
            Vec::<String>::new()
        );

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        legacy(root, &["claude-v2.1.312.img", "claude-v2.1.314.img"]);
        let unreadable = root.join("claude-v2.1.312.img");
        std::fs::create_dir(marker_path(&unreadable)).unwrap();
        let swept = sweep(root, "claude", "v2.1.314", &[], &[]);
        assert_eq!(swept.disks, Vec::<PathBuf>::new());
        assert_eq!(swept.files, Vec::<PathBuf>::new());
        assert_eq!(swept.unreadable.len(), 1);
        assert_eq!(swept.unreadable[0].0, unreadable);
    }

    /// The name is the record of whether codegraph is on a disk: the two
    /// builds of one CLI version never share a file, and only the one built
    /// with it reads as carrying it.
    #[test]
    fn a_disk_name_says_whether_it_carries_codegraph() {
        let root = Path::new("/d/tools");
        let plain = disk_path(root, "claude", "2.1.0", None);
        let with = disk_path(root, "claude", "2.1.0", Some("v1.4.1"));
        assert_eq!(
            with,
            PathBuf::from("/d/tools/claude-2.1.0+codegraph-v1.4.1.img")
        );
        assert!(!carries_codegraph(&plain));
        assert!(carries_codegraph(&with));
        // The build's staging file is the disk's name plus a suffix, so two
        // variants never stage over each other either.
        assert_ne!(
            plain.with_extension("img.building"),
            with.with_extension("img.building")
        );
        assert_eq!(
            codegraph_bin_path(),
            format!("{}/codegraph/bin/codegraph", guest::TOOLS_MOUNT)
        );
    }

    #[test]
    fn the_codegraph_script_installs_beside_the_provider_clis() {
        assert!(CODEGRAPH_SCRIPT.starts_with("#!/bin/sh"));
        assert!(CODEGRAPH_SCRIPT.contains("PREFIX=/data/tools/codegraph"));
        // Installs what it is handed; the build box never downloads it.
        assert!(!CODEGRAPH_SCRIPT.contains("curl") && !CODEGRAPH_SCRIPT.contains("wget"));
        assert!(!PROVIDERS.contains(&"codegraph"));
    }

    #[test]
    fn versions_that_cannot_name_a_file_are_refused() {
        assert!(valid_version("2.1.0"));
        assert!(valid_version("v1.4.1"));
        assert!(!valid_version(""));
        assert!(!valid_version("1.0 beta"));
        assert!(!valid_version("../x"));
    }
}

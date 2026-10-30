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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::channel::{self, Channel};
use crate::disks;
use crate::egress::{EgressEvent, EgressPolicy, EgressProxy};
use crate::error::{Error, Result};
use crate::guest;
use crate::runtime::{RuntimePaths, StopOutcome, Vm, VmSpec};

/// The providers with an install script, by id.
pub const PROVIDERS: &[&str] = &["claude", "codex", "cursor", "opencode"];

/// How big the sparse disk is. Only what the install writes is allocated.
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

/// A version that can stand in a disk's file name and a script's argument.
fn valid_version(version: &str) -> bool {
    !version.is_empty() && !version.contains(|c: char| c.is_whitespace() || c == '/')
}

/// Builds the tools disk for `provider` at `version` into `out`, with
/// `codegraph` installed next to the CLI when given, booting a throwaway box
/// on `image` under `paths.run_root`. `log` sees the install's output lines.
/// A failure leaves nothing at `out`.
pub async fn build(
    paths: &RuntimePaths,
    image: &Path,
    provider: &str,
    version: &str,
    codegraph: Option<Codegraph<'_>>,
    out: &Path,
    mut log: impl FnMut(&str),
) -> Result<()> {
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
    let staging = out.with_extension("img.building");
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
    // what a version's name promises.
    let mut perms = std::fs::metadata(&staging)
        .map_err(Error::io(format!("stat {}", staging.display())))?
        .permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&staging, perms)
        .map_err(Error::io(format!("chmod {}", staging.display())))?;
    std::fs::rename(&staging, out).map_err(Error::io(format!("install {}", out.display())))?;
    Ok(())
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
        assert!(install_script("pi").is_none());
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

//! Tools disks: one read-only ext4 disk per `<provider>-<version>` holding that
//! provider's CLI, built by booting a throwaway box on the base image with a
//! fresh sparse disk as `/data` and running the provider's install script from
//! `packaging/box-image/tools/` through the egress proxy (docs/boxes-spec.md
//! §7.5). Init then mounts the disk's `tools/` directory at
//! `/opt/fletch/tools`, so the CLI is at `/opt/fletch/tools/<provider>/bin/<cli>`.
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

/// The provider's install script, as `packaging/box-image/tools/<provider>.sh`.
pub fn install_script(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "claude" => include_str!("../../../packaging/box-image/tools/claude.sh"),
        "codex" => include_str!("../../../packaging/box-image/tools/codex.sh"),
        "cursor" => include_str!("../../../packaging/box-image/tools/cursor.sh"),
        "opencode" => include_str!("../../../packaging/box-image/tools/opencode.sh"),
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

/// The disk file for `provider` at `version` under `root` (the engine's
/// `<data dir>/box/tools`).
pub fn disk_path(root: &Path, provider: &str, version: &str) -> PathBuf {
    root.join(format!("{provider}-{version}.img"))
}

/// Builds the tools disk for `provider` at `version` into `out`, booting a
/// throwaway box on `image` under `paths.run_root`. `log` sees the install's
/// output lines. A failure leaves nothing at `out`.
pub async fn build(
    paths: &RuntimePaths,
    image: &Path,
    provider: &str,
    version: &str,
    out: &Path,
    mut log: impl FnMut(&str),
) -> Result<()> {
    let script = install_script(provider)
        .ok_or_else(|| Error::Other(format!("no install script for provider `{provider}`")))?;
    if version.is_empty() || version.contains(char::is_whitespace) {
        return Err(Error::Other(format!("bad version {version:?}")));
    }
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
        paths, image, provider, version, script, &dir, &staging, &mut log,
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

    let result = install(&chan, provider, version, script, &guest_home, log).await;
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
    let script_path = PathBuf::from(format!("/tmp/install-{provider}.sh"));
    chan.put(script.as_bytes(), &script_path, 0o755).await?;

    let script_arg = script_path.to_string_lossy();
    let cmd = ["sh", script_arg.as_ref(), version];
    let run = chan.exec(Some(Path::new("/tmp")), Some(&env_file), &cmd);
    let out = tokio::time::timeout(INSTALL_TIMEOUT, run)
        .await
        .map_err(|_| Error::Other(format!("installing {provider} timed out")))??;
    for line in String::from_utf8_lossy(&out.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&out.stderr).lines())
    {
        log(line);
    }
    if !out.status.success() {
        return Err(Error::Other(format!(
            "installing {provider}@{version} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    // The script's last line is `<cli> --version`; `sync` so the stop finds
    // nothing left to flush.
    chan.exec(None, None, &["sync"]).await?;
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
            disk_path(Path::new("/d/tools"), "claude", "2.1.0"),
            PathBuf::from("/d/tools/claude-2.1.0.img")
        );
    }
}

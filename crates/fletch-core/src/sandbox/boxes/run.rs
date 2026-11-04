//! The Run panel's process in a box: the project's setup and dev commands
//! run in the guest, where the checkout is, and the dev server's port is
//! forwarded to a port on this host, where the browser is.
//!
//! A run is a launch of the box like an agent's session or the workspace
//! terminal: it holds the box against its idle stop from before it reaches
//! it ([`super::Hold`], booting a stopped box) until the session ends, and
//! one [`BoxEngine::kill`](super::BoxEngine) — run by the session's own kill,
//! by its drop when the process exits on its own, and at app exit — removes
//! the env file, drops the forward and detaches the launch, which arms the
//! idle timer. The command gets exactly the environment an agent launch gets
//! (`fletch-guest exec --env-file`), plus what the panel adds (the project's
//! shared env, the pinned `PORT`), and runs under a tty so the dev server
//! prints as it does on the host.
//!
//! Ports: the dev server binds its own port in the guest, whose loopback is
//! the VM's alone, so two workspaces on port 3000 never clash there. Each
//! guest port is forwarded to a free host port — the same number when it is
//! free on this host, else the next one, as the host's own Run bumps a taken
//! port — on an ssh connection of its own, which ends with the launch.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fletch_box::channel::Forward;
use fletch_box::Channel;

use super::registry::{BoxIdentity, BoxRegistry};
use super::{box_holding, executor, launch_files, BoxEngine, Hold, ENV_DENY};
use crate::error::{Error, Result};
use crate::sandbox::KillHandle;

/// How long a forward's host listener may take to come up: one ssh
/// connection through the proxy helper, handshake included.
const FORWARD_READY: Duration = Duration::from_secs(20);

/// The first free TCP port in the guest at or after `$1`, up to `$1 + $2`:
/// a port something accepts a connection on is taken, as the host's own
/// probe counts a port it cannot bind. `localhost` tries every loopback
/// address, so a server on `::1` alone counts too. Exits 3 with none free.
const FREE_PORT_SCRIPT: &str = "end=$(($1 + $2)); p=$1; \
    while [ \"$p\" -le \"$end\" ] && [ \"$p\" -le 65535 ]; do \
    (exec 3<>\"/dev/tcp/localhost/$p\") 2>/dev/null || { echo \"$p\"; exit 0; }; \
    p=$((p + 1)); done; exit 3";

/// A Run in a box, from the hold on the box to the command's launch.
pub struct GuestRun {
    identity: BoxIdentity,
    launch_id: String,
    hold: Option<Hold>,
    channel: Arc<Channel>,
    forward: Option<Forward>,
}

impl GuestRun {
    /// Hold the box that keeps `tree` and reach it, booting it when it is
    /// stopped. `None` when `tree` is a host path. Reached through
    /// `sandbox::guest_run`, the facade the supervisor calls.
    pub(in crate::sandbox) fn open(tree: &Path) -> Result<Option<Self>> {
        let Some(identity) = box_holding(tree) else {
            return Ok(None);
        };
        let launch_id = uuid::Uuid::new_v4().to_string();
        // Attached before the box is reached, as every launch is: the idle
        // stop spares only a box attached when it takes the VM.
        let hold = Hold::take(&identity.agent_id, &launch_id);
        let channel = BoxRegistry::global().channel_for(&identity.agent_id)?;
        Ok(Some(Self {
            identity,
            launch_id,
            hold: Some(hold),
            channel,
            forward: None,
        }))
    }

    /// The first free TCP port in the guest at or after `start`, scanning up
    /// to `start + cap`; `None` when every port in range is taken. The
    /// guest's twin of `run_detect::port::find_free_port`: the dev server
    /// binds there, not on this host.
    pub fn free_port(&self, start: u16, cap: u16) -> Result<Option<u16>> {
        let (start, cap) = (start.to_string(), cap.to_string());
        let words = ["bash", "-c", FREE_PORT_SCRIPT, "bash", &start, &cap];
        let out = executor::run(&self.channel, &words, None)
            .map_err(|e| Error::Other(format!("probe the box's ports: {e}")))?;
        match out.status.code() {
            Some(0) => String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<u16>()
                .map(Some)
                .map_err(|e| Error::Other(format!("probe the box's ports: {e}"))),
            Some(3) => Ok(None),
            _ => Err(Error::Other(format!(
                "probe the box's ports: exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))),
        }
    }

    /// Forward `guest_port` of the box to a free port on this host — the same
    /// number when it is free here, else the next free one up to `cap` past
    /// it — and wait for the listener to come up. Returns the host port. The
    /// forward lives with the launch from [`Self::launch`] on; before that it
    /// drops with this run.
    pub fn forward(&mut self, guest_port: u16, cap: u16) -> Result<u16> {
        let host_port =
            crate::run_detect::port::find_free_port(guest_port, cap).ok_or_else(|| {
                Error::Other(format!(
                    "No free port available on this Mac in {guest_port}\u{2013}{}",
                    guest_port.saturating_add(cap)
                ))
            })?;
        let channel = self.channel.clone();
        let forward = super::block_on(async move {
            let mut forward = channel.forward(host_port, guest_port)?;
            forward.wait_ready(FORWARD_READY).await?;
            Ok::<_, fletch_box::Error>(forward)
        })
        .map_err(|e| {
            Error::Other(format!(
                "forward port {guest_port} of the box to port {host_port} on this Mac: {e}"
            ))
        })?;
        self.forward = Some(forward);
        Ok(host_port)
    }

    /// The host program and arguments that run the shell command `cmd` in
    /// `cwd` of the guest, under a tty, with `env` on top of what every guest
    /// process gets (`guest_env`, the project's toolchain cache), and the
    /// kill handle that ends the launch: the env file removed, the forward
    /// dropped, the box released. The session the caller spawns must own the
    /// handle, or run it (`sandbox::KillGuard`).
    pub fn launch(
        mut self,
        cwd: &Path,
        cmd: &str,
        env: &[(String, String)],
    ) -> Result<(PathBuf, Vec<String>, KillHandle)> {
        let (cache_root, cache_env) =
            super::guest_toolchain_cache(&self.identity.home, &self.identity.project_id)?;
        // The cache root, as before an agent launch; a guest that cannot make
        // it leaves the toolchains their defaults (its own disk anyway).
        let made = executor::run(
            &self.channel,
            &["mkdir", "-p", "--", &cache_root.to_string_lossy()],
            None,
        );
        let mut all = match made {
            Ok(out) if out.status.success() => cache_env,
            _ => Vec::new(),
        };
        all.extend_from_slice(env);
        let contents =
            super::render_env_file(&all, &self.identity.provider, &self.identity.home, ENV_DENY)?;
        let env_file = launch_files::env_file(&self.launch_id);
        executor::put(&self.channel, contents.as_bytes(), &env_file, 0o600)?;
        // Job control off, as on the host: everything the command starts
        // stays in one process group under the guest tty, which sshd hangs
        // up when the host side ends, so one hangup reaches all of it.
        let script = format!("set +m; {cmd}");
        let (program, args) = self
            .channel
            .pty_command(cwd, &env_file, &["bash", "-c", &script]);
        if let Some(forward) = self.forward.take() {
            BoxEngine::shared().park_forward(&self.launch_id, forward);
        }
        let kill = self
            .hold
            .take()
            .expect("the hold is kept until the launch")
            .into_kill();
        Ok((program, args, kill))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe's shape: bash's own loopback connect, every address of
    /// `localhost`, the range inclusive, nothing free a distinct exit.
    #[test]
    fn the_free_port_script_scans_the_range_inclusive_and_exits_three_when_none() {
        let run = |start: &str, cap: &str| {
            std::process::Command::new("bash")
                .args(["-c", FREE_PORT_SCRIPT, "bash", start, cap])
                .output()
                .unwrap()
        };
        // A port held open is skipped; the next one is free.
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        assert!(port < u16::MAX - 1);
        let out = run(&port.to_string(), "1");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            (port + 1).to_string()
        );
        // A range of one held port only: nothing free.
        let out = run(&port.to_string(), "0");
        assert_eq!(out.status.code(), Some(3));
    }
}

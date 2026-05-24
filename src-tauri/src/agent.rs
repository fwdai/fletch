//! Per-agent lifecycle.
//!
//! An `Agent` is the live, in-memory counterpart of an `AgentRecord`. It owns
//! the running `tart run` child process and the SSH PTY session pumping bytes
//! to the frontend.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::time::{sleep, timeout};

use crate::error::{Error, Result};
use crate::pty_bridge::{PtySession, SshSpawn, SshTarget};
use crate::vm::{MountSpec, Vm};

/// How long to wait for a freshly cloned VM to acquire an IP. Cloned VMs
/// boot faster than the original (cloud-init artifacts are baked in) but
/// 90s gives some headroom for system load.
const VM_BOOT_TIMEOUT: Duration = Duration::from_secs(90);
/// How long to wait for sshd to start accepting on port 22 after the VM
/// has an IP. Bumped to 2 minutes because cirruslabs cloned VMs sometimes
/// reboot during cloud-init — when that happens the host kernel sees
/// "No route to host" until a fresh DHCP lease finalizes and we re-query
/// `tart ip` (see [`wait_for_ssh_port_with_reip`]).
const SSH_PORT_TIMEOUT: Duration = Duration::from_secs(120);
/// Per-`connect` timeout. The macOS kernel will spend ~75s retrying SYN
/// on its own if we don't cut it short.
const SSH_CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
/// How often to re-query `tart ip` during the SSH wait, in case the VM
/// rebooted and the IP we already have is stale.
const REIP_INTERVAL: Duration = Duration::from_secs(8);

/// Username baked into the base image. Hardcoded for v1 — eventually moves
/// into per-workspace config alongside `base_image`.
pub const GUEST_USER: &str = "admin";

/// Mount point inside the guest. The base image is expected to either
/// pre-create this directory or have an autofs/systemd unit that creates it
/// before the mount runs.
pub const GUEST_WORKSPACE: &str = "/workspace";

/// Tag used for the virtiofs share. Must match the guest-side mount unit.
pub const SHARE_TAG: &str = "workspace";

pub struct Agent {
    #[allow(dead_code)]
    pub id: String,
    pub vm_name: String,
    #[allow(dead_code)]
    pub worktree: PathBuf,
    #[allow(dead_code)]
    pub ip: String,
    /// `tart run` child process. Killed on shutdown.
    vm_child: Child,
    /// PTY-wrapped SSH session running `claude` inside the guest.
    pty: PtySession,
}

pub struct SpawnSpec<'a> {
    pub agent_id: &'a str,
    pub vm_name: &'a str,
    pub base_image: &'a str,
    pub worktree: PathBuf,
    pub task: &'a str,
    pub key_path: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

impl Agent {
    /// Full spawn flow: clone the VM, start it, wait for SSH, mount the
    /// worktree, launch `claude` over an SSH PTY.
    ///
    /// `on_progress` is called between stages with a short human-readable
    /// message ("Cloning VM image", "Waiting for SSH", …). Used by the
    /// supervisor to surface what's happening while the agent is still in
    /// the Spawning state.
    pub async fn spawn<F, P>(
        vm: Arc<Vm>,
        spec: SpawnSpec<'_>,
        on_output: F,
        on_progress: P,
    ) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
        P: Fn(&str) + Send + Sync + 'static,
    {
        // 1. Clone the base image into a fresh per-agent VM (APFS CoW, fast).
        on_progress("Cloning base image (APFS CoW, fast)…");
        vm.clone_image(spec.base_image, spec.vm_name).await?;

        // 2. Boot it with the worktree mounted as a virtiofs share. `tart run`
        //    is long-running; we keep the child handle so we can kill it
        //    later. Drain stdout+stderr immediately — a full pipe buffer
        //    silently blocks the child.
        let mount = MountSpec {
            name: SHARE_TAG,
            path: &spec.worktree,
            readonly: false,
        };
        on_progress("Booting VM…");
        let mut vm_child = vm.run_detached(spec.vm_name, &[mount]).await?;
        drain_child_io(&mut vm_child);

        // 3. Wait for the VM to come up and report an IP, then for sshd to
        //    actually start listening (these are separate events — sshd
        //    starts a beat after networking does). On any failure here we
        //    tear down the half-baked VM so a retry doesn't trip over a
        //    name collision.
        on_progress("Waiting for VM network…");
        let ip = match vm.wait_for_ip(spec.vm_name, VM_BOOT_TIMEOUT).await {
            Ok(ip) => ip,
            Err(e) => {
                let _ = vm.stop(spec.vm_name).await;
                let _ = vm.delete(spec.vm_name).await;
                return Err(e);
            }
        };

        on_progress(&format!("VM at {ip}. Waiting for SSH (port 22)…"));
        let ip = match wait_for_ssh_port_with_reip(
            &vm,
            spec.vm_name,
            ip,
            &on_progress,
            SSH_PORT_TIMEOUT,
        )
        .await
        {
            Ok(ip) => ip,
            Err(e) => {
                // Collect host-side diagnostics before reporting. We
                // deliberately do NOT delete the VM here — the user needs
                // to be able to SSH in manually to figure out what's
                // going on inside the guest. They can clean it up via
                // the Remove button when they're done.
                let last_ip = vm
                    .try_ip(spec.vm_name)
                    .await
                    .unwrap_or(None)
                    .unwrap_or_else(|| "<unknown>".into());
                let diag = diagnose_unreachable(&last_ip).await;
                return Err(crate::error::Error::Ssh(format!(
                    "{e}\n\nDiagnostics for {last_ip}:\n{diag}\n\
                     The VM '{}' is still running so you can inspect it:\n\
                     • ssh admin@{last_ip}  (password is no longer 'admin' — use the algiers SSH key)\n\
                     • Click Remove on this agent to clean it up.",
                    spec.vm_name
                )));
            }
        };

        // 4. Mount the share inside the guest, then start claude under a PTY.
        //    Combining both into one remote command keeps spawn atomic from
        //    the host's perspective.
        on_progress("Mounting worktree and launching claude…");
        let remote_cmd = build_remote_cmd(spec.task);

        let pty = PtySession::spawn_ssh(
            SshSpawn {
                target: SshTarget {
                    user: GUEST_USER,
                    host: &ip,
                    key_path: &spec.key_path,
                    port: None,
                },
                remote_cmd: &remote_cmd,
                cols: spec.cols,
                rows: spec.rows,
            },
            on_output,
        )?;

        Ok(Self {
            id: spec.agent_id.to_string(),
            vm_name: spec.vm_name.to_string(),
            worktree: spec.worktree,
            ip,
            vm_child,
            pty,
        })
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        self.pty.write(bytes)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.pty.resize(cols, rows)
    }

    /// Tear down: kill PTY, stop VM, delete the per-agent VM image. Worktree
    /// is left in place so the user can inspect/commit work.
    pub async fn shutdown(mut self, vm: Arc<Vm>) -> Result<()> {
        let _ = self.pty.kill();
        let _ = self.vm_child.start_kill();
        let _ = vm.stop(&self.vm_name).await;
        vm.delete(&self.vm_name).await?;
        Ok(())
    }
}

/// Poll TCP port 22 on the guest until sshd accepts a connection, or fail
/// after `timeout`. Each `connect` attempt is wrapped in a short timeout —
/// without it, macOS's kernel SYN-retry budget (~75s) would freeze the
/// outer retry loop on the first attempt.
///
/// Also periodically re-queries `tart ip` so that if the VM rebooted
/// during cloud-init and got a new DHCP lease, we follow it to the new
/// address instead of hammering the dead one. Returns the IP that
/// eventually accepted the connection.
async fn wait_for_ssh_port_with_reip(
    vm: &Vm,
    image_name: &str,
    initial_ip: String,
    on_progress: &(impl Fn(&str) + Send + 'static),
    total: Duration,
) -> Result<String> {
    let deadline = Instant::now() + total;
    let mut current_ip = initial_ip;
    let mut last_err: Option<String> = None;
    let mut last_reip_at = Instant::now();
    let _ = &last_err;
    loop {
        match timeout(
            SSH_CONNECT_ATTEMPT_TIMEOUT,
            tokio::net::TcpStream::connect((current_ip.as_str(), 22)),
        )
        .await
        {
            Ok(Ok(_)) => return Ok(current_ip),
            Ok(Err(e)) => last_err = Some(format!("connect: {e}")),
            Err(_) => last_err = Some("connect: timed out".into()),
        }

        if Instant::now() >= deadline {
            return Err(Error::Ssh(format!(
                "SSH did not start listening on {current_ip}:22 within {}s (last: {})",
                total.as_secs(),
                last_err.as_deref().unwrap_or("?")
            )));
        }

        // If we're seeing "No route to host" or repeated timeouts, the
        // VM might have rebooted and the IP we have is stale. Re-query
        // every `REIP_INTERVAL` and follow if it changed.
        if last_reip_at.elapsed() >= REIP_INTERVAL {
            last_reip_at = Instant::now();
            if let Ok(Some(new_ip)) = vm.try_ip(image_name).await {
                if new_ip != current_ip {
                    on_progress(&format!(
                        "VM IP changed: {current_ip} → {new_ip}. Retrying SSH there…"
                    ));
                    current_ip = new_ip;
                }
            }
        }

        sleep(Duration::from_secs(1)).await;
    }
}

/// Best-effort network diagnostics for "VM is unreachable" errors. Runs
/// `ping` + `arp -n` on the host and returns a short report. All errors
/// are folded into the report itself — this function never fails so it
/// can be safely chained into an error builder.
async fn diagnose_unreachable(ip: &str) -> String {
    use tokio::process::Command;
    let mut out = String::new();

    // ICMP ping — distinguishes "VM down" from "VM up but sshd off".
    match Command::new("ping")
        .args(["-c", "2", "-W", "1500", ip])
        .output()
        .await
    {
        Ok(o) if o.status.success() => {
            out.push_str("  ping: ✓ VM responds to ICMP (it's alive, just no sshd)\n");
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let stdout = String::from_utf8_lossy(&o.stdout);
            out.push_str(&format!(
                "  ping: ✗ VM does not respond to ICMP — {}\n",
                stderr.trim().is_empty().then(|| stdout.trim()).unwrap_or(stderr.trim())
            ));
        }
        Err(e) => out.push_str(&format!("  ping: could not run ({e})\n")),
    }

    // ARP table — does the host even know how to reach this IP?
    match Command::new("arp").args(["-n", ip]).output().await {
        Ok(o) => {
            let line = String::from_utf8_lossy(&o.stdout);
            let line = line.trim();
            if line.is_empty() {
                out.push_str("  arp: (no entry — host can't resolve VM MAC at all)\n");
            } else {
                out.push_str(&format!("  arp: {line}\n"));
            }
        }
        Err(e) => out.push_str(&format!("  arp: could not run ({e})\n")),
    }

    out
}

/// Drain stdout/stderr of a long-running child so the pipe buffers don't
/// fill and freeze the process. We don't surface the output anywhere for
/// agent spawns (the bake reports it to the UI; for spawns it's not
/// useful enough to plumb through), just discard.
fn drain_child_io(child: &mut Child) {
    if let Some(out) = child.stdout.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(_)) = lines.next_line().await {}
        });
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "tart_run", "{line}");
            }
        });
    }
}

/// Build the shell command we run inside the guest. Two things happen:
/// 1. Mount the virtiofs share (idempotent — succeeds if already mounted).
/// 2. Exec `claude` with the task prompt in yolo mode.
fn build_remote_cmd(task: &str) -> String {
    // Shell-escape the task for single-quoted embedding.
    let escaped_task = task.replace('\'', "'\\''");
    format!(
        "set -e; \
         mountpoint -q {ws} || sudo mount -t virtiofs {tag} {ws}; \
         cd {ws}; \
         exec claude --dangerously-skip-permissions '{task}'",
        ws = GUEST_WORKSPACE,
        tag = SHARE_TAG,
        task = escaped_task,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_task_prompt() {
        let cmd = build_remote_cmd("it's a test");
        assert!(cmd.contains("'it'\\''s a test'"));
    }

    #[test]
    fn mounts_share_then_execs_claude() {
        let cmd = build_remote_cmd("hello");
        assert!(cmd.contains("mount -t virtiofs"));
        assert!(cmd.contains("exec claude --dangerously-skip-permissions"));
    }
}

//! Per-agent lifecycle.
//!
//! An `Agent` is the live, in-memory counterpart of an `AgentRecord`. It owns
//! the running `tart run` child process and the SSH PTY session pumping bytes
//! to the frontend.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Child;

use crate::error::Result;
use crate::pty_bridge::{PtySession, SshSpawn, SshTarget};
use crate::vm::{MountSpec, Vm};

/// How long to wait for a freshly cloned VM to acquire an IP before we give
/// up and tear it down.
const VM_BOOT_TIMEOUT: Duration = Duration::from_secs(60);

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
    pub async fn spawn<F>(vm: Arc<Vm>, spec: SpawnSpec<'_>, on_output: F) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
    {
        // 1. Clone the base image into a fresh per-agent VM (APFS CoW, fast).
        vm.clone_image(spec.base_image, spec.vm_name).await?;

        // 2. Boot it with the worktree mounted as a virtiofs share. `tart run`
        //    is long-running; we keep the child handle so we can kill it later.
        let mount = MountSpec {
            name: SHARE_TAG,
            path: &spec.worktree,
            readonly: false,
        };
        let vm_child = vm.run_detached(spec.vm_name, &[mount]).await?;

        // 3. Wait for the VM to come up and report an IP.
        let ip = match vm.wait_for_ip(spec.vm_name, VM_BOOT_TIMEOUT).await {
            Ok(ip) => ip,
            Err(e) => {
                // Boot failed — try to clean up the half-baked VM so a retry
                // doesn't trip over a name collision.
                let _ = vm.stop(spec.vm_name).await;
                let _ = vm.delete(spec.vm_name).await;
                return Err(e);
            }
        };

        // 4. Mount the share inside the guest, then start claude under a PTY.
        //    Combining both into one remote command keeps spawn atomic from
        //    the host's perspective.
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

//! In-app "build the base VM image" flow.
//!
//! End-to-end: clones the upstream Ubuntu image, boots it, SSHes in with the
//! default `admin`/`admin` password via [`russh`] (no `sshpass`/`expect`
//! needed — we want the user to never touch a terminal), runs the install
//! script that bakes in node + claude code CLI + our public key + the
//! virtiofs mountpoint, then powers the VM down cleanly. Progress events
//! are streamed through `on_progress` so the UI can show what's happening.

use async_trait::async_trait;
use russh::client;
use russh::keys::key::PublicKey;
use russh::ChannelMsg;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};

use crate::error::{Error, Result};
use crate::vm::Vm;

const UPSTREAM_IMAGE: &str = "ghcr.io/cirruslabs/ubuntu:latest";
const GUEST_USER: &str = "admin";
const INITIAL_PASSWORD: &str = "admin";
const SSH_READY_TIMEOUT: Duration = Duration::from_secs(120);
const SSH_TRY_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BakeStage {
    Cloning,
    Booting,
    WaitingForSsh,
    Installing,
    Finalizing,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct BakeProgress {
    pub stage: BakeStage,
    pub message: String,
    /// Best-effort tail of the most recent install output. Useful when the
    /// install stalls and the user wants to see what's happening.
    pub tail: Option<String>,
}

pub struct BakeSpec<'a> {
    pub image_name: &'a str,
    pub public_key_path: &'a Path,
}

/// Drive the whole bake. The VM is left in `stopped` state ready to be
/// cloned by [`crate::agent::Agent::spawn`].
pub async fn bake_base_image<F>(
    vm: Arc<Vm>,
    spec: BakeSpec<'_>,
    on_progress: F,
) -> Result<()>
where
    F: Fn(BakeProgress) + Send + Sync + 'static,
{
    let report = |stage: BakeStage, message: &str, tail: Option<&str>| {
        on_progress(BakeProgress {
            stage,
            message: message.to_string(),
            tail: tail.map(|s| s.to_string()),
        });
    };

    let res = bake_inner(&vm, &spec, &report).await;
    if let Err(e) = &res {
        report(BakeStage::Error, &e.to_string(), None);
        // Best-effort cleanup so the user can retry without a stale VM
        // lying around.
        let _ = vm.stop(spec.image_name).await;
        let _ = vm.delete(spec.image_name).await;
    } else {
        report(BakeStage::Done, "Base image ready", None);
    }
    res
}

async fn bake_inner<R>(vm: &Vm, spec: &BakeSpec<'_>, report: &R) -> Result<()>
where
    R: Fn(BakeStage, &str, Option<&str>),
{
    let public_key = std::fs::read_to_string(spec.public_key_path)?
        .trim()
        .to_string();
    if public_key.is_empty() {
        return Err(Error::Other(
            "host public key file is empty — check ~/Library/Application Support/com.algiers.app/"
                .into(),
        ));
    }

    // 1. Clone the upstream image. This is the long step — it downloads ~1–2 GB.
    report(
        BakeStage::Cloning,
        "Downloading Ubuntu base image (1–2 GB, may take several minutes)…",
        None,
    );
    vm.clone_image(UPSTREAM_IMAGE, spec.image_name).await?;

    // 2. Boot it. Keep the child handle — we kill it via `tart stop` later
    //    rather than killing the process directly (gives the guest a clean
    //    shutdown).
    report(BakeStage::Booting, "Booting the VM…", None);
    let mut vm_child = vm.run_detached(spec.image_name, &[]).await?;

    // 3. Wait for the VM to get an IP and for SSH to actually accept
    //    connections (which is slightly later than IP assignment).
    report(BakeStage::WaitingForSsh, "Waiting for SSH to come up…", None);
    let ip = vm
        .wait_for_ip(spec.image_name, SSH_READY_TIMEOUT)
        .await?;
    wait_for_ssh_port(&ip, SSH_READY_TIMEOUT).await?;

    // 4. SSH in with the upstream default password and run the install
    //    script. Output lines are forwarded to the progress callback so the
    //    UI can show a live tail.
    report(BakeStage::Installing, "Installing node, claude code, SSH key, sudoers…", None);
    let script = install_script(&public_key);
    let exit = run_setup_over_ssh(&ip, &script, |line| {
        report(BakeStage::Installing, "Installing…", Some(line));
    })
    .await?;
    if exit != 0 {
        return Err(Error::Other(format!(
            "install script exited with status {exit}"
        )));
    }

    // 5. Stop the VM cleanly. The `tart run` child will exit on its own once
    //    the guest powers off; we wait for it so the user can re-clone
    //    immediately after.
    report(BakeStage::Finalizing, "Shutting down the VM…", None);
    vm.stop(spec.image_name).await?;
    // 30s ceiling — `tart stop --timeout 10` already nudges the guest.
    let _ = timeout(Duration::from_secs(30), vm_child.wait()).await;

    Ok(())
}

/// Bash script that turns a stock cirruslabs Ubuntu image into a usable
/// base for algiers agents. Designed to be idempotent enough that re-runs
/// don't break (so a retry after a partial failure is safe).
fn install_script(public_key: &str) -> String {
    // Single-quote-escape the public key. Public keys are usually one line
    // with no quotes, but be defensive.
    let escaped_key = public_key.replace('\'', "'\\''");
    format!(
        r#"
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive

echo '[1/6] Updating apt indexes'
sudo apt-get update -y -qq

echo '[2/6] Installing core packages'
sudo apt-get install -y -qq curl git ca-certificates build-essential

echo '[3/6] Installing Node.js 20'
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash - >/dev/null
sudo apt-get install -y -qq nodejs

echo '[4/6] Installing Claude Code CLI'
sudo npm install -g @anthropic-ai/claude-code

echo '[5/6] Baking in host SSH key + sudoers'
mkdir -p ~/.ssh && chmod 700 ~/.ssh
KEY='{escaped_key}'
grep -qxF "$KEY" ~/.ssh/authorized_keys 2>/dev/null || echo "$KEY" >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys

echo 'admin ALL=(ALL) NOPASSWD: /usr/bin/mount, /usr/bin/umount' | \
  sudo tee /etc/sudoers.d/algiers >/dev/null
sudo chmod 440 /etc/sudoers.d/algiers

echo '[6/6] Pre-creating /workspace mount point'
sudo mkdir -p /workspace
sudo chown admin:admin /workspace

echo 'BAKE_COMPLETE'
"#,
        escaped_key = escaped_key,
    )
}

async fn wait_for_ssh_port(ip: &str, total: Duration) -> Result<()> {
    let deadline = std::time::Instant::now() + total;
    loop {
        match tokio::net::TcpStream::connect((ip, 22)).await {
            Ok(_) => return Ok(()),
            Err(_) if std::time::Instant::now() < deadline => sleep(SSH_TRY_INTERVAL).await,
            Err(_) => return Err(Error::VmBootTimeout(total.as_secs())),
        }
    }
}

struct AcceptAnyServerKey;

#[async_trait]
impl client::Handler for AcceptAnyServerKey {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        // We just booted this VM ourselves. Pinning the host key would
        // require us to scrape it out of the guest first; not worth the
        // complexity for a freshly-cloned local VM we control.
        Ok(true)
    }
}

async fn run_setup_over_ssh<F>(ip: &str, script: &str, on_line: F) -> Result<i32>
where
    F: Fn(&str),
{
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(300)),
        ..Default::default()
    });

    let mut session = client::connect(config, (ip, 22), AcceptAnyServerKey)
        .await
        .map_err(|e| Error::Ssh(format!("connect: {e}")))?;

    let authed = session
        .authenticate_password(GUEST_USER, INITIAL_PASSWORD)
        .await
        .map_err(|e| Error::Ssh(format!("auth: {e}")))?;
    if !authed {
        return Err(Error::Ssh(
            "password auth rejected — has the upstream image changed defaults?".into(),
        ));
    }

    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| Error::Ssh(format!("open channel: {e}")))?;
    channel
        .exec(true, script.as_bytes())
        .await
        .map_err(|e| Error::Ssh(format!("exec: {e}")))?;

    // Pump output. We buffer until newline so progress callbacks get whole
    // lines, which is much more useful for a tail UI.
    let mut buf: Vec<u8> = Vec::new();
    let mut exit_status: i32 = -1;
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => {
                buf.extend_from_slice(&data);
                flush_lines(&mut buf, &on_line);
            }
            ChannelMsg::ExtendedData { data, .. } => {
                buf.extend_from_slice(&data);
                flush_lines(&mut buf, &on_line);
            }
            ChannelMsg::ExitStatus { exit_status: s } => {
                exit_status = s as i32;
            }
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }
    if !buf.is_empty() {
        on_line(&String::from_utf8_lossy(&buf));
    }

    Ok(exit_status)
}

fn flush_lines<F: Fn(&str)>(buf: &mut Vec<u8>, on_line: &F) {
    while let Some(idx) = buf.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = buf.drain(..=idx).collect();
        let line = String::from_utf8_lossy(&line[..line.len() - 1]);
        let trimmed = line.trim_end_matches('\r');
        if !trimmed.is_empty() {
            on_line(trimmed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_script_embeds_public_key() {
        let s = install_script("ssh-ed25519 AAAA dev@host");
        assert!(s.contains("ssh-ed25519 AAAA dev@host"));
        // Must single-quote into bash and escape any quotes safely.
        assert!(s.contains("KEY='ssh-ed25519 AAAA dev@host'"));
    }

    #[test]
    fn install_script_escapes_single_quotes() {
        let s = install_script("ssh-ed25519 AAAA who's-key");
        assert!(s.contains("who'\\''s-key"));
    }

    #[test]
    fn flush_lines_splits_on_newline() {
        let mut buf: Vec<u8> = b"hello\nworld\npart".to_vec();
        let collected = std::sync::Mutex::new(Vec::<String>::new());
        flush_lines(&mut buf, &|s| collected.lock().unwrap().push(s.to_string()));
        assert_eq!(*collected.lock().unwrap(), vec!["hello", "world"]);
        assert_eq!(buf, b"part");
    }
}

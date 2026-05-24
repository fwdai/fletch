//! In-app "build the base VM image" flow.
//!
//! End-to-end: clones the upstream Ubuntu image, boots it, SSHes in with the
//! default `admin`/`admin` password via [`russh`] (no `sshpass`/`expect`
//! needed — we want the user to never touch a terminal), runs the install
//! script that bakes in node + claude code CLI + our public key + the
//! virtiofs mountpoint, then powers the VM down cleanly. Progress events
//! are streamed through `on_progress` so the UI can show what's happening.

use async_trait::async_trait;
use parking_lot::Mutex;
use russh::client;
use russh::keys::key::PublicKey;
use russh::ChannelMsg;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::time::{sleep, timeout};

use crate::error::{Error, Result};
use crate::vm::{DiskAttach, Vm};

/// Pinned to an explicit LTS tag instead of `:latest` because `:latest` has
/// been observed to have unreliable first-boot behavior (sshd starting and
/// stopping multiple times during cloud-init, transient firewall states).
/// If 24.04 is also problematic, we'll need to generate a cidata ISO and
/// attach via `--disk` so cloud-init runs our config first.
const UPSTREAM_IMAGE: &str = "ghcr.io/cirruslabs/ubuntu:24.04";
const GUEST_USER: &str = "admin";
const INITIAL_PASSWORD: &str = "admin";
/// Total budget for the VM to acquire an IP. First boots can take a while
/// because cloud-init runs synchronously before networking comes up.
const IP_READY_TIMEOUT: Duration = Duration::from_secs(180);
/// Total budget for SSH to start listening after the VM has an IP.
///
/// First boot of a freshly-cloned cirruslabs Ubuntu `:latest` image is
/// agonizingly slow — empirically 3–5 minutes from IP-up to sshd
/// listening. During that window we see a mix of `Connection refused`
/// (sshd not started yet) and `No route to host` (transient firewall
/// state during cloud-init's network module). 8 minutes gives enough
/// headroom for a worst-case first boot.
const SSH_READY_TIMEOUT: Duration = Duration::from_secs(480);
/// Hard cap on each individual `TcpStream::connect` attempt. The macOS
/// kernel's SYN retry budget is 75+ seconds, which silently freezes the
/// retry loop if we don't cut it short.
const SSH_CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
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
        // Don't auto-delete on failure. The VM may have made progress
        // (e.g. successfully booted, just not in time for our SSH wait),
        // and a retry can salvage it via the pre-clean logic at the top
        // of bake_inner. Auto-deleting forces the user to redo the
        // expensive 1–2GB upstream download every time.
        //
        // We still stop the VM to free up RAM, but leave the image on
        // disk so retry can re-run it without re-downloading.
        let _ = vm.stop(spec.image_name).await;
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

    // 0. If a previous bake left this VM behind (e.g. timed out during the
    //    SSH wait but the disk image is intact), reuse it — saves the 1–2 GB
    //    upstream download. We just need to start the VM and (re-)try the
    //    SSH wait. If the VM doesn't exist, fall through to a fresh clone.
    let reused = vm.exists(spec.image_name).await.unwrap_or(false);
    if reused {
        report(
            BakeStage::Cloning,
            &format!(
                "Reusing existing '{}' VM from a previous bake attempt (skipping 1–2 GB download)…",
                spec.image_name
            ),
            None,
        );
        // Make sure it's stopped before we re-launch it (idempotent).
        let _ = vm.stop(spec.image_name).await;
    } else {
        // 1. Clone the upstream image. This is the long step — it downloads
        //    ~1–2 GB.
        report(
            BakeStage::Cloning,
            "Downloading Ubuntu base image (1–2 GB, may take several minutes)…",
            None,
        );
        vm.clone_image(UPSTREAM_IMAGE, spec.image_name).await?;
    }

    // 1.5 Build a NoCloud cidata ISO with user-data that forces cloud-init
    //     to disable UFW, ensure sshd is running, and pre-add our key on
    //     first boot. Without this, cirruslabs's default cloud-init flow
    //     takes 3–5 minutes (and occasionally never opens port 22 at all
    //     because of transient firewall state).
    report(
        BakeStage::Booting,
        "Generating cloud-init seed (cidata.iso)…",
        None,
    );
    let cidata_dir = tempfile::tempdir()
        .map_err(|e| Error::Other(format!("tempdir for cidata: {e}")))?;
    let cidata_iso = create_cidata_iso(&public_key, cidata_dir.path()).await?;

    // 2. Boot it. Keep the child handle — we kill it via `tart stop` later
    //    rather than killing the process directly (gives the guest a clean
    //    shutdown). Drain stderr/stdout in background tasks so the pipe
    //    buffers never fill up (which would silently freeze tart), and so
    //    we can surface tart's error message if it dies on us.
    report(BakeStage::Booting, "Booting the VM (with cidata attached)…", None);
    let mut vm_child = vm
        .run_detached_with(
            spec.image_name,
            &[],
            &[DiskAttach {
                path: &cidata_iso,
                readonly: true,
            }],
        )
        .await?;
    let vm_stderr_tail = spawn_drain(&mut vm_child);
    // Hold the tempdir until after the VM is stopped so the ISO file stays
    // valid for the duration of the boot.
    let _cidata_dir_guard = cidata_dir;

    // 3. Wait for the VM to get an IP. Poll our own loop instead of
    //    `Vm::wait_for_ip` so we can also check whether the `tart run`
    //    child has died and surface its stderr to the UI.
    report(
        BakeStage::WaitingForSsh,
        "Waiting for VM to acquire a network address…",
        None,
    );
    let ip = wait_for_ip_with_progress(
        vm,
        spec.image_name,
        &mut vm_child,
        &vm_stderr_tail,
        IP_READY_TIMEOUT,
        report,
    )
    .await?;

    report(
        BakeStage::WaitingForSsh,
        &format!("VM reached {ip}. Waiting for SSH on port 22…"),
        None,
    );
    wait_for_ssh_port_with_progress(
        &ip,
        spec.image_name,
        &mut vm_child,
        &vm_stderr_tail,
        SSH_READY_TIMEOUT,
        report,
    )
    .await?;

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

/// Generate a NoCloud cloud-init seed ISO. cloud-init in the guest auto-
/// detects ISOs labeled `cidata` (or `CIDATA`) attached at boot time and
/// applies the user-data inside, which we use to force-disable any
/// firewall and ensure sshd is running.
///
/// Cloud-init phases (relevant for our config):
///   - `bootcmd` runs in `cloud-init-local.service`, before networking.
///   - `runcmd`  runs in `cloud-final.service`, after `cloud-config`.
///
/// We use both — bootcmd to set up sshd/firewall as early as possible,
/// runcmd as belt-and-suspenders in case `bootcmd` was raced.
async fn create_cidata_iso(public_key: &str, dir: &Path) -> Result<PathBuf> {
    let staging = dir.join("seed");
    std::fs::create_dir_all(&staging)?;

    let escaped_key = public_key.replace('"', r#"\""#);

    let user_data = format!(
        r#"#cloud-config
# Generated by algiers — applied on first boot via NoCloud cidata ISO.

bootcmd:
  - [ sh, -xc, "ufw --force disable || true" ]
  - [ sh, -xc, "systemctl stop ufw 2>/dev/null || true" ]
  - [ sh, -xc, "systemctl disable ufw 2>/dev/null || true" ]
  - [ sh, -xc, "iptables -F INPUT 2>/dev/null || true" ]
  - [ sh, -xc, "iptables -P INPUT ACCEPT 2>/dev/null || true" ]
  - [ sh, -xc, "systemctl enable ssh 2>/dev/null || true" ]
  - [ sh, -xc, "systemctl start ssh 2>/dev/null || true" ]

users:
  - name: admin
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    lock_passwd: false
    ssh_authorized_keys:
      - "{escaped_key}"

runcmd:
  - [ sh, -xc, "ufw --force disable || true" ]
  - [ sh, -xc, "systemctl restart ssh || true" ]
"#
    );

    let meta_data = "instance-id: algiers-base\nlocal-hostname: algiers-base\n";

    std::fs::write(staging.join("user-data"), user_data)?;
    std::fs::write(staging.join("meta-data"), meta_data)?;

    let iso_path = dir.join("cidata.iso");
    let out = tokio::process::Command::new("hdiutil")
        .arg("makehybrid")
        .arg("-iso")
        .arg("-joliet")
        .arg("-default-volume-name")
        .arg("CIDATA")
        .arg("-o")
        .arg(&iso_path)
        .arg(&staging)
        .output()
        .await
        .map_err(|e| Error::Other(format!("hdiutil exec: {e}")))?;

    if !out.status.success() {
        return Err(Error::Other(format!(
            "hdiutil makehybrid failed (status {}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    Ok(iso_path)
}

/// Bash script that turns a stock cirruslabs Ubuntu image into a usable
/// base for algiers agents. Designed to be idempotent enough that re-runs
/// don't break (so a retry after a partial failure is safe).
///
/// Critical for cloned VMs:
/// - Disables cloud-init from re-running on first boot of a clone. Without
///   this, cloud-init detects the new instance-id and runs end-to-end
///   again on every clone — regenerating SSH host keys, occasionally
///   re-enabling ufw, and generally taking 2–5 min during which port 22
///   may be firewalled or sshd may be temporarily stopped.
/// - Disables ufw outright. Defensive: cloud-init's default config on
///   recent cirruslabs images can leave it enabled with rules that send
///   ICMP Destination Unreachable for unauthorized inbound traffic, which
///   the host sees as `No route to host` on `connect()`.
fn install_script(public_key: &str) -> String {
    // Single-quote-escape the public key. Public keys are usually one line
    // with no quotes, but be defensive.
    let escaped_key = public_key.replace('\'', "'\\''");
    format!(
        r#"
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive

echo '[1/8] Updating apt indexes'
sudo apt-get update -y -qq

echo '[2/8] Installing core packages'
sudo apt-get install -y -qq curl git ca-certificates build-essential

echo '[3/8] Installing Node.js 20'
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash - >/dev/null
sudo apt-get install -y -qq nodejs

echo '[4/8] Installing Claude Code CLI'
sudo npm install -g @anthropic-ai/claude-code

echo '[5/8] Baking in host SSH key + sudoers'
mkdir -p ~/.ssh && chmod 700 ~/.ssh
KEY='{escaped_key}'
grep -qxF "$KEY" ~/.ssh/authorized_keys 2>/dev/null || echo "$KEY" >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys

echo 'admin ALL=(ALL) NOPASSWD: /usr/bin/mount, /usr/bin/umount' | \
  sudo tee /etc/sudoers.d/algiers >/dev/null
sudo chmod 440 /etc/sudoers.d/algiers

echo '[6/8] Pre-creating /workspace mount point'
sudo mkdir -p /workspace
sudo chown admin:admin /workspace

echo '[7/8] Hardening: disabling firewall (clones occasionally inherit a blocking ufw config)'
sudo ufw --force disable 2>/dev/null || true
sudo systemctl stop ufw 2>/dev/null || true
sudo systemctl disable ufw 2>/dev/null || true
sudo iptables -F INPUT 2>/dev/null || true
sudo iptables -P INPUT ACCEPT 2>/dev/null || true
# Make sure sshd is enabled and running.
sudo systemctl enable ssh 2>/dev/null || true
sudo systemctl start ssh 2>/dev/null || true

echo '[8/8] Disabling cloud-init re-runs on cloned VMs'
# Cloud-init detects "new instance" on every clone (different instance-id)
# and re-runs all modules — regenerating SSH host keys, possibly re-
# enabling firewall, etc. Since we've completed setup, lock it out.
sudo touch /etc/cloud/cloud-init.disabled
# Belt-and-suspenders: also mask cloud-init's services so they can't
# spuriously activate.
sudo systemctl mask cloud-init.service 2>/dev/null || true
sudo systemctl mask cloud-init-local.service 2>/dev/null || true
sudo systemctl mask cloud-config.service 2>/dev/null || true
sudo systemctl mask cloud-final.service 2>/dev/null || true

echo 'BAKE_COMPLETE'
"#,
        escaped_key = escaped_key,
    )
}

/// Poll for SSH (TCP 22) to start accepting on the guest. Each `connect`
/// attempt is wrapped in [`SSH_CONNECT_ATTEMPT_TIMEOUT`] because macOS's
/// kernel-level SYN retry budget would otherwise leave each failed attempt
/// hanging for ~75 seconds — silently freezing the outer retry loop.
async fn wait_for_ssh_port_with_progress<R>(
    ip: &str,
    image_name: &str,
    vm_child: &mut Child,
    stderr_tail: &StderrTail,
    total: Duration,
    report: &R,
) -> Result<()>
where
    R: Fn(BakeStage, &str, Option<&str>),
{
    let deadline = Instant::now() + total;
    let mut attempt: u32 = 0;
    let mut last_err: Option<String> = None;
    let _ = &last_err;
    loop {
        if let Some(status) = vm_child.try_wait()? {
            let tail = stderr_tail.lock().join("\n");
            return Err(Error::Other(format!(
                "`tart run {image_name}` died while we were waiting for SSH (status: {status}). \
                 Recent stderr:\n{}",
                if tail.is_empty() { "(no output)" } else { &tail }
            )));
        }

        match timeout(
            SSH_CONNECT_ATTEMPT_TIMEOUT,
            tokio::net::TcpStream::connect((ip, 22)),
        )
        .await
        {
            Ok(Ok(_)) => return Ok(()),
            Ok(Err(e)) => last_err = Some(format!("connect: {e}")),
            Err(_) => last_err = Some("connect: timed out".into()),
        }

        if Instant::now() >= deadline {
            return Err(Error::Other(format!(
                "Timed out after {}s waiting for SSH on {ip}:22. Last error: {}",
                total.as_secs(),
                last_err.as_deref().unwrap_or("(none)")
            )));
        }

        attempt += 1;
        if attempt % 5 == 0 {
            let elapsed = Instant::now()
                .saturating_duration_since(deadline - total)
                .as_secs();
            // Tell the user *why* we're still waiting with a one-line
            // network diagnosis. ping ✓ means the VM is alive and we're
            // just waiting on sshd; arp empty means the bridge can't
            // route at all; "No route to host" means the guest is
            // ICMP-rejecting (firewall).
            let diag = diagnose(ip).await;
            let detail = match last_err.as_deref() {
                Some(e) if e.contains("Connection refused") => {
                    "VM is reachable; ssh.service not yet listening (cloud-init regenerates host keys on first boot — usually 2–3 min)".to_string()
                }
                Some(e) if e.contains("No route") => {
                    format!("{diag}\nVM is responding but its kernel is rejecting port 22 — likely a firewall on the guest. Waiting for it to come down…")
                }
                _ => format!("{diag}\nLast: {}", last_err.as_deref().unwrap_or("?")),
            };
            report(
                BakeStage::WaitingForSsh,
                &format!("Still waiting for SSH on {ip} ({elapsed}s elapsed)…"),
                Some(&detail),
            );
        }

        sleep(SSH_TRY_INTERVAL).await;
    }
}

/// One-line network diagnosis used in heartbeats. Best-effort — never
/// fails, just reports what it sees.
async fn diagnose(ip: &str) -> String {
    use tokio::process::Command;
    let ping_ok = Command::new("ping")
        .args(["-c", "1", "-W", "1000", ip])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    let arp_line = Command::new("arp")
        .args(["-n", ip])
        .output()
        .await
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    let arp_state = match arp_line {
        Some(l) if l.contains("incomplete") || l.contains("no entry") => "arp ✗",
        Some(_) => "arp ✓",
        None => "arp ?",
    };
    format!("ping {} · {}", if ping_ok { "✓" } else { "✗" }, arp_state)
}

/// Tail of stderr from a long-running child process. We capture it so that
/// if the child dies the bake can show a useful message instead of just
/// timing out.
type StderrTail = Arc<Mutex<Vec<String>>>;

const STDERR_TAIL_LINES: usize = 50;

/// Take `Child::stderr` and `Child::stdout`, spawn background tasks that
/// drain them line by line. Returns a shared buffer containing the last
/// `STDERR_TAIL_LINES` lines of stderr.
fn spawn_drain(child: &mut Child) -> StderrTail {
    let tail: StderrTail = Arc::new(Mutex::new(Vec::new()));

    if let Some(stderr) = child.stderr.take() {
        let tail = tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut g = tail.lock();
                g.push(line);
                if g.len() > STDERR_TAIL_LINES {
                    let drop_n = g.len() - STDERR_TAIL_LINES;
                    g.drain(..drop_n);
                }
            }
        });
    }
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(async move {
            // Discard tart's stdout — it doesn't write anything useful here
            // and we just need the pipe drained so it doesn't block.
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(_)) = lines.next_line().await {}
        });
    }

    tail
}

/// Poll for the VM's IP, but also watch the `tart run` child for early
/// exit. Emits periodic "still waiting" progress so the dialog isn't
/// frozen-looking.
async fn wait_for_ip_with_progress<R>(
    vm: &Vm,
    image_name: &str,
    vm_child: &mut Child,
    stderr_tail: &StderrTail,
    total: Duration,
    report: &R,
) -> Result<String>
where
    R: Fn(BakeStage, &str, Option<&str>),
{
    let deadline = Instant::now() + total;
    let mut attempt: u32 = 0;
    loop {
        if let Some(status) = vm_child.try_wait()? {
            let tail = stderr_tail.lock().join("\n");
            return Err(Error::Other(format!(
                "`tart run {image_name}` exited unexpectedly (status: {status}). \
                 Recent stderr:\n{}",
                if tail.is_empty() { "(no output)" } else { &tail }
            )));
        }

        if let Some(ip) = vm.try_ip(image_name).await? {
            return Ok(ip);
        }

        if Instant::now() >= deadline {
            let tail = stderr_tail.lock().join("\n");
            return Err(Error::Other(format!(
                "Timed out after {}s waiting for VM '{image_name}' to acquire an IP. \
                 If `tart` printed anything:\n{}",
                total.as_secs(),
                if tail.is_empty() { "(no output)" } else { &tail }
            )));
        }

        attempt += 1;
        // Every ~10s of waiting, emit a heartbeat so the UI shows progress.
        if attempt % 5 == 0 {
            let elapsed = total.saturating_sub(deadline.saturating_duration_since(Instant::now()));
            let tail = stderr_tail.lock();
            let last_line = tail.last().cloned();
            drop(tail);
            report(
                BakeStage::WaitingForSsh,
                &format!(
                    "Still waiting for VM IP ({}s elapsed)…",
                    elapsed.as_secs()
                ),
                last_line.as_deref(),
            );
        }

        sleep(Duration::from_secs(2)).await;
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

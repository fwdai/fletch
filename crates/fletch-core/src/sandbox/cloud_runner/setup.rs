//! What is done on the runner over ssh, once its machine runs: the set-up of
//! a new runner ([`configure`]), what is asked of one that is set up
//! ([`wind_down`], [`pairing`]), and the clean end of its service before its
//! machine is stopped ([`end_service`]).
//!
//! The set-up puts on the disk, in order: `fletch-host` (the release of this
//! engine's version for the guest's architecture, downloaded by the guest and
//! checked against the release's `.sha256`, which the engine fetches itself;
//! or the binary [`HOST_BINARY_ENV`] names, pushed), the Claude CLI at the
//! version this engine launches, the env file holding the Claude token, and
//! the start script the image's entrypoint runs. Then it stops and starts the
//! machine, so the service comes up the way it will at every start, and over
//! its admin socket it gives the runner this machine's provider connection,
//! authorises the calling device, runs the runner's GitHub sign-in and reads
//! the runner's identity.
//!
//! The admin socket is reached through `fletch-host call <op>`, run over the
//! channel with the op's JSON arguments on stdin and its JSON result on
//! stdout ([`admin`]): one way for every op, and no secret in a command line.
//!
//! No ssh step here has a test against a real guest. The pieces that are text
//! are tested on their own: the start script and [`END_SERVICE`] run under a
//! local shell with a stand-in service, the env file, the release's address
//! and digest.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fletch_box::channel::{self, shell_quote, Channel};
use fletch_box::guest;
use serde_json::{json, Value};

use super::{
    bring_up, Machine, Progress, RunnerPairing, RunnerRecord, DATA_DIR, HOME, HOST_BINARY_ENV,
};
use crate::cloud::{self, Client, Handle, ProviderId};
use crate::error::{Error, Result};
use crate::sandbox::boxes::cloud as boxes_cloud;
use crate::sandbox::boxes::{registry, CloudBoxesDestroyed};

/// `fletch-host` on the runner's disk, with the rest of its release tarball
/// beside it (the box runtime, which `fletch-host update` keeps current).
pub(super) const HOST_BIN: &str = "/data/host/fletch-host";

/// The runner's credentials, owner-only: `KEY=value` lines its start script
/// exports into the service's environment.
pub(super) const ENV_FILE: &str = "/data/host/env";

/// The variable claude reads an OAuth token from; the engine forwards it to
/// every box launch as the login (`sandbox::provider::auth`).
const CLAUDE_TOKEN_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// The agent CLI the runner has on its disk, so its engine reports it
/// installed and its workspaces get it at its version.
const CLI: &str = "claude";

/// This engine's version: the `fletch-host` release a runner gets.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How long fetching the release's checksum may take.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the service may take to answer once the machine runs: the
/// entrypoint starts it, and a first start makes its database.
const SERVICE_BOUND: Duration = Duration::from_secs(120);
const SERVICE_PACE: Duration = Duration::from_secs(3);

/// How often the runner is asked whether the GitHub code was entered.
const GITHUB_POLL: Duration = Duration::from_secs(3);

/// How long the start script waits before starting an exited service again.
const RESTART_PAUSE_SECS: u32 = 5;

/// The running start script's pid, written by the script when it starts and
/// removed when it ends: what a stop signals ([`end_service`]).
pub(super) const PID_FILE: &str = "/data/host/start.pid";

/// How long a stop waits for the service to end once its start script was
/// told to: `fletch-host`'s shutdown stops each running workspace machine
/// (its lock, its sessions pull and its halt each bounded at ten seconds,
/// the machines side by side), and the bound leaves room past that.
const SERVICE_END_SECS: u32 = 90;

/// More than [`SERVICE_END_SECS`] for the whole ssh command, so a guest that
/// stops answering mid-way does not hold the stop up.
const SERVICE_END_SLACK: Duration = Duration::from_secs(30);

/// Ends the runner's service and syncs the disk:
/// `sh -c END_SERVICE sh <pid file> <start script> <seconds>`. The start
/// script whose pid the file holds (and only while that pid is a process
/// running the script, as `pgrep -f` finds it: a stale pid is not signalled)
/// is sent TERM and waited for, at most `<seconds>`; the disk is synced
/// either way. Exits 0 when the service ended, 3 when none was running, 4
/// when it did not end in time.
pub(super) const END_SERVICE: &str = r#"set -u
pidfile=$1 script=$2 bound=$3
pid=$(cat "$pidfile" 2>/dev/null || true)
ended=3
if [ -n "$pid" ] && pgrep -f -- "$script" | grep -qx -- "$pid"; then
  kill -TERM "$pid" 2>/dev/null || true
  waited=0
  ended=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge "$bound" ]; then
      ended=4
      break
    fi
    sleep 1
    waited=$((waited + 1))
  done
fi
sync
exit "$ended"
"#;

/// Downloads a release tarball into a directory, checks it against a digest
/// and unpacks it there: `sh -c DOWNLOAD sh <url> <sha256> <dir>`.
const DOWNLOAD: &str = r#"set -eu
url=$1 digest=$2 dir=$3
tarball=$dir/fletch-host.tar.gz
curl -fsSL --retry 3 -o "$tarball" "$url"
if ! printf '%s  %s\n' "$digest" "$tarball" | sha256sum -c --status -; then
  rm -f "$tarball"
  echo "the download of $url does not match its release's checksum" >&2
  exit 1
fi
tar -xzf "$tarball" -C "$dir"
rm -f "$tarball"
"#;

/// What [`configure`] gives the runner, besides its machine. No `Debug`: the
/// provider connection's token and the Claude token are in it.
pub(super) struct Setup {
    pub connection: cloud::Connection,
    pub relay: String,
    pub name: String,
    pub device_public_key: String,
    pub device_name: Option<String>,
    pub device_platform: Option<String>,
    pub claude_token: String,
}

/// Set up the runner on `machine` with `setup`, telling `progress` each step;
/// answers its record. Everything is written to its disk before the machine
/// is stopped and started, so the service comes up through the image's
/// entrypoint as it will at every start.
pub(super) async fn configure(
    machine: Machine,
    setup: &Setup,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<RunnerRecord> {
    let key = machine.dir.join(channel::KEY_FILE);
    progress(Progress::Connecting);
    let ssh = boxes_cloud::reach(&machine.dir, &key, &machine.handle).await?;
    let written = async {
        install_host(&ssh, progress).await?;
        install_cli(&ssh, progress).await?;
        progress(Progress::Configuring);
        ssh.put(
            env_file(&setup.claude_token).as_bytes(),
            Path::new(ENV_FILE),
            0o600,
        )
        .await
        .map_err(io_error)?;
        ssh.put(
            start_script(&setup.relay, &setup.name).as_bytes(),
            Path::new(guest::CLOUD_HOST_SERVICE),
            0o755,
        )
        .await
        .map_err(io_error)?;
        run(&ssh, &["sync"]).await.map(drop)
    }
    .await;
    close(&ssh).await;
    written?;

    progress(Progress::StartingService);
    machine.client.stop(&machine.handle).await?;
    let handle =
        boxes_cloud::machine_up(&machine.client, &machine.dir, &machine.spec, &|_| {}).await?;
    let ssh = boxes_cloud::reach(&machine.dir, &key, &handle).await?;
    let record = async {
        wait_for_service(&ssh).await?;
        progress(Progress::ConnectingProvider);
        admin(
            &ssh,
            "cloud_connect",
            &json!({
                "provider": setup.connection.provider().as_str(),
                "token": setup.connection.token(),
                "account": setup.connection.account(),
            }),
        )
        .await?;
        progress(Progress::AuthorizingDevice);
        admin(
            &ssh,
            "add_device",
            &json!({
                "publicKey": setup.device_public_key,
                "name": setup.device_name,
                "platform": setup.device_platform,
                "preset": "full",
            }),
        )
        .await?;
        github_login(&ssh, progress).await?;
        identity(&ssh).await
    }
    .await;
    close(&ssh).await;
    record
}

/// Have the runner in `dir` destroy every machine it made at its providers,
/// its spares and its workspaces' boxes, and make no spare after (its
/// `destroy_cloud_boxes` op), starting it first when it is stopped; answers
/// what it did.
pub(super) async fn wind_down(
    dir: &Path,
    client_for: &impl Fn(ProviderId) -> Result<Client>,
) -> Result<CloudBoxesDestroyed> {
    let handle = boxes_cloud::read_handle(dir)?.ok_or_else(super::no_runner)?;
    let client = Arc::new(client_for(handle.provider())?);
    let handle = bring_up(dir, &client, &handle).await?;
    let ssh = boxes_cloud::reach(dir, &dir.join(channel::KEY_FILE), &handle).await?;
    let outcome = async {
        wait_for_service(&ssh).await?;
        let answer = admin(&ssh, "destroy_cloud_boxes", &json!({})).await?;
        serde_json::from_value(answer).map_err(|e| {
            Error::Other(format!(
                "the runner's account of its workspaces' machines did not parse: {e}"
            ))
        })
    }
    .await;
    close(&ssh).await;
    outcome
}

/// A pairing code from the running runner whose machine `handle` names, with
/// the access `preset` names.
pub(super) async fn pairing(
    dir: &Path,
    handle: &Handle,
    preset: Option<&str>,
) -> Result<RunnerPairing> {
    let ssh = boxes_cloud::reach(dir, &dir.join(channel::KEY_FILE), handle).await?;
    let answer = admin(&ssh, "begin_pairing", &json!({ "preset": preset })).await;
    close(&ssh).await;
    serde_json::from_value(answer?)
        .map_err(|e| Error::Other(format!("the runner's pairing code did not parse: {e}")))
}

/// Put `fletch-host` in the runner's directory on its disk, which is made
/// first, the agent's alone: the release of [`VERSION`] for the guest's
/// architecture, or the binary [`HOST_BINARY_ENV`] names. Checked by running
/// it.
async fn install_host(ssh: &Channel, progress: &(dyn Fn(Progress) + Sync)) -> Result<()> {
    run(
        ssh,
        &[
            "sudo",
            "-n",
            "install",
            "-d",
            "-o",
            guest::USER,
            "-g",
            guest::USER,
            "-m",
            "700",
            guest::CLOUD_HOST_DIR,
        ],
    )
    .await?;
    match std::env::var_os(HOST_BINARY_ENV).filter(|path| !path.is_empty()) {
        Some(path) => {
            progress(Progress::InstallingHost { version: None });
            let binary = std::fs::read(&path).map_err(|e| {
                Error::Other(format!(
                    "read the fletch-host {HOST_BINARY_ENV} names ({}): {e}",
                    Path::new(&path).display()
                ))
            })?;
            ssh.put(&binary, Path::new(HOST_BIN), 0o755)
                .await
                .map_err(io_error)?;
        }
        None => {
            let target = guest_target(ssh).await?;
            progress(Progress::InstallingHost {
                version: Some(VERSION.to_string()),
            });
            let url = release_url(VERSION, target);
            let digest = fetch_digest(&format!("{url}.sha256")).await?;
            run(
                ssh,
                &[
                    "sh",
                    "-c",
                    DOWNLOAD,
                    "sh",
                    &url,
                    &digest,
                    guest::CLOUD_HOST_DIR,
                ],
            )
            .await?;
        }
    }
    let version = run(ssh, &[HOST_BIN, "--version"]).await?;
    tracing::info!(
        version = version.trim(),
        "fletch-host installed on the cloud runner"
    );
    Ok(())
}

/// The Claude CLI on the runner's disk at the version this engine launches
/// (`registry::tools_version`), where the entrypoint binds the CLIs and the
/// start script finds them: a host with no agent CLI reports its provider not
/// installed, and its clients start no agent of it.
async fn install_cli(ssh: &Channel, progress: &(dyn Fn(Progress) + Sync)) -> Result<()> {
    let version = registry::tools_version(CLI);
    let label = crate::agent::provider_bin_label(CLI).map_or(CLI, |(_, label)| label);
    progress(Progress::InstallingCli {
        cli: label.to_string(),
        version: version.clone(),
    });
    fletch_box::tools::install_cli(
        ssh,
        CLI,
        &version,
        Path::new(HOME),
        guest::Egress::Direct,
        |line| tracing::info!(target: "fletch::box_tools", line, "cloud runner CLI install"),
    )
    .await
    .map_err(|e| {
        Error::Other(format!(
            "installing {label} {version} on the runner failed: {e}"
        ))
    })
}

/// The Rust target the guest runs, as the release names its `fletch-host`
/// tarballs.
async fn guest_target(ssh: &Channel) -> Result<&'static str> {
    let arch = run(ssh, &["uname", "-m"]).await?;
    match arch.trim() {
        "x86_64" => Ok("x86_64-unknown-linux-gnu"),
        "aarch64" => Ok("aarch64-unknown-linux-gnu"),
        other => Err(Error::Other(format!(
            "the runner's machine is {other}, which no fletch-host release is built for"
        ))),
    }
}

/// The release tarball of `fletch-host` `version` for `target`, as the
/// release workflow publishes it (`.github/workflows/release.yml`, `host`).
pub(super) fn release_url(version: &str, target: &str) -> String {
    format!(
        "{}/v{version}/fletch-host-{version}-{target}.tar.gz",
        fletch_box::image_store::RELEASES_URL
    )
}

/// The digest a release's `.sha256` file at `url` holds, fetched by this
/// engine over HTTPS.
async fn fetch_digest(url: &str) -> Result<String> {
    let http = reqwest::Client::builder()
        .user_agent("Fletch")
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| Error::Other(format!("http client: {e}")))?;
    let response = http
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Other(format!("could not fetch {url}: {e}")))?;
    if !response.status().is_success() {
        return Err(Error::Other(format!(
            "{url} answered {}: there is no fletch-host release of this version of Fletch to put \
             on the runner (a development build has none; set {HOST_BINARY_ENV} to a fletch-host \
             built for the runner's machine)",
            response.status()
        )));
    }
    let text = response
        .text()
        .await
        .map_err(|e| Error::Other(format!("could not read {url}: {e}")))?;
    digest_of(&text)
}

/// The digest out of a `sha256sum` line: `<64 hex digits>  <file name>`.
pub(super) fn digest_of(text: &str) -> Result<String> {
    let digest = text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Other(
            "the release's .sha256 file does not start with a digest".into(),
        ));
    }
    Ok(digest)
}

/// `token` as the runner gets it, or why it cannot be one. Never quoted in an
/// error.
pub(super) fn check_token(token: &str) -> Result<String> {
    let token = token.trim();
    if token.is_empty() {
        return Err(Error::Other(
            "a Claude Code token is needed: run `claude setup-token` and paste what it prints"
                .into(),
        ));
    }
    if token.chars().any(char::is_control) {
        return Err(Error::Other(
            "the Claude Code token holds a line break or another control character; paste it \
             again"
                .into(),
        ));
    }
    Ok(token.to_string())
}

/// The env file's text: the Claude token, quoted for the shell that exports
/// it.
pub(super) fn env_file(claude_token: &str) -> String {
    format!("{CLAUDE_TOKEN_VAR}={}\n", shell_quote(claude_token))
}

/// The start script the image's entrypoint runs at every start of the
/// machine, as the agent, with an empty environment: the service's
/// environment (its home on the disk, the agent CLIs where the entrypoint
/// binds them, the credentials), then `fletch-host serve` with its data
/// directory on the disk, the relay `relay` and the name `name`, started again
/// whenever it exits on its own. The script keeps its pid in [`PID_FILE`]; a
/// TERM to it is passed to the service, whose own shutdown it waits for, and
/// then it ends instead of starting the service again ([`end_service`]). Its
/// flags live here, written by the engine, and not in the image: a machine
/// keeps the image it was made with.
pub(super) fn start_script(relay: &str, name: &str) -> String {
    format!(
        r#"#!/bin/sh
# The cloud runner's host service, written by the Fletch engine that set this
# machine up. The image's entrypoint runs it at every start of the machine, as
# the agent, with an empty environment; it runs fletch-host with its data on
# this disk, and starts it again whenever it exits on its own. Its pid is in
# {pidfile}: a TERM to it ends the service, which shuts down cleanly, and then
# the script, which starts nothing more.
export HOME={home} USER={user} LOGNAME={user} SHELL=/bin/bash LANG=C.UTF-8
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
for bin in {tools}/*/bin; do
  [ -d "$bin" ] && PATH=$bin:$PATH
done
export PATH
set -a
. {env}
set +a
cd "$HOME" || exit 1
stopping=
service=
trap 'stopping=1; [ -n "$service" ] && kill -TERM "$service" 2>/dev/null' TERM
echo $$ > {pidfile}
while [ -z "$stopping" ]; do
  {bin} serve --data-dir {data} --relay {relay} --name {name} &
  service=$!
  status=0
  while kill -0 "$service" 2>/dev/null; do
    [ -n "$stopping" ] && kill -TERM "$service" 2>/dev/null
    wait "$service"
    status=$?
  done
  service=
  [ -n "$stopping" ] && break
  echo "fletch-host exited with status $status; starting it again in {pause} s" >&2
  sleep {pause}
done
rm -f {pidfile}
"#,
        home = HOME,
        user = guest::USER,
        tools = guest::TOOLS_MOUNT,
        env = ENV_FILE,
        pidfile = PID_FILE,
        bin = HOST_BIN,
        data = DATA_DIR,
        relay = shell_quote(relay),
        name = shell_quote(name),
        pause = RESTART_PAUSE_SECS,
    )
}

/// End the runner's service cleanly before its machine is stopped, over ssh:
/// its start script is sent TERM (`END_SERVICE`), which ends `fletch-host`
/// as a TERM does (its workspaces' machines stopped, its databases closed)
/// and then itself, and is waited for within a bound; the disk is synced
/// then. Ok when the service ended or none was running; an error when the
/// guest could not be reached or the service did not end in time.
pub(super) async fn end_service(dir: &Path, handle: &Handle) -> Result<()> {
    let ssh = boxes_cloud::reach(dir, &dir.join(channel::KEY_FILE), handle).await?;
    let bound = SERVICE_END_SECS.to_string();
    let ended = tokio::time::timeout(
        Duration::from_secs(u64::from(SERVICE_END_SECS)) + SERVICE_END_SLACK,
        ssh.exec(
            None,
            None,
            &[
                "sh",
                "-c",
                END_SERVICE,
                "sh",
                PID_FILE,
                guest::CLOUD_HOST_SERVICE,
                &bound,
            ],
        ),
    )
    .await;
    close(&ssh).await;
    match ended {
        Err(_) => Err(Error::Other(format!(
            "the runner did not answer within {} s while its service was ended",
            u64::from(SERVICE_END_SECS) + SERVICE_END_SLACK.as_secs()
        ))),
        Ok(Err(e)) => Err(io_error(e)),
        Ok(Ok(out)) => match out.status.code() {
            Some(0) => {
                tracing::info!("the cloud runner's service ended cleanly");
                Ok(())
            }
            Some(3) => {
                tracing::info!("the cloud runner had no service running to end");
                Ok(())
            }
            Some(4) => Err(Error::Other(format!(
                "the runner's service did not end within {SERVICE_END_SECS} s"
            ))),
            _ => Err(Error::Other(format!(
                "ending the runner's service failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))),
        },
    }
}

/// Wait until the runner's `fletch-host` answers on its admin socket. When it
/// does not within the bound, the error says whether the machine started the
/// service at all: an image without the entrypoint's hook never does.
async fn wait_for_service(ssh: &Channel) -> Result<()> {
    let deadline = Instant::now() + SERVICE_BOUND;
    loop {
        let why = match admin(ssh, "status", &json!({})).await {
            Ok(_) => return Ok(()),
            Err(e) => e.to_string(),
        };
        if Instant::now() >= deadline {
            // A bracket, so the pattern does not match pgrep's own command
            // line, nor the shell that runs it.
            let script = guest::CLOUD_HOST_SERVICE;
            let pattern = format!("[{}]{}", &script[..1], &script[1..]);
            let started = ssh
                .exec(None, None, &["pgrep", "-f", &pattern])
                .await
                .is_ok_and(|out| out.status.success());
            return Err(Error::SandboxUnavailable(if started {
                format!(
                    "the runner's fletch-host did not answer within {} s: {why}",
                    SERVICE_BOUND.as_secs()
                )
            } else {
                format!(
                    "the runner's machine did not start its host service ({script}): its cloud \
                     image has no hook for it. A machine keeps the image it was made with; new \
                     ones are made with {}",
                    boxes_cloud::image()
                )
            }));
        }
        tokio::time::sleep(SERVICE_PACE).await;
    }
}

/// Run the runner's GitHub sign-in (the device flow, on the runner): tell
/// `progress` the code, then wait until it was entered, failed or lapsed.
async fn github_login(ssh: &Channel, progress: &(dyn Fn(Progress) + Sync)) -> Result<()> {
    let started = admin(ssh, "github_login_start", &json!({})).await?;
    let expires_in = started["expiresIn"].as_u64().unwrap_or(900);
    progress(Progress::GithubLogin {
        user_code: text(&started, "userCode"),
        verification_url: text(&started, "verificationUrl"),
        expires_in,
    });
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    loop {
        tokio::time::sleep(GITHUB_POLL).await;
        let status = admin(ssh, "github_login_status", &json!({})).await?;
        match status["state"].as_str() {
            Some("complete") => return Ok(()),
            Some("failed") => {
                return Err(Error::Other(format!(
                    "the runner's GitHub sign-in failed: {}",
                    text(&status, "error")
                )))
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(Error::Other(
                "the GitHub code lapsed before it was entered".into(),
            ));
        }
    }
}

/// The runner's identity, as its `fletch-host` reports itself.
async fn identity(ssh: &Channel) -> Result<RunnerRecord> {
    let status = admin(ssh, "status", &json!({})).await?;
    let host_id = text(&status, "hostId");
    if host_id.is_empty() {
        return Err(Error::Other(format!(
            "the runner has no host key: {}",
            text(&status, "error")
        )));
    }
    Ok(RunnerRecord {
        host_id,
        name: text(&status, "name"),
        relay: status["relay"]["url"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    })
}

/// Ask the runner's admin socket `op` with `args` (`fletch-host call`), the
/// arguments on the command's stdin, never in its command line; answers the
/// op's result, or the runner's refusal.
async fn admin(ssh: &Channel, op: &str, args: &Value) -> Result<Value> {
    let out = ssh
        .exec_with_input(
            &[HOST_BIN, "--data-dir", DATA_DIR, "call", op],
            args.to_string().as_bytes(),
        )
        .await
        .map_err(|e| Error::Other(format!("ask the runner for {op}: {e}")))?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "the runner did not do {op}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| Error::Other(format!("the runner's answer to {op} is not JSON: {e}")))
}

/// Run `argv` in the guest; its stdout, or an error with its stderr.
async fn run(ssh: &Channel, argv: &[&str]) -> Result<String> {
    let out = ssh.exec(None, None, argv).await.map_err(io_error)?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "{} on the runner failed ({}): {}",
            argv.first().copied().unwrap_or_default(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// End the channel's ssh master: nothing of this machine's stays connected to
/// the runner once a step is done with it.
async fn close(ssh: &Channel) {
    if let Err(e) = ssh.close_master().await {
        tracing::debug!(error = %e, "ending the cloud runner's ssh master failed");
    }
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

fn io_error(e: fletch_box::Error) -> Error {
    Error::Other(e.to_string())
}

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
//!
//! The command is project code run the moment the user clicks ▶, so it gets
//! the view the host's Run profile gives such code, in the guest
//! ([`run_view`]): the agent's home read-only whole — the provider's state,
//! which a `postinstall` could otherwise seed with a hook or an MCP server the
//! agent then runs, stays as the agent left it — with the checkout, the
//! project's toolchain cache and the scratch and toolchain dirs a build
//! writes outside its tree given back. The view also hides `/data`, where
//! every launch's env file lies, the agent's own with its access token among
//! them. What it does not hide: a credential file a provider keeps in the
//! home for the agent's launch (codex's launch `auth.json`, for one), which
//! the Run reads as any process the agent itself starts does; hiding those
//! needs an option the guest does not have yet.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fletch_box::channel::Forward;
use fletch_box::Channel;

use super::registry::{BoxIdentity, BoxRegistry, RunningBox};
use super::{box_holding, executor, launch_files, BoxEngine, ConfinedView, Hold, ENV_DENY};
use crate::error::{Error, Result};
use crate::sandbox::KillHandle;

/// The dirs under the home a Run may write besides the checkout and the
/// project's toolchain cache: the Linux twin of the host Run profile's
/// scratch grants (`policy::agent_scratch_dirs`) and its toolchain dirs
/// (`seatbelt::RUN_TOOLCHAIN_DIRS`), for the build that ignores the cache
/// redirect and writes its default location. Made before the launch, since
/// the guest binds only what exists.
const RUN_WRITABLE_HOME_DIRS: &[&str] = &[
    ".cache",  // XDG cache root
    ".config", // whole config root, as the host Run gets it
    ".local",  // whole ~/.local incl. bin, share, state
    ".npm",    // npm's package cache
    ".cargo",  // Rust: registry, git checkouts, installed bins
    ".rustup", // Rust: downloaded toolchains
    "go",      // Go: GOPATH
    ".bun",    // Bun: global install cache
    ".bundle", // Bundler: config + cache
    ".gem",    // RubyGems: default gem home
    ".rbenv",  // rbenv: shims + installed Ruby versions
    ".rvm",    // rvm: alternative Ruby version manager
];

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

/// Makes each directory given, where it can, and prints those that exist
/// afterwards, one per line: what the Run's view may give back.
const MAKE_DIRS_SCRIPT: &str = "for d in \"$@\"; do mkdir -p -- \"$d\" 2>/dev/null; \
    [ -d \"$d\" ] && printf '%s\\n' \"$d\"; done; exit 0";

/// Ends the process group of the Run launch `$1` in the guest: the oldest
/// process whose arguments name the launch's env file is the `fletch-guest
/// exec` sshd started, the leader of the group every process of the run
/// is in — those in the confined pid namespace too, seen from outside it —
/// so one `TERM` to the group reaches them all, and a `KILL` follows for
/// what is still there after two seconds. The bracketed first character
/// keeps the pattern from matching this script's own shell. Run by bash:
/// the guest's `sh` is dash, whose `kill` takes neither `--` nor a group.
/// Exits 0 whether or not anything was left to end.
const END_SCRIPT: &str = "pid=$(pgrep -of -- \"[/]data/run/$1.env\") || exit 0; \
    pgid=$(ps -o pgid= -p \"$pid\" | tr -d ' ') || exit 0; [ -n \"$pgid\" ] || exit 0; \
    kill -TERM -- \"-$pgid\" 2>/dev/null || exit 0; \
    for i in 1 2 3 4 5 6 7 8 9 10; do kill -0 -- \"-$pgid\" 2>/dev/null || exit 0; sleep 0.2; done; \
    kill -KILL -- \"-$pgid\" 2>/dev/null; exit 0";

/// End the Run launch `launch_id`'s process group in the guest
/// ([`END_SCRIPT`]); best-effort, since the box may be going down with it.
pub(super) fn end_in_guest(channel: &Channel, launch_id: &str) {
    let words = ["bash", "-c", END_SCRIPT, "bash", launch_id];
    match executor::run(channel, &words, None) {
        Ok(out) if !out.status.success() => tracing::warn!(
            launch_id,
            status = %out.status,
            stderr = %String::from_utf8_lossy(&out.stderr).trim(),
            "ending the run in the guest"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(launch_id, error = %e, "ending the run in the guest"),
    }
}

/// A Run in a box, from the hold on the box to the command's launch.
pub struct GuestRun {
    identity: BoxIdentity,
    launch_id: String,
    hold: Option<Hold>,
    running: Arc<RunningBox>,
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
        let registry = BoxRegistry::global();
        let channel = registry.channel_for(&identity.agent_id)?;
        let running = registry.running(&identity.agent_id).ok_or_else(|| {
            Error::Other(format!(
                "box {} stopped as the run began",
                identity.agent_id
            ))
        })?;
        Ok(Some(Self {
            identity,
            launch_id,
            hold: Some(hold),
            running,
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
    /// `cwd` of the guest, under a tty, confined to the Run's view of the
    /// guest ([`run_view`]), with `env` on top of what every guest process
    /// gets (`guest_env`, the project's toolchain cache), and the kill handle
    /// that ends the launch: the env file removed, the forward dropped, the
    /// box released. The session the caller spawns must own the handle, or
    /// run it (`sandbox::KillGuard`). A guest that cannot confine the command
    /// refuses the run rather than run it with the agent's whole view.
    pub fn launch(
        mut self,
        cwd: &Path,
        cmd: &str,
        env: &[(String, String)],
    ) -> Result<(PathBuf, Vec<String>, KillHandle)> {
        let (cache_root, cache_env) =
            super::guest_toolchain_cache(&self.identity.home, &self.identity.project_id)?;
        let mut view = run_view(&self.identity.home, cwd, &cache_root);
        // Every dir the view gives back, made first, since the guest binds
        // only what exists; one the guest cannot make (the cache root under
        // a `~/.fletch` the guest's init owns, as before an agent launch) is
        // left out of the view, read-only like the rest of the home, and the
        // toolchains keep their defaults when it is the cache root.
        let dirs: Vec<String> = view
            .writable_dirs
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        let mut words: Vec<&str> = vec!["sh", "-c", MAKE_DIRS_SCRIPT, "sh"];
        words.extend(dirs.iter().map(String::as_str));
        let made = executor::run(&self.channel, &words, None)?;
        let present: Vec<PathBuf> = String::from_utf8_lossy(&made.stdout)
            .lines()
            .map(PathBuf::from)
            .collect();
        view.writable_dirs.retain(|d| present.contains(d));
        if !super::guest_confines(&self.running, &view)? {
            return Err(Error::SandboxUnavailable(
                "the box image predates the Run panel's confinement of project code; update \
                 the box image"
                    .into(),
            ));
        }
        let mut all = if present.contains(&cache_root) {
            cache_env
        } else {
            tracing::warn!(
                root = %cache_root.display(),
                "could not create the toolchain cache in the box; toolchains keep their defaults"
            );
            Vec::new()
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
        let args = confined_pty_args(
            &self.channel.config,
            cwd,
            &env_file,
            &view,
            &["bash", "-c", &script],
        );
        if let Some(forward) = self.forward.take() {
            BoxEngine::shared().park_forward(&self.launch_id, forward);
        }
        BoxEngine::shared().note_run(&self.launch_id);
        let kill = self
            .hold
            .take()
            .expect("the hold is kept until the launch")
            .into_kill();
        Ok((PathBuf::from(fletch_box::channel::SSH), args, kill))
    }
}

/// The Run's view of the guest: `home` read-only whole, with `cwd` (the
/// checkout), `cache_root` (the project's toolchain cache) and
/// [`RUN_WRITABLE_HOME_DIRS`] writable again — each only when it lies under
/// `home`, since a path outside it is not held read-only to begin with.
/// Parents first, as the guest binds them.
fn run_view(home: &Path, cwd: &Path, cache_root: &Path) -> ConfinedView {
    let mut writable_dirs: Vec<PathBuf> = RUN_WRITABLE_HOME_DIRS
        .iter()
        .map(|d| home.join(d))
        .collect();
    writable_dirs.push(cwd.to_path_buf());
    writable_dirs.push(cache_root.to_path_buf());
    writable_dirs.retain(|d| d.starts_with(home) && d != home);
    writable_dirs.sort_by_key(|d| (d.components().count(), d.clone()));
    writable_dirs.dedup();
    ConfinedView {
        read_only: vec![home.to_path_buf()],
        writable_dirs,
        cow_dirs: Vec::new(),
        cow_files: Vec::new(),
    }
}

/// The argv after `ssh` for the Run's PTY: `cmd` in `cwd` under
/// `fletch-guest exec` with `env_file` and `view`'s options, a tty on both
/// ends (`-tt`), over the box's shared connection unless the command is too
/// long for it (`executor::SHARED_CONNECTION_COMMAND_MAX`, as an agent
/// launch decides).
fn confined_pty_args(
    ssh_config: &Path,
    cwd: &Path,
    env_file: &Path,
    view: &ConfinedView,
    cmd: &[&str],
) -> Vec<String> {
    let remote = Channel::remote_command_confined(
        Some(cwd),
        Some(env_file),
        &view.read_only(),
        &view.writable(),
        &[],
        &[],
        cmd,
    )
    .join(" ");
    let mut args = vec!["-F".to_string(), ssh_config.to_string_lossy().into_owned()];
    if remote.len() > executor::SHARED_CONNECTION_COMMAND_MAX {
        args.extend(["-S".to_string(), "none".to_string()]);
    }
    args.extend([
        "-tt".to_string(),
        fletch_box::guest::SSH_HOST_ALIAS.to_string(),
        remote,
    ]);
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Run's view: the home read-only, the checkout, the cache root and
    /// the scratch and toolchain dirs writable, parents before children, and
    /// nothing outside the home (which is not held) listed.
    #[test]
    fn the_run_view_gives_back_the_checkout_cache_and_toolchain_dirs_under_the_home() {
        let home = Path::new("/Users/u");
        let cwd = Path::new("/Users/u/.fletch/workspaces/a1/repo");
        let cache = Path::new("/Users/u/.fletch/toolchain-cache/p1");
        let view = run_view(home, cwd, cache);
        assert_eq!(view.read_only, vec![home.to_path_buf()]);
        assert!(view.cow_dirs.is_empty() && view.cow_files.is_empty());
        let writable = view.writable();
        assert!(writable.contains(&cwd), "{writable:?}");
        assert!(writable.contains(&cache), "{writable:?}");
        for dir in RUN_WRITABLE_HOME_DIRS {
            assert!(writable.contains(&home.join(dir).as_path()), "{dir}");
        }
        assert!(!writable.contains(&home), "the home itself stays read-only");
        let depth = |p: &&Path| p.components().count();
        assert!(writable.windows(2).all(|w| depth(&w[0]) <= depth(&w[1])));

        // A checkout outside the home is not held, so not listed either.
        let elsewhere = run_view(home, Path::new("/srv/repo"), cache);
        assert!(!elsewhere.writable().contains(&Path::new("/srv/repo")));
    }

    /// The PTY argv: the confined `fletch-guest exec` with the env file, the
    /// view and the cwd, a tty on both ends, the shared connection for a
    /// command of ordinary length and one of its own past the limit.
    #[test]
    fn the_confined_pty_args_name_the_view_and_take_their_own_connection_when_long() {
        let home = Path::new("/Users/u");
        let cwd = Path::new("/Users/u/.fletch/workspaces/a1/repo");
        let view = run_view(home, cwd, Path::new("/Users/u/.fletch/toolchain-cache/p1"));
        let args = confined_pty_args(
            Path::new("/b/ssh_config"),
            cwd,
            Path::new("/data/run/l1.env"),
            &view,
            &["bash", "-c", "set +m; pnpm dev"],
        );
        assert_eq!(&args[..4], ["-F", "/b/ssh_config", "-tt", "box"]);
        let remote = args.last().unwrap();
        assert!(
            remote.starts_with("/opt/fletch/bin/fletch-guest exec --env-file /data/run/l1.env --read-only /Users/u --writable "),
            "{remote}"
        );
        assert!(remote
            .contains(" --cwd /Users/u/.fletch/workspaces/a1/repo -- bash -c 'set +m; pnpm dev'"));
        assert!(remote.len() <= executor::SHARED_CONNECTION_COMMAND_MAX);

        let long = "x".repeat(executor::SHARED_CONNECTION_COMMAND_MAX);
        let args = confined_pty_args(
            Path::new("/b/ssh_config"),
            cwd,
            Path::new("/data/run/l1.env"),
            &view,
            &["bash", "-c", &long],
        );
        assert_eq!(
            &args[..6],
            ["-F", "/b/ssh_config", "-S", "none", "-tt", "box"]
        );
    }

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

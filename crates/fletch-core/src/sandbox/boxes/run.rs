//! The Run panel's process in a box: the project's setup and dev commands
//! run in the guest, where the checkout is.
//!
//! A run is a launch of the box like an agent's session or the workspace
//! terminal: it holds the box against its idle stop from before it reaches
//! it ([`super::Hold`], booting a stopped box) until the session ends, and
//! one [`BoxEngine::kill`](super::BoxEngine) — run by the session's own kill,
//! by its drop when the process exits on its own, and at app exit — removes
//! the env file and detaches the launch, which arms the idle timer. The
//! command gets exactly the environment an agent launch gets (`fletch-guest
//! exec --env-file`), plus what the panel adds (the project's shared env, the
//! pinned `PORT`), and runs under a tty so the dev server prints as it does
//! on the host.
//!
//! Ports: the dev server binds its own port in the guest, whose loopback is
//! the VM's alone, so two workspaces on port 3000 never clash there. The
//! browser reaches it through the box's forward for that port, which comes
//! up once the server listens, as for anything else listening in the guest
//! ([`super::ports`]).
//!
//! The command is project code run the moment the user clicks ▶, so it gets
//! the view the host's Run profile gives such code, in the guest
//! ([`run_view`]): everything the box mounts of the user's read-only whole —
//! the agent's home, so the provider's state, which a `postinstall` could
//! otherwise seed with a hook or an MCP server the agent then runs, stays as
//! the agent left it; the agent's writable root and mailbox; and every folder
//! the user shared with the agent writable, which the host's Run profile
//! grants no more than this view does — with the checkout, the project's
//! toolchain cache and the scratch and toolchain dirs a build writes outside
//! its tree given back. The view also hides `/data`, where every launch's env
//! file lies, the agent's own with its access token among them. What it does
//! not hide: a credential file a provider keeps in the home for the agent's
//! launch (codex's launch `auth.json`, for one), which the Run reads as any
//! process the agent itself starts does; hiding those needs an option the
//! guest does not have yet.
//!
//! A command whose program the guest cannot find is refused by that name
//! before anything runs ([`GuestRun::launch`]): the image carries Node with
//! npm, pnpm and yarn through corepack, and Bun in the devtools layer, while
//! a detected `cargo run` or `bundle exec` names a toolchain the user
//! installs in the VM first. What a launch made in the guest before it
//! failed — its copy-on-write stores, its env file — goes with the
//! [`GuestRun`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fletch_box::Channel;

use super::registry::{BoxIdentity, BoxRegistry, RunningBox};
use super::{
    box_holding, executor, launch_files, BoxEngine, ConfinedView, Hold, COW_DIR, ENV_DENY,
};
use crate::error::{Error, Result};
use crate::sandbox::{KillHandle, ToolchainCache};

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

/// Whether `$1`, the program a Run command starts, is on the PATH, asked of
/// bash as the command's own shell would resolve it: a builtin or a keyword
/// counts, and a leading `~` is the home, as the shell expands it. Exits 3
/// when nothing answers to the name.
const PROGRAM_SCRIPT: &str = "case $1 in \"~\"*) set -- \"$HOME${1#\"~\"}\";; esac; \
    command -v -- \"$1\" >/dev/null 2>&1 || exit 3";

/// Ends the process group of the Run launch `$1` in the guest, then removes
/// `$2`, the launch's copy-on-write stores, if given. The oldest process
/// whose arguments name the launch's env file is the `fletch-guest exec`
/// sshd started, the leader of the group every process of the run is in —
/// those in the confined pid namespace too, seen from outside it — so one
/// `TERM` to the group reaches them all, and a `KILL` follows for what is
/// still there after two seconds. The bracketed first character keeps the
/// pattern from matching this script's own shell. Run by bash: the guest's
/// `sh` is dash, whose `kill` takes neither `--` nor a group. Exits 0
/// whether or not anything was left to end.
const END_SCRIPT: &str = "end() { pid=$(pgrep -of -- \"[/]data/run/$1.env\") || return 0; \
    pgid=$(ps -o pgid= -p \"$pid\" | tr -d ' ') || return 0; [ -n \"$pgid\" ] || return 0; \
    kill -TERM -- \"-$pgid\" 2>/dev/null || return 0; \
    for i in 1 2 3 4 5 6 7 8 9 10; do kill -0 -- \"-$pgid\" 2>/dev/null || return 0; sleep 0.2; done; \
    kill -KILL -- \"-$pgid\" 2>/dev/null; }; \
    end \"$1\"; [ -n \"$2\" ] && rm -rf -- \"$2\"; exit 0";

/// End the Run launch `launch_id`'s process group in the guest and remove
/// its copy-on-write `stores` ([`END_SCRIPT`]); best-effort, since the box
/// may be going down with it.
pub(super) fn end_in_guest(channel: &Channel, launch_id: &str, stores: Option<&Path>) {
    let stores = stores
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let words = ["bash", "-c", END_SCRIPT, "bash", launch_id, &stores];
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

/// A Run in a box, from the hold on the box to the command's launch. What the
/// launch has made in the guest goes with this value when it fails before
/// the engine owns the launch ([`Drop`]), as the hold does.
pub struct GuestRun {
    identity: BoxIdentity,
    launch_id: String,
    hold: Option<Hold>,
    running: Arc<RunningBox>,
    channel: Arc<Channel>,
    /// The launch's copy-on-write stores, from when the guest made them to
    /// when the engine took the launch ([`BoxEngine::note_run`]).
    stores: Option<PathBuf>,
    /// Whether the launch's env file is in the guest and not yet the
    /// engine's to remove.
    env_file_written: bool,
}

impl Drop for GuestRun {
    /// A launch that failed after it wrote to the guest leaves nothing there:
    /// its stores and its env file are removed as the engine's kill removes
    /// them for a launch that ran.
    fn drop(&mut self) {
        if let Some(stores) = self.stores.take() {
            end_in_guest(&self.channel, &self.launch_id, Some(&stores));
        }
        if std::mem::take(&mut self.env_file_written) {
            let stem = launch_files::stem(&self.launch_id);
            let stem = stem.to_string_lossy();
            let words = ["sh", "-c", launch_files::REMOVE, "sh", &stem];
            if let Err(e) = executor::run(&self.channel, &words, None) {
                tracing::warn!(launch_id = %self.launch_id, error = %e, "removing a failed run's env file");
            }
        }
    }
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
            stores: None,
            env_file_written: false,
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

    /// The host program and arguments that run the shell command `cmd` in
    /// `cwd` of the guest, under a tty, confined to the Run's view of the
    /// guest ([`run_view`]), with `env` on top of what every guest process
    /// gets (`guest_env`, the project's toolchain cache), and the kill handle
    /// that ends the launch: the command ended, the env file removed, the
    /// box released. The session the caller spawns must own the handle, or
    /// run it (`sandbox::KillGuard`). A guest that cannot confine the command
    /// refuses the run rather than run it with the agent's whole view, and a
    /// command whose program is not on the PATH its env gives it is refused
    /// by that name ([`Self::check_program`]). A refused launch leaves
    /// nothing in the guest ([`Drop`]).
    pub fn launch(
        mut self,
        cwd: &Path,
        cmd: &str,
        env: &[(String, String)],
    ) -> Result<(PathBuf, Vec<String>, KillHandle)> {
        let cache = ToolchainCache::in_guest(&self.identity.home, &self.identity.project_id)?;
        let stores = run_stores(&self.identity.writable_root, &self.launch_id);
        let mut view = run_view(&self.identity, cwd, &cache.root, &stores);
        // Every dir the view gives back or overlays, made first, since the
        // guest binds only what exists; one the guest cannot make (the cache
        // root under a `~/.fletch` the guest's init owns, as before an agent
        // launch) is left out of the view, read-only like the rest of the
        // home, and the toolchains keep their defaults when it is the cache
        // root. A provider's state dir that could not be made, or the stores
        // every overlay needs, takes the carve-out it lies in down with it:
        // read-only beats a dir a script could create and seed.
        let dirs: Vec<String> = view
            .writable_dirs
            .iter()
            .chain(view.cow_dirs.iter().map(|(dir, _)| dir))
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        let mut words: Vec<&str> = vec!["sh", "-c", MAKE_DIRS_SCRIPT, "sh"];
        words.extend(dirs.iter().map(String::as_str));
        let made = executor::run(&self.channel, &words, None)?;
        let present: Vec<PathBuf> = String::from_utf8_lossy(&made.stdout)
            .lines()
            .map(PathBuf::from)
            .collect();
        if present.contains(&stores) {
            self.stores = Some(stores.clone());
        }
        view.writable_dirs.retain(|d| present.contains(d));
        let unprotected: Vec<PathBuf> = view
            .cow_dirs
            .iter()
            .map(|(dir, _)| dir.clone())
            .filter(|dir| !present.contains(dir) || !present.contains(&stores))
            .collect();
        view.writable_dirs
            .retain(|w| !unprotected.iter().any(|dir| dir.starts_with(w)));
        view.cow_dirs.retain(|(dir, _)| !unprotected.contains(dir));
        let carved = |p: &Path| view.writable_dirs.iter().any(|w| super::inside(p, w));
        view.cow_files
            .retain(|(file, _)| carved(file) && present.contains(&stores));
        if !super::guest_confines(&self.running, &view)? {
            return Err(Error::SandboxUnavailable(
                "the box image predates the Run panel's confinement of project code; update \
                 the box image"
                    .into(),
            ));
        }
        let mut all = if present.contains(&cache.root) {
            cache.env
        } else {
            tracing::warn!(
                root = %cache.root.display(),
                "could not create the toolchain cache in the box; toolchains keep their defaults"
            );
            Vec::new()
        };
        all.extend_from_slice(env);
        let contents =
            super::render_env_file(&all, &self.identity.provider, &self.identity.home, ENV_DENY)?;
        let env_file = launch_files::env_file(&self.launch_id);
        executor::put(&self.channel, contents.as_bytes(), &env_file, 0o600)?;
        self.env_file_written = true;
        self.check_program(cwd, &env_file, cmd)?;
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
        // The engine owns the launch from here: its kill ends the command,
        // removes the stores (made or not) and the env file, and detaches.
        self.stores = None;
        self.env_file_written = false;
        BoxEngine::shared().note_run(&self.launch_id, stores);
        let kill = self
            .hold
            .take()
            .expect("the hold is kept until the launch")
            .into_kill();
        Ok((PathBuf::from(fletch_box::channel::SSH), args, kill))
    }

    /// Refuse `cmd` when the program it starts
    /// (`run_detect::command::program`) is not on the PATH `env_file` gives
    /// it — asked of the guest under that env file and in `cwd`, so the
    /// answer is the launch's own ([`PROGRAM_SCRIPT`]) — naming the program
    /// and what to do: the shell's `command not found`, after the box booted
    /// for it, would say less. A command the shell takes apart first is left
    /// to the shell.
    fn check_program(&self, cwd: &Path, env_file: &Path, cmd: &str) -> Result<()> {
        let Some(program) = crate::run_detect::command::program(cmd) else {
            return Ok(());
        };
        let options = Channel::exec_options(Some(cwd), Some(env_file), &[], &[], &[], &[]);
        let mut words: Vec<&str> = vec![fletch_box::guest::GUEST_AGENT, fletch_box::guest::EXEC];
        words.extend(options.iter().map(String::as_str));
        words.extend(["--", "bash", "-c", PROGRAM_SCRIPT, "bash", program]);
        let out = executor::run(&self.channel, &words, None)
            .map_err(|e| Error::Other(format!("look up `{program}` in the box: {e}")))?;
        match out.status.code() {
            Some(0) => Ok(()),
            Some(3) => Err(Error::Other(format!(
                "`{program}` is not installed in the Linux VM: nothing on its PATH is called \
                 that. Install it in the VM first (the workspace terminal and the agent can), \
                 or set a Run command the VM has in Project Settings › Run — a program outside \
                 the PATH by its full path."
            ))),
            _ => Err(Error::Other(format!(
                "look up `{program}` in the box: exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))),
        }
    }
}

/// Where the Run launch `launch_id`'s copy-on-write stores lie: under the
/// agent's own root, in [`COW_DIR`], a dir of this launch's own, removed
/// with the launch ([`END_SCRIPT`]).
fn run_stores(own_root: &Path, launch_id: &str) -> PathBuf {
    own_root.join(COW_DIR).join(format!("run-{launch_id}"))
}

/// The Run's view of the guest: everything the box mounts of the user's
/// read-only whole — `host`'s identity binds (its writable root, the whole
/// home, its mailbox) and every folder it shares writable, each with the
/// mounts below it; a read-only share is read-only as mounted — with `cwd`
/// (the checkout), `cache_root` (the project's toolchain cache), `stores`
/// (the launch's own stores) and [`RUN_WRITABLE_HOME_DIRS`] writable again,
/// each when it lies inside a read-only tree (one that does not is not held
/// to begin with), parents first, as the guest binds them; and, inside those,
/// every provider's shared state a private copy-on-write view, as a confined
/// tenant gets it: a toolchain writes where it likes under `~/.config` or
/// `~/.local`, while opencode's config and data dirs there take nothing a
/// project script writes, so no hook, MCP config or login of a provider is
/// the Run's to change. Provider state outside every carve-out (`~/.claude`,
/// `~/.codex`, `~/.cursor`, `~/.pi`, `~/.claude.json`) is read-only with the
/// rest of the home. A state dir that holds a mount (a share, a bind, a
/// carve-out) cannot be overlaid — an overlay's lower layer shows one
/// filesystem and would hide the mount — and takes the carve-out it lies in
/// down with it: read-only beats a dir a script could seed.
fn run_view(host: &BoxIdentity, cwd: &Path, cache_root: &Path, stores: &Path) -> ConfinedView {
    let home = host.home.as_path();
    let writable_shares = host
        .shares
        .iter()
        .filter(|share| share.writable)
        .map(|share| share.path.as_path());
    let binds = [&host.writable_root, &host.home, &host.rpc_dir].map(PathBuf::as_path);
    let read_only = super::outermost(binds.into_iter().chain(writable_shares));
    let protected = |path: &Path| read_only.iter().any(|dir| super::inside(path, dir));

    let own = RUN_WRITABLE_HOME_DIRS
        .iter()
        .map(|d| home.join(d))
        .chain([cwd, cache_root, stores].map(Path::to_path_buf));
    let mut writable_dirs: Vec<PathBuf> = own.filter(|dir| protected(dir)).collect();
    writable_dirs.sort_by_key(|d| (d.components().count(), d.clone()));
    writable_dirs.dedup();

    let (mut shared_dirs, mut shared_files) = (Vec::new(), Vec::new());
    for adapter in crate::sandbox::provider::adapters() {
        let state = adapter.shared_state(home);
        shared_dirs.extend(state.dirs);
        shared_files.extend(state.files);
    }
    let carved = |p: &Path, dirs: &[PathBuf]| dirs.iter().any(|w| super::inside(p, w));
    let mounts: Vec<&Path> = [&host.writable_root, &host.rpc_dir]
        .map(PathBuf::as_path)
        .into_iter()
        .chain(host.shares.iter().map(|share| share.path.as_path()))
        .chain(writable_dirs.iter().map(PathBuf::as_path))
        .collect();
    let holds_a_mount = |dir: &Path| mounts.iter().any(|mount| mount.starts_with(dir));
    let in_a_carve_out = shared_dirs
        .iter()
        .map(PathBuf::as_path)
        .filter(|&dir| carved(dir, &writable_dirs));
    let (blocked, overlaid): (Vec<PathBuf>, Vec<PathBuf>) = super::outermost(in_a_carve_out)
        .into_iter()
        .partition(|dir| holds_a_mount(dir));
    writable_dirs.retain(|w| !blocked.iter().any(|dir| dir.starts_with(w)));
    let cow_dirs: Vec<(PathBuf, PathBuf)> = overlaid
        .into_iter()
        .filter(|dir| carved(dir, &writable_dirs))
        .map(|dir| {
            let store = stores.join(super::cow_name(home, &dir));
            (dir, store)
        })
        .collect();
    let cow_files = shared_files
        .into_iter()
        .filter(|file| {
            carved(file, &writable_dirs) && !cow_dirs.iter().any(|(dir, _)| file.starts_with(dir))
        })
        .map(|file| {
            let copy = stores.join(super::cow_name(home, &file));
            (file, copy)
        })
        .collect();
    ConfinedView {
        read_only,
        writable_dirs,
        cow_dirs,
        cow_files,
        // Docker stays in sight: the Run command is what starts a project's
        // containers.
        hidden: Vec::new(),
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
        &view.cow_dirs(),
        &view.cow_files(),
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

    /// The identity of a box whose home is `home` and whose writable root is
    /// `root`, sharing `shares` (`(path, writable)`), with the mailbox under
    /// the home as the app keeps it.
    fn identity(home: &str, root: &str, shares: &[(&str, bool)]) -> BoxIdentity {
        BoxIdentity {
            agent_id: "a1".into(),
            project_id: "p1".into(),
            provider: "claude".into(),
            writable_root: PathBuf::from(root),
            rpc_dir: Path::new(home).join(".fletch/rpc/a1"),
            home: PathBuf::from(home),
            shares: shares
                .iter()
                .map(|(path, writable)| crate::workspace::FolderGrant {
                    path: PathBuf::from(path),
                    writable: *writable,
                })
                .collect(),
        }
    }

    /// The Run's view: the home read-only, the checkout, the cache root, the
    /// stores and the scratch and toolchain dirs writable, parents before
    /// children, nothing outside every held tree listed, and every
    /// provider's state inside a carve-out a private copy-on-write view
    /// with its store under the launch's own stores dir.
    #[test]
    fn the_run_view_gives_back_the_checkout_cache_and_toolchain_dirs_under_the_home() {
        let host = identity("/Users/u", "/Users/u/.fletch/workspaces/a1", &[]);
        let home = host.home.as_path();
        let root = host.writable_root.as_path();
        let cwd = root.join("repo");
        let cache = Path::new("/Users/u/.fletch/toolchain-cache/p1");
        let stores = run_stores(root, "l1");
        assert_eq!(stores, root.join(".fletch-cow").join("run-l1"));
        let view = run_view(&host, &cwd, cache, &stores);
        assert_eq!(
            view.read_only,
            vec![home.to_path_buf()],
            "the root and the mailbox lie inside the home"
        );
        let writable = view.writable();
        assert!(writable.contains(&cwd.as_path()), "{writable:?}");
        assert!(writable.contains(&cache), "{writable:?}");
        assert!(writable.contains(&stores.as_path()), "{writable:?}");
        for dir in RUN_WRITABLE_HOME_DIRS {
            assert!(writable.contains(&home.join(dir).as_path()), "{dir}");
        }
        assert!(!writable.contains(&home), "the home itself stays read-only");
        let depth = |p: &&Path| p.components().count();
        assert!(writable.windows(2).all(|w| depth(&w[0]) <= depth(&w[1])));

        // opencode keeps its config and data under `~/.config` and
        // `~/.local`, both carved out: each is overlaid, its store named
        // for it under the launch's stores. `~/.claude` and the like lie
        // outside every carve-out, read-only with the home, so not overlaid.
        let cow: Vec<(&Path, &Path)> = view.cow_dirs();
        assert!(
            cow.contains(&(
                Path::new("/Users/u/.config/opencode"),
                stores.join(".config%2Fopencode").as_path()
            )),
            "{cow:?}"
        );
        assert!(
            cow.contains(&(
                Path::new("/Users/u/.local/share/opencode"),
                stores.join(".local%2Fshare%2Fopencode").as_path()
            )),
            "{cow:?}"
        );
        assert!(cow
            .iter()
            .all(|(dir, _)| !dir.starts_with("/Users/u/.claude")));
        assert!(
            view.cow_files.is_empty(),
            "no provider keeps a state file inside a carve-out: {:?}",
            view.cow_files
        );
        assert!(view.hidden.is_empty(), "the Run reaches Docker");

        // A checkout outside every held tree is not held, so not listed
        // either.
        let elsewhere = run_view(&host, Path::new("/srv/repo"), cache, &stores);
        assert!(!elsewhere.writable().contains(&Path::new("/srv/repo")));
    }

    /// Every mount of the user's is read-only, not the home alone: a folder
    /// shared writable outside the home (an external disk), as the host's
    /// Run profile grants no share; a read-only share is read-only as
    /// mounted, and a share inside the home is covered by it. A writable
    /// root on another disk is read-only too, with its checkout and the
    /// launch's stores carved back out of it.
    #[test]
    fn the_run_view_holds_every_mount_of_the_users_read_only() {
        let cache = Path::new("/Users/u/.fletch/toolchain-cache/p1");
        let host = identity(
            "/Users/u",
            "/Users/u/.fletch/workspaces/a1",
            &[
                ("/Volumes/Data/assets", true),
                ("/Volumes/Data/reference", false),
                ("/Users/u/Documents/notes", true),
            ],
        );
        let stores = run_stores(&host.writable_root, "l1");
        let view = run_view(&host, &host.writable_root.join("repo"), cache, &stores);
        assert_eq!(
            view.read_only,
            vec![
                PathBuf::from("/Users/u"),
                PathBuf::from("/Volumes/Data/assets")
            ]
        );
        assert!(
            !view.writable().iter().any(|w| w.starts_with("/Volumes")),
            "{:?}",
            view.writable()
        );

        let host = identity("/Users/u", "/Volumes/Work/ws/a1", &[]);
        let cwd = host.writable_root.join("repo");
        let stores = run_stores(&host.writable_root, "l1");
        let view = run_view(&host, &cwd, cache, &stores);
        assert_eq!(
            view.read_only,
            vec![
                PathBuf::from("/Volumes/Work/ws/a1"),
                PathBuf::from("/Users/u")
            ]
        );
        assert!(view.writable().contains(&cwd.as_path()));
        assert!(view.writable().contains(&stores.as_path()));
        assert!(view.writable().contains(&Path::new("/Users/u/.cache")));
    }

    /// A provider's state dir that holds a mount cannot be overlaid, and
    /// read-only beats a dir a script could seed: the carve-out it lies in
    /// goes with it, while the other carve-outs and their overlays stay.
    #[test]
    fn a_state_dir_that_holds_a_mount_takes_its_carve_out_down() {
        let host = identity(
            "/Users/u",
            "/Users/u/.fletch/workspaces/a1",
            &[("/Users/u/.config/opencode/shared", true)],
        );
        let stores = run_stores(&host.writable_root, "l1");
        let view = run_view(
            &host,
            &host.writable_root.join("repo"),
            Path::new("/Users/u/.fletch/toolchain-cache/p1"),
            &stores,
        );
        let writable = view.writable();
        assert!(
            !writable.contains(&Path::new("/Users/u/.config")),
            "{writable:?}"
        );
        assert!(
            writable.contains(&Path::new("/Users/u/.local")),
            "{writable:?}"
        );
        let cow: Vec<&Path> = view.cow_dirs().into_iter().map(|(dir, _)| dir).collect();
        assert!(
            !cow.contains(&Path::new("/Users/u/.config/opencode")),
            "{cow:?}"
        );
        assert!(
            cow.contains(&Path::new("/Users/u/.local/share/opencode")),
            "{cow:?}"
        );
    }

    /// The program probe, as the launch's bash runs it: a program on the
    /// PATH, a builtin and a keyword answer, a `~` is the home, and a name
    /// nothing answers to exits 3.
    #[test]
    fn the_program_script_finds_what_the_shell_would_run() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("fletch-run-probe-{}", std::process::id()));
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("tool"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(bin.join("tool"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let run = |program: &str| {
            std::process::Command::new("bash")
                .env("HOME", &home)
                .args(["-c", PROGRAM_SCRIPT, "bash", program])
                .status()
                .unwrap()
                .code()
        };
        assert_eq!(run("ls"), Some(0));
        assert_eq!(run("cd"), Some(0), "a builtin");
        assert_eq!(run("if"), Some(0), "a keyword");
        assert_eq!(run("~/bin/tool"), Some(0), "the home, expanded");
        assert_eq!(run("no-such-program-for-fletch"), Some(3));
        assert_eq!(run("~/bin/none"), Some(3));
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// The PTY argv: the confined `fletch-guest exec` with the env file, the
    /// view and the cwd, a tty on both ends, the shared connection for a
    /// command of ordinary length and one of its own past the limit.
    #[test]
    fn the_confined_pty_args_name_the_view_and_take_their_own_connection_when_long() {
        let host = identity("/Users/u", "/Users/u/.fletch/workspaces/a1", &[]);
        let cwd = Path::new("/Users/u/.fletch/workspaces/a1/repo");
        let stores = run_stores(&host.writable_root, "l1");
        let view = run_view(
            &host,
            cwd,
            Path::new("/Users/u/.fletch/toolchain-cache/p1"),
            &stores,
        );
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
            remote.starts_with("/opt/fletch/bin/fletch-guest exec --env-file /data/run/l1.env --read-only /Users/u --writable ")
                && remote.contains(" --cow /Users/u/.config/opencode '/Users/u/.fletch/workspaces/a1/.fletch-cow/run-l1/.config%2Fopencode' "),
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

//! Running one VM: where the runtime's files are ([`RuntimePaths`]), what to
//! boot ([`VmSpec`]) and the `fletch-vm` child that boots it ([`Vm`]).

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::guest;

pub mod console;
pub mod launch;

pub use console::HostKey;
use launch::{Disk, LaunchConfig, RootDisk, VsockPort};

pub const HELPER: &str = "fletch-vm";
pub const LIBKRUN: &str = "libkrun.1.dylib";
pub const LIBKRUNFW: &str = "libkrunfw.5.dylib";
/// macOS `sun_path` is 104 bytes including the NUL.
pub const MAX_SOCKET_PATH: usize = 103;
/// How much console is kept in an error.
const CONSOLE_TAIL: usize = 4096;
const POLL: Duration = Duration::from_millis(25);

/// Where a dev checkout stages the runtime (`packaging/box-bundle/ensure-dev.sh`
/// via `prepare.sh`): the helper and both dylibs, side by side.
pub const DEV_OUT_DIR: &str = "packaging/box-bundle/out";
/// A file every checkout has (this crate's own manifest), so a dev build is
/// recognised before the runtime or its packaging exists and the probe can say
/// what to run.
const DEV_MARKER: &str = "crates/fletch-box/Cargo.toml";
/// What a dev checkout runs to stage the runtime into [`DEV_OUT_DIR`]; named in
/// the probe's message when the helper is missing.
const DEV_BUILD_HOOK: &str = "packaging/box-bundle/ensure-dev.sh";

/// Where the runtime's own files live on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    /// The `fletch-vm` binary, signed with the hypervisor entitlement.
    pub helper: PathBuf,
    /// Directory with the libkrun dylibs. `None`: the bundle's Frameworks folder.
    pub lib_dir: Option<PathBuf>,
    /// Default parent of box directories. Keep it short: socket paths live under it.
    pub run_root: PathBuf,
}

impl RuntimePaths {
    /// The helper inside the macOS app whose executable is `exe`
    /// (`<name>.app/Contents/MacOS/<exe>`): `Contents/Helpers/fletch-vm`, with
    /// libkrun in `Contents/Frameworks`, where the helper looks when `lib_dir` is `None`.
    pub fn bundled(exe: &Path, run_root: PathBuf) -> Option<RuntimePaths> {
        let macos = exe.parent().filter(|dir| dir.ends_with("MacOS"))?;
        let contents = macos.parent().filter(|dir| dir.ends_with("Contents"))?;
        if contents.parent()?.extension()? != "app" {
            return None;
        }
        Some(RuntimePaths {
            helper: contents.join("Helpers").join(HELPER),
            lib_dir: None,
            run_root,
        })
    }

    /// A dev checkout: `<repo>/packaging/box-bundle/out/{fletch-vm,*.dylib}`
    /// for the repo some ancestor of `exe` is (`src-tauri/target/debug/fletch`
    /// under `tauri dev`). Resolves as soon as the checkout is recognised, built
    /// or not, so [`probe`](Self::probe) can name the missing build.
    pub fn dev(exe: &Path, run_root: PathBuf) -> Option<RuntimePaths> {
        let repo = exe.ancestors().find(|dir| dir.join(DEV_MARKER).is_file())?;
        let out = repo.join(DEV_OUT_DIR);
        Some(RuntimePaths {
            helper: out.join(HELPER),
            lib_dir: Some(out),
            run_root,
        })
    }

    /// An explicit override: `FLETCH_BOX_HELPER` and `FLETCH_BOX_LIB_DIR`
    /// (what `scripts/box-dev.sh` prints), with `FLETCH_BOX_RUN_ROOT`
    /// defaulting to `~/.fletch/box-dev`.
    pub fn from_env() -> Option<RuntimePaths> {
        let helper = PathBuf::from(std::env::var_os("FLETCH_BOX_HELPER")?);
        let run_root = std::env::var_os("FLETCH_BOX_RUN_ROOT")
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".fletch/box-dev")))?;
        Some(RuntimePaths {
            helper,
            lib_dir: std::env::var_os("FLETCH_BOX_LIB_DIR").map(PathBuf::from),
            run_root,
        })
    }

    /// Where the helper will look for the dylibs.
    pub fn lib_dir(&self) -> PathBuf {
        self.lib_dir.clone().unwrap_or_else(|| {
            self.helper
                .parent()
                .map_or_else(|| PathBuf::from(".."), Path::to_path_buf)
                .join("../Frameworks")
        })
    }

    /// Whether this machine and these files can boot a box, item by item.
    pub fn probe(&self) -> Probe {
        let lib_dir = self.lib_dir();
        Probe {
            helper: self.helper.clone(),
            helper_present: self.helper.is_file(),
            helper_entitled: helper_entitled(&self.helper),
            dylibs_present: lib_dir.join(LIBKRUN).is_file() && lib_dir.join(LIBKRUNFW).is_file(),
            lib_dir,
            arch_ok: cfg!(all(target_os = "macos", target_arch = "aarch64")),
            hypervisor: hypervisor_available(),
        }
    }
}

/// The result of [`RuntimePaths::probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub helper: PathBuf,
    pub helper_present: bool,
    /// Signed with `com.apple.security.hypervisor`.
    pub helper_entitled: bool,
    pub lib_dir: PathBuf,
    pub dylibs_present: bool,
    /// Apple silicon running macOS; libkrun's macOS backend runs nowhere else.
    pub arch_ok: bool,
    /// `kern.hv_support`.
    pub hypervisor: bool,
}

impl Probe {
    pub fn ok(&self) -> bool {
        self.problems().is_empty()
    }

    /// One sentence per failing item, for the Settings page and error messages.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.arch_ok {
            out.push("boxes need an Apple silicon Mac".to_owned());
        } else if !self.hypervisor {
            out.push("this Mac reports no hypervisor support".to_owned());
        }
        if !self.helper_present {
            // In a dev checkout the runtime is staged by the hook `tauri dev`
            // runs; the one way to end up here is that it could not build.
            out.push(format!(
                "box runtime not built; run {DEV_BUILD_HOOK} (needs Xcode); no helper at {}",
                self.helper.display()
            ));
        } else if !self.helper_entitled {
            out.push(format!(
                "the helper is not signed with the hypervisor entitlement: {}",
                self.helper.display()
            ));
        }
        if !self.dylibs_present {
            out.push(format!(
                "libkrun is missing: {} and {} in {}",
                LIBKRUN,
                LIBKRUNFW,
                self.lib_dir.display()
            ));
        }
        out
    }
}

#[cfg(target_os = "macos")]
fn helper_entitled(helper: &Path) -> bool {
    Command::new("codesign")
        .args(["-d", "--entitlements", "-", "--xml"])
        .arg(helper)
        .stderr(Stdio::null())
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("com.apple.security.hypervisor"))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn hypervisor_available() -> bool {
    Command::new("sysctl")
        .args(["-n", "kern.hv_support"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "1")
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn helper_entitled(_helper: &Path) -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn hypervisor_available() -> bool {
    false
}

/// What to boot. The listen ports are libkrun's to bind (the host connects to
/// them); the connect ports must already be served by the host when the guest
/// dials them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmSpec {
    /// `[A-Za-z0-9_-]`, at most 32 chars; labels the helper process.
    pub id: String,
    pub cpus: u8,
    pub memory_mib: u32,
    pub base_image: PathBuf,
    pub data_disk: PathBuf,
    pub tools_disk: Option<PathBuf>,
    /// Further raw disks after the tools disk: `(block id, path, read-only)`.
    /// For script disks in tests and the CLI; the product attaches none.
    pub extra_disks: Vec<(String, PathBuf, bool)>,
    /// Guest listens; host connects to the unix socket libkrun binds.
    pub listen_ports: Vec<(u32, PathBuf)>,
    /// Guest connects; the host already serves the unix socket.
    pub connect_ports: Vec<(u32, PathBuf)>,
    /// Kernel-side env for init (identity paths, authorized key). `FLETCH_BOX_ID` is added.
    pub env: BTreeMap<String, String>,
    pub console: PathBuf,
    /// The guest entrypoint; `None` is [`guest::INIT`]. Tests boot Calina's image
    /// with `/opt/calina/init`.
    pub init: Option<PathBuf>,
    /// Arguments after the entrypoint. The product init takes none; a test
    /// init runs a script disk with `/bin/sh /dev/vdc`.
    pub init_args: Vec<String>,
}

impl VmSpec {
    /// Two vCPUs, 2 GiB, the product init, no ports.
    pub fn new(id: &str, base_image: &Path, data_disk: &Path, console: &Path) -> VmSpec {
        VmSpec {
            id: id.into(),
            cpus: 2,
            memory_mib: 2048,
            base_image: base_image.into(),
            data_disk: data_disk.into(),
            tools_disk: None,
            extra_disks: Vec::new(),
            listen_ports: Vec::new(),
            connect_ports: Vec::new(),
            env: BTreeMap::new(),
            console: console.into(),
            init: None,
            init_args: Vec::new(),
        }
    }

    /// The listen socket for `port`, if the spec maps one.
    pub fn listen_socket(&self, port: u32) -> Option<&Path> {
        self.listen_ports
            .iter()
            .find(|(p, _)| *p == port)
            .map(|(_, sock)| sock.as_path())
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |msg: String| Err(Error::InvalidSpec(msg));
        let id_ok = !self.id.is_empty()
            && self.id.len() <= 32
            && self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !id_ok {
            return invalid(format!("id {:?} must be 1-32 of [A-Za-z0-9_-]", self.id));
        }
        if self.cpus == 0 || self.memory_mib < 128 {
            return invalid("needs at least 1 vCPU and 128 MiB".into());
        }
        if self.env.keys().any(|k| k.is_empty() || k.contains('=')) {
            return invalid("environment names must be non-empty and contain no '='".into());
        }
        let mut ports: Vec<u32> = Vec::new();
        for (port, socket) in self.listen_ports.iter().chain(&self.connect_ports) {
            if ports.contains(port) {
                return invalid(format!("vsock port {port} used twice"));
            }
            ports.push(*port);
            if socket.as_os_str().len() > MAX_SOCKET_PATH {
                return invalid(format!(
                    "socket path longer than {MAX_SOCKET_PATH} bytes: {}",
                    socket.display()
                ));
            }
        }
        Ok(())
    }

    pub fn launch_config(&self, paths: &RuntimePaths) -> LaunchConfig {
        let mut disks = vec![
            // Every box shares the one base image, so the guest must not write
            // it, not even through the raw device.
            Disk {
                id: "base".into(),
                path: self.base_image.clone(),
                read_only: true,
            },
            Disk {
                id: "data".into(),
                path: self.data_disk.clone(),
                read_only: false,
            },
        ];
        if let Some(tools) = &self.tools_disk {
            disks.push(Disk {
                id: "tools".into(),
                path: tools.clone(),
                read_only: true,
            });
        }
        disks.extend(self.extra_disks.iter().map(|(id, path, ro)| Disk {
            id: id.clone(),
            path: path.clone(),
            read_only: *ro,
        }));

        let base_env = [
            (
                "PATH",
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            ),
            ("HOME", "/root"),
            ("TERM", "xterm"),
            (guest::ENV_BOX_ID, self.id.as_str()),
        ];
        let mut env: BTreeMap<&str, &str> = base_env.into_iter().collect();
        env.extend(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));

        let port = |(port, socket): &(u32, PathBuf)| VsockPort {
            port: *port,
            socket: socket.clone(),
        };
        let init = self
            .init
            .as_ref()
            .map_or(guest::INIT.to_owned(), |p| p.to_string_lossy().into_owned());

        LaunchConfig {
            id: self.id.clone(),
            host_pid: Some(std::process::id()),
            lib_dir: paths.lib_dir.clone(),
            cpus: self.cpus,
            memory_mib: self.memory_mib,
            disks,
            // Stock libkrun passes options as mount data and sets no flags;
            // erofs takes "ro" there, and without it a read-only disk fails to
            // mount (packaging/libkrun/README.md, patch 1).
            root: RootDisk {
                device: guest::DISK_BASE.into(),
                fstype: "erofs".into(),
                options: "ro".into(),
            },
            vsock_connect: self.connect_ports.iter().map(port).collect(),
            vsock_listen: self.listen_ports.iter().map(port).collect(),
            env: env.into_iter().map(|(k, v)| format!("{k}={v}")).collect(),
            workdir: "/".into(),
            log_level: 0,
            exec: std::iter::once(init)
                .chain(self.init_args.iter().cloned())
                .collect(),
        }
    }
}

/// Which rung of the stop ladder ended the VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// It had already exited.
    AlreadyExited,
    /// Init shut the guest down after `stop` on the control port.
    Control,
    /// The engine's ssh sysrq rung (reported by the engine; the runtime never returns it).
    Sysrq,
    /// SIGTERM to the helper.
    Term,
    /// SIGKILL: the data disk was not unmounted cleanly; leave `dirty` in place.
    Killed,
}

impl StopOutcome {
    /// Whether the guest got to sync and unmount its disk.
    pub fn clean(self) -> bool {
        !matches!(self, StopOutcome::Killed)
    }
}

/// One running VM: the `fletch-vm` child and its console file.
#[derive(Debug)]
pub struct Vm {
    child: Mutex<Child>,
    /// `fletch-vm` exits when this pipe closes (its `watch_parent`), so a crashed
    /// host never leaves VMs behind.
    _lifeline: ChildStdin,
    spec: VmSpec,
    pid: u32,
    started: Instant,
}

impl Vm {
    /// Spawns the helper. Returns once the process is running; the guest is
    /// still booting: follow with [`wait_ready`](Self::wait_ready).
    pub fn start(paths: &RuntimePaths, spec: &VmSpec) -> Result<Vm> {
        spec.validate()?;
        // libkrun binds listen-mode sockets itself and fails with EEXIST on a
        // stale path.
        for (_, socket) in &spec.listen_ports {
            match std::fs::remove_file(socket) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::io(format!("remove {}", socket.display()))(e)),
            }
        }
        let console = File::create(&spec.console)
            .map_err(Error::io(format!("create {}", spec.console.display())))?;
        let console_err = console
            .try_clone()
            .map_err(Error::io("clone console handle"))?;

        let mut child = Command::new(&paths.helper)
            .args(spec.launch_config(paths).to_argv())
            .stdin(Stdio::piped())
            .stdout(Stdio::from(console))
            .stderr(Stdio::from(console_err))
            .spawn()
            .map_err(Error::io(format!("spawn {}", paths.helper.display())))?;
        let lifeline = child.stdin.take().expect("stdin is piped");
        Ok(Vm {
            pid: child.id(),
            child: Mutex::new(child),
            _lifeline: lifeline,
            spec: spec.clone(),
            started: Instant::now(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn spec(&self) -> &VmSpec {
        &self.spec
    }

    pub fn started(&self) -> Instant {
        self.started
    }

    /// The console so far.
    pub fn console(&self) -> String {
        std::fs::read(&self.spec.console)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    }

    /// Follows the console until init prints `FLETCH_READY`, returning the host
    /// key it printed first. Fails closed, with the console tail, on
    /// `FLETCH_INIT_ERROR`, on the helper exiting, or after `timeout`.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<HostKey> {
        let deadline = Instant::now() + timeout;
        loop {
            let text = self.console();
            match console::scan(&text) {
                console::Progress::Ready(key) => return Ok(key),
                console::Progress::Failed(reason) => return Err(self.not_ready(reason, &text)),
                console::Progress::Booting { .. } => {}
            }
            if let Some(status) = self.try_wait()? {
                return Err(self.not_ready(format!("fletch-vm exited: {status}"), &text));
            }
            if Instant::now() >= deadline {
                return Err(self.not_ready(format!("guest not ready after {:.1?}", timeout), &text));
            }
            tokio::time::sleep(POLL).await;
        }
    }

    fn not_ready(&self, reason: String, console: &str) -> Error {
        Error::NotReady {
            reason,
            console: console::tail(console, CONSOLE_TAIL).to_owned(),
        }
    }

    pub fn try_wait(&self) -> Result<Option<ExitStatus>> {
        self.child
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .try_wait()
            .map_err(Error::io("check fletch-vm"))
    }

    /// Waits up to `timeout` for the helper to exit; `Duration::MAX` waits forever.
    pub async fn wait_exit(&self, timeout: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            if let Ok(Some(status)) = self.try_wait() {
                return Some(status);
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return None;
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Asks init for a clean shutdown: connects to the control listen socket
    /// and writes [`guest::STOP_LINE`]. Errors if the spec maps no control port
    /// or nothing in the guest answers.
    pub async fn send_stop(&self) -> Result<()> {
        let socket = self
            .spec
            .listen_socket(guest::CONTROL_PORT)
            .ok_or_else(|| Error::Other("the spec maps no control port".into()))?;
        send_stop(socket).await
    }

    /// The runtime's stop ladder: `stop` on the control port and `grace` to
    /// exit, then SIGTERM and 3 s, then SIGKILL. The engine inserts its ssh
    /// sysrq rung between the first two with [`send_stop`](Self::send_stop),
    /// [`wait_exit`](Self::wait_exit), [`terminate`](Self::terminate) and
    /// [`kill`](Self::kill) directly.
    pub async fn stop(&self, grace: Duration) -> StopOutcome {
        if matches!(self.try_wait(), Ok(Some(_))) {
            return StopOutcome::AlreadyExited;
        }
        match self.send_stop().await {
            Ok(()) => {
                if self.wait_exit(grace).await.is_some() {
                    return StopOutcome::Control;
                }
            }
            Err(e) => tracing::info!(id = %self.spec.id, error = %e, "control stop failed"),
        }
        self.terminate();
        if self.wait_exit(Duration::from_secs(3)).await.is_some() {
            return StopOutcome::Term;
        }
        self.kill();
        self.wait_exit(Duration::from_secs(5)).await;
        StopOutcome::Killed
    }

    /// SIGTERM to the helper.
    pub fn terminate(&self) {
        signal(self.pid, libc::SIGTERM);
    }

    /// SIGKILL to the helper. Recent guest writes are lost; `dirty` stays.
    pub fn kill(&self) {
        signal(self.pid, libc::SIGKILL);
    }
}

/// Writes [`guest::STOP_LINE`] to a box's control listen socket (`ctl.sock`).
/// Errors if nothing in the guest accepts within 2 s.
pub async fn send_stop(ctl_sock: &Path) -> Result<()> {
    let connect = tokio::net::UnixStream::connect(ctl_sock);
    let mut stream = tokio::time::timeout(Duration::from_secs(2), connect)
        .await
        .map_err(|_| Error::Other("control socket: connect timed out".into()))?
        .map_err(Error::io(format!("connect {}", ctl_sock.display())))?;
    stream
        .write_all(guest::STOP_LINE.as_bytes())
        .await
        .map_err(Error::io("write stop"))?;
    let _ = stream.shutdown().await;
    Ok(())
}

fn signal(pid: u32, sig: libc::c_int) {
    // SAFETY: signals a child this process spawned and has not reaped by pid
    // reuse: `Vm` holds the Child, so the pid stays ours until `wait`.
    unsafe { libc::kill(pid as libc::pid_t, sig) };
}

impl Drop for Vm {
    /// Closing the lifeline ends the helper; this only reaps it off-thread so
    /// a dropped VM leaves no zombie.
    fn drop(&mut self) {
        if let Ok(None) = self.try_wait() {
            let pid = self.pid as libc::pid_t;
            std::thread::spawn(move || {
                let mut status = 0;
                // SAFETY: waits on our own unreaped child by pid.
                unsafe { libc::waitpid(pid, &mut status, 0) };
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> VmSpec {
        let mut spec = VmSpec::new(
            "box-1",
            Path::new("/img/base.erofs"),
            Path::new("/b/data.img"),
            Path::new("/b/console.log"),
        );
        spec.listen_ports = vec![
            (guest::SSH_PORT, "/b/ssh.sock".into()),
            (guest::CONTROL_PORT, "/b/ctl.sock".into()),
        ];
        spec.connect_ports = vec![(guest::EGRESS_PORT, "/b/egress.sock".into())];
        spec.env
            .insert(guest::ENV_HOME.into(), "/Users/alex".into());
        spec
    }

    fn paths() -> RuntimePaths {
        RuntimePaths {
            helper: "/h/fletch-vm".into(),
            lib_dir: Some("/libs".into()),
            run_root: "/run".into(),
        }
    }

    #[test]
    fn spec_becomes_helper_argv() {
        let mut spec = spec();
        spec.tools_disk = Some("/t/claude.img".into());
        spec.extra_disks
            .push(("script".into(), "/b/script.img".into(), true));
        let config = spec.launch_config(&paths());
        let argv = config.to_argv();
        assert_eq!(LaunchConfig::parse(&argv).unwrap(), config);

        let disks: Vec<_> = config
            .disks
            .iter()
            .map(|d| (d.id.as_str(), d.path.to_str().unwrap(), d.read_only))
            .collect();
        assert_eq!(
            disks,
            [
                ("base", "/img/base.erofs", true),
                ("data", "/b/data.img", false),
                ("tools", "/t/claude.img", true),
                ("script", "/b/script.img", true),
            ]
        );
        assert_eq!(config.root.device, "/dev/vda");
        assert_eq!(config.lib_dir.as_deref(), Some(Path::new("/libs")));
        assert_eq!(config.exec, ["/opt/fletch/init"]);
        assert_eq!(
            config
                .vsock_listen
                .iter()
                .map(|p| p.port)
                .collect::<Vec<_>>(),
            [22, 5000]
        );
        assert_eq!(config.vsock_connect[0].port, 6000);
        assert!(config
            .env
            .contains(&"FLETCH_BOX_HOME=/Users/alex".to_string()));
        assert!(config.env.contains(&"FLETCH_BOX_ID=box-1".to_string()));
        assert!(config.env.contains(&"HOME=/root".to_string()));
    }

    #[test]
    fn test_init_overrides_the_entrypoint() {
        let mut spec = spec();
        spec.init = Some("/opt/calina/init".into());
        spec.init_args = vec!["/bin/sh".into(), "/dev/vdc".into()];
        assert_eq!(
            spec.launch_config(&paths()).exec,
            ["/opt/calina/init", "/bin/sh", "/dev/vdc"]
        );
    }

    #[test]
    fn validate_rejects_bad_specs() {
        assert!(spec().validate().is_ok());
        let mut bad = spec();
        bad.id = "../etc".into();
        assert!(bad.validate().is_err());
        let mut bad = spec();
        bad.connect_ports
            .push((guest::SSH_PORT, "/b/x.sock".into()));
        assert!(bad.validate().is_err());
        let mut bad = spec();
        bad.listen_ports
            .push((7, Path::new("/tmp").join("x".repeat(100)).join("s.sock")));
        assert!(bad.validate().is_err());
    }

    #[test]
    fn finds_the_helper_in_an_app_bundle_only() {
        let run_root = PathBuf::from("/r");
        assert_eq!(
            RuntimePaths::bundled(
                Path::new("/Applications/Fletch.app/Contents/MacOS/fletch"),
                run_root.clone()
            )
            .map(|p| p.helper),
            Some("/Applications/Fletch.app/Contents/Helpers/fletch-vm".into())
        );
        for exe in [
            "/repo/src-tauri/target/debug/fletch",
            "/x/Contents/MacOS/fletch",
            "/x/Fletch.app/MacOS/fletch",
        ] {
            assert!(
                RuntimePaths::bundled(Path::new(exe), run_root.clone()).is_none(),
                "{exe}"
            );
        }
    }

    #[test]
    fn probe_names_every_problem() {
        let probe = RuntimePaths {
            helper: "/nowhere/fletch-vm".into(),
            lib_dir: Some("/nowhere".into()),
            run_root: "/r".into(),
        }
        .probe();
        assert!(!probe.ok());
        let text = probe.problems().join("\n");
        assert!(text.contains("box runtime not built; run packaging/box-bundle/ensure-dev.sh"));
        assert!(text.contains("/nowhere/fletch-vm"));
        assert!(text.contains("libkrun is missing"));
    }

    /// The dev lookup finds this checkout from a binary anywhere under it,
    /// whether or not the runtime has been built, and nothing outside one.
    #[test]
    fn finds_the_dev_runtime_from_a_binary_inside_the_checkout() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let exe = repo.join("src-tauri/target/debug/fletch");
        let paths = RuntimePaths::dev(&exe, "/r".into()).expect("inside the checkout");
        assert_eq!(paths.helper, repo.join(DEV_OUT_DIR).join(HELPER));
        assert_eq!(paths.lib_dir, Some(repo.join(DEV_OUT_DIR)));
        assert!(RuntimePaths::dev(
            Path::new("/Applications/Fletch.app/Contents/MacOS/fletch"),
            "/r".into()
        )
        .is_none());
    }
}

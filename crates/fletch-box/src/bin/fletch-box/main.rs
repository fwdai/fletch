//! `fletch-box`: drives the runtime by hand for development and the VM tests.
//! Box directories live under `~/.fletch/box-dev/<id>/`, laid out as the
//! engine's are: key pair, `known_hosts`, `ssh_config`, the three sockets,
//! `console.log`, `dirty` and `vm.pid`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use fletch_box::{
    channel, disks, guest, Channel, EgressEvent, EgressPolicy, EgressProxy, ImageStore,
    PinnedImage, RuntimePaths, StopOutcome, Vm, VmSpec,
};

#[derive(Parser)]
#[command(
    name = "fletch-box",
    about = "Boot, reach and stop box micro-VMs by hand"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Report whether this machine, the helper and libkrun can boot a box.
    Probe,
    /// Boot a box and stay attached until Ctrl-C, then stop it cleanly.
    Boot(BootArgs),
    /// Open a shell in a running box, or run a command in it.
    Ssh {
        id: String,
        /// Log in as this user instead of `agent` (dev images without that user).
        #[arg(short = 'l', long)]
        user: Option<String>,
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Stop a box booted by another `fletch-box boot`.
    Stop { id: String },
    /// The base image store.
    Image {
        #[command(subcommand)]
        command: ImageCmd,
    },
    /// Tools disks: a provider's CLI on a read-only disk.
    Tools {
        #[command(subcommand)]
        command: ToolsCmd,
    },
}

#[derive(Subcommand)]
enum ToolsCmd {
    /// Boot a throwaway box on the base image and install a provider's CLI
    /// onto a fresh disk, which boxes then mount read-only as `/opt/fletch/tools`.
    Build {
        /// `claude`, `codex`, `cursor` or `opencode`.
        provider: String,
        /// The CLI version to install (`latest` for cursor).
        version: String,
        /// The base image (`base.erofs`).
        #[arg(long)]
        image: PathBuf,
        /// Where the disk lands; default `<run root>/tools/<provider>-<version>.img`.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(clap::Args)]
struct BootArgs {
    #[arg(long)]
    id: String,
    /// The base image (`base.erofs`).
    #[arg(long)]
    image: PathBuf,
    /// The data disk; created sparse (8 GiB) if absent.
    #[arg(long)]
    data: PathBuf,
    #[arg(long)]
    tools: Option<PathBuf>,
    #[arg(long, default_value_t = 2048)]
    mem: u32,
    #[arg(long, default_value_t = 2)]
    cpus: u8,
    /// Guest entrypoint; default /opt/fletch/init. Calina's image: /opt/calina/init.
    #[arg(long)]
    init: Option<PathBuf>,
    /// Arguments after the entrypoint.
    #[arg(long = "init-arg")]
    init_args: Vec<String>,
    /// A shell script attached as a raw read-only disk after the tools disk;
    /// with no --init-arg the guest runs `/bin/sh /dev/vdX` on it.
    #[arg(long)]
    script_disk: Option<PathBuf>,
    /// Extra kernel-side env for init (`K=V`).
    #[arg(long = "env")]
    env: Vec<String>,
    /// Egress hosts allowed in addition to the defaults.
    #[arg(long = "allow")]
    allow: Vec<String>,
    /// How long to wait for FLETCH_READY.
    #[arg(long, default_value_t = 15)]
    ready_timeout: u64,
}

#[derive(Subcommand)]
enum ImageCmd {
    Status,
    Install {
        /// Alternate release base URL (the `box_image_url` setting).
        #[arg(long)]
        url: Option<String>,
    },
}

const PID_FILE: &str = "vm.pid";

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("fletch-box: {err}");
            ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.command {
        Cmd::Probe => {
            let probe = paths()?.probe();
            println!("{probe:#?}");
            if probe.ok() {
                println!("ok: this machine can boot boxes");
                Ok(ExitCode::SUCCESS)
            } else {
                for p in probe.problems() {
                    println!("problem: {p}");
                }
                Ok(ExitCode::from(1))
            }
        }
        Cmd::Boot(args) => boot(args).await.map(|()| ExitCode::SUCCESS),
        Cmd::Ssh { id, user, cmd } => ssh(&id, user.as_deref(), &cmd),
        Cmd::Stop { id } => stop(&id).await.map(|()| ExitCode::SUCCESS),
        Cmd::Image { command } => image(command).await.map(|()| ExitCode::SUCCESS),
        Cmd::Tools {
            command:
                ToolsCmd::Build {
                    provider,
                    version,
                    image,
                    out,
                },
        } => {
            let paths = paths()?;
            let out = match out {
                Some(out) => out,
                None => {
                    fletch_box::tools::disk_path(&dev_root()?.join("tools"), &provider, &version)
                }
            };
            let t0 = Instant::now();
            fletch_box::tools::build(&paths, &image, &provider, &version, &out, |line| {
                println!("{line}")
            })
            .await
            .map_err(|e| e.to_string())?;
            println!("built {} in {:.1?}", out.display(), t0.elapsed());
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// `FLETCH_BOX_RUN_ROOT`, else `~/.fletch/box-dev`.
fn dev_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("FLETCH_BOX_RUN_ROOT") {
        return Ok(PathBuf::from(root));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".fletch/box-dev"))
}

fn box_dir(id: &str) -> Result<PathBuf, String> {
    Ok(dev_root()?.join(id))
}

/// `FLETCH_BOX_HELPER`/`FLETCH_BOX_LIB_DIR` (what `scripts/box-dev.sh` prints),
/// else a `fletch-vm` next to this binary with `FLETCH_BOX_LIB_DIR`.
fn paths() -> Result<RuntimePaths, String> {
    if let Some(paths) = RuntimePaths::from_env() {
        return Ok(paths);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let helper = exe.with_file_name("fletch-vm");
    if !helper.is_file() {
        return Err(
            "set FLETCH_BOX_HELPER and FLETCH_BOX_LIB_DIR (scripts/box-dev.sh prints them)".into(),
        );
    }
    Ok(RuntimePaths {
        helper,
        lib_dir: std::env::var_os("FLETCH_BOX_LIB_DIR").map(PathBuf::from),
        run_root: dev_root()?,
    })
}

async fn boot(args: BootArgs) -> Result<(), String> {
    let paths = paths()?;
    let probe = paths.probe();
    if !probe.ok() {
        return Err(probe.problems().join("; "));
    }
    let dir = box_dir(&args.id)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let key = channel::keygen(&dir).map_err(|e| e.to_string())?;
    let authorized = channel::authorized_key(&key).map_err(|e| e.to_string())?;
    if !args.data.exists() {
        disks::create_sparse(&args.data, disks::DEFAULT_DATA_DISK_BYTES)
            .map_err(|e| format!("create data disk: {e}"))?;
        println!("created sparse data disk {}", args.data.display());
    }
    let dirty = disks::Dirty::in_dir(&dir);
    if dirty.set().map_err(|e| e.to_string())? {
        println!("dirty marker found: the last VM on this disk was not stopped cleanly");
    }

    let policy = EgressPolicy::with_extra(&args.allow).map_err(|e| e.to_string())?;
    let proxy = EgressProxy::start(&dir.join("egress.sock"), policy, |event| match event {
        EgressEvent::Allowed {
            host,
            port,
            bytes_up,
            bytes_down,
        } => println!("egress: allow {host}:{port} up {bytes_up} down {bytes_down}"),
        EgressEvent::Denied { host, port, reason } => {
            println!("egress: DENY {host}:{port} ({reason})")
        }
        EgressEvent::Error { message, .. } => println!("egress: error {message}"),
    })
    .map_err(|e| format!("egress proxy: {e}"))?;

    let mut spec = VmSpec::new(&args.id, &args.image, &args.data, &dir.join("console.log"));
    spec.cpus = args.cpus;
    spec.memory_mib = args.mem;
    spec.tools_disk = args.tools.clone();
    spec.init = args.init.clone();
    spec.init_args = args.init_args.clone();
    spec.listen_ports = vec![
        (guest::SSH_PORT, dir.join("ssh.sock")),
        (guest::CONTROL_PORT, dir.join("ctl.sock")),
    ];
    spec.connect_ports = vec![(guest::EGRESS_PORT, proxy.socket().to_path_buf())];
    spec.env
        .insert(guest::ENV_AUTHORIZED_KEY.into(), authorized);
    for kv in &args.env {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("--env wants K=V, got {kv}"))?;
        spec.env.insert(k.into(), v.into());
    }
    if let Some(script) = &args.script_disk {
        let bytes = std::fs::read(script).map_err(|e| format!("read {}: {e}", script.display()))?;
        let disk = dir.join("script.img");
        disks::write_script_disk(&bytes, &disk).map_err(|e| e.to_string())?;
        // vda base, vdb data, then the tools disk if any, then this one.
        let letter = (b'c' + u8::from(args.tools.is_some())) as char;
        if spec.init_args.is_empty() {
            spec.init_args = vec!["/bin/sh".into(), format!("/dev/vd{letter}")];
        }
        spec.extra_disks.push(("script".into(), disk, true));
    }

    let t0 = Instant::now();
    let vm = Vm::start(&paths, &spec).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(PID_FILE), vm.pid().to_string()).map_err(|e| e.to_string())?;
    println!(
        "fletch-vm pid {} spawned in {:.0?}; console {}",
        vm.pid(),
        t0.elapsed(),
        spec.console.display()
    );
    let host_key = match vm.wait_ready(Duration::from_secs(args.ready_timeout)).await {
        Ok(key) => key,
        Err(e) => {
            vm.kill();
            let _ = std::fs::remove_file(dir.join(PID_FILE));
            return Err(e.to_string());
        }
    };
    let ready = t0.elapsed();
    channel::write_known_hosts(&dir.join(channel::KNOWN_HOSTS_FILE), &host_key)
        .map_err(|e| e.to_string())?;
    let chan = Channel::new(
        &dir,
        &dir.join("ssh.sock"),
        &key,
        &dir.join(channel::KNOWN_HOSTS_FILE),
        &paths.helper,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "ready in {ready:.2?}; host key {} {}",
        host_key.key_type,
        &host_key.base64[..host_key.base64.len().min(16)]
    );
    println!(
        "ssh config {}; try: fletch-box ssh {} -- uname -r",
        chan.config.display(),
        args.id
    );
    println!("Ctrl-C stops the box");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            let t1 = Instant::now();
            let outcome = vm.stop(Duration::from_secs(10)).await;
            println!("stopped via {outcome:?} in {:.2?}", t1.elapsed());
            if outcome.clean() {
                dirty.clear().map_err(|e| e.to_string())?;
            }
        }
        status = vm.wait_exit(Duration::MAX) => {
            let status = status.expect("wait_exit without a deadline returns a status");
            println!("fletch-vm exited: {status}");
            if status.success() {
                dirty.clear().map_err(|e| e.to_string())?;
            } else {
                println!("console tail:\n{}", fletch_box::runtime::console::tail(&vm.console(), 2000));
            }
        }
    }
    let _ = std::fs::remove_file(dir.join(PID_FILE));
    proxy.stop();
    Ok(())
}

fn ssh(id: &str, user: Option<&str>, cmd: &[String]) -> Result<ExitCode, String> {
    let chan = Channel::open(&box_dir(id)?);
    if !chan.config.exists() {
        return Err(format!("no {}: is the box booted?", chan.config.display()));
    }
    let mut ssh = std::process::Command::new(channel::SSH);
    ssh.arg("-F").arg(&chan.config);
    if let Some(user) = user {
        ssh.arg("-l").arg(user);
    }
    if cmd.is_empty() {
        ssh.arg("-t");
    }
    ssh.arg(guest::SSH_HOST_ALIAS);
    ssh.args(cmd.iter().map(|w| channel::shell_quote(w)));
    let status = ssh.status().map_err(|e| format!("run ssh: {e}"))?;
    Ok(ExitCode::from(status.code().unwrap_or(255) as u8))
}

/// The stop ladder from outside the process holding the VM: `stop` on the
/// control socket, then SIGTERM, then SIGKILL, judging by the helper's pid.
async fn stop(id: &str) -> Result<(), String> {
    let dir = box_dir(id)?;
    let pid: i32 = std::fs::read_to_string(dir.join(PID_FILE))
        .map_err(|_| format!("no {} for {id}: not booted by fletch-box boot?", PID_FILE))?
        .trim()
        .parse()
        .map_err(|_| "bad pid file".to_owned())?;
    let alive = || unsafe { libc::kill(pid, 0) } == 0;
    let wait = |secs: u64| async move {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while alive() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        !alive()
    };
    if !alive() {
        println!("{id}: already stopped");
        return Ok(());
    }
    let t0 = Instant::now();
    let outcome = if fletch_box::runtime::send_stop(&dir.join("ctl.sock"))
        .await
        .is_ok()
        && wait(10).await
    {
        StopOutcome::Control
    } else {
        unsafe { libc::kill(pid, libc::SIGTERM) };
        if wait(3).await {
            StopOutcome::Term
        } else {
            unsafe { libc::kill(pid, libc::SIGKILL) };
            wait(5).await;
            StopOutcome::Killed
        }
    };
    println!("{id}: stopped via {outcome:?} in {:.2?}", t0.elapsed());
    Ok(())
}

async fn image(command: ImageCmd) -> Result<(), String> {
    let store = ImageStore::new(&dev_root()?);
    match command {
        ImageCmd::Status => {
            let pinned = PinnedImage::this_build(None);
            println!(
                "pinned: {} ({}){}",
                pinned.version,
                pinned.url,
                if pinned.sha256.is_empty() {
                    " [not published yet]"
                } else {
                    ""
                }
            );
            match store.current().map_err(|e| e.to_string())? {
                Some(image) => {
                    println!("installed: {} at {}", image.version, image.path().display())
                }
                None => println!("installed: none under {}", store.dir().display()),
            }
        }
        ImageCmd::Install { url } => {
            let pinned = PinnedImage::this_build(url.as_deref());
            let mut last = 0;
            let installed = store
                .install(&pinned, |p| {
                    let pct = (p.done * 100).checked_div(p.total).unwrap_or(0);
                    if pct != last {
                        last = pct;
                        println!("{pct}%");
                    }
                    std::ops::ControlFlow::Continue(())
                })
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "installed {} at {}",
                installed.version,
                installed.path().display()
            );
        }
    }
    Ok(())
}

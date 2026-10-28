//! `fletch-vm`: runs one libkrun micro-VM, then exits with it; or bridges stdio
//! to a unix socket (`fletch-vm pipe <socket>`, ssh's `ProxyCommand`).
//!
//! `krun_start_enter()` takes over the calling process, so the host spawns one
//! `fletch-vm` per VM. The host keeps the helper's stdin open for the VM's
//! life: EOF means the host is gone, and the helper exits at once, taking the
//! VM with it. Guest console output goes to stdout; the helper's own errors to
//! stderr, prefixed `fletch-vm:`, with exit code 125 (libkrun's "couldn't set
//! up the VM").
//!
//! libkrun is loaded with `dlopen` at runtime, so the binary builds and runs on
//! machines that don't have it. This binary links none of the library crate.

#[path = "../../runtime/launch.rs"]
#[allow(dead_code)]
mod launch;

#[cfg(target_os = "macos")]
mod krun;
mod pipe;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("pipe") {
        let Some(socket) = args.get(1) else {
            fail("usage: fletch-vm pipe <unix socket>");
        };
        if let Err(err) = pipe::run(socket) {
            fail(&err);
        }
        return;
    }
    let config = match launch::LaunchConfig::parse(&args) {
        Ok(config) => config,
        Err(err) => fail(&format!(
            "{err}\nusage: fletch-vm [flags] -- <guest command> | fletch-vm pipe <socket>"
        )),
    };
    if let Err(err) = watch_parent() {
        fail(&err);
    }
    run(&config);
}

#[cfg(target_os = "macos")]
fn run(config: &launch::LaunchConfig) -> ! {
    let krun = match krun::Krun::load(config.lib_dir.as_deref()) {
        Ok(krun) => krun,
        Err(err) => fail(&err),
    };
    // Only returns on failure; on success libkrun exits the process when the guest stops.
    fail(&krun.run(config))
}

#[cfg(not(target_os = "macos"))]
fn run(_config: &launch::LaunchConfig) -> ! {
    fail("VMs run on macOS only")
}

fn fail(err: &str) -> ! {
    eprintln!("fletch-vm: {err}");
    std::process::exit(125);
}

/// Moves the host's pipe off stdin and exits the process when it reaches EOF.
///
/// libkrun takes over stdin for the guest console, so stdin becomes /dev/null
/// and the pipe lives on in another descriptor watched by a thread.
fn watch_parent() -> Result<(), String> {
    let os_err = |what: &str| format!("{what}: {}", std::io::Error::last_os_error());
    // SAFETY: plain descriptor syscalls on fds this process owns.
    let pipe = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 3) };
    if pipe < 0 {
        return Err(os_err("dup stdin"));
    }
    let null = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) };
    if null < 0 || unsafe { libc::dup2(null, 0) } < 0 {
        return Err(os_err("redirect stdin"));
    }
    unsafe { libc::close(null) };

    std::thread::Builder::new()
        .name("parent-watch".into())
        .spawn(move || {
            let mut buf = [0u8; 64];
            loop {
                // SAFETY: reads into a valid local buffer.
                let n = unsafe { libc::read(pipe, buf.as_mut_ptr().cast(), buf.len()) };
                let interrupted = n < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted;
                // The host never writes; anything that arrives is ignored.
                if n > 0 || interrupted {
                    continue;
                }
                eprintln!("fletch-vm: host went away, stopping the VM");
                // _exit, not exit: the VMM's threads are mid-flight and atexit
                // handlers could block on them.
                unsafe { libc::_exit(1) };
            }
        })
        .map(drop)
        .map_err(|err| format!("spawn parent watch: {err}"))
}

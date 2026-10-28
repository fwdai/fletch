//! VM tests: boot a real guest. Ignored by default; `scripts/box-dev.sh` prints
//! the environment and crates/fletch-box/README.md the command. Needs:
//!
//! - `FLETCH_BOX_HELPER`: a built, ad-hoc signed `fletch-vm`
//! - `FLETCH_BOX_LIB_DIR`: libkrun dylibs
//! - `FLETCH_BOX_TEST_IMAGE`: a `base.erofs` whose init runs a command, by
//!   default Calina's (`/opt/calina/init`, override with `FLETCH_BOX_TEST_INIT`)
//!
//! The runtime does not care which Linux is inside: the guest script prints the
//! `FLETCH_HOSTKEY` and `FLETCH_READY` markers the product init will, and
//! listens on the control port the way `fletch-guest wait-stop` will.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fletch_box::{disks, guest, RuntimePaths, StopOutcome, Vm, VmSpec};

const FAKE_KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFakeFakeFakeFakeFak";

/// Runs under the test init as `/bin/sh /dev/vdc`: the markers, then wait for
/// `stop` on vsock 5000 (socat is in Calina's image), then a clean exit, which
/// libkrun's init turns into a VM exit.
fn guest_script() -> String {
    format!(
        "echo \"{} ssh-ed25519 {FAKE_KEY}\"\necho {}\n\
         socat -u VSOCK-LISTEN:{},reuseaddr - | while read line; do [ \"$line\" = stop ] && break; done\n\
         sync; umount /data 2>/dev/null; exit 0\n",
        guest::MARK_HOSTKEY,
        guest::MARK_READY,
        guest::CONTROL_PORT
    )
}

struct Setup {
    paths: RuntimePaths,
    dir: PathBuf,
    spec: VmSpec,
    dirty: disks::Dirty,
}

fn setup(name: &str) -> Setup {
    let paths =
        RuntimePaths::from_env().expect("FLETCH_BOX_HELPER (scripts/box-dev.sh prints the env)");
    let image =
        PathBuf::from(std::env::var_os("FLETCH_BOX_TEST_IMAGE").expect("FLETCH_BOX_TEST_IMAGE"));
    let init = std::env::var("FLETCH_BOX_TEST_INIT").unwrap_or_else(|_| "/opt/calina/init".into());
    let probe = paths.probe();
    assert!(probe.ok(), "{:?}", probe.problems());

    // Under /tmp, not $TMPDIR: socket paths must stay short.
    let dir = PathBuf::from(format!(
        "/tmp/fletch-box-test-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data.img");
    disks::create_sparse(&data, 1 << 30).unwrap();
    let script = dir.join("script.img");
    disks::write_script_disk(guest_script().as_bytes(), &script).unwrap();

    let mut spec = VmSpec::new(name, &image, &data, &dir.join("console.log"));
    spec.cpus = 1;
    spec.memory_mib = 1024;
    spec.init = Some(PathBuf::from(init));
    spec.init_args = vec!["/bin/sh".into(), guest::DISK_TOOLS.into()];
    spec.extra_disks.push(("script".into(), script, true));
    spec.listen_ports = vec![
        (guest::SSH_PORT, dir.join("ssh.sock")),
        (guest::CONTROL_PORT, dir.join("ctl.sock")),
    ];
    let dirty = disks::Dirty::in_dir(&dir);
    Setup {
        paths,
        dir,
        spec,
        dirty,
    }
}

async fn boot(s: &Setup) -> (Vm, Duration, Duration) {
    assert!(!s.dirty.set().unwrap());
    let t0 = Instant::now();
    let vm = Vm::start(&s.paths, &s.spec).unwrap();
    let spawned = t0.elapsed();
    let key = vm
        .wait_ready(Duration::from_secs(20))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let ready = t0.elapsed();
    assert_eq!(key.key_type, "ssh-ed25519");
    assert_eq!(key.base64, FAKE_KEY);
    assert!(
        Path::new(&s.dir.join("ssh.sock")).exists(),
        "libkrun bound the ssh listen socket"
    );
    (vm, spawned, ready)
}

#[tokio::test]
#[ignore = "boots a VM; see crates/fletch-box/README.md"]
async fn clean_stop_clears_dirty() {
    let s = setup("clean");
    // First boot formats the data disk; the second is the steady state.
    let (vm, spawned, ready) = boot(&s).await;
    let t0 = Instant::now();
    let outcome = vm.stop(Duration::from_secs(10)).await;
    let stopped = t0.elapsed();
    assert_eq!(outcome, StopOutcome::Control, "console:\n{}", vm.console());
    s.dirty.clear().unwrap();
    assert!(!s.dirty.is_set());
    drop(vm);
    eprintln!("first boot: spawn {spawned:.0?}, ready {ready:.2?}, control stop {stopped:.2?}");

    let (vm, spawned, ready) = boot(&s).await;
    let t0 = Instant::now();
    let outcome = vm.stop(Duration::from_secs(10)).await;
    assert_eq!(outcome, StopOutcome::Control, "console:\n{}", vm.console());
    eprintln!(
        "second boot: spawn {spawned:.0?}, ready {ready:.2?}, control stop {:.2?}",
        t0.elapsed()
    );
    s.dirty.clear().unwrap();
    let _ = std::fs::remove_dir_all(&s.dir);
}

#[tokio::test]
#[ignore = "boots a VM; see crates/fletch-box/README.md"]
async fn kill_leaves_dirty_and_a_stale_socket_is_removed_on_restart() {
    let s = setup("kill");
    let (vm, _, ready) = boot(&s).await;
    eprintln!("boot ready {ready:.2?}");
    vm.kill();
    let status = vm
        .wait_exit(Duration::from_secs(5))
        .await
        .expect("killed helper exits");
    assert!(!status.success());
    assert!(s.dirty.is_set(), "dirty stays after a kill");
    drop(vm);

    // The stale ssh.sock from the killed VM would make libkrun fail with EEXIST.
    assert!(s.dir.join("ssh.sock").exists());
    s.dirty.clear().unwrap();
    let (vm, _, ready) = boot(&s).await;
    eprintln!("boot after kill (init fscks) ready {ready:.2?}");
    assert_eq!(vm.stop(Duration::from_secs(10)).await, StopOutcome::Control);
    let _ = std::fs::remove_dir_all(&s.dir);
}

#[tokio::test]
#[ignore = "boots a VM; see crates/fletch-box/README.md"]
async fn closing_the_lifeline_ends_the_helper() {
    let s = setup("lifeline");
    let (vm, _, _) = boot(&s).await;
    let pid = vm.pid();
    drop(vm);
    let deadline = Instant::now() + Duration::from_secs(5);
    // SAFETY: signal 0 only checks for existence.
    while unsafe { libc::kill(pid as i32, 0) } == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_ne!(
        unsafe { libc::kill(pid as i32, 0) },
        0,
        "fletch-vm outlived its host"
    );
    let _ = std::fs::remove_dir_all(&s.dir);
}

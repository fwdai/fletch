//! A workbench: a throwaway box booted on one disk, with no agent and no
//! workspace, for the host to work on that disk from the inside over its
//! channel. A tools disk is built on one; a workspace disk is reduced to a
//! project's seed on one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::channel::{self, Channel};
use crate::error::{Error, Result};
use crate::guest;
use crate::runtime::{RuntimePaths, StopOutcome, Vm, VmSpec};

const READY_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_GRACE: Duration = Duration::from_secs(15);

static SEQ: AtomicU32 = AtomicU32::new(0);

pub struct Workbench {
    pub channel: Channel,
    /// The guest user's `$HOME` on the bench, on the disk.
    pub guest_home: PathBuf,
    vm: Vm,
    dir: PathBuf,
    stopped: bool,
}

impl Workbench {
    /// A fresh directory for a bench's sockets and key, named for its
    /// `purpose` and unlike any other bench's of this process, under the run
    /// root so socket paths stay short. [`Workbench::boot`] takes it over and
    /// removes it when the bench goes.
    pub fn fresh_dir(paths: &RuntimePaths, purpose: &str) -> Result<PathBuf> {
        let dir = paths.run_root.join(format!(
            "{purpose}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(Error::io(format!("create {}", dir.display())))?;
        Ok(dir)
    }

    /// Boots `image` in `dir` ([`Workbench::fresh_dir`]) with `disk` as its
    /// data disk. `egress` is the socket of a proxy the caller serves, for
    /// work that needs the network.
    pub async fn boot(
        paths: &RuntimePaths,
        image: &Path,
        dir: PathBuf,
        disk: &Path,
        egress: Option<&Path>,
    ) -> Result<Workbench> {
        let id = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let probe = paths.probe();
        if !probe.ok() {
            return Err(Error::Unsupported(probe.problems().join("; ")));
        }
        let key = channel::keygen(&dir)?;
        let authorized = channel::authorized_key(&key)?;

        // The identity paths are a contract with init, not something the work
        // uses; a bench has no agent, so they point at a scratch tree under
        // the user's home (whose top directory the image has).
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Error::Other("HOME is not set".into()))?;
        let scratch = home.join(".fletch").join("tools-build");
        let guest_home = scratch.join("home");

        let mut spec = VmSpec::new(&id, image, disk, &dir.join("console.log"));
        spec.listen_ports = vec![
            (guest::SSH_PORT, dir.join("ssh.sock")),
            (guest::CONTROL_PORT, dir.join("ctl.sock")),
        ];
        spec.connect_ports = egress
            .map(|socket| (guest::EGRESS_PORT, socket.to_path_buf()))
            .into_iter()
            .collect();
        spec.env = BTreeMap::from([
            (
                guest::ENV_WRITABLE_ROOT.to_owned(),
                path_string(&scratch.join("ws")),
            ),
            (
                guest::ENV_RPC_DIR.to_owned(),
                path_string(&scratch.join("rpc")),
            ),
            (guest::ENV_HOME.to_owned(), path_string(&guest_home)),
            (guest::ENV_AUTHORIZED_KEY.to_owned(), authorized),
        ]);

        let vm = Vm::start(paths, &spec)?;
        let reached = async {
            let host_key = vm.wait_ready(READY_TIMEOUT).await?;
            let known_hosts = dir.join(channel::KNOWN_HOSTS_FILE);
            channel::write_known_hosts(&known_hosts, &host_key)?;
            Channel::new(
                &dir,
                &dir.join("ssh.sock"),
                &key,
                &known_hosts,
                &paths.helper,
            )
        }
        .await;
        match reached {
            Ok(channel) => Ok(Workbench {
                channel,
                guest_home,
                vm,
                dir,
                stopped: false,
            }),
            Err(e) => {
                vm.kill();
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    /// Stops the bench through init, so what the work wrote is on the disk.
    /// [`StopOutcome::Killed`] means it may not be.
    pub async fn stop(mut self) -> StopOutcome {
        let outcome = self.vm.stop(STOP_GRACE).await;
        self.stopped = true;
        outcome
    }
}

impl Drop for Workbench {
    fn drop(&mut self) {
        if !self.stopped {
            self.vm.kill();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

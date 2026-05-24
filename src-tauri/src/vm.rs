//! Tart CLI wrapper.
//!
//! Wraps `tart clone / run / ip / stop / delete` behind a small async API. The
//! actual command invocation goes through a [`TartCli`] trait so tests can
//! substitute a fake.

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::time::sleep;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct CapturedOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CapturedOutput {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

#[async_trait]
pub trait TartCli: Send + Sync + std::fmt::Debug {
    async fn capture(&self, args: &[&str]) -> Result<CapturedOutput>;

    /// Spawn `tart run` (or similar long-running command) and return the
    /// child process handle.
    async fn spawn_detached(&self, args: &[&str]) -> Result<Child>;
}

#[derive(Debug, Clone)]
pub struct RealTartCli {
    pub binary: PathBuf,
}

impl RealTartCli {
    pub fn new(binary: PathBuf) -> Self {
        Self { binary }
    }
}

#[async_trait]
impl TartCli for RealTartCli {
    async fn capture(&self, args: &[&str]) -> Result<CapturedOutput> {
        let out = Command::new(&self.binary).args(args).output().await?;
        Ok(CapturedOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    async fn spawn_detached(&self, args: &[&str]) -> Result<Child> {
        let child = Command::new(&self.binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        Ok(child)
    }
}

#[derive(Debug, Clone)]
pub struct MountSpec<'a> {
    pub name: &'a str,
    pub path: &'a Path,
    pub readonly: bool,
}

pub struct Vm {
    cli: Box<dyn TartCli>,
}

impl Vm {
    pub fn new(cli: Box<dyn TartCli>) -> Self {
        Self { cli }
    }

    #[allow(dead_code)]
    pub async fn version(&self) -> Result<String> {
        let out = self.cli.capture(&["--version"]).await?;
        if !out.ok() {
            return Err(Error::Tart(out.stderr));
        }
        Ok(out.stdout.trim().to_string())
    }

    pub async fn clone_image(&self, from: &str, to: &str) -> Result<()> {
        let out = self.cli.capture(&["clone", from, to]).await?;
        if !out.ok() {
            return Err(Error::Tart(format!(
                "clone {from} -> {to} failed: {}",
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        let out = self.cli.capture(&["delete", name]).await?;
        if !out.ok() {
            return Err(Error::Tart(format!(
                "delete {name} failed: {}",
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    pub async fn stop(&self, name: &str) -> Result<()> {
        let out = self.cli.capture(&["stop", name, "--timeout", "10"]).await?;
        if !out.ok() {
            return Err(Error::Tart(format!(
                "stop {name} failed: {}",
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    pub async fn list_names(&self) -> Result<Vec<String>> {
        let out = self.cli.capture(&["list", "--quiet"]).await?;
        if !out.ok() {
            return Err(Error::Tart(out.stderr));
        }
        Ok(out
            .stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    /// Block until tart reports an IP for the VM, or fail after `timeout`.
    pub async fn wait_for_ip(&self, name: &str, timeout: Duration) -> Result<String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match self.try_ip(name).await? {
                Some(ip) => return Ok(ip),
                None => {
                    if std::time::Instant::now() >= deadline {
                        return Err(Error::VmBootTimeout(timeout.as_secs()));
                    }
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    /// Single-shot IP probe. Returns `Ok(None)` if Tart doesn't (yet) have an
    /// IP for this VM, `Ok(Some(ip))` once it does, or `Err` for any error
    /// other than "no IP yet". Callers wanting their own polling loop (e.g.
    /// the bake flow, which also watches the `tart run` child for early
    /// exit) use this directly.
    pub async fn try_ip(&self, name: &str) -> Result<Option<String>> {
        let out = self.cli.capture(&["ip", name, "--wait", "2"]).await?;
        if out.ok() {
            let trimmed = out.stdout.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        } else {
            // Tart returns nonzero when it can't find the IP. Distinguish
            // "real error" from "not ready yet" by looking at stderr — but
            // for now treat both as "not ready" and let the outer loop
            // timeout. The bake flow surfaces the underlying `tart run`
            // stderr separately, which is the more useful signal.
            Ok(None)
        }
    }

    /// Has a VM by this name already been cloned? Returns the result of
    /// `tart list --quiet` filtered to the exact name.
    pub async fn exists(&self, name: &str) -> Result<bool> {
        Ok(self.list_names().await?.iter().any(|n| n == name))
    }

    /// Spawn `tart run` in the background. Caller owns the child and is
    /// responsible for killing it when shutting the VM down.
    pub async fn run_detached(&self, name: &str, mounts: &[MountSpec<'_>]) -> Result<Child> {
        let mut args: Vec<String> = vec!["run".into(), name.into(), "--no-graphics".into()];
        for m in mounts {
            let mut spec = format!("--dir={}:{}", m.name, m.path.display());
            if m.readonly {
                spec.push_str(":ro");
            }
            args.push(spec);
        }
        let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
        self.cli.spawn_detached(&args_ref).await
    }
}

// ---------- tests ----------

#[cfg(test)]
pub mod testing {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::VecDeque;

    #[derive(Debug, Clone)]
    pub struct ExpectedCall {
        pub args: Vec<String>,
        pub response: CapturedOutput,
    }

    #[derive(Debug, Default)]
    pub struct FakeTartCli {
        pub expected: Mutex<VecDeque<ExpectedCall>>,
        pub calls: Mutex<Vec<Vec<String>>>,
    }

    impl FakeTartCli {
        pub fn push(&self, args: &[&str], status: i32, stdout: &str, stderr: &str) {
            self.expected.lock().push_back(ExpectedCall {
                args: args.iter().map(|s| (*s).to_string()).collect(),
                response: CapturedOutput {
                    status,
                    stdout: stdout.to_string(),
                    stderr: stderr.to_string(),
                },
            });
        }
    }

    #[async_trait]
    impl TartCli for FakeTartCli {
        async fn capture(&self, args: &[&str]) -> Result<CapturedOutput> {
            let args_owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
            self.calls.lock().push(args_owned.clone());
            let expected = self
                .expected
                .lock()
                .pop_front()
                .ok_or_else(|| Error::Other(format!("unexpected tart call: {:?}", args_owned)))?;
            if expected.args != args_owned {
                return Err(Error::Other(format!(
                    "tart call mismatch: expected {:?}, got {:?}",
                    expected.args, args_owned
                )));
            }
            Ok(expected.response)
        }

        async fn spawn_detached(&self, _args: &[&str]) -> Result<Child> {
            Err(Error::Other(
                "FakeTartCli does not implement spawn_detached".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeTartCli;
    use super::*;

    #[tokio::test]
    async fn clone_passes_correct_args() {
        let fake = FakeTartCli::default();
        fake.push(&["clone", "base", "agent-1"], 0, "", "");
        let vm = Vm::new(Box::new(fake));
        vm.clone_image("base", "agent-1").await.unwrap();
    }

    #[tokio::test]
    async fn clone_propagates_tart_error() {
        let fake = FakeTartCli::default();
        fake.push(&["clone", "base", "x"], 1, "", "boom");
        let vm = Vm::new(Box::new(fake));
        let err = vm.clone_image("base", "x").await.unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn list_parses_quiet_output() {
        let fake = FakeTartCli::default();
        fake.push(&["list", "--quiet"], 0, "base\nagent-1\nagent-2\n", "");
        let vm = Vm::new(Box::new(fake));
        let names = vm.list_names().await.unwrap();
        assert_eq!(names, vec!["base", "agent-1", "agent-2"]);
    }
}

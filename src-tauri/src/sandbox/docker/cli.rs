//! Locate the docker binary and run it with hard timeouts.
//!
//! Two rules every docker invocation in the app must follow, enforced by
//! funneling them through this module:
//!
//! 1. **Resolve the binary like a GUI app.** Docker Desktop symlinks the CLI
//!    into `/usr/local/bin`, which a Finder-launched Tauri app's PATH may not
//!    include — `bin_resolve::resolve_bin` handles that (its common-dirs
//!    fallback already covers `/usr/local/bin`).
//! 2. **Bound every call.** A stopped Docker Desktop leaves a socket that
//!    accepts connections and then hangs; an unbounded `docker` call would
//!    wedge whatever thread issued it (UI polling, startup sweep). Callers
//!    pass an explicit timeout and get a clear "timed out" error instead.

use std::process::{Command, Output, Stdio};
use std::time::Duration;

use crate::error::{Error, Result};

/// Absolute path of the docker CLI, or `None` when it isn't installed.
/// Resolved fresh on every call (the underlying login-shell env is cached, so
/// this is just a stat walk): caching a `None` here would pin the probe to
/// `NotInstalled` for the whole app run even after the user installs Docker,
/// and the probe's own 5s cache already bounds the frequency.
pub(super) fn docker_bin() -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    crate::bin_resolve::resolve_bin("docker", &home).map(std::path::PathBuf::from)
}

/// Run `docker <args>` capturing stdout/stderr, failing after `timeout`.
/// Returns the raw `Output` — callers inspect the exit status themselves,
/// since several docker commands use non-zero exits as answers (e.g.
/// `image inspect` on a missing image), not as errors.
pub(super) fn run_docker(args: &[&str], timeout: Duration) -> Result<Output> {
    let bin = docker_bin()
        .ok_or_else(|| Error::Other("docker binary not found — is Docker installed?".into()))?;
    let mut cmd = Command::new(bin);
    cmd.args(args);
    let what = format!("docker {}", args.first().copied().unwrap_or_default());
    run_with_timeout(cmd, timeout, &what)
}

/// Run `docker <args>` streaming every output line (stdout and stderr) to
/// `on_line` as it appears — the shape `docker build` needs so image-build
/// progress can reach the UI. Fails on non-zero exit with the last output
/// lines in the message, or on `timeout` expiry.
pub(super) fn run_docker_streaming(
    args: &[&str],
    timeout: Duration,
    on_line: &(dyn Fn(&str) + Send + Sync),
) -> Result<()> {
    let bin = docker_bin()
        .ok_or_else(|| Error::Other("docker binary not found — is Docker installed?".into()))?;
    let what = format!("docker {}", args.first().copied().unwrap_or_default());
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout piped above");
    let stderr = child.stderr.take().expect("stderr piped above");

    // Keep a bounded tail of everything seen, so a failure message carries
    // the actual docker error (which lands near the end of the stream)
    // without buffering an entire multi-minute build log.
    let tail = std::sync::Mutex::new(std::collections::VecDeque::<String>::new());

    // Scoped threads: the readers borrow `on_line` and `tail`, and both
    // pipes are drained continuously so the child can never block on a full
    // pipe while we sit in the wait loop below.
    let status = std::thread::scope(|scope| {
        scope.spawn(|| forward_lines(stdout, on_line, &tail));
        scope.spawn(|| forward_lines(stderr, on_line, &tail));
        wait_with_deadline(&mut child, timeout, &what)
    })?;

    if !status.success() {
        let tail = tail.lock().unwrap();
        return Err(Error::Other(format!(
            "{what} failed (exit {}):\n{}",
            status.code().unwrap_or(-1),
            tail.iter().cloned().collect::<Vec<_>>().join("\n"),
        )));
    }
    Ok(())
}

/// Forward each line of `reader` to `on_line`, retaining a bounded tail for
/// error reporting.
fn forward_lines(
    reader: impl std::io::Read,
    on_line: &(dyn Fn(&str) + Send + Sync),
    tail: &std::sync::Mutex<std::collections::VecDeque<String>>,
) {
    use std::io::BufRead;
    const TAIL_LINES: usize = 20;
    for line in std::io::BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        on_line(&line);
        let mut tail = tail.lock().unwrap();
        if tail.len() == TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

/// Poll `child` until it exits or `timeout` passes; on expiry kill it so a
/// hung docker CLI (daemon gone mid-call) doesn't outlive the error we return.
fn wait_with_deadline(
    child: &mut std::process::Child,
    timeout: Duration,
    what: &str,
) -> Result<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait(); // reap; readers see EOF and finish
            return Err(timeout_error(what, timeout));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run any command to completion under `timeout`, capturing output. Both
/// pipes are drained on scoped threads (so a chatty child can't deadlock on
/// a full pipe) while this thread keeps ownership of the `Child` and waits
/// with a deadline — on expiry [`wait_with_deadline`] kills and reaps it on
/// any platform, the pipes hit EOF, and the readers finish: nothing outlives
/// the error we return.
fn run_with_timeout(mut cmd: Command, timeout: Duration, what: &str) -> Result<Output> {
    fn read_all(mut reader: impl std::io::Read) -> Vec<u8> {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("stdout piped above");
    let stderr = child.stderr.take().expect("stderr piped above");
    std::thread::scope(|scope| {
        let stdout = scope.spawn(move || read_all(stdout));
        let stderr = scope.spawn(move || read_all(stderr));
        let status = wait_with_deadline(&mut child, timeout, what)?;
        Ok(Output {
            status,
            stdout: stdout.join().expect("stdout reader panicked"),
            stderr: stderr.join().expect("stderr reader panicked"),
        })
    })
}

fn timeout_error(what: &str, timeout: Duration) -> Error {
    Error::Other(format!(
        "{what} timed out after {:.0?} — is the Docker daemon responding?",
        timeout,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property every docker invocation relies on: a hung child comes
    /// back as a bounded error, and the child is killed rather than orphaned.
    #[test]
    fn run_with_timeout_kills_hung_child() {
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("30");
        let started = std::time::Instant::now();
        let err = run_with_timeout(cmd, Duration::from_millis(200), "sleep")
            .expect_err("a 30s sleep must trip a 200ms timeout");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout must not wait for the child's natural exit",
        );
        assert!(err.to_string().contains("timed out"), "got: {err}");
    }

    #[test]
    fn run_with_timeout_captures_output_of_fast_child() {
        let mut cmd = Command::new("/bin/echo");
        cmd.arg("hello");
        let out = run_with_timeout(cmd, Duration::from_secs(5), "echo").unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    /// Streaming runs deliver lines to the callback and fail loudly (with the
    /// output tail) on a non-zero exit — the contract `docker build` needs.
    #[test]
    fn streaming_forwards_lines_and_reports_failure_tail() {
        let lines = std::sync::Mutex::new(Vec::new());
        let on_line = |line: &str| lines.lock().unwrap().push(line.to_string());

        // Use /bin/sh directly (not via docker) to exercise the machinery.
        let mut child = Command::new("/bin/sh")
            .args(["-c", "echo one; echo two >&2; exit 3"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let tail = std::sync::Mutex::new(std::collections::VecDeque::new());
        let status = std::thread::scope(|scope| {
            scope.spawn(|| forward_lines(stdout, &on_line, &tail));
            scope.spawn(|| forward_lines(stderr, &on_line, &tail));
            wait_with_deadline(&mut child, Duration::from_secs(5), "sh")
        })
        .unwrap();

        assert_eq!(status.code(), Some(3));
        let mut seen = lines.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, vec!["one".to_string(), "two".to_string()]);
        assert_eq!(tail.lock().unwrap().len(), 2);
    }
}

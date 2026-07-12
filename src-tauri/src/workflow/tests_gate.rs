//! Tests-gate execution (spec §9.4). The `tests` gate resolves the project's
//! test command (project override > `run_detect`), runs it — after a one-time
//! setup/install command — bounded and sandboxed inside the step worktree under
//! the Run-panel seatbelt profile, and reports a [`gates::TestsOutcome`] that the
//! pure `gates::evaluate` turns into done/blocked. No test command resolvable →
//! [`TestsOutcome::NoCommand`], and the gate degrades to `verdict` (§9.4).
//!
//! Execution lives behind the [`TestRunner`] trait so `attempt.rs` stays
//! unit-testable with a scripted mock — the same seam `AgentDriver` provides.
//! The pure resolution and output-tail helpers are tested directly, and the
//! bounded runner is exercised with plain shell commands (green / red / timeout
//! / unrunnable) so the fast tests need no sandbox.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::error::{Error, Result};

use super::driver::BoxFuture;
use super::gates::TestsOutcome;

/// Lines of output kept from a failed / timed-out run for the journal and the
/// re-prompt (spec §9.4 keeps the last 100).
const OUTPUT_TAIL_LINES: usize = 100;

/// Default bound on a single test (or setup) run (spec §11.1 `tests_timeout_secs`).
pub const DEFAULT_TESTS_TIMEOUT_SECS: u64 = 900;

/// Resolves and runs the `tests` gate. Behind a trait so the attempt lifecycle
/// is unit-testable with a scripted mock (`AgentDriver`'s pattern).
pub trait TestRunner: Send + Sync {
    /// Resolve and run the project's tests in `worktree`, returning the outcome.
    fn run_tests<'a>(&'a self, worktree: &'a Path) -> BoxFuture<'a, TestsOutcome>;
}

/// The production runner: resolves the command via `run_detect` (project
/// overrides winning), runs the setup command once per workspace, then runs the
/// test command — each `sandbox-exec`-wrapped under the Run-panel profile.
pub struct SandboxTestRunner {
    /// Project override for the test command (`run.test`); wins over detection.
    test_override: Option<String>,
    /// Project override for the setup command (`run.install`); wins over detection.
    setup_override: Option<String>,
    home: PathBuf,
    timeout: Duration,
    /// Worktrees whose setup command has already succeeded — setup runs once per
    /// workspace (spec §9.4). A fresh clone has no deps, but by gate time the
    /// step agent has usually installed them, so this is normally a fast no-op.
    setup_done: Mutex<HashSet<PathBuf>>,
}

impl SandboxTestRunner {
    pub fn new(
        test_override: Option<String>,
        setup_override: Option<String>,
        timeout_secs: u64,
    ) -> Result<Self> {
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        Ok(Self {
            test_override,
            setup_override,
            home,
            timeout: Duration::from_secs(timeout_secs),
            setup_done: Mutex::new(HashSet::new()),
        })
    }

    /// Build the `sandbox-exec -f <profile> <shell> -lic <cmd>` invocation under
    /// the Run-panel profile (writable root = the step worktree). The profile
    /// tempfile is returned so it outlives the child's `exec`; the caller keeps
    /// it alive until the process has finished.
    fn sandbox_command(
        &self,
        worktree: &Path,
        cmd: &str,
    ) -> Result<(PathBuf, Vec<String>, tempfile::NamedTempFile)> {
        let profile_text = crate::sandbox::build_run_profile(worktree, &self.home, &[])?;
        let profile_file = crate::sandbox::profile_tempfile(&profile_text)?;
        let profile_path = profile_file
            .path()
            .to_str()
            .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
            .to_string();
        let shell = crate::run_session::user_shell();
        let shell_str = shell
            .to_str()
            .ok_or_else(|| Error::Other("shell path not utf-8".into()))?
            .to_string();
        let mut args = vec!["-f".to_string(), profile_path, shell_str];
        args.extend(crate::run_session::shell_args(cmd));
        Ok((
            PathBuf::from(crate::sandbox::SANDBOX_EXEC),
            args,
            profile_file,
        ))
    }

    async fn run_sandboxed(&self, worktree: &Path, cmd: &str) -> Bounded {
        let (program, args, _profile) = match self.sandbox_command(worktree, cmd) {
            Ok(t) => t,
            Err(e) => return Bounded::Unrunnable(format!("could not build sandbox profile: {e}")),
        };
        // `_profile` must outlive the child's `exec`; it drops at the end of this
        // fn, after `run_bounded` has awaited the process to completion.
        run_bounded(&program, &args, worktree, self.timeout).await
    }
}

impl TestRunner for SandboxTestRunner {
    fn run_tests<'a>(&'a self, worktree: &'a Path) -> BoxFuture<'a, TestsOutcome> {
        Box::pin(async move {
            let Some(resolved) = resolve(worktree, &self.test_override, &self.setup_override)
            else {
                return TestsOutcome::NoCommand;
            };

            // Setup once per workspace, distinct failure from failing tests.
            if let Some(setup) = &resolved.setup {
                let already = self.setup_done.lock().unwrap().contains(worktree);
                if !already {
                    match self.run_sandboxed(worktree, setup).await {
                        Bounded::Exited { success: true, .. } => {
                            self.setup_done
                                .lock()
                                .unwrap()
                                .insert(worktree.to_path_buf());
                        }
                        Bounded::Exited {
                            success: false,
                            tail,
                        } => return TestsOutcome::SetupFailed { tail },
                        Bounded::TimedOut => {
                            return TestsOutcome::SetupFailed {
                                tail: self.timeout_note("setup command"),
                            }
                        }
                        Bounded::Unrunnable(e) => return TestsOutcome::SetupFailed { tail: e },
                    }
                }
            }

            match self.run_sandboxed(worktree, &resolved.test).await {
                Bounded::Exited { success: true, .. } => TestsOutcome::Passed,
                Bounded::Exited {
                    success: false,
                    tail,
                } => TestsOutcome::Failed { tail },
                Bounded::TimedOut => TestsOutcome::TimedOut {
                    tail: self.timeout_note("test command"),
                },
                Bounded::Unrunnable(e) => TestsOutcome::Failed { tail: e },
            }
        })
    }
}

impl SandboxTestRunner {
    fn timeout_note(&self, what: &str) -> String {
        format!(
            "{what} exceeded the {}s limit and was terminated",
            self.timeout.as_secs()
        )
    }
}

/// The test + setup commands resolved for a worktree.
struct Resolved {
    test: String,
    setup: Option<String>,
}

/// Resolve the test and setup commands for `worktree`: a non-empty override
/// wins, else the highest-confidence detected config's `test` / `install` rows
/// (spec §9.4). `None` when no test command can be found (→ degrade to verdict).
fn resolve(
    worktree: &Path,
    test_override: &Option<String>,
    setup_override: &Option<String>,
) -> Option<Resolved> {
    let configs = crate::run_detect::detect_all(worktree);
    let detected = |id: &str| -> Option<String> {
        configs
            .first()
            .and_then(|c| c.rows.iter().find(|r| r.id == id))
            .map(|r| r.value.clone())
    };
    let test =
        nonempty(test_override.clone()).or_else(|| detected("test").and_then(some_nonempty))?;
    let setup =
        nonempty(setup_override.clone()).or_else(|| detected("install").and_then(some_nonempty));
    Some(Resolved { test, setup })
}

/// `Some(trimmed)` when the value is present and non-blank, else `None`.
fn nonempty(v: Option<String>) -> Option<String> {
    v.and_then(some_nonempty)
}

fn some_nonempty(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The bounded outcome of running one command.
enum Bounded {
    /// The process exited; `success` is exit status 0. `tail` is its output tail.
    Exited { success: bool, tail: String },
    /// The process did not finish within the deadline (killed).
    TimedOut,
    /// The command could not be launched at all.
    Unrunnable(String),
}

/// Run `program args` in `cwd`, capturing combined stdout+stderr, bounded by
/// `deadline`. On timeout the child is killed (`kill_on_drop`) and `TimedOut` is
/// returned. Pure of any workflow state so it is directly unit-testable.
async fn run_bounded(program: &Path, args: &[String], cwd: &Path, deadline: Duration) -> Bounded {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => return Bounded::Unrunnable(format!("could not launch command: {e}")),
    };
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    // Read both pipes concurrently (so a full pipe buffer can't deadlock the
    // child) and reap it. `read_to_end` completes when the process closes its
    // fds, i.e. at exit.
    let collect = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let _ = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        let status = child.wait().await;
        (status, out, err)
    };

    match timeout(deadline, collect).await {
        Ok((status, mut out, err)) => {
            out.extend_from_slice(&err);
            let tail = tail_lines(&String::from_utf8_lossy(&out), OUTPUT_TAIL_LINES);
            match status {
                Ok(s) => Bounded::Exited {
                    success: s.success(),
                    tail,
                },
                Err(e) => Bounded::Unrunnable(format!("command errored: {e}")),
            }
        }
        // `collect` drops here; `child` drops at fn end → killed via kill_on_drop.
        Err(_elapsed) => Bounded::TimedOut,
    }
}

/// The last `n` lines of `text`, joined with `\n`.
fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn resolve_prefers_override_over_detection() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"test":"vitest"}}"#,
        );
        let r = resolve(
            dir.path(),
            &Some("cargo test --all".into()),
            &Some("cargo fetch".into()),
        )
        .unwrap();
        assert_eq!(r.test, "cargo test --all");
        assert_eq!(r.setup.as_deref(), Some("cargo fetch"));
    }

    #[test]
    fn resolve_detects_test_and_install_commands() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"test":"vitest"}}"#,
        );
        let r = resolve(dir.path(), &None, &None).unwrap();
        assert_eq!(r.test, "npm test");
        assert_eq!(r.setup.as_deref(), Some("npm install"));
    }

    #[test]
    fn resolve_none_when_no_command_found() {
        let dir = tempfile::tempdir().unwrap();
        // No manifest, no override → nothing to run → degrade to verdict.
        assert!(resolve(dir.path(), &None, &None).is_none());
    }

    #[test]
    fn resolve_blank_override_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve(dir.path(), &Some("   ".into()), &None).is_none());
    }

    #[tokio::test]
    async fn run_bounded_reports_success() {
        let cwd = tempfile::tempdir().unwrap();
        let b = run_bounded(
            Path::new("/bin/sh"),
            &["-c".into(), "echo ok; exit 0".into()],
            cwd.path(),
            Duration::from_secs(10),
        )
        .await;
        match b {
            Bounded::Exited { success, tail } => {
                assert!(success);
                assert!(tail.contains("ok"), "tail: {tail}");
            }
            _ => panic!("expected Exited"),
        }
    }

    #[tokio::test]
    async fn run_bounded_captures_failure_tail() {
        let cwd = tempfile::tempdir().unwrap();
        let b = run_bounded(
            Path::new("/bin/sh"),
            &["-c".into(), "echo boom 1>&2; exit 3".into()],
            cwd.path(),
            Duration::from_secs(10),
        )
        .await;
        match b {
            Bounded::Exited { success, tail } => {
                assert!(!success);
                assert!(tail.contains("boom"), "tail: {tail}");
            }
            _ => panic!("expected Exited"),
        }
    }

    #[tokio::test]
    async fn run_bounded_times_out() {
        let cwd = tempfile::tempdir().unwrap();
        let b = run_bounded(
            Path::new("/bin/sh"),
            &["-c".into(), "sleep 30".into()],
            cwd.path(),
            Duration::from_millis(150),
        )
        .await;
        assert!(matches!(b, Bounded::TimedOut));
    }

    #[tokio::test]
    async fn run_bounded_reports_unrunnable() {
        let cwd = tempfile::tempdir().unwrap();
        let b = run_bounded(
            Path::new("/nonexistent/definitely/not/here"),
            &[],
            cwd.path(),
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(b, Bounded::Unrunnable(_)));
    }

    #[test]
    fn tail_lines_keeps_last_n() {
        let text = (1..=250)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let tail = tail_lines(&text, 100);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines.len(), 100);
        assert_eq!(lines.first(), Some(&"151"));
        assert_eq!(lines.last(), Some(&"250"));
    }

    #[test]
    fn tail_lines_shorter_than_n_is_whole() {
        assert_eq!(tail_lines("a\nb", 100), "a\nb");
    }
}

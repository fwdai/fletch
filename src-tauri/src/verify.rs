//! Engine-owned deterministic verification — a reusable primitive.
//!
//! Verification runs a project's own checks (`install` → `test` → `lint`) in a
//! checkout, each bounded and sandboxed under the Run-panel seatbelt profile,
//! and reports a [`VerificationReport`]: one [`CheckResult`] per check with its
//! outcome, timing, and output tail. It is the shared core behind two callers:
//!
//! * the workflow `tests` gate (spec §9.4), via the thin
//!   [`crate::workflow::tests_gate`] adapter, which asks for a *tests-only*
//!   report and maps it back to a `TestsOutcome`; and
//! * ad-hoc agent checkouts, via the `run_verification` Tauri command, which
//!   surfaces the full report (lint included) to the UI.
//!
//! Invariants worth preserving:
//! * **Setup once per worktree.** The `install` command runs at most once per
//!   worktree per [`Verifier`] (spec §9.4); the workflow scheduler holds one
//!   `Verifier` across an attempt's gate evaluations, so a re-prompt does not
//!   re-install. A fresh `Verifier` (as the Tauri command builds) always
//!   installs once.
//! * **Same ecosystem for install + test.** Detected commands come from the
//!   highest-confidence config that actually defines a `test` row, and its
//!   `install` pairs with it — so a mixed repo whose primary ecosystem has no
//!   test script still runs a sibling's tests rather than degrading. `lint` is
//!   resolved independently (its own highest-confidence config with a `lint`
//!   row) so an ad-hoc lint check works even where no test command exists.
//! * **Project overrides win.** A non-empty `run.test` / `run.install` /
//!   `run.lint` override beats detection, same layering as the tests gate.
//! * **Graceful degrade off-macOS.** The seatbelt profile needs `sandbox-exec`;
//!   where it's absent every check is `Skipped` rather than run unsandboxed.
//!
//! The resolution and bounded-runner helpers are pure of any Tauri/DB state, so
//! they are unit-tested directly with plain shell commands (green / red /
//! timeout / unrunnable) and fixture directories — no sandbox required.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::error::{Error, Result};

/// Lines of output kept from a check's run for the report, the journal, and the
/// re-prompt (spec §9.4 keeps the last 100).
const OUTPUT_TAIL_LINES: usize = 100;

/// The result of running the project's checks in a checkout. Serde-serialized
/// (snake_case, matching the frontend's other IPC payloads) for the
/// `run_verification` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerificationReport {
    pub checks: Vec<CheckResult>,
}

impl VerificationReport {
    /// Whether verification is clean: no check failed, timed out, or was blocked
    /// by a failed setup. `Skipped` (no command resolved) counts as clean —
    /// there was nothing to run, not a failure.
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|c| {
            matches!(
                c.outcome,
                CheckOutcome::Passed | CheckOutcome::Skipped
            )
        })
    }

    /// The check with the given `name` (`"install"` | `"test"` | `"lint"`).
    pub fn check(&self, name: &str) -> Option<&CheckResult> {
        self.checks.iter().find(|c| c.name == name)
    }
}

/// One check's outcome inside a [`VerificationReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    /// Stable check id: `"install"` | `"test"` | `"lint"`.
    pub name: String,
    /// The command that ran (or would have), e.g. `"npm test"`. Empty when the
    /// check was skipped for lack of a resolvable command.
    pub command: String,
    pub outcome: CheckOutcome,
    /// Wall-clock duration of the command, 0 when it didn't run.
    pub duration_ms: u64,
    /// Last [`OUTPUT_TAIL_LINES`] lines of combined stdout+stderr (empty on
    /// success or when nothing ran).
    pub tail: Vec<String>,
}

/// The terminal shape of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    /// The command exited 0.
    Passed,
    /// The command exited non-zero (or could not be launched).
    Failed,
    /// The command exceeded the timeout and was terminated.
    TimedOut,
    /// A prerequisite setup/install command failed, so this check never ran.
    SetupFailed,
    /// No command could be resolved for this check (nothing to run).
    Skipped,
}

/// The reusable verification primitive. Holds the resolved project overrides,
/// the seatbelt profile inputs, and the per-worktree setup-done cache. Cheap to
/// construct; one instance may verify several worktrees.
pub struct Verifier {
    /// Project override for the test command (`run.test`); wins over detection.
    test_override: Option<String>,
    /// Project override for the setup command (`run.install`); wins over detection.
    setup_override: Option<String>,
    /// Project override for the lint command (`run.lint`); wins over detection.
    lint_override: Option<String>,
    home: PathBuf,
    timeout: Duration,
    /// Worktrees whose setup command has already succeeded — setup runs once per
    /// worktree (spec §9.4).
    setup_done: Mutex<HashSet<PathBuf>>,
}

impl Verifier {
    pub fn new(
        test_override: Option<String>,
        setup_override: Option<String>,
        lint_override: Option<String>,
        timeout_secs: u64,
    ) -> Result<Self> {
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        Ok(Self {
            test_override,
            setup_override,
            lint_override,
            home,
            timeout: Duration::from_secs(timeout_secs),
            setup_done: Mutex::new(HashSet::new()),
        })
    }

    /// Run the full suite — `install` (once) → `test` → `lint` — and report every
    /// check. Used by the ad-hoc `run_verification` command.
    pub async fn verify(&self, worktree: &Path) -> VerificationReport {
        self.run(worktree, /* include_lint = */ true).await
    }

    /// Run only `install` (once) → `test`, for the workflow tests gate, which has
    /// no use for a lint result and must stay byte-identical to its prior shape.
    pub async fn verify_tests_only(&self, worktree: &Path) -> VerificationReport {
        self.run(worktree, /* include_lint = */ false).await
    }

    async fn run(&self, worktree: &Path, include_lint: bool) -> VerificationReport {
        // The checks run under the Run-panel seatbelt profile, which needs macOS
        // `sandbox-exec`. Where it isn't present we can't safely run a
        // repo-derived command, so skip every check rather than run unsandboxed
        // (the tests gate then degrades to the verdict gate — spec §9.4).
        if !sandbox_available() {
            tracing::warn!(
                "verify: `{}` unavailable on this host; skipping checks",
                crate::sandbox::SANDBOX_EXEC
            );
            let mut checks = vec![skipped("test", "")];
            if include_lint {
                checks.push(skipped("lint", ""));
            }
            return VerificationReport { checks };
        }

        let resolved = resolve(
            worktree,
            &self.test_override,
            &self.setup_override,
            &self.lint_override,
        );

        // Nothing to verify: no test and (for the full suite) no lint. Report the
        // requested checks as skipped so the shape is stable; the tests-gate
        // mapping turns a skipped test into `NoCommand`.
        let will_run = resolved.test.is_some() || (include_lint && resolved.lint.is_some());
        if !will_run {
            let mut checks = vec![skipped("test", "")];
            if include_lint {
                checks.push(skipped("lint", ""));
            }
            return VerificationReport { checks };
        }

        let mut checks = Vec::new();

        // Setup once per worktree, a distinct failure from a failing check.
        let mut setup_failed = false;
        if let Some(setup) = &resolved.install {
            let already = self.setup_done.lock().unwrap().contains(worktree);
            if already {
                checks.push(passed_cached("install", setup));
            } else {
                let (outcome, duration_ms, tail) =
                    self.run_check(worktree, "install", setup).await;
                if matches!(outcome, CheckOutcome::Passed) {
                    self.setup_done
                        .lock()
                        .unwrap()
                        .insert(worktree.to_path_buf());
                } else {
                    setup_failed = true;
                }
                checks.push(CheckResult {
                    name: "install".to_string(),
                    command: setup.clone(),
                    outcome,
                    duration_ms,
                    tail,
                });
            }
        }

        // test, then (optionally) lint — each skipped when unresolved, or marked
        // `SetupFailed` when the install above failed so they never ran.
        let mut order: Vec<(&str, &Option<String>)> = vec![("test", &resolved.test)];
        if include_lint {
            order.push(("lint", &resolved.lint));
        }
        for (name, cmd) in order {
            match cmd {
                None => checks.push(skipped(name, "")),
                Some(cmd) if setup_failed => checks.push(CheckResult {
                    name: name.to_string(),
                    command: cmd.clone(),
                    outcome: CheckOutcome::SetupFailed,
                    duration_ms: 0,
                    tail: Vec::new(),
                }),
                Some(cmd) => {
                    let (outcome, duration_ms, tail) = self.run_check(worktree, name, cmd).await;
                    checks.push(CheckResult {
                        name: name.to_string(),
                        command: cmd.clone(),
                        outcome,
                        duration_ms,
                        tail,
                    });
                }
            }
        }

        VerificationReport { checks }
    }

    /// Run one check's command, mapping the bounded result to a [`CheckOutcome`]
    /// plus its duration and output tail. `what` labels the synthetic timeout
    /// note (`"setup command"` for install, `"test command"`, `"lint command"`).
    async fn run_check(
        &self,
        worktree: &Path,
        name: &str,
        cmd: &str,
    ) -> (CheckOutcome, u64, Vec<String>) {
        let started = Instant::now();
        let bounded = self.run_sandboxed(worktree, cmd).await;
        let duration_ms = started.elapsed().as_millis() as u64;
        let outcome_tail = match bounded {
            Bounded::Exited { success: true, .. } => (CheckOutcome::Passed, Vec::new()),
            Bounded::Exited {
                success: false,
                tail,
            } => (CheckOutcome::Failed, tail),
            Bounded::TimedOut => (CheckOutcome::TimedOut, vec![self.timeout_note(name)]),
            Bounded::Unrunnable(e) => (CheckOutcome::Failed, vec![e]),
        };
        (outcome_tail.0, duration_ms, outcome_tail.1)
    }

    /// The synthetic tail line for a timed-out check. Preserves the exact wording
    /// the tests gate has always journaled: `install` → "setup command …".
    fn timeout_note(&self, name: &str) -> String {
        let what = match name {
            "install" => "setup command",
            "lint" => "lint command",
            _ => "test command",
        };
        format!(
            "{what} exceeded the {}s limit and was terminated",
            self.timeout.as_secs()
        )
    }

    /// Build the `sandbox-exec -f <profile> <shell> -lic <cmd>` invocation under
    /// the Run-panel profile (writable root = the checkout). The profile tempfile
    /// is returned so it outlives the child's `exec`; the caller keeps it alive
    /// until the process has finished.
    fn sandbox_command(
        &self,
        worktree: &Path,
        cmd: &str,
    ) -> Result<(PathBuf, Vec<String>, tempfile::NamedTempFile)> {
        // Grant the target's git *common dir* (as the Run panel does) so commands
        // that touch git — e.g. `git worktree add` — work in a linked worktree
        // checkout, whose common dir lives outside `worktree`. For a plain
        // `--shared` clone this is `<worktree>/.git`, already writable, so the
        // grant is harmless.
        let extra_writable: Vec<PathBuf> =
            crate::supervisor::run::run_target_git_common_dir(worktree)
                .into_iter()
                .collect();
        let profile_text =
            crate::sandbox::build_run_profile(worktree, &self.home, &extra_writable)?;
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

/// A `Skipped` check with no command resolved.
fn skipped(name: &str, command: &str) -> CheckResult {
    CheckResult {
        name: name.to_string(),
        command: command.to_string(),
        outcome: CheckOutcome::Skipped,
        duration_ms: 0,
        tail: Vec::new(),
    }
}

/// A `Passed` install check that was already satisfied earlier this worktree
/// (setup-once): no work done this call, so 0 duration and no tail.
fn passed_cached(name: &str, command: &str) -> CheckResult {
    CheckResult {
        name: name.to_string(),
        command: command.to_string(),
        outcome: CheckOutcome::Passed,
        duration_ms: 0,
        tail: Vec::new(),
    }
}

/// The install / test / lint commands resolved for a worktree. Any may be
/// `None` (no override, nothing detected).
struct Resolved {
    install: Option<String>,
    test: Option<String>,
    lint: Option<String>,
}

/// Resolve the install, test, and lint commands for `worktree`: a non-empty
/// override wins, else the detected command (spec §9.4).
///
/// Detected `test` + `install` are taken from a single ecosystem: the
/// highest-confidence config that actually defines a `test` row (`detect_all` is
/// confidence-sorted). This keeps `test` and `install` from the same ecosystem
/// and lets a lower-ranked detector supply the test command when the top one has
/// none. `lint` is resolved independently — its own highest-confidence config
/// with a `lint` row — so an ad-hoc lint check works even where no test exists.
fn resolve(
    worktree: &Path,
    test_override: &Option<String>,
    setup_override: &Option<String>,
    lint_override: &Option<String>,
) -> Resolved {
    let configs = crate::run_detect::detect_all(worktree);
    let row = |c: &crate::run_detect::DetectedConfig, id: &str| -> Option<String> {
        c.rows.iter().find(|r| r.id == id).map(|r| r.value.clone())
    };
    // The ecosystem we take detected test/install from. Its `install` pairs with
    // its `test`; for an override test with no detected test config, setup falls
    // back to the highest-confidence config.
    let test_config = configs.iter().find(|c| row(c, "test").is_some());
    let setup_config = test_config.or_else(|| configs.first());

    let test = nonempty(test_override.clone()).or_else(|| {
        test_config
            .and_then(|c| row(c, "test"))
            .and_then(some_nonempty)
    });
    let install = nonempty(setup_override.clone()).or_else(|| {
        setup_config
            .and_then(|c| row(c, "install"))
            .and_then(some_nonempty)
    });
    let lint = nonempty(lint_override.clone()).or_else(|| {
        configs
            .iter()
            .find_map(|c| row(c, "lint"))
            .and_then(some_nonempty)
    });
    Resolved {
        install,
        test,
        lint,
    }
}

/// Whether the seatbelt sandbox binary the checks run under is present (macOS).
fn sandbox_available() -> bool {
    Path::new(crate::sandbox::SANDBOX_EXEC).exists()
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
    Exited { success: bool, tail: Vec<String> },
    /// The process did not finish within the deadline (killed).
    TimedOut,
    /// The command could not be launched at all.
    Unrunnable(String),
}

/// Run `program args` in `cwd`, capturing combined stdout+stderr, bounded by
/// `deadline`. On timeout the child is killed (`kill_on_drop`) and `TimedOut` is
/// returned. Pure of any workflow/Tauri state so it is directly unit-testable.
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

/// The last `n` lines of `text`, as owned strings.
fn tail_lines(text: &str, n: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    // ── resolution ────────────────────────────────────────────────────────

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
            &None,
        );
        assert_eq!(r.test.as_deref(), Some("cargo test --all"));
        assert_eq!(r.install.as_deref(), Some("cargo fetch"));
    }

    #[test]
    fn resolve_detects_test_and_install_commands() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"test":"vitest"}}"#,
        );
        let r = resolve(dir.path(), &None, &None, &None);
        assert_eq!(r.test.as_deref(), Some("npm test"));
        assert_eq!(r.install.as_deref(), Some("npm install"));
    }

    #[test]
    fn resolve_detects_lint_command() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"test":"vitest","lint":"eslint ."}}"#,
        );
        let r = resolve(dir.path(), &None, &None, &None);
        assert_eq!(r.lint.as_deref(), Some("npm run lint"));
    }

    #[test]
    fn resolve_lint_override_wins() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "package.json", r#"{"scripts":{"lint":"eslint"}}"#);
        let r = resolve(dir.path(), &None, &None, &Some("biome check".into()));
        assert_eq!(r.lint.as_deref(), Some("biome check"));
    }

    #[test]
    fn resolve_no_lint_row_is_none() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "package.json", r#"{"scripts":{"test":"vitest"}}"#);
        let r = resolve(dir.path(), &None, &None, &None);
        assert!(r.lint.is_none());
    }

    #[test]
    fn resolve_uses_lower_ranked_config_when_top_has_no_test() {
        // Node (lockfile → 90, ranks first) has no `test` script, but Go (also
        // 90, ranked after node) always defines one. Verification must run Go's
        // tests rather than skip, paired with Go's install.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "package.json", "{}");
        write(dir.path(), "pnpm-lock.yaml", "");
        write(dir.path(), "go.mod", "module example.com/x\n\ngo 1.22\n");
        write(dir.path(), "go.sum", "");
        let r = resolve(dir.path(), &None, &None, &None);
        assert_eq!(r.test.as_deref(), Some("go test ./..."));
        assert_eq!(r.install.as_deref(), Some("go mod download"));
    }

    #[test]
    fn resolve_none_when_no_command_found() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolve(dir.path(), &None, &None, &None);
        assert!(r.test.is_none());
        assert!(r.install.is_none());
        assert!(r.lint.is_none());
    }

    #[test]
    fn resolve_blank_override_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolve(dir.path(), &Some("   ".into()), &None, &None);
        assert!(r.test.is_none());
    }

    // ── report helpers ──────────────────────────────────────────────────────

    #[test]
    fn passed_ignores_skipped_but_fails_on_failure() {
        let clean = VerificationReport {
            checks: vec![skipped("test", ""), skipped("lint", "")],
        };
        assert!(clean.passed());

        let failed = VerificationReport {
            checks: vec![CheckResult {
                name: "test".into(),
                command: "x".into(),
                outcome: CheckOutcome::Failed,
                duration_ms: 1,
                tail: vec!["boom".into()],
            }],
        };
        assert!(!failed.passed());
    }

    #[test]
    fn check_outcome_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&CheckOutcome::TimedOut).unwrap(),
            "\"timed_out\""
        );
        assert_eq!(
            serde_json::to_string(&CheckOutcome::SetupFailed).unwrap(),
            "\"setup_failed\""
        );
    }

    // ── bounded runner ────────────────────────────────────────────────────

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
                assert!(tail.iter().any(|l| l.contains("ok")), "tail: {tail:?}");
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
                assert!(tail.iter().any(|l| l.contains("boom")), "tail: {tail:?}");
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
        assert_eq!(tail.len(), 100);
        assert_eq!(tail.first().map(String::as_str), Some("151"));
        assert_eq!(tail.last().map(String::as_str), Some("250"));
    }

    #[test]
    fn tail_lines_shorter_than_n_is_whole() {
        assert_eq!(tail_lines("a\nb", 100), vec!["a".to_string(), "b".to_string()]);
    }
}

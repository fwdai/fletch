//! Provider binary resolution and `--version` probing.

use std::path::PathBuf;

use crate::error::{Error, Result};

use super::capabilities::{provider_bin_label, PER_TURN_AGENTS};

/// The probed CLI version for a provider (`v1.2.3`), memoized per process so the
/// `--version` subprocess runs at most once per provider. Stamped onto
/// session_records at ingest so read-time normalizers can branch by version
/// when a vendor format changes. `None` if the binary is missing, unparseable,
/// or did not answer inside [`VERSION_TIMEOUT`] — ingest is not somewhere a
/// wedged vendor CLI gets to stop.
pub fn cached_provider_version(provider: &str) -> Option<String> {
    static CACHE: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<String, Option<String>>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()));
    if let Some(v) = cache.lock().get(provider) {
        return v.clone();
    }
    let version = provider_bin_label(provider).and_then(|(bin, label)| {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        resolve_agent_bin(provider, bin, label, &home)
            .ok()
            .and_then(|p| probe_version(&p))
    });
    cache.lock().insert(provider.to_string(), version.clone());
    version
}

/// Locate an agent CLI by name: PATH first, then the user's login shell
/// (catches nvm / fnm / volta / homebrew setups the GUI process's bare
/// PATH misses), then the usual install dirs. `label` is the
/// human-facing product name used only in the not-found error.
pub fn resolve_agent_bin(
    agent_id: &str,
    name: &str,
    label: &str,
    home: &std::path::Path,
) -> Result<String> {
    // A user-set custom path wins over PATH discovery. If it no longer points
    // at an executable we surface a clear error rather than silently falling
    // back to a different binary off PATH — the user chose this one explicitly.
    if let Some(result) = crate::bin_resolve::resolve_agent_override(agent_id, home) {
        return result.map_err(|path| {
            Error::Other(format!(
                "The custom binary path for {label} is not executable: {path}"
            ))
        });
    }
    crate::bin_resolve::resolve_bin(name, home).ok_or_else(|| {
        Error::Other(format!(
            "Could not find the `{name}` executable. Install {label} or make it available on PATH."
        ))
    })
}

// ── Version probing ───────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct ProviderProbe {
    pub id: String,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(serde::Serialize)]
pub struct BinValidation {
    /// The path is an executable regular file (after `~` expansion).
    pub executable: bool,
    /// The version `<path> --version` reported, if it ran and parsed.
    pub version: Option<String>,
}

/// Pre-flight a user-entered custom binary path before it's saved as an
/// override: expand a leading `~`, confirm it's an executable file, and probe
/// `--version` when it is. Powers the providers settings UI's inline feedback.
pub fn validate_bin(path: &str) -> BinValidation {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let expanded = crate::bin_resolve::expand_tilde(path, &home);
    let executable = crate::bin_resolve::is_executable_path(&expanded);
    let version = if executable {
        probe_version(&expanded.to_string_lossy())
    } else {
        None
    };
    BinValidation {
        executable,
        version,
    }
}

#[derive(serde::Serialize)]
pub struct ToolStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    /// `"system"` or `"portable"` for git (see `git_dist`); `None` for plain
    /// PATH-resolved tools.
    pub source: Option<String>,
}

/// Resolve a plain CLI on PATH and probe its `--version`. Used by the
/// first-run readiness check for required tools that aren't agent providers.
/// `git` goes through `git_dist` instead: presence isn't enough there (the
/// macOS CLT shim exists but doesn't run) and a portable install counts.
pub fn check_cli(name: &str) -> ToolStatus {
    if name == "git" {
        return crate::git_dist::tool_status();
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let path = crate::bin_resolve::resolve_bin(name, &home);
    let version = path.as_deref().and_then(probe_version);
    ToolStatus {
        installed: path.is_some(),
        version,
        path,
        source: None,
    }
}

/// Probe every known provider in parallel and return their resolved path +
/// version string. Missing/uninstalled providers return `None` for both fields;
/// the frontend falls back to the hardcoded defaults in that case.
///
/// Every `--version` here is bounded (see [`VERSION_TIMEOUT`]), so one wedged
/// CLI — or a custom binary path pointed at something that never exits — costs
/// a slow probe rather than a caller that never returns.
pub async fn probe_all_providers() -> Vec<ProviderProbe> {
    probe_providers(true).await
}

/// The same set, resolved but never *run*: no `--version`, no subprocess at
/// all. For callers that only need to know whether a provider's CLI is on this
/// machine (`fletch-host status`), where executing six vendor binaries to
/// answer that would be both wasteful and a way to hang.
pub async fn resolve_all_providers() -> Vec<ProviderProbe> {
    probe_providers(false).await
}

async fn probe_providers(with_version: bool) -> Vec<ProviderProbe> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

    // (id, bin_name, human_label)
    let mut targets: Vec<(&str, &str, &str)> = vec![("claude", "claude", "Claude Code")];
    for d in PER_TURN_AGENTS {
        targets.push((d.id, d.bin, d.label));
    }

    let mut handles = Vec::new();
    for (id, bin, label) in targets {
        let home = home.clone();
        let id = id.to_string();
        let bin = bin.to_string();
        let label = label.to_string();
        handles.push(tokio::spawn(async move {
            let resolving = id.clone();
            // Resolution touches the filesystem (and, once per process, the
            // login shell), so it stays on the blocking pool.
            let path = tokio::task::spawn_blocking(move || {
                resolve_agent_bin(&resolving, &bin, &label, &home).ok()
            })
            .await
            .ok()
            .flatten();
            let version = match path.as_deref() {
                Some(path) if with_version => probe_version_bounded(path).await,
                _ => None,
            };
            ProviderProbe { id, version, path }
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        if let Ok(probe) = handle.await {
            results.push(probe);
        }
    }
    results
}

/// Ceiling on a single `--version`. Every CLI here answers in well under a
/// second; a probe that reaches this is a binary that is wedged, prompting, or
/// not the CLI the user thought they pointed us at.
const VERSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run `<bin> --version` under a deadline and extract the first semver-like
/// token from stdout (or stderr as fallback). The one place in this process a
/// version subprocess is spawned — the sync callers reach it through
/// [`probe_version`].
///
/// The deadline has to reach more than the process we spawned: a provider's
/// `claude` may well be a wrapper script, and killing it would leave whatever
/// it started running with nobody waiting on it — one orphan per wedged probe,
/// forever. So the child is spawned in a process group of its own
/// (`pty_session::spawn_in_own_group`, the same pair the installer teardown
/// uses) and a timeout signals the *group*; `kill_on_drop` remains the backstop
/// for the leader. A probe that times out reports no version, exactly like one
/// that printed nothing.
async fn probe_version_bounded(bin: &str) -> Option<String> {
    let out = run_bounded(bin, &["--version"]).await?;
    // Stdout, or stderr when stdout was silent — some CLIs print their version
    // on the latter.
    let said = if out.stdout.is_empty() {
        &out.stderr
    } else {
        &out.stdout
    };
    parse_semver(&String::from_utf8_lossy(said))
}

/// Run `<bin> <args>` under [`VERSION_TIMEOUT`] and hand back what it printed
/// and how it exited; `None` when it could not be started or did not finish
/// in time (and was torn down, group and all).
async fn run_bounded(bin: &str, args: &[&str]) -> Option<std::process::Output> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .kill_on_drop(true)
        // Explicit, because this no longer goes through `Command::output()`:
        // a version probe reads nothing and its output is ours to parse, never
        // the caller's terminal.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::bin_resolve::apply_login_shell_env(cmd.as_std_mut());

    #[cfg(unix)]
    let (child, pgid) = crate::pty_session::spawn_in_own_group(&mut cmd).ok()?;
    #[cfg(not(unix))]
    let child = cmd.spawn().ok()?;

    let out = match tokio::time::timeout(VERSION_TIMEOUT, child.wait_with_output()).await {
        Ok(result) => result.ok()?,
        Err(_) => {
            // Dropping the future killed the leader; the group signal is what
            // reaches everything below it. Best effort — the reaper reports
            // only what it could confirm, and an unreaped leader is enough to
            // make it say "unconfirmed" even though the signals landed.
            #[cfg(unix)]
            {
                let killed = tokio::task::spawn_blocking(move || {
                    crate::pty_session::kill_process_group(pgid)
                })
                .await;
                if let Ok(Err(e)) = killed {
                    tracing::debug!(bin, error = %e, "version probe group teardown unconfirmed");
                }
            }
            return None;
        }
    };
    Some(out)
}

/// The value the capability probe passes: a bad one on purpose.
const THINKING_DISPLAY_PROBE: &str = "__fletch_probe__";

/// The argv of the `--thinking-display` capability probe, after the binary:
/// what [`claude_supports_thinking_display`] runs on the host, and what the
/// container runtimes run inside their images (`sandbox::container::
/// thinking_display`), so one reading serves every engine.
pub(crate) const THINKING_DISPLAY_PROBE_ARGS: [&str; 3] =
    ["--thinking-display", THINKING_DISPLAY_PROBE, "--version"];

/// Whether a probe's output proves the CLI parses `--thinking-display`: it
/// failed, named the option, and named the sentinel value it rejected. An
/// "unknown option '--thinking-display'" diagnostic from a CLI that reports
/// unknown options before `--version` names the option but never the value,
/// and is not support; neither is a run that failed for any other reason.
pub(crate) fn probe_shows_thinking_display(out: &std::process::Output) -> bool {
    let stderr = String::from_utf8_lossy(&out.stderr);
    !out.status.success()
        && stderr.contains("--thinking-display")
        && stderr.contains(THINKING_DISPLAY_PROBE)
        && !stderr.contains("unknown option")
}

/// What has been learnt about `--thinking-display`, by the identity of the
/// binary it was learnt about: a host path, a container image id, a box's
/// tools disk. Each engine records what it found about the binary it actually
/// runs (`SandboxEngine::launch_agent`, `LaunchPlan::claude_thinking_summaries`)
/// and reads it back before the next launch of the same one, so a binary is
/// probed once per app run and never guessed at from another.
pub(crate) fn thinking_display_known(identity: &str) -> Option<bool> {
    known_map().lock().get(identity).copied()
}

pub(crate) fn remember_thinking_display(identity: &str, supported: bool) {
    known_map().lock().insert(identity.to_string(), supported);
    tracing::info!(
        identity,
        supported,
        "claude --thinking-display capability probed"
    );
}

type Known = parking_lot::Mutex<std::collections::HashMap<String, bool>>;

fn known_map() -> &'static Known {
    static KNOWN: std::sync::OnceLock<Known> = std::sync::OnceLock::new();
    KNOWN.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()))
}

/// Whether `bin` — a claude CLI on the host — parses `--thinking-display`,
/// remembered per path. The flag is hidden from `--help`, and `--version` short-circuits
/// before unknown options are reported, so neither can say. What does: a
/// CLI that knows the option validates its value as it parses, so
/// `--thinking-display __fletch_probe__ --version` fails naming the option
/// ("argument '__fletch_probe__' is invalid", 2.1.314), while a CLI that does
/// not know it prints its version and exits 0. No network, no login, no
/// session. The proof is the sentinel value named in the rejection
/// ([`probe_shows_thinking_display`]). A binary that can't be probed counts
/// as not supporting it, so a launch never carries a flag it can't vouch for.
pub fn claude_supports_thinking_display(bin: &str) -> bool {
    let identity = format!("host binary {bin}");
    if let Some(known) = thinking_display_known(&identity) {
        return known;
    }
    let path = bin.to_string();
    let out =
        on_probe_thread(move |rt| rt.block_on(run_bounded(&path, &THINKING_DISPLAY_PROBE_ARGS)));
    let supported = out.is_some_and(|out| probe_shows_thinking_display(&out));
    remember_thinking_display(&identity, supported);
    supported
}

/// The sync callers' door to [`probe_version_bounded`] — the readiness check
/// (`check_cli`), the custom-path validator, and the per-session version stamp
/// (`cached_provider_version`), none of which are async and all of which used
/// to wait on a vendor binary forever.
///
/// Its own thread with its own current-thread runtime, rather than blocking on
/// an ambient one: this is called from a blocking-pool task, from a plain
/// thread, and (at ingest) from inside an async one, and only a runtime of its
/// own is safe from all three. The thread lives for one bounded probe.
fn probe_version(bin: &str) -> Option<String> {
    let bin = bin.to_string();
    on_probe_thread(move |rt| rt.block_on(probe_version_bounded(&bin)))
}

/// Run one bounded probe on a thread of its own with a runtime of its own
/// (see [`probe_version`] for why), and hand back what it found.
fn on_probe_thread<T: Send + 'static>(
    probe: impl FnOnce(&tokio::runtime::Runtime) -> Option<T> + Send + 'static,
) -> Option<T> {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        let found = probe(&runtime);
        // Let the runtime's process driver reap a child the deadline killed,
        // so a wedged probe leaves nothing behind. Returns at once when there
        // is nothing to wait for, which is every ordinary probe.
        runtime.shutdown_timeout(std::time::Duration::from_millis(200));
        found
    })
    .join()
    .ok()
    .flatten()
}

/// Extract the first `N.N[.N[.N]]` token from arbitrary version output.
/// Strips a leading `v` from each word before testing so `v1.0.42` and
/// `1.0.42` both match. Returns the token with a `v` prefix.
pub fn parse_semver(s: &str) -> Option<String> {
    for word in s.split_whitespace() {
        let word = word.trim_start_matches('v');
        // Accept anything that is purely digit-and-dot with at least one dot.
        if word.contains('.')
            && word.chars().all(|c| c.is_ascii_digit() || c == '.')
            && !word.starts_with('.')
            && !word.ends_with('.')
        {
            return Some(format!("v{word}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(dir: &std::path::Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// Poll until `pid` is gone, or the budget runs out. Group-signal delivery
    /// and the reparenting that reaps an orphan are both asynchronous, so the
    /// answer is "shortly", not "by the time the probe returned".
    fn gone_within(pid: i32, budget: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + budget;
        loop {
            if let Err(nix::errno::Errno::ESRCH) =
                nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
            {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    /// A claude that knows `--thinking-display` rejects the probe's value
    /// before `--version` can answer; one that doesn't answers as if the flag
    /// weren't there. Older hosts keep launching without the flag.
    #[test]
    fn the_thinking_display_probe_reads_the_option_off_a_rejection() {
        let dir = tempfile::tempdir().unwrap();
        let knows = script(
            dir.path(),
            "knows",
            "#!/bin/sh\nif [ \"$1\" = --thinking-display ]; then echo \"error: option '--thinking-display <display>' argument '$2' is invalid.\" >&2; exit 1; fi\necho 2.1.314\n",
        );
        assert!(claude_supports_thinking_display(&knows));
        let older = script(dir.path(), "older", "#!/bin/sh\necho 2.0.0\n");
        assert!(!claude_supports_thinking_display(&older));
        // One that reports unknown options before answering `--version`:
        // the option is named, the value is not — no support.
        let strict = script(
            dir.path(),
            "strict",
            "#!/bin/sh\necho \"error: unknown option '$1'\" >&2; exit 1\n",
        );
        assert!(!claude_supports_thinking_display(&strict));
        let missing = dir.path().join("missing").to_string_lossy().into_owned();
        assert!(!claude_supports_thinking_display(&missing));
    }

    /// What an engine learnt is read back by the same identity and by no
    /// other: a rebuilt image or disk is a new identity, probed afresh.
    #[test]
    fn thinking_display_facts_are_kept_by_binary_identity() {
        assert_eq!(thinking_display_known("test image a"), None);
        remember_thinking_display("test image a", true);
        assert_eq!(thinking_display_known("test image a"), Some(true));
        assert_eq!(thinking_display_known("test image b"), None);
        remember_thinking_display("test image a", false);
        assert_eq!(thinking_display_known("test image a"), Some(false));
    }

    /// The sync door every non-async caller goes through — the readiness check,
    /// the custom-path validator, and the ingest version stamp. A CLI that
    /// answers is read as before; one that never answers is given up on at the
    /// deadline, and everything it started goes with it: these CLIs are often
    /// wrapper scripts, and an orphan per probe would accumulate for the life
    /// of the process.
    #[test]
    fn the_sync_probe_reads_a_version_and_gives_up_on_a_wedged_binary() {
        let dir = tempfile::tempdir().unwrap();

        let answers = script(dir.path(), "answers", "#!/bin/sh\necho 1.2.3\n");
        assert_eq!(probe_version(&answers), Some("v1.2.3".to_string()));

        // A wrapper that never answers, with a child of its own — the shape
        // `kill_on_drop` alone would leave running.
        let child_pid = dir.path().join("child.pid");
        let wedged = script(
            dir.path(),
            "wedged",
            &format!(
                "#!/bin/sh\nsleep 300 &\necho $! > '{}'\nwait\n",
                child_pid.display()
            ),
        );
        let started = std::time::Instant::now();
        assert_eq!(probe_version(&wedged), None, "a wedged probe has no answer");
        assert!(
            started.elapsed() < VERSION_TIMEOUT * 3,
            "the probe waited past its deadline: {:?}",
            started.elapsed()
        );

        let pid: i32 = std::fs::read_to_string(&child_pid)
            .expect("the wrapper wrote its child's pid")
            .trim()
            .parse()
            .expect("a pid");
        assert!(
            gone_within(pid, std::time::Duration::from_secs(1)),
            "the wrapper's child ({pid}) outlived the probe"
        );
    }
}

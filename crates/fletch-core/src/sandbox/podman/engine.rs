//! The Podman sandbox engine: one container per agent process, agent ≈ PID 1.
//!
//! Behaviourally the Docker engine with a different binary — the shared parts
//! live under [`container`](crate::sandbox::container). What this file adds is
//! the podman binary, the podman teardown commands, and the machine preflight:
//! a bind mount from outside the machine's shares silently yields an empty dir
//! rather than an error (see [`super::machine`]), so it is checked before the
//! launch, while the path can still be named.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::container::run_args::{
    mount_sources, run_args, RunSpec, DEFAULT_CPUS, DEFAULT_MEMORY,
};
use crate::sandbox::container::util::{container_name, describe_exit_code, non_blank, ExitCopy};
use crate::sandbox::container::ContainerProvider;
use crate::sandbox::engine::{
    AgentLaunchCtx, EngineKind, KillHandle, KillPlan, LaunchPlan, SandboxEngine,
};

use super::settings::{IMAGE_SETTING, LAUNCH_SETTINGS};
use super::{cli, image, machine};

/// Signal/removal podman calls during teardown.
const KILL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a TERM'd container gets to exit before escalating to KILL.
const TERM_GRACE: Duration = Duration::from_millis(500);
/// Liveness lookups (`podman inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Podman's wording for the shared reserved-exit-code messages.
const EXIT_COPY: ExitCopy = ExitCopy {
    runtime: super::RUNTIME_NAME,
    error_source: "the machine",
    remedy: "Is the Podman machine running?",
    image_setting: Some(IMAGE_SETTING),
};

/// The `SandboxEngine` implementation for Podman. Obtain it via
/// [`PodmanEngine::shared`] — one instance per app run, so launches share the
/// image resolution cache.
pub struct PodmanEngine {
    /// Images resolved for this app run. The connection is part of the key
    /// because image stores are per-machine: a tag present on one says nothing
    /// about another. Only successes are cached, so a failed build retries.
    resolved_image: Mutex<ImageCache>,
}

/// `(provider, image override, connection)` → resolved tag.
type ImageCache =
    std::collections::HashMap<(ContainerProvider, Option<String>, Option<String>), String>;

impl PodmanEngine {
    /// The process-wide engine instance.
    pub fn shared() -> Arc<PodmanEngine> {
        static ENGINE: OnceLock<Arc<PodmanEngine>> = OnceLock::new();
        ENGINE
            .get_or_init(|| {
                Arc::new(PodmanEngine {
                    resolved_image: Mutex::new(ImageCache::new()),
                })
            })
            .clone()
    }

    /// The image to launch `provider` from on `connection`, resolving (and
    /// building) at most once per app run per (provider, override, connection)
    /// triple — which is also the cadence of the freshness checks
    /// `image::resolve_image` kicks off.
    fn resolve_image_cached(
        &self,
        provider: ContainerProvider,
        override_image: Option<&str>,
        connection: Option<&str>,
    ) -> Result<String> {
        let key = (
            provider,
            override_image.map(str::to_string),
            connection.map(str::to_string),
        );
        // The lock is dropped before resolving: a build can take ten minutes and
        // would otherwise block every cache-hit launch. Two cold launches racing
        // the same key is safe — `BUILD_LOCK` makes the second a no-op.
        if let Some(tag) = self.resolved_image.lock().unwrap().get(&key) {
            return Ok(tag.clone());
        }
        // No host probe on the override path: the user's image is never
        // inspected or refreshed, so there is nothing to compare.
        let host_cli_version = if non_blank(override_image).is_none() {
            crate::agent::cached_provider_version(provider.id())
        } else {
            None
        };
        // Free-form output rides in the `line` field, not the message, so the
        // sentry scrubber drops it — see the privacy invariant in `lib.rs`.
        let on_progress = |line: &str| tracing::info!(target: "fletch::podman_build", line = %line, "podman build output");
        let tag = image::resolve_image(
            provider,
            connection,
            override_image,
            host_cli_version.as_deref(),
            &on_progress,
        )
        .map_err(|e| Error::Other(format!("preparing the Podman sandbox image failed: {e}")))?;
        self.resolved_image.lock().unwrap().insert(key, tag.clone());
        Ok(tag)
    }
}

/// `run_argv` with this launch's connection pin ahead of it. `--connection` is a
/// *global* flag: after `run` podman would read it as a container argument.
fn pinned_argv(connection: Option<&str>, run_argv: Vec<String>) -> Vec<String> {
    let Some(connection) = connection else {
        return run_argv;
    };
    let mut argv = vec!["--connection".to_string(), connection.to_string()];
    argv.extend(run_argv);
    argv
}

impl SandboxEngine for PodmanEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Podman
    }

    /// Launch a container for `ctx.provider`. `ensure_engine_supports_provider`
    /// gates this, so the `from_id` failure below is defensive only.
    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan> {
        let provider = ContainerProvider::from_id(ctx.provider).ok_or_else(|| {
            Error::Other(format!(
                "Podman sandbox has no support for provider `{}`",
                ctx.provider
            ))
        })?;
        let podman = cli::podman_bin()
            .ok_or_else(|| Error::Other("podman binary not found — is Podman installed?".into()))?;
        // Resolved first: the image store, the mount check, the run and the
        // teardown all have to be the one endpoint named here, and a remote
        // default is refused before a minutes-long build for it.
        let target = machine::resolve_launch_target()?;
        let connection = target.connection.clone();
        let settings = LAUNCH_SETTINGS.read().clone();
        let name = container_name(ctx.agent_id);
        let prep = crate::sandbox::container::launch::prepare(ctx, provider)?;

        let prefix_args = {
            let auth_vars = prep.auth_vars();
            let mut spec = RunSpec {
                interactive: ctx.interactive,
                name: &name,
                agent_id: ctx.agent_id,
                writable_root: ctx.writable_root,
                rpc_dir: ctx.rpc_dir,
                home: ctx.home,
                cwd: ctx.cwd,
                blackboard: ctx.blackboard,
                adopted_workspace: ctx.adopted_workspace(),
                mounts: prep.mounts(),
                borrowed_object_stores: &prep.borrowed_object_stores,
                memory: non_blank(settings.memory.as_deref()).unwrap_or(DEFAULT_MEMORY),
                cpus: non_blank(settings.cpus.as_deref()).unwrap_or(DEFAULT_CPUS),
                // Filled in below, once the preflight has passed.
                image: "",
                agent_bin,
                auth_vars: &auth_vars,
            };
            // Before the image, not just before the run: an unshared source
            // mounts empty rather than failing, and the refusal shouldn't come
            // after waiting out a build.
            machine::ensure_sources_are_shared(&mount_sources(&spec), &target)?;
            let image = self.resolve_image_cached(
                provider,
                settings.image_override.as_deref(),
                connection.as_deref(),
            )?;
            spec.image = &image;
            pinned_argv(connection.as_deref(), run_args(&spec))
        };

        Ok(LaunchPlan {
            program: podman,
            prefix_args,
            env: prep.env,
            kill: KillHandle::Engine {
                engine: PodmanEngine::shared(),
                plan: KillPlan::Container { name, connection },
            },
        })
    }

    /// TERM, a grace window, then KILL, then a best-effort `rm -f`. Always `Ok`:
    /// the container is usually already gone, and an error here would abort the
    /// caller's local process-group teardown of the podman CLI child.
    fn kill(&self, plan: &KillPlan) -> Result<()> {
        // The launch's pinned connection, not the current default — a default
        // that moved since would signal into the wrong machine.
        let KillPlan::Container { name, connection } = plan;
        let on = connection.as_deref();
        match cli::run_podman_on(on, &["kill", "-s", "TERM", name], KILL_TIMEOUT) {
            Ok(out) if out.status.success() => {
                if !container_gone_within(name, on, TERM_GRACE) {
                    tracing::info!(container = %name, "container survived TERM grace; escalating to KILL");
                    let _ = cli::run_podman_on(on, &["kill", name], KILL_TIMEOUT);
                }
            }
            // Non-zero exit = "no such container" (already exited and
            // auto-removed) — nothing to escalate.
            Ok(_) => {}
            Err(e) => tracing::warn!(container = %name, error = %e, "podman kill failed"),
        }
        // Usually a no-op thanks to --rm; covers a wedged auto-remove.
        let _ = cli::run_podman_on(on, &["rm", "-f", name], KILL_TIMEOUT);
        Ok(())
    }

    fn describe_exit(&self, _plan: &KillPlan, code: i32) -> Option<String> {
        describe_exit_code(code, &EXIT_COPY)
    }
}

/// Whether the container is running on the connection it was launched on.
/// Errors (container gone, machine down, timeout) read as not running.
fn container_running(name: &str, connection: Option<&str>) -> bool {
    match cli::run_podman_on(
        connection,
        &["inspect", "-f", "{{.State.Running}}", name],
        INSPECT_TIMEOUT,
    ) {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(e) => {
            tracing::debug!(container = %name, error = %e, "podman inspect failed; treating as dead");
            false
        }
    }
}

/// Poll until the container stops running or `budget` elapses.
fn container_gone_within(name: &str, connection: Option<&str>, budget: Duration) -> bool {
    let deadline = std::time::Instant::now() + budget;
    loop {
        if !container_running(name, connection) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::container::run_args::prepare_config_mount_dir;
    use std::path::PathBuf;

    #[test]
    fn exit_code_wording_is_podman_shaped() {
        let daemon = describe_exit_code(125, &EXIT_COPY).unwrap();
        assert!(daemon.contains("Podman"), "{daemon}");
        assert!(daemon.contains("Podman machine"), "{daemon}");
        for code in [125, 126, 127] {
            let msg = describe_exit_code(code, &EXIT_COPY).unwrap();
            assert!(msg.contains("agent itself exited"), "must hedge: {msg}");
            assert!(!msg.contains("docker"), "{msg}");
        }
        // 126/127 point at this engine's own override key.
        for code in [126, 127] {
            let msg = describe_exit_code(code, &EXIT_COPY).unwrap();
            assert!(msg.contains("podman_image"), "{msg}");
        }
        assert_eq!(describe_exit_code(1, &EXIT_COPY), None);
    }

    #[test]
    fn the_connection_pin_precedes_the_run_subcommand() {
        let run_argv = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            "fletch-agent".to_string(),
        ];
        assert_eq!(
            pinned_argv(Some("work-vm-root"), run_argv.clone()),
            [
                "--connection",
                "work-vm-root",
                "run",
                "--rm",
                "--name",
                "fletch-agent",
            ],
        );
        assert_eq!(pinned_argv(None, run_argv.clone()), run_argv);
    }

    /// Integration: a marker file round-trips through the workspace mount at its
    /// identical host path. `FLETCH_PODMAN_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Podman; opt in via FLETCH_PODMAN_TESTS=1"]
    fn podman_run_echo_round_trip() {
        if !crate::sandbox::podman::podman_tests_enabled() {
            return;
        }
        // Under `$HOME`, not `$TMPDIR`: the machine shares `$HOME` by default,
        // and a mount from outside its shares comes up empty rather than
        // failing, which would make this test's failure unreadable.
        let td = tempfile::tempdir_in(dirs::home_dir().unwrap()).unwrap();
        let root = td.path().join("workspace");
        let rpc = td.path().join("rpc");
        let home = td.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&rpc).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(root.join("marker.txt"), "hello from the host\n").unwrap();
        prepare_config_mount_dir(&home.join(".claude")).unwrap();
        let target = machine::resolve_launch_target().unwrap();
        machine::ensure_sources_are_shared(&[root.clone(), rpc.clone(), home.clone()], &target)
            .unwrap();

        let name = container_name("podman-int-test");
        let projects = root.join("projects-src");
        std::fs::create_dir_all(&projects).unwrap();
        let args = run_args(&RunSpec {
            interactive: false,
            name: &name,
            agent_id: "podman-int-test",
            writable_root: &root,
            rpc_dir: &rpc,
            home: &home,
            cwd: &root,
            blackboard: None,
            adopted_workspace: None,
            mounts: crate::sandbox::container::run_args::ProviderMounts::Claude {
                config_dir: None,
                credentials_rw: false,
                config_dir_credentials_rw: false,
                projects_src: &projects,
            },
            borrowed_object_stores: &[],
            memory: DEFAULT_MEMORY,
            cpus: DEFAULT_CPUS,
            image: "busybox",
            agent_bin: "cat",
            auth_vars: &[],
        });
        let mut argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let marker = root.join("marker.txt").to_string_lossy().into_owned();
        argv.push(&marker);

        // Pinned like the engine pins it, so the run lands on the machine whose
        // shares were just validated.
        let out = cli::run_podman_on(
            target.connection.as_deref(),
            &argv,
            Duration::from_secs(120),
        )
        .unwrap();
        assert!(
            out.status.success(),
            "podman run failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "hello from the host",
            "the workspace must be readable at its identical host path",
        );
    }

    /// Integration. `FLETCH_PODMAN_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Podman; opt in via FLETCH_PODMAN_TESTS=1"]
    fn kill_and_liveness_against_live_container() {
        if !crate::sandbox::podman::podman_tests_enabled() {
            return;
        }
        let name = container_name("podman-kill-test");
        let out = cli::run_podman(
            &[
                "run", "-d", "--rm", "--name", &name, "busybox", "sleep", "120",
            ],
            Duration::from_secs(120),
        )
        .unwrap();
        assert!(
            out.status.success(),
            "podman run failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(
            container_running(&name, None),
            "a started container reads as live"
        );

        PodmanEngine::shared()
            .kill(&KillPlan::Container {
                name: name.clone(),
                connection: None,
            })
            .unwrap();
        assert!(
            !container_running(&name, None),
            "killed container reads as dead"
        );
    }

    /// Integration; skipped on a host with no machine, where the check is inert.
    /// `FLETCH_PODMAN_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Podman; opt in via FLETCH_PODMAN_TESTS=1"]
    fn machine_preflight_refuses_unshared_sources() {
        if !crate::sandbox::podman::podman_tests_enabled() {
            return;
        }
        let home = dirs::home_dir().unwrap();
        let Ok(target) = machine::resolve_launch_target() else {
            return; // a remote default connection: refused before any mounts
        };
        if machine::ensure_sources_are_shared(std::slice::from_ref(&home), &target).is_err() {
            return; // no machine, or $HOME isn't shared: nothing to assert
        }
        let outside = PathBuf::from("/fletch-definitely-not-shared");
        let err = machine::ensure_sources_are_shared(std::slice::from_ref(&outside), &target)
            .unwrap_err();
        assert!(
            err.to_string().contains(&outside.display().to_string()),
            "the refusal must name the path: {err}",
        );
    }
}

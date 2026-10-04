//! The host-owned global settings: the `settings` rows the engine itself reads
//! (alerts, the idle sweep, code indexing, the sandbox engine and its launch
//! knobs, provider binary overrides, the publishing preferences), and the one
//! setter per group that persists a value *and* updates the in-memory mirror
//! the spawn and publish paths read. The desktop's Tauri commands are thin
//! wrappers over these, and the remote dispatcher calls them directly, so the
//! Settings pane edits whichever host it is driving.
//!
//! See docs/remote-protocol.md, "Settings".

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::supervisor::Supervisor;
use crate::{
    attribution, bin_resolve, codegraph, database, publish_prefs, remote, rpc, sandbox, supervisor,
};

/// The exact global keys a client may read. Every one is read host-side and
/// written by one of the setters below; nothing secret is on it.
const HOST_SETTING_KEYS: &[&str] = &[
    remote::push::TURN_COMPLETE_SETTING,
    remote::push::PR_ACTIVITY_SETTING,
    supervisor::auto_archive::IDLE_DAYS_SETTING,
    codegraph::SETTING,
    sandbox::ENGINE_SETTING,
    sandbox::docker::IMAGE_SETTING,
    sandbox::docker::MEMORY_SETTING,
    sandbox::docker::CPUS_SETTING,
    sandbox::podman::IMAGE_SETTING,
    sandbox::podman::MEMORY_SETTING,
    sandbox::podman::CPUS_SETTING,
    publish_prefs::BRANCH_PREFIX_SETTING,
    publish_prefs::DRAFT_PRS_SETTING,
    rpc::approval::SETTING,
    rpc::approval::WAIT_SETTING,
    attribution::SETTING,
];

/// The event every setter emits, one per key it wrote.
pub const SETTINGS_CHANGED: &str = "settings:changed";

/// Whether `key` is a host-owned setting a client may read: the exact list
/// plus the `agent_bin_path_<id>` family.
pub fn is_host_setting_key(key: &str) -> bool {
    HOST_SETTING_KEYS.contains(&key)
        || key
            .strip_prefix(database::AGENT_BIN_PREFIX)
            .is_some_and(|id| !id.is_empty())
}

#[derive(Serialize)]
struct Changed<'a> {
    key: &'a str,
    value: Option<&'a str>,
}

fn announce(ctx: &EngineCtx, key: &str, value: Option<&str>) {
    crate::host::emit(ctx.sink.as_ref(), SETTINGS_CHANGED, &Changed { key, value });
}

/// Persist one value and tell every other view.
fn store(ctx: &EngineCtx, key: &str, value: &str) -> Result<()> {
    database::set_setting(&ctx.db.lock(), key, value)?;
    announce(ctx, key, Some(value));
    Ok(())
}

fn flag(on: bool) -> &'static str {
    if on {
        "true"
    } else {
        "false"
    }
}

/// The host-owned settings as stored, and nothing else. An absent key is unset
/// and reads as its default on both sides.
pub fn get_settings_impl(ctx: &EngineCtx) -> Result<BTreeMap<String, String>> {
    let conn = ctx.db.lock();
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (key, value) = row?;
        if is_host_setting_key(&key) {
            out.insert(key, value);
        }
    }
    Ok(out)
}

/// Whether finishing a turn alerts at all (chime, banner, phone push); needing
/// input always does. Mirrored for the push taps, which run without a DB.
pub fn set_notify_turn_complete_impl(ctx: &EngineCtx, enabled: bool) -> Result<()> {
    store(ctx, remote::push::TURN_COMPLETE_SETTING, flag(enabled))?;
    remote::push::set_turn_complete(enabled);
    Ok(())
}

/// Whether the ship loop alerts the phone at all — checks settling, a review
/// comment, a PR merging or closing. Same shape as the turn-complete switch.
pub fn set_notify_pr_activity_impl(ctx: &EngineCtx, enabled: bool) -> Result<()> {
    store(ctx, remote::push::PR_ACTIVITY_SETTING, flag(enabled))?;
    remote::push::set_pr_activity(enabled);
    Ok(())
}

/// Days a sidebar workspace may sit idle before the hourly sweep archives it;
/// `0` turns the sweep off. The sweep reads the setting on every pass, so
/// there is no mirror to update.
pub fn set_auto_archive_idle_days_impl(ctx: &EngineCtx, days: u32) -> Result<()> {
    store(
        ctx,
        supervisor::auto_archive::IDLE_DAYS_SETTING,
        &days.to_string(),
    )
}

/// How long a publish-approval prompt waits before denying; `0` waits until
/// answered.
pub fn set_publish_approval_wait_impl(ctx: &EngineCtx, secs: u64) -> Result<()> {
    store(ctx, rpc::approval::WAIT_SETTING, &secs.to_string())?;
    rpc::approval::set_wait_secs(secs);
    Ok(())
}

/// Turn the publish-approval prompt on or off. Off by default: autopilot
/// publishes while nobody is watching, and a prompt would hang it until the
/// decision timeout and then refuse.
pub fn set_publish_confirmation_impl(ctx: &EngineCtx, enabled: bool) -> Result<()> {
    store(ctx, rpc::approval::SETTING, flag(enabled))?;
    rpc::approval::set_enabled(enabled);
    Ok(())
}

/// The prefix prepended to every branch an agent creates. Validated and
/// trimmed; the stored form is returned so the UI shows exactly what applies.
/// Empty clears it.
pub fn set_branch_prefix_impl(ctx: &EngineCtx, prefix: &str) -> Result<String> {
    let prefix = publish_prefs::validate_branch_prefix(prefix).map_err(Error::Other)?;
    store(ctx, publish_prefs::BRANCH_PREFIX_SETTING, &prefix)?;
    publish_prefs::set_branch_prefix(&prefix);
    Ok(prefix)
}

/// Whether pull requests Fletch opens start as drafts.
pub fn set_draft_prs_impl(ctx: &EngineCtx, enabled: bool) -> Result<()> {
    store(ctx, publish_prefs::DRAFT_PRS_SETTING, flag(enabled))?;
    publish_prefs::set_draft_prs(enabled);
    Ok(())
}

/// "Remove agent attribution": on strips agents' co-author trailers and
/// "Generated with" lines regardless of their own settings; off leaves those
/// settings in charge. Applies from each agent's next spawn or resume.
pub fn set_agent_attribution_removed_impl(ctx: &EngineCtx, removed: bool) -> Result<()> {
    store(ctx, attribution::SETTING, flag(removed))?;
    attribution::set_removed(removed);
    Ok(())
}

/// Flip code-indexing consent and update the mirror the spawn path reads.
///
/// Turning it ON kicks a best-effort background task: install the codegraph
/// bundle, then warm the index mirror for every pinned repo so the first agent
/// spawn after a fresh enable already has an index to copy in. Turning it OFF
/// does nothing else — indexes die with their workspaces.
pub fn set_code_indexing_enabled_impl(
    ctx: &EngineCtx,
    sup: &Supervisor,
    enabled: bool,
) -> Result<()> {
    store(ctx, codegraph::SETTING, flag(enabled))?;
    codegraph::set_enabled(enabled);
    if enabled {
        // Snapshot the pinned repos (path + owning project) up front so the
        // background task holds no DB handle across awaits.
        let repos: Vec<(String, PathBuf)> = sup
            .workspace
            .current()
            .map(|w| {
                w.projects
                    .into_iter()
                    .map(|p| (p.project_id, p.path))
                    .collect()
            })
            .unwrap_or_default();
        crate::host::spawn(warm_codegraph_index(repos));
    }
    Ok(())
}

/// Best-effort: ensure codegraph is installed, then build/refresh the index
/// mirror for each `(project_id, source_repo)`. Every step logs and continues —
/// a failure here just means indexing warms up on a later spawn.
async fn warm_codegraph_index(repos: Vec<(String, PathBuf)>) {
    let bin = match codegraph::ensure_installed().await {
        Ok(bin) => bin,
        Err(e) => {
            tracing::warn!(error = %e, "codegraph install failed; indexing stays off until retry");
            return;
        }
    };
    for (project_id, source_repo) in repos {
        let mirror = match codegraph::mirror_dir(&project_id, &source_repo) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, repo = %source_repo.display(), "codegraph mirror path failed");
                continue;
            }
        };
        if let Err(e) = codegraph::ensure_mirror(&source_repo, &mirror, &bin).await {
            tracing::warn!(error = %e, repo = %source_repo.display(), "codegraph mirror warm-up failed");
        }
    }
}

/// Change the sandbox engine stamped onto *new* agents. Container engines are
/// validated live before being accepted — Docker against a daemon probe, Podman
/// against its availability plus its launch blocker — so a success here means
/// the choice is actionable on this host. Existing agents keep the engine
/// stamped on their record at creation.
pub async fn set_sandbox_engine_impl(ctx: &EngineCtx, engine: &str) -> Result<()> {
    let kind = sandbox::EngineKind::from_setting(engine)
        .ok_or_else(|| Error::Other(format!("unknown sandbox engine: {engine}")))?;
    let blocking = |e: tokio::task::JoinError| Error::Other(e.to_string());
    if kind == sandbox::EngineKind::Podman {
        // A stored engine is stamped onto every new agent, so one whose runtime
        // can't launch fails every spawn. "Available" isn't "launchable":
        // `podman info` can answer over a remote default connection that every
        // launch then refuses, hence the blocker check on the same worker.
        let (probe, blocker) = tokio::task::spawn_blocking(|| {
            let probe = sandbox::podman_availability();
            let blocker = matches!(probe, sandbox::PodmanAvailability::Available { .. })
                .then(sandbox::podman_launch_blocker)
                .flatten();
            (probe, blocker)
        })
        .await
        .map_err(blocking)?;
        match probe {
            sandbox::PodmanAvailability::Available { .. } => {
                if let Some(blocker) = blocker {
                    return Err(Error::Other(blocker));
                }
            }
            sandbox::PodmanAvailability::NotInstalled => {
                return Err(Error::Other(
                    "Podman is not installed — install Podman first.".into(),
                ))
            }
            sandbox::PodmanAvailability::MachineDown => {
                return Err(Error::Other(
                    "The Podman machine isn't running — run `podman machine start` first.".into(),
                ))
            }
        }
    }
    if kind == sandbox::EngineKind::Docker {
        // A blocking worker: the probe can take up to its 2s timeout.
        let probe = tokio::task::spawn_blocking(sandbox::docker_availability)
            .await
            .map_err(blocking)?;
        match probe {
            sandbox::DockerAvailability::Available { .. } => {}
            sandbox::DockerAvailability::NotInstalled => {
                return Err(Error::Other(
                    "Docker is not installed — install Docker Desktop first.".into(),
                ))
            }
            sandbox::DockerAvailability::DaemonDown => {
                return Err(Error::Other(
                    "Docker isn't running — start Docker Desktop first.".into(),
                ))
            }
        }
    }
    store(ctx, sandbox::ENGINE_SETTING, kind.as_setting())?;
    sandbox::set_selected_engine_kind(kind);
    Ok(())
}

/// One runtime's three launch knobs, normalized: blank is `None`, because a
/// cleared field must not be stored as a launch override (an empty `--memory`
/// or image would break the launch) and the mirror reads blank as "default".
pub struct LaunchKnobs {
    pub image: Option<String>,
    pub memory: Option<String>,
    pub cpus: Option<String>,
}

impl LaunchKnobs {
    pub fn new(image: Option<String>, memory: Option<String>, cpus: Option<String>) -> Self {
        let norm = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Self {
            image: norm(image),
            memory: norm(memory),
            cpus: norm(cpus),
        }
    }
}

/// Write three keys in one transaction, then announce each. Written one by one,
/// a mid-way failure would leave a mixed config committed that the UI never
/// shows (it reverts all three), and a restart would hydrate it.
fn store_launch_knobs(ctx: &EngineCtx, keys: [&str; 3], knobs: &LaunchKnobs) -> Result<()> {
    let values = [&knobs.image, &knobs.memory, &knobs.cpus];
    {
        let conn = ctx.db.lock();
        let tx = conn.unchecked_transaction()?;
        for (key, value) in keys.iter().zip(values) {
            database::set_setting(&tx, key, value.as_deref().unwrap_or(""))?;
        }
        tx.commit()?;
    }
    for (key, value) in keys.iter().zip(values) {
        announce(ctx, key, Some(value.as_deref().unwrap_or("")));
    }
    Ok(())
}

/// Persist the docker launch knobs (`docker_image` override + `docker_memory`
/// / `docker_cpus` limits) and update the mirror the spawn path reads, so a
/// change applies to the next docker spawn without a restart.
pub fn set_docker_launch_settings_impl(ctx: &EngineCtx, knobs: LaunchKnobs) -> Result<()> {
    store_launch_knobs(
        ctx,
        [
            sandbox::docker::IMAGE_SETTING,
            sandbox::docker::MEMORY_SETTING,
            sandbox::docker::CPUS_SETTING,
        ],
        &knobs,
    )?;
    sandbox::docker::set_launch_settings(sandbox::docker::LaunchSettings {
        image_override: knobs.image,
        memory: knobs.memory,
        cpus: knobs.cpus,
    });
    Ok(())
}

/// The podman twin of [`set_docker_launch_settings_impl`], over its own keys
/// and mirror — a user running both points each at its own image and limits.
pub fn set_podman_launch_settings_impl(ctx: &EngineCtx, knobs: LaunchKnobs) -> Result<()> {
    store_launch_knobs(
        ctx,
        [
            sandbox::podman::IMAGE_SETTING,
            sandbox::podman::MEMORY_SETTING,
            sandbox::podman::CPUS_SETTING,
        ],
        &knobs,
    )?;
    sandbox::podman::set_launch_settings(sandbox::podman::LaunchSettings {
        image_override: knobs.image,
        memory: knobs.memory,
        cpus: knobs.cpus,
    });
    Ok(())
}

/// Set or clear (blank/`None`) a per-agent custom binary path — a path on this
/// host. Writes or deletes `agent_bin_path_<id>`, refreshes the in-memory
/// registry binary resolution reads, then restarts any live agents on that
/// provider: resolution happens only at spawn time, so an already-running
/// agent would otherwise keep the old binary (and the old account).
pub async fn set_agent_bin_override_impl(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    id: &str,
    path: Option<&str>,
) -> Result<()> {
    if id.is_empty() {
        return Err(Error::Other("an agent id is required".into()));
    }
    let key = format!("{}{}", database::AGENT_BIN_PREFIX, id);
    let path = path.map(str::trim).filter(|p| !p.is_empty());
    // Scoped so the guard drops before the respawn, which re-locks the DB.
    {
        let conn = ctx.db.lock();
        match path {
            Some(p) => database::set_setting(&conn, &key, p)?,
            None => {
                conn.execute("DELETE FROM settings WHERE key = ?1", [&key])?;
            }
        }
        bin_resolve::set_agent_overrides(database::load_agent_bin_overrides(&conn));
    }
    announce(ctx, &key, path);
    sup.respawn_provider(ctx, id).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ctx::test_ctx;
    use serde_json::json;

    /// Every key the host reads and a setter writes is answered.
    #[test]
    fn every_host_owned_key_is_readable() {
        for key in [
            "notify_turn_complete",
            "notify_pr_activity",
            "auto_archive_idle_days",
            "git_branch_prefix",
            "github_draft_prs",
            "publish_confirmation",
            "publish_approval_wait",
            "agent_attribution_removed",
            "code_indexing_enabled",
            "sandbox_engine",
            "docker_image",
            "docker_memory",
            "docker_cpus",
            "podman_image",
            "podman_memory",
            "podman_cpus",
            "agent_bin_path_claude",
        ] {
            assert!(is_host_setting_key(key), "{key} must be answered");
        }
        assert!(!is_host_setting_key("agent_bin_path_"));
    }

    /// The point of the allowlist: secrets, this machine's remote-access state,
    /// telemetry identity and the client's own preferences never cross the
    /// wire, even when the table holds them.
    #[test]
    fn get_settings_never_answers_a_secret_or_a_client_key() {
        let (ctx, _sink, _dir) = test_ctx();
        {
            let conn = ctx.db.lock();
            for (key, value) in [
                ("github_token", "ghp_secret"),
                ("linear_token", "lin_secret"),
                ("claude_container_token", "sk-ant-oat-secret"),
                ("telemetry_distinct_id", "abc"),
                ("telemetry_enabled", "true"),
                ("remote.enabled", "true"),
                ("remote.port", "7878"),
                ("remote.relay_url", "wss://relay"),
                ("theme", "dark"),
                ("admin", "true"),
                ("notify_turn_complete", "false"),
                ("agent_bin_path_codex", "/opt/codex"),
            ] {
                database::set_setting(&conn, key, value).unwrap();
            }
        }
        let got = get_settings_impl(&ctx).unwrap();
        assert_eq!(
            got,
            BTreeMap::from([
                ("agent_bin_path_codex".to_string(), "/opt/codex".to_string()),
                ("notify_turn_complete".to_string(), "false".to_string()),
            ])
        );
        let text = serde_json::to_string(&got).unwrap();
        for secret in [
            "ghp_secret",
            "lin_secret",
            "sk-ant-oat-secret",
            "wss://relay",
        ] {
            assert!(!text.contains(secret), "{secret} reached the answer");
        }
    }

    /// A setter persists, updates its mirror and announces the key it wrote.
    #[test]
    fn a_setter_persists_mirrors_and_announces() {
        let (ctx, sink, _dir) = test_ctx();
        set_auto_archive_idle_days_impl(&ctx, 14).unwrap();
        assert_eq!(
            get_settings_impl(&ctx)
                .unwrap()
                .get("auto_archive_idle_days"),
            Some(&"14".to_string())
        );
        assert_eq!(
            sink.events(),
            vec![(
                SETTINGS_CHANGED.to_string(),
                json!({ "key": "auto_archive_idle_days", "value": "14" })
            )]
        );
    }

    #[test]
    fn a_bad_branch_prefix_is_refused_and_writes_nothing() {
        let (ctx, sink, _dir) = test_ctx();
        assert!(set_branch_prefix_impl(&ctx, "has space/").is_err());
        assert!(get_settings_impl(&ctx).unwrap().is_empty());
        assert!(sink.events().is_empty());
    }

    /// The persisting half only: the process-wide launch mirror is left alone
    /// so this cannot race the sandbox tests that read it.
    #[test]
    fn launch_knobs_land_together_with_blank_as_default() {
        let (ctx, sink, _dir) = test_ctx();
        store_launch_knobs(
            &ctx,
            [
                sandbox::podman::IMAGE_SETTING,
                sandbox::podman::MEMORY_SETTING,
                sandbox::podman::CPUS_SETTING,
            ],
            &LaunchKnobs::new(Some(" img:1 ".into()), Some("  ".into()), None),
        )
        .unwrap();
        let got = get_settings_impl(&ctx).unwrap();
        assert_eq!(got.get("podman_image"), Some(&"img:1".to_string()));
        assert_eq!(got.get("podman_memory"), Some(&String::new()));
        assert_eq!(got.get("podman_cpus"), Some(&String::new()));
        assert_eq!(sink.events().len(), 3);
    }

    #[tokio::test]
    async fn an_unknown_sandbox_engine_is_refused() {
        let (ctx, _sink, _dir) = test_ctx();
        let err = set_sandbox_engine_impl(&ctx, "firecracker")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("firecracker"));
    }
}

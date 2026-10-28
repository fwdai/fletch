//! The engine halves of the box commands (docs/boxes-spec.md §9): what the
//! Settings › Sandbox page, the Developer pane and the agent's isolation row
//! ask. Phase 4b wraps each in a `#[tauri::command]` and a remote op.

use fletch_box::Probe;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::sandbox::boxes::{
    self, registry::ToolsStatus, settings, setup, BoxRegistry, StopReason,
};
use crate::{database, sandbox};

use super::settings::announce;

pub use crate::sandbox::boxes::setup::BoxImageStatus;

/// `box_runtime_probe`: can this host boot a box, item by item.
#[derive(Debug, Clone, Serialize)]
pub struct BoxRuntimeProbe {
    /// The developer flag.
    pub enabled: bool,
    /// Every item passed, the flag and image included.
    pub ok: bool,
    /// The machine and the runtime pass: Apple silicon, the hypervisor, the
    /// helper and its dylibs. What the toggle needs; the image and the tools
    /// disks are set up after it.
    pub runtime_ok: bool,
    /// One sentence per failing runtime item.
    pub problems: Vec<String>,
    pub helper: Option<String>,
    pub lib_dir: Option<String>,
    pub arch_ok: bool,
    pub hypervisor: bool,
    pub image: BoxImageStatus,
    /// Each supported provider's tools disk.
    pub tools: Vec<ToolsStatus>,
}

pub fn box_image_status_impl() -> BoxImageStatus {
    setup::image_status()
}

pub fn box_runtime_probe_impl() -> BoxRuntimeProbe {
    let enabled = settings::enabled();
    let probe = boxes::probe();
    let image = box_image_status_impl();
    let problems = match &probe {
        Some(probe) => probe.problems(),
        None => vec![boxes::RUNTIME_NOT_FOUND.to_string()],
    };
    let runtime_ok = problems.is_empty();
    let tools = fletch_box::tools::PROVIDERS
        .iter()
        .filter(|p| boxes::supports_provider(p))
        .map(|p| BoxRegistry::global().tools_status(p))
        .collect();
    BoxRuntimeProbe {
        enabled,
        ok: enabled && runtime_ok && image.phase == "installed",
        runtime_ok,
        problems,
        helper: probe
            .as_ref()
            .map(|p| p.helper.to_string_lossy().into_owned()),
        lib_dir: probe
            .as_ref()
            .map(|p| p.lib_dir.to_string_lossy().into_owned()),
        arch_ok: probe.as_ref().is_some_and(|p| p.arch_ok),
        hypervisor: probe.as_ref().is_some_and(|p| p.hypervisor),
        image,
        tools,
    }
}

/// Start downloading this build's pinned image in the background, then build
/// the tools disks; progress and the outcome arrive as `box:image` and
/// `box:tools` events. Idempotent, so it is also the Retry after a failure.
pub fn box_image_install_impl() -> Result<()> {
    setup::start_install()
}

pub fn box_image_cancel_impl() -> Result<()> {
    setup::cancel_install()
}

/// `box_state(agent_id)`: the registry's phase for the agent's box.
pub fn box_state_impl(agent_id: &str) -> boxes::registry::BoxStateReport {
    BoxRegistry::global().report(agent_id)
}

/// `box_stop(agent_id)`: stop the box now, reporting which rung worked.
pub async fn box_stop_impl(agent_id: &str) -> Result<String> {
    let outcome = BoxRegistry::global()
        .stop(agent_id, StopReason::User)
        .await?;
    Ok(format!("{outcome:?}").to_ascii_lowercase())
}

/// `box_egress_allow(project_id, pattern)`: add one host pattern to the
/// project's allow-list, persist it, and update the project's running boxes.
/// Returns the list as stored.
pub fn box_egress_allow_impl(
    ctx: &EngineCtx,
    project_id: &str,
    pattern: &str,
) -> Result<Vec<String>> {
    let pattern: fletch_box::HostPattern = pattern
        .parse()
        .map_err(|e: fletch_box::egress::PatternError| Error::Other(e.to_string()))?;
    let mut patterns = settings::egress_allow(project_id);
    let spelled = pattern.to_string();
    if !patterns.contains(&spelled) {
        patterns.push(spelled);
    }
    let json = serde_json::to_string(&patterns)?;
    super::set_project_setting_impl(ctx, project_id, settings::EGRESS_ALLOW_KEY, Some(&json))?;
    settings::set_egress_allow(project_id, patterns.clone());
    BoxRegistry::global().update_egress(project_id);
    Ok(patterns)
}

/// Write one box setting (`boxes_enabled`, `box_memory_mib`, `box_cpus`,
/// `box_image_url`, `box_idle_stop_minutes`), validated, and mirror it for the
/// engine; `None` restores the default. The Developer pane's toggle and the
/// Sandbox page's knobs go through here, not the client-side `set_setting`,
/// since the engine reads these. The flag is the whole setup
/// ([`set_enabled`]): on, it starts the image install and makes `box` the
/// engine for new agents.
pub fn set_box_setting_impl(ctx: &EngineCtx, key: &str, value: Option<&str>) -> Result<()> {
    let normalized = match value {
        Some(value) => Some(
            settings::validate(key, value)
                .ok_or_else(|| Error::Other(format!("`{key}` is not a box setting")))??,
        ),
        None => {
            if !settings::KEYS.contains(&key) {
                return Err(Error::Other(format!("`{key}` is not a box setting")));
            }
            None
        }
    };
    if key == settings::ENABLED_SETTING {
        let on = normalized.as_deref() == Some("true");
        let probe = on.then(boxes::probe).flatten();
        set_enabled(ctx, on, probe.as_ref())?;
        if on {
            // The download, then the tools disks (`setup::prebuild_tools`);
            // quick when both are in place already.
            if let Err(e) = setup::start_install() {
                tracing::warn!(error = %e, "the box image install did not start");
            }
        }
        return Ok(());
    }
    {
        let conn = ctx.db.lock();
        match &normalized {
            Some(v) => database::set_setting(&conn, key, v)?,
            None => {
                conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
            }
        }
        settings::set(settings::load(&conn));
    }
    announce(ctx, key, normalized.as_deref());
    Ok(())
}

/// Flip the flag. On: refused, and nothing stored, unless `probe` says this
/// machine and runtime can boot a box (Intel, a missing helper); then `box`
/// becomes the engine for new agents, remembered as the flag's doing
/// (`box_engine_auto`). Off: the engine goes back to `sandbox-exec` only if
/// the flag set it and the user did not choose otherwise since, and every
/// running box is stopped cleanly — nothing is deleted. The install itself is
/// the caller's, so this stays testable without a download.
pub(crate) fn set_enabled(ctx: &EngineCtx, on: bool, probe: Option<&Probe>) -> Result<()> {
    if on {
        let problem = match probe {
            None => Some(boxes::RUNTIME_NOT_FOUND.to_string()),
            Some(probe) if !probe.ok() => Some(format!(
                "the box runtime is not usable: {}.",
                probe.problems().join("; ")
            )),
            Some(_) => None,
        };
        if let Some(why) = problem {
            return Err(Error::Other(why));
        }
    }
    let value = if on { "true" } else { "false" };
    {
        let conn = ctx.db.lock();
        database::set_setting(&conn, settings::ENABLED_SETTING, value)?;
        settings::set(settings::load(&conn));
    }
    announce(ctx, settings::ENABLED_SETTING, Some(value));

    let selected = sandbox::selected_engine_kind();
    if on {
        if selected != sandbox::EngineKind::Box {
            database::set_setting(&ctx.db.lock(), settings::ENGINE_AUTO_SETTING, "true")?;
            select_engine(ctx, sandbox::EngineKind::Box)?;
        }
        return Ok(());
    }
    let auto = {
        let conn = ctx.db.lock();
        let auto =
            database::get_setting(&conn, settings::ENGINE_AUTO_SETTING).as_deref() == Some("true");
        conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            [settings::ENGINE_AUTO_SETTING],
        )?;
        auto
    };
    if auto && selected == sandbox::EngineKind::Box {
        select_engine(ctx, sandbox::EngineKind::SandboxExec)?;
    }
    // Only when something runs: the stop is async, and a bare unit test has
    // no runtime to spawn it on.
    if !BoxRegistry::global().running_ids().is_empty() {
        crate::host::spawn(BoxRegistry::global().shutdown_all());
    }
    Ok(())
}

/// Persist, mirror and announce the engine new agents get, with none of the
/// probing `set_sandbox_engine_impl` does: the flag's own switch is already
/// behind the runtime probe, and the switch back is to the default.
fn select_engine(ctx: &EngineCtx, kind: sandbox::EngineKind) -> Result<()> {
    database::set_setting(&ctx.db.lock(), sandbox::ENGINE_SETTING, kind.as_setting())?;
    sandbox::set_selected_engine_kind(kind);
    announce(ctx, sandbox::ENGINE_SETTING, Some(kind.as_setting()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ctx::test_ctx;
    use serde_json::json;
    use std::path::PathBuf;

    use settings::TEST_LOCK as ENGINE_LOCK;

    fn probe(ok: bool) -> Probe {
        Probe {
            helper: PathBuf::from("/x/fletch-vm"),
            helper_present: true,
            helper_entitled: true,
            lib_dir: PathBuf::from("/x/lib"),
            dylibs_present: true,
            arch_ok: ok,
            hypervisor: true,
        }
    }

    fn engine_row(ctx: &EngineCtx) -> Option<String> {
        database::get_setting(&ctx.db.lock(), sandbox::ENGINE_SETTING)
    }

    #[test]
    fn a_box_setting_is_validated_persisted_mirrored_and_announced() {
        let (ctx, sink, _dir) = test_ctx();
        set_box_setting_impl(&ctx, settings::MEMORY_SETTING, Some("4096")).unwrap();
        assert_eq!(settings::current().memory_mib, 4096);
        assert_eq!(
            sink.events(),
            vec![(
                super::super::settings::SETTINGS_CHANGED.to_string(),
                json!({ "key": "box_memory_mib", "value": "4096" })
            )]
        );
        assert!(set_box_setting_impl(&ctx, settings::MEMORY_SETTING, Some("12")).is_err());
        assert_eq!(
            settings::current().memory_mib,
            4096,
            "a refused value changes nothing"
        );
        assert!(set_box_setting_impl(&ctx, "docker_image", Some("x")).is_err());
        set_box_setting_impl(&ctx, settings::MEMORY_SETTING, None).unwrap();
        assert_eq!(settings::current().memory_mib, settings::DEFAULT_MEMORY_MIB);
    }

    /// An Intel Mac, or one without the runtime, cannot turn the flag on: the
    /// refusal carries the probe's sentence and nothing is stored or announced.
    #[test]
    fn enabling_with_a_failing_probe_is_refused() {
        let _serial = ENGINE_LOCK.blocking_lock();
        let (ctx, sink, _dir) = test_ctx();
        settings::set(settings::BoxSettings::default());
        sandbox::set_selected_engine_kind(sandbox::EngineKind::SandboxExec);
        let err = set_enabled(&ctx, true, Some(&probe(false))).unwrap_err();
        assert!(err.to_string().contains("Apple silicon"), "{err}");
        let err = set_enabled(&ctx, true, None).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
        assert!(!settings::enabled());
        assert!(sink.events().is_empty());
        assert_eq!(engine_row(&ctx), None);
        assert_eq!(
            sandbox::selected_engine_kind(),
            sandbox::EngineKind::SandboxExec
        );
    }

    /// On makes `box` the engine and remembers that it did; off undoes exactly
    /// that, and both ends are announced.
    #[test]
    fn the_flag_switches_the_engine_to_box_and_back() {
        let _serial = ENGINE_LOCK.blocking_lock();
        let (ctx, sink, _dir) = test_ctx();
        settings::set(settings::BoxSettings::default());
        sandbox::set_selected_engine_kind(sandbox::EngineKind::SandboxExec);

        set_enabled(&ctx, true, Some(&probe(true))).unwrap();
        assert!(settings::enabled());
        assert_eq!(sandbox::selected_engine_kind(), sandbox::EngineKind::Box);
        assert_eq!(engine_row(&ctx).as_deref(), Some("box"));
        assert_eq!(
            database::get_setting(&ctx.db.lock(), settings::ENGINE_AUTO_SETTING).as_deref(),
            Some("true")
        );
        assert_eq!(
            sink.events(),
            vec![
                (
                    super::super::settings::SETTINGS_CHANGED.to_string(),
                    json!({ "key": "boxes_enabled", "value": "true" })
                ),
                (
                    super::super::settings::SETTINGS_CHANGED.to_string(),
                    json!({ "key": "sandbox_engine", "value": "box" })
                ),
            ]
        );

        set_enabled(&ctx, false, None).unwrap();
        assert!(!settings::enabled());
        assert_eq!(
            sandbox::selected_engine_kind(),
            sandbox::EngineKind::SandboxExec
        );
        assert_eq!(engine_row(&ctx).as_deref(), Some("sandbox-exec"));
        assert_eq!(
            database::get_setting(&ctx.db.lock(), settings::ENGINE_AUTO_SETTING),
            None,
            "the marker is cleared"
        );
        assert_eq!(sink.events().len(), 4, "the flag and the engine, twice");
    }

    /// A user who picked another engine in between keeps it: the flag only
    /// undoes its own switch.
    #[test]
    fn the_flag_leaves_an_engine_the_user_chose_meanwhile() {
        let _serial = ENGINE_LOCK.blocking_lock();
        let (ctx, _sink, _dir) = test_ctx();
        settings::set(settings::BoxSettings::default());
        sandbox::set_selected_engine_kind(sandbox::EngineKind::SandboxExec);

        set_enabled(&ctx, true, Some(&probe(true))).unwrap();
        // The user's own choice, as `set_sandbox_engine_impl` records it.
        database::set_setting(&ctx.db.lock(), sandbox::ENGINE_SETTING, "docker").unwrap();
        sandbox::set_selected_engine_kind(sandbox::EngineKind::Docker);
        ctx.db
            .lock()
            .execute(
                "DELETE FROM settings WHERE key = ?1",
                [settings::ENGINE_AUTO_SETTING],
            )
            .unwrap();

        set_enabled(&ctx, false, None).unwrap();
        assert_eq!(sandbox::selected_engine_kind(), sandbox::EngineKind::Docker);
        assert_eq!(engine_row(&ctx).as_deref(), Some("docker"));

        // Flag on with `box` chosen by hand: nothing to switch, nothing to
        // remember, so off leaves `box` as the user set it.
        sandbox::set_selected_engine_kind(sandbox::EngineKind::Box);
        set_enabled(&ctx, true, Some(&probe(true))).unwrap();
        assert_eq!(
            database::get_setting(&ctx.db.lock(), settings::ENGINE_AUTO_SETTING),
            None
        );
        set_enabled(&ctx, false, None).unwrap();
        assert_eq!(sandbox::selected_engine_kind(), sandbox::EngineKind::Box);
        sandbox::set_selected_engine_kind(sandbox::EngineKind::SandboxExec);
    }

    #[test]
    fn egress_allow_validates_persists_and_dedupes() {
        let (ctx, _sink, _dir) = test_ctx();
        ctx.db
            .lock()
            .execute(
                "INSERT INTO projects (id, name, created_at) VALUES ('pb', 'p', 0)",
                [],
            )
            .unwrap();
        assert!(box_egress_allow_impl(&ctx, "pb", "not a host").is_err());
        let list = box_egress_allow_impl(&ctx, "pb", "Example.com").unwrap();
        assert_eq!(list, vec!["example.com".to_string()]);
        let list = box_egress_allow_impl(&ctx, "pb", "example.com").unwrap();
        assert_eq!(list.len(), 1, "a repeat is not added twice");
        let stored = super::super::get_project_settings_impl(&ctx, "pb").unwrap();
        assert_eq!(
            stored.get(settings::EGRESS_ALLOW_KEY).unwrap(),
            r#"["example.com"]"#
        );
        assert_eq!(settings::egress_allow("pb"), list);
    }

    #[test]
    fn the_probe_reports_the_flag_the_image_and_the_tools() {
        let probe = box_runtime_probe_impl();
        assert_eq!(probe.enabled, settings::enabled());
        if !probe.enabled {
            assert!(!probe.ok);
        }
        assert_eq!(probe.runtime_ok, probe.problems.is_empty());
        assert!(!probe.image.pinned_version.is_empty());
        assert!(probe.tools.iter().any(|t| t.provider == "claude"));
    }
}

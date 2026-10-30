//! The engine halves of the box commands: what the Settings › Sandbox page,
//! the Developer pane and the agent's isolation row ask. `src-tauri` wraps
//! each in a `#[tauri::command]` and `remote::dispatch` in a remote op.

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
    crate::host::emit(
        ctx.sink.as_ref(),
        boxes::EVENT_POLICY,
        &boxes::PolicyEventPayload {
            project_id: Some(project_id.to_string()),
        },
    );
    Ok(patterns)
}

/// One template entry, for the Network lists.
#[derive(Debug, Clone, Serialize)]
pub struct TemplateEntry {
    pub pattern: &'static str,
    pub category: &'static str,
}

/// A project's egress allow-list, layer by layer.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectEgress {
    /// What its boxes may reach, each entry with the layer it came from.
    pub entries: Vec<boxes::allowlist::Entry>,
    /// Its settings files, read now.
    pub file: crate::project_file::ProjectFileStatus,
    /// The hosts added in the app before the settings files existed.
    pub in_app: Vec<String>,
}

/// `box_egress_overview(project_id?)`: what the Network lists show — the
/// template with its categories, the quiet list, the user's global changes,
/// the list every project starts from, and with a project, its own.
#[derive(Debug, Clone, Serialize)]
pub struct EgressOverview {
    pub template: Vec<TemplateEntry>,
    pub quiet: Vec<&'static str>,
    pub global: boxes::allowlist::Delta,
    /// The template with the global changes applied.
    pub base: Vec<boxes::allowlist::Entry>,
    pub project: Option<ProjectEgress>,
}

pub fn box_egress_overview_impl(
    ctx: &EngineCtx,
    project_id: Option<&str>,
) -> Result<EgressOverview> {
    use boxes::allowlist::{effective, Source};
    let global = settings::global_egress();
    let project = match project_id {
        Some(project_id) => {
            let (file, _) = crate::project_file::refresh(&ctx.db.lock(), project_id);
            Some(ProjectEgress {
                entries: boxes::registry::egress_entries(project_id),
                file,
                in_app: settings::egress_allow(project_id),
            })
        }
        None => None,
    };
    Ok(EgressOverview {
        template: fletch_box::egress::TEMPLATE
            .iter()
            .map(|(pattern, category)| TemplateEntry {
                pattern,
                category: category.as_str(),
            })
            .collect(),
        quiet: fletch_box::egress::QUIET.to_vec(),
        base: effective(&[(Source::Global, &global)]),
        global,
        project,
    })
}

/// `set_box_egress_global(delta)`: replace the user's global changes over the
/// template — hosts every project's boxes may also reach, template entries
/// turned off. Validated and normalized; every running box takes the new list
/// from its next connection. Answers what was stored.
pub fn set_box_egress_global_impl(
    ctx: &EngineCtx,
    delta: &boxes::allowlist::Delta,
) -> Result<boxes::allowlist::Delta> {
    let delta = delta.normalized()?;
    {
        let conn = ctx.db.lock();
        database::set_setting(
            &conn,
            boxes::allowlist::GLOBAL_SETTING,
            &serde_json::to_string(&delta)?,
        )?;
    }
    settings::set_global_egress(delta.clone());
    BoxRegistry::global().update_egress_all();
    crate::host::emit(
        ctx.sink.as_ref(),
        boxes::EVENT_POLICY,
        &boxes::PolicyEventPayload { project_id: None },
    );
    Ok(delta)
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
/// becomes the engine for new agents and the engine it replaced is remembered.
/// Off: `box` goes back to that engine (another engine the user picked
/// meanwhile stays), and every running box is stopped cleanly — nothing is
/// deleted. The install itself is the caller's, so this stays testable without
/// a download.
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
            // Remembered so providers boxes cannot run keep this engine, and
            // so off restores it (`settings::fallback_engine`).
            let conn = ctx.db.lock();
            database::set_setting(
                &conn,
                settings::PREVIOUS_ENGINE_SETTING,
                selected.as_setting(),
            )?;
            settings::set(settings::load(&conn));
            drop(conn);
            select_engine(ctx, sandbox::EngineKind::Box)?;
        }
        return Ok(());
    }
    // Off must never leave `box` selected: with the flag down the engine is
    // unavailable and every spawn would fail. The engine the flag replaced
    // comes back; another engine the user picked meanwhile is theirs and stays.
    let restore = settings::fallback_engine();
    {
        let conn = ctx.db.lock();
        conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            [settings::PREVIOUS_ENGINE_SETTING],
        )?;
        settings::set(settings::load(&conn));
    }
    if selected == sandbox::EngineKind::Box {
        select_engine(ctx, restore)?;
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

    /// On makes `box` the engine; off takes it back to the default, and both
    /// ends are announced.
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
        assert_eq!(sink.events().len(), 4, "the flag and the engine, twice");
    }

    /// A user who picked another engine in between keeps it; `box` chosen by
    /// hand still cannot survive the flag going off, since the engine is
    /// unavailable without it.
    #[test]
    fn the_flag_leaves_another_engine_but_never_leaves_box_selected() {
        let _serial = ENGINE_LOCK.blocking_lock();
        let (ctx, _sink, _dir) = test_ctx();
        settings::set(settings::BoxSettings::default());
        sandbox::set_selected_engine_kind(sandbox::EngineKind::SandboxExec);

        set_enabled(&ctx, true, Some(&probe(true))).unwrap();
        // The user's own choice, as `set_sandbox_engine_impl` records it.
        database::set_setting(&ctx.db.lock(), sandbox::ENGINE_SETTING, "docker").unwrap();
        sandbox::set_selected_engine_kind(sandbox::EngineKind::Docker);

        set_enabled(&ctx, false, None).unwrap();
        assert_eq!(sandbox::selected_engine_kind(), sandbox::EngineKind::Docker);
        assert_eq!(engine_row(&ctx).as_deref(), Some("docker"));

        sandbox::set_selected_engine_kind(sandbox::EngineKind::Box);
        set_enabled(&ctx, true, Some(&probe(true))).unwrap();
        set_enabled(&ctx, false, None).unwrap();
        assert_eq!(
            sandbox::selected_engine_kind(),
            sandbox::EngineKind::SandboxExec
        );
        assert_eq!(engine_row(&ctx).as_deref(), Some("sandbox-exec"));
    }

    /// Docker selected, flag on: a Codex agent (which boxes cannot run yet) is
    /// stamped Docker, not seatbelt, and off brings Docker back.
    #[test]
    fn the_engine_the_flag_replaced_serves_unsupported_providers_and_comes_back() {
        let _serial = ENGINE_LOCK.blocking_lock();
        let (ctx, _sink, _dir) = test_ctx();
        settings::set(settings::BoxSettings::default());
        database::set_setting(&ctx.db.lock(), sandbox::ENGINE_SETTING, "docker").unwrap();
        sandbox::set_selected_engine_kind(sandbox::EngineKind::Docker);

        set_enabled(&ctx, true, Some(&probe(true))).unwrap();
        assert_eq!(sandbox::selected_engine_kind(), sandbox::EngineKind::Box);
        assert_eq!(settings::fallback_engine(), sandbox::EngineKind::Docker);
        assert_eq!(
            boxes::engine_for_new_agent(sandbox::EngineKind::Box, "codex"),
            sandbox::EngineKind::Docker
        );
        assert_eq!(
            boxes::engine_for_new_agent(sandbox::EngineKind::Box, "claude"),
            sandbox::EngineKind::Box
        );

        set_enabled(&ctx, false, None).unwrap();
        assert_eq!(sandbox::selected_engine_kind(), sandbox::EngineKind::Docker);
        assert_eq!(engine_row(&ctx).as_deref(), Some("docker"));
        assert_eq!(
            database::get_setting(&ctx.db.lock(), settings::PREVIOUS_ENGINE_SETTING),
            None
        );
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

    /// The global list is validated, stored, mirrored and announced; the
    /// overview's base list and a project's entries both follow it, the
    /// project's own in-app additions on top.
    #[test]
    fn the_global_list_reaches_every_project() {
        use crate::sandbox::boxes::allowlist::{Delta, Source};
        let (ctx, sink, _dir) = test_ctx();
        ctx.db
            .lock()
            .execute(
                "INSERT INTO projects (id, name, created_at) VALUES ('pg', 'p', 0)",
                [],
            )
            .unwrap();
        let bad = Delta {
            add: vec!["not a host".into()],
            remove: Vec::new(),
        };
        assert!(set_box_egress_global_impl(&ctx, &bad).is_err());
        let stored = set_box_egress_global_impl(
            &ctx,
            &Delta {
                add: vec!["Internal.Example.com".into()],
                remove: vec!["rubygems.org".into()],
            },
        )
        .unwrap();
        assert_eq!(stored.add, vec!["internal.example.com"]);
        assert_eq!(
            Delta::parse(
                database::get_setting(
                    &ctx.db.lock(),
                    crate::sandbox::boxes::allowlist::GLOBAL_SETTING
                )
                .as_deref()
            ),
            stored
        );
        assert!(sink
            .events()
            .iter()
            .any(|(event, _)| event == boxes::EVENT_POLICY));

        box_egress_allow_impl(&ctx, "pg", "mine.example").unwrap();
        let overview = box_egress_overview_impl(&ctx, Some("pg")).unwrap();
        let base: Vec<&str> = overview.base.iter().map(|e| e.pattern.as_str()).collect();
        assert!(base.contains(&"internal.example.com"));
        assert!(!base.contains(&"rubygems.org"));
        let project = overview.project.unwrap();
        let mine = project
            .entries
            .iter()
            .find(|e| e.pattern == "mine.example")
            .unwrap();
        assert_eq!(mine.source, Source::Project);
        assert_eq!(project.in_app, vec!["mine.example"]);
        assert_eq!(overview.template.len(), fletch_box::egress::TEMPLATE.len());

        // Process-wide: leave it as the other tests expect it.
        settings::set_global_egress(Delta::default());
    }

    #[test]
    fn the_probe_reports_the_flag_the_image_and_the_tools() {
        // The flag is process-wide and the tests above flip it; the probe
        // reads it once and this test reads it again, so both reads happen
        // under the same lock those tests hold, or they can straddle a flip.
        let _serial = ENGINE_LOCK.blocking_lock();
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

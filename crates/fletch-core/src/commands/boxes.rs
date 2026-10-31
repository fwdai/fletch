//! The engine halves of the box commands: what the Settings › Sandbox page
//! and the agent's isolation row ask. `src-tauri` wraps each in a
//! `#[tauri::command]` and `remote::dispatch` in a remote op.

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
    /// The switch (`boxes_enabled`).
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
/// Refused on a machine that cannot boot a box (the desktop calls it blindly
/// when onboarding closes): the image would be dead weight there.
pub fn box_image_install_impl() -> Result<()> {
    if !boxes::runtime_usable() {
        return Err(Error::Other(match boxes::probe() {
            Some(probe) => format!(
                "the box runtime is not usable: {}.",
                probe.problems().join("; ")
            ),
            None => boxes::RUNTIME_NOT_FOUND.to_string(),
        }));
    }
    setup::start_install()
}

pub fn box_image_cancel_impl() -> Result<()> {
    setup::cancel_install()
}

/// `box_tools_retry(provider)`: build the provider's tools disk now, past the
/// backoff that holds builds off after failures; the outcome arrives as
/// `box:tools`. The Retry of a failed tools disk.
pub fn box_tools_retry_impl(provider: &str) -> Result<()> {
    setup::retry_tools(provider)
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
/// `box_image_url`, `box_idle_stop_minutes`, `box_layers`), validated, and
/// mirror it for the engine; `None` restores the default. The Sandbox page's
/// switch and knobs go through here, not the client-side `set_setting`,
/// since the engine reads these. The switch going on starts the image
/// install ([`set_enabled`]), and so does a layer turned on (the next boot of
/// every box attaches it); the engine choice itself is `set_sandbox_engine`'s.
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
    if key == settings::ENABLED_SETTING && normalized.is_some() {
        let on = normalized.as_deref() != Some("false");
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
    if key == settings::LAYERS_SETTING && settings::enabled() {
        if let Err(e) = setup::start_install() {
            tracing::warn!(error = %e, "the box layer install did not start");
        }
    }
    Ok(())
}

/// Flip the switch. On: refused, and nothing stored, unless `probe` says this
/// machine and runtime can boot a box (Intel, a missing helper); the engine
/// choice is untouched — automatic moves new agents to boxes on its own once
/// the image is in. Off: a pinned `box` goes back to automatic (which lands
/// on seatbelt while the switch is down; another pinned engine stays), and
/// every running box is stopped cleanly — nothing is deleted. The install
/// itself is the caller's, so this stays testable without a download.
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
    if on {
        return Ok(());
    }
    // Off must never leave `box` pinned: with the switch down the engine is
    // unavailable and every spawn would fail.
    if sandbox::selected_engine() == Some(sandbox::EngineKind::Box) {
        database::set_setting(
            &ctx.db.lock(),
            sandbox::ENGINE_SETTING,
            sandbox::ENGINE_AUTO,
        )?;
        sandbox::set_selected_engine(None);
        announce(ctx, sandbox::ENGINE_SETTING, Some(sandbox::ENGINE_AUTO));
    }
    // Only when something runs: the stop is async, and a bare unit test has
    // no runtime to spawn it on.
    if !BoxRegistry::global().running_ids().is_empty() {
        crate::host::spawn(BoxRegistry::global().shutdown_all());
    }
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

    /// An Intel Mac, or one without the runtime, cannot turn the switch on:
    /// the refusal carries the probe's sentence and nothing is stored or
    /// announced.
    #[test]
    fn enabling_with_a_failing_probe_is_refused() {
        let _serial = ENGINE_LOCK.blocking_lock();
        let (ctx, sink, _dir) = test_ctx();
        settings::set(settings::BoxSettings {
            enabled: false,
            ..Default::default()
        });
        sandbox::set_selected_engine(None);
        let err = set_enabled(&ctx, true, Some(&probe(false))).unwrap_err();
        assert!(err.to_string().contains("Apple silicon"), "{err}");
        let err = set_enabled(&ctx, true, None).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
        assert!(!settings::enabled());
        assert!(sink.events().is_empty());
        assert_eq!(engine_row(&ctx), None);
        assert_eq!(sandbox::selected_engine(), None);
        settings::set(settings::BoxSettings::default());
    }

    /// The switch never touches an automatic or hand-picked engine: on and
    /// off announce the switch alone, and automatic stays automatic.
    #[test]
    fn the_switch_leaves_the_engine_choice_alone() {
        let _serial = ENGINE_LOCK.blocking_lock();
        let (ctx, sink, _dir) = test_ctx();
        settings::set(settings::BoxSettings::default());
        sandbox::set_selected_engine(None);

        set_enabled(&ctx, false, None).unwrap();
        assert!(!settings::enabled());
        set_enabled(&ctx, true, Some(&probe(true))).unwrap();
        assert!(settings::enabled());
        assert_eq!(sandbox::selected_engine(), None);
        assert_eq!(engine_row(&ctx), None);
        assert_eq!(
            sink.events(),
            vec![
                (
                    super::super::settings::SETTINGS_CHANGED.to_string(),
                    json!({ "key": "boxes_enabled", "value": "false" })
                ),
                (
                    super::super::settings::SETTINGS_CHANGED.to_string(),
                    json!({ "key": "boxes_enabled", "value": "true" })
                ),
            ]
        );

        // A hand-picked Docker survives the switch going off.
        database::set_setting(&ctx.db.lock(), sandbox::ENGINE_SETTING, "docker").unwrap();
        sandbox::set_selected_engine(Some(sandbox::EngineKind::Docker));
        set_enabled(&ctx, false, None).unwrap();
        assert_eq!(
            sandbox::selected_engine(),
            Some(sandbox::EngineKind::Docker)
        );
        assert_eq!(engine_row(&ctx).as_deref(), Some("docker"));
        sandbox::set_selected_engine(None);
        settings::set(settings::BoxSettings::default());
    }

    /// A pinned `box` cannot survive the switch going off, since the engine is
    /// unavailable without it: it goes back to automatic, announced.
    #[test]
    fn the_switch_going_off_never_leaves_box_pinned() {
        let _serial = ENGINE_LOCK.blocking_lock();
        let (ctx, sink, _dir) = test_ctx();
        settings::set(settings::BoxSettings::default());
        database::set_setting(&ctx.db.lock(), sandbox::ENGINE_SETTING, "box").unwrap();
        sandbox::set_selected_engine(Some(sandbox::EngineKind::Box));

        set_enabled(&ctx, false, None).unwrap();
        assert_eq!(sandbox::selected_engine(), None);
        assert_eq!(engine_row(&ctx).as_deref(), Some("auto"));
        assert_eq!(
            sink.events().last().unwrap(),
            &(
                super::super::settings::SETTINGS_CHANGED.to_string(),
                json!({ "key": "sandbox_engine", "value": "auto" })
            )
        );
        settings::set(settings::BoxSettings::default());
    }

    /// The engine a new agent gets: a pinned engine is what it is (a pinned
    /// `box` for a provider boxes cannot run lands on seatbelt), and automatic
    /// is seatbelt whenever a box cannot boot right now — a project placed on
    /// this Mac, no image yet — and never waits.
    #[test]
    fn a_new_agent_gets_the_pinned_engine_or_what_can_run_now() {
        use crate::sandbox::placement::Placement;
        let _serial = ENGINE_LOCK.blocking_lock();
        settings::set(settings::BoxSettings {
            enabled: true,
            ..Default::default()
        });
        let engine = boxes::engine_for_new_agent;

        assert_eq!(
            engine(Some(sandbox::EngineKind::Docker), "claude", Placement::Auto),
            sandbox::EngineKind::Docker
        );
        assert_eq!(
            engine(Some(sandbox::EngineKind::Box), "claude", Placement::Host),
            sandbox::EngineKind::Box,
            "a pinned engine ignores the placement"
        );
        assert_eq!(
            engine(
                Some(sandbox::EngineKind::Box),
                "antigravity",
                Placement::Auto
            ),
            sandbox::EngineKind::SandboxExec
        );
        assert_eq!(
            engine(None, "claude", Placement::Host),
            sandbox::EngineKind::SandboxExec
        );
        // Automatic is whatever can boot right now and nothing else: on a
        // developer's Mac with the runtime, the image and the disk in place
        // that is a box, on every other machine (CI, a Linux host) seatbelt.
        let now = if boxes::ready_for("claude") {
            sandbox::EngineKind::Box
        } else {
            sandbox::EngineKind::SandboxExec
        };
        assert_eq!(engine(None, "claude", Placement::Auto), now);
        // A project pinned to boxes waits for the image like a pinned engine.
        assert_eq!(
            engine(None, "claude", Placement::Box),
            sandbox::EngineKind::Box
        );
        assert_eq!(
            engine(None, "antigravity", Placement::Box),
            sandbox::EngineKind::SandboxExec
        );
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
    fn the_probe_reports_the_switch_the_image_and_the_tools() {
        // The switch is process-wide and the tests above flip it; the probe
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

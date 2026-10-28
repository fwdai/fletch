//! The engine halves of the box commands (docs/boxes-spec.md §9): what the
//! Settings › Sandbox page, the Developer pane and the agent's isolation row
//! ask. Phase 4b wraps each in a `#[tauri::command]` and a remote op.

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::sandbox::boxes::{self, settings, BoxRegistry, StopReason, EVENT_IMAGE};
use crate::{database, sandbox};

use super::settings::announce;

/// `box_runtime_probe`: can this host boot a box, item by item.
#[derive(Debug, Clone, Serialize)]
pub struct BoxRuntimeProbe {
    /// The developer flag.
    pub enabled: bool,
    /// Every item passed and an image is installed.
    pub ok: bool,
    /// One sentence per failing item, the flag and image included.
    pub problems: Vec<String>,
    pub helper: Option<String>,
    pub lib_dir: Option<String>,
    pub arch_ok: bool,
    pub hypervisor: bool,
    pub image: BoxImageStatus,
}

/// `box_image_status` and the `box:image` event.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BoxImageStatus {
    /// `not_installed` | `downloading` | `verifying` | `installed` | `damaged` | `unavailable`
    pub phase: &'static str,
    /// Percent, while downloading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// The installed version, or the one being installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The version this build pins and where it downloads from.
    pub pinned_version: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The one install at a time, with its cancel flag.
struct Install {
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Option<u8>>>,
}

fn install_slot() -> &'static Mutex<Option<Install>> {
    static SLOT: OnceLock<Mutex<Option<Install>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn box_image_status_impl() -> BoxImageStatus {
    let pinned = boxes::pinned_image();
    let mut status = BoxImageStatus {
        phase: "not_installed",
        progress: None,
        version: None,
        pinned_version: pinned.version.clone(),
        url: pinned.url.clone(),
        error: None,
    };
    if let Some(install) = install_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        status.phase = "downloading";
        status.progress = *install.progress.lock().unwrap_or_else(|p| p.into_inner());
        status.version = Some(pinned.version);
        return status;
    }
    match boxes::image_store().current() {
        Ok(Some(image)) => {
            status.phase = "installed";
            status.version = Some(image.version);
        }
        Ok(None) => {}
        Err(e) => {
            status.phase = "unavailable";
            status.error = Some(e.to_string());
        }
    }
    status
}

pub fn box_runtime_probe_impl() -> BoxRuntimeProbe {
    let enabled = settings::enabled();
    let probe = boxes::probe();
    let image = box_image_status_impl();
    let mut problems = Vec::new();
    if !enabled {
        problems.push("box sandbox is off; enable it in Developer settings".to_string());
    }
    match &probe {
        Some(probe) => problems.extend(probe.problems()),
        None => problems.push(
            "the box runtime was not found: no fletch-vm helper in the app bundle, in \
             crates/fletch-box/target/release, or named by FLETCH_BOX_HELPER"
                .to_string(),
        ),
    }
    if image.phase != "installed" {
        problems.push("the box image is not installed".to_string());
    }
    BoxRuntimeProbe {
        enabled,
        ok: problems.is_empty(),
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
    }
}

/// Start downloading (or copying, for a `file://` URL) this build's pinned
/// image in the background; progress and the outcome arrive as `box:image`
/// events. Refused while an install is running.
pub fn box_image_install_impl(ctx: &EngineCtx) -> Result<()> {
    if !settings::enabled() {
        return Err(Error::Other(
            "box sandbox is off; enable it in Developer settings".into(),
        ));
    }
    let install = Install {
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(Mutex::new(Some(0))),
    };
    let (cancel, progress) = (install.cancel.clone(), install.progress.clone());
    {
        let mut slot = install_slot().lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_some() {
            return Err(Error::Other(
                "the box image is already being installed".into(),
            ));
        }
        *slot = Some(install);
    }
    let sink = ctx.sink.clone();
    let pinned = boxes::pinned_image();
    crate::host::spawn(async move {
        let emit = |status: &BoxImageStatus| crate::host::emit(sink.as_ref(), EVENT_IMAGE, status);
        let mut status = BoxImageStatus {
            phase: "downloading",
            progress: Some(0),
            version: Some(pinned.version.clone()),
            pinned_version: pinned.version.clone(),
            url: pinned.url.clone(),
            error: None,
        };
        emit(&status);
        let mut last = 0u8;
        let result = boxes::image_store()
            .install(&pinned, |p| {
                if cancel.load(Ordering::Relaxed) {
                    return ControlFlow::Break(());
                }
                let pct = (p.done * 100).checked_div(p.total).unwrap_or(0) as u8;
                if pct != last {
                    last = pct;
                    *progress.lock().unwrap_or_else(|e| e.into_inner()) = Some(pct);
                    status.progress = Some(pct);
                    emit(&status);
                }
                ControlFlow::Continue(())
            })
            .await;
        *install_slot().lock().unwrap_or_else(|p| p.into_inner()) = None;
        match result {
            Ok(image) => {
                status.phase = "installed";
                status.progress = None;
                status.version = Some(image.version);
            }
            Err(fletch_box::Error::Cancelled) => {
                status = box_image_status_impl();
            }
            Err(e) => {
                tracing::warn!(error = %e, "box image install failed");
                status.phase = "unavailable";
                status.progress = None;
                status.error = Some(e.to_string());
            }
        }
        emit(&status);
    });
    Ok(())
}

pub fn box_image_cancel_impl() -> Result<()> {
    match install_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        Some(install) => {
            install.cancel.store(true, Ordering::Relaxed);
            Ok(())
        }
        None => Err(Error::Other("no box image install is running".into())),
    }
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
/// since the engine reads these.
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
    // Turning the flag off while `box` is the selected engine would stamp new
    // agents with an engine that refuses to launch; fall back to the default.
    if key == settings::ENABLED_SETTING
        && !settings::enabled()
        && sandbox::selected_engine_kind() == sandbox::EngineKind::Box
    {
        let kind = sandbox::EngineKind::SandboxExec;
        database::set_setting(&ctx.db.lock(), sandbox::ENGINE_SETTING, kind.as_setting())?;
        sandbox::set_selected_engine_kind(kind);
        announce(ctx, sandbox::ENGINE_SETTING, Some(kind.as_setting()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ctx::test_ctx;
    use serde_json::json;

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

    /// Switching the flag off while `box` is selected moves the selection
    /// back to the default engine, so no new agent is stamped with an engine
    /// that cannot launch.
    #[test]
    fn disabling_the_flag_unselects_the_box_engine() {
        let (ctx, _sink, _dir) = test_ctx();
        set_box_setting_impl(&ctx, settings::ENABLED_SETTING, Some("true")).unwrap();
        sandbox::set_selected_engine_kind(sandbox::EngineKind::Box);
        set_box_setting_impl(&ctx, settings::ENABLED_SETTING, Some("false")).unwrap();
        assert_eq!(
            sandbox::selected_engine_kind(),
            sandbox::EngineKind::SandboxExec
        );
        assert_eq!(
            database::get_setting(&ctx.db.lock(), sandbox::ENGINE_SETTING).as_deref(),
            Some("sandbox-exec")
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

    #[test]
    fn the_probe_reports_the_flag_and_the_image() {
        let probe = box_runtime_probe_impl();
        assert_eq!(probe.enabled, settings::enabled());
        if !probe.enabled {
            assert!(!probe.ok);
            assert!(probe
                .problems
                .iter()
                .any(|p| p.contains("Developer settings")));
        }
        assert!(!probe.image.pinned_version.is_empty());
    }
}

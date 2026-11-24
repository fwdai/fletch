//! The engine halves of the box commands: what the Settings › Sandbox page,
//! the agent's isolation row and the composer's workspace type ask, and
//! connecting or disconnecting a cloud provider, which the spare cloud boxes
//! follow. `src-tauri` wraps each in a `#[tauri::command]` and
//! `remote::dispatch` the box ones in a remote op.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::cloud::{self, CloudProviderStatus, ProviderId};
use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::sandbox::boxes::{
    self, egress_log, egress_prompt, gate, registry::ToolsStatus, settings, setup, spares,
    BoxRegistry, StopReason,
};
use crate::workspace::WorkspaceManager;
use crate::{database, sandbox, DbState};

use super::settings::announce;

pub use crate::sandbox::boxes::setup::BoxImageStatus;

/// `box_runtime_probe`: can this host boot a box, item by item.
#[derive(Debug, Clone, Serialize)]
pub struct BoxRuntimeProbe {
    /// Every item passed, the image included.
    pub ok: bool,
    /// The machine and the runtime pass: Apple silicon, the hypervisor, the
    /// helper and its dylibs. What a box needs before anything else; the
    /// image and the tools disks are set up after it.
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
    /// This machine's logical cores and physical memory: what the Sandbox
    /// page's memory and CPU scales stop at. `None` where the OS will not say.
    pub host_cpus: Option<usize>,
    pub host_memory_mib: Option<u64>,
}

pub fn box_image_status_impl() -> BoxImageStatus {
    setup::image_status()
}

pub fn box_runtime_probe_impl() -> BoxRuntimeProbe {
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
        ok: runtime_ok && image.phase == "installed",
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
        host_cpus: std::thread::available_parallelism().ok().map(|n| n.get()),
        host_memory_mib: fletch_box::host_memory_mib(),
    }
}

/// Start downloading this build's pinned image in the background, then build
/// the tools disks; progress and the outcome arrive as `box:image` and
/// `box:tools` events. Idempotent, so it is also the Retry after a failure.
/// Refused on a machine that cannot boot a box: the image would be dead
/// weight there.
pub fn box_image_install_impl() -> Result<()> {
    if let Some(refusal) = boxes::runtime_refusal() {
        return Err(Error::Other(refusal));
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

/// `available_workspace_types(repo_path, provider)`: what the composer offers
/// a new agent of `provider` in the repo at `repo_path` — this Mac always, a
/// box with the reason when it cannot be had, and which one the spawn picks
/// with no choice made (`sandbox::workspace_types`). Off the async runtime:
/// the answer stats the repository and may run the runtime probe.
pub async fn available_workspace_types_impl(
    ctx: &EngineCtx,
    repo_path: &str,
    provider: &str,
) -> Result<Vec<sandbox::WorkspaceTypeOption>> {
    let placement = project_placement(ctx, repo_path);
    let (repo, provider) = (std::path::PathBuf::from(repo_path), provider.to_string());
    tokio::task::spawn_blocking(move || {
        sandbox::workspace_types(placement.as_deref(), &repo, &provider)
    })
    .await
    .map_err(|e| Error::Other(e.to_string()))
}

/// The stored `sandbox.placement` of the project the repo at `repo_path`
/// belongs to, as the spawn reads it; `None` for a repo no project owns yet.
/// A plain read: unlike the spawn, asking never creates the project.
fn project_placement(ctx: &EngineCtx, repo_path: &str) -> Option<String> {
    ctx.db
        .lock()
        .query_row(
            "SELECT ps.value FROM repos r \
             JOIN project_settings ps ON ps.project_id = r.project_id \
             WHERE r.path = ?1 AND ps.key = ?2",
            rusqlite::params![repo_path, sandbox::placement::KEY],
            |row| row.get::<_, String>(0),
        )
        .ok()
}

/// One VM on the Sandbox page's list.
#[derive(Debug, Clone, Serialize)]
pub struct BoxVm {
    pub agent_id: String,
    pub name: String,
    pub project_id: String,
    pub created_at: String,
    /// The cloud provider whose machine the VM is
    /// (`AgentRecord::cloud_provider`); `None` for one on this machine.
    pub cloud_provider: Option<String>,
    pub state: boxes::registry::BoxStateReport,
}

/// `box_list`: every VM that exists, running or not. Built from the disks and
/// the registry rather than the sidebar's snapshot, which leaves out hidden
/// workspaces such as a Roadmap chat. The registry adds a box still on its way
/// to a first disk; on its own it knows only what ran since launch.
pub fn box_list_impl(ctx: &EngineCtx) -> Result<Vec<BoxVm>> {
    let supervisor = ctx
        .supervisor()
        .ok_or_else(|| Error::Other("the engine is still starting".into()))?;
    let ids = boxes::created_box_ids().into_iter().chain(
        BoxRegistry::global()
            .identities()
            .into_iter()
            .map(|i| i.agent_id),
    );
    Ok(vms_of(&supervisor.workspace, ids))
}

/// `ids` as VMs, each once. `agent()` applies no sidebar filter, so a hidden
/// workspace comes through; one archived or gone does not.
fn vms_of(workspace: &WorkspaceManager, ids: impl Iterator<Item = String>) -> Vec<BoxVm> {
    let ids: std::collections::BTreeSet<String> = ids.collect();
    ids.into_iter()
        .filter_map(|id| {
            let record = workspace.agent(&id).ok()?;
            record.archive.is_none().then(|| BoxVm {
                state: BoxRegistry::global().report(&id),
                agent_id: id,
                name: record.name,
                project_id: record.project_id,
                created_at: record.created_at,
                cloud_provider: record.cloud_provider,
            })
        })
        .collect()
}

/// `box_stop(agent_id)`: stop the box now, reporting which rung worked. The
/// sessions in the box the stop takes are put to rest first
/// (`Supervisor::rest_sessions`): the user stops them with it. An agent with
/// no running box has nothing stopped, its session included.
pub async fn box_stop_impl(ctx: &Arc<EngineCtx>, agent_id: &str) -> Result<String> {
    let supervisor = ctx.supervisor();
    let outcome = BoxRegistry::global()
        .stop_with(agent_id, StopReason::User, |sessions| {
            if let Some(supervisor) = &supervisor {
                supervisor.rest_sessions(ctx, sessions);
            }
        })
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

/// `set_box_egress_global(delta)`: replace the user's global additions over
/// the template — hosts every project's boxes may also reach. Validated and
/// normalized; a `remove` (template entries turned off, a control that is
/// gone) is dropped on the way in. Every running box takes the new list from
/// its next connection. Answers what was stored.
pub fn set_box_egress_global_impl(
    ctx: &EngineCtx,
    delta: &boxes::allowlist::Delta,
) -> Result<boxes::allowlist::Delta> {
    let delta = boxes::allowlist::Delta {
        add: delta.normalized()?.add,
        remove: Vec::new(),
    };
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

/// `box_egress_pending`: the hosts boxes are held waiting to reach, oldest
/// first, for a client that connected after the prompts were raised.
pub fn box_egress_pending_impl() -> Vec<egress_prompt::PendingEgressPrompt> {
    egress_prompt::pending()
}

/// One answer to a `box:egress-requested` prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressAnswer {
    /// Allow the host for the project, in its in-app list (`box.egress_allow`).
    Project,
    /// Allow the host in every project (the global list).
    Always,
    /// Refuse it on this box until the box restarts.
    Block,
    /// Open this box's network until the turn ends.
    Turn,
}

/// `box_egress_answer(id, answer)`: answer one held host. The allow travels
/// the list it lands in, so every box of that scope takes it; the held
/// connections follow at once. An id the host no longer holds (answered
/// elsewhere, expired) is ignored.
pub fn box_egress_answer_impl(ctx: &EngineCtx, id: &str, answer: EgressAnswer) -> Result<()> {
    let Some(prompt) = egress_prompt::get(id) else {
        return Ok(());
    };
    let outcome = match answer {
        EgressAnswer::Project => {
            box_egress_allow_impl(ctx, &prompt.project_id, &prompt.host)?;
            egress_prompt::Outcome::Allowed
        }
        EgressAnswer::Always => {
            let mut global = settings::global_egress();
            if !global.add.contains(&prompt.host) {
                global.add.push(prompt.host.clone());
            }
            set_box_egress_global_impl(ctx, &global)?;
            egress_prompt::Outcome::Allowed
        }
        EgressAnswer::Turn => {
            BoxRegistry::global().set_turn_open(&prompt.agent_id, true);
            egress_prompt::Outcome::Allowed
        }
        EgressAnswer::Block => {
            // Closed before the refusal lands, so the refusal closes nothing
            // twice and the card says "blocked", not "expired".
            egress_prompt::resolve(&prompt.id, egress_prompt::Outcome::Blocked);
            BoxRegistry::global().decline_host(&prompt.agent_id, &prompt.host);
            return Ok(());
        }
    };
    egress_prompt::resolve(&prompt.id, outcome);
    Ok(())
}

/// `box_egress_activity(project_id?, hours?)`: the hosts boxes reached or were
/// refused in the last `hours` (24 by default, a month at most), newest
/// first, grouped by host; with a project, its boxes only.
pub fn box_egress_activity_impl(
    ctx: &EngineCtx,
    project_id: Option<&str>,
    hours: Option<u64>,
) -> Result<Vec<egress_log::HostActivity>> {
    let hours = hours.unwrap_or(24).clamp(1, 24 * 30) as i64;
    let since = database::now_millis() - hours * 3_600_000;
    egress_log::activity(&ctx.db.lock(), project_id, since)
}

/// `box_egress_open_turn(agent_id, open)`: open the agent's box's network
/// until its turn ends, or close it again. Refused for a workspace with no
/// box.
pub fn box_egress_open_turn_impl(agent_id: &str, open: bool) -> Result<()> {
    if BoxRegistry::global().set_turn_open(agent_id, open) {
        Ok(())
    } else {
        Err(Error::Other(
            "this workspace does not run in a Linux VM".into(),
        ))
    }
}

/// Write one box setting (`box_memory_mib`, `box_cpus`, `box_disk_gib`,
/// `box_image_url`, `box_idle_stop_minutes`, `box_egress_mode`,
/// `box_cloud_spares`), validated, and mirror it for the engine; `None`
/// restores the default. The Sandbox page's knobs go through here, not the
/// client-side `set_setting`, since the engine reads these. A new posture
/// reaches every running box at once, and a new number of spare cloud boxes
/// is made or destroyed at once.
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
    if key == settings::EGRESS_MODE_SETTING {
        BoxRegistry::global().update_egress_all();
    }
    if key == settings::CLOUD_SPARES_SETTING {
        spares::replenish_in_background();
    }
    announce(ctx, key, normalized.as_deref());
    Ok(())
}

/// `cloud_connect(provider, token, account)`: connect a cloud provider with a
/// pasted token (`cloud::connect`, which checks it with the provider before
/// storing anything), then bring the provider's spare cloud boxes to their
/// number in the background (`spares::replenish`). This machine's only:
/// never on the remote wire.
pub async fn cloud_connect_impl(
    db: &DbState,
    provider: ProviderId,
    token: &str,
    account: Option<&str>,
) -> Result<CloudProviderStatus> {
    connected_then(
        cloud::connect(db, provider, token, account).await,
        spares::replenish_in_background,
    )
}

/// A connect's answer, with `replenish` run once it succeeded.
fn connected_then(
    connected: Result<CloudProviderStatus>,
    replenish: impl FnOnce(),
) -> Result<CloudProviderStatus> {
    let status = connected?;
    replenish();
    Ok(status)
}

/// `cloud_disconnect(provider)`: refused while this machine's cloud runner
/// is at the provider (`cloud_runner::check_disconnect`: the runner can only
/// be managed through that connection), asked at once and again once the
/// hold below has waited for any machine being made. Else the provider is
/// held for the sweep (`boxes::gate`: no machine is made there from then on,
/// and one being made is waited for), its spare cloud boxes are destroyed
/// (`spares::destroy_all`), its token and account forgotten
/// (`cloud::disconnect`), and only then is the hold released. A spare is
/// billed while it exists and only a connected provider can destroy it, so
/// while one cannot be destroyed the provider stays connected and the
/// disconnect is refused, to be tried again: the hold is released and a
/// replenish asked for, leaving the provider as it was. With no spares it is
/// the provider's disconnect alone. The boxes of workspaces at the provider
/// are not touched. This machine's only: never on the remote wire.
pub async fn cloud_disconnect_impl(db: &DbState, provider: ProviderId) -> Result<()> {
    let no_runner = || crate::sandbox::cloud_runner::check_disconnect(provider);
    no_runner()?;
    disconnect_held(
        db,
        provider,
        no_runner,
        spares::destroy_all(Some(provider)),
        spares::replenish_in_background,
    )
    .await
}

/// [`cloud_disconnect_impl`] past its first runner check: the provider held,
/// `allowed` asked again (a runner whose machine was being made when the hold
/// was taken has its handle on disk once the hold has waited for it), then
/// [`disconnect_after`] with `destroyed`, the destruction of its spares, and
/// the hold released once the connection is forgotten. A refusal, or a
/// machine still being made there when the hold's wait ran out, releases the
/// hold and runs `replenish`.
async fn disconnect_held(
    db: &DbState,
    provider: ProviderId,
    allowed: impl FnOnce() -> Result<()>,
    destroyed: impl std::future::Future<Output = Result<()>>,
    replenish: impl FnOnce(),
) -> Result<()> {
    let hold = gate::hold(Some(provider)).await;
    let done = if hold.settled() {
        match allowed() {
            Ok(()) => disconnect_after(db, provider, destroyed).await,
            Err(e) => Err(e),
        }
    } else {
        Err(Error::Other(format!(
            "{} is still connected: a machine was still being made there. Try disconnecting \
             again in a moment.",
            provider.label()
        )))
    };
    if done.is_err() {
        drop(hold);
        replenish();
    }
    done
}

/// [`cloud_disconnect_impl`] once `destroyed`, the destruction of the
/// provider's spares, has succeeded.
async fn disconnect_after(
    db: &DbState,
    provider: ProviderId,
    destroyed: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    if let Err(e) = destroyed.await {
        return Err(Error::Other(format!(
            "{label} is still connected: its spare machines could not all be deleted, and \
             {label} bills them while they exist. Try disconnecting again in a moment. ({e})",
            label = provider.label()
        )));
    }
    cloud::disconnect(&db.lock(), provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ctx::test_ctx;
    use crate::workspace::tests::{mk_repo, seed_repo, test_db};
    use crate::workspace::{new_agent_record, AgentRecord, PURPOSE_ROADMAP_PM};
    use serde_json::json;

    /// The list is the VMs that exist, not the sidebar: a Roadmap chat, which
    /// the sidebar hides, is on it, and an archived workspace or an id with no
    /// row is not.
    #[test]
    fn the_vm_list_keeps_hidden_workspaces_and_drops_archived_ones() {
        let db = test_db();
        let wm = WorkspaceManager::new(db.clone());
        seed_repo(&db, "/r");
        let record = |name: &str| -> AgentRecord {
            new_agent_record(
                name.into(),
                name.into(),
                "claude".into(),
                mk_repo("/r"),
                "task".into(),
            )
        };
        let mut chat = record("shasta");
        chat.purpose = Some(PURPOSE_ROADMAP_PM.to_string());
        wm.add_agent(&mut chat).unwrap();
        let mut plain = record("denali");
        wm.add_agent(&mut plain).unwrap();
        let mut archived = record("rainier");
        wm.add_agent(&mut archived).unwrap();
        wm.begin_archive(&archived.id).unwrap();
        wm.finish_archive(&archived.id, &[], None).unwrap();

        assert!(wm.current().unwrap().agents.iter().all(|a| a.id != chat.id));

        let ids = [
            chat.id.clone(),
            plain.id.clone(),
            plain.id.clone(),
            archived.id.clone(),
            "gone".to_string(),
        ];
        let mut names: Vec<String> = vms_of(&wm, ids.into_iter())
            .into_iter()
            .map(|v| v.name)
            .collect();
        names.sort();
        assert_eq!(names, ["denali", "shasta"]);
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

    /// A database connected to `fake`'s account, and a spares root holding one
    /// spare made there and one directory with nothing made, no provider's.
    async fn connected_with_a_spare(
        fake: &crate::cloud::fake::FakeCloud,
    ) -> (
        tempfile::TempDir,
        DbState,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db = database::init(dir.path()).unwrap();
        let provider = fake.client().provider();
        let connection =
            cloud::Connection::new(provider, "fake-token-0123456789", Some("acme")).unwrap();
        cloud::connect_with(&db, connection, &fake.client())
            .await
            .unwrap();
        let root = dir.path().join("box-spares");
        let spare = root.join("spare-0123456789ab");
        std::fs::create_dir_all(&spare).unwrap();
        let made = fake
            .client()
            .create(&cloud::MachineSpec {
                name: "spare-0123456789ab".into(),
                image: "registry.test/fletch-cloud:1".into(),
                env: Vec::new(),
                cpus: 1,
                memory_mib: 256,
                disk_gib: None,
                ssh_port: 2222,
                keep_running: false,
            })
            .await
            .unwrap();
        std::fs::write(
            spare.join(boxes::cloud::HANDLE_FILE),
            format!("{}\n", made.handle),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("spare-000000000000")).unwrap();
        (dir, db, root, spare)
    }

    /// Connecting a provider asks for its spares once it is connected; a
    /// connect the provider refused asks for nothing.
    #[tokio::test]
    async fn connecting_a_provider_asks_for_its_spares() {
        let fake = crate::cloud::fake::FakeCloud::start().await;
        let dir = tempfile::tempdir().unwrap();
        let db = database::init(dir.path()).unwrap();
        let provider = fake.client().provider();
        let asked = std::cell::Cell::new(0);

        let connection =
            cloud::Connection::new(provider, "fake-token-0123456789", Some("acme")).unwrap();
        let status = connected_then(
            cloud::connect_with(&db, connection, &fake.client()).await,
            || asked.set(asked.get() + 1),
        )
        .unwrap();
        assert!(status.connected);
        assert_eq!(asked.get(), 1);

        let refused = connected_then(Err(Error::Other("the token was refused".into())), || {
            asked.set(asked.get() + 1)
        });
        assert!(refused.is_err());
        assert_eq!(asked.get(), 1, "nothing asked for a refused connect");
    }

    /// Disconnecting a provider destroys its spares while it is still
    /// connected, then forgets it; a directory that is no provider's spare is
    /// left alone, and with no spare left the next disconnect is the
    /// provider's alone.
    #[tokio::test]
    async fn a_disconnect_destroys_the_providers_spares_first() {
        let fake = crate::cloud::fake::FakeCloud::start().await;
        let (_dir, db, root, spare) = connected_with_a_spare(&fake).await;
        let provider = fake.client().provider();
        let client_for = |_: ProviderId| -> Result<cloud::Client> {
            assert!(
                cloud::connection(&db.lock(), provider).unwrap().is_some(),
                "a spare is destroyed while its provider is connected"
            );
            Ok(fake.client())
        };

        disconnect_after(
            &db,
            provider,
            spares::destroy_all_in(&root, Some(provider), &client_for),
        )
        .await
        .unwrap();
        assert_eq!(fake.boxes(), 0);
        assert!(!spare.exists());
        assert!(root.join("spare-000000000000").is_dir(), "no provider's");
        assert!(cloud::connection(&db.lock(), provider).unwrap().is_none());

        let connection =
            cloud::Connection::new(provider, "fake-token-0123456789", Some("acme")).unwrap();
        cloud::connect_with(&db, connection, &fake.client())
            .await
            .unwrap();
        disconnect_after(
            &db,
            provider,
            spares::destroy_all_in(&root, Some(provider), &client_for),
        )
        .await
        .unwrap();
        assert!(cloud::connection(&db.lock(), provider).unwrap().is_none());
    }

    /// A disconnect whose spare cannot be destroyed is refused: the provider
    /// stays connected, the spare's handle is kept for the next try, and the
    /// refusal says so in words a person can act on.
    #[tokio::test]
    async fn a_disconnect_whose_spare_cannot_be_destroyed_is_refused() {
        let fake = crate::cloud::fake::FakeCloud::start().await;
        let (_dir, db, root, spare) = connected_with_a_spare(&fake).await;
        let provider = fake.client().provider();
        fake.refuse_destroy();

        let err = disconnect_after(
            &db,
            provider,
            spares::destroy_all_in(&root, Some(provider), &|_| Ok(fake.client())),
        )
        .await
        .unwrap_err()
        .to_string();
        for said in [
            "is still connected",
            "bills them while they exist",
            "Try disconnecting again",
        ] {
            assert!(err.contains(said), "{said:?} in {err}");
        }
        assert!(err.contains(provider.label()), "{err}");
        assert!(cloud::connection(&db.lock(), provider).unwrap().is_some());
        assert!(spare.join(boxes::cloud::HANDLE_FILE).is_file());
        assert_eq!(fake.boxes(), 1);
    }

    /// A disconnect holds its provider (no machine made there) from before
    /// its spares are destroyed until its connection is forgotten. A refused
    /// one lets go of the hold and asks for a replenish, so the provider is
    /// as it was; a done one asks for none.
    #[tokio::test]
    async fn a_disconnect_holds_its_provider_until_it_is_forgotten() {
        gate::with_own_gate(async {
            let fake = crate::cloud::fake::FakeCloud::start().await;
            let (_dir, db, _root, _spare) = connected_with_a_spare(&fake).await;
            let provider = fake.client().provider();
            let asked = std::cell::Cell::new(0);

            let refused = disconnect_held(
                &db,
                provider,
                || Ok(()),
                async {
                    assert!(gate::make(provider).is_err(), "held while it sweeps");
                    Err(Error::Other("a spare is kept".into()))
                },
                || asked.set(asked.get() + 1),
            )
            .await;
            assert!(refused.is_err());
            assert!(!gate::held(provider), "a refused disconnect lets go");
            assert_eq!(asked.get(), 1, "and asks for the spares again");
            assert!(cloud::connection(&db.lock(), provider).unwrap().is_some());

            // A runner whose machine was made while the hold waited: asked
            // under the hold, nothing is destroyed and the provider stays.
            let refused = disconnect_held(
                &db,
                provider,
                || {
                    assert!(gate::held(provider), "asked once the hold stands");
                    Err(Error::Other("a runner is there".into()))
                },
                async { panic!("nothing is destroyed under a runner") },
                || asked.set(asked.get() + 1),
            )
            .await;
            assert!(refused.is_err());
            assert!(!gate::held(provider));
            assert_eq!(asked.get(), 2);
            assert!(cloud::connection(&db.lock(), provider).unwrap().is_some());

            disconnect_held(
                &db,
                provider,
                || Ok(()),
                async {
                    assert!(gate::held(provider), "held while it sweeps");
                    Ok(())
                },
                || asked.set(asked.get() + 1),
            )
            .await
            .unwrap();
            assert!(cloud::connection(&db.lock(), provider).unwrap().is_none());
            assert!(!gate::held(provider), "let go once forgotten");
            assert_eq!(
                asked.get(),
                2,
                "no spares asked for a provider disconnected"
            );
        })
        .await;
    }

    /// The engine a new agent gets on a Mac: seatbelt whenever a box cannot
    /// boot right now — a project placed on this Mac, no image yet — never
    /// waiting.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_new_agent_gets_what_can_run_now() {
        use crate::sandbox::placement::Placement;
        let _serial = settings::TEST_LOCK.blocking_lock();
        settings::set(settings::BoxSettings::default());
        let engine = boxes::engine_for_new_agent;

        assert_eq!(
            engine("claude", Placement::Host),
            sandbox::EngineKind::SandboxExec
        );
        // Automatic is whatever can boot right now and nothing else: on a
        // developer's Mac with the runtime, the image and the disk in place
        // that is a box, on every other Mac seatbelt.
        let now = if boxes::ready_for("claude") {
            sandbox::EngineKind::Box
        } else {
            sandbox::EngineKind::SandboxExec
        };
        assert_eq!(engine("claude", Placement::Auto), now);
        // A project pinned to boxes waits for the image wherever a box can
        // boot at all.
        let pinned = if boxes::runtime_usable() {
            sandbox::EngineKind::Box
        } else {
            sandbox::EngineKind::SandboxExec
        };
        assert_eq!(engine("claude", Placement::Box), pinned);
        assert_eq!(
            engine("antigravity", Placement::Box),
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
    /// project's own in-app additions on top. A `remove` is dropped on the
    /// way in: the control that wrote one is gone, and the template entry
    /// stays allowed.
    #[test]
    fn the_global_list_reaches_every_project() {
        use crate::sandbox::boxes::allowlist::{Delta, Source};
        // The global list's mirror is process-wide (`settings::TEST_LOCK`).
        let _serial = settings::TEST_LOCK.blocking_lock();
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
        assert!(stored.remove.is_empty(), "a turn-off is not stored");
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
        assert!(base.contains(&"rubygems.org"), "the template entry stays");
        assert!(overview.global.remove.is_empty());
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

    /// A held host answered for the project lands in the project's in-app
    /// list, answered for every project in the global list, and a block
    /// closes its prompt; each prompt closes once, and a late answer to a
    /// closed one changes nothing.
    #[test]
    fn a_held_host_is_answered_into_a_list_and_its_prompt_closes() {
        use crate::sandbox::boxes::allowlist::{Delta, GLOBAL_SETTING};
        use crate::sandbox::boxes::egress_prompt;
        let _serial = settings::TEST_LOCK.blocking_lock();
        let (ctx, _sink, _dir) = test_ctx();
        ctx.db
            .lock()
            .execute(
                "INSERT INTO projects (id, name, created_at) VALUES ('pa', 'p', 0)",
                [],
            )
            .unwrap();
        egress_prompt::request("held-fuji", "pa", "api.stripe.com", 443, 1);
        egress_prompt::request("held-fuji", "pa", "docs.python.org", 443, 2);
        egress_prompt::request("held-etna", "pa", "evil.test", 443, 1);
        let mine = |host: &str| {
            egress_prompt::pending()
                .into_iter()
                .find(|p| p.host == host && p.project_id == "pa")
                .unwrap()
        };

        let stripe = mine("api.stripe.com");
        box_egress_answer_impl(&ctx, &stripe.id, EgressAnswer::Project).unwrap();
        assert_eq!(
            settings::egress_allow("pa"),
            vec!["api.stripe.com".to_string()]
        );
        assert!(
            egress_prompt::get(&stripe.id).is_none(),
            "the prompt closed"
        );
        box_egress_answer_impl(&ctx, &stripe.id, EgressAnswer::Always).unwrap();
        assert_eq!(
            settings::egress_allow("pa").len(),
            1,
            "a late answer changes nothing"
        );

        let docs = mine("docs.python.org");
        box_egress_answer_impl(&ctx, &docs.id, EgressAnswer::Always).unwrap();
        let stored = Delta::parse(database::get_setting(&ctx.db.lock(), GLOBAL_SETTING).as_deref());
        assert!(stored.add.contains(&"docs.python.org".to_string()));
        assert!(egress_prompt::get(&docs.id).is_none());

        let evil = mine("evil.test");
        box_egress_answer_impl(&ctx, &evil.id, EgressAnswer::Block).unwrap();
        assert!(egress_prompt::get(&evil.id).is_none());
        assert!(!settings::egress_allow("pa").contains(&"evil.test".to_string()));

        // Process-wide: leave it as the other tests expect it.
        settings::set_global_egress(Delta::default());
    }

    /// The posture is a validated box setting with a per-project override that
    /// is validated the same way, spelled the one way when stored, mirrored
    /// for the proxies, and cleared back to the global by a `null`.
    #[test]
    fn the_posture_is_validated_globally_and_per_project() {
        use crate::sandbox::boxes::settings::{EgressMode, EGRESS_MODE_KEY, EGRESS_MODE_SETTING};
        let _serial = settings::TEST_LOCK.blocking_lock();
        let (ctx, _sink, _dir) = test_ctx();
        ctx.db
            .lock()
            .execute(
                "INSERT INTO projects (id, name, created_at) VALUES ('pp', 'p', 0)",
                [],
            )
            .unwrap();
        assert!(set_box_setting_impl(&ctx, EGRESS_MODE_SETTING, Some("yolo")).is_err());
        set_box_setting_impl(&ctx, EGRESS_MODE_SETTING, Some(" ask ")).unwrap();
        assert_eq!(
            database::get_setting(&ctx.db.lock(), EGRESS_MODE_SETTING).as_deref(),
            Some("ask"),
            "stored trimmed"
        );
        // The in-process mirror is shared with every other test's `set`, so the
        // project is checked against whatever the global reads right now.
        assert_eq!(settings::egress_mode("pp"), settings::current().egress_mode);

        assert!(
            super::super::set_project_setting_impl(&ctx, "pp", EGRESS_MODE_KEY, Some("never"))
                .is_err()
        );
        super::super::set_project_setting_impl(&ctx, "pp", EGRESS_MODE_KEY, Some("allowlist "))
            .unwrap();
        assert_eq!(settings::egress_mode("pp"), EgressMode::AllowList);
        assert_eq!(
            super::super::get_project_settings_impl(&ctx, "pp")
                .unwrap()
                .get(EGRESS_MODE_KEY)
                .map(String::as_str),
            Some("allowlist"),
            "stored in the one spelling"
        );
        super::super::set_project_setting_impl(&ctx, "pp", EGRESS_MODE_KEY, None).unwrap();
        assert_eq!(
            settings::egress_mode("pp"),
            settings::current().egress_mode,
            "back to the global"
        );

        set_box_setting_impl(&ctx, EGRESS_MODE_SETTING, None).unwrap();
        assert_eq!(
            database::get_setting(&ctx.db.lock(), EGRESS_MODE_SETTING),
            None,
            "a null restores the default"
        );
        assert_eq!(settings::load(&ctx.db.lock()).egress_mode, EgressMode::Open);
        settings::set(settings::BoxSettings::default());
    }

    #[test]
    fn the_probe_reports_the_runtime_the_image_and_the_tools() {
        let probe = box_runtime_probe_impl();
        if !probe.runtime_ok {
            assert!(!probe.ok);
        }
        assert_eq!(probe.runtime_ok, probe.problems.is_empty());
        assert!(!probe.image.pinned_version.is_empty());
        assert!(probe.tools.iter().any(|t| t.provider == "claude"));
    }
}

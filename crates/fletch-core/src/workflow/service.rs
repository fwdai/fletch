//! `WorkflowService` (host-managed state): launching, cancelling and deleting
//! runs, and the registry of drive tasks — one per active run, each driving the
//! kernel (`runner`). Panic containment (§6.1): a watchdog awaits each drive
//! task's `JoinHandle`; a panicked task marks its run
//! `failed("internal scheduler error")` so a run is never left `running` with no
//! live driver.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::sandbox::{self, EngineKind};
use crate::supervisor::Supervisor;

use super::cleanup::{
    check_deletable, delete_run_data, discard_all, recover_staged_run_dirs, stage_run_dirs,
    ProjectRunCleanup,
};
use super::driver::AgentDriver;
use super::journal::{fail_run, set_status};
use super::runner::{live_step_agents, run_kernel, RunCtx};
use super::spec::{self, Spec};
use super::{blackboard, gitops, journal, Db};

/// Live drive tasks, keyed by run id, each holding its run's cancel flag.
type Drivers = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

/// App-state singleton: the active-run registry plus launch / control.
pub struct WorkflowService {
    db: Db,
    driver: Arc<dyn AgentDriver>,
    engine: Arc<EngineCtx>,
    /// Behind an `Arc` so a drive task's watchdog can remove its own entry on
    /// exit without borrowing the service.
    runs: Drivers,
    /// Serializes workflow creation/deletion. Project deletion holds this from
    /// its run preflight through the project commit, so no new run can appear
    /// between cleanup and deletion.
    pub(crate) lifecycle: tokio::sync::Mutex<()>,
}

impl WorkflowService {
    pub fn new(db: Db, driver: Arc<dyn AgentDriver>, engine: Arc<EngineCtx>) -> Self {
        Self {
            db,
            driver,
            engine,
            runs: Arc::new(Mutex::new(HashMap::new())),
            lifecycle: tokio::sync::Mutex::new(()),
        }
    }

    /// Launch a run from a snapshot `spec` against `repo_path`. Provisions the
    /// run directory (blackboard + run repo), inserts the `wf_run` row, and
    /// spawns its drive task. Returns the new run id.
    #[allow(clippy::too_many_arguments)]
    pub async fn launch(
        &self,
        spec: Spec,
        task: String,
        project_id: String,
        repo_path: String,
        definition_id: Option<String>,
        base_branch: Option<String>,
        // Launch-time file attachments (absolute paths). Persisted durably in a
        // host-only sidecar and rendered as `Attached file: {path}` lines into the
        // entry step's prompt only — the same form a chat message delivers them,
        // scoped to the first agent.
        attachments: Vec<String>,
        // The GitHub issue this run was started from (Home-inbox "Start work" →
        // Pipeline), as a bare issue number. Threaded onto the run row so the
        // finalize open-PR path can append a `Closes #<n>` trailer. `None` for a
        // normal launch.
        issue_ref: Option<String>,
        // The roadmap item this run was dispatched for (the queue drainer,
        // `roadmap::drainer`). Written in the same INSERT as the rest of the
        // row rather than patched in afterwards, so a crash between launch and
        // the drainer's next write can never orphan a run from the item that
        // owns it — the drainer counts a project's live runs through this
        // column. `None` for every human-initiated launch.
        roadmap_item_id: Option<String>,
    ) -> Result<String> {
        spec::validate(&spec).map_err(|errs| Error::Other(errs.join("; ")))?;
        // Every step adopts the run's host-side workspace, and an adopting
        // agent runs where its tree lives: on this Mac's sandbox, never in a
        // box (`sandbox::engine_for_spawn`). So a project whose agents would
        // land in a box is refused here, with why, rather than have its
        // workflow run on the host behind the isolation it chose. Before the
        // lifecycle lock, like a spawn: the automatic choice stats the
        // repository and may probe the runtime once.
        let placement_setting = {
            let conn = self.db.lock();
            project_setting(&conn, &project_id, sandbox::placement::KEY)
        };
        let providers = step_providers(&spec);
        let repo_for_placement = PathBuf::from(&repo_path);
        let boxed = tokio::task::spawn_blocking(move || {
            lands_in_a_box(
                placement_setting.as_deref(),
                &repo_for_placement,
                providers.iter().map(String::as_str),
                sandbox::boxes::ready_for,
            )
        })
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
        if boxed {
            return Err(Error::Other(BOX_REFUSAL.to_string()));
        }
        let _lifecycle_guard = self.lifecycle.lock().await;
        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        let repo = PathBuf::from(&repo_path);

        // Resolve the fork point to a SHA in the source repo now, so it is fixed
        // and journaled (§12.2): the caller's branch, the spec's pr_base, then HEAD.
        let base_ref = base_branch
            .clone()
            .or_else(|| spec.finalize.as_ref().and_then(|f| f.pr_base.clone()))
            .unwrap_or_else(|| "HEAD".to_string());
        let base_sha = crate::git::rev_parse(&repo, &base_ref)
            .await
            .map_err(|e| Error::Other(format!("cannot resolve base '{base_ref}': {e}")))?;
        // Persist the caller-selected branch name (not the "HEAD"/pr_base
        // fallback), so finalization can open the PR against the branch the run
        // forked from when the spec doesn't pin `finalize.pr_base` (§12.2).
        let base_branch = base_branch.unwrap_or_default();

        let run_dir = blackboard::run_dir(&run_id)?;
        let task_md = format!("# {}\n\n{}\n", spec.name, task);
        blackboard::provision(&run_dir, &task_md)?;
        // Durable, read-only: persisted once here and rendered only into the
        // entry step's prompt, so non-entry steps never see them.
        blackboard::write_attachments(&run_dir, &attachments)?;
        gitops::provision_run_repo(&repo, &run_dir).await?;

        let branch = format!("wf/{}-{}", slugify(&spec.name), &run_id[run_id.len() - 8..]);
        let spec_json = serde_json::to_string(&spec).map_err(|e| Error::Other(e.to_string()))?;

        let now = super::now_ms();
        {
            let conn = self.db.lock();
            // `budgets_json` / `spent_json` are NOT NULL in the schema (0019) but
            // nothing reads them any more: the kernel has no budgets.
            conn.execute(
                "INSERT INTO wf_run (id, definition_id, name, spec_json, task, project_id,
                     repo_path, run_dir, branch, base_sha, base_branch, status, budgets_json,
                     spent_json, created_at, updated_at, issue_ref, roadmap_item_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'pending', '{}', '{}',
                     ?12, ?12, ?13, ?14)",
                rusqlite::params![
                    run_id,
                    definition_id,
                    spec.name,
                    spec_json,
                    task,
                    project_id,
                    repo_path,
                    run_dir.to_string_lossy(),
                    branch,
                    base_sha,
                    base_branch,
                    now,
                    issue_ref,
                    roadmap_item_id,
                ],
            )
            .map_err(|e| Error::Other(e.to_string()))?;
        }

        self.spawn_drive(run_id.clone());
        Ok(run_id)
    }

    /// Re-drive every run left `pending`/`running` at startup (spec §6.1). The
    /// kernel starts a `pending` run and fails a `running` one, since runs do not
    /// resume yet. Also reconciles run dirs a crashed `wf_delete_run` left staged
    /// (§13), so a surviving run is valid again before anyone opens it.
    pub fn resume_active_runs(&self) {
        {
            let conn = self.db.lock();
            if let Ok(root) = blackboard::runs_root() {
                recover_staged_run_dirs(&conn, &root);
            }
        }
        let ids: Vec<String> = {
            let conn = self.db.lock();
            conn.prepare("SELECT id FROM wf_run WHERE status IN ('pending','running')")
                .and_then(|mut s| {
                    s.query_map([], |r| r.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()
                })
                .unwrap_or_default()
        };
        for id in ids {
            self.spawn_drive(id);
        }
    }

    /// Cancel a run: flag its live driver, which stops the step agent and writes
    /// `canceled`. With no live driver, stop any lingering run-owned agent and
    /// mark the run canceled directly.
    pub async fn cancel(&self, run_id: &str) -> Result<()> {
        let flag = self.runs.lock().get(run_id).cloned();
        match flag {
            Some(cancel) => cancel.store(true, Ordering::SeqCst),
            None => {
                self.stop_live_step_agents(run_id).await;
                let conn = self.db.lock();
                set_status(&conn, Some(&self.engine), run_id, "canceled", None);
            }
        }
        Ok(())
    }

    /// Delete a terminal run (`wf_delete_run`, spec §13): discard every run-owned
    /// step-agent workspace (`owner_run_id`) through the app path — a DB cascade
    /// would orphan checkouts and dangle supervisor state (0019) — remove
    /// `~/.fletch/runs/<id>/` (blackboard + run repo), and delete the run's rows
    /// (`wf_step_exec` / `wf_event` / `wf_message` cascade off `wf_run`). Chats of
    /// deleted runs are gone; the UI's confirm dialog says so.
    ///
    /// Workspace discards are irreversible, so a failure can't roll back.
    /// Instead every discard is attempted, and if any fails the run's row is
    /// kept — it is what a retry rediscovers the cleanup through — and the
    /// failures are aggregated into one error that says deleting again finishes
    /// the job.
    pub async fn delete_run(&self, supervisor: &Arc<Supervisor>, run_id: &str) -> Result<()> {
        let _lifecycle_guard = self.lifecycle.lock().await;
        {
            let conn = self.db.lock();
            check_deletable(&conn, &[run_id.to_string()])?;
        }
        // A terminal run normally has no live driver, but a drive task can
        // still be winding down (status written, registry entry not yet
        // removed). Deleting its rows out from under it would race — reject.
        if self.runs.lock().contains_key(run_id) {
            return Err(Error::Other(format!(
                "run {run_id} is still winding down — try again in a moment"
            )));
        }

        // Discard every owned workspace before giving up, rather than bailing on
        // the first failure: a single wedged agent must not strand the others. A
        // successful discard removes the workspace row, so `agents_for_run` no
        // longer lists it — a re-delete only sees the ones that failed.
        let agent_ids: Vec<String> = supervisor
            .workspace
            .agents_for_run(run_id)
            .into_iter()
            .map(|a| a.id)
            .collect();
        let mut errors = Vec::new();
        let all_discarded = discard_all(&agent_ids, run_id, &mut errors, |id| {
            let sup = supervisor.clone();
            async move { sup.discard_agent(&id).await }
        })
        .await;
        if all_discarded {
            let deleted = {
                let conn = self.db.lock();
                delete_run_data(&conn, run_id)
            };
            match deleted {
                Ok(()) => journal::emit_run_deleted(self.engine.sink.as_ref(), run_id),
                Err(e) => errors.push(format!("run {run_id}: {e}")),
            }
        }
        if errors.is_empty() {
            return Ok(());
        }
        Err(Error::Other(format!(
            "run deletion incomplete: {}. The run is kept — delete again to finish.",
            errors.join("; ")
        )))
    }

    /// Preflight and stage every workflow run owned by a project. The caller
    /// holds `lifecycle` through the subsequent project/run DB transaction, so
    /// the checked set cannot change before that single commit point.
    pub(crate) fn prepare_project_runs_locked(
        &self,
        project_id: &str,
    ) -> Result<ProjectRunCleanup> {
        let all_ids = {
            let conn = self.db.lock();
            let all_ids = conn
                .prepare("SELECT id FROM wf_run WHERE project_id = ?1 ORDER BY created_at, id")?
                .query_map([project_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            check_deletable(&conn, &all_ids)?;
            all_ids
        };

        // Match delete_run's winding-down guard, but check the whole project
        // before touching any run so rejection is side-effect free.
        {
            let runs = self.runs.lock();
            if let Some(id) = all_ids.iter().find(|id| runs.contains_key(id.as_str())) {
                return Err(Error::Other(format!(
                    "run {id} is still winding down — try again in a moment"
                )));
            }
        }

        let staged_dirs = stage_run_dirs(&all_ids)?;
        Ok(ProjectRunCleanup::new(all_ids, staged_dirs))
    }

    pub(crate) fn finish_project_runs(&self, cleanup: ProjectRunCleanup) {
        cleanup.finish(&self.engine);
    }

    fn spawn_drive(&self, run_id: String) {
        spawn_drive_task(
            self.db.clone(),
            self.driver.clone(),
            self.engine.clone(),
            self.runs.clone(),
            run_id,
        );
    }

    async fn stop_live_step_agents(&self, run_id: &str) {
        let agent_ids = live_step_agents(&self.db.lock(), run_id);
        for a in agent_ids {
            let _ = self.driver.stop(&a).await;
        }
    }
}

/// The single place a driver enters the run registry: insert it and mark the
/// run active in one step, under the caller's lock, so the invariant "a run
/// with a live driver counts as active" is enforced by construction, not
/// convention.
///
/// Caller must hold the registry lock and must have checked the run has no live
/// driver (a run has at most one, §6.1). Marking active *under* the lock is what
/// upholds the ordering guarantee against [`deregister_driver`]: a clear done
/// under the lock strictly precedes any later re-insert.
fn register_driver(
    m: &mut HashMap<String, Arc<AtomicBool>>,
    run_id: &str,
    cancel: Arc<AtomicBool>,
) {
    m.insert(run_id.to_string(), cancel);
    crate::power::ActivityMonitor::global().set_run_active(run_id, true);
}

/// The single place a driver leaves the registry: remove it and clear the run's
/// activity in one step, under the caller's lock, so the clear strictly
/// precedes any subsequent insert.
fn deregister_driver(m: &mut HashMap<String, Arc<AtomicBool>>, run_id: &str) {
    m.remove(run_id);
    crate::power::ActivityMonitor::global().set_run_active(run_id, false);
}

/// Register `run_id` in the active-run map and spawn its drive task, which runs
/// the kernel. A run has at most one live driver (§6.1): a run already
/// registered is left to the driver it has.
fn spawn_drive_task(
    db: Db,
    driver: Arc<dyn AgentDriver>,
    engine: Arc<EngineCtx>,
    runs: Drivers,
    run_id: String,
) {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut m = runs.lock();
        if m.contains_key(&run_id) {
            return;
        }
        // A registered drive task is exactly a run in `pending`/`running` (a
        // terminal run has no live driver), so registry membership is the
        // faithful, no-poll signal for "workflow work is active" that the sleep
        // assertion + menu-bar status line consume. Inert until the app arms the
        // monitor at setup, so tests that spin drive tasks touch no power state.
        register_driver(&mut m, &run_id, cancel.clone());
    }

    let ctx = RunCtx {
        db: db.clone(),
        driver,
        engine: Some(engine.clone()),
        cancel,
    };
    let id = run_id.clone();
    let join = crate::host::spawn(async move { run_kernel(&ctx, &id).await });
    crate::host::spawn(async move {
        if join.await.is_err() {
            let conn = db.lock();
            fail_run(&conn, Some(&engine), &run_id, "internal scheduler error");
        }
        deregister_driver(&mut runs.lock(), &run_id);
    });
}

/// The run branch's readable part: the spec name lowercased, every run of
/// non-alphanumerics collapsed to one `-`, capped at 40 chars.
fn slugify(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "run".to_string()
    } else {
        s.chars().take(40).collect()
    }
}

/// Launch a run from a launch-time `spec` snapshot (spec §13).
#[allow(clippy::too_many_arguments)]
pub async fn wf_launch_impl(
    spec: Spec,
    task: String,
    project_id: String,
    repo_path: String,
    definition_id: Option<String>,
    base_branch: Option<String>,
    attachments: Vec<String>,
    // Set when the launch originates from a Home-inbox issue (Pipeline mode);
    // carried onto the run row so its finalized PR closes the issue. `None` for
    // a normal launch.
    issue_ref: Option<String>,
    service: &Arc<WorkflowService>,
    supervisor: &Arc<Supervisor>,
) -> std::result::Result<String, String> {
    // A run targets one repo, so its project is authoritatively that repo's.
    // Resolve it from `repo_path` here rather than trusting the caller's
    // snapshot: a path-normalization mismatch or a stale workspace snapshot
    // could otherwise pass an empty id and orphan the run from project-scoped
    // queries. Fall back to the caller's value only if resolution fails.
    let project_id = supervisor
        .workspace
        .project_id_for_repo(&repo_path)
        .unwrap_or(project_id);
    service
        .launch(
            spec,
            task,
            project_id,
            repo_path,
            definition_id,
            base_branch,
            attachments,
            issue_ref,
            // A hand-launched run answers to nobody's queue.
            None,
        )
        .await
        .map_err(|e| e.to_string())
}

pub async fn wf_cancel_impl(
    run_id: String,
    service: &Arc<WorkflowService>,
) -> std::result::Result<(), String> {
    service.cancel(&run_id).await.map_err(|e| e.to_string())
}

/// Delete a terminal run and everything it owns (spec §13): run-owned step
/// agents (and their chats), the run directory, and the run's rows.
pub async fn wf_delete_run_impl(
    run_id: String,
    service: &Arc<WorkflowService>,
    supervisor: &Arc<Supervisor>,
) -> std::result::Result<(), String> {
    service
        .delete_run(supervisor, &run_id)
        .await
        .map_err(|e| e.to_string())
}

/// Why a launch is refused for a project whose agents run in a box. A fact,
/// not advice: the user picked that placement and nothing here suggests
/// changing it.
const BOX_REFUSAL: &str =
    "Workflow runs cannot run in a box yet, and this project's agents run in one.";

/// The providers the run will spawn: the base of every alias a step names,
/// resolved through `spec.agents`. An alias declared but named by no step is
/// left out — validation allows one and a YAML import keeps every declared
/// agent — so it cannot refuse a launch over an agent the run would never
/// start. Validation has already checked that each step's alias exists.
fn step_providers(spec: &Spec) -> BTreeSet<String> {
    spec.workflow
        .iter()
        .filter_map(|step| spec.agents.get(&step.agent))
        .map(|agent| agent.base.clone())
        .collect()
}

/// Whether a fresh agent of any of `providers` would land in a box for the
/// project at `repo` whose stored placement is `placement_setting` — the
/// automatic choice `spawn_agent` makes when neither an adopted tree nor a
/// session's own workspace type decides (`sandbox::engine_for_spawn`), with
/// the runtime probe supplied as `ready` so the answer can be fixed in a test.
fn lands_in_a_box<'a>(
    placement_setting: Option<&str>,
    repo: &Path,
    providers: impl IntoIterator<Item = &'a str>,
    ready: impl Fn(&str) -> bool,
) -> bool {
    let placement = sandbox::placement::resolve(placement_setting, repo);
    providers.into_iter().any(|provider| {
        sandbox::boxes::engine_for_new_agent_with(provider, placement, || ready(provider))
            == EngineKind::Box
    })
}

/// One project setting, or `None` when unset (or unreadable: a setting that
/// cannot be read decides nothing).
fn project_setting(conn: &Connection, project_id: &str, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
        [project_id, key],
        |r| r.get(0),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::driver::MockDriver;

    /// The refusal mirrors the spawn path's automatic choice: a project pinned
    /// to boxes is boxed wherever one can boot, a project pinned to this Mac
    /// never is, and an undecided project follows whether a box is ready.
    #[test]
    fn a_launch_is_boxed_exactly_when_a_fresh_agent_would_be() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let claude = ["claude"];

        assert!(lands_in_a_box(Some("box"), repo, claude, |_| true));
        if cfg!(target_os = "macos") {
            assert!(!lands_in_a_box(Some("host"), repo, claude, |_| true));
            assert!(!lands_in_a_box(None, repo, claude, |_| false));
            assert!(lands_in_a_box(None, repo, claude, |_| true));
            // One boxed provider among several is enough: every step must run.
            assert!(lands_in_a_box(
                None,
                repo,
                ["claude", "codex"],
                |provider| provider == "codex"
            ));
            // A native-looking project runs on this Mac whatever is ready.
            std::fs::create_dir(repo.join("App.xcodeproj")).unwrap();
            assert!(!lands_in_a_box(None, repo, claude, |_| true));
        } else {
            // Off macOS there is no seatbelt: every fresh agent is a box.
            assert!(lands_in_a_box(Some("host"), repo, claude, |_| false));
        }
    }

    /// An alias no step names is declared, kept by import, and never spawned:
    /// its provider must not decide the launch.
    #[test]
    fn only_the_providers_steps_name_decide_the_placement() {
        let spec: Spec = serde_json::from_str(
            r#"{"version":1,"name":"s",
                "agents":{"coder":{"base":"claude"},"spare":{"base":"codex"}},
                "workflow":[{"step":{"id":"plan","agent":"coder","goal":"g"}},
                            {"step":{"id":"code","agent":"coder","goal":"g"}}]}"#,
        )
        .unwrap();
        let providers = step_providers(&spec);
        assert_eq!(providers, BTreeSet::from(["claude".to_string()]));

        if cfg!(target_os = "macos") {
            let dir = tempfile::tempdir().unwrap();
            // Only the spare alias's provider has a box ready: no refusal.
            assert!(!lands_in_a_box(
                None,
                dir.path(),
                providers.iter().map(String::as_str),
                |provider| provider == "codex"
            ));
        }
    }

    fn insert_run(db: &Db, id: &str, status: &str, spec_json: &str) {
        db.lock()
            .execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                        base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'demo',?2,'t','p','/r','/d','wf/x','sha',?3,'{}','{}',0,0)",
                rusqlite::params![id, spec_json, status],
            )
            .unwrap();
    }

    fn run_status(db: &Db, id: &str) -> (String, Option<String>) {
        db.lock()
            .query_row(
                "SELECT status, error FROM wf_run WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn cancel_with_no_live_driver_stops_lingering_agents_and_marks_canceled() {
        let (engine, _sink, _dir) = crate::host::ctx::test_ctx();
        let db = engine.db.clone();
        insert_run(&db, "r1", "running", "{}");
        db.lock()
            .execute(
                "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,
                        agent_id)
                 VALUES ('e1','r1','plan',1,0,'running','commit','a1')",
                [],
            )
            .unwrap();
        let driver = MockDriver::new();
        let service = WorkflowService::new(db.clone(), driver.clone(), engine);

        service.cancel("r1").await.unwrap();

        assert_eq!(run_status(&db, "r1").0, "canceled");
        assert!(
            driver.was_stopped("a1"),
            "the lingering step agent is stopped"
        );
    }

    #[tokio::test]
    async fn cancel_with_a_live_driver_leaves_the_status_to_it() {
        let (engine, _sink, _dir) = crate::host::ctx::test_ctx();
        let db = engine.db.clone();
        insert_run(&db, "r1", "running", "{}");
        let service = WorkflowService::new(db.clone(), MockDriver::new(), engine);
        let flag = Arc::new(AtomicBool::new(false));
        service.runs.lock().insert("r1".into(), flag.clone());

        service.cancel("r1").await.unwrap();

        assert!(flag.load(Ordering::SeqCst), "the driver is told to cancel");
        assert_eq!(
            run_status(&db, "r1").0,
            "running",
            "the driver writes `canceled` once its step winds down"
        );
    }

    #[tokio::test]
    async fn a_drive_task_fails_a_run_it_cannot_read_and_leaves_the_registry() {
        let (engine, _sink, _dir) = crate::host::ctx::test_ctx();
        let db = engine.db.clone();
        insert_run(&db, "r1", "pending", "not a spec");
        let runs: Drivers = Arc::new(Mutex::new(HashMap::new()));

        spawn_drive_task(
            db.clone(),
            MockDriver::new(),
            engine,
            runs.clone(),
            "r1".into(),
        );
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while runs.lock().contains_key("r1") {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the drive task never deregistered"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let (status, error) = run_status(&db, "r1");
        assert_eq!(status, "failed");
        assert!(
            error.unwrap().contains("unsupported workflow spec"),
            "the cause is on the row"
        );
    }

    #[test]
    fn slugify_keeps_the_branch_name_readable_and_bounded() {
        assert_eq!(slugify("Plan & Build: v2"), "plan-build-v2");
        assert_eq!(slugify("***"), "run");
        assert_eq!(slugify(&"x".repeat(60)).len(), 40);
    }
}

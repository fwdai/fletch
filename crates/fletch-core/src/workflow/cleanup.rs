//! Deleting runs: the terminal-status guard, the run-dir staging that makes a
//! row delete the single commit point, and the startup sweep that reconciles a
//! staging rename an app exit interrupted (§13).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::host::EngineCtx;

use super::{blackboard, journal};

/// Filesystem half of an atomic project deletion. Run directories are renamed
/// aside before the database transaction; they can therefore be restored if
/// the transaction fails, or swept after it commits. Startup recovery handles
/// the same two states if the app exits between either boundary.
pub(crate) struct ProjectRunCleanup {
    run_ids: Vec<String>,
    staged_dirs: Vec<StagedRunDir>,
    finalized: bool,
}

impl ProjectRunCleanup {
    pub(crate) fn new(run_ids: Vec<String>, staged_dirs: Vec<StagedRunDir>) -> Self {
        Self {
            run_ids,
            staged_dirs,
            finalized: false,
        }
    }

    pub(crate) fn run_ids(&self) -> &[String] {
        &self.run_ids
    }

    pub(crate) fn restore(mut self) {
        restore_staged_run_dirs(&self.staged_dirs);
        self.finalized = true;
    }

    pub(crate) fn finish(mut self, engine: &Arc<EngineCtx>) {
        // The DB commit has happened; never let Drop restore directories for
        // rows that no longer exist. Failed removals are startup-recoverable.
        self.finalized = true;
        for dir in &self.staged_dirs {
            dir.remove_staged();
        }
        for run_id in &self.run_ids {
            journal::emit_run_deleted(engine.sink.as_ref(), run_id);
        }
    }
}

impl Drop for ProjectRunCleanup {
    fn drop(&mut self) {
        if !self.finalized {
            restore_staged_run_dirs(&self.staged_dirs);
        }
    }
}

/// The `wf_delete_run` guard (spec §13): every run must be terminal (`done` /
/// `failed` / `canceled`). Checked before anything is deleted.
pub(crate) fn check_deletable(conn: &Connection, run_ids: &[String]) -> Result<()> {
    for id in run_ids {
        let status: String = conn
            .query_row("SELECT status FROM wf_run WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .map_err(|e| Error::Other(format!("run {id} not found: {e}")))?;
        if !matches!(status.as_str(), "done" | "failed" | "canceled") {
            return Err(Error::Other(format!(
                "cannot delete a {status} run — cancel it first"
            )));
        }
    }
    Ok(())
}

/// Delete one run's on-disk directory and its DB rows. The row delete cascades
/// `wf_step_exec` / `wf_event` / `wf_message` (0019 + `PRAGMA foreign_keys`).
pub(crate) fn delete_run_data(conn: &Connection, run_id: &str) -> Result<()> {
    delete_run_data_at(conn, run_id, &blackboard::run_dir(run_id)?)
}

#[derive(Debug)]
pub(crate) struct StagedRunDir {
    live: PathBuf,
    staged: PathBuf,
    exists: bool,
}

impl StagedRunDir {
    fn stage(run_id: &str, live: PathBuf) -> Result<Self> {
        let staged = live.with_file_name(format!(
            "{}.deleting",
            live.file_name().and_then(|n| n.to_str()).unwrap_or(run_id)
        ));
        let exists = match std::fs::rename(&live, &staged) {
            Ok(()) => true,
            // No live dir: never provisioned, already cleaned, or a prior
            // crash left the staged form for startup/retry recovery.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => staged.exists(),
            Err(e) => {
                return Err(Error::Other(format!(
                    "cannot stage run dir for {run_id}: {e}"
                )))
            }
        };
        Ok(Self {
            live,
            staged,
            exists,
        })
    }

    fn restore(&self) {
        if self.exists {
            if let Err(error) = std::fs::rename(&self.staged, &self.live) {
                tracing::warn!(
                    error = %error,
                    staged = %self.staged.display(),
                    "restore staged run directory failed; startup recovery will retry"
                );
            }
        }
    }

    pub(crate) fn remove_staged(&self) {
        if self.exists {
            if let Err(error) = std::fs::remove_dir_all(&self.staged) {
                tracing::warn!(
                    error = %error,
                    staged = %self.staged.display(),
                    "remove committed staged run directory failed; startup recovery will retry"
                );
            }
        }
    }
}

pub(crate) fn restore_staged_run_dirs(staged: &[StagedRunDir]) {
    for dir in staged.iter().rev() {
        dir.restore();
    }
}

pub(crate) fn stage_run_dirs(run_ids: &[String]) -> Result<Vec<StagedRunDir>> {
    stage_run_dirs_at(run_ids, blackboard::run_dir)
}

pub(crate) fn stage_run_dirs_at<F>(run_ids: &[String], resolve: F) -> Result<Vec<StagedRunDir>>
where
    F: Fn(&str) -> Result<PathBuf>,
{
    let mut staged = Vec::with_capacity(run_ids.len());
    for run_id in run_ids {
        let live = match resolve(run_id) {
            Ok(path) => path,
            Err(error) => {
                restore_staged_run_dirs(&staged);
                return Err(error);
            }
        };
        match StagedRunDir::stage(run_id, live) {
            Ok(dir) => staged.push(dir),
            Err(error) => {
                restore_staged_run_dirs(&staged);
                return Err(error);
            }
        }
    }
    Ok(staged)
}

/// Discard every id, pressing on past a failure instead of bailing on the first:
/// one wedged agent must not strand the run's other workspaces. Returns whether
/// *all* discards succeeded — the caller keeps the run row (for a re-delete)
/// whenever any failed. A per-failure message lands in `errors`. Split out from
/// `WorkflowService::delete_run` so the policy is unit-testable without a real
/// `Supervisor` (whose discards can't be made to fail on demand).
pub(crate) async fn discard_all<F, Fut>(
    ids: &[String],
    run_id: &str,
    errors: &mut Vec<String>,
    discard: F,
) -> bool
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut all = true;
    for id in ids {
        if let Err(e) = discard(id.clone()).await {
            errors.push(format!("run {run_id}: discard agent {id}: {e}"));
            all = false;
        }
    }
    all
}

/// Inner form taking the resolved run dir, so tests can exercise the staging
/// without the process-global `FLETCH_RUNS_ROOT` override.
///
/// The row delete is the commit point: the dir is first staged aside with an
/// atomic rename and renamed back if the row delete fails, so a run that
/// survives the command is always openable — never a visible row whose
/// blackboard and run repo are already gone. An app exit inside that window
/// can't rename back — [`recover_staged_run_dirs`] reconciles it at the next
/// startup. Only once the rows are gone is the staged dir actually removed
/// (best-effort: at that point the run no longer exists to retry against, and
/// a leftover `<id>.deleting` dir is invisible junk, which the next attempt or
/// startup would also sweep). A missing dir is fine — a retried partial delete
/// or a cleaned disk.
pub(crate) fn delete_run_data_at(conn: &Connection, run_id: &str, dir: &Path) -> Result<()> {
    let staged = StagedRunDir::stage(run_id, dir.to_path_buf())?;
    if let Err(e) = conn.execute("DELETE FROM wf_run WHERE id = ?1", [run_id]) {
        staged.restore();
        return Err(Error::Other(e.to_string()));
    }
    staged.remove_staged();
    Ok(())
}

/// Startup reconciliation for `<id>.deleting` run dirs (§13): an app exit
/// between `delete_run_data_at`'s staging rename and its row delete strands
/// the staged dir with the run row still live — the exact broken state the
/// staging exists to prevent, and one only a manual retry would otherwise
/// clear. A staged dir whose row survives is renamed back (the run is fully
/// openable again before anyone touches it); one whose row is gone is the
/// tail of a completed delete and is swept. Best-effort throughout.
pub(crate) fn recover_staged_run_dirs(conn: &Connection, runs_root: &Path) {
    let Ok(entries) = std::fs::read_dir(runs_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(run_id) = name.to_str().and_then(|n| n.strip_suffix(".deleting")) else {
            continue;
        };
        let row_exists =
            match conn.query_row("SELECT COUNT(*) FROM wf_run WHERE id = ?1", [run_id], |r| {
                r.get::<_, i64>(0)
            }) {
                Ok(n) => n > 0,
                // Can't tell — never destroy data on a failed lookup.
                Err(_) => continue,
            };
        if row_exists {
            // Restore. Renaming onto an existing non-empty dir fails, which is
            // the safe outcome if a live dir somehow reappeared.
            let _ = std::fs::rename(entry.path(), runs_root.join(run_id));
        } else {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_guard_rejects_a_live_run() {
        // `wf_delete_run` (§13): the guard rejects the whole delete while any run
        // in it is non-terminal.
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::database::init(tmp.path()).unwrap();
        let insert = |id: &str, status: &str| {
            db.lock()
                .execute(
                    "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,
                            branch,base_sha,status,budgets_json,spent_json,created_at,updated_at)
                         VALUES (?1,'n','{}','t','p','/r','/d','wf/x','sha',?2,'{}','{}',0,0)",
                    rusqlite::params![id, status],
                )
                .unwrap();
        };
        insert("r-done", "done");
        insert("r-live", "running");
        let ids = vec!["r-done".to_string(), "r-live".to_string()];

        let conn = db.lock();
        let err = check_deletable(&conn, &ids).unwrap_err().to_string();
        assert!(err.contains("cannot delete a running run"), "{err}");

        conn.execute("UPDATE wf_run SET status='failed' WHERE id='r-live'", [])
            .unwrap();
        assert!(check_deletable(&conn, &ids).is_ok());
    }

    #[tokio::test]
    async fn discard_all_presses_past_a_failure_and_reports_it() {
        // A discard failing mid-loop must not strand the run's other workspaces. Every id is attempted; the surviving one is
        // reported; the "all succeeded" result is false so the caller keeps the
        // run row for a re-delete.
        let ids = vec!["a1".to_string(), "a2".to_string(), "a3".to_string()];
        let mut errors = Vec::new();
        let attempted = std::sync::Mutex::new(Vec::new());
        let all = discard_all(&ids, "r", &mut errors, |id| {
            attempted.lock().unwrap().push(id.clone());
            async move {
                if id == "a2" {
                    Err(Error::Other("wedged checkout".into()))
                } else {
                    Ok(())
                }
            }
        })
        .await;

        assert!(!all, "a failed discard means not-all-discarded");
        assert_eq!(
            *attempted.lock().unwrap(),
            vec!["a1", "a2", "a3"],
            "every agent is attempted even after a1..a2 — the loop does not bail"
        );
        assert_eq!(errors.len(), 1, "only the failed discard is reported");
        assert!(
            errors[0].contains("a2") && errors[0].contains("wedged checkout"),
            "the failure names the agent and cause: {}",
            errors[0]
        );
    }

    #[tokio::test]
    async fn discard_all_is_all_when_every_discard_succeeds() {
        let ids = vec!["a1".to_string(), "a2".to_string()];
        let mut errors = Vec::new();
        let all = discard_all(&ids, "r", &mut errors, |_| async { Ok(()) }).await;
        assert!(all);
        assert!(errors.is_empty());
    }

    #[test]
    fn staging_multiple_run_dirs_restores_earlier_dirs_on_later_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let runs = tmp.path().join("runs");
        let first = runs.join("r-first");
        let second = runs.join("r-second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join("marker"), "first").unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("marker"), "second").unwrap();

        // Make the second staging rename fail after the first has succeeded.
        let second_staged = runs.join("r-second.deleting");
        std::fs::create_dir_all(&second_staged).unwrap();
        std::fs::write(second_staged.join("collision"), "occupied").unwrap();

        let ids = vec!["r-first".to_string(), "r-second".to_string()];
        let error = stage_run_dirs_at(&ids, |id| Ok(runs.join(id))).unwrap_err();
        assert!(error.to_string().contains("cannot stage run dir"));
        assert!(first.join("marker").exists(), "first dir restored intact");
        assert!(second.join("marker").exists(), "failed dir remains intact");
        assert!(
            !runs.join("r-first.deleting").exists(),
            "earlier staged dir is not stranded"
        );
    }

    #[test]
    fn delete_run_data_cascades_the_runs_rows() {
        // Deleting the `wf_run` row must take its journal, execs, and messages
        // with it (0019 ON DELETE CASCADE + the connection's foreign_keys
        // pragma), and tolerate an already-missing run directory.
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::database::init(tmp.path()).unwrap();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES ('r-del','n','{}','t','p','/r','/d','wf/x','sha','done','{}','{}',0,0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
                 VALUES ('e1','r-del','s',1,0,'done','verdict')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_event (run_id,seq,ts,type,payload_json)
                 VALUES ('r-del',1,0,'run_launched','{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_message (id,run_id,kind,body_json,status,created_at)
                 VALUES ('m1','r-del','report','{}','delivered',0)",
            [],
        )
        .unwrap();

        // A real run dir with content: it must be gone (not just staged) after.
        let dir = tmp.path().join("runs").join("r-del");
        std::fs::create_dir_all(dir.join("blackboard")).unwrap();
        std::fs::write(dir.join("blackboard").join("task.md"), "t").unwrap();

        delete_run_data_at(&conn, "r-del", &dir).expect("delete succeeds");

        let count = |table: &str| -> i64 {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE run_id = 'r-del'"),
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM wf_run WHERE id='r-del'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(runs, 0);
        assert_eq!(count("wf_step_exec"), 0, "execs cascade");
        assert_eq!(count("wf_event"), 0, "journal cascades");
        assert_eq!(count("wf_message"), 0, "messages cascade");
        assert!(!dir.exists(), "run dir removed");
        assert!(
            !dir.with_file_name("r-del.deleting").exists(),
            "no staged dir left behind"
        );
    }

    #[test]
    fn delete_run_data_restores_the_dir_when_the_row_delete_fails() {
        // The row delete is the commit point: if it fails, the staged dir is
        // renamed back so the surviving run row never points at missing state.
        // The failure here is real — the schema's `parent_run_id` FK (no cascade)
        // rejects deleting a row another one still references.
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::database::init(tmp.path()).unwrap();
        let conn = db.lock();
        let insert = |id: &str, parent: Option<&str>| {
            conn.execute(
                "INSERT INTO wf_run (id,parent_run_id,name,spec_json,task,project_id,repo_path,
                        run_dir,branch,base_sha,status,budgets_json,spent_json,created_at,updated_at)
                     VALUES (?1,?2,'n','{}','t','p','/r','/d','wf/x','sha','done','{}','{}',0,0)",
                rusqlite::params![id, parent],
            )
            .unwrap();
        };
        insert("r-parent", None);
        insert("r-child", Some("r-parent"));
        let dir = tmp.path().join("runs").join("r-parent");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("marker"), "m").unwrap();

        let err = delete_run_data_at(&conn, "r-parent", &dir).unwrap_err();
        assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
        assert!(dir.join("marker").exists(), "dir renamed back intact");
        assert!(
            !dir.with_file_name("r-parent.deleting").exists(),
            "staged dir gone after restore"
        );
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM wf_run WHERE id='r-parent'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(runs, 1, "row untouched");
    }

    #[test]
    fn delete_run_data_sweeps_a_crashed_attempts_staged_dir() {
        // A crash between the staging rename and the row delete leaves rows
        // plus a `<id>.deleting` dir and no live dir; the retry must finish
        // the job rather than orphan the staged dir.
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::database::init(tmp.path()).unwrap();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES ('r-crash','n','{}','t','p','/r','/d','wf/x','sha','done','{}','{}',0,0)",
            [],
        )
        .unwrap();
        let dir = tmp.path().join("runs").join("r-crash");
        let staged = dir.with_file_name("r-crash.deleting");
        std::fs::create_dir_all(&staged).unwrap();

        delete_run_data_at(&conn, "r-crash", &dir).expect("retry completes");
        assert!(!staged.exists(), "leftover staged dir swept");
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM wf_run WHERE id='r-crash'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(runs, 0);
    }

    #[test]
    fn startup_recovery_reconciles_staged_run_dirs() {
        // An app exit between the staging rename and the row delete strands a
        // `<id>.deleting` dir. At the next startup: a staged dir whose run row
        // survives is renamed back (the run is openable again without user
        // action); one whose row is gone is swept; live dirs are untouched.
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::database::init(tmp.path()).unwrap();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES ('r-alive','n','{}','t','p','/r','/d','wf/x','sha','done','{}','{}',0,0)",
            [],
        )
        .unwrap();
        let root = tmp.path().join("runs");
        std::fs::create_dir_all(root.join("r-alive.deleting")).unwrap();
        std::fs::write(root.join("r-alive.deleting").join("marker"), "m").unwrap();
        std::fs::create_dir_all(root.join("r-gone.deleting")).unwrap();
        std::fs::create_dir_all(root.join("r-normal")).unwrap();

        recover_staged_run_dirs(&conn, &root);

        assert!(
            root.join("r-alive").join("marker").exists(),
            "surviving run's dir restored intact"
        );
        assert!(!root.join("r-alive.deleting").exists(), "staged name gone");
        assert!(
            !root.join("r-gone.deleting").exists(),
            "completed delete's tail swept"
        );
        assert!(root.join("r-normal").exists(), "live dirs untouched");
    }
}

use super::*;

/// The run tree rooted at `run_id`, post-order — every child precedes its
/// parent, so a delete walked in this order never dangles the `parent_run_id`
/// FK (which deliberately has no cascade).
pub(crate) fn run_tree_post_order(conn: &Connection, run_id: &str, out: &mut Vec<String>) {
    for child in child_run_ids(conn, run_id) {
        run_tree_post_order(conn, &child, out);
    }
    out.push(run_id.to_string());
}

/// The `wf_delete_run` guard (spec §13): every run in the tree must be terminal
/// (`done` / `failed` / `canceled`). Checked before anything is deleted.
pub(crate) fn check_deletable(conn: &Connection, run_ids: &[String]) -> Result<()> {
    for id in run_ids {
        let (status, _) = run_status(conn, id)?;
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

/// Discard every id, pressing on past a failure instead of bailing on the first
/// (the review blocker): one wedged agent must not strand the run's other
/// workspaces. Returns whether *all* discards succeeded — the caller keeps the
/// run row (for a re-delete) whenever any failed. A per-failure message lands in
/// `errors`. Split out from [`WorkflowService::delete_tree`] so the policy is
/// unit-testable without a real `Supervisor` (whose discards can't be made to
/// fail on demand).
pub(crate) async fn discard_all<F, Fut>(
    ids: &[String],
    run_id: &str,
    errors: &mut Vec<String>,
    discard: F,
) -> bool
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
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

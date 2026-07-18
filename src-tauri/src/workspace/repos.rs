//! `impl WorkspaceManager` — repo & project management (add / remove / attach /
//! detach / relabel / rename / delete / relocate pinned repos).

use super::*;

impl WorkspaceManager {
    /// Append a repo to the sidebar's pinned list. Idempotent — adding
    /// a path that's already pinned is a no-op (returns Ok).
    pub fn add_workspace_repo(&self, repo_path: PathBuf) -> Result<Workspace> {
        if !repo_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                repo_path.display()
            )));
        }

        let conn = self.db.lock();
        let path_str = repo_path.to_string_lossy().to_string();

        // Check if repo already exists.
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM repos WHERE path = ?1",
                [&path_str],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !exists {
            // Look up or create a project named after the repo dir basename.
            let project_name = repo_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let project_id = Self::find_or_create_project(&conn, &project_name)?;

            let repo_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO repos (id, project_id, path, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![repo_id, project_id, path_str, now_millis()],
            )?;
        }

        drop(conn);
        Ok(self.current().expect("workspace initialized"))
    }

    /// Remove a repo from the sidebar's pinned list. Does NOT touch
    /// agents — agents whose primary points at the removed repo keep
    /// working and continue to show in the sidebar under that repo
    /// (the sidebar takes the union of pinned + agent-primary repos).
    pub fn remove_workspace_repo(&self, repo_path: &Path) -> Result<Workspace> {
        let conn = self.db.lock();
        let path_str = repo_path.to_string_lossy().to_string();
        conn.execute("DELETE FROM repos WHERE path = ?1", [&path_str])?;
        drop(conn);
        Ok(self.current().expect("workspace initialized"))
    }

    /// Attach a repo to an existing project, making it a multi-repo project.
    /// An unpinned path is inserted under the target project; a path already
    /// attached to the target is a no-op. A path pinned as its own project is
    /// *moved* here only when that project is empty (no agents, no workflow
    /// runs) — its emptied project row is then removed. A path belonging to a
    /// project with history is rejected: moving it would strand that project's
    /// agents under a project the repo no longer belongs to.
    ///
    /// This is the DB phase only, one atomic transaction, and it runs *before*
    /// any filesystem mutation: the command layer initializes a non-git folder
    /// only after this commits, and calls [`Self::undo_attach`] with the
    /// returned outcome if that on-disk step fails. Ordering is the point — a
    /// doomed attach (stale modal, deleted project) must never touch the
    /// picked folder.
    pub fn attach_repo_to_project(
        &self,
        project_id: &str,
        repo_path: &Path,
    ) -> Result<AttachOutcome> {
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        let path_str = repo_path.to_string_lossy().to_string();

        let target_exists: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE id = ?1",
                [project_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)?;
        if !target_exists {
            return Err(Error::Other(format!("project not found: {project_id}")));
        }

        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT id, project_id FROM repos WHERE path = ?1",
                [&path_str],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        let outcome = match existing {
            None => {
                let repo_id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO repos (id, project_id, path, created_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![repo_id, project_id, path_str, now_millis()],
                )?;
                AttachOutcome::Inserted { repo_id }
            }
            Some((_, ref pid)) if pid == project_id => AttachOutcome::AlreadyAttached,
            Some((repo_id, source_pid)) => {
                let in_use: i64 = tx.query_row(
                    "SELECT (SELECT COUNT(*) FROM workspaces WHERE project_id = ?1)
                          + (SELECT COUNT(*) FROM wf_run WHERE project_id = ?1)",
                    [&source_pid],
                    |row| row.get(0),
                )?;
                if in_use > 0 {
                    let source_name: String = tx
                        .query_row(
                            "SELECT name FROM projects WHERE id = ?1",
                            [&source_pid],
                            |row| row.get(0),
                        )
                        .unwrap_or_else(|_| "another project".into());
                    return Err(Error::Other(format!(
                        "{path_str} belongs to project \"{source_name}\", which has agents or \
                         workflow runs. Delete those first, or attach a different repo."
                    )));
                }
                tx.execute(
                    "UPDATE repos SET project_id = ?1 WHERE id = ?2",
                    rusqlite::params![project_id, repo_id],
                )?;
                // The source project is empty and now repo-less — drop the row
                // so it doesn't linger as an invisible orphan. Keep its fields
                // so undo can restore it verbatim.
                let repos_left: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM repos WHERE project_id = ?1",
                    [&source_pid],
                    |row| row.get(0),
                )?;
                let dropped_source = if repos_left == 0 {
                    let (name, created_at): (String, i64) = tx.query_row(
                        "SELECT name, created_at FROM projects WHERE id = ?1",
                        [&source_pid],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    tx.execute("DELETE FROM projects WHERE id = ?1", [&source_pid])?;
                    Some(DroppedProject { name, created_at })
                } else {
                    None
                };
                AttachOutcome::Moved {
                    repo_id,
                    source_project_id: source_pid,
                    dropped_source,
                }
            }
        };

        tx.commit()?;
        Ok(outcome)
    }

    /// Undo the DB phase of an attach after the follow-up filesystem step
    /// failed. Restores exactly what [`Self::attach_repo_to_project`] changed
    /// moments earlier: a fresh row is deleted, a moved row is pointed back at
    /// its source project, and a dropped source project row is re-created.
    pub fn undo_attach(&self, outcome: &AttachOutcome) -> Result<()> {
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        match outcome {
            AttachOutcome::AlreadyAttached => {}
            AttachOutcome::Inserted { repo_id } => {
                tx.execute("DELETE FROM repos WHERE id = ?1", [repo_id])?;
            }
            AttachOutcome::Moved {
                repo_id,
                source_project_id,
                dropped_source,
            } => {
                if let Some(p) = dropped_source {
                    tx.execute(
                        "INSERT OR IGNORE INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)",
                        rusqlite::params![source_project_id, p.name, p.created_at],
                    )?;
                }
                tx.execute(
                    "UPDATE repos SET project_id = ?1 WHERE id = ?2",
                    rusqlite::params![source_project_id, repo_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Detach a repo from a project. Guarded twice: the last repo can't be
    /// detached (delete the project instead), and a repo referenced by any
    /// agent checkout — live or archived — can't be detached, because the
    /// `worktrees.repo_id` FK cascade would silently destroy that agent's
    /// tracked branches, PRs, and archive snapshots.
    pub fn detach_repo_from_project(
        &self,
        project_id: &str,
        repo_path: &Path,
    ) -> Result<Workspace> {
        let conn = self.db.lock();
        let path_str = repo_path.to_string_lossy().to_string();

        let (repo_id, actual_pid): (String, String) = conn
            .query_row(
                "SELECT id, project_id FROM repos WHERE path = ?1",
                [&path_str],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| Error::Other(format!("repo not found: {path_str}")))?;
        if actual_pid != project_id {
            return Err(Error::Other(format!(
                "{path_str} does not belong to project {project_id}"
            )));
        }

        let repo_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM repos WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )?;
        if repo_count <= 1 {
            return Err(Error::Other(
                "a project needs at least one repository — delete the project instead".into(),
            ));
        }

        let checkouts: i64 = conn.query_row(
            "SELECT COUNT(*) FROM worktrees WHERE repo_id = ?1",
            [&repo_id],
            |row| row.get(0),
        )?;
        if checkouts > 0 {
            return Err(Error::Other(format!(
                "{path_str} is used by existing agents (including archived ones). \
                 Delete those agents before detaching it."
            )));
        }

        conn.execute("DELETE FROM repos WHERE id = ?1", [&repo_id])?;
        drop(conn);
        Ok(self.current().expect("workspace initialized"))
    }

    /// Set a repo's display label ("Frontend", "Gateway"), independent of its
    /// folder name. A blank label clears back to the basename fallback.
    pub fn set_repo_label(&self, repo_path: &Path, label: &str) -> Result<Workspace> {
        let trimmed = label.trim();
        let value: Option<&str> = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };

        let conn = self.db.lock();
        let path_str = repo_path.to_string_lossy().to_string();
        let changed = conn.execute(
            "UPDATE repos SET label = ?1 WHERE path = ?2",
            rusqlite::params![value, path_str],
        )?;
        if changed == 0 {
            return Err(Error::Other(format!("repo not found: {path_str}")));
        }
        drop(conn);
        Ok(self.current().expect("workspace initialized"))
    }

    /// Set a project's display name, decoupled from its folder basename. The
    /// name is trimmed; an empty name is rejected so a project always has a
    /// label. Does not touch the repo path or anything on disk.
    pub fn rename_project(&self, project_id: &str, name: &str) -> Result<Workspace> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(Error::Other("project name cannot be empty".into()));
        }

        let conn = self.db.lock();
        let changed = conn.execute(
            "UPDATE projects SET name = ?1 WHERE id = ?2",
            rusqlite::params![trimmed, project_id],
        )?;
        if changed == 0 {
            return Err(Error::Other(format!("project not found: {project_id}")));
        }
        drop(conn);
        Ok(self.current().expect("workspace initialized"))
    }

    /// Atomically delete a project and its workflow runs after filesystem and
    /// runtime cleanup has been staged by the supervisor. Workflow runs do not
    /// have a project FK, so they share this transaction explicitly; every
    /// other project-owned row is removed by foreign-key cascade.
    pub fn delete_project(&self, project_id: &str, expected_run_ids: &[String]) -> Result<()> {
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        let actual_run_ids = {
            let mut stmt =
                tx.prepare("SELECT id FROM wf_run WHERE project_id = ?1 ORDER BY created_at, id")?;
            let ids = stmt
                .query_map([project_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };
        if actual_run_ids != expected_run_ids {
            return Err(Error::Other(format!(
                "project workflow set changed during deletion: expected {}, found {}",
                expected_run_ids.len(),
                actual_run_ids.len()
            )));
        }
        tx.execute("DELETE FROM wf_run WHERE project_id = ?1", [project_id])?;
        let changed = tx.execute("DELETE FROM projects WHERE id = ?1", [project_id])?;
        if changed == 0 {
            return Err(Error::Other(format!("project not found: {project_id}")));
        }
        tx.commit()?;
        Ok(())
    }

    /// Repoint a pinned repo at a new location on disk. The user has already
    /// moved the folder; this only updates the stored reference so future
    /// agents spawn from the right place. Validates the new path is a git repo
    /// and isn't already pinned. Existing agents' worktrees are NOT relinked —
    /// they were forked from the old location and keep pointing there.
    pub fn relocate_repo(&self, old_path: &Path, new_path: &Path) -> Result<Workspace> {
        if !new_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                new_path.display()
            )));
        }

        let conn = self.db.lock();
        let old_str = old_path.to_string_lossy().to_string();
        let new_str = new_path.to_string_lossy().to_string();

        if old_str == new_str {
            drop(conn);
            return Ok(self.current().expect("workspace initialized"));
        }

        // `repos.path` is UNIQUE — refuse to collide with a repo already pinned
        // at the destination rather than fail with an opaque constraint error.
        let taken: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM repos WHERE path = ?1",
                [&new_str],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)?;
        if taken {
            return Err(Error::Other(format!(
                "a project is already pinned at {new_str}"
            )));
        }

        let changed = conn.execute(
            "UPDATE repos SET path = ?1 WHERE path = ?2",
            rusqlite::params![new_str, old_str],
        )?;
        if changed == 0 {
            return Err(Error::Other(format!("repo not found: {old_str}")));
        }
        drop(conn);
        Ok(self.current().expect("workspace initialized"))
    }

    /// All repo paths attached to a project, in creation order (the first is
    /// the primary). Used at spawn to provision one checkout per project repo.
    pub fn project_repo_paths(&self, project_id: &str) -> Vec<PathBuf> {
        let conn = self.db.lock();
        let mut stmt = match conn
            .prepare("SELECT path FROM repos WHERE project_id = ?1 ORDER BY created_at")
        {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([project_id], |row| {
            let p: String = row.get(0)?;
            Ok(PathBuf::from(p))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }
}

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

        let added = if !exists {
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
            Some(project_id)
        } else {
            None
        };

        drop(conn);
        if let Some(project_id) = added {
            self.detect_icon(&project_id);
        }
        Ok(self.current().expect("workspace initialized"))
    }

    /// Give a project the icon its repositories already have, unless it has a
    /// choice on record (`icon_detect`): on add and on relocate.
    pub(crate) fn detect_icon(&self, project_id: &str) -> bool {
        let repos = self.project_repo_paths(project_id);
        crate::icon_detect::detect_if_unset(&self.db, project_id, &repos)
    }

    /// The same for every project with nothing decided: the startup pass that
    /// gives projects from before icons theirs. Blocking file reads; whether
    /// any project got an icon.
    pub fn detect_missing_icons(&self) -> bool {
        match self.project_ids() {
            Ok(ids) => ids.iter().filter(|id| self.detect_icon(id)).count() > 0,
            Err(e) => {
                tracing::warn!(error = %e, "skipping the project icon pass");
                false
            }
        }
    }

    /// Remove a repo from the sidebar's pinned list. Does NOT touch
    /// agents — agents whose primary points at the removed repo keep
    /// working and continue to show in the sidebar under that repo
    /// (the sidebar takes the union of pinned + agent-primary repos).
    pub fn remove_workspace_repo(&self, repo_path: &Path) -> Result<Workspace> {
        let conn = self.db.lock();
        let path_str = repo_path.to_string_lossy().to_string();
        Self::refuse_copy_repo(&conn, &path_str)?;
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
        Self::refuse_copy(&tx, project_id)?;
        Self::refuse_copy_repo(&tx, &path_str)?;

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
                    // Its context goes with it, like every row that hangs off
                    // the project (undo restores the project row alone).
                    crate::context::purge_project(&tx, &source_pid)?;
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
        Self::refuse_copy(&conn, project_id)?;

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

        if Self::has_checkouts(&conn, &repo_id)? {
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
        Self::refuse_copy_repo(&conn, &path_str)?;
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
        Self::refuse_copy(&conn, project_id)?;
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

    /// Every project id in this build's DB: what the toolchain cache reclaim
    /// keeps (`sandbox::reclaim_toolchain_caches`). Fails rather than skipping
    /// a row it cannot read, since a missing id is a cache deleted.
    pub fn project_ids(&self) -> Result<std::collections::HashSet<String>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare("SELECT id FROM projects")?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Atomically delete a project and its workflow runs after filesystem and
    /// runtime cleanup has been staged by the supervisor. Workflow runs do not
    /// have a project FK, so they share this transaction explicitly, as does
    /// the project's context (`context.db` has no FK to `projects`); every
    /// other project-owned row is removed by foreign-key cascade, once any
    /// session outside the project that inherits from one inside is detached.
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
        let doomed = {
            let mut stmt = tx.prepare("SELECT id FROM workspaces WHERE project_id = ?1")?;
            let ids = stmt
                .query_map([project_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };
        let cleanup = sessions::TranscriptCleanup {
            trims: lineage::detach_children(&tx, &doomed)?,
            sessions: sessions::session_ids_for_workspaces(&tx, &doomed)?,
        };
        // Before the row goes: the context id's mapping cascades with it.
        crate::context::purge_project(&tx, project_id)?;
        let changed = tx.execute("DELETE FROM projects WHERE id = ?1", [project_id])?;
        if changed == 0 {
            return Err(Error::Other(format!("project not found: {project_id}")));
        }
        tx.commit()?;
        cleanup.apply(&conn);
        crate::icon_detect::forget(&conn, project_id);
        Ok(())
    }

    /// Repoint a pinned repo at a new location on disk. The user has already
    /// moved the folder; this only updates the stored reference so future
    /// agents spawn from the right place. Validates the new path is a git repo
    /// and isn't already pinned. Existing agents' checkouts still borrow their
    /// objects from the old location: `Supervisor::relocate_repo` relinks them
    /// (`sandbox::provision::relink_alternates`).
    pub fn relocate_repo(&self, old_path: &Path, new_path: &Path) -> Result<Workspace> {
        if !new_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                new_path.display()
            )));
        }

        let mut conn = self.db.lock();
        let old_str = old_path.to_string_lossy().to_string();
        let new_str = new_path.to_string_lossy().to_string();
        Self::refuse_copy_repo(&conn, &old_str)?;

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

        // Repoint the repo and, in the same transaction, every workflow run that
        // snapshotted the old path at launch. `wf_run.repo_path` is a stored
        // column with no FK to `repos`, so — unlike the agents' worktree join,
        // which follows automatically via `repo_id` — it does not track the move
        // on its own. Leaving it stale would fork the sidebar across both paths
        // and resume runs from the old location.
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE repos SET path = ?1 WHERE path = ?2",
            rusqlite::params![new_str, old_str],
        )?;
        if changed == 0 {
            return Err(Error::Other(format!("repo not found: {old_str}")));
        }
        // Scope the run rewrite to the relocated repo's project. `repo_path` is a
        // launch-time snapshot that isn't unique across projects — a lingering
        // historical run from a previous occupant of this same path string would
        // otherwise be dragged to the new location and resume against the wrong
        // repo. `repos.path` is UNIQUE, so this resolves the one owning project.
        let project_id: String = tx.query_row(
            "SELECT project_id FROM repos WHERE path = ?1",
            [&new_str],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE wf_run SET repo_path = ?1 WHERE repo_path = ?2 AND project_id = ?3",
            rusqlite::params![new_str, old_str, project_id],
        )?;
        tx.commit()?;
        drop(conn);
        self.detect_icon(&project_id);
        Ok(self.current().expect("workspace initialized"))
    }

    /// All repo paths attached to a project, in creation order (the first is
    /// the primary). Used at spawn to provision one checkout per project repo.
    /// A project's name, whose it is when it is a copy, and its repositories
    /// (the id each has on the project's home, its path and its label) in the
    /// project's own order: what
    /// `commands::project_definition_impl` describes to another device.
    #[allow(clippy::type_complexity)]
    pub fn project_outline(
        &self,
        project_id: &str,
    ) -> Result<(
        String,
        Option<ProjectHome>,
        Vec<(String, PathBuf, Option<String>)>,
    )> {
        let conn = self.db.lock();
        let (name, home_device, home_name): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT name, home_device, home_name FROM projects WHERE id = ?1",
                [project_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| Error::Other(format!("project not found: {project_id}")))?;
        let mut stmt = conn.prepare(
            "SELECT COALESCE(home_id, id), path, label FROM repos
             WHERE project_id = ?1 ORDER BY created_at, rowid",
        )?;
        let repos = stmt
            .query_map([project_id], |row| {
                Ok((
                    row.get(0)?,
                    PathBuf::from(row.get::<_, String>(1)?),
                    row.get(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let home = home_device.map(|device| ProjectHome {
            device,
            name: home_name.unwrap_or_default(),
        });
        Ok((name, home, repos))
    }

    /// Whether this host owns the project `project_id`: `Some(true)` for one
    /// added here, `Some(false)` for a copy of another device's, `None` when
    /// there is no such project.
    pub fn owns_project(&self, project_id: &str) -> Option<bool> {
        self.db
            .lock()
            .query_row(
                "SELECT home_device IS NULL FROM projects WHERE id = ?1",
                [project_id],
                |row| row.get(0),
            )
            .ok()
    }

    /// The ids, as the home knows them, of the repositories of `project_id`
    /// on this host.
    pub fn project_repo_ids(&self, project_id: &str) -> HashSet<String> {
        let conn = self.db.lock();
        let Ok(mut stmt) =
            conn.prepare("SELECT COALESCE(home_id, id) FROM repos WHERE project_id = ?1")
        else {
            return HashSet::new();
        };
        stmt.query_map([project_id], |row| row.get(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Bring the copy of `def` in line with it, in one transaction. A
    /// repository is known by the id it has on its home (`repos.home_id`),
    /// which is what tells one from another across calls.
    ///
    /// - The project row is created on first sight, and on every later one
    ///   brought up to the name the home has for it now.
    /// - Each of `cloned` (a repository's home id and where its new clone is)
    ///   is recorded; every repository already recorded takes the label the
    ///   home has for it now.
    /// - A recorded repository `def` no longer names leaves the project under
    ///   the rule [`Self::detach_repo_from_project`] holds the home to: once
    ///   no agent here has a checkout of it. Until then it is a repository of
    ///   the project on this host like any other, so nothing that reads the
    ///   project has a second kind of repository to know about.
    ///
    /// A project this host owns is never touched.
    pub fn reconcile_copy(
        &self,
        def: &ProjectDefinition,
        cloned: &[(String, PathBuf)],
    ) -> Result<()> {
        let conn = self.db.lock();
        let tx = conn.unchecked_transaction()?;
        let now = now_millis();
        tx.execute(
            "INSERT INTO projects (id, name, created_at, home_device, home_name)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, home_name = excluded.home_name
             WHERE projects.home_device IS NOT NULL",
            rusqlite::params![
                def.project_id,
                def.name,
                now,
                def.home.device,
                def.home.name
            ],
        )?;
        for (order, repo) in def.repos.iter().enumerate() {
            match cloned.iter().find(|(id, _)| *id == repo.id) {
                Some((_, path)) => tx.execute(
                    "INSERT INTO repos (id, project_id, path, label, created_at, home_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        uuid::Uuid::new_v4().to_string(),
                        def.project_id,
                        path.to_string_lossy(),
                        repo.label,
                        // Keeps the home's order among rows written together.
                        now + order as i64,
                        repo.id
                    ],
                )?,
                None => tx.execute(
                    "UPDATE repos SET label = ?1 WHERE home_id = ?2 AND project_id = ?3",
                    rusqlite::params![repo.label, repo.id, def.project_id],
                )?,
            };
        }

        let recorded: Vec<(String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id, home_id FROM repos WHERE project_id = ?1 AND home_id IS NOT NULL",
            )?;
            let rows = stmt
                .query_map([&def.project_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            rows
        };
        for (repo_id, home_id) in recorded {
            let named = def.repos.iter().any(|r| r.id == home_id);
            if !named && !Self::has_checkouts(&tx, &repo_id)? {
                tx.execute("DELETE FROM repos WHERE id = ?1", [&repo_id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Whether any agent, live or archived, has a checkout of the repository
    /// `repo_id`. One that does cannot leave its project: the
    /// `worktrees.repo_id` cascade would take those agents' tracked branches,
    /// PRs and archive snapshots with it.
    fn has_checkouts(conn: &Connection, repo_id: &str) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM worktrees WHERE repo_id = ?1",
            [repo_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Refuse a change to the structure of a project this host only runs: a
    /// copy is edited on its home, and what is changed here would be neither
    /// seen there nor undone by the next `ensure_project`.
    fn refuse_copy(conn: &Connection, project_id: &str) -> Result<()> {
        let home: Option<(Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT home_device, home_name FROM projects WHERE id = ?1",
                [project_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        match home {
            Some((Some(_), name)) => {
                let name = name.filter(|n| !n.trim().is_empty());
                Err(Error::Other(format!(
                    "This project belongs to {}. Change it there.",
                    name.as_deref().unwrap_or("another device")
                )))
            }
            _ => Ok(()),
        }
    }

    /// [`Self::refuse_copy`] for the project the repository at `path_str`
    /// belongs to. A path that is not pinned belongs to none.
    fn refuse_copy_repo(conn: &Connection, path_str: &str) -> Result<()> {
        let project_id: Option<String> = conn
            .query_row(
                "SELECT project_id FROM repos WHERE path = ?1",
                [path_str],
                |row| row.get(0),
            )
            .ok();
        match project_id {
            Some(project_id) => Self::refuse_copy(conn, &project_id),
            None => Ok(()),
        }
    }

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

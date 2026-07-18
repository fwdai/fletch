//! `impl WorkspaceManager` — private query / row-mapping helpers shared across
//! the domain submodules.

use super::*;

impl WorkspaceManager {
    // ── Internal helpers ──────────────────────────────────────────────────

    /// Translate a requested runtime status into durable disposition writes.
    /// There is no status column — only the workspace's `stopped_at` and the
    /// session's `last_error` are persisted; everything else is derived.
    pub(super) fn apply_status(
        conn: &Connection,
        id: &str,
        status: &AgentStatus,
        last_error: Option<&str>,
    ) -> Result<()> {
        match status {
            // User-stopped — stamp it once (don't clobber an earlier stop time).
            AgentStatus::Stopped => {
                conn.execute(
                    "UPDATE workspaces SET stopped_at = ?1
                     WHERE id = ?2 AND stopped_at IS NULL",
                    rusqlite::params![now_millis(), id],
                )?;
            }
            // Resuming or active — clear the stop disposition and any stale error.
            AgentStatus::Spawning | AgentStatus::Running => {
                conn.execute(
                    "UPDATE workspaces SET stopped_at = NULL WHERE id = ?1",
                    [id],
                )?;
                conn.execute(
                    "UPDATE sessions SET last_error = NULL WHERE workspace_id = ?1",
                    [id],
                )?;
            }
            // Record the failure on the session row.
            AgentStatus::Error => {
                conn.execute(
                    "UPDATE sessions SET last_error = ?1 WHERE workspace_id = ?2",
                    rusqlite::params![last_error, id],
                )?;
            }
            // Idle is a pure runtime state with no durable representation.
            AgentStatus::Idle => {}
        }
        Ok(())
    }

    /// One `ProjectRef` per repo, keyed by path (the frontend looks these up by
    /// path, not by index). A LEFT JOIN keeps a repo even if its project row is
    /// somehow missing — the name then falls back to the folder basename rather
    /// than the repo silently vanishing from the sidebar.
    pub(super) fn query_project_refs(conn: &Connection) -> Vec<ProjectRef> {
        let mut stmt = match conn.prepare(
            "SELECT r.path, p.name, p.id, r.label
             FROM repos r LEFT JOIN projects p ON p.id = r.project_id
             ORDER BY r.created_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let name: Option<String> = row.get(1)?;
            let project_id: Option<String> = row.get(2)?;
            let label: Option<String> = row.get(3)?;
            let name = name.unwrap_or_else(|| {
                Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&path)
                    .to_string()
            });
            Ok(ProjectRef {
                path: PathBuf::from(path),
                name,
                project_id: project_id.unwrap_or_default(),
                label,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    pub(super) fn query_all_repo_paths(conn: &Connection) -> Vec<PathBuf> {
        let mut stmt = conn
            .prepare("SELECT path FROM repos ORDER BY created_at")
            .unwrap_or_else(|_| conn.prepare("SELECT 1 WHERE 0").unwrap());
        stmt.query_map([], |row| {
            let p: String = row.get(0)?;
            Ok(PathBuf::from(p))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    pub(super) fn query_all_agents(conn: &Connection) -> Vec<AgentRecord> {
        // Run-owned step agents live under their workflow run, not the
        // sidebar; the frontend never sees owner_run_id, so filter here.
        let mut stmt = match conn.prepare(&format!(
            "{AGENT_SELECT} WHERE w.owner_run_id IS NULL ORDER BY w.created_at"
        )) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        // Collect the raw rows first, then build records: building a record
        // issues further queries on `conn`, which can't run while `stmt`
        // still borrows it.
        stmt.query_map([], Self::map_agent_row)
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .map(|row| Self::build_agent_record(conn, row))
            .collect()
    }

    /// Step agents owned by a workflow run (`owner_run_id = run_id`), including
    /// archived ones so the run monitor can open the chat of any attempt — live
    /// or abandoned. The inverse of [`query_all_agents`]'s sidebar filter.
    pub(super) fn query_agents_for_run(conn: &Connection, run_id: &str) -> Vec<AgentRecord> {
        let mut stmt = match conn.prepare(&format!(
            "{AGENT_SELECT} WHERE w.owner_run_id = ?1 ORDER BY w.created_at"
        )) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([run_id], Self::map_agent_row)
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .map(|row| Self::build_agent_record(conn, row))
            .collect()
    }

    pub(super) fn query_agents_for_project(
        conn: &Connection,
        project_id: &str,
    ) -> Vec<AgentRecord> {
        let mut stmt = match conn.prepare(&format!(
            "{AGENT_SELECT} WHERE w.project_id = ?1 ORDER BY w.created_at"
        )) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([project_id], Self::map_agent_row)
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .map(|row| Self::build_agent_record(conn, row))
            .collect()
    }

    fn query_tracked_repos(conn: &Connection, agent_id: &str) -> Vec<TrackedRepo> {
        let mut stmt = match conn.prepare(
            "SELECT r.path, w.subdir, w.branch, w.parent_branch, w.base_sha, w.pr_number,
                    w.pr_url, w.pr_title, w.pr_state, r.label
             FROM worktrees w
             JOIN repos r ON r.id = w.repo_id
             WHERE w.workspace_id = ?1
             ORDER BY w.created_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([agent_id], |row| {
            let path: String = row.get(0)?;
            let subdir: String = row.get(1)?;
            let branch: Option<String> = row.get(2)?;
            let parent_branch: Option<String> = row.get(3)?;
            let base_sha: Option<String> = row.get(4)?;
            let pr_number: Option<i64> = row.get(5)?;
            let pr_url: Option<String> = row.get(6)?;
            let pr_title: Option<String> = row.get(7)?;
            let pr_state: Option<String> = row.get(8)?;
            let label: Option<String> = row.get(9)?;
            Ok(TrackedRepo {
                repo_path: PathBuf::from(path),
                subdir,
                branch,
                parent_branch,
                base_sha,
                pr_number,
                pr_url,
                pr_title,
                pr_state,
                label,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    fn build_archive_metadata(
        conn: &Connection,
        agent_id: &str,
        archived_millis: i64,
    ) -> ArchiveMetadata {
        let mut stmt = match conn.prepare(
            "SELECT r.path, w.subdir, w.branch, w.branch_tip_sha,
                    w.parent_branch, w.parent_branch_sha,
                    w.diff_additions, w.diff_deletions
             FROM worktrees w
             JOIN repos r ON r.id = w.repo_id
             WHERE w.workspace_id = ?1
             ORDER BY w.created_at",
        ) {
            Ok(s) => s,
            Err(_) => {
                return ArchiveMetadata {
                    archived_at: millis_to_iso(archived_millis),
                    repos: Vec::new(),
                    diff_stats: DiffStats::default(),
                }
            }
        };

        let snapshots: Vec<ArchivedRepoSnapshot> = stmt
            .query_map([agent_id], |row| {
                let path: String = row.get(0)?;
                let subdir: String = row.get(1)?;
                let branch: Option<String> = row.get(2)?;
                let branch_tip_sha: Option<String> = row.get(3)?;
                let parent_branch: Option<String> = row.get(4)?;
                let parent_branch_sha: Option<String> = row.get(5)?;
                let additions: u32 = row.get(6)?;
                let deletions: u32 = row.get(7)?;
                Ok(ArchivedRepoSnapshot {
                    repo_path: PathBuf::from(path),
                    subdir,
                    branch_name: branch,
                    branch_tip_sha,
                    parent_branch,
                    parent_branch_sha,
                    diff_stats: DiffStats {
                        additions,
                        deletions,
                    },
                })
            })
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        // Aggregate diff stats across all repos.
        let total_additions: u32 = snapshots.iter().map(|s| s.diff_stats.additions).sum();
        let total_deletions: u32 = snapshots.iter().map(|s| s.diff_stats.deletions).sum();

        ArchiveMetadata {
            archived_at: millis_to_iso(archived_millis),
            repos: snapshots,
            diff_stats: DiffStats {
                additions: total_additions,
                deletions: total_deletions,
            },
        }
    }

    pub(super) fn find_or_create_project(conn: &Connection, name: &str) -> Result<String> {
        // Try to find an existing project by name.
        let existing: Option<String> = conn
            .query_row("SELECT id FROM projects WHERE name = ?1", [name], |row| {
                row.get(0)
            })
            .ok();

        if let Some(id) = existing {
            return Ok(id);
        }

        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, now_millis()],
        )?;
        Ok(id)
    }

    /// Look up the project_id for a repo path. If the repo doesn't exist
    /// yet, create both the project and repo record.
    pub(super) fn project_id_for_repo_path(conn: &Connection, path_str: &str) -> Result<String> {
        // Try to find existing repo.
        let existing: Option<String> = conn
            .query_row(
                "SELECT project_id FROM repos WHERE path = ?1",
                [path_str],
                |row| row.get(0),
            )
            .ok();

        if let Some(pid) = existing {
            return Ok(pid);
        }

        // Create project + repo.
        let repo_path = Path::new(path_str);
        let project_name = repo_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let project_id = Self::find_or_create_project(conn, &project_name)?;

        let repo_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO repos (id, project_id, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![repo_id, project_id, path_str, now_millis()],
        )?;

        Ok(project_id)
    }

    pub(super) fn insert_worktree(
        conn: &Connection,
        agent_id: &str,
        repo: &TrackedRepo,
    ) -> Result<()> {
        let path_str = repo.repo_path.to_string_lossy().to_string();

        // Look up repo_id from repos table.
        let repo_id: String = conn
            .query_row("SELECT id FROM repos WHERE path = ?1", [&path_str], |row| {
                row.get(0)
            })
            .map_err(|_| Error::Other(format!("repo not found in database: {path_str}")))?;

        let wt_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO worktrees (id, workspace_id, repo_id, subdir, branch, parent_branch, base_sha, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                wt_id,
                agent_id,
                repo_id,
                repo.subdir,
                repo.branch,
                repo.parent_branch,
                repo.base_sha,
                now_millis(),
            ],
        )?;
        Ok(())
    }

    pub(super) fn ensure_agent_exists(conn: &Connection, id: &str) -> Result<()> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM workspaces WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        if count == 0 {
            return Err(Error::AgentNotFound(id.to_string()));
        }
        Ok(())
    }

    pub(super) fn load_agent(conn: &Connection, id: &str) -> Result<AgentRecord> {
        let row = conn
            .query_row(
                &format!("{AGENT_SELECT} WHERE w.id = ?1"),
                [id],
                Self::map_agent_row,
            )
            .map_err(|_| Error::AgentNotFound(id.to_string()))?;

        Ok(Self::build_agent_record(conn, row))
    }

    /// Map a row from an [`AGENT_SELECT`] query into the raw column tuple.
    /// Shared by `query_all_agents` and `load_agent` so the 21-column layout
    /// is decoded in exactly one place.
    fn map_agent_row(row: &rusqlite::Row) -> rusqlite::Result<AgentRow> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
            row.get(11)?,
            row.get(12)?,
            row.get(13)?,
            row.get(14)?,
            row.get(15)?,
            row.get(16)?,
            row.get(17)?,
            row.get(18)?,
            row.get(19)?,
            row.get(20)?,
        ))
    }

    /// Build an [`AgentRecord`] from a raw [`AGENT_SELECT`] row, issuing the
    /// follow-up queries for tracked repos or archive metadata. Status is
    /// derived from durable dispositions, never selected.
    fn build_agent_record(conn: &Connection, row: AgentRow) -> AgentRecord {
        let (
            id,
            project_id,
            name,
            task,
            created_millis,
            stopped_millis,
            archived_millis,
            provider,
            view_str,
            session_id,
            last_error,
            effort,
            model,
            instructions,
            forked_context,
            custom_agent_id,
            skills_json,
            mcp_servers_json,
            sandbox_engine,
            owner_run_id,
            issue_ref,
        ) = row;

        let is_archived = archived_millis.is_some();

        let (repos, archive) = if is_archived {
            // Build ArchiveMetadata from checkout snapshot fields.
            let archive_meta = Self::build_archive_metadata(conn, &id, archived_millis.unwrap());
            (Vec::new(), Some(archive_meta))
        } else {
            // Build TrackedRepo vec from worktrees+repos join.
            let tracked = Self::query_tracked_repos(conn, &id);
            (tracked, None)
        };

        let status = derive_status(
            is_archived,
            stopped_millis.is_some(),
            false,
            last_error.as_deref(),
        );

        AgentRecord {
            id,
            project_id,
            name,
            provider: provider.unwrap_or_else(default_provider),
            repos,
            task,
            status,
            view: str_to_view(view_str.as_deref().unwrap_or("custom")),
            session_id,
            effort,
            model,
            instructions,
            forked_context,
            custom_agent_id,
            skills: decode_json_vec(skills_json.as_deref()),
            mcp_servers: decode_json_vec(mcp_servers_json.as_deref()),
            sandbox_engine,
            owner_run_id,
            issue_ref,
            created_at: millis_to_iso(created_millis),
            last_error,
            archive,
        }
    }
}

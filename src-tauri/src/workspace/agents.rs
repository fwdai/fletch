//! `impl WorkspaceManager` — agent CRUD, per-repo metadata (branch / base_sha /
//! pr_number / pr_snapshot), archive & restore, setup flags, settings, run env.

use super::*;

impl WorkspaceManager {
    pub fn allocate_agent_id(&self) -> Result<String> {
        // DB-authoritative (see `live_agent_ids`); no filesystem check needed.
        // Two instances of the same build share this DB, so a concurrent
        // allocation race is resolved by the `workspaces.id` primary key: the
        // loser's `add_agent` INSERT fails, and since insert precedes provision
        // in the spawn path it never creates — or clears — a checkout dir.
        let used: HashSet<String> = self.live_agent_ids()?.into_iter().collect();
        Ok(names::allocate(&used))
    }

    /// Ids of every live (non-archived) agent — the only names that are
    /// reserved. Archived agents have had their checkout torn down, so their
    /// name is free to reuse. With a per-build checkouts root (see
    /// `checkouts_root`) no other build shares this namespace, so the DB is
    /// authoritative and allocation never consults the filesystem: a stale dir
    /// from a crashed spawn or a failed teardown can't collide either, since
    /// provision clears any leftover at the clone target.
    pub fn live_agent_ids(&self) -> Result<Vec<String>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare("SELECT id FROM workspaces WHERE archived_at IS NULL")?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }

    pub fn add_agent(&self, record: &mut AgentRecord) -> Result<()> {
        let conn = self.db.lock();

        // Look up project_id from the primary repo path.
        let project_id = if let Some(primary) = record.repos.first() {
            let path_str = primary.repo_path.to_string_lossy().to_string();
            Self::project_id_for_repo_path(&conn, &path_str)?
        } else {
            return Err(Error::Other("agent must have at least one repo".into()));
        };
        record.project_id = project_id.clone();

        // Parse created_at ISO string to millis.
        let created_millis = chrono::DateTime::parse_from_rfc3339(&record.created_at)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| now_millis());

        // Evicting the recycled archived row and writing its replacement must be
        // atomic. Without a transaction the DELETE auto-commits immediately, so
        // any later failure (a failed INSERT, disk-full, UUID collision) would
        // leave the archived agent — and its cascaded sessions/worktrees —
        // permanently gone with nothing in its place. The transaction rolls the
        // DELETE back on any error before `commit`.
        let tx = conn.unchecked_transaction()?;

        // Recycling a freed name: the allocator only hands back ids held by
        // *archived* agents (live ones and on-disk checkouts are excluded), but
        // the archived row still owns this primary key. Evict it so the INSERT
        // below doesn't trip the PK constraint. Cascades clear its sessions,
        // worktrees, and session records. A *live* row with this id would be a
        // genuine bug, so we deliberately don't touch those — the INSERT will
        // surface the conflict instead of silently clobbering a running agent.
        let recycled = tx.execute(
            "DELETE FROM workspaces WHERE id = ?1 AND archived_at IS NOT NULL",
            rusqlite::params![record.id],
        )?;
        if recycled > 0 {
            tracing::info!(
                agent_id = %record.id,
                "reusing archived agent name; evicted its archived record",
            );
        }

        // The workspace is the durable work-area (identity + task metadata).
        tx.execute(
            "INSERT INTO workspaces (id, project_id, name, task, created_at, sandbox_engine, owner_run_id, issue_ref)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                record.id,
                project_id,
                record.name,
                record.task,
                created_millis,
                record.sandbox_engine,
                record.owner_run_id,
                record.issue_ref,
            ],
        )?;

        // Exactly one provider run per workspace today. The runtime status is
        // not persisted — it derives from the workspace/session dispositions.
        let session_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO sessions (id, workspace_id, provider, view, provider_session_id, last_error, effort, model, instructions, forked_context, custom_agent_id, skills, mcp_servers, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                session_id,
                record.id,
                record.provider,
                view_to_str(&record.view),
                record.session_id,
                record.last_error,
                record.effort,
                record.model,
                record.instructions,
                record.forked_context,
                record.custom_agent_id,
                encode_json_vec(&record.skills),
                encode_json_vec(&record.mcp_servers),
                created_millis,
            ],
        )?;

        // Insert checkout records for each TrackedRepo.
        for repo in &record.repos {
            Self::insert_worktree(&tx, &record.id, repo)?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn update_agent_status(
        &self,
        id: &str,
        status: AgentStatus,
        last_error: Option<String>,
    ) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        Self::apply_status(&conn, id, &status, last_error.as_deref())?;
        Ok(())
    }

    pub fn set_agent_task_if_empty(&self, id: &str, task: &str) -> Result<bool> {
        let conn = self.db.lock();
        let changed = conn.execute(
            "UPDATE workspaces SET task = ?1 WHERE id = ?2 AND (task = '' OR task IS NULL)",
            rusqlite::params![task, id],
        )?;
        Ok(changed > 0)
    }

    /// Set the branch on a specific tracked repo within an agent — but
    /// only if it isn't set yet. Identified by subdir (unique per
    /// agent). Returns true iff it actually wrote.
    /// Record the branch a tracked repo's checkout is on, identified by subdir.
    /// Written when the agent materializes its branch at first push (see
    /// `open_pr`/`git_push`). Overwrites unconditionally — a second PR cuts a
    /// fresh branch in the same checkout, so the recorded name can change.
    pub fn set_repo_branch(&self, agent_id: &str, subdir: &str, branch: &str) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE worktrees SET branch = ?1 WHERE workspace_id = ?2 AND subdir = ?3",
            rusqlite::params![branch, agent_id, subdir],
        )?;
        Ok(())
    }

    /// Record the fork-point SHA for a tracked repo, identified by subdir.
    /// Written once the spawn task has created the checkout and resolved its
    /// HEAD. Overwrites unconditionally — the fork point is fixed for the
    /// checkout's life, so a re-write only ever sets the same value.
    pub fn set_repo_base_sha(&self, agent_id: &str, subdir: &str, base_sha: &str) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE worktrees SET base_sha = ?1 WHERE workspace_id = ?2 AND subdir = ?3",
            rusqlite::params![base_sha, agent_id, subdir],
        )?;
        Ok(())
    }

    /// Record the GitHub PR number for a tracked repo, identified by subdir.
    /// Written when a PR is created through the app or adopted from an OPEN
    /// out-of-band PR. Overwrites unconditionally — the latest PR opened for
    /// the branch is the one we track.
    pub fn set_repo_pr_number(&self, agent_id: &str, subdir: &str, pr_number: i64) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE worktrees SET pr_number = ?1 WHERE workspace_id = ?2 AND subdir = ?3",
            rusqlite::params![pr_number, agent_id, subdir],
        )?;
        Ok(())
    }

    /// Stamp the PR's GitHub-reported open/merge times onto a tracked repo,
    /// identified by subdir. Called from every PR-state fetch path; GitHub is
    /// the source of truth, so values overwrite (COALESCE keeps an existing
    /// stamp when a fetch reports none). NULL until first observed — a PR
    /// merged while the app was closed still gets its real merge time on the
    /// next fetch.
    /// Persist a successful PR fetch: identity (number), the display snapshot
    /// (url / title / state), and GitHub's own lifecycle times. One write per
    /// fetch keeps the database the durable source of truth the UI can render
    /// from when GitHub or the checkout is unavailable. Times COALESCE so an
    /// earlier-observed value is never erased by a payload that omits it.
    pub fn set_repo_pr_snapshot(
        &self,
        agent_id: &str,
        subdir: &str,
        pr: &crate::github::PrState,
    ) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE worktrees SET pr_number = ?1, pr_url = ?2, pr_title = ?3, pr_state = ?4,
                                  pr_opened_at = COALESCE(?5, pr_opened_at),
                                  pr_merged_at = COALESCE(?6, pr_merged_at)
             WHERE workspace_id = ?7 AND subdir = ?8",
            rusqlite::params![
                pr.number as i64,
                pr.url,
                pr.title,
                pr.state.as_str(),
                pr.opened_at,
                pr.merged_at,
                agent_id,
                subdir,
            ],
        )?;
        Ok(())
    }

    pub fn append_tracked_repo(&self, agent_id: &str, repo: TrackedRepo) -> Result<()> {
        let conn = self.db.lock();
        Self::insert_worktree(&conn, agent_id, &repo)?;
        Ok(())
    }

    /// Persist the agent's session id. Used for Codex, whose thread id
    /// is assigned by the CLI and captured from its first turn's events
    /// (Claude's id is generated up front, so it never changes here).
    pub fn set_agent_session_id(&self, id: &str, session_id: &str) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE sessions SET provider_session_id = ?1 WHERE workspace_id = ?2",
            rusqlite::params![session_id, id],
        )?;
        Ok(())
    }

    pub fn update_agent_view(&self, id: &str, view: AgentView) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE sessions SET view = ?1 WHERE workspace_id = ?2",
            rusqlite::params![view_to_str(&view), id],
        )?;
        Ok(())
    }

    /// Re-tag an agent with the issue it's working ("123" / "ENG-123"), or
    /// clear it with `None`. The row is the durable source for the PR
    /// closing trailer across restarts; the caller also updates the live
    /// registry (`crate::issues`) the git dispatcher reads mid-session.
    pub fn update_agent_issue_ref(&self, id: &str, issue_ref: Option<&str>) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE workspaces SET issue_ref = ?1 WHERE id = ?2",
            rusqlite::params![issue_ref, id],
        )?;
        Ok(())
    }

    pub fn agent(&self, id: &str) -> Result<AgentRecord> {
        let conn = self.db.lock();
        Self::load_agent(&conn, id)
    }

    /// Mark an agent as archived. Stamps `archived_at`, stores the
    /// snapshot of every tracked repo, and clears `repos` so the
    /// frontend doesn't treat the (now-deleted) checkouts as live.
    /// Status moves to `Stopped` so resume-on-launch ignores it.
    pub fn archive_agent(&self, id: &str, archive: ArchiveMetadata) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;

        // Parse archived_at string to millis for storage.
        let archived_millis = chrono::DateTime::parse_from_rfc3339(&archive.archived_at)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| now_millis());

        // Clear setup_completed_at too — restore recreates the checkout
        // from scratch, so node_modules etc. won't be there. Stamping
        // archived_at is enough to derive `Stopped`; there is no status
        // column to flip.
        conn.execute(
            "UPDATE workspaces SET archived_at = ?1,
                    setup_completed_at = NULL WHERE id = ?2",
            rusqlite::params![archived_millis, id],
        )?;

        // Update checkout rows with snapshot data from ArchiveMetadata.repos.
        for snap in &archive.repos {
            conn.execute(
                "UPDATE worktrees SET branch_tip_sha = ?1, parent_branch_sha = ?2,
                        diff_additions = ?3, diff_deletions = ?4
                 WHERE workspace_id = ?5 AND subdir = ?6",
                rusqlite::params![
                    snap.branch_tip_sha,
                    snap.parent_branch_sha,
                    snap.diff_stats.additions,
                    snap.diff_stats.deletions,
                    id,
                    snap.subdir,
                ],
            )?;
        }

        Ok(())
    }

    /// Clear archive metadata and re-seed `repos`. Clearing `archived_at`
    /// (with no `stopped_at`/error) makes the workspace derive back to
    /// `Idle`; the supervisor's restore path drives the live spawn explicitly.
    pub fn restore_agent(&self, id: &str, repos: Vec<TrackedRepo>) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;

        // Clearing both dispositions (archived + user-stopped) returns the
        // record to its resting `Idle` state — a restored agent should be
        // live-able again, not stuck Stopped.
        conn.execute(
            "UPDATE workspaces SET archived_at = NULL, stopped_at = NULL WHERE id = ?1",
            [id],
        )?;

        // Update checkout records with new branch info and clear snapshot fields.
        for repo in &repos {
            conn.execute(
                "UPDATE worktrees SET branch = ?1, parent_branch = ?2,
                        branch_tip_sha = NULL, parent_branch_sha = NULL,
                        diff_additions = 0, diff_deletions = 0
                 WHERE workspace_id = ?3 AND subdir = ?4",
                rusqlite::params![repo.branch, repo.parent_branch, id, repo.subdir],
            )?;
        }

        Ok(())
    }

    /// Has the Run panel's setup command ever succeeded for this agent?
    /// Cleared on archive so a restored agent re-runs setup against the
    /// freshly-recreated checkout.
    pub fn is_setup_completed(&self, id: &str) -> Result<bool> {
        let conn = self.db.lock();
        let value: Option<i64> = conn
            .query_row(
                "SELECT setup_completed_at FROM workspaces WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|_| Error::AgentNotFound(id.to_string()))?;
        Ok(value.is_some())
    }

    /// A single project-scoped setting value (e.g. the Run panel's
    /// `run.install` / `run.dev` overrides). `None` when unset.
    pub fn project_setting(&self, project_id: &str, key: &str) -> Option<String> {
        let conn = self.db.lock();
        conn.query_row(
            "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
            rusqlite::params![project_id, key],
            |row| row.get::<_, String>(0),
        )
        .ok()
    }

    /// Resolve the project's shared `.env` variables into `(NAME, VALUE)`
    /// pairs to inject into a sandboxed Run process — the opt-in env membrane
    /// (see [`crate::run_env`]). Reads the `.env` from the *source* `repo_path`
    /// (gitignored files are absent from the worktree). Never errors: an
    /// unreadable `.env`, absent config, or an unavailable keychain simply
    /// yields fewer (or no) injected vars.
    pub fn run_env(
        &self,
        project_id: &str,
        repo_path: &std::path::Path,
        agent_id: &str,
        worktree: &std::path::Path,
    ) -> Vec<(String, String)> {
        let conn = self.db.lock();
        crate::run_env::resolve(
            &conn,
            project_id,
            repo_path,
            &crate::run_env::InterpCtx { agent_id, worktree },
        )
    }

    /// Resolve the project_id for a repo path (creating the project/repo
    /// record if it doesn't exist yet — idempotent). The sidebar keys its
    /// project groups by repo path, so the Project Settings surface uses
    /// this to reach the `project_settings` rows, which are keyed by
    /// project_id.
    pub fn project_id_for_repo(&self, repo_path: &str) -> Result<String> {
        let conn = self.db.lock();
        Self::project_id_for_repo_path(&conn, repo_path)
    }

    /// Stamp the setup command as having succeeded. Idempotent.
    pub fn mark_setup_completed(&self, id: &str) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE workspaces SET setup_completed_at = ?1 WHERE id = ?2",
            rusqlite::params![now_millis(), id],
        )?;
        Ok(())
    }

    pub fn remove_agent(&self, id: &str) -> Result<()> {
        let conn = self.db.lock();
        // Cascades to the workspace's sessions, worktrees, and session_records.
        conn.execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
        Ok(())
    }
}

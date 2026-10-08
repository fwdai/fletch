//! `impl WorkspaceManager` — agent CRUD, per-repo metadata (branch / base_sha /
//! pr_number / pr_snapshot), archive & restore, setup flags, settings, run env.

use rusqlite::OptionalExtension;

use super::*;

/// Every provider session Fletch has run, by its provider session id, with
/// the account its workspace is stamped with (`default` for no stamp) — what
/// the usage scan credits that session's spend to. Archived workspaces
/// included: their spend still happened under the stamp.
pub fn session_accounts(
    conn: &rusqlite::Connection,
) -> Result<std::collections::HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT s.provider_session_id, w.provider_account FROM sessions s
           JOIN workspaces w ON w.id = s.workspace_id
          WHERE s.provider_session_id IS NOT NULL AND s.provider_session_id != ''",
    )?;
    let rows = stmt.query_map([], |row| {
        let session: String = row.get(0)?;
        let account: Option<String> = row.get(1)?;
        Ok((session, account))
    })?;
    let mut out = std::collections::HashMap::new();
    for row in rows {
        let (session, account) = row?;
        let account = account
            .filter(|a| !crate::agent::accounts::is_default(a))
            .unwrap_or_else(|| crate::agent::accounts::DEFAULT_ACCOUNT.to_string());
        out.insert(session, account);
    }
    Ok(out)
}

/// Every agent by id, with the account its workspace is stamped with
/// (`default` for no stamp), archived ones included — what the usage scan
/// credits an agent's own codex session dir to.
pub fn agent_accounts(
    conn: &rusqlite::Connection,
) -> Result<std::collections::HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT id, provider_account FROM workspaces")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    let mut out = std::collections::HashMap::new();
    for row in rows {
        let (id, account) = row?;
        let account = account
            .filter(|a| !crate::agent::accounts::is_default(a))
            .unwrap_or_else(|| crate::agent::accounts::DEFAULT_ACCOUNT.to_string());
        out.insert(id, account);
    }
    Ok(out)
}

/// How many live (non-archived) agents of `provider` are stamped with the
/// managed account `account` (see `agent::accounts`). Gates removing the
/// account: its directory holds the login those agents launch on (and a codex
/// account's transcripts), and a running codex agent would write it straight
/// back. Reads the current stamp, so an agent switched off the account
/// (`Supervisor::switch_account`) no longer holds it. A free function over the
/// connection because the accounts commands hold one, not a manager; the
/// provider is read off the workspace's
/// sessions, where it lives.
pub fn live_agents_on_account(
    conn: &rusqlite::Connection,
    provider: &str,
    account: &str,
) -> Result<i64> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM workspaces w
          WHERE w.archived_at IS NULL
            AND w.provider_account = ?2
            AND EXISTS (SELECT 1 FROM sessions s
                         WHERE s.workspace_id = w.id AND s.provider = ?1)",
        rusqlite::params![provider, account],
        |row| row.get(0),
    )?;
    Ok(count)
}

impl WorkspaceManager {
    /// Ids of every live (non-archived) agent in this build's DB — the set of
    /// names that are actually reserved. Archived agents have had their
    /// checkout and mailbox torn down, so their name is free to reuse.
    ///
    /// The one definition of "taken", shared by the draft-name preview
    /// (`allocate_draft_name`) and the startup mailbox sweep. Callers never
    /// assemble this set themselves — a caller-supplied set is how archived
    /// agents once leaked into name allocation and saturated the pool.
    /// `add_agent_allocating` runs the same query, but inline, because it must
    /// read inside its own `IMMEDIATE` transaction to stay serialized.
    pub fn live_agent_ids(&self) -> Result<HashSet<String>> {
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
        let tx = conn.unchecked_transaction()?;
        let cleanup = Self::insert_agent(&tx, record)?;
        tx.commit()?;
        cleanup.apply(&conn);
        Ok(())
    }

    /// Allocate a fresh name and insert the record in one serialized transaction.
    ///
    /// Allocation is DB-authoritative: the only reserved names are this build's
    /// live (non-archived) rows. Archived agents have had their checkout torn
    /// down, and each build has its own checkouts root (see `checkouts_root`),
    /// so no other build shares this namespace and we never consult the
    /// filesystem — a stale dir from a crashed spawn or a failed teardown can't
    /// collide, since provision clears any leftover at the clone target.
    ///
    /// The live-name read and the insert run in a single `IMMEDIATE`
    /// transaction, so allocation is serialized by the database's write lock —
    /// the one coordinator threads *and* separate processes share. Two instances
    /// of the same build hold separate connections the in-process mutex can't
    /// coordinate, but `IMMEDIATE` takes the write lock up front, so a second
    /// allocator waits (WAL + `busy_timeout`), then reads the committed set and
    /// picks a distinct name. No `workspaces.id` conflict can arise, so there is
    /// nothing to retry. Overwrites `record.id`/`record.name`; callers that must
    /// keep a specific id (restore, a draft-supplied name) use `add_agent`,
    /// where a clash is a real error.
    pub fn add_agent_allocating(&self, record: &mut AgentRecord) -> Result<()> {
        let mut conn = self.db.lock();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let live: HashSet<String> = {
            let mut stmt = tx.prepare("SELECT id FROM workspaces WHERE archived_at IS NULL")?;
            let ids = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();
            ids
        };
        let id = names::allocate(&live);
        record.name = id.clone();
        record.id = id;
        let cleanup = Self::insert_agent(&tx, record)?;
        tx.commit()?;
        cleanup.apply(&conn);
        Ok(())
    }

    /// Write the workspace + session + worktree rows for a new agent. Runs
    /// inside the caller's transaction — which also scopes name allocation in
    /// `add_agent_allocating` — so the recycle-delete and every insert commit
    /// (or roll back) as one unit.
    fn insert_agent(
        tx: &rusqlite::Transaction,
        record: &mut AgentRecord,
    ) -> Result<sessions::TranscriptCleanup> {
        // Look up project_id from the primary repo path.
        let project_id = if let Some(primary) = record.repos.first() {
            let path_str = primary.repo_path.to_string_lossy().to_string();
            Self::project_id_for_repo_path(tx, &path_str)?
        } else {
            return Err(Error::Other("agent must have at least one repo".into()));
        };
        record.project_id = project_id.clone();

        // Parse created_at ISO string to millis.
        let created_millis = chrono::DateTime::parse_from_rfc3339(&record.created_at)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| now_millis());

        // Recycling a freed name: the allocator only hands back ids held by
        // *archived* agents (live ones and on-disk checkouts are excluded), but
        // the archived row still owns this primary key. Evict it so the INSERT
        // below doesn't trip the PK constraint. Cascades clear its sessions and
        // worktrees, once any fork still inheriting from them is detached; its
        // transcript rows are returned for the caller to delete after commit.
        // A *live* row with this id would be a genuine bug,
        // so we deliberately don't touch those — the INSERT will surface the
        // conflict instead of silently clobbering a running agent.
        let archived = tx
            .query_row(
                "SELECT id FROM workspaces WHERE id = ?1 AND archived_at IS NOT NULL",
                [&record.id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        let mut cleanup = sessions::TranscriptCleanup::default();
        if let Some(evicted) = archived {
            let doomed = std::slice::from_ref(&evicted);
            cleanup.trims = lineage::detach_children(tx, doomed)?;
            cleanup.sessions = sessions::session_ids_for_workspaces(tx, doomed)?;
            tx.execute("DELETE FROM workspaces WHERE id = ?1", [&evicted])?;
            tracing::info!(
                agent_id = %record.id,
                "reusing archived agent name; evicted its archived record",
            );
        }

        // The workspace is the durable work-area (identity + task metadata).
        tx.execute(
            "INSERT INTO workspaces (id, project_id, name, task, created_at, sandbox_engine, owner_run_id, issue_ref, purpose, provider_account)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                record.id,
                project_id,
                record.name,
                record.task,
                created_millis,
                record.sandbox_engine,
                record.owner_run_id,
                record.issue_ref,
                record.purpose,
                record.account,
            ],
        )?;

        // The workspace's first (and current) session, carrying its lineage
        // when it continues an earlier conversation (a fork) — written with the
        // row so a session never exists without the history it was created
        // with. The runtime status is not persisted — it derives from the
        // workspace/session dispositions.
        let session_id = uuid::Uuid::new_v4().to_string();
        let (parent_session_id, parent_cut_seq) = match &record.lineage {
            Some(l) => (Some(l.parent_session_id.as_str()), Some(l.cut_seq)),
            None => (None, None),
        };
        tx.execute(
            "INSERT INTO sessions (id, workspace_id, provider, view, provider_session_id, last_error, effort, model, instructions, handoff_context, custom_agent_id, skills, mcp_servers, parent_session_id, parent_cut_seq, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
                record.handoff_context,
                record.custom_agent_id,
                encode_json_vec(&record.skills),
                encode_json_vec(&record.mcp_servers),
                parent_session_id,
                parent_cut_seq,
                created_millis,
            ],
        )?;

        // Insert checkout records for each TrackedRepo.
        for repo in &record.repos {
            Self::insert_worktree(tx, &record.id, repo)?;
        }

        Ok(cleanup)
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

    /// Record the agent-written title for the workspace's work (see the
    /// `set_title` mailbox op). Overwrites unconditionally: the agent may set a
    /// tentative title early and refine it once the purpose is clearer.
    pub fn set_agent_title(&self, id: &str, title: &str) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE workspaces SET title = ?1 WHERE id = ?2",
            rusqlite::params![title, id],
        )?;
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

    /// Record the base branch a tracked repo's work is reviewed against,
    /// identified by subdir. Seeded at spawn from the project's default and
    /// rewritten when `open_pr` targets a different base (`args.base`) — the user
    /// can point the agent at a feature branch mid-session, and everything keyed
    /// on the base (ahead/behind, "rebase onto", the `update-branch` fetch) has to
    /// follow the PR that actually exists rather than the spawn-time guess.
    /// Overwrites unconditionally; re-recording the same base is a no-op write.
    ///
    /// Deliberately leaves `base_sha` alone: that is the immutable fork point the
    /// checkout was cut from, which the diff base reads in preference to this
    /// name, and it stays true whatever the PR targets.
    pub fn set_repo_parent_branch(&self, agent_id: &str, subdir: &str, base: &str) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE worktrees SET parent_branch = ?1 WHERE workspace_id = ?2 AND subdir = ?3",
            rusqlite::params![base, agent_id, subdir],
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

    /// Bind PR `pr_number` to a tracked repo, identified by subdir: the
    /// `git.pr_opened` path, and adoption of a PR opened out of band. The PR is
    /// logged into the checkout's set (`worktree_prs`) and bound in one
    /// transaction, so the sweep watches it from the moment it binds rather
    /// than from its first successful fetch. Overwrites unconditionally — the
    /// latest PR opened is the one we focus.
    ///
    /// `url`/`title` may be empty and `branch` absent when the caller doesn't
    /// know them; an existing row keeps what it had for those, and a new row
    /// starts `open` (a PR just opened is open; an existing row keeps its state,
    /// which only a fetch may move). The binding's snapshot columns are copied
    /// from that row, never kept from the PR being replaced — so a follow-up
    /// bound after a merge can't inherit "merged" (which `resolve_pr_state`
    /// serves with no network, forever), and `pr_snapshot` is never `None` for
    /// a bound PR.
    pub fn set_repo_pr_number(
        &self,
        agent_id: &str,
        subdir: &str,
        pr_number: i64,
        url: &str,
        title: &str,
        branch: Option<&str>,
    ) -> Result<()> {
        let conn = self.db.lock();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO worktree_prs (workspace_id, subdir, number, url, title, state, branch)
             VALUES (?1, ?2, ?3, ?4, ?5, 'open', NULLIF(?6, ''))
             ON CONFLICT(workspace_id, subdir, number) DO UPDATE SET
                    url    = CASE WHEN excluded.url   <> '' THEN excluded.url   ELSE url   END,
                    title  = CASE WHEN excluded.title <> '' THEN excluded.title ELSE title END,
                    branch = COALESCE(excluded.branch, branch)",
            rusqlite::params![agent_id, subdir, pr_number, url, title, branch],
        )?;
        bind_pr(&tx, agent_id, subdir, pr_number)?;
        tx.commit()?;
        Ok(())
    }

    /// One checkout's PR set — every PR it has held, the focused one included —
    /// newest number first: `all_pr_sets` for a single checkout. Read by the
    /// merged binding's forced rescan, which must not re-adopt a PR the set
    /// already holds.
    ///
    /// `mergeable` reads `Unknown` for the same reason it does in `pr_snapshot`:
    /// it isn't persisted, and a stored merge verdict would be stale anyway.
    pub fn repo_pr_history(
        &self,
        agent_id: &str,
        subdir: &str,
    ) -> Result<Vec<crate::github::PrState>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PR_ROW_COLUMNS} FROM worktree_prs
              WHERE workspace_id = ?1 AND subdir = ?2
              ORDER BY number DESC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![agent_id, subdir], |row| pr_row(row, 0))?;
        let mut out = Vec::new();
        for row in rows {
            out.extend(row?);
        }
        Ok(out)
    }

    /// The head branch of the checkout's focused PR while that PR is open, or
    /// `None` (no PR, a settled one, or a branch nobody has learned yet). Once
    /// the user focuses an older PR this can differ from the branch the checkout
    /// is on, so a delegation about the PR names it (see
    /// `supervisor::delegation`). `mergeable` and friends are irrelevant here,
    /// hence one column rather than a `PrState`.
    pub fn focused_pr_branch(&self, agent_id: &str, subdir: &str) -> Result<Option<String>> {
        let conn = self.db.lock();
        let branch = conn
            .query_row(
                "SELECT p.branch FROM worktrees w
                   JOIN worktree_prs p ON p.workspace_id = w.workspace_id
                                      AND p.subdir = w.subdir AND p.number = w.pr_number
                  WHERE w.workspace_id = ?1 AND w.subdir = ?2 AND p.state = 'open'",
                rusqlite::params![agent_id, subdir],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .filter(|b| !b.is_empty());
        Ok(branch)
    }

    /// Every checkout's PR set, for every non-archived workspace, keyed by
    /// `(workspace_id, subdir)` and newest number first, so the PR sweep reads
    /// its working set without a query per checkout. Each set includes the
    /// focused PR.
    ///
    /// `mergeable` reads `Unknown` for the same reason it does in `pr_snapshot`:
    /// it isn't persisted, and a stored merge verdict would be stale anyway.
    pub fn all_pr_sets(
        &self,
    ) -> Result<std::collections::HashMap<(String, String), Vec<crate::github::PrState>>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT workspace_id, subdir, {PR_ROW_COLUMNS} FROM worktree_prs
              WHERE workspace_id IN (SELECT id FROM workspaces WHERE archived_at IS NULL)
              ORDER BY workspace_id, subdir, number DESC"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                pr_row(row, 2)?,
            ))
        })?;
        let mut out: std::collections::HashMap<_, Vec<_>> = std::collections::HashMap::new();
        for row in rows {
            let (workspace_id, subdir, pr) = row?;
            if let Some(pr) = pr {
                out.entry((workspace_id, subdir)).or_default().push(pr);
            }
        }
        Ok(out)
    }

    /// Persist a successful fetch of one of the checkout's PRs: its set row
    /// (url / title / state / branch, and GitHub's own lifecycle times), plus —
    /// only when it is the bound PR — the binding's snapshot columns, copied
    /// from that row. One write per fetch keeps the database the durable source
    /// of truth the UI renders from when GitHub or the checkout is unavailable.
    ///
    /// A refresh never moves the binding. A fetch is a network read, and one in
    /// flight across a `set_focused_pr` (or a `git.pr_opened` bind) would
    /// otherwise put the old focus back when it lands; and the sweep reads every
    /// open PR of a checkout, so the last one fetched would steal it every tick.
    /// Binding is [`Self::set_repo_pr_number`] / [`Self::set_focused_pr`] only.
    pub fn record_repo_pr(
        &self,
        agent_id: &str,
        subdir: &str,
        pr: &crate::github::PrState,
    ) -> Result<()> {
        let conn = self.db.lock();
        let tx = conn.unchecked_transaction()?;
        upsert_pr_row(&tx, agent_id, subdir, pr)?;
        tx.execute(
            &format!(
                "UPDATE worktrees SET {SNAPSHOT_FROM_ROW}
                   FROM worktree_prs p
                  WHERE worktrees.workspace_id = ?1 AND worktrees.subdir = ?2
                    AND worktrees.pr_number = ?3
                    AND p.workspace_id = ?1 AND p.subdir = ?2 AND p.number = ?3"
            ),
            rusqlite::params![agent_id, subdir, pr.number as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Make `number` — already one of the checkout's PRs — its focused PR: the
    /// binding every single-PR reader (badge, panel header, `get_pr_live`)
    /// follows. Returns the PR as now bound; `Ok(None)` when `number` isn't in
    /// this checkout's set (nothing is written).
    pub fn set_focused_pr(
        &self,
        agent_id: &str,
        subdir: &str,
        number: u32,
    ) -> Result<Option<crate::github::PrState>> {
        let conn = self.db.lock();
        let tx = conn.unchecked_transaction()?;
        let pr = tx
            .query_row(
                &format!(
                    "SELECT {PR_ROW_COLUMNS} FROM worktree_prs
                      WHERE workspace_id = ?1 AND subdir = ?2 AND number = ?3"
                ),
                rusqlite::params![agent_id, subdir, number as i64],
                |row| pr_row(row, 0),
            )
            .optional()?
            .flatten();
        let Some(pr) = pr else {
            return Ok(None);
        };
        bind_pr(&tx, agent_id, subdir, number as i64)?;
        tx.commit()?;
        Ok(Some(pr))
    }

    pub fn append_tracked_repo(&self, agent_id: &str, repo: TrackedRepo) -> Result<()> {
        let conn = self.db.lock();
        Self::insert_worktree(&conn, agent_id, &repo)?;
        Ok(())
    }

    /// Persist the agent's session id. Used for Codex, whose thread id
    /// is assigned by the CLI and captured from its first turn's events
    /// (Claude's id is generated up front, so it never changes here).
    ///
    /// Returns whether the current session took it. An id one of the
    /// workspace's superseded sessions already has is refused: a provider
    /// session belongs to one session, and a capture that finds it (agy's
    /// checkout → conversation map, before a rewound session's first turn) has
    /// found the conversation the current session replaced.
    pub fn set_agent_session_id(&self, id: &str, session_id: &str) -> Result<bool> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        let taken = conn.execute(
            "UPDATE sessions SET provider_session_id = ?1
             WHERE workspace_id = ?2 AND superseded_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM sessions
                                WHERE workspace_id = ?2 AND superseded_at IS NOT NULL
                                  AND provider_session_id = ?1)",
            rusqlite::params![session_id, id],
        )?;
        Ok(taken > 0)
    }

    pub fn update_agent_view(&self, id: &str, view: AgentView) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE sessions SET view = ?1 WHERE workspace_id = ?2 AND superseded_at IS NULL",
            rusqlite::params![view_to_str(&view), id],
        )?;
        Ok(())
    }

    /// Update the session's reasoning-effort level mid-conversation. Unlike the
    /// spawn-time value, this is user-changeable (see
    /// `Supervisor::set_agent_effort`): claude re-applies it on the next
    /// `--resume`, while per-turn agents read it on their next turn. `None`
    /// clears the selection, falling back to the provider's default.
    pub fn update_agent_effort(&self, id: &str, effort: Option<&str>) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE sessions SET effort = ?1 WHERE workspace_id = ?2 AND superseded_at IS NULL",
            rusqlite::params![effort, id],
        )?;
        Ok(())
    }

    /// Update the session's model mid-conversation. Like `update_agent_effort`,
    /// this is user-changeable after spawn (see `Supervisor::set_agent_model`):
    /// claude re-applies it on the next `--resume`, while per-turn agents read
    /// it on their next turn. `None` clears the selection (provider default).
    pub fn update_agent_model(&self, id: &str, model: Option<&str>) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE sessions SET model = ?1 WHERE workspace_id = ?2 AND superseded_at IS NULL",
            rusqlite::params![model, id],
        )?;
        Ok(())
    }

    /// Drop the session's recorded failure, so the agent rests as idle again.
    pub fn clear_agent_error(&self, id: &str) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE sessions SET last_error = NULL
             WHERE workspace_id = ?1 AND superseded_at IS NULL",
            [id],
        )?;
        Ok(())
    }

    /// Restamp the workspace's provider account (`None` for the default).
    /// Every later launch reads it back, and so do usage attribution and the
    /// account-removal gate (see `Supervisor::switch_account`).
    pub fn update_agent_account(&self, id: &str, account: Option<&str>) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE workspaces SET provider_account = ?1 WHERE id = ?2",
            rusqlite::params![account, id],
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

    /// Mark an agent as archived — the first of archive's two writes, made
    /// the moment the user asks, before any cleanup. Stamping `archived_at`
    /// is what hides the agent from every workspace read and derives its
    /// status to `Stopped` (so resume-on-launch ignores it); there is no
    /// status column to flip. `setup_completed_at` is cleared too — restore
    /// recreates the checkout from scratch, so node_modules etc. won't be
    /// there. The completion trace is reset so a re-archive after restore
    /// starts a fresh measurement.
    pub fn begin_archive(&self, id: &str) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE workspaces SET archived_at = ?1, setup_completed_at = NULL,
                    archive_completed_at = NULL, archive_error = NULL
             WHERE id = ?2",
            rusqlite::params![now_millis(), id],
        )?;
        Ok(())
    }

    /// Record the outcome of archive's cleanup — the second write. Stores the
    /// snapshot of every tracked repo (what restore rebuilds from), stamps
    /// `archive_completed_at`, and keeps `error` (the joined cleanup failures,
    /// `None` when clean) as the audit trail. `archived_at` is untouched, so
    /// the agent stays archived whatever happened here.
    pub fn finish_archive(
        &self,
        id: &str,
        snapshots: &[ArchivedRepoSnapshot],
        error: Option<&str>,
    ) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;

        for snap in snapshots {
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

        conn.execute(
            "UPDATE workspaces SET archive_completed_at = ?1, archive_error = ?2 WHERE id = ?3",
            rusqlite::params![now_millis(), error, id],
        )?;
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
            "UPDATE workspaces SET archived_at = NULL, stopped_at = NULL,
                    archive_completed_at = NULL, archive_error = NULL
             WHERE id = ?1",
            [id],
        )?;

        // Update checkout records with new branch info and clear snapshot fields.
        // `adopted_checkout` is dropped with them: restore always provisions an
        // agent-owned clone at the derived path (see `restore_agent` in
        // `supervisor::disposition`), so a row still naming the adopted tree
        // would point every reader away from the checkout that was just built.
        for repo in &repos {
            conn.execute(
                "UPDATE worktrees SET branch = ?1, parent_branch = ?2,
                        branch_tip_sha = NULL, parent_branch_sha = NULL,
                        diff_additions = 0, diff_deletions = 0,
                        adopted_checkout = NULL
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

    /// Key-name facts for the per-agent env note
    /// ([`crate::instructions::env_awareness_note`]): `(shared, unshared)`,
    /// where `shared` are the keys the user shares into app-run processes and
    /// `unshared` are the keys discovered in the repo's env files (`.env` plus
    /// `.env.example`/`.env.sample`) but not shared. Names only — values never
    /// leave `run_env`'s resolution paths. Never errors: missing files or
    /// config simply yield fewer (or no) names.
    pub fn run_env_key_names(
        &self,
        project_id: &str,
        repo_path: &std::path::Path,
    ) -> (Vec<String>, Vec<String>) {
        let doc = {
            let conn = self.db.lock();
            crate::run_env::load_doc(&conn, project_id)
        };
        let shared: Vec<String> = doc
            .vars
            .iter()
            .filter(|v| v.shared)
            .map(|v| v.key.clone())
            .collect();
        let discovered = crate::run_env::discover_env_keys(repo_path);
        let mut unshared: Vec<String> = Vec::new();
        for key in discovered
            .env
            .iter()
            .map(|e| e.key.as_str())
            .chain(discovered.declared.iter().map(String::as_str))
        {
            if !shared.iter().any(|s| s == key) && !unshared.iter().any(|s| s == key) {
                unshared.push(key.to_string());
            }
        }
        (shared, unshared)
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
        let tx = conn.unchecked_transaction()?;
        // Cascades to the workspace's sessions and worktrees, once any fork
        // still inheriting from those sessions is detached; the transcript
        // rows are in another file and go once this has committed.
        let doomed = [id.to_string()];
        let cleanup = sessions::TranscriptCleanup {
            trims: lineage::detach_children(&tx, &doomed)?,
            sessions: sessions::session_ids_for_workspaces(&tx, &doomed)?,
        };
        tx.execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
        tx.commit()?;
        cleanup.apply(&conn);
        Ok(())
    }
}

/// The `worktree_prs` columns [`pr_row`] reads, in its order.
const PR_ROW_COLUMNS: &str = "number, url, title, state, opened_at, merged_at, branch";

/// One `worktree_prs` row (selected as [`PR_ROW_COLUMNS`] from column `at`) as
/// a `PrState`. `None` for an unparseable state: a row we can't render
/// honestly is skipped rather than given a fabricated status, matching
/// `pr_snapshot`.
fn pr_row(row: &rusqlite::Row, at: usize) -> rusqlite::Result<Option<crate::github::PrState>> {
    let state: String = row.get(at + 3)?;
    let Some(state) = crate::github::PrStatus::parse(&state) else {
        return Ok(None);
    };
    Ok(Some(crate::github::PrState {
        number: row.get::<_, i64>(at)? as u32,
        url: row.get(at + 1)?,
        title: row.get(at + 2)?,
        state,
        mergeable: crate::github::MergeableState::Unknown,
        opened_at: row.get(at + 4)?,
        merged_at: row.get(at + 5)?,
        branch: row.get(at + 6)?,
    }))
}

/// The binding's snapshot columns, set from a `worktree_prs` row aliased `p`.
const SNAPSHOT_FROM_ROW: &str = "pr_url = p.url, pr_title = p.title, pr_state = p.state, \
     pr_opened_at = p.opened_at, pr_merged_at = p.merged_at";

/// Bind PR `number` — which must already have a row in the checkout's set — as
/// the checkout's focused PR, with the snapshot columns copied from that row in
/// the same statement, so the binding never shows one PR's number with
/// another's title, state or times.
fn bind_pr(conn: &rusqlite::Connection, agent_id: &str, subdir: &str, number: i64) -> Result<()> {
    conn.execute(
        &format!(
            "UPDATE worktrees SET pr_number = p.number, {SNAPSHOT_FROM_ROW}
               FROM worktree_prs p
              WHERE worktrees.workspace_id = ?1 AND worktrees.subdir = ?2
                AND p.workspace_id = ?1 AND p.subdir = ?2 AND p.number = ?3"
        ),
        rusqlite::params![agent_id, subdir, number],
    )?;
    Ok(())
}

/// Upsert one PR into its checkout's log (spec: every PR a checkout ever held).
/// Upserted by number so a PR's row tracks its latest state and times, and a
/// re-bound follow-up adds a row instead of overwriting the one it replaces.
fn upsert_pr_row(
    conn: &rusqlite::Connection,
    agent_id: &str,
    subdir: &str,
    pr: &crate::github::PrState,
) -> Result<()> {
    conn.execute(
        "INSERT INTO worktree_prs
                (workspace_id, subdir, number, url, title, state, opened_at, merged_at, branch)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(workspace_id, subdir, number) DO UPDATE SET
                url       = excluded.url,
                title     = excluded.title,
                state     = excluded.state,
                -- Times and branch COALESCE: a payload that omits one must not
                -- erase an earlier-observed value. Identity is the PK here, so
                -- there's no cross-PR bleed to guard against.
                opened_at = COALESCE(excluded.opened_at, opened_at),
                merged_at = COALESCE(excluded.merged_at, merged_at),
                branch    = COALESCE(excluded.branch, branch)",
        rusqlite::params![
            agent_id,
            subdir,
            pr.number as i64,
            pr.url,
            pr.title,
            pr.state.as_str(),
            pr.opened_at,
            pr.merged_at,
            pr.branch,
        ],
    )?;
    Ok(())
}

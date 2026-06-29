//! Workflows backend — persistence + git side-effects for the workflow builder
//! and run engine.
//!
//! A workflow is a named, ordered chain of steps (stored as a JSON blob, edited
//! and saved whole by the builder). A *run* is one execution: the renderer holds
//! the live orchestration state and upserts the whole run/step rows on each
//! transition, so resume always reads a consistent snapshot.
//!
//! The orchestration engine drives runs from the frontend, but the renderer
//! can't run git — so the `workflow_*` git commands here perform the side-effects
//! a run needs: keeping `.quorum/` out of commits, ferrying handoff notes between
//! per-step worktrees, the step-boundary commit, gate probes, and the final push
//! + PR. Worktree paths are resolved server-side from `(agent_id, subdir)` via
//! `crate::workspace::repo_worktree_path`, so callers pass the ids from the
//! AgentRecord and never construct paths themselves.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row};
use serde_json::{json, Value};

type Db = Arc<Mutex<Connection>>;

/// Epoch milliseconds, matching the core schema's timestamp convention.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ───────────────────────────── workflow definitions ─────────────────────────

/// Every workflow, newest-edited first. `steps` is parsed back into a JSON array
/// so the frontend receives real objects, not a string.
#[tauri::command]
pub async fn workflow_list(db: tauri::State<'_, Db>) -> Result<Value, String> {
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, hue, steps, run_count, created_at, updated_at \
             FROM workflow ORDER BY updated_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            let steps_str: String = r.get(4)?;
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "description": r.get::<_, String>(2)?,
                "hue": r.get::<_, i64>(3)?,
                "steps": serde_json::from_str::<Value>(&steps_str).unwrap_or_else(|_| json!([])),
                "run_count": r.get::<_, i64>(5)?,
                "created_at": r.get::<_, i64>(6)?,
                "updated_at": r.get::<_, i64>(7)?,
            }))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(json!(out))
}

/// Upsert the whole workflow. On conflict only the mutable columns are
/// overwritten, so created_at and run_count survive an edit.
#[tauri::command]
pub async fn workflow_save(workflow: Value, db: tauri::State<'_, Db>) -> Result<Value, String> {
    let w = &workflow;
    let id = w.get("id").and_then(|v| v.as_str()).ok_or("workflow.id required")?;
    let name = w.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let desc = w.get("description").and_then(|v| v.as_str()).unwrap_or("");
    let hue = w.get("hue").and_then(|v| v.as_i64()).unwrap_or(265);
    let steps = w.get("steps").cloned().unwrap_or_else(|| json!([]));
    let steps_str = serde_json::to_string(&steps).map_err(|e| e.to_string())?;
    let now = now_ms();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO workflow \
           (id, name, description, hue, steps, run_count, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6) \
         ON CONFLICT(id) DO UPDATE SET \
           name = excluded.name, \
           description = excluded.description, \
           hue = excluded.hue, \
           steps = excluded.steps, \
           updated_at = excluded.updated_at",
        (id, name, desc, hue, steps_str, now),
    )
    .map_err(|e| e.to_string())?;
    Ok(json!({ "id": id }))
}

/// Delete a workflow by id.
#[tauri::command]
pub async fn workflow_delete(id: String, db: tauri::State<'_, Db>) -> Result<Value, String> {
    let conn = db.lock();
    conn.execute("DELETE FROM workflow WHERE id = ?1", (&id,))
        .map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true }))
}

// ───────────────────────────── run persistence ──────────────────────────────

fn run_row_to_json(r: &Row) -> rusqlite::Result<Value> {
    let steps_str: String = r.get("steps_snapshot")?;
    Ok(json!({
        "id": r.get::<_, String>("id")?,
        "workflow_id": r.get::<_, String>("workflow_id")?,
        "name": r.get::<_, String>("name")?,
        "steps_snapshot": serde_json::from_str::<Value>(&steps_str).unwrap_or_else(|_| json!([])),
        "task": r.get::<_, String>("task")?,
        "project_id": r.get::<_, String>("project_id")?,
        "repo_path": r.get::<_, String>("repo_path")?,
        "run_dir": r.get::<_, String>("run_dir")?,
        "branch": r.get::<_, String>("branch")?,
        "base_sha": r.get::<_, String>("base_sha")?,
        "status": r.get::<_, String>("status")?,
        "current_step_id": r.get::<_, Option<String>>("current_step_id")?,
        "current_iter": r.get::<_, i64>("current_iter")?,
        "created_at": r.get::<_, i64>("created_at")?,
        "updated_at": r.get::<_, i64>("updated_at")?,
    }))
}

fn step_row_to_json(r: &Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, String>("id")?,
        "run_id": r.get::<_, String>("run_id")?,
        "step_id": r.get::<_, String>("step_id")?,
        "iteration": r.get::<_, i64>("iteration")?,
        "agent_id": r.get::<_, Option<String>>("agent_id")?,
        "status": r.get::<_, String>("status")?,
        "advance_mode": r.get::<_, String>("advance_mode")?,
        "head_start": r.get::<_, Option<String>>("head_start")?,
        "head_end": r.get::<_, Option<String>>("head_end")?,
        "summary": r.get::<_, Option<String>>("summary")?,
        "started_at": r.get::<_, Option<i64>>("started_at")?,
        "ended_at": r.get::<_, Option<i64>>("ended_at")?,
    }))
}

/// Upsert the whole run. created_at survives an update (it's not in the conflict
/// SET); updated_at is stamped now.
#[tauri::command]
pub async fn workflow_save_run(run: Value, db: tauri::State<'_, Db>) -> Result<Value, String> {
    let r = &run;
    let id = r.get("id").and_then(|v| v.as_str()).ok_or("run.id required")?;
    let s = |k: &str| r.get(k).and_then(|v| v.as_str());
    let steps = r.get("steps_snapshot").cloned().unwrap_or_else(|| json!([]));
    let steps_str = serde_json::to_string(&steps).map_err(|e| e.to_string())?;
    let now = now_ms();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO workflow_run \
           (id, workflow_id, name, steps_snapshot, task, project_id, repo_path, \
            run_dir, branch, base_sha, status, current_step_id, current_iter, \
            created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14) \
         ON CONFLICT(id) DO UPDATE SET \
           workflow_id=excluded.workflow_id, name=excluded.name, \
           steps_snapshot=excluded.steps_snapshot, task=excluded.task, \
           project_id=excluded.project_id, repo_path=excluded.repo_path, \
           run_dir=excluded.run_dir, branch=excluded.branch, base_sha=excluded.base_sha, \
           status=excluded.status, current_step_id=excluded.current_step_id, \
           current_iter=excluded.current_iter, updated_at=excluded.updated_at",
        (
            id,
            s("workflow_id").unwrap_or(""),
            s("name").unwrap_or(""),
            steps_str,
            s("task").unwrap_or(""),
            s("project_id").unwrap_or(""),
            s("repo_path").unwrap_or(""),
            s("run_dir").unwrap_or(""),
            s("branch").unwrap_or(""),
            s("base_sha").unwrap_or(""),
            s("status").unwrap_or("pending"),
            s("current_step_id"),
            r.get("current_iter").and_then(|v| v.as_i64()).unwrap_or(0),
            now,
        ),
    )
    .map_err(|e| e.to_string())?;
    Ok(json!({ "id": id }))
}

/// A run plus its step executions (creation order), or null.
#[tauri::command]
pub async fn workflow_get_run(id: String, db: tauri::State<'_, Db>) -> Result<Value, String> {
    let conn = db.lock();
    let run = conn
        .query_row("SELECT * FROM workflow_run WHERE id = ?1", (&id,), run_row_to_json)
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(run) = run else { return Ok(Value::Null) };
    let mut stmt = conn
        .prepare("SELECT * FROM workflow_run_step WHERE run_id = ?1 ORDER BY rowid")
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map((&id,), step_row_to_json).map_err(|e| e.to_string())?;
    let mut steps = Vec::new();
    for row in rows {
        steps.push(row.map_err(|e| e.to_string())?);
    }
    Ok(json!({ "run": run, "steps": steps }))
}

/// All runs, newest-updated first (resume scan + history view).
#[tauri::command]
pub async fn workflow_list_runs(db: tauri::State<'_, Db>) -> Result<Value, String> {
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT * FROM workflow_run ORDER BY updated_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], run_row_to_json).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(json!(out))
}

/// Upsert a step execution. Identity columns (run_id, step_id, iteration) are
/// immutable, so they're not in the conflict SET.
#[tauri::command]
pub async fn workflow_save_run_step(step: Value, db: tauri::State<'_, Db>) -> Result<Value, String> {
    let st = &step;
    let id = st.get("id").and_then(|v| v.as_str()).ok_or("step.id required")?;
    let s = |k: &str| st.get(k).and_then(|v| v.as_str());
    let i = |k: &str| st.get(k).and_then(|v| v.as_i64());
    let conn = db.lock();
    conn.execute(
        "INSERT INTO workflow_run_step \
           (id, run_id, step_id, iteration, agent_id, status, advance_mode, \
            head_start, head_end, summary, started_at, ended_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) \
         ON CONFLICT(id) DO UPDATE SET \
           agent_id=excluded.agent_id, status=excluded.status, \
           advance_mode=excluded.advance_mode, head_start=excluded.head_start, \
           head_end=excluded.head_end, summary=excluded.summary, \
           started_at=excluded.started_at, ended_at=excluded.ended_at",
        (
            id,
            s("run_id").unwrap_or(""),
            s("step_id").unwrap_or(""),
            i("iteration").unwrap_or(0),
            s("agent_id"),
            s("status").unwrap_or("pending"),
            s("advance_mode").unwrap_or("signal"),
            s("head_start"),
            s("head_end"),
            s("summary"),
            i("started_at"),
            i("ended_at"),
        ),
    )
    .map_err(|e| e.to_string())?;
    Ok(json!({ "id": id }))
}

/// Delete a run and its steps.
#[tauri::command]
pub async fn workflow_delete_run(id: String, db: tauri::State<'_, Db>) -> Result<Value, String> {
    let conn = db.lock();
    conn.execute("DELETE FROM workflow_run_step WHERE run_id = ?1", (&id,))
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM workflow_run WHERE id = ?1", (&id,))
        .map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true }))
}

// ───────────────────────────── git side-effects ─────────────────────────────

/// Name of the gitignored, ferried coordination dir inside each step's cwd.
const HANDOFF_DIR: &str = ".quorum";

fn run_git(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))
}

/// Run git and require success, returning trimmed stdout.
fn git_ok(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = run_git(dir, args)?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Resolve a step worktree from `(agent_id, subdir)`.
fn worktree(agent_id: &str, subdir: &str) -> Result<PathBuf, String> {
    crate::workspace::repo_worktree_path(agent_id, subdir).map_err(|e| e.to_string())
}

/// Recursively copy `src` into `dst` (no-op if `src` is absent).
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Ensure `.quorum/` is excluded locally (never committed, never pushed) for this
/// repo and all its worktrees. Idempotent.
#[tauri::command]
pub async fn workflow_prepare_repo(repo_path: String) -> Result<Value, String> {
    let repo = PathBuf::from(repo_path);
    let common = git_ok(&repo, &["rev-parse", "--git-common-dir"])?;
    // --git-common-dir may be relative to the repo.
    let common = if Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        repo.join(common)
    };
    let info = common.join("info");
    std::fs::create_dir_all(&info).map_err(|e| e.to_string())?;
    let exclude = info.join("exclude");
    let body = std::fs::read_to_string(&exclude).unwrap_or_default();
    let entry = format!("{HANDOFF_DIR}/");
    if !body.lines().any(|l| l.trim() == entry) {
        let mut next = body;
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(&entry);
        next.push('\n');
        std::fs::write(&exclude, next).map_err(|e| e.to_string())?;
    }
    Ok(json!({ "ok": true }))
}

/// Copy the previous step's `.quorum/` into the next step's worktree, so handoff
/// notes accumulate forward across forked worktrees.
#[tauri::command]
pub async fn workflow_ferry_notes(
    from_agent_id: String,
    from_subdir: String,
    to_agent_id: String,
    to_subdir: String,
) -> Result<Value, String> {
    let from = worktree(&from_agent_id, &from_subdir)?;
    let to = worktree(&to_agent_id, &to_subdir)?;
    copy_dir(&from.join(HANDOFF_DIR), &to.join(HANDOFF_DIR)).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true }))
}

/// Stage everything and commit if the tree is dirty; always return the resulting
/// HEAD so the engine can fork the next step from it.
#[tauri::command]
pub async fn workflow_boundary_commit(
    agent_id: String,
    subdir: String,
    message: String,
) -> Result<Value, String> {
    let wt = worktree(&agent_id, &subdir)?;
    git_ok(&wt, &["add", "-A"])?;
    // `diff --cached --quiet` exits 1 when there is something staged.
    let clean = run_git(&wt, &["diff", "--cached", "--quiet"])?.status.success();
    let committed = !clean;
    if committed {
        git_ok(&wt, &["commit", "-m", &message])?;
    }
    let head = git_ok(&wt, &["rev-parse", "HEAD"])?;
    Ok(json!({ "head": head, "committed": committed }))
}

/// Current HEAD of a step worktree (head_start snapshot + the "did the agent
/// commit?" gate).
#[tauri::command]
pub async fn workflow_head_sha(agent_id: String, subdir: String) -> Result<Value, String> {
    let wt = worktree(&agent_id, &subdir)?;
    Ok(json!({ "head": git_ok(&wt, &["rev-parse", "HEAD"])? }))
}

/// Gate probe for the "file written" advance mode. `path` is relative to the
/// worktree (e.g. ".quorum/PLAN.md").
#[tauri::command]
pub async fn workflow_file_exists(
    agent_id: String,
    subdir: String,
    path: String,
) -> Result<Value, String> {
    use std::path::Component;
    let wt = worktree(&agent_id, &subdir)?;
    // The gate only ever checks a file *inside* the worktree. The path comes
    // from the workflow's `artifact` field (renderer-supplied), so reject
    // absolute paths and `..` traversal — otherwise `wt.join(path)` would let a
    // crafted artifact name probe the filesystem outside the worktree.
    let rel = Path::new(&path);
    if rel.is_absolute() || rel.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("artifact path must be relative to the worktree".into());
    }
    Ok(json!({ "exists": wt.join(rel).exists() }))
}

/// Push the run's final HEAD to its branch, then best-effort open a PR. Pushing
/// detached `HEAD:refs/heads/<branch>` needs no local branch, so it never
/// collides with a worktree's checkout.
#[tauri::command]
pub async fn workflow_finalize(
    agent_id: String,
    subdir: String,
    branch: String,
    base_branch: Option<String>,
    title: Option<String>,
    body: Option<String>,
) -> Result<Value, String> {
    let wt = worktree(&agent_id, &subdir)?;
    git_ok(&wt, &["push", "origin", &format!("HEAD:refs/heads/{branch}")])?;
    // PR is best-effort: a missing/unauthed gh, or an existing PR, must not fail
    // an otherwise-complete run.
    let base = base_branch.as_deref().unwrap_or("main");
    let title = title.as_deref().unwrap_or("Workflow run");
    let body = body.as_deref().unwrap_or("");
    let pr = run_git_gh(&wt, base, &branch, title, body);
    Ok(json!({ "pushed": true, "branch": branch, "pr": pr.0, "pr_error": pr.1 }))
}

/// `gh pr create`, returning `(url, error)` — at most one is set.
fn run_git_gh(
    dir: &Path,
    base: &str,
    branch: &str,
    title: &str,
    body: &str,
) -> (Option<String>, Option<String>) {
    let out = Command::new("gh")
        .current_dir(dir)
        .args([
            "pr", "create", "--base", base, "--head", branch, "--title", title, "--body", body,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            (Some(String::from_utf8_lossy(&o.stdout).trim().to_string()), None)
        }
        Ok(o) => (None, Some(String::from_utf8_lossy(&o.stderr).trim().to_string())),
        Err(e) => (None, Some(e.to_string())),
    }
}

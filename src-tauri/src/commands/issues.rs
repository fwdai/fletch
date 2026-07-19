//! The generalized issue-tracker commands: the merged per-repo issue list
//! (Home inbox + composer issue picker) and the Linear connection lifecycle.

use std::sync::Arc;

use tauri::State;

use crate::error::Result;
use crate::issues::TrackerIssue;
use crate::linear;
use crate::supervisor::Supervisor;

use super::files::expand_tilde;

type Db = Arc<parking_lot::Mutex<rusqlite::Connection>>;

/// Plain `settings` key caching the connected Linear user's display name, so
/// status renders without a network round-trip. Not a secret — the key
/// itself lives in `crate::secrets` under [`linear::TOKEN_SETTING`].
const LINEAR_USER_SETTING: &str = "linear_user";

/// Linear connection state. Mirrors `GhStatus`'s shape (minus the legacy
/// `installed` flag): `authenticated` gates Linear affordances app-wide.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LinearStatus {
    pub authenticated: bool,
    pub user: Option<String>,
}

/// List open issues for a repo across every configured tracker source —
/// GitHub via the repo's origin, Linear via the project's configured team
/// (`linear.team_id` in `project_settings`, passed by the caller). Sources
/// degrade quietly to nothing on their own failures, so the list never
/// errors for a partly-connected setup. Capped at 30, matching the old
/// GitHub-only `list_repo_issues`.
#[tauri::command]
pub async fn list_tracker_issues(
    repo_path: String,
    linear_team_id: Option<String>,
) -> Result<Vec<TrackerIssue>> {
    Ok(crate::issues::issue_list(&expand_tilde(&repo_path), linear_team_id.as_deref(), 30).await)
}

/// Re-tag a running agent with the issue it's working — the composer's issue
/// picker in an existing chat. A spawn-time ref only exists for drafts;
/// picking mid-session must land the same way, so this persists the ref to
/// the workspace row (the durable source across restarts) and updates the
/// live registry the git dispatcher reads at `open_pr` time, so the PR's
/// closing trailer follows the latest pick. `None`/blank clears the tag.
#[tauri::command]
pub fn set_agent_issue_ref(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    issue_ref: Option<String>,
) -> Result<()> {
    let issue_ref = issue_ref.filter(|s| !s.trim().is_empty());
    supervisor
        .workspace
        .update_agent_issue_ref(&agent_id, issue_ref.as_deref())?;
    crate::issues::set_live_issue_ref(&agent_id, issue_ref);
    Ok(())
}

/// The Linear connection state: whether an API key is stored, and who it
/// belongs to (cached at connect time).
#[tauri::command]
pub fn linear_status(db: State<'_, Db>) -> Result<LinearStatus> {
    let authenticated = crate::linear::client::token().is_some();
    let user = authenticated
        .then(|| crate::database::get_setting(&db.lock(), LINEAR_USER_SETTING))
        .flatten();
    Ok(LinearStatus {
        authenticated,
        user,
    })
}

/// Connect Linear with a personal API key: validate it against the API
/// *before* persisting (a bad paste must fail loudly, not store a dud), then
/// store it via `crate::secrets` and mirror it in-process.
#[tauri::command]
pub async fn linear_connect(db: State<'_, Db>, api_key: String) -> Result<LinearStatus> {
    let key = api_key.trim().to_string();
    let user = linear::viewer(key.clone()).await?;
    {
        let conn = db.lock();
        crate::secrets::set(&conn, linear::TOKEN_SETTING, &key)?;
        crate::database::set_setting(&conn, LINEAR_USER_SETTING, &user)?;
    }
    linear::set_token(Some(key));
    Ok(LinearStatus {
        authenticated: true,
        user: Some(user).filter(|u| !u.is_empty()),
    })
}

/// Drop the stored Linear API key — mirrors `github_disconnect`.
#[tauri::command]
pub fn linear_disconnect(db: State<'_, Db>) -> Result<()> {
    {
        let conn = db.lock();
        crate::secrets::delete(&conn, linear::TOKEN_SETTING)?;
        crate::database::set_setting(&conn, LINEAR_USER_SETTING, "")?;
    }
    linear::set_token(None);
    Ok(())
}

/// The Linear workspace's teams, for the per-project team picker in Project
/// Settings.
#[tauri::command]
pub async fn linear_list_teams() -> Result<Vec<linear::LinearTeam>> {
    linear::teams().await
}

//! A project's own settings file, kept in its primary repo the way editors and
//! agents keep theirs: `.fletch/settings.json` (committed, the team's) and
//! `.fletch/settings.local.json` (one person's). JSON with comments.
//!
//! Two rules make a file in a repository safe to obey:
//!
//! - **It is read from the user's repository on this machine, never from an
//!   agent's checkout.** An agent can write its own checkout; were that copy
//!   read, an agent could widen its own sandbox. A change an agent makes to the
//!   file takes effect only once the user has it in their repository (a merge),
//!   in a diff they can review. [`read_files`] refuses a path under the agents'
//!   workspaces root outright.
//! - **What a file would let an agent do beyond the app's own settings applies
//!   only once the user approved it.** A cloned repository can ship a file;
//!   its host additions wait, listed as pending, until the user approves them
//!   ([`approve`]), and an addition the user has not approved yet waits again.
//!   Approval is stored host-side under a key no client may write
//!   ([`APPROVED_EGRESS_KEY`] is not on `commands::project_settings`' list).
//!
//! Sections today: `egress.allow`, host patterns a project's boxes may reach on
//! top of the app's list (`sandbox::boxes::registry::egress_policy`). Both
//! files' additions need approval alike for now.
//!
//! Like the box settings it feeds, the state is mirrored in-process — the box
//! registry has no database handle — and kept current by [`watch`]: a poll of
//! the files' size and mtime that re-reads what changed, updates the project's
//! running boxes and emits [`EVENT_CHANGED`].

mod file;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use parking_lot::RwLock;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::host::Sink;

pub use file::{parse, FileReport, Parsed, DIR, LOCAL_FILE, SHARED_FILE};
use file::{read_files, stamp, Read, Stamp};

/// Project setting: a JSON array of the host patterns the user approved from
/// the files. Host-only.
pub const APPROVED_EGRESS_KEY: &str = "settings_file.approved_egress";
/// `project_file:changed { project_id }`: what the files say, or what of it is
/// approved, changed.
pub const EVENT_CHANGED: &str = "project_file:changed";

/// How often [`watch`] looks at the files.
const POLL: Duration = Duration::from_secs(3);

/// The files' egress additions, split by approval.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct EgressStatus {
    pub applied: Vec<String>,
    pub pending: Vec<String>,
}

/// `project_file_status`: what a project's files say and what of it applies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProjectFileStatus {
    pub project_id: String,
    /// The primary repo the files are read from; `None` for a project without
    /// one.
    pub repo_path: Option<String>,
    /// Why the repository was not read at all.
    pub error: Option<String>,
    pub files: Vec<FileReport>,
    pub egress: EgressStatus,
}

// ---------------------------------------------------------------------------
// The mirror
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Entry {
    repo_path: Option<PathBuf>,
    read: Read,
    approved: Vec<String>,
    stamp: Stamp,
}

impl Entry {
    fn status(&self, project_id: &str) -> ProjectFileStatus {
        let (applied, pending) = self
            .read
            .egress_allow
            .iter()
            .cloned()
            .partition(|p| self.approved.contains(p));
        ProjectFileStatus {
            project_id: project_id.to_string(),
            repo_path: self
                .repo_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            error: self.read.error.clone(),
            files: self.read.files.clone(),
            egress: EgressStatus { applied, pending },
        }
    }
}

static STATE: RwLock<Option<HashMap<String, Entry>>> = RwLock::new(None);

/// The egress additions of `project_id`'s files that the user approved: what
/// `egress_policy` adds for them. Empty for a project never read.
pub fn applied_egress(project_id: &str) -> Vec<String> {
    STATE
        .read()
        .as_ref()
        .and_then(|m| m.get(project_id))
        .map(|e| e.status(project_id).egress.applied)
        .unwrap_or_default()
}

/// What the files of `project_id` say, as last read.
pub fn status(project_id: &str) -> ProjectFileStatus {
    STATE
        .read()
        .as_ref()
        .and_then(|m| m.get(project_id))
        .map(|e| e.status(project_id))
        .unwrap_or_else(|| ProjectFileStatus {
            project_id: project_id.to_string(),
            ..ProjectFileStatus::default()
        })
}

/// Store `entry` for `project_id`; whether anything a reader sees changed —
/// the status, or the applied additions the running boxes enforce.
fn store(project_id: &str, entry: Entry) -> (bool, bool) {
    let mut state = STATE.write();
    let map = state.get_or_insert_with(HashMap::new);
    let before = map.get(project_id).map(|e| e.status(project_id));
    let after = entry.status(project_id);
    map.insert(project_id.to_string(), entry);
    let changed = before.as_ref() != Some(&after);
    let applied_changed = before.map(|b| b.egress.applied) != Some(after.egress.applied);
    (changed, applied_changed)
}

fn agents_root() -> Option<PathBuf> {
    crate::workspace::checkouts_root().ok()
}

fn load_approved(conn: &Connection, project_id: &str) -> Vec<String> {
    conn.query_row(
        "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
        [project_id, APPROVED_EGRESS_KEY],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
    .unwrap_or_default()
}

/// Re-read `project_id`'s repository and approvals into the mirror, updating
/// its running boxes when what they enforce changed. Returns the status and
/// whether it changed.
pub fn refresh(conn: &Connection, project_id: &str) -> (ProjectFileStatus, bool) {
    let repo_path = crate::roadmap::drainer::primary_repo_path(conn, project_id).map(PathBuf::from);
    let entry = Entry {
        read: repo_path
            .as_deref()
            .map(|repo| read_files(repo, agents_root().as_deref()))
            .unwrap_or_default(),
        stamp: repo_path.as_deref().map(stamp).unwrap_or_default(),
        approved: load_approved(conn, project_id),
        repo_path,
    };
    let status = entry.status(project_id);
    let (changed, applied_changed) = store(project_id, entry);
    if applied_changed {
        crate::sandbox::boxes::BoxRegistry::global().update_egress(project_id);
    }
    (status, changed)
}

/// Approve `patterns` from `project_id`'s files: re-read them first, so only
/// what they say now can be approved, then store the union with what was
/// approved before and update the running boxes.
pub fn approve(
    conn: &Connection,
    project_id: &str,
    patterns: &[String],
) -> Result<ProjectFileStatus> {
    let (current, _) = refresh(conn, project_id);
    let offered: Vec<&String> = current
        .egress
        .applied
        .iter()
        .chain(&current.egress.pending)
        .collect();
    let mut approved = load_approved(conn, project_id);
    for pattern in patterns {
        let spelled = pattern
            .parse::<fletch_box::HostPattern>()
            .map_err(|e| Error::Other(e.to_string()))?
            .to_string();
        if !offered.contains(&&spelled) {
            return Err(Error::Other(format!(
                "`{spelled}` is not in this project's settings files"
            )));
        }
        if !approved.contains(&spelled) {
            approved.push(spelled);
        }
    }
    conn.execute(
        "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3) \
         ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
        rusqlite::params![
            project_id,
            APPROVED_EGRESS_KEY,
            serde_json::to_string(&approved)?
        ],
    )?;
    Ok(refresh(conn, project_id).0)
}

/// Read every project's files into the mirror; boot.
pub fn seed(conn: &Connection) {
    for project_id in project_ids(conn) {
        refresh(conn, &project_id);
    }
}

fn project_ids(conn: &Connection) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT DISTINCT project_id FROM repos") else {
        return Vec::new();
    };
    stmt.query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

#[derive(Serialize)]
struct Changed<'a> {
    project_id: &'a str,
}

/// Keep the mirror current for as long as the host runs: every [`POLL`], each
/// project whose files' size or mtime moved — or that is new since the last
/// look — is re-read, and a change emits [`EVENT_CHANGED`].
pub fn watch(db: crate::roadmap::Db, sink: Sink) {
    crate::host::spawn(async move {
        loop {
            tokio::time::sleep(POLL).await;
            let (db, sink) = (db.clone(), sink.clone());
            let _ = tokio::task::spawn_blocking(move || {
                let conn = db.lock();
                for project_id in project_ids(&conn) {
                    let moved = STATE
                        .read()
                        .as_ref()
                        .and_then(|m| m.get(&project_id))
                        .is_none_or(|e| {
                            e.repo_path.as_deref().map(stamp).unwrap_or_default() != e.stamp
                        });
                    if moved && refresh(&conn, &project_id).1 {
                        crate::host::emit(
                            sink.as_ref(),
                            EVENT_CHANGED,
                            &Changed {
                                project_id: &project_id,
                            },
                        );
                    }
                }
            })
            .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn write(repo: &Path, text: &str) {
        std::fs::create_dir_all(repo.join(DIR)).unwrap();
        std::fs::write(repo.join(DIR).join(SHARED_FILE), text).unwrap();
    }

    /// A database holding project `pf` whose primary repo is `repo`, and the
    /// directory it lives in (dropping that deletes it).
    fn db_with_project(repo: &Path) -> (crate::roadmap::Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES ('pf', 'p', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repos (id, project_id, path, created_at) VALUES ('r', 'pf', ?1, 0)",
            [repo.to_string_lossy()],
        )
        .unwrap();
        drop(conn);
        (db, dir)
    }

    /// Additions apply only once approved: pending until then, applied after,
    /// and a new addition in the file waits again while the approved ones keep
    /// applying. Only what the files say can be approved, and a host taken out
    /// of the file stops applying.
    #[test]
    fn additions_apply_only_once_approved() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        write(&repo, r#"{ "egress": { "allow": ["a.com"] } }"#);
        let (db, _dir) = db_with_project(&repo);
        let conn = db.lock();

        let (status, _) = refresh(&conn, "pf");
        assert_eq!(status.egress.pending, vec!["a.com"]);
        assert!(status.egress.applied.is_empty());
        assert!(applied_egress("pf").is_empty());

        assert!(approve(&conn, "pf", &["evil.example".into()]).is_err());
        let status = approve(&conn, "pf", &["a.com".into()]).unwrap();
        assert_eq!(status.egress.applied, vec!["a.com"]);
        assert_eq!(applied_egress("pf"), vec!["a.com"]);

        write(&repo, r#"{ "egress": { "allow": ["a.com", "b.com"] } }"#);
        let (status, changed) = refresh(&conn, "pf");
        assert!(changed);
        assert_eq!(status.egress.applied, vec!["a.com"]);
        assert_eq!(status.egress.pending, vec!["b.com"]);

        write(&repo, r#"{ "egress": { "allow": ["b.com"] } }"#);
        assert!(refresh(&conn, "pf").0.egress.applied.is_empty());
        assert!(applied_egress("pf").is_empty());
    }

    /// The approval is no client's to write: it is not a client project key,
    /// so `set_project_setting` refuses it.
    #[test]
    fn the_approval_is_host_only() {
        assert!(!crate::commands::is_client_project_key(APPROVED_EGRESS_KEY));
    }
}

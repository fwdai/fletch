//! Folders on the user's machine granted to one workspace's agent: the user
//! @-mentions or drops a folder in the composer, and every sandbox engine
//! exposes it to that agent live, at its own path — read-only unless the user
//! made it writable. Seatbelt re-allows it in the profile, a container binds
//! it, a box mounts it as a virtiofs share (`AgentLaunchCtx::folder_grants`).
//!
//! A grant widens a sandbox on purpose, so [`check`] runs on the host before
//! one is stored, and refuses the folders no single grant should expose:
//! - `/`, the home folder and anything above it: every credential store at once.
//! - a hidden folder at the top of home (`~/.ssh`, `~/.aws`, `~/.config`,
//!   `~/.fletch`, the provider logins) or anything inside one: credentials,
//!   config that runs code, and Fletch's own workspaces and mailboxes.
//! - `~/Library`, except iCloud Drive (`~/Library/Mobile Documents`): app
//!   state, keychains, Fletch's database.
//! - Fletch's own data, checkouts and mailbox roots wherever they live.
//! - writable only: a system folder, or a folder that is or holds a directory
//!   on the PATH, where a file the agent writes runs as the user the next time
//!   a command is looked up.
//! - for a box only: anything outside `/Users` and `/Volumes`, the two roots
//!   the guest image can mount a share under.
//!
//! Everything else is the user's call, including a writable grant on another
//! project's repository: the agent may then change anything in it.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::sandbox::EngineKind;

use super::{now_millis, WorkspaceManager};

/// One folder granted to a workspace's agent. `path` is canonical (symlinks
/// resolved), so every engine names the same directory the sandbox sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderGrant {
    pub path: PathBuf,
    #[serde(default)]
    pub writable: bool,
}

/// What a grant or revoke left in place: the workspace's grants, and whether
/// the change still waits on a relaunch held for the running turn's end
/// (`Supervisor::apply_changes_now` ends the turn early instead).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderGrantsUpdate {
    pub folder_grants: Vec<FolderGrant>,
    pub applies_at_turn_end: bool,
}

/// The only folder under `~/Library` a grant may name: iCloud Drive.
const ICLOUD_DRIVE: &str = "Library/Mobile Documents";

/// Folders outside home a writable grant may not name or sit inside: the
/// system's own, and the package managers' bin roots.
const SYSTEM_ROOTS: &[&str] = &[
    "/System",
    "/Library",
    "/Applications",
    "/usr",
    "/bin",
    "/sbin",
    "/etc",
    "/var",
    "/private/etc",
    "/private/var",
    "/opt/homebrew",
    "/opt/local",
    "/dev",
    "/cores",
];

/// The roots a box guest can mount a share under (the image ships both as
/// empty mountpoints; see `packaging/box-image/build.sh`).
const BOX_SHARE_ROOTS: &[&str] = &["/Users", "/Volumes"];

/// Validate `requested` as a grant for an agent on `engine` and return the
/// canonical folder to store. `~/` is expanded. The folder is listed once
/// here, as the app: if macOS guards it with a privacy prompt (Documents,
/// Desktop, Downloads), it asks while the user is granting it rather than in
/// the middle of an agent's turn, and a refusal surfaces now.
pub fn check(requested: &Path, writable: bool, engine: EngineKind) -> Result<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    let home = home.canonicalize().unwrap_or(home);
    let requested = match requested.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => requested.to_path_buf(),
    };
    if !requested.is_absolute() {
        return Err(Error::InvalidPath(format!(
            "{} is not an absolute path",
            requested.display()
        )));
    }
    let path = std::fs::canonicalize(&requested)
        .map_err(|e| Error::InvalidPath(format!("{}: {e}", requested.display())))?;
    if !path.is_dir() {
        return Err(Error::InvalidPath(format!(
            "{} is not a folder",
            path.display()
        )));
    }
    let path_dirs = if writable { path_dirs() } else { Vec::new() };
    if let Some(reason) = refusal(
        &path,
        &home,
        &protected_roots(),
        writable,
        &path_dirs,
        engine,
    ) {
        return Err(Error::InvalidPath(format!(
            "Fletch won't share {} with an agent: {reason}",
            path.display()
        )));
    }
    std::fs::read_dir(&path).map_err(|e| {
        Error::Other(format!(
            "Fletch can't read {}: {e}. If macOS asked about this folder, allow Fletch in \
             System Settings › Privacy & Security › Files and Folders.",
            path.display()
        ))
    })?;
    Ok(path)
}

/// [`check`] each of `grants` off the async runtime (each lists its folder,
/// and a privacy prompt waits on the user), returning them canonical with one
/// entry per folder (the last mode asked for wins). The first refusal fails
/// the lot, with its reason.
pub async fn check_all(grants: Vec<FolderGrant>, engine: EngineKind) -> Result<Vec<FolderGrant>> {
    if grants.is_empty() {
        return Ok(grants);
    }
    tokio::task::spawn_blocking(move || {
        let mut checked: Vec<FolderGrant> = Vec::new();
        for grant in grants {
            let path = check(&grant.path, grant.writable, engine)?;
            checked.retain(|g| g.path != path);
            checked.push(FolderGrant {
                path,
                writable: grant.writable,
            });
        }
        Ok(checked)
    })
    .await
    .map_err(|e| Error::Other(e.to_string()))?
}

/// The grants that change what an agent on `engine` launches with, sorted by
/// path — what a grant or revoke must relaunch the agent for when this set
/// differs before and after. Seatbelt reads are open already outside the
/// folders macOS guards with a privacy prompt, so a read-only grant elsewhere
/// adds no rule to the profile: granting it takes effect at once (the agent
/// could read it before) and revoking it takes nothing away. Every grant is a
/// container mount or a box share. `home` is canonical, as the profile's is.
pub fn launch_relevant(
    engine: EngineKind,
    grants: &[FolderGrant],
    home: &Path,
) -> Vec<FolderGrant> {
    let mut relevant: Vec<FolderGrant> = match engine {
        EngineKind::SandboxExec => grants
            .iter()
            .filter(|g| {
                g.writable || crate::sandbox::policy::inside_macos_privacy_dir(home, &g.path)
            })
            .cloned()
            .collect(),
        EngineKind::Docker | EngineKind::Podman | EngineKind::Box => grants.to_vec(),
    };
    relevant.sort_by(|a, b| a.path.cmp(&b.path));
    relevant
}

/// The grants a launch applies: those whose folder is still there. A folder
/// deleted or moved since it was granted would otherwise fail every launch —
/// a container refuses a bind with no source, a box a share with no directory
/// — so it is skipped (and logged) until the user grants it again or takes it
/// back; the stored grant is left as it is.
pub fn present(grants: &[FolderGrant]) -> Vec<FolderGrant> {
    grants
        .iter()
        .filter(|grant| {
            let there = grant.path.is_dir();
            if !there {
                tracing::warn!(path = %grant.path.display(), "granted folder is gone; not applied");
            }
            there
        })
        .cloned()
        .collect()
}

/// Why `path` (canonical) may not be granted, or `None` when it may. Pure, for
/// the tests: `protected` are Fletch's own roots, `path_dirs` the PATH entries
/// (only consulted for a writable grant).
fn refusal(
    path: &Path,
    home: &Path,
    protected: &[PathBuf],
    writable: bool,
    path_dirs: &[PathBuf],
    engine: EngineKind,
) -> Option<&'static str> {
    if path.parent().is_none() || home.starts_with(path) {
        return Some("it holds your whole home folder");
    }
    if let Ok(rest) = path.strip_prefix(home) {
        let hidden = rest
            .components()
            .next()
            .is_some_and(|c| c.as_os_str().to_string_lossy().starts_with('.'));
        if hidden {
            return Some("hidden folders in your home folder hold logins and tool config");
        }
        if rest.starts_with("Library") && !rest.starts_with(ICLOUD_DRIVE) {
            return Some("~/Library holds app data and logins");
        }
    }
    if protected
        .iter()
        .any(|root| path.starts_with(root) || root.starts_with(path))
    {
        return Some("it holds Fletch's own data");
    }
    if writable {
        if SYSTEM_ROOTS.iter().any(|root| path.starts_with(root)) {
            return Some("it is a system folder");
        }
        if path_dirs.iter().any(|dir| dir.starts_with(path)) {
            return Some(
                "a program the agent writes there would run the next time you use your shell",
            );
        }
    }
    if engine == EngineKind::Box && !BOX_SHARE_ROOTS.iter().any(|root| path.starts_with(root)) {
        return Some("a box can only mount folders under /Users or /Volumes");
    }
    None
}

/// Fletch's own roots, wherever they were configured: the app data dir (the
/// desktop's and a host's), and the checkouts and mailbox roots, which an
/// override can move out of `~/.fletch`.
fn protected_roots() -> Vec<PathBuf> {
    let mut roots = vec![crate::data_dir()];
    roots.extend(crate::sandbox::configured_data_dir());
    roots.extend(super::checkouts_root().ok());
    roots.extend(crate::rpc::rpc_root().ok());
    roots
        .iter()
        .map(|root| crate::sandbox::policy::resolve_existing_prefix(root))
        .collect()
}

/// The directories on the PATH a launched agent's shell would search: the
/// login shell's, and the app's own.
fn path_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let shell = crate::bin_resolve::login_shell_env().and_then(|env| env.get("PATH").cloned());
    let process = std::env::var("PATH").ok();
    for value in shell.iter().chain(process.iter()) {
        dirs.extend(
            std::env::split_paths(value)
                .filter(|d| d.is_absolute())
                .map(|d| crate::sandbox::policy::resolve_existing_prefix(&d)),
        );
    }
    dirs
}

impl WorkspaceManager {
    /// Grant `path` (already [`check`]ed) to workspace `id`, or change an
    /// existing grant's mode. Returns the workspace's grants afterwards.
    pub fn upsert_folder_grant(
        &self,
        id: &str,
        path: &Path,
        writable: bool,
    ) -> Result<Vec<FolderGrant>> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "INSERT INTO workspace_folder_grants (workspace_id, path, writable, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (workspace_id, path) DO UPDATE SET writable = excluded.writable",
            rusqlite::params![id, path.to_string_lossy(), writable, now_millis()],
        )?;
        Ok(load_folder_grants(&conn, id))
    }

    /// Drop the grant of `path` from workspace `id`. Returns the workspace's
    /// grants afterwards; revoking a folder that was never granted is a no-op.
    pub fn remove_folder_grant(&self, id: &str, path: &Path) -> Result<Vec<FolderGrant>> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "DELETE FROM workspace_folder_grants WHERE workspace_id = ?1 AND path = ?2",
            rusqlite::params![id, path.to_string_lossy()],
        )?;
        Ok(load_folder_grants(&conn, id))
    }
}

/// Write a new workspace's grants with its row (`add_agent`).
pub(super) fn insert_folder_grants(
    conn: &Connection,
    id: &str,
    grants: &[FolderGrant],
) -> Result<()> {
    let now = now_millis();
    for grant in grants {
        conn.execute(
            "INSERT OR REPLACE INTO workspace_folder_grants (workspace_id, path, writable, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, grant.path.to_string_lossy(), grant.writable, now],
        )?;
    }
    Ok(())
}

/// Workspace `id`'s grants, oldest first. Shallower paths come before the
/// folders nested in them only by accident of order, so engines that mount
/// sort for themselves.
pub(super) fn load_folder_grants(conn: &Connection, id: &str) -> Vec<FolderGrant> {
    let mut stmt = match conn.prepare(
        "SELECT path, writable FROM workspace_folder_grants
          WHERE workspace_id = ?1 ORDER BY created_at, path",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([id], |row| {
        let path: String = row.get(0)?;
        Ok(FolderGrant {
            path: PathBuf::from(path),
            writable: row.get(1)?,
        })
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/u";

    fn refused(path: &str, writable: bool, engine: EngineKind) -> Option<&'static str> {
        refusal(
            Path::new(path),
            Path::new(HOME),
            &[PathBuf::from("/Users/u/Library/Application Support/fletch")],
            writable,
            &[
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/Users/u/bin"),
            ],
            engine,
        )
    }

    #[test]
    fn a_project_folder_is_granted_either_way() {
        for writable in [false, true] {
            assert_eq!(
                refused("/Users/u/Projects/app", writable, EngineKind::SandboxExec),
                None
            );
            assert_eq!(
                refused("/Users/u/Projects/app", writable, EngineKind::Box),
                None
            );
            assert_eq!(
                refused("/Users/u/Documents/specs", writable, EngineKind::Docker),
                None
            );
        }
    }

    #[test]
    fn home_and_everything_above_it_is_refused() {
        for path in ["/", "/Users", "/Users/u"] {
            assert!(
                refused(path, false, EngineKind::SandboxExec).is_some(),
                "{path}"
            );
        }
    }

    #[test]
    fn hidden_home_folders_are_refused_however_deep() {
        for path in [
            "/Users/u/.ssh",
            "/Users/u/.config/gh",
            "/Users/u/.fletch/workspaces/a",
        ] {
            assert!(
                refused(path, false, EngineKind::SandboxExec).is_some(),
                "{path}"
            );
        }
        // A dot folder deeper inside a project is the project's business.
        assert_eq!(
            refused(
                "/Users/u/Projects/app/.github",
                true,
                EngineKind::SandboxExec
            ),
            None
        );
    }

    #[test]
    fn library_is_refused_but_icloud_drive_is_not() {
        assert!(refused("/Users/u/Library/Keychains", false, EngineKind::SandboxExec).is_some());
        assert!(refused("/Users/u/Library", false, EngineKind::SandboxExec).is_some());
        let icloud = "/Users/u/Library/Mobile Documents/com~apple~CloudDocs/Notes";
        assert_eq!(refused(icloud, true, EngineKind::SandboxExec), None);
    }

    #[test]
    fn fletch_data_and_any_folder_holding_it_are_refused() {
        let data = "/Users/u/Library/Application Support/fletch/attachments";
        assert!(refused(data, false, EngineKind::SandboxExec).is_some());
    }

    #[test]
    fn system_and_path_folders_are_read_only_at_most() {
        for path in [
            "/usr/local/share/doc",
            "/opt/homebrew/Cellar",
            "/Users/u/bin",
            "/usr/local",
        ] {
            assert!(
                refused(path, true, EngineKind::SandboxExec).is_some(),
                "{path}"
            );
        }
        assert_eq!(
            refused("/usr/local/share/doc", false, EngineKind::SandboxExec),
            None
        );
        // Reading a PATH dir hands the agent nothing it can run on the host.
        assert_eq!(
            refused("/Users/u/bin", false, EngineKind::SandboxExec),
            None
        );
    }

    #[test]
    fn a_box_mounts_only_under_users_and_volumes() {
        assert_eq!(refused("/Volumes/Work/repo", false, EngineKind::Box), None);
        assert!(refused("/opt/data", false, EngineKind::Box).is_some());
        assert_eq!(refused("/opt/data", false, EngineKind::SandboxExec), None);
    }

    /// Under seatbelt only a writable grant or one inside a privacy-guarded
    /// folder changes the profile, so only those relaunch the agent; every
    /// grant changes a container's or a box's launch.
    #[test]
    fn only_grants_the_launch_sees_need_a_relaunch() {
        let home = Path::new(HOME);
        let grant = |p: &str, writable| FolderGrant {
            path: PathBuf::from(p),
            writable,
        };
        let readable_anyway = [grant("/Users/u/Projects/lib", false)];
        assert!(launch_relevant(EngineKind::SandboxExec, &readable_anyway, home).is_empty());
        for engine in [EngineKind::Docker, EngineKind::Podman, EngineKind::Box] {
            assert_eq!(launch_relevant(engine, &readable_anyway, home).len(), 1);
        }
        let guarded = [
            grant("/Users/u/Projects/out", true),
            grant("/Users/u/Documents/specs", false),
            grant("/Users/u/Projects/lib", false),
        ];
        let relevant = launch_relevant(EngineKind::SandboxExec, &guarded, home);
        assert_eq!(
            relevant,
            [guarded[1].clone(), guarded[0].clone()],
            "sorted by path, the open read left out"
        );
    }

    #[test]
    fn grants_round_trip_through_the_store() {
        use crate::workspace::tests::{mk_repo, seed_repo, test_db};
        use crate::workspace::{new_agent_record, AgentView};
        let db = test_db();
        seed_repo(&db, "/r");
        let manager = WorkspaceManager::new(db);
        let mut record = new_agent_record(
            "ws-grants".into(),
            "a".into(),
            "claude".into(),
            mk_repo("/r"),
            "t".into(),
            AgentView::Custom,
        );
        record.folder_grants = vec![FolderGrant {
            path: PathBuf::from("/Users/u/Projects/lib"),
            writable: false,
        }];
        manager.add_agent(&mut record).unwrap();
        assert_eq!(
            manager.agent("ws-grants").unwrap().folder_grants,
            record.folder_grants
        );

        let out = Path::new("/Users/u/Projects/out");
        let grants = manager
            .upsert_folder_grant("ws-grants", out, false)
            .unwrap();
        assert_eq!(grants.len(), 2);
        let grants = manager.upsert_folder_grant("ws-grants", out, true).unwrap();
        assert!(grants.iter().any(|g| g.path == out && g.writable));
        assert_eq!(grants.len(), 2, "changing a mode keeps one row per folder");

        let grants = manager.remove_folder_grant("ws-grants", out).unwrap();
        assert_eq!(grants, record.folder_grants);
        assert_eq!(
            manager.agent("ws-grants").unwrap().folder_grants,
            record.folder_grants
        );
    }
}

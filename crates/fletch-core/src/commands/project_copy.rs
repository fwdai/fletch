//! Running a project's workspaces on a device other than its home.
//!
//! A project belongs to the device it was added on. Another device that is
//! asked to run one of its workspaces needs the repositories on its own disk,
//! so the home describes the project ([`project_definition_impl`]) and the
//! device that will run it makes a copy from that ([`ensure_project_impl`]):
//! the same project id, a clone of each repository from its `origin`. Nothing
//! flows back; the copy is never edited where it runs.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::supervisor::Supervisor;
use crate::workspace::{
    ProjectDefinition, ProjectHome, RepoDefinition, Workspace, WorkspaceManager,
};

/// Describe the project `project_id` for a device that will run it. For a
/// project this host owns the home is this host; for a copy it is whoever the
/// copy came from, so a definition can be read back off any device that has
/// one.
pub async fn project_definition_impl(
    ctx: &EngineCtx,
    supervisor: &Supervisor,
    project_id: &str,
) -> Result<ProjectDefinition> {
    let (name, home, repos) = supervisor.workspace.project_outline(project_id)?;
    let home = match home {
        Some(home) => home,
        None => ProjectHome {
            device: ctx
                .host_id()
                .ok_or_else(|| {
                    Error::Other(
                        "This device has no identity to share a project under. \
                         Check Settings › Devices."
                            .into(),
                    )
                })?
                .to_string(),
            name: crate::remote::host_info().name,
        },
    };
    let mut described = Vec::with_capacity(repos.len());
    for (id, path, label) in repos {
        let dir = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::InvalidPath(path.display().to_string()))?
            .to_string();
        // A repository whose origin cannot be read is described without one:
        // the device asked to clone it says why it cannot.
        let origin = crate::git::origin_url(&path).await.ok().flatten();
        described.push(RepoDefinition {
            id,
            dir,
            origin: origin.as_deref().map(without_credentials),
            label,
        });
    }
    Ok(ProjectDefinition {
        project_id: project_id.to_string(),
        name,
        home,
        repos: described,
    })
}

/// `origin` as another device may be told it. A definition is readable by any
/// paired device and kept by clients, and the device that clones signs in with
/// its own login, so a credential the home keeps in the URL
/// (`https://user:token@host/…`, `…/repo.git?access_token=…`) never leaves it:
/// a `scheme://` URL loses its query and fragment, an http(s) one its whole
/// userinfo, any other its password. An scp-style remote (`git@host:path`) has
/// no place for one.
fn without_credentials(origin: &str) -> String {
    let Some((scheme, rest)) = origin.split_once("://") else {
        return origin.to_string();
    };
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return format!("{scheme}://{authority}{path}");
    };
    let user = userinfo.split(':').next().unwrap_or_default();
    if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        format!("{scheme}://{host}{path}")
    } else {
        format!("{scheme}://{user}@{host}{path}")
    }
}

/// One `ensure_project` at a time: two spawns into a project this host does
/// not have yet would otherwise clone into the same folder at once.
static ENSURING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Make sure this host can run workspaces of `def`, and answer with the
/// project list.
///
/// A project this host owns is left as it is. Otherwise each repository this
/// host does not have yet is cloned from its `origin`, and the copy is brought
/// in line with the definition (`WorkspaceManager::reconcile_copy`): its name
/// and labels follow the home, and a repository the home has removed leaves
/// the project, its clone with it, once no agent here has a checkout of it.
///
/// All or nothing: a repository with no `origin` fails the call before
/// anything is cloned, and the clones are made in a staging folder and only
/// moved under [`copy_root`] and recorded, in one transaction, once every one
/// of them has finished. A call that fails or is abandoned partway leaves no
/// project behind, and what it left on disk is cleared by the next call.
pub async fn ensure_project_impl(
    supervisor: &Supervisor,
    def: ProjectDefinition,
) -> Result<Workspace> {
    let root = crate::workspace::projects_root()?;
    ensure_project_under(supervisor, def, &root).await
}

/// [`ensure_project_impl`] with the projects root given, so a test can point
/// it at a scratch folder without touching the process environment.
async fn ensure_project_under(
    supervisor: &Supervisor,
    def: ProjectDefinition,
    projects_root: &Path,
) -> Result<Workspace> {
    validate(&def)?;
    let _one_at_a_time = ENSURING.lock().await;
    let workspace = &supervisor.workspace;

    if workspace.owns_project(&def.project_id) != Some(true) {
        let root = copy_root(projects_root, &def.project_id);
        let have = workspace.project_repo_ids(&def.project_id);
        let missing: Vec<&RepoDefinition> =
            def.repos.iter().filter(|r| !have.contains(&r.id)).collect();
        for repo in &missing {
            if repo.origin.is_none() {
                return Err(Error::Other(format!(
                    "{} has no remote to clone from. Publish it from {} first.",
                    repo.dir, def.home.name
                )));
            }
        }

        let project_dir = projects_root.join(&def.project_id);
        clear_staging(&project_dir).await;
        let staging = project_dir.join(format!("{STAGING_PREFIX}{}", uuid::Uuid::new_v4()));
        for repo in &missing {
            let origin = repo.origin.as_deref().expect("checked above");
            let staged = staging.join(&repo.id).join(&repo.dir);
            if let Err(e) = crate::github::repo_clone(clone_spec(origin), &staged).await {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                return Err(e);
            }
        }

        let mut cloned = Vec::with_capacity(missing.len());
        for repo in missing {
            let slot = root.join(&repo.id);
            // Not recorded, so nothing refers to it: what an earlier call
            // moved into place before it failed to record the project.
            if slot.exists() {
                tokio::fs::remove_dir_all(&slot).await?;
            }
            tokio::fs::create_dir_all(&root).await?;
            tokio::fs::rename(staging.join(&repo.id), &slot).await?;
            cloned.push((repo.id.clone(), slot.join(&repo.dir)));
        }
        let _ = tokio::fs::remove_dir_all(&staging).await;
        workspace.reconcile_copy(&def, &cloned)?;
        prune_copy_under(workspace, projects_root, &def.project_id).await;
        workspace.detect_icon(&def.project_id);
    }

    Ok(workspace.current().expect("workspace initialized"))
}

/// Where a copy's repositories are cloned: `<projects root>/<project id>/repos`,
/// each in a slot of its own, `<repo id>/<folder name>`. The id keeps two
/// repositories apart whatever their folders are called; the folder keeps the
/// name the home knows it by.
fn copy_root(projects_root: &Path, project_id: &str) -> PathBuf {
    projects_root.join(project_id).join("repos")
}

/// Remove the clones this host made for the project `project_id` that no
/// repository of it is recorded at any more: the whole copy once the project
/// is deleted. Best effort, and a no-op for a project this host owns, which
/// has no clones of its own making.
pub async fn prune_copy(workspace: &WorkspaceManager, project_id: &str) {
    // A copy's id is always one (`validate`), and it names a folder here.
    if uuid::Uuid::parse_str(project_id).is_err() {
        return;
    }
    let Ok(root) = crate::workspace::projects_root() else {
        return;
    };
    // Not while `ensure_project` has clones in place that it has yet to record.
    let _one_at_a_time = ENSURING.lock().await;
    prune_copy_under(workspace, &root, project_id).await;
}

/// [`prune_copy`] with the projects root given and the caller holding
/// [`ENSURING`]. A clone's row is kept for as long as anything borrows from
/// the clone (`WorkspaceManager::reconcile_copy`), and deleting a project
/// deletes its agents and workflow runs with it, so a clone with no row has
/// nothing left that reads it. Nothing is removed when the rows cannot be
/// read: a clone is only known to be unrecorded by a list that is whole.
async fn prune_copy_under(workspace: &WorkspaceManager, projects_root: &Path, project_id: &str) {
    let root = copy_root(projects_root, project_id);
    let Ok(recorded) = workspace.copy_clones(project_id) else {
        return;
    };
    let Ok(mut slots) = tokio::fs::read_dir(&root).await else {
        return;
    };
    while let Ok(Some(slot)) = slots.next_entry().await {
        let slot = slot.path();
        if !recorded.iter().any(|clone| clone.starts_with(&slot)) {
            let _ = tokio::fs::remove_dir_all(&slot).await;
        }
    }
    // Goes with its last clone; refused while one is left.
    let _ = tokio::fs::remove_dir(&root).await;
}

/// What to hand `github::repo_clone` for `origin`. A GitHub ssh remote becomes
/// its `owner/repo`, which clones over https with this host's GitHub login:
/// the home's ssh key is the home's, and the login is what this host pushes
/// and opens pull requests with anyway. Anything else is cloned as written.
fn clone_spec(origin: &str) -> &str {
    let path = origin
        .strip_prefix("git@github.com:")
        .or_else(|| origin.strip_prefix("ssh://git@github.com/"));
    match path {
        Some(path) => path.strip_suffix(".git").unwrap_or(path),
        None => origin,
    }
}

/// What a staging folder is named, beside a copy's `repos` folder.
const STAGING_PREFIX: &str = "repos-staging-";

/// Remove the staging folders earlier calls left in `project_dir`: one whose
/// request was abandoned mid-clone never got to remove its own. Best effort.
async fn clear_staging(project_dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(project_dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
        {
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
}

/// A definition arrives off the wire and names paths: the project id and each
/// repository's id and folder become path components under [`copy_root`], so
/// none may be anything but one plain component.
fn validate(def: &ProjectDefinition) -> Result<()> {
    if uuid::Uuid::parse_str(&def.project_id).is_err() {
        return Err(Error::InvalidPath(format!(
            "not a project id: {}",
            def.project_id
        )));
    }
    if def.name.trim().is_empty() {
        return Err(Error::Other("project name cannot be empty".into()));
    }
    if def.home.device.trim().is_empty() {
        return Err(Error::Other("a project needs a home device".into()));
    }
    if def.repos.is_empty() {
        return Err(Error::Other("a project needs a repository".into()));
    }
    let mut seen = std::collections::HashSet::new();
    for repo in &def.repos {
        let dir = repo.dir.as_str();
        let plain = !dir.is_empty()
            && dir != "."
            && dir != ".."
            && !dir.contains(['/', '\\', '\0'])
            && Path::new(dir).file_name().and_then(|n| n.to_str()) == Some(dir);
        if !plain {
            return Err(Error::InvalidPath(format!(
                "not a folder name: {}",
                repo.dir
            )));
        }
        let id = repo.id.as_str();
        let plain_id = !id.is_empty()
            && id.len() <= 64
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !plain_id {
            return Err(Error::InvalidPath(format!("not a repository id: {id}")));
        }
        if !seen.insert(id) {
            return Err(Error::InvalidPath(format!(
                "two repositories share the id {id}"
            )));
        }
        if let Some(origin) = &repo.origin {
            // The URL is handed to `git clone` as an argument.
            if origin.starts_with('-') {
                return Err(Error::InvalidPath(format!("not a remote: {origin}")));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testing::{commit_file, git, init_repo};
    use std::sync::Arc;

    const HOME_ID: &str = "aG9tZS1kZXZpY2U";

    /// A supervisor over a fresh DB under `dir`, and the projects root its
    /// copies go under.
    fn host(dir: &Path) -> (Supervisor, PathBuf) {
        let db = crate::database::init(&dir.join("db")).unwrap();
        let sup = Supervisor::new(Arc::new(WorkspaceManager::new(db)));
        (sup, dir.join("projects-root"))
    }

    /// A repository with one commit at `<dir>/<name>`.
    fn origin_repo(dir: &Path, name: &str) -> PathBuf {
        let repo = dir.join(name);
        init_repo(&repo);
        commit_file(&repo, "README.md");
        repo
    }

    /// `repo` as an `origin` another device can clone.
    fn url(repo: &Path) -> String {
        format!("file://{}", repo.display())
    }

    fn definition(repos: Vec<RepoDefinition>) -> ProjectDefinition {
        ProjectDefinition {
            project_id: uuid::Uuid::new_v4().to_string(),
            name: "Fletch".into(),
            home: ProjectHome {
                device: HOME_ID.into(),
                name: "MacBook".into(),
            },
            repos,
        }
    }

    /// Where the copy of `def` under `root` keeps its `i`th repository.
    fn clone_of(root: &Path, def: &ProjectDefinition, i: usize) -> PathBuf {
        copy_root(root, &def.project_id)
            .join(&def.repos[i].id)
            .join(&def.repos[i].dir)
    }

    fn repo(dir: &str, origin: Option<&Path>) -> RepoDefinition {
        RepoDefinition {
            id: uuid::Uuid::new_v4().to_string(),
            dir: dir.into(),
            origin: origin.map(url),
            label: None,
        }
    }

    #[tokio::test]
    async fn a_project_this_host_lacks_is_cloned_and_recorded_as_a_copy() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let api = origin_repo(td.path(), "api");
        let def = definition(vec![repo("app", Some(&app)), repo("api", Some(&api))]);

        let ws = ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();

        let refs: Vec<_> = ws
            .projects
            .iter()
            .filter(|p| p.project_id == def.project_id)
            .collect();
        // Both repositories, in the home's order, under the copy root.
        assert_eq!(
            refs.iter().map(|p| p.path.clone()).collect::<Vec<_>>(),
            [clone_of(&root, &def, 0), clone_of(&root, &def, 1)]
        );
        assert!(clone_of(&root, &def, 0).join("README.md").is_file());
        for p in refs {
            assert_eq!(p.name, "Fletch");
            assert_eq!(p.home.as_ref(), Some(&def.home));
        }
    }

    #[tokio::test]
    async fn asking_again_clones_nothing_and_follows_the_homes_name() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let mut def = definition(vec![repo("app", Some(&app))]);
        ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();
        let clone = clone_of(&root, &def, 0);
        std::fs::write(clone.join("marker"), "kept").unwrap();

        def.name = "Fletch 2".into();
        // The home has since added a repository.
        let docs = origin_repo(td.path(), "docs");
        def.repos.push(repo("docs", Some(&docs)));
        let ws = ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();

        assert!(clone.join("marker").is_file(), "the clone was replaced");
        let refs: Vec<_> = ws
            .projects
            .iter()
            .filter(|p| p.project_id == def.project_id)
            .collect();
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().all(|p| p.name == "Fletch 2"));
    }

    #[tokio::test]
    async fn a_repository_without_a_remote_refuses_the_whole_project() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let def = definition(vec![repo("app", Some(&app)), repo("local-only", None)]);

        let err = ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "local-only has no remote to clone from. Publish it from MacBook first."
        );
        assert!(sup.workspace.owns_project(&def.project_id).is_none());
        assert!(!copy_root(&root, &def.project_id).exists());
    }

    #[tokio::test]
    async fn a_project_this_host_owns_is_left_as_it_is() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        sup.workspace.add_workspace_repo(app.clone()).unwrap();
        let project_id = sup.current_workspace().unwrap().projects[0]
            .project_id
            .clone();
        let mut def = definition(vec![repo("app", Some(&app))]);
        def.project_id = project_id.clone();
        def.name = "Renamed elsewhere".into();

        let ws = ensure_project_under(&sup, def, &root).await.unwrap();

        assert_eq!(ws.projects.len(), 1);
        assert_eq!(ws.projects[0].path, app);
        assert_eq!(ws.projects[0].name, "app");
        assert!(ws.projects[0].home.is_none());
        assert!(!copy_root(&root, &project_id).exists());
    }

    #[tokio::test]
    async fn a_folder_added_here_does_not_join_a_copy_of_the_same_name() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let theirs = origin_repo(td.path(), "app");
        let mut def = definition(vec![repo("app", Some(&theirs))]);
        def.name = "mine".into();
        ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();

        let mine = origin_repo(td.path(), "mine");
        let ws = sup.workspace.add_workspace_repo(mine.clone()).unwrap();

        let own = ws.projects.iter().find(|p| p.path == mine).unwrap();
        assert_ne!(own.project_id, def.project_id);
        assert!(own.home.is_none());
    }

    #[tokio::test]
    async fn a_definition_naming_a_path_is_refused() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        for dir in ["../escape", "a/b", "..", ""] {
            let def = definition(vec![repo(dir, Some(&app))]);
            assert!(
                ensure_project_under(&sup, def, &root).await.is_err(),
                "accepted {dir:?}"
            );
        }
        let mut def = definition(vec![repo("app", Some(&app))]);
        def.project_id = "../../etc".into();
        assert!(ensure_project_under(&sup, def, &root).await.is_err());
        let mut def = definition(vec![repo("app", Some(&app))]);
        def.repos[0].origin = Some("--upload-pack=touch /tmp/x".into());
        assert!(ensure_project_under(&sup, def, &root).await.is_err());
    }

    #[tokio::test]
    async fn a_definition_names_this_host_as_home_and_a_copy_names_its_own() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let (ctx, _sink, _db) = crate::host::ctx::test_ctx();
        ctx.set_host_id("dGhpcy1ob3N0".into());

        // Owned here: this host is the home, and the origin is read off disk.
        let upstream = origin_repo(td.path(), "upstream");
        let app = origin_repo(td.path(), "app");
        git(&app, &["remote", "add", "origin", &url(&upstream)]);
        sup.workspace.add_workspace_repo(app.clone()).unwrap();
        let project_id = sup.current_workspace().unwrap().projects[0]
            .project_id
            .clone();
        let def = project_definition_impl(&ctx, &sup, &project_id)
            .await
            .unwrap();
        assert_eq!(def.home.device, "dGhpcy1ob3N0");
        assert_eq!(def.name, "app");
        assert_eq!(def.repos.len(), 1);
        assert_eq!(def.repos[0].dir, "app");
        assert_eq!(def.repos[0].origin, Some(url(&upstream)));

        // A copy: its home is whoever it came from, not this host.
        let theirs = definition(vec![repo("upstream", Some(&upstream))]);
        ensure_project_under(&sup, theirs.clone(), &root)
            .await
            .unwrap();
        let again = project_definition_impl(&ctx, &sup, &theirs.project_id)
            .await
            .unwrap();
        // Read back off the copy, it is the definition the home wrote: the
        // repository keeps the home's id.
        assert_eq!(again, theirs);
    }

    #[tokio::test]
    async fn a_clone_that_fails_leaves_no_project_behind() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let gone = td.path().join("gone");
        let mut def = definition(vec![repo("app", Some(&app)), repo("gone", Some(&gone))]);

        assert!(ensure_project_under(&sup, def.clone(), &root)
            .await
            .is_err());

        assert!(sup.workspace.owns_project(&def.project_id).is_none());
        assert!(!copy_root(&root, &def.project_id).exists());
        let left: Vec<_> = std::fs::read_dir(root.join(&def.project_id))
            .map(|d| d.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "left behind: {left:?}");

        // The home fixes the remote; the same call now makes the whole copy.
        let fixed = origin_repo(td.path(), "fixed");
        def.repos[1].origin = Some(url(&fixed));
        let ws = ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();
        assert_eq!(
            ws.projects
                .iter()
                .filter(|p| p.project_id == def.project_id)
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn what_an_abandoned_call_left_on_disk_is_replaced_not_adopted() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let def = definition(vec![repo("app", Some(&app))]);
        // An unfinished clone where the copy goes, and a staging folder beside
        // it: neither is recorded.
        let target = clone_of(&root, &def, 0);
        std::fs::create_dir_all(target.join(".git")).unwrap();
        let stale = root
            .join(&def.project_id)
            .join(format!("{STAGING_PREFIX}old"));
        std::fs::create_dir_all(stale.join("app")).unwrap();

        ensure_project_under(&sup, def, &root).await.unwrap();

        assert!(target.join("README.md").is_file(), "the stub was adopted");
        assert!(!stale.exists());
    }

    #[tokio::test]
    async fn repositories_sharing_a_folder_name_are_cloned_side_by_side() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let client = origin_repo(&td.path().join("client"), "app");
        let server = origin_repo(&td.path().join("server"), "app");
        std::fs::write(server.join("server.txt"), "").unwrap();
        git(&server, &["add", "-A"]);
        git(&server, &["commit", "-q", "-m", "server"]);
        // The second differs from the first only by case, which the default
        // macOS filesystem does not tell apart.
        let def = definition(vec![repo("app", Some(&client)), repo("App", Some(&server))]);

        ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();

        assert!(!clone_of(&root, &def, 0).join("server.txt").exists());
        assert!(clone_of(&root, &def, 1).join("server.txt").is_file());

        // One id named twice is one repository named twice.
        let mut twice = definition(vec![repo("a", Some(&client)), repo("b", Some(&server))]);
        twice.repos[1].id = twice.repos[0].id.clone();
        assert!(ensure_project_under(&sup, twice, &root).await.is_err());
    }

    /// The copy's repositories, as the project list has them.
    fn copied(ws: &Workspace, def: &ProjectDefinition) -> Vec<(PathBuf, Option<String>)> {
        ws.projects
            .iter()
            .filter(|p| p.project_id == def.project_id)
            .map(|p| (p.path.clone(), p.label.clone()))
            .collect()
    }

    #[tokio::test]
    async fn a_copy_follows_the_homes_labels_and_removals() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let api = origin_repo(td.path(), "api");
        let whole = definition(vec![repo("app", Some(&app)), repo("api", Some(&api))]);
        ensure_project_under(&sup, whole.clone(), &root)
            .await
            .unwrap();

        // On the home: the first repository is labelled, the second removed.
        let mut def = whole.clone();
        def.repos[0].label = Some("Frontend".into());
        def.repos.truncate(1);
        let ws = ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();

        assert_eq!(
            copied(&ws, &def),
            [(clone_of(&root, &def, 0), Some("Frontend".to_string()))]
        );
        assert_eq!(
            sup.workspace.project_repo_paths(&def.project_id),
            [clone_of(&root, &def, 0)]
        );
        // Nothing here used it, so its clone went with it.
        assert!(!copy_root(&root, &whole.project_id)
            .join(&whole.repos[1].id)
            .exists());

        // The home has it again: it is cloned again.
        let ws = ensure_project_under(&sup, whole.clone(), &root)
            .await
            .unwrap();
        assert_eq!(copied(&ws, &whole).len(), 2);
        assert!(clone_of(&root, &whole, 1).join("README.md").is_file());
    }

    /// A repository the home dropped while an agent here has a checkout of it
    /// stays the project's for that agent, is given to no new workspace, and
    /// leaves once that agent is gone.
    #[tokio::test]
    async fn a_removed_repository_an_agent_here_uses_stays_until_the_agent_is_gone() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let api = origin_repo(td.path(), "api");
        let whole = definition(vec![repo("app", Some(&app)), repo("api", Some(&api))]);
        ensure_project_under(&sup, whole.clone(), &root)
            .await
            .unwrap();
        // An agent started in the first repository, the one the home removes.
        let app_clone = clone_of(&root, &whole, 0);
        let mut agent = crate::workspace::new_agent_record(
            "dolomites".into(),
            "a".into(),
            "claude".into(),
            crate::workspace::tests::mk_repo(app_clone.to_str().unwrap()),
            "".into(),
            crate::workspace::AgentView::Custom,
        );
        sup.workspace.add_agent(&mut agent).unwrap();

        let mut def = whole.clone();
        def.repos.remove(0);
        let ws = ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();

        // Still listed with the project, behind the one the home names, so
        // the agent on it keeps the project's name and settings.
        let listed: Vec<PathBuf> = copied(&ws, &whole).into_iter().map(|(p, _)| p).collect();
        assert_eq!(listed, [clone_of(&root, &whole, 1), app_clone.clone()]);
        // But no new workspace is given it, and none can be started on it:
        // nothing new borrows from it, so it can leave.
        assert_eq!(
            sup.workspace.project_repo_paths(&whole.project_id),
            [clone_of(&root, &whole, 1)]
        );
        let mut late = crate::workspace::new_agent_record(
            "matterhorn".into(),
            "b".into(),
            "claude".into(),
            crate::workspace::tests::mk_repo(app_clone.to_str().unwrap()),
            "".into(),
            crate::workspace::AgentView::Custom,
        );
        let refused = sup.workspace.add_agent(&mut late).unwrap_err().to_string();
        assert!(
            refused.contains("no longer part of this project"),
            "{refused}"
        );
        // A copy asked to describe the project names what the home does.
        let (_, _, described) = sup.workspace.project_outline(&whole.project_id).unwrap();
        assert_eq!(described.len(), 1);
        assert_eq!(
            ws.agents.iter().map(|a| &a.id).collect::<Vec<_>>(),
            [&agent.id]
        );

        // Its clone is what that agent's checkout borrows from.
        assert!(app_clone.is_dir());

        sup.workspace.remove_agent(&agent.id).unwrap();
        let ws = ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();
        assert_eq!(copied(&ws, &def), [(clone_of(&root, &def, 0), None)]);
        assert!(!app_clone.exists());
    }

    /// A saved run's tree shares the git objects of the clone it was started
    /// from, with no agent checkout left to say so.
    #[tokio::test]
    async fn a_removed_repository_a_saved_workflow_run_borrows_from_stays() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let api = origin_repo(td.path(), "api");
        let whole = definition(vec![repo("app", Some(&app)), repo("api", Some(&api))]);
        ensure_project_under(&sup, whole.clone(), &root)
            .await
            .unwrap();
        let api_clone = clone_of(&root, &whole, 1);
        sup.workspace
            .db()
            .lock()
            .execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES ('r1','n','{}','t',?1,?2,'/d','wf/x','sha','done','{}','{}',0,0)",
                [&whole.project_id, &api_clone.to_string_lossy().to_string()],
            )
            .unwrap();

        let mut def = whole.clone();
        def.repos.truncate(1);
        ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();
        assert!(api_clone.is_dir());
        assert_eq!(
            sup.workspace.project_repo_paths(&def.project_id),
            [clone_of(&root, &def, 0)]
        );

        // The home takes it back: the same clone, for new workspaces again.
        ensure_project_under(&sup, whole.clone(), &root)
            .await
            .unwrap();
        assert_eq!(
            sup.workspace.project_repo_paths(&whole.project_id),
            [clone_of(&root, &whole, 0), api_clone.clone()]
        );
        ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();

        // The run is gone: so is what it borrowed from, on the next sync.
        sup.workspace
            .db()
            .lock()
            .execute("DELETE FROM wf_run WHERE id = 'r1'", [])
            .unwrap();
        ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();
        assert!(!api_clone.exists());
    }

    /// What is left of a path once its repository has gone from a copy: not
    /// a folder to pin as a project of this host's own.
    #[test]
    fn a_clone_with_no_record_is_not_added_as_a_project() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = host(td.path());
        let stale = copy_root(&crate::workspace::projects_root().unwrap(), "p1")
            .join("r1")
            .join("app");

        let refused = sup
            .workspace
            .project_id_for_repo(&stale.to_string_lossy())
            .unwrap_err()
            .to_string();

        assert!(
            refused.contains("no longer part of its project"),
            "{refused}"
        );
        assert!(sup.workspace.current().unwrap().projects.is_empty());
    }

    #[tokio::test]
    async fn a_deleted_copy_takes_its_clones_and_no_other_projects() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let gone = definition(vec![repo("app", Some(&app))]);
        let kept = definition(vec![repo("app", Some(&app))]);
        for def in [&gone, &kept] {
            ensure_project_under(&sup, def.clone(), &root)
                .await
                .unwrap();
        }

        sup.workspace.delete_project(&gone.project_id, &[]).unwrap();
        prune_copy_under(&sup.workspace, &root, &gone.project_id).await;

        assert!(!copy_root(&root, &gone.project_id).exists());
        assert!(clone_of(&root, &kept, 0).is_dir());
    }

    #[tokio::test]
    async fn clones_stay_when_their_records_cannot_be_read() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let def = definition(vec![repo("app", Some(&app))]);
        ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();
        sup.workspace
            .db()
            .lock()
            .execute_batch("ALTER TABLE repos RENAME TO repos_unreadable")
            .unwrap();

        prune_copy_under(&sup.workspace, &root, &def.project_id).await;

        assert!(clone_of(&root, &def, 0).is_dir());
    }

    /// On the home a repository that joins a project keeps the date it was
    /// pinned, so it can come first; its clone here is the newest.
    #[tokio::test]
    async fn a_copy_keeps_the_homes_order_whenever_each_repository_was_cloned() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let core = origin_repo(td.path(), "core");
        let mut def = definition(vec![repo("app", Some(&app))]);
        ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();

        // The home's project gains a repository ahead of the one it had.
        def.repos.insert(0, repo("core", Some(&core)));
        let ws = ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();
        let in_order = [clone_of(&root, &def, 0), clone_of(&root, &def, 1)];
        assert_eq!(sup.workspace.project_repo_paths(&def.project_id), in_order);
        // The project list's first is the primary a new workspace starts in.
        let listed: Vec<PathBuf> = copied(&ws, &def).into_iter().map(|(p, _)| p).collect();
        assert_eq!(listed, in_order);

        // And when two it already has change places.
        def.repos.swap(0, 1);
        ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();
        assert_eq!(
            sup.workspace.project_repo_paths(&def.project_id),
            [clone_of(&root, &def, 0), clone_of(&root, &def, 1)]
        );
    }

    #[tokio::test]
    async fn a_repository_the_home_moved_to_another_project_is_copied_for_both() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let old = definition(vec![repo("app", Some(&app))]);
        ensure_project_under(&sup, old.clone(), &root)
            .await
            .unwrap();

        // The same repository, by its id, now under another project.
        let mut new = definition(vec![old.repos[0].clone()]);
        new.name = "Elsewhere".into();
        let ws = ensure_project_under(&sup, new.clone(), &root)
            .await
            .unwrap();

        assert!(ws
            .projects
            .iter()
            .any(|p| p.path == clone_of(&root, &old, 0)));
        assert!(ws
            .projects
            .iter()
            .any(|p| p.path == clone_of(&root, &new, 0)));
    }

    #[test]
    fn a_definition_carries_no_credentials() {
        for (origin, told) in [
            (
                "https://alice:token@example.com/org/repo.git",
                "https://example.com/org/repo.git",
            ),
            (
                "https://token@example.com/org/repo.git",
                "https://example.com/org/repo.git",
            ),
            (
                "ssh://git:secret@example.com/org/repo.git",
                "ssh://git@example.com/org/repo.git",
            ),
            (
                "https://example.com/org/a@b.git",
                "https://example.com/org/a@b.git",
            ),
            (
                "ssh://git@github.com/fwdai/fletch",
                "ssh://git@github.com/fwdai/fletch",
            ),
            (
                "https://git.example/repo.git?access_token=secret",
                "https://git.example/repo.git",
            ),
            (
                "https://alice@git.example/repo.git#token=secret",
                "https://git.example/repo.git",
            ),
            (
                "git@github.com:fwdai/fletch.git",
                "git@github.com:fwdai/fletch.git",
            ),
        ] {
            assert_eq!(without_credentials(origin), told);
        }
    }

    #[tokio::test]
    async fn a_copy_is_not_edited_where_it_runs() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let app = origin_repo(td.path(), "app");
        let def = definition(vec![repo("app", Some(&app))]);
        ensure_project_under(&sup, def.clone(), &root)
            .await
            .unwrap();
        let clone = clone_of(&root, &def, 0);
        let other = origin_repo(td.path(), "other");
        let ws = &sup.workspace;
        const REFUSAL: &str = "This project belongs to MacBook. Change it there.";

        let refused = [
            ws.rename_project(&def.project_id, "Mine now").map(drop),
            ws.attach_repo_to_project(&def.project_id, &other).map(drop),
            ws.set_repo_label(&clone, "Frontend").map(drop),
            ws.relocate_repo(&clone, &other).map(drop),
            ws.remove_workspace_repo(&clone).map(drop),
            ws.detach_repo_from_project(&def.project_id, &clone)
                .map(drop),
        ];
        for result in refused {
            assert_eq!(result.unwrap_err().to_string(), REFUSAL);
        }
        // A project this host owns is edited as before.
        let own = ws.add_workspace_repo(other.clone()).unwrap();
        let own_id = &own
            .projects
            .iter()
            .find(|p| p.path == other)
            .unwrap()
            .project_id;
        ws.rename_project(own_id, "Renamed").unwrap();
        // Nor can a copy's repository be moved into a project of this host's.
        assert_eq!(
            ws.attach_repo_to_project(own_id, &clone)
                .unwrap_err()
                .to_string(),
            REFUSAL
        );
    }

    #[test]
    fn a_github_ssh_remote_is_cloned_with_this_hosts_login() {
        assert_eq!(
            clone_spec("git@github.com:fwdai/fletch.git"),
            "fwdai/fletch"
        );
        assert_eq!(
            clone_spec("ssh://git@github.com/fwdai/fletch"),
            "fwdai/fletch"
        );
        assert_eq!(
            clone_spec("https://github.com/fwdai/fletch.git"),
            "https://github.com/fwdai/fletch.git"
        );
        assert_eq!(
            clone_spec("git@gitlab.com:a/b.git"),
            "git@gitlab.com:a/b.git"
        );
    }
}

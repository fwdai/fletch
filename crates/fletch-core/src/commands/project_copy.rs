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
use crate::workspace::{ProjectDefinition, ProjectHome, RepoDefinition, Workspace};

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
    let mut taken = std::collections::HashSet::new();
    for (path, label) in repos {
        let folder = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::InvalidPath(path.display().to_string()))?;
        described.push(RepoDefinition {
            dir: distinct_dir(folder, &mut taken),
            // A repository whose origin cannot be read is described without
            // one: the device asked to clone it says why it cannot.
            origin: crate::git::origin_url(&path).await.ok().flatten(),
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

/// `folder`, or `folder-2`, `folder-3`, … when an earlier repository of the
/// project already has that name. Two repositories of one project may share a
/// folder name on their home (`client/app` and `server/app`) but not in a
/// copy, where they sit side by side; names that differ only by case count as
/// the same, since the device that makes the copy may not tell them apart.
fn distinct_dir(folder: &str, taken: &mut std::collections::HashSet<String>) -> String {
    let mut dir = folder.to_string();
    let mut n = 2;
    while !taken.insert(dir.to_lowercase()) {
        dir = format!("{folder}-{n}");
        n += 1;
    }
    dir
}

/// One `ensure_project` at a time: two spawns into a project this host does
/// not have yet would otherwise clone into the same folder at once.
static ENSURING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Make sure this host can run workspaces of `def`, and answer with the
/// project list.
///
/// A project this host already has, its own or a copy, costs nothing beyond
/// bringing a copy's name up to date and cloning a repository the home has
/// added since. Otherwise each repository is cloned from its `origin` and the
/// project is recorded as a copy.
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
        let have = workspace.project_repo_paths(&def.project_id);
        let missing: Vec<(usize, &RepoDefinition, PathBuf)> = def
            .repos
            .iter()
            .enumerate()
            .map(|(i, repo)| (i, repo, root.join(&repo.dir)))
            .filter(|(_, _, target)| !have.contains(target))
            .collect();
        for (_, repo, _) in &missing {
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
        for (_, repo, _) in &missing {
            let origin = repo.origin.as_deref().expect("checked above");
            let staged = staging.join(&repo.dir);
            if let Err(e) = crate::github::repo_clone(clone_spec(origin), &staged).await {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                return Err(e);
            }
        }

        let mut cloned = Vec::with_capacity(missing.len());
        for (order, repo, target) in missing {
            // Not recorded, so nothing refers to it: what an earlier call
            // moved into place before it failed to record the project.
            if target.exists() {
                tokio::fs::remove_dir_all(&target).await?;
            }
            tokio::fs::create_dir_all(&root).await?;
            tokio::fs::rename(staging.join(&repo.dir), &target).await?;
            cloned.push((target, repo.label.clone(), order));
        }
        let _ = tokio::fs::remove_dir_all(&staging).await;
        workspace.record_copy(&def, &cloned)?;
        workspace.detect_icon(&def.project_id);
    }

    Ok(workspace.current().expect("workspace initialized"))
}

/// Where a copy's repositories are cloned: `<projects root>/<project id>/repos`.
fn copy_root(projects_root: &Path, project_id: &str) -> PathBuf {
    projects_root.join(project_id).join("repos")
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
/// repository's folder become path components under [`copy_root`], so neither
/// may be anything but one plain component.
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
        // Case-insensitively: the default macOS filesystem would put `App`
        // and `app` in one folder.
        if !seen.insert(dir.to_lowercase()) {
            return Err(Error::InvalidPath(format!(
                "two repositories share the folder name {dir}"
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
    use crate::workspace::WorkspaceManager;
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

    fn repo(dir: &str, origin: Option<&Path>) -> RepoDefinition {
        RepoDefinition {
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
        let clones = copy_root(&root, &def.project_id);
        assert_eq!(
            refs.iter().map(|p| p.path.clone()).collect::<Vec<_>>(),
            [clones.join("app"), clones.join("api")]
        );
        assert!(clones.join("app").join("README.md").is_file());
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
        let clone = copy_root(&root, &def.project_id).join("app");
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
        assert_eq!(
            def.repos,
            [RepoDefinition {
                dir: "app".into(),
                origin: Some(url(&upstream)),
                label: None,
            }]
        );

        // A copy: its home is whoever it came from, not this host.
        let theirs = definition(vec![repo("upstream", Some(&upstream))]);
        ensure_project_under(&sup, theirs.clone(), &root)
            .await
            .unwrap();
        let again = project_definition_impl(&ctx, &sup, &theirs.project_id)
            .await
            .unwrap();
        assert_eq!(again.home, theirs.home);
        assert_eq!(again.repos.len(), 1);
        assert_eq!(again.repos[0].dir, "upstream");
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
        assert!(!copy_root(&root, &def.project_id).join("app").exists());
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
        let target = copy_root(&root, &def.project_id).join("app");
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
    async fn repositories_sharing_a_folder_name_get_distinct_ones() {
        let td = tempfile::tempdir().unwrap();
        let (sup, root) = host(td.path());
        let (ctx, _sink, _db) = crate::host::ctx::test_ctx();
        ctx.set_host_id("dGhpcy1ob3N0".into());
        let client = origin_repo(&td.path().join("client"), "app");
        let server = origin_repo(&td.path().join("server"), "App");
        sup.workspace.add_workspace_repo(client).unwrap();
        let project_id = sup.current_workspace().unwrap().projects[0]
            .project_id
            .clone();
        sup.workspace
            .attach_repo_to_project(&project_id, &server)
            .unwrap();

        let def = project_definition_impl(&ctx, &sup, &project_id)
            .await
            .unwrap();
        let dirs: Vec<_> = def.repos.iter().map(|r| r.dir.as_str()).collect();
        assert_eq!(dirs, ["app", "App-2"]);

        // And a definition that names two folders differing only by case is
        // refused rather than cloned into one.
        let app = origin_repo(td.path(), "one");
        let clash = definition(vec![repo("app", Some(&app)), repo("App", Some(&app))]);
        assert!(ensure_project_under(&sup, clash, &root).await.is_err());
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
        let clone = copy_root(&root, &def.project_id).join("app");
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

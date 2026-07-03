//! New Project flows: clone an existing GitHub repo, or create a fresh repo
//! locally and on GitHub. Both terminate by handing a local path to
//! `WorkspaceManager::add_workspace_repo` (done by the command layer).
//!
//! All GitHub work goes through the API client (see `github/`) — the same
//! trusted path the PR flow already uses.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::{git, github as gh};

/// Derive the repository name from a clone spec. Accepts:
///   - `owner/repo`
///   - `https://github.com/owner/repo` (optionally `.git`, trailing slash)
///   - `git@github.com:owner/repo.git`
///
/// Returns just the repo segment (`repo`), which becomes the local dir name.
pub fn repo_name_from_spec(spec: &str) -> Result<String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(Error::InvalidPath("empty repository".into()));
    }

    // Take the last path-ish segment, after either `/` or `:` (ssh form).
    let tail = spec
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(spec);

    let name = tail.strip_suffix(".git").unwrap_or(tail).trim();
    if name.is_empty() {
        return Err(Error::InvalidPath(format!("cannot parse repo name from: {spec}")));
    }
    validate_new_name(name)?;
    Ok(name.to_string())
}

/// Validate a name the user typed for a brand-new repo. GitHub allows letters,
/// digits, `.`, `-`, `_`; no slashes, spaces, or path traversal.
pub fn validate_new_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::InvalidPath("project name is required".into()));
    }
    if name == "." || name == ".." {
        return Err(Error::InvalidPath("invalid project name".into()));
    }
    // A leading hyphen is read as a flag by `gh` (e.g. `--push`), and GitHub
    // disallows it anyway — reject before it can reach the CLI.
    if name.starts_with('-') {
        return Err(Error::InvalidPath(
            "project name may not start with '-'".into(),
        ));
    }
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        Ok(())
    } else {
        Err(Error::InvalidPath(
            "project name may only contain letters, digits, '.', '-', '_'".into(),
        ))
    }
}

/// Compute the local target path and reject a collision before doing any work.
fn resolve_target(dest_parent: &Path, name: &str) -> Result<PathBuf> {
    let target = dest_parent.join(name);
    if target.exists() {
        return Err(Error::InvalidPath(format!(
            "a folder already exists at {}",
            target.display()
        )));
    }
    Ok(target)
}

/// Clone `spec` into `dest_parent/<repo-name>` and return the local path.
pub async fn clone(spec: &str, dest_parent: &Path) -> Result<PathBuf> {
    let name = repo_name_from_spec(spec)?;
    let target = resolve_target(dest_parent, &name)?;
    // `gh` creates `target` and clones into it; if the clone fails partway
    // (e.g. a dropped connection) the partial dir would otherwise make
    // `resolve_target` reject every retry with "a folder already exists".
    // Remove it on failure — same self-heal as `create`.
    if let Err(e) = gh::repo_clone(spec, &target).await {
        // Async remove: a large partial clone could otherwise block the
        // executor thread while tearing down hundreds of files.
        let _ = tokio::fs::remove_dir_all(&target).await;
        return Err(e);
    }
    Ok(target)
}

/// Ensure the folder at `path` is a git repository so agents can operate in
/// worktrees, commit, and track history — the progressive-disclosure ramp for
/// users who've never heard of git. A plain folder is initialized with an
/// initial commit (worktrees can't fork a repo with no HEAD); a folder nested
/// inside an existing repository is rejected with a pointer to the actual
/// root, since silently initializing a nested repo would split its history.
pub async fn ensure_git_repo(path: &Path) -> Result<()> {
    if path.join(".git").exists() {
        return Ok(());
    }
    if let Some(root) = git_toplevel(path).await {
        return Err(Error::InvalidPath(format!(
            "{} is inside the git repository at {root} — add that folder instead",
            path.display(),
        )));
    }
    git::init_repo(path).await?;
    git::commit_initial(path).await
}

/// The repository root containing `path`, or `None` when `path` is not inside
/// any git repository (the normal case for a folder we're about to init).
async fn git_toplevel(path: &Path) -> Option<String> {
    let out = crate::git_dist::command(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!root.is_empty()).then_some(root)
}

/// Create a new local repo at `dest_parent/<name>` (seeded with a README and
/// an initial commit) and return the local path. With `publish` it is also
/// created on GitHub and pushed; without (no GitHub connection yet) it stays
/// local — the git panel offers "Publish to GitHub" later.
pub async fn create(
    name: &str,
    dest_parent: &Path,
    private: bool,
    description: Option<&str>,
    publish: bool,
) -> Result<PathBuf> {
    let name = name.trim();
    validate_new_name(name)?;
    let target = resolve_target(dest_parent, name)?;

    std::fs::create_dir_all(&target)?;

    // Once the directory exists, any later failure must remove it — otherwise
    // the orphaned folder makes `resolve_target` reject every retry.
    let result = async {
        git::init_repo(&target).await?;

        let readme = target.join("README.md");
        let body = match description.map(str::trim).filter(|d| !d.is_empty()) {
            Some(desc) => format!("# {name}\n\n{desc}\n"),
            None => format!("# {name}\n"),
        };
        std::fs::write(&readme, body)?;

        git::commit_all(&target, "Initial commit").await?;
        if publish {
            gh::repo_create_and_push(&target, name, private, description).await?;
        }
        Ok::<(), Error>(())
    }
    .await;

    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&target);
        return Err(e);
    }
    Ok(target)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_from_owner_repo() {
        assert_eq!(repo_name_from_spec("octocat/hello").unwrap(), "hello");
    }

    #[test]
    fn name_from_https_url() {
        assert_eq!(
            repo_name_from_spec("https://github.com/octocat/Hello-World").unwrap(),
            "Hello-World"
        );
    }

    #[test]
    fn name_from_https_url_with_git_suffix_and_slash() {
        assert_eq!(
            repo_name_from_spec("https://github.com/octocat/Hello-World.git/").unwrap(),
            "Hello-World"
        );
    }

    #[test]
    fn name_from_ssh_url() {
        assert_eq!(
            repo_name_from_spec("git@github.com:octocat/my_repo.git").unwrap(),
            "my_repo"
        );
    }

    #[test]
    fn name_rejects_empty() {
        assert!(repo_name_from_spec("   ").is_err());
    }

    #[test]
    fn name_rejects_illegal_chars() {
        // A spec whose tail contains spaces is not a legal repo name.
        assert!(repo_name_from_spec("owner/bad name").is_err());
    }

    /// The GitHub-unaware path: an empty non-repo folder becomes a repo WITH
    /// a HEAD (worktrees can't fork without one), and adopting it again is a
    /// no-op rather than an error.
    #[tokio::test]
    async fn ensure_git_repo_initializes_folder_and_is_idempotent() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("my-notes");
        std::fs::create_dir(&dir).unwrap();

        ensure_git_repo(&dir).await.unwrap();

        assert!(dir.join(".git").exists());
        let head = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(
            head.status.success(),
            "an adopted folder must have a HEAD commit: {}",
            String::from_utf8_lossy(&head.stderr),
        );

        ensure_git_repo(&dir).await.unwrap();
    }

    /// A folder nested inside an existing repository must be rejected with a
    /// pointer to the real root — silently initializing a nested repo would
    /// split its history.
    #[tokio::test]
    async fn ensure_git_repo_rejects_folder_inside_existing_repo() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        assert!(std::process::Command::new("git")
            .current_dir(&repo)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        let nested = repo.join("src");
        std::fs::create_dir(&nested).unwrap();

        let err = ensure_git_repo(&nested).await.unwrap_err().to_string();
        assert!(
            err.contains("inside the git repository"),
            "unexpected error: {err}",
        );
        assert!(!nested.join(".git").exists());
    }

    #[test]
    fn validate_accepts_typical_names() {
        for ok in ["my-app", "my_app", "App.2", "x"] {
            assert!(validate_new_name(ok).is_ok(), "{ok} should be valid");
        }
    }

    #[test]
    fn validate_rejects_bad_names() {
        for bad in ["", "  ", ".", "..", "a/b", "a b", "a:b", "café", "-foo", "--push"] {
            assert!(validate_new_name(bad).is_err(), "{bad} should be invalid");
        }
    }

    #[test]
    fn resolve_target_rejects_existing() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        std::fs::create_dir(parent.join("taken")).unwrap();
        assert!(resolve_target(parent, "taken").is_err());
        assert_eq!(
            resolve_target(parent, "fresh").unwrap(),
            parent.join("fresh")
        );
    }
}

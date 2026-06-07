//! New Project flows: clone an existing GitHub repo, or create a fresh repo
//! locally and on GitHub. Both terminate by handing a local path to
//! `WorkspaceManager::add_workspace_repo` (done by the command layer).
//!
//! All GitHub work goes through the `gh` CLI (see `gh.rs`) — the same trusted
//! path the PR flow already uses.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::{gh, git};

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
    gh::repo_clone(spec, &target).await?;
    Ok(target)
}

/// Create a new local repo at `dest_parent/<name>` (seeded with a README and an
/// initial commit), publish it to GitHub, and return the local path.
pub async fn create(
    name: &str,
    dest_parent: &Path,
    private: bool,
    description: Option<&str>,
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
        gh::repo_create_and_push(&target, name, private, description).await?;
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

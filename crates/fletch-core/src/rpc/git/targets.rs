use std::path::PathBuf;

use serde_json::{json, Value};

use super::GitDispatcher;
use crate::rpc::Response;

/// `subdir` is `None` for dispatchers built without `with_repos`; consumers then
/// fall back to the primary.
pub(super) struct Target {
    pub(super) subdir: Option<String>,
    pub(super) cwd: PathBuf,
    /// The user's own repository `cwd` was cloned from: where a publish reads
    /// its signing config and signs (`git::publish`). Never `cwd`, which the
    /// agent writes.
    pub(super) source_repo: PathBuf,
    pub(super) base_branch: String,
    /// See `RepoTarget::owned_branches`. A branch created this session joins
    /// it once the host has recorded it and the dispatcher was rebuilt; until
    /// then a force on it fails closed.
    pub(super) owned_branches: Vec<String>,
}

impl Target {
    /// What `caps::refuses_publish` judges a branch against.
    pub(super) fn records(&self) -> crate::rpc::caps::BranchRecords<'_> {
        crate::rpc::caps::BranchRecords {
            base: &self.base_branch,
            owned: &self.owned_branches,
        }
    }

    pub(super) fn owns(&self, branch: &str) -> bool {
        self.owned_branches
            .iter()
            .any(|own| crate::rpc::caps::same_branch(own, branch))
    }
}

impl GitDispatcher {
    pub(super) fn target(&self, id: &str, args: &Value) -> std::result::Result<Target, Response> {
        let requested = args
            .get("repo")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match requested {
            None => Ok(Target {
                subdir: self.default_subdir.clone(),
                cwd: self.cwd.clone(),
                source_repo: self.source_repo.clone(),
                base_branch: self.base_branch.clone(),
                owned_branches: self.owned_branches.clone(),
            }),
            Some(name) => match self.repos.get(name) {
                Some(repo) => Ok(Target {
                    subdir: Some(name.to_string()),
                    cwd: repo.cwd.clone(),
                    source_repo: repo.source_repo.clone(),
                    base_branch: repo.base_branch.clone(),
                    owned_branches: repo.owned_branches.clone(),
                }),
                None => {
                    let mut known: Vec<&str> = self.repos.keys().map(String::as_str).collect();
                    known.sort_unstable();
                    Err(Response::err(
                        id,
                        format!(
                            "unknown repo {name:?}; tracked checkouts: {}",
                            known.join(", ")
                        ),
                    ))
                }
            },
        }
    }
}

pub(super) fn with_repo(mut payload: Value, subdir: &Option<String>) -> Value {
    if let Some(s) = subdir {
        payload["repo"] = json!(s);
    }
    payload
}

pub(super) fn approval_repo<'a>(subdir: Option<&'a str>, primary: Option<&str>) -> Option<&'a str> {
    subdir.filter(|s| Some(*s) != primary)
}

#[cfg(test)]
#[path = "tests/targets.rs"]
mod tests;

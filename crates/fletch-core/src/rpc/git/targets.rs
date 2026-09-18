use std::path::PathBuf;

use serde_json::{json, Value};

use super::GitDispatcher;
use crate::rpc::Response;

/// `subdir` is `None` for dispatchers built without `with_repos`; consumers then
/// fall back to the primary.
pub(super) struct Target {
    pub(super) subdir: Option<String>,
    pub(super) cwd: PathBuf,
    pub(super) base_branch: String,
    /// The force-push anchor. `None` until the host has recorded the branch and the
    /// dispatcher was rebuilt, which fails a force closed.
    pub(super) own_branch: Option<String>,
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
                base_branch: self.base_branch.clone(),
                own_branch: self.own_branch.clone(),
            }),
            Some(name) => match self.repos.get(name) {
                Some((cwd, base, own)) => Ok(Target {
                    subdir: Some(name.to_string()),
                    cwd: cwd.clone(),
                    base_branch: base.clone(),
                    own_branch: own.clone(),
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

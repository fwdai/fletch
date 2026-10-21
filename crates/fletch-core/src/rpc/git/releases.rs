//! Release reads: what shipped last and what a tag carries. Agents on
//! release work read these through `gh release list` / `view` in a dozen
//! sessions; publishing a release stays with CI and the user. Not gated by
//! `AgentCaps` and never an `EVENT_ACTION_DONE`, like the PR reads.

use serde_json::Value;

use super::pr_reads::{json_ok, positive_u32_arg};
use super::GitDispatcher;
use crate::rpc::Response;

const RELEASE_LIST_DEFAULT_LIMIT: u32 = 20;

impl GitDispatcher {
    /// The repository's releases, newest first, notes left out;
    /// `args.limit` default 20, max 100.
    pub(super) async fn release_list(&self, id: &str, args: &Value) -> Response {
        let limit = match positive_u32_arg(args, "limit") {
            Ok(n) => n
                .unwrap_or(RELEASE_LIST_DEFAULT_LIMIT)
                .min(crate::github::RELEASE_LIST_MAX),
            Err(e) => return Response::err(id, format!("release_list: {e}")),
        };
        let t = match self.pr_read_target(id, "release_list", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        match crate::github::release_list(&t.source_repo, limit).await {
            Ok(Some(releases)) => json_ok(id, "release_list", &releases),
            Ok(None) => Response::err(
                id,
                "release_list: GitHub did not list this repository's releases — it may not be \
                 visible to the host's GitHub account; try again shortly",
            ),
            Err(e) => Response::err(id, format!("release_list: {e}")),
        }
    }

    /// One release with its notes and assets: `args.tag`, or the latest
    /// published release without it.
    pub(super) async fn release_view(&self, id: &str, args: &Value) -> Response {
        let tag = match tag_arg(args) {
            Ok(tag) => tag,
            Err(e) => return Response::err(id, format!("release_view: {e}")),
        };
        let t = match self.pr_read_target(id, "release_view", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        match crate::github::release_view(&t.source_repo, tag.as_deref()).await {
            Ok(Some(release)) => json_ok(id, "release_view", &release),
            Ok(None) => Response::err(
                id,
                match tag {
                    Some(tag) => format!(
                        "release_view: no release for tag {tag:?} in this repository, or the \
                         repository is not visible to the host's GitHub account"
                    ),
                    None => "release_view: this repository has no published release yet (drafts \
                             and prereleases are listed by release_list), or it is not visible \
                             to the host's GitHub account"
                        .to_string(),
                },
            ),
            Err(e) => Response::err(id, format!("release_view: {e}")),
        }
    }
}

/// `args.tag`, trimmed; `Ok(None)` when absent. A tag reaches the URL path
/// as given, so only what git itself allows in a tag name is accepted — no
/// whitespace, no option-like leading dash, no `..`.
pub(super) fn tag_arg(args: &Value) -> Result<Option<String>, String> {
    let tag = match args.get("tag") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => s.trim(),
        Some(other) => return Err(format!("`tag` must be a string, got {other}")),
    };
    if !is_well_formed_tag(tag) {
        return Err(format!(
            "`tag` must be a tag name like \"v1.2.0\", got {tag:?}"
        ));
    }
    Ok(Some(tag.to_string()))
}

/// Git's own rules for a tag name (`git check-ref-format`), so every tag a
/// release can carry — `v1.0.0@rc1`, `releases/2026.10` — is accepted and
/// what git itself refuses is refused here: control characters, space and
/// `~ ^ : ? * [ \`, a `..` or `@{` sequence, a lone `@`, a component that
/// starts with `.` or ends in `.lock`, a trailing `.` or `/`. A leading `-`
/// is refused as well, as every branch argument is. The tag is sent as one
/// encoded path segment, so what passes here cannot change the request.
pub(super) fn is_well_formed_tag(tag: &str) -> bool {
    if tag.is_empty()
        || tag == "@"
        || tag.starts_with('-')
        || tag.starts_with('/')
        || tag.ends_with('/')
        || tag.ends_with('.')
        || tag.contains("..")
        || tag.contains("@{")
        || tag.contains("//")
    {
        return false;
    }
    if tag.chars().any(|c| {
        c.is_ascii_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    }) {
        return false;
    }
    tag.split('/')
        .all(|component| !component.starts_with('.') && !component.ends_with(".lock"))
}

#[cfg(test)]
#[path = "tests/releases.rs"]
mod tests;

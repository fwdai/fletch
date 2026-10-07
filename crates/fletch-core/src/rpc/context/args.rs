use serde::Deserialize;
use serde_json::Value;

use crate::context::{AssertionKind, ContextError, Domain, EntityKind, Rel, Stance, Supersede};
use crate::rpc::Response;

/// A statement is one sentence the next agent reads in a list; the rationale
/// is the paragraph behind it. Anything longer belongs in a doc the entity's
/// `paths` point at.
pub(super) const MAX_STATEMENT: usize = 300;
pub(super) const MAX_RATIONALE: usize = 1000;
pub(super) const MAX_SUMMARY: usize = 600;
pub(super) const MAX_NAME: usize = 120;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GetArgs {
    #[serde(default)]
    pub(super) entities: Vec<String>,
    #[serde(default)]
    pub(super) query: Option<String>,
    #[serde(default)]
    pub(super) paths: Vec<String>,
    #[serde(default)]
    pub(super) include_history: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecordEntityArgs {
    pub(super) slug: String,
    #[serde(default)]
    pub(super) kind: Option<EntityKind>,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) summary: String,
    #[serde(default)]
    pub(super) aliases: Vec<String>,
    #[serde(default)]
    pub(super) paths: Vec<String>,
    /// Present: a revision of that entity rather than a new one.
    #[serde(default)]
    pub(super) id: Option<String>,
    #[serde(default)]
    pub(super) relates: Vec<RelatesArg>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RelatesArg {
    pub(super) rel: Rel,
    pub(super) to: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecordDecisionArgs {
    #[serde(default)]
    pub(super) kind: Option<AssertionKind>,
    pub(super) domain: Domain,
    #[serde(default)]
    pub(super) stance: Option<Stance>,
    #[serde(default)]
    pub(super) statement: String,
    #[serde(default)]
    pub(super) rationale: String,
    #[serde(default)]
    pub(super) about: Vec<String>,
    #[serde(default)]
    pub(super) paths: Vec<String>,
    #[serde(default)]
    pub(super) supersedes: Option<Supersede>,
    #[serde(default)]
    pub(super) contradicts: Vec<String>,
    /// The user's own words, verbatim. Fletch looks for them in the user's
    /// turns of this workspace (`context::trust`); found, the quote *is* the
    /// statement and the record lands confirmed as the user's; not found, the
    /// call is refused.
    #[serde(default)]
    pub(super) user_quote: Option<String>,
    /// The checkout (repo subdir) this decision is about. Optional with one
    /// checkout; required with several.
    #[serde(default)]
    pub(super) repo: Option<String>,
    /// The second step of the conflict protocol: record alongside the head
    /// `classify` found, without replacing or contradicting it.
    #[serde(default)]
    pub(super) coexists: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinkArgs {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) rel: Rel,
    #[serde(default)]
    pub(super) remove: bool,
}

/// A missing `args` arrives as JSON null, same as `{}`.
pub(super) fn parse_args<T: Default + serde::de::DeserializeOwned>(
    args: &Value,
) -> Result<T, String> {
    if args.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(args.clone()).map_err(|e| e.to_string())
}

pub(super) fn parse_required<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, String> {
    if args.is_null() {
        return Err("`args` are required".into());
    }
    serde_json::from_value(args.clone()).map_err(|e| e.to_string())
}

/// Trimmed, non-empty, within `max` characters.
pub(super) fn text(field: &str, value: &str, max: usize) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("`{field}` is required"));
    }
    if value.chars().count() > max {
        return Err(format!(
            "`{field}` exceeds {max} characters; keep it short and point at a file for the rest"
        ));
    }
    Ok(value.to_string())
}

pub(super) fn clean_list(v: &[String]) -> Vec<String> {
    v.iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The store's error, plus what to do about it.
pub(super) fn explain(e: ContextError) -> String {
    match e {
        ContextError::SlugTaken(slug) => format!(
            "slug `{slug}` is already taken — use it as-is in `about` / `entities`, pick another slug, or pass that entity's `id` to revise it"
        ),
        ContextError::VisionExists => {
            format!("{e} — pass the existing vision's `id` to revise it")
        }
        ContextError::UnknownEntity(_) | ContextError::AmbiguousEntity(..) => format!(
            "{e} — record it with context_record_entity first, or use a slug from the index"
        ),
        ContextError::NotHead(_) => format!(
            "{e} — call context_get with include_history to find the current head, and supersede that"
        ),
        other => other.to_string(),
    }
}

fn refuse(id: &str, op: &str, msg: String) -> Response {
    Response::err(id, format!("{op}: {msg}"))
}

/// A JSON payload as the response's `stdout`.
pub(super) fn reply(id: &str, op: &str, result: Result<Value, String>) -> Response {
    let stdout =
        result.and_then(|payload| serde_json::to_string(&payload).map_err(|e| e.to_string()));
    match stdout {
        Ok(stdout) => Response::ok(id, 0, stdout, String::new()),
        Err(msg) => refuse(id, op, msg),
    }
}

/// Markdown as the response's `stdout`.
pub(super) fn reply_text(id: &str, op: &str, result: Result<String, String>) -> Response {
    match result {
        Ok(stdout) => Response::ok(id, 0, stdout, String::new()),
        Err(msg) => refuse(id, op, msg),
    }
}

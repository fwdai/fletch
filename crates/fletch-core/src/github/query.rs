//! Repo/branch context resolution, the GraphQL degradation wrapper, shared
//! query builders, batched multi-PR helpers, and time parsing.

use std::path::Path;

use serde_json::{json, Value};

use crate::error::{Error, Result};

use super::checks::{pr_checks_from_node, PR_CHECKS_FIELDS};
use super::client;
use super::pr::{parse_pr_state, PR_STATE_FIELDS};
use super::types::*;

pub(crate) async fn repo_ref(checkout: &Path) -> Option<(String, String)> {
    let out = crate::git_dist::command(checkout)
        .args(["remote", "get-url", "origin"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let web = crate::git_state::github_web_url(String::from_utf8_lossy(&out.stdout).trim())?;
    let mut parts = web.strip_prefix("https://github.com/")?.split('/');
    Some((parts.next()?.to_string(), parts.next()?.to_string()))
}

pub(crate) async fn require_repo_ref(checkout: &Path) -> Result<(String, String)> {
    repo_ref(checkout).await.ok_or_else(|| {
        Error::Gh("this repository's `origin` remote is not a GitHub repository".into())
    })
}

/// Resolve `owner/repo` from the checkout's origin, falling back to the source
/// repo when the checkout is broken or gone (they share an origin). `None` when
/// neither resolves to a github.com remote. Shared by the by-number lookups
/// (single and batched) that must survive a checkout casualty.
pub(crate) async fn resolve_slug(
    checkout: &Path,
    source: Option<&Path>,
) -> Option<(String, String)> {
    match repo_ref(checkout).await {
        Some(slug) => Some(slug),
        None => match source {
            Some(src) => repo_ref(src).await,
            None => None,
        },
    }
}

pub(crate) async fn require_current_branch(checkout: &Path, what: &str) -> Result<String> {
    crate::git::current_branch(checkout)
        .await?
        .ok_or_else(|| Error::Gh(format!("{what}: HEAD is detached — no branch to look up")))
}

/// The budget probe every read query carries. GraphQL bills *points* (~5000/hr),
/// not requests, and the cost is charged on the query's declared shape — so a
/// poll path can drain the whole budget without ever seeing an error. Riding
/// this selection on every read (it is itself free) lets [`note_budget`] arm the
/// backoff gate *before* exhaustion rather than after GitHub starts 403-ing.
pub(crate) const RATE_LIMIT_PROBE: &str = "rateLimit{cost remaining resetAt}";

/// Shared query fields for a PR looked up by branch. Created-desc so
/// `pick_branch_pr` sees the newest first, mirroring gh's branch resolution.
///
/// Note the `first:30`: every connection nested under this one is billed
/// *per parent*, so a selection with its own `first:N` costs 30×N requests
/// here. Prefer [`pr_node_by_number`] for anything with a nested connection —
/// the branch scan is for *discovery* (no PR number known yet), not polling.
pub(crate) fn branch_prs_query(inner_fields: &str) -> String {
    format!(
        r#"query($owner:String!,$repo:String!,$branch:String!){{
  repository(owner:$owner,name:$repo){{
    pullRequests(headRefName:$branch, states:[OPEN,CLOSED,MERGED], first:30,
                 orderBy:{{field:CREATED_AT,direction:DESC}}){{
      nodes{{ state {inner_fields} }}
    }}
  }}
  {RATE_LIMIT_PROBE}
}}"#
    )
}

/// Fetch one PR node by number with `inner_fields` selected. The by-number
/// counterpart to [`branch_prs_query`]: `pullRequest(number:)` is a single
/// node, not a connection, so nested selections are billed once instead of
/// thirty times — the same collapse the batched fetchers rely on.
///
/// `owner/repo` resolves from the checkout's origin, falling back to
/// `source_repo` (same origin) when the checkout is broken or gone.
/// `Ok(None)` when the slug or the PR can't be resolved, or while a
/// rate-limit backoff is active.
/// The PR node for `number`, plus the connected account's login from the SAME
/// response.
///
/// `viewer` is a single node, not a connection, so selecting it costs nothing on
/// top of the PR read — which is the point: the alternative is `auth_status()`,
/// a second round trip, on a read that already runs on a timer. Callers that
/// need to know "is this comment ours?" get the answer consistent with the data
/// it applies to, rather than from a separately-cached login.
pub(crate) async fn pr_node_and_viewer(
    checkout: &Path,
    source_repo: Option<&Path>,
    number: u32,
    inner_fields: &str,
) -> Result<Option<(Value, Option<String>)>> {
    let Some((owner, repo)) = resolve_slug(checkout, source_repo).await else {
        return Ok(None);
    };
    let query = format!(
        r#"query($owner:String!,$repo:String!,$number:Int!){{
  viewer{{ login }}
  repository(owner:$owner,name:$repo){{ pullRequest(number:$number){{ state {inner_fields} }} }}
  {RATE_LIMIT_PROBE}
}}"#
    );
    let Some(data) = graphql_opt(
        &query,
        json!({ "owner": owner, "repo": repo, "number": number }),
    )
    .await?
    else {
        return Ok(None);
    };
    let node = &data["repository"]["pullRequest"];
    if node.is_null() {
        return Ok(None);
    }
    let login = data["viewer"]["login"].as_str().map(str::to_string);
    Ok(Some((node.clone(), login)))
}

/// The PR a branch "belongs to": the newest open PR, else the newest PR of
/// any state — the same preference gh used, so a branch whose PR just merged
/// still resolves to that merged PR instead of disappearing.
pub(crate) fn pick_branch_pr(nodes: &[Value]) -> Option<&Value> {
    nodes
        .iter()
        .find(|n| n["state"].as_str() == Some("OPEN"))
        .or_else(|| nodes.first())
}

/// Run a GraphQL query, mapping GitHub's "not found" errors to `Ok(None)` —
/// the same degradation the gh wrapper applied to its stderr ("could not
/// resolve to a PullRequest", "...not found").
pub(crate) async fn graphql_opt(query: &str, variables: Value) -> Result<Option<Value>> {
    // A rate-limit pause is in effect — skip the request so callers degrade to
    // the persisted snapshot instead of spending one that would likely 403.
    if client::is_backing_off() {
        return Ok(None);
    }
    let client = match client::Client::new() {
        Ok(c) => c,
        // Read paths poll in the background; not being connected is a normal
        // state there, not an error to surface on every tick.
        Err(_) => return Ok(None),
    };
    match client.graphql(query, variables).await {
        Ok(data) => {
            // Feed the budget back into the gate for any query carrying
            // `RATE_LIMIT_PROBE` (a no-op for those that don't), so the
            // single-PR read paths throttle themselves like the batches do.
            note_budget(&data);
            Ok(Some(data))
        }
        Err(Error::Gh(msg)) => {
            let low = msg.to_lowercase();
            if low.contains("could not resolve") || low.contains("not found") {
                Ok(None)
            } else {
                Err(Error::Gh(msg))
            }
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn branch_pr_nodes(data: &Value) -> Vec<Value> {
    data["repository"]["pullRequests"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// GitHub ISO-8601 timestamp → ms epoch. None for absent/null/unparseable.
pub(crate) fn gh_time_ms(node: &Value, field: &str) -> Option<i64> {
    let iso = node[field].as_str()?;
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|t| t.timestamp_millis())
}

// ---------------------------------------------------------------------------
// Batched multi-PR queries
//
// The app-wide polls (`refresh_all_pr_states`/`refresh_all_pr_checks`) used to
// fan one GraphQL request out per agent, concurrently — the burst that trips
// GitHub's secondary rate limit. Instead we collapse them into a single aliased
// query (`a0: repository(...){pullRequest(number:…){…}} a1: …`), chunked, so N
// agents cost ⌈N/50⌉ *sequential* requests rather than N concurrent ones.
// ---------------------------------------------------------------------------

/// One PR to look up by number in a batched query.
#[derive(Debug, Clone)]
pub(crate) struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u32,
}

/// Max PRs per batched request — each adds a top-level `repository` alias, so
/// chunking keeps the query under GitHub's node/complexity limits.
const BATCH_CHUNK: usize = 50;

/// Build an aliased multi-PR query with a trailing `rateLimit` probe. Values
/// ride in as variables (`$oN/$rN/$nN`) so nothing user-derived is interpolated
/// into the query text.
fn build_batch_query(chunk: &[PrRef], inner_fields: &str) -> (String, Value) {
    let mut decls = Vec::with_capacity(chunk.len());
    let mut aliases = Vec::with_capacity(chunk.len());
    let mut vars = serde_json::Map::new();
    for (i, r) in chunk.iter().enumerate() {
        decls.push(format!("$o{i}:String!,$r{i}:String!,$n{i}:Int!"));
        aliases.push(format!(
            "a{i}:repository(owner:$o{i},name:$r{i}){{pullRequest(number:$n{i}){{{inner_fields}}}}}"
        ));
        vars.insert(format!("o{i}"), json!(r.owner));
        vars.insert(format!("r{i}"), json!(r.repo));
        vars.insert(format!("n{i}"), json!(r.number));
    }
    let query = format!(
        "query({}){{{} rateLimit{{cost remaining resetAt}}}}",
        decls.join(","),
        aliases.join(" "),
    );
    (query, Value::Object(vars))
}

/// Feed the queried `rateLimit` budget into the client's backoff gate.
fn note_budget(data: &Value) {
    let rl = &data["rateLimit"];
    if let Some(remaining) = rl["remaining"].as_i64() {
        let reset = rl["resetAt"]
            .as_str()
            .and_then(|iso| chrono::DateTime::parse_from_rfc3339(iso).ok())
            .map(|t| t.timestamp_millis());
        client::note_rate_budget(remaining, reset);
    }
}

/// Run `refs` through one or more batched queries, mapping each alias's
/// `pullRequest` node with `parse`. Results align 1:1 with `refs`; a
/// missing/inaccessible PR yields `None` for its slot (partial-error tolerant).
/// `Ok(vec![])` for empty input; an active backoff short-circuits to all-`None`
/// so callers fall back to the persisted snapshot without spending a request.
async fn pr_batch<T>(
    refs: &[PrRef],
    inner_fields: &str,
    parse: impl Fn(&Value) -> T,
) -> Result<Vec<Option<T>>> {
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    if client::is_backing_off() {
        return Ok(refs.iter().map(|_| None).collect());
    }
    let client = client::Client::new()?;
    let mut out = Vec::with_capacity(refs.len());
    for chunk in refs.chunks(BATCH_CHUNK) {
        let (query, vars) = build_batch_query(chunk, inner_fields);
        let data = client.graphql_partial(&query, vars).await?;
        note_budget(&data);
        for i in 0..chunk.len() {
            let node = &data[format!("a{i}")]["pullRequest"];
            out.push((!node.is_null()).then(|| parse(node)));
        }
        // A signal in this chunk's response — the budget crossing its floor, a
        // Retry-After, or a RATE_LIMITED error — armed the gate. Stop before the
        // next chunk spends the reserve, padding the unfetched refs with `None`
        // so the result stays aligned 1:1 with `refs` (callers zip on that).
        if client::is_backing_off() {
            out.resize_with(refs.len(), || None);
            break;
        }
    }
    Ok(out)
}

/// Fetch PR state *and* the merge gate + checks for many PRs by number in one
/// (chunked) round-trip — the app-wide sidebar sweep.
///
/// State and checks used to be two separate sweeps on two clocks. Selecting
/// both off the same aliased `pullRequest` node costs the same as either one
/// alone (measured: 1 point at 50 aliases, the chunk size), so merging them
/// halves the sweep's cost and — more importantly — means the sidebar's badge
/// and its CI tint always come from the same instant instead of drifting up to
/// a cadence apart.
pub(crate) async fn pr_status_batch(refs: &[PrRef]) -> Result<Vec<Option<(PrState, PrChecks)>>> {
    pr_batch(
        refs,
        &format!("state {PR_STATE_FIELDS} {PR_CHECKS_FIELDS}"),
        |node| (parse_pr_state(node), pr_checks_from_node(node)),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_node(state: &str, number: u32, mergeable: &str) -> Value {
        json!({
            "number": number,
            "url": format!("https://github.com/o/r/pull/{number}"),
            "state": state,
            "title": format!("PR {number}"),
            "mergeable": mergeable,
        })
    }

    /// The branch→PR pick prefers the newest open PR, falling back to the
    /// newest of any state — a merged PR must still resolve (session adoption
    /// depends on seeing MERGED, not "no PR").
    #[test]
    fn picks_open_pr_over_newer_closed_and_falls_back_to_newest() {
        let nodes = vec![
            pr_node("CLOSED", 9, "UNKNOWN"),
            pr_node("OPEN", 7, "MERGEABLE"),
            pr_node("MERGED", 5, "UNKNOWN"),
        ];
        let picked = pick_branch_pr(&nodes).unwrap();
        assert_eq!(picked["number"].as_u64(), Some(7));

        let no_open = vec![
            pr_node("MERGED", 9, "UNKNOWN"),
            pr_node("CLOSED", 5, "UNKNOWN"),
        ];
        assert_eq!(
            pick_branch_pr(&no_open).unwrap()["number"].as_u64(),
            Some(9)
        );
        assert!(pick_branch_pr(&[]).is_none());
    }

    #[test]
    fn batch_query_builds_aliases_and_variables() {
        let refs = vec![
            PrRef {
                owner: "acme".into(),
                repo: "web".into(),
                number: 7,
            },
            PrRef {
                owner: "acme".into(),
                repo: "api".into(),
                number: 12,
            },
        ];
        let (query, vars) = build_batch_query(&refs, "state number");
        // One aliased repository/pullRequest per ref, values via variables.
        assert!(
            query.contains("a0:repository(owner:$o0,name:$r0)"),
            "{query}"
        );
        assert!(
            query.contains("a1:repository(owner:$o1,name:$r1)"),
            "{query}"
        );
        assert!(query.contains("pullRequest(number:$n1)"), "{query}");
        // The budget probe rides along on every batch.
        assert!(query.contains("rateLimit"), "{query}");
        assert_eq!(vars["o0"], json!("acme"));
        assert_eq!(vars["r1"], json!("api"));
        assert_eq!(vars["n1"], json!(12));
    }
}

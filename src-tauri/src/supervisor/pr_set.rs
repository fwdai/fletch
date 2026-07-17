//! Multi-repo PR-set cross-linking: when one agent's task produces PRs in
//! more than one repo, stamp each PR's body with a trailer linking the whole
//! set ("Part of a multi-repo change in <project>: owner/frontend#12,
//! owner/backend#34"), so a reviewer landing on any one PR can find the rest.
//!
//! The trailer is wrapped in HTML-comment sentinels and *replaced* between
//! them on every sync — never string-appended — so re-running (a third PR
//! opening, a retried sync) can't duplicate it, and it composes safely with
//! bodies the agent wrote itself. Everything here is best-effort: a body edit
//! is decoration, so failures are logged, never surfaced.

use crate::workspace::{repo_checkout_path, WorkspaceManager};

/// Sentinels marking the generated trailer inside a PR body. Content between
/// them is owned by Fletch and replaced wholesale on every sync.
const PR_SET_START: &str = "<!-- fletch:pr-set -->";
const PR_SET_END: &str = "<!-- /fletch:pr-set -->";

/// Replace the sentinel-marked trailer block in `body` (or append one when
/// absent). Pure — the sync's only string surgery, so it's unit-tested hard.
fn apply_pr_set_trailer(body: &str, trailer: &str) -> String {
    let block = format!("{PR_SET_START}\n{trailer}\n{PR_SET_END}");
    if let (Some(start), Some(end)) = (body.find(PR_SET_START), body.rfind(PR_SET_END)) {
        // Replace everything from the first start sentinel through the last
        // end sentinel, so even a mangled body (duplicated blocks from an
        // older bug, an agent quoting the block) converges to one clean block.
        if end >= start {
            let mut out = String::with_capacity(body.len() + block.len());
            out.push_str(&body[..start]);
            out.push_str(&block);
            out.push_str(&body[end + PR_SET_END.len()..]);
            return out;
        }
    }
    if body.trim().is_empty() {
        block
    } else {
        format!("{}\n\n{}", body.trim_end(), block)
    }
}

/// The trailer's human-facing content: an `---` rule plus one line naming the
/// project and every PR in the set as `owner/repo#N` (GitHub auto-links these
/// across repos).
fn pr_set_trailer(project_name: Option<&str>, refs: &[String]) -> String {
    let list = refs.join(", ");
    match project_name {
        Some(name) => format!("---\nPart of a multi-repo change in {name}: {list}"),
        None => format!("---\nPart of a multi-repo change: {list}"),
    }
}

/// Sync the PR-set trailer across every PR bound to this agent. No-op unless
/// the agent has PRs in **two or more** repos — single-PR agents never get a
/// trailer. Called after any path that binds a new PR (panel `create_pr`, the
/// agent's `open_pr` RPC); each call rewrites the full set, which is what
/// backfills the older PRs' bodies when the second PR appears.
///
/// Best-effort throughout: an unresolvable slug drops that repo from the set,
/// a fetch/update failure is logged and skipped, and an active rate-limit
/// backoff skips the whole sync (the next PR-binding event retries it).
pub(crate) async fn sync_pr_set_links(workspace: &WorkspaceManager, agent_id: &str) {
    if crate::github::client::is_backing_off() {
        return;
    }
    let Ok(record) = workspace.agent(agent_id) else {
        return;
    };

    // Every bound PR of the agent, as (checkout, source repo, number, ref).
    let mut prs = Vec::new();
    for repo in &record.repos {
        let Some(number) = repo.pr_number else {
            continue;
        };
        let Ok(checkout) = repo_checkout_path(agent_id, &repo.subdir) else {
            continue;
        };
        let Some((owner, name)) =
            crate::github::resolve_slug(&checkout, Some(&repo.repo_path)).await
        else {
            continue;
        };
        prs.push((
            checkout,
            repo.repo_path.clone(),
            number as u32,
            format!("{owner}/{name}#{number}"),
        ));
    }
    if prs.len() < 2 {
        return;
    }

    let project_name = workspace.current().and_then(|ws| {
        ws.projects
            .iter()
            .find(|p| p.project_id == record.project_id)
            .map(|p| p.name.clone())
    });
    let refs: Vec<String> = prs.iter().map(|(_, _, _, r)| r.clone()).collect();
    let trailer = pr_set_trailer(project_name.as_deref(), &refs);

    for (checkout, source, number, pr_ref) in &prs {
        let body = match crate::github::pr_body(checkout, Some(source), *number).await {
            Ok(Some(body)) => body,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(error = %e, pr = %pr_ref, "pr-set: body fetch failed");
                continue;
            }
        };
        let updated = apply_pr_set_trailer(&body, &trailer);
        // Idempotency backstop: an unchanged body (the set didn't grow) costs
        // no write — the common case for every PR but the newest.
        if updated == body {
            continue;
        }
        if let Err(e) = crate::github::pr_update_body(checkout, Some(source), *number, &updated).await
        {
            tracing::warn!(error = %e, pr = %pr_ref, "pr-set: body update failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_appends_to_a_body_without_one() {
        let body = "Adds the login flow.";
        let out = apply_pr_set_trailer(body, "---\nPart of a multi-repo change in X: o/a#1, o/b#2");
        assert_eq!(
            out,
            "Adds the login flow.\n\n<!-- fletch:pr-set -->\n---\nPart of a multi-repo change in X: o/a#1, o/b#2\n<!-- /fletch:pr-set -->"
        );
    }

    #[test]
    fn trailer_replaces_between_sentinels_never_duplicates() {
        let body = "Adds the login flow.";
        let first = apply_pr_set_trailer(body, "---\nset: o/a#1, o/b#2");
        // The set grew — re-applying must swap the block in place, keeping the
        // prose above it and producing exactly one sentinel pair.
        let second = apply_pr_set_trailer(&first, "---\nset: o/a#1, o/b#2, o/c#3");
        assert_eq!(second.matches(PR_SET_START).count(), 1);
        assert_eq!(second.matches(PR_SET_END).count(), 1);
        assert!(second.starts_with("Adds the login flow."));
        assert!(second.contains("o/c#3"));
        assert!(!second.contains("set: o/a#1, o/b#2\n"), "old block must be gone");
        // Same trailer again → byte-identical (the sync skips the PATCH).
        let third = apply_pr_set_trailer(&second, "---\nset: o/a#1, o/b#2, o/c#3");
        assert_eq!(second, third);
    }

    #[test]
    fn trailer_preserves_text_after_the_block() {
        // The agent (or a human) edited below the trailer — replacement must
        // keep that text, not truncate the body at the block.
        let body = "intro\n\n<!-- fletch:pr-set -->\nold\n<!-- /fletch:pr-set -->\n\noutro";
        let out = apply_pr_set_trailer(body, "new");
        assert_eq!(
            out,
            "intro\n\n<!-- fletch:pr-set -->\nnew\n<!-- /fletch:pr-set -->\n\noutro"
        );
    }

    #[test]
    fn trailer_is_the_whole_body_when_body_is_empty() {
        let out = apply_pr_set_trailer("", "content");
        assert_eq!(out, "<!-- fletch:pr-set -->\ncontent\n<!-- /fletch:pr-set -->");
        // Whitespace-only bodies count as empty (no leading blank lines).
        let out = apply_pr_set_trailer("  \n", "content");
        assert!(out.starts_with(PR_SET_START));
    }

    #[test]
    fn trailer_content_names_project_and_set() {
        let refs = vec!["o/frontend#12".to_string(), "o/backend#34".to_string()];
        assert_eq!(
            pr_set_trailer(Some("Acme"), &refs),
            "---\nPart of a multi-repo change in Acme: o/frontend#12, o/backend#34"
        );
        assert_eq!(
            pr_set_trailer(None, &refs),
            "---\nPart of a multi-repo change: o/frontend#12, o/backend#34"
        );
    }
}

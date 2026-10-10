//! The context layer's human-facing command surface: what the project page's
//! Context tab reads and writes, reached the same way from a paired client
//! (`remote::dispatch`). Every op takes the host-local `projects.id` and
//! resolves the project's context id itself; every write is stamped as the
//! user acting through the UI. The `context:changed` pulse any open tab
//! reloads on is the service's, raised for every writer's writes alike.
//!
//! See docs/remote-protocol.md, "Project context".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::capture::bootstrap;
use crate::context::{
    self, compile, render, AssertionInput, AssertionStatus, Author, Candidate, CompileQuery,
    DismissReason, EntityInput, Graph, Id, Landing, LinkChange, Proposal, ProposalStatus,
    Provenance, Source, Stamp, Stats,
};
use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::roadmap::drainer::primary_repo_path;

pub use crate::context::CHANGED_EVENT as CONTEXT_CHANGED;

/// Everything the Context tab shows in one read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextOverview {
    /// `context.enabled` as the host reads it (absent = on).
    pub enabled: bool,
    /// `context.extract` as the host reads it; implies `enabled`.
    pub extract: bool,
    pub graph: Graph,
    /// Pending proposals only: the review queue.
    pub proposals: Vec<Proposal>,
    pub stats: Stats,
}

/// How the user rules on a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalVerdict {
    Accept,
    Dismiss,
}

/// The stamp every UI write carries: the user, through the UI, from no
/// particular workspace or turn.
fn ui_stamp() -> Stamp {
    Stamp {
        author: Author::user(),
        source: Source::ui(),
        provenance: Provenance::default(),
    }
}

/// The project's context id, minted on first use — and the gate: a client
/// talking straight to the host (an older remote, a stale window) must not
/// read or rewrite context the layer is off for, so every command resolves
/// its project through here. The overview is the one deliberate exception
/// (it reports the toggles), and reads the id itself. Takes and releases the
/// connection lock so the store's own locking can follow.
fn open(ctx: &EngineCtx, project_id: &str) -> Result<context::Project> {
    Ok(ctx.context()?.open(project_id)?)
}

pub fn context_overview_impl(ctx: &EngineCtx, project_id: &str) -> Result<ContextOverview> {
    let (enabled, extract, id) = {
        let conn = ctx.db.lock();
        (
            context::enabled(&conn, project_id),
            context::extract_enabled(&conn, project_id),
            context::context_project_id(&conn, project_id)?,
        )
    };
    let store = ctx.context()?.store();
    Ok(ContextOverview {
        enabled,
        extract,
        graph: store.load(&id)?,
        proposals: store.proposals(&id, Some(ProposalStatus::Pending))?,
        stats: store.stats(&id)?,
    })
}

/// What an agent would be served for `query`: the compiled bundle rendered as
/// markdown, with path anchors checked against the project's primary repo —
/// or, with `query.overview`, exactly the overview every agent's instructions
/// carry at spawn.
pub fn context_preview_impl(
    ctx: &EngineCtx,
    project_id: &str,
    query: CompileQuery,
) -> Result<String> {
    let project = open(ctx, project_id)?;
    let repo = primary_repo_path(&ctx.db.lock(), project_id).map(PathBuf::from);
    let graph = ctx.context()?.store().load(&project.id)?;
    Ok(render::render_markdown(&compile::compile(
        &graph,
        &query,
        repo.as_deref(),
    )))
}

/// What a bootstrap did, and the task that maps the meaning onto it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBootstrap {
    /// The commit the repository was read at.
    pub commit: String,
    /// Modules the tree describes.
    pub modules: usize,
    /// Slugs recorded by this run; empty when every module was already there.
    pub created: Vec<String>,
    /// The canned task for the mapping session (`instructions/context_mapping.md`).
    pub mapping_task: String,
}

/// Tier one of the cold start: record the modules of the project's primary
/// repo at its `HEAD` (`capture::bootstrap`). Idempotent — a slug the project
/// already has is skipped — so it is also how the UI gets the mapping task
/// again.
pub async fn context_bootstrap_impl(ctx: &EngineCtx, project_id: &str) -> Result<ContextBootstrap> {
    let project = open(ctx, project_id)?;
    let (repo, brief) = {
        let conn = ctx.db.lock();
        (
            primary_repo_path(&conn, project_id),
            legacy_brief(&conn, project_id),
        )
    };
    let repo = repo.ok_or_else(|| Error::Other("this project has no repository to read".into()))?;
    let skeleton = bootstrap::derive(Path::new(&repo)).await?;
    let applied = bootstrap::apply(
        ctx.context()?,
        &project,
        &skeleton.modules,
        bootstrap::stamp(&skeleton.commit),
    )?;
    Ok(ContextBootstrap {
        commit: skeleton.commit,
        modules: skeleton.modules.len(),
        created: applied.created,
        mapping_task: crate::instructions::context_mapping_task(brief.as_deref()),
    })
}

/// The project's roadmap brief, read straight from its table: the UI and ops
/// that wrote it are gone or going, the user-reviewed content is not, and
/// the mapping session is where it lands. A missing table or row is `None`.
fn legacy_brief(conn: &rusqlite::Connection, project_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT content FROM roadmap_briefs WHERE project_id = ?1",
        [project_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .filter(|content| !content.trim().is_empty())
}

/// Create an entity, or record a revision of one (`input.id` set).
pub fn context_record_entity_impl(
    ctx: &EngineCtx,
    project_id: &str,
    input: EntityInput,
) -> Result<Id> {
    let project = open(ctx, project_id)?;
    let recorded = ctx.context()?.record_entity(&project, input, ui_stamp())?;
    Ok(recorded)
}

/// Record an assertion. The user saying it is the confirmation, so the status
/// is always `confirmed` here whatever the caller sent; `input.supersedes` is
/// the "change a decision" path and must carry reasoning.
pub fn context_record_assertion_impl(
    ctx: &EngineCtx,
    project_id: &str,
    mut input: AssertionInput,
) -> Result<Id> {
    input.status = AssertionStatus::Confirmed;
    let project = open(ctx, project_id)?;
    let candidate = Candidate {
        input,
        user: None,
        relation: None,
        evidence: Vec::new(),
        about_pending: Vec::new(),
        observation_id: None,
    };
    // A user write always lands (or names the head it restates).
    let recorded = match ctx
        .context()?
        .record_decision(&project, candidate, ui_stamp())?
    {
        Landing::Recorded { id, .. } | Landing::Duplicate { id } => id,
        Landing::Related { .. } | Landing::Held { .. } | Landing::Dismissed { .. } => {
            return Err(Error::Other("a user write cannot be held".into()))
        }
    };
    Ok(recorded)
}

/// Hide an assertion that was never right (as opposed to superseding one we
/// changed our mind about).
pub fn context_retract_impl(
    ctx: &EngineCtx,
    project_id: &str,
    assertion_id: &str,
    reason: &str,
) -> Result<()> {
    let project = open(ctx, project_id)?;
    ctx.context()?
        .retract(&project, assertion_id, reason, ui_stamp())?;
    Ok(())
}

pub fn context_archive_entity_impl(
    ctx: &EngineCtx,
    project_id: &str,
    entity_id: &str,
) -> Result<()> {
    let project = open(ctx, project_id)?;
    ctx.context()?
        .archive_entity(&project, entity_id, ui_stamp())?;
    Ok(())
}

/// Fold `from` into `into`: its edges move, it stays behind as `merged`.
pub fn context_merge_entities_impl(
    ctx: &EngineCtx,
    project_id: &str,
    from: &str,
    into: &str,
) -> Result<()> {
    let project = open(ctx, project_id)?;
    ctx.context()?
        .merge_entities(&project, from, into, ui_stamp())?;
    Ok(())
}

pub fn context_link_impl(ctx: &EngineCtx, project_id: &str, change: LinkChange) -> Result<()> {
    let project = open(ctx, project_id)?;
    ctx.context()?.link(&project, change, ui_stamp())?;
    Ok(())
}

/// Close the tension between `a` and `b` with a ruling. Neither side
/// changes; retract or supersede one of them for that.
pub fn context_resolve_contradiction_impl(
    ctx: &EngineCtx,
    project_id: &str,
    a: &str,
    b: &str,
    reasoning: &str,
) -> Result<()> {
    let project = open(ctx, project_id)?;
    ctx.context()?
        .resolve_contradiction(&project, a, b, reasoning, ui_stamp())?;
    Ok(())
}

/// Rule on a pending proposal. Accepting lands it as events and answers the
/// recorded id; dismissing needs a reason and answers `None`.
pub fn context_rule_proposal_impl(
    ctx: &EngineCtx,
    project_id: &str,
    proposal_id: &str,
    verdict: ProposalVerdict,
    dismiss_reason: Option<DismissReason>,
) -> Result<Option<Id>> {
    let service = ctx.context()?;
    let project = open(ctx, project_id)?;
    let recorded = match verdict {
        ProposalVerdict::Accept => {
            Some(service.accept_proposal(&project, proposal_id, Author::user())?)
        }
        ProposalVerdict::Dismiss => {
            let reason = dismiss_reason
                .ok_or_else(|| Error::Other("dismissing a proposal needs a reason".into()))?;
            service.dismiss_proposal(&project, proposal_id, reason, Author::user())?;
            None
        }
    };
    Ok(recorded)
}

/// Dismiss the named pending proposals as trivial: the way out of a backlog
/// that is too long to rule on card by card. The caller passes the ids it
/// showed, so nothing unseen is ruled on. Answers how many were dismissed.
pub fn context_dismiss_proposals_impl(
    ctx: &EngineCtx,
    project_id: &str,
    proposal_ids: &[String],
) -> Result<usize> {
    let service = ctx.context()?;
    let project = open(ctx, project_id)?;
    Ok(service.dismiss_proposals(
        &project,
        proposal_ids,
        DismissReason::Trivial,
        Author::user(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The wire spelling a client sends for a ruling.
    #[test]
    fn verdict_is_spelled_in_snake_case() {
        assert_eq!(
            serde_json::from_value::<ProposalVerdict>(json!("accept")).unwrap(),
            ProposalVerdict::Accept
        );
        assert_eq!(
            serde_json::from_value::<ProposalVerdict>(json!("dismiss")).unwrap(),
            ProposalVerdict::Dismiss
        );
        assert!(serde_json::from_value::<ProposalVerdict>(json!("Accept")).is_err());
    }

    /// A UI write is the user's own words through the UI: `trust` treats it
    /// as user-stated, and `land` lets it land outright.
    #[test]
    fn ui_writes_are_user_stated() {
        let stamp = ui_stamp();
        assert_eq!(stamp.author, Author::user());
        assert_eq!(stamp.source, Source::ui());
        assert_eq!(stamp.provenance, Provenance::default());
    }

    /// Neither refusal touches the store (which would panic until it lands),
    /// so the checks are reachable now.
    #[test]
    fn a_blank_retract_reason_and_a_self_merge_are_refused_before_any_write() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        crate::database::set_setting(&ctx.db.lock(), context::DEV_SETTING, "true").unwrap();
        assert!(context_retract_impl(&ctx, "p1", "a1", "  ").is_err());
        assert!(context_merge_entities_impl(&ctx, "p1", "e1", "e1").is_err());
        assert!(context_resolve_contradiction_impl(&ctx, "p1", "a1", "a2", " ").is_err());
        assert!(sink.events().is_empty());
    }

    /// With the gate shut every write is refused with the layer's own
    /// message, before any store is touched.
    #[test]
    fn a_closed_gate_refuses_writes_by_name() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let e = context_resolve_contradiction_impl(&ctx, "p1", "a1", "a2", "both hold")
            .unwrap_err()
            .to_string();
        assert!(e.contains("context layer is off"), "{e}");
        assert!(sink.events().is_empty());
    }

    /// The bootstrap hands the mapping session a project's legacy brief, and
    /// only when it has one.
    #[tokio::test]
    async fn the_mapping_task_carries_a_legacy_brief_only_when_there_is_one() {
        let (ctx, _sink, dir) = crate::host::ctx::test_ctx();
        crate::database::set_setting(&ctx.db.lock(), context::DEV_SETTING, "true").unwrap();
        for project in ["p1", "p2"] {
            let repo = dir.path().join(project);
            std::fs::create_dir_all(repo.join("src/app")).unwrap();
            std::fs::write(repo.join("package.json"), "{}").unwrap();
            std::fs::write(repo.join("src/app/main.ts"), "").unwrap();
            crate::git::init_repo(&repo).await.unwrap();
            crate::git::commit_all(&repo, "init").await.unwrap();
            let conn = ctx.db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, created_at) VALUES (?1, ?1, 0)",
                [project],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO repos (id, project_id, path, created_at) VALUES (?1, ?1, ?2, 0)",
                [project, repo.to_str().unwrap()],
            )
            .unwrap();
        }
        ctx.db
            .lock()
            .execute(
                "INSERT INTO roadmap_briefs (project_id, content, updated_at)
                 VALUES ('p1', 'Fletch runs coding agents side by side.', 0)",
                [],
            )
            .unwrap();

        let with = context_bootstrap_impl(&ctx, "p1").await.unwrap();
        assert!(
            with.mapping_task.contains("## Legacy product brief"),
            "{}",
            with.mapping_task
        );
        assert!(with
            .mapping_task
            .contains("<legacy-product-brief>\nFletch runs coding agents side by side.\n"));
        assert_eq!(with.created, vec!["src", "src-app"]);

        let without = context_bootstrap_impl(&ctx, "p2").await.unwrap();
        assert!(!without.mapping_task.contains("## Legacy product brief"));
        assert!(!without.mapping_task.contains("<legacy-product-brief>"));
        assert_eq!(
            without.mapping_task,
            crate::instructions::context_mapping_task(None)
        );
    }

    /// A ruling on a tension between two recorded heads is one event and one
    /// `context:changed` pulse.
    #[test]
    fn resolving_a_contradiction_emits_a_change() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        {
            let conn = ctx.db.lock();
            crate::database::set_setting(&conn, context::DEV_SETTING, "true").unwrap();
            conn.execute(
                "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'p', 0)",
                [],
            )
            .unwrap();
        }
        let entity = context_record_entity_impl(
            &ctx,
            "p1",
            EntityInput {
                slug: "billing".into(),
                name: "Billing".into(),
                summary: "invoices".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let assertion = |statement: &str, contradicts: Vec<Id>| AssertionInput {
            kind: context::AssertionKind::Decision,
            domain: context::Domain::Architectural,
            stance: context::Stance::Adopted,
            statement: statement.into(),
            rationale: "because".into(),
            valid_from: None,
            paths: vec![],
            about: vec![entity.clone()],
            supersedes: None,
            contradicts: contradicts
                .into_iter()
                .map(|id| context::Contradict {
                    id,
                    reasoning: None,
                })
                .collect(),
            status: AssertionStatus::Provisional,
        };
        let a =
            context_record_assertion_impl(&ctx, "p1", assertion("Invoices are immutable", vec![]))
                .unwrap();
        let b = context_record_assertion_impl(
            &ctx,
            "p1",
            assertion("Invoices may be voided", vec![a.clone()]),
        )
        .unwrap();
        let before = sink.events().len();

        context_resolve_contradiction_impl(&ctx, "p1", &a, &b, "voids are a new invoice").unwrap();

        let events = sink.events();
        assert_eq!(events.len(), before + 1);
        assert_eq!(events.last().unwrap().0, CONTEXT_CHANGED);
        let project = open(&ctx, "p1").unwrap();
        let graph = ctx.context().unwrap().store().load(&project.id).unwrap();
        let edge = graph
            .contradictions
            .iter()
            .find(|c| (c.a == a && c.b == b) || (c.a == b && c.b == a))
            .expect("the edge is loaded");
        let ruling = edge.resolution.as_ref().expect("ruled");
        assert_eq!(ruling.reasoning, "voids are a new invoice");
        assert_eq!(ruling.by, Author::user());
    }
}

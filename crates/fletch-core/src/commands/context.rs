//! The context layer's human-facing command surface: what the project page's
//! Context tab reads and writes, reached the same way from a paired client
//! (`remote::dispatch`). Every op takes the host-local `projects.id` and
//! resolves the project's context id itself; every write is stamped as the
//! user acting through the UI and ends in a `context:changed` pulse so any
//! open tab reloads.
//!
//! See docs/remote-protocol.md, "Project context".

use serde::{Deserialize, Serialize};

use crate::context::{
    self, compile, render, AssertionInput, AssertionStatus, Author, CompileQuery, DismissReason,
    EntityInput, Graph, Id, LinkChange, Proposal, ProposalStatus, Provenance, Source, Stamp, Stats,
};
use crate::error::{Error, Result};
use crate::host::EngineCtx;

/// Fired with `{ project_id }` (the fletch project id) after every write below.
/// Carries no row: the tab reloads the overview, which is small by design.
pub const CONTEXT_CHANGED: &str = "context:changed";

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

#[derive(Serialize)]
struct Changed<'a> {
    project_id: &'a str,
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

/// The project's context id, minted on first use. Takes and releases the
/// connection lock itself so the store's own locking can follow.
fn context_id(ctx: &EngineCtx, project_id: &str) -> Result<String> {
    let conn = ctx.db.lock();
    Ok(context::context_project_id(&conn, project_id)?)
}

fn emit_changed(ctx: &EngineCtx, project_id: &str) {
    crate::host::emit(ctx.sink.as_ref(), CONTEXT_CHANGED, &Changed { project_id });
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
    let store = ctx.context()?;
    Ok(ContextOverview {
        enabled,
        extract,
        graph: store.load(&id)?,
        proposals: store.proposals(&id, Some(ProposalStatus::Pending))?,
        stats: store.stats(&id)?,
    })
}

/// What an agent would be served for `query`: the compiled bundle rendered as
/// markdown, with the roadmap brief standing in for a vision nobody has
/// recorded yet.
pub fn context_preview_impl(
    ctx: &EngineCtx,
    project_id: &str,
    query: CompileQuery,
) -> Result<String> {
    let (id, brief) = {
        let conn = ctx.db.lock();
        (
            context::context_project_id(&conn, project_id)?,
            crate::roadmap::memory::load(&conn, project_id)?.map(|b| b.content),
        )
    };
    let graph = ctx.context()?.load(&id)?;
    Ok(render::render_markdown(&compile::compile(
        &graph, &query, brief,
    )))
}

/// Create an entity, or record a revision of one (`input.id` set).
pub fn context_record_entity_impl(
    ctx: &EngineCtx,
    project_id: &str,
    input: EntityInput,
) -> Result<Id> {
    let id = context_id(ctx, project_id)?;
    let recorded = ctx.context()?.record_entity(&id, input, ui_stamp())?;
    emit_changed(ctx, project_id);
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
    let id = context_id(ctx, project_id)?;
    let recorded = ctx.context()?.record_assertion(&id, input, ui_stamp())?;
    emit_changed(ctx, project_id);
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
    if reason.trim().is_empty() {
        return Err(Error::Other(
            "retracting an assertion needs a reason".into(),
        ));
    }
    let id = context_id(ctx, project_id)?;
    ctx.context()?
        .retract(&id, assertion_id, reason, ui_stamp())?;
    emit_changed(ctx, project_id);
    Ok(())
}

pub fn context_archive_entity_impl(
    ctx: &EngineCtx,
    project_id: &str,
    entity_id: &str,
) -> Result<()> {
    let id = context_id(ctx, project_id)?;
    ctx.context()?.archive_entity(&id, entity_id, ui_stamp())?;
    emit_changed(ctx, project_id);
    Ok(())
}

/// Fold `from` into `into`: its edges move, it stays behind as `merged`.
pub fn context_merge_entities_impl(
    ctx: &EngineCtx,
    project_id: &str,
    from: &str,
    into: &str,
) -> Result<()> {
    if from == into {
        return Err(Error::Other(
            "an entity cannot be merged into itself".into(),
        ));
    }
    let id = context_id(ctx, project_id)?;
    ctx.context()?.merge_entities(&id, from, into, ui_stamp())?;
    emit_changed(ctx, project_id);
    Ok(())
}

pub fn context_link_impl(ctx: &EngineCtx, project_id: &str, change: LinkChange) -> Result<()> {
    let id = context_id(ctx, project_id)?;
    ctx.context()?.link(&id, change, ui_stamp())?;
    emit_changed(ctx, project_id);
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
    let store = ctx.context()?;
    let context_id = context::context_project_id(&ctx.db.lock(), project_id)?;
    match store.proposal(proposal_id)? {
        Some(p) if p.project_id == context_id => {}
        _ => return Err(Error::Other("no such proposal in this project".into())),
    }
    let recorded = match verdict {
        ProposalVerdict::Accept => {
            Some(store.accept_proposal(proposal_id, ProposalStatus::Accepted, Author::user())?)
        }
        ProposalVerdict::Dismiss => {
            let reason = dismiss_reason
                .ok_or_else(|| Error::Other("dismissing a proposal needs a reason".into()))?;
            store.dismiss_proposal(proposal_id, reason, Author::user())?;
            None
        }
    };
    emit_changed(ctx, project_id);
    Ok(recorded)
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

    /// A UI write is the user's own words through the UI: that is what lets a
    /// later user-stated supersession land without review (`resolve::auto_rule`).
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
        assert!(sink.events().is_empty());
    }
}

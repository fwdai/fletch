//! One extraction, end to end, over a store: record the observation, run the
//! model, keep the run, then turn the answer into proposals — entities land at
//! once (they are cheap and the assertions need subjects), assertions go
//! through `resolve::classify` (duplicates), the model's own `supersedes` /
//! `contradicts` relation when it names a live head, and the auto rule. Pure
//! over its arguments apart from the store, so tests drive it with a fake
//! extractor. The observation is marked extracted only when the run landed,
//! so the turns of a failed one are picked up again by the next.

use std::time::Instant;

use super::super::model::*;
use super::super::{resolve, ContextStore, Result};
use super::input::ExtractInput;
use super::{prompt, Extractor};
use crate::database::now_millis;

/// What one run did, for the log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub observation_id: Id,
    pub entities_landed: usize,
    pub entities_skipped: usize,
    pub auto: usize,
    pub pending: usize,
    pub dismissed: usize,
    pub assertions_skipped: usize,
    /// Set when the run failed or its output could not be read; nothing was
    /// proposed then.
    pub error: Option<String>,
}

/// Run `extractor` over `input` for `project_id` and apply what it proposes.
/// `author` is the extractor's identity (agent and provider); `provenance`
/// names the workspace, session and turn every record gets stamped with.
/// `agent_status` is what an agent-stated assertion gets: `Provisional` while
/// the branch is open, its settled outcome when the run happens at archive.
pub fn process(
    store: &ContextStore,
    project_id: &str,
    author: Author,
    provenance: Provenance,
    agent_status: AssertionStatus,
    input: ExtractInput,
    extractor: &dyn Extractor,
) -> Result<Summary> {
    let text = prompt::render(&input);
    let observation = Observation {
        id: new_id(),
        project_id: project_id.to_string(),
        source: Source::new(
            SourceKind::UserTurn,
            input.last_turn_id().map(str::to_string),
        ),
        provenance: provenance.clone(),
        input_hash: sha256(&text),
        plan: input.plan.clone(),
        created_at: now_millis(),
        extracted_at: None,
    };
    store.add_observation(&observation)?;
    let mut summary = Summary {
        observation_id: observation.id.clone(),
        ..Default::default()
    };

    let started = Instant::now();
    let outcome = extractor.extract(&input);
    let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let (output, mut error) = match outcome {
        Ok(out) => (Some(out), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let parsed = output.as_ref().map(|out| prompt::parse(&out.text));
    if let Some(Err(e)) = &parsed {
        error = Some(format!("unparseable output: {e}"));
    }
    store.add_extractor_run(&ExtractorRun {
        id: new_id(),
        observation_id: observation.id.clone(),
        model: extractor.model(),
        prompt_version: prompt::PROMPT_VERSION.to_string(),
        output: output.as_ref().map(|o| o.text.clone()),
        tokens_in: output.as_ref().and_then(|o| o.tokens_in),
        tokens_out: output.as_ref().and_then(|o| o.tokens_out),
        duration_ms: Some(duration_ms),
        error: error.clone(),
        created_at: now_millis(),
    })?;
    let Some(Ok(parsed)) = parsed else {
        summary.error = error;
        return Ok(summary);
    };
    if parsed.malformed > 0 {
        tracing::warn!(
            observation = %observation.id,
            malformed = parsed.malformed,
            "context extract: items of the wrong shape were skipped"
        );
    }

    let stamp = |kind: SourceKind| Stamp {
        author: author.clone(),
        source: Source::new(kind, observation.source.reference.clone()),
        provenance: provenance.clone(),
    };
    let proposal = |payload: ProposalPayload, evidence: Vec<Evidence>| Proposal {
        id: new_id(),
        project_id: project_id.to_string(),
        observation_id: Some(observation.id.clone()),
        payload,
        evidence,
        status: ProposalStatus::Pending,
        dismiss_reason: None,
        created_at: now_millis(),
        ruled_at: None,
        ruled_by: None,
    };

    let mut graph = store.load(project_id)?;
    for entity in parsed.entities {
        let known = std::iter::once(&entity.slug)
            .chain(std::iter::once(&entity.name))
            .chain(&entity.aliases)
            .any(|reference| resolve::entity(&graph, reference).is_ok());
        if known {
            summary.entities_skipped += 1;
            continue;
        }
        let input = EntityInput {
            id: None,
            slug: entity.slug,
            kind: entity.kind,
            name: entity.name,
            summary: entity.summary,
            aliases: entity.aliases,
            paths: entity.paths,
        };
        let p = proposal(
            ProposalPayload::Entity {
                input,
                stamp: stamp(SourceKind::AgentTurn),
            },
            Vec::new(),
        );
        store.add_proposal(&p)?;
        match store.accept_proposal(&p.id, ProposalStatus::Auto, author.clone()) {
            Ok(_) => summary.entities_landed += 1,
            Err(e) => {
                tracing::warn!(proposal = %p.id, error = %e, "context extract: entity did not land");
                summary.entities_skipped += 1;
            }
        }
    }
    if summary.entities_landed > 0 {
        graph = store.load(project_id)?;
    }

    for assertion in parsed.assertions {
        let (found, unknown) = resolve::entities(&graph, &assertion.about);
        if !unknown.is_empty() {
            tracing::warn!(?unknown, statement = %assertion.statement, "context extract: unknown entity slugs dropped");
        }
        let mut about: Vec<Id> = found.iter().map(|e| e.id.clone()).collect();
        about.dedup();
        if about.is_empty() {
            summary.assertions_skipped += 1;
            continue;
        }
        let stated_by_user = assertion.stated_by_user;
        let input = AssertionInput {
            kind: assertion.kind,
            domain: assertion.domain,
            stance: assertion.stance,
            statement: assertion.statement,
            rationale: assertion.rationale,
            valid_from: None,
            paths: Vec::new(),
            about,
            supersedes: None,
            contradicts: Vec::new(),
            status: if stated_by_user {
                AssertionStatus::Confirmed
            } else {
                agent_status
            },
        };
        // Text alone only tells duplicates apart; whether the statement
        // replaces or contradicts a head is the model's call, kept when it
        // names a live head.
        let mut relation = resolve::classify(&graph, &input);
        if let (RelationKind::New, Some(proposed)) = (relation.kind, assertion.relation) {
            let targets_a_live_head = proposed
                .target
                .as_deref()
                .and_then(|id| graph.assertion(id))
                .is_some_and(|a| a.is_head() && resolve::is_live(a.status));
            if matches!(
                proposed.kind,
                RelationKind::Supersedes | RelationKind::Contradicts
            ) && targets_a_live_head
            {
                relation = proposed;
            }
        }
        if relation.kind == RelationKind::Supersedes
            && relation.reasoning.as_deref().map_or(true, str::is_empty)
        {
            relation.reasoning = Some(if input.rationale.trim().is_empty() {
                "Changed in the conversation this was extracted from.".to_string()
            } else {
                input.rationale.clone()
            });
        }
        let evidence = assertion
            .evidence
            .into_iter()
            .filter(|q| !q.quote.trim().is_empty())
            .map(|q| Evidence {
                session_id: provenance.session_id.clone(),
                turn_id: observation.source.reference.clone(),
                quote: q.quote,
            })
            .collect();
        let stamp = stamp(if stated_by_user {
            SourceKind::UserTurn
        } else {
            SourceKind::AgentTurn
        });
        let lands_by_rule = resolve::auto_rule(&graph, &relation, &stamp);
        let p = proposal(
            ProposalPayload::Assertion {
                input,
                stamp,
                relation: relation.clone(),
            },
            evidence,
        );
        store.add_proposal(&p)?;
        if relation.kind == RelationKind::Duplicate {
            store.dismiss_proposal(&p.id, DismissReason::Duplicate, author.clone())?;
            summary.dismissed += 1;
        } else if lands_by_rule {
            match store.accept_proposal(&p.id, ProposalStatus::Auto, author.clone()) {
                Ok(_) => {
                    summary.auto += 1;
                    graph = store.load(project_id)?;
                }
                Err(e) => {
                    tracing::warn!(proposal = %p.id, error = %e, "context extract: assertion did not land; left pending");
                    summary.pending += 1;
                }
            }
        } else {
            summary.pending += 1;
        }
    }

    store.mark_extracted(&observation.id)?;
    Ok(summary)
}

fn sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

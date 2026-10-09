//! One extraction, end to end: record the observation, run the model, keep
//! the run, then turn the answer into proposals. Nothing the extractor says
//! lands on its own — entities become pending proposals, assertions go
//! through `ContextService::record_decision`, where the one write policy
//! holds every extractor write for review (or answers `Duplicate`). Pure
//! over its arguments apart from the service, so tests drive it with a fake
//! extractor. The observation is marked extracted only when the run was
//! read, so the turns of a failed one are picked up again by the next.
//!
//! Nothing the model says about itself is trusted. Whether the user stated an
//! assertion follows from its evidence: each quote is looked for verbatim in
//! the turns the model was shown, and only one found in the user's own text
//! (`trust::find_user_quote`) makes the assertion user-stated — `user_turn`
//! source, `confirmed` once accepted. A quote found in the agent's text is
//! evidence too, but leaves the assertion agent-stated and `provisional`.
//! An assertion with no quote found anywhere is dropped, as is one past the
//! run's caps or in the implementation domain. The store cleans every text
//! it writes; only slugs are normalised here, since the store refuses a bad
//! one.

use std::time::Instant;

use super::input::ExtractInput;
use super::prompt::{self, ProposedEntity, Quote};
use super::Extractor;
use crate::context::model::*;
use crate::context::service::{ContextService, Project};
use crate::context::trust::{self, UserStated, UserTurnText};
use crate::context::{resolve, store, Result};
use crate::database::now_millis;

/// The store's limit on a slug.
const MAX_SLUG: usize = 64;

/// What one run did, for the log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub observation_id: Id,
    /// Entities proposed for review.
    pub entities_proposed: usize,
    /// Entities already in the graph, or with no usable slug.
    pub entities_skipped: usize,
    /// Assertions held for review.
    pub held: usize,
    /// Assertions that restate a current head; nothing kept.
    pub duplicates: usize,
    /// Assertions about nothing the graph or this run knows.
    pub assertions_skipped: usize,
    /// Assertions none of whose quotes were found in the turns; the statement
    /// may be invented, so nothing is held.
    pub unverified: usize,
    /// Entities and assertions past the run's caps; never looked at.
    pub capped: usize,
    /// Implementation-domain assertions; below the extractor's bar, dropped.
    pub off_domain: usize,
    /// Set when the run failed or its output could not be read; nothing was
    /// proposed then.
    pub error: Option<String>,
}

/// A model quote found verbatim in the turns, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifiedQuote {
    turn_id: String,
    /// Set when it was found in the user's text; `None` means the agent's.
    user: Option<UserStated>,
    quote: String,
}

/// Run `extractor` over `input` for `project` and propose what it found.
/// `author` is the extractor's identity (agent and provider); `provenance`
/// names the workspace, checkout, session and turn every record gets
/// stamped with.
pub fn process(
    service: &ContextService,
    project: &Project,
    author: Author,
    provenance: Provenance,
    input: ExtractInput,
    extractor: &dyn Extractor,
) -> Result<Summary> {
    let store = service.store();
    let project_id = project.id.as_str();
    let text = prompt::render(&input);
    // The observation is of the conversation as a whole, agent's replies
    // included; a `user_turn` source is reserved for what the user said.
    let observation = Observation {
        id: new_id(),
        project_id: project_id.to_string(),
        source: Source::new(
            SourceKind::AgentTurn,
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
    let Some(Ok(mut parsed)) = parsed else {
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
    summary.capped = parsed.entities.len().saturating_sub(prompt::MAX_ENTITIES)
        + parsed
            .assertions
            .len()
            .saturating_sub(prompt::MAX_ASSERTIONS);
    if summary.capped > 0 {
        parsed.entities.truncate(prompt::MAX_ENTITIES);
        parsed.assertions.truncate(prompt::MAX_ASSERTIONS);
        tracing::info!(
            observation = %observation.id,
            capped = summary.capped,
            "context extract: proposals past the run's caps dropped"
        );
    }

    let agent_stamp = || Stamp {
        author: author.clone(),
        source: observation.source.clone(),
        provenance: provenance.clone(),
    };

    let graph = store.load(project_id)?;
    // Slugs proposed by this run: an assertion about one carries it as
    // `about_pending`, resolved when the entity is accepted.
    let mut pending_slugs: Vec<String> = Vec::new();
    for entity in parsed.entities {
        let Some(input) = entity_input(entity) else {
            summary.entities_skipped += 1;
            continue;
        };
        let known = std::iter::once(&input.slug)
            .chain(std::iter::once(&input.name))
            .chain(&input.aliases)
            .any(|reference| resolve::entity(&graph, reference).is_ok())
            || pending_slugs.contains(&input.slug);
        if known {
            summary.entities_skipped += 1;
            continue;
        }
        pending_slugs.push(input.slug.clone());
        service.add_proposal(
            project,
            &Proposal {
                id: new_id(),
                project_id: project_id.to_string(),
                observation_id: Some(observation.id.clone()),
                payload: ProposalPayload::Entity {
                    input,
                    stamp: agent_stamp(),
                },
                evidence: Vec::new(),
                status: ProposalStatus::Pending,
                dismiss_reason: None,
                created_at: now_millis(),
                ruled_at: None,
                ruled_by: None,
            },
        )?;
        summary.entities_proposed += 1;
    }

    let user_turns: Vec<UserTurnText> = input
        .turns
        .iter()
        .map(|t| UserTurnText {
            turn_id: t.turn_id.clone(),
            text: t.user.clone(),
        })
        .collect();
    let agent_turns: Vec<(&str, String)> = input
        .turns
        .iter()
        .filter_map(|t| {
            Some((
                t.turn_id.as_str(),
                trust::normalise(t.assistant.as_deref()?),
            ))
        })
        .collect();

    for assertion in parsed.assertions {
        if assertion.domain == Domain::Implementation {
            tracing::debug!(observation = %observation.id, statement = %assertion.statement, "context extract: implementation-domain assertion dropped");
            summary.off_domain += 1;
            continue;
        }
        let (found, unknown) = resolve::entities(&graph, &assertion.about);
        let mut about: Vec<Id> = found.iter().map(|e| e.id.clone()).collect();
        about.dedup();
        let (about_pending, unknown): (Vec<String>, Vec<String>) = unknown
            .into_iter()
            .map(|reference| slug(&reference).unwrap_or(reference))
            .partition(|s| pending_slugs.contains(s));
        if !unknown.is_empty() {
            tracing::warn!(?unknown, statement = %assertion.statement, "context extract: unknown entity slugs dropped");
        }
        if about.is_empty() && about_pending.is_empty() {
            summary.assertions_skipped += 1;
            continue;
        }
        let verified = verify(&user_turns, &agent_turns, &assertion.evidence);
        if verified.is_empty() {
            summary.unverified += 1;
            tracing::info!(
                observation = %observation.id,
                statement = %assertion.statement,
                quotes = assertion.evidence.len(),
                "context extract: no quote found in the turns"
            );
            continue;
        }
        let user_stated = verified.iter().find_map(|q| q.user.clone());
        // A user-stated record states the user's words (the service holds
        // every writer to that); the model's sentence is its reading and
        // goes in the rationale when there is no other.
        let (statement, rationale) = match &user_stated {
            Some(found) => (
                found.quote().to_string(),
                if assertion.rationale.trim().is_empty() {
                    assertion.statement
                } else {
                    assertion.rationale
                },
            ),
            None => (assertion.statement, assertion.rationale),
        };
        let input = AssertionInput {
            kind: assertion.kind,
            domain: assertion.domain,
            stance: assertion.stance,
            statement,
            rationale,
            valid_from: None,
            paths: Vec::new(),
            about,
            supersedes: None,
            contradicts: Vec::new(),
            status: if user_stated.is_some() {
                AssertionStatus::Confirmed
            } else {
                AssertionStatus::Provisional
            },
        };
        // Whether the statement replaces or contradicts a head is the
        // model's call; `New` is left to the store to classify.
        let relation = assertion
            .relation
            .filter(|r| matches!(r.kind, RelationKind::Supersedes | RelationKind::Contradicts));
        let relation = relation.map(|mut r| {
            if r.kind == RelationKind::Supersedes
                && r.reasoning.as_deref().map_or(true, str::is_empty)
            {
                r.reasoning = Some(if input.rationale.trim().is_empty() {
                    "Changed in the conversation this was extracted from.".to_string()
                } else {
                    input.rationale.clone()
                });
            }
            r
        });
        // The service applies the user's quote (statement, source, status);
        // the pipeline only hands over the proof.
        let stamp = agent_stamp();
        let evidence = verified
            .into_iter()
            .map(|q| Evidence {
                session_id: provenance.session_id.clone(),
                turn_id: Some(q.turn_id),
                quote: q.quote,
            })
            .collect();
        let candidate = Candidate {
            input,
            user: user_stated,
            relation,
            evidence,
            about_pending,
            observation_id: Some(observation.id.clone()),
        };
        match service.record_decision(project, candidate, stamp)? {
            Landing::Held { .. } => summary.held += 1,
            Landing::Duplicate { id } => {
                tracing::debug!(observation = %observation.id, head = %id, "context extract: duplicate of a head");
                summary.duplicates += 1;
            }
            other => {
                tracing::warn!(observation = %observation.id, ?other, "context extract: the write policy did not hold an extractor write");
            }
        }
    }

    store.mark_extracted(&observation.id)?;
    Ok(summary)
}

/// A proposed entity ready for the store, or `None` (with a warning) when
/// nothing usable is left of its slug. The texts are the store's to clean.
fn entity_input(entity: ProposedEntity) -> Option<EntityInput> {
    let Some(slug) = slug(&entity.slug) else {
        tracing::warn!(slug = %entity.slug, "context extract: entity skipped; no usable slug");
        return None;
    };
    Some(EntityInput {
        id: None,
        slug,
        kind: entity.kind,
        name: entity.name,
        summary: entity.summary,
        aliases: entity.aliases,
        paths: entity.paths,
    })
}

/// The model's quotes that are in the turns, each with the turn it was found
/// in. The user's turns are searched first, so a quote the agent echoed back
/// is still the user's; `agent_turns` hold the agents' replies already
/// normalised the way `trust` matches. Quotes found nowhere are dropped.
fn verify(
    user_turns: &[UserTurnText],
    agent_turns: &[(&str, String)],
    quotes: &[Quote],
) -> Vec<VerifiedQuote> {
    quotes
        .iter()
        .filter_map(|q| {
            let quote = store::clean_line(&q.quote);
            if let Some(found) = trust::find_user_quote(user_turns, &quote) {
                return Some(VerifiedQuote {
                    turn_id: found.turn_id().to_string(),
                    user: Some(found),
                    quote,
                });
            }
            let needle = trust::normalise(&quote);
            if needle.chars().count() < trust::MIN_QUOTE_CHARS {
                return None;
            }
            let (turn_id, _) = agent_turns
                .iter()
                .rev()
                .find(|(_, reply)| reply.contains(&needle))?;
            Some(VerifiedQuote {
                turn_id: turn_id.to_string(),
                user: None,
                quote,
            })
        })
        .collect()
}

/// `raw` as a slug the store accepts: `[a-z0-9][a-z0-9._-]*`, at most
/// [`MAX_SLUG`] characters. Lowercased; any run of other characters becomes
/// one `-`; leading non-alphanumerics and trailing `-` go. `None` when
/// nothing is left.
fn slug(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if out.is_empty() {
            continue;
        } else if matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.chars().take(MAX_SLUG).collect();
    let out = out.trim_end_matches('-');
    (!out.is_empty()).then(|| out.to_string())
}

fn sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

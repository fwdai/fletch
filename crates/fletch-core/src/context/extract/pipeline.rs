//! One extraction, end to end, over a store: record the observation, run the
//! model, keep the run, then turn the answer into proposals — entities land at
//! once (they are cheap and the assertions need subjects), assertions go
//! through `resolve::classify` (duplicates), the model's own `supersedes` /
//! `contradicts` relation when it names a current head, and the auto rule.
//! Pure over its arguments apart from the store, so tests drive it with a fake
//! extractor. The observation is marked extracted only when the run landed,
//! so the turns of a failed one are picked up again by the next.
//!
//! Nothing the model says about itself is trusted. Whether the user stated an
//! assertion follows from its evidence: each quote is looked for verbatim in
//! the turns the model was shown, and only one found in the user's own text
//! (`trust::find_user_quote`) makes the assertion user-stated — `user_turn`
//! source, confirmed. A quote found in the agent's text is evidence too, but
//! leaves the assertion agent-stated. One with no quote found anywhere is
//! held for review, never landed by rule. The store cleans every text it
//! writes; only slugs are normalised here, since the store refuses a bad one.

use std::time::Instant;

use super::super::model::*;
use super::super::trust::{self, UserStated, UserTurnText};
use super::super::{compile, resolve, store, ContextStore, Result};
use super::input::ExtractInput;
use super::prompt::{self, ProposedEntity, Quote};
use super::Extractor;
use crate::database::now_millis;

/// The store's limit on a slug.
const MAX_SLUG: usize = 64;

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
    /// Assertions none of whose quotes were found in the turns; held for
    /// review (counted in `pending` too, unless dismissed as duplicates).
    pub unverified: usize,
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

    let agent_stamp = || Stamp {
        author: author.clone(),
        source: observation.source.clone(),
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
        let Some(input) = entity_input(entity) else {
            summary.entities_skipped += 1;
            continue;
        };
        let known = std::iter::once(&input.slug)
            .chain(std::iter::once(&input.name))
            .chain(&input.aliases)
            .any(|reference| resolve::entity(&graph, reference).is_ok());
        if known {
            summary.entities_skipped += 1;
            continue;
        }
        let p = proposal(
            ProposalPayload::Entity {
                input,
                stamp: agent_stamp(),
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
        let verified = verify(&user_turns, &agent_turns, &assertion.evidence);
        let user_stated = verified.iter().find_map(|q| q.user.clone());
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
            status: if user_stated.is_some() {
                AssertionStatus::Confirmed
            } else {
                agent_status
            },
        };
        // Text alone only tells duplicates apart; whether the statement
        // replaces or contradicts a head is the model's call, kept when it
        // names a current head.
        let mut relation = resolve::classify(&graph, &input);
        if let (RelationKind::New, Some(proposed)) = (relation.kind, assertion.relation) {
            let targets_a_current_head = proposed
                .target
                .as_deref()
                .and_then(|id| graph.assertion(id))
                .is_some_and(|a| compile::is_current(&graph, a));
            if matches!(
                proposed.kind,
                RelationKind::Supersedes | RelationKind::Contradicts
            ) && targets_a_current_head
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
        let stamp = match &user_stated {
            Some(found) => Stamp {
                source: found.source(),
                ..agent_stamp()
            },
            None => agent_stamp(),
        };
        // No quote found in the turns: the model may have made the statement
        // up, so it waits for a human whatever the rule would say.
        let held = verified.is_empty();
        if held {
            summary.unverified += 1;
            tracing::info!(
                observation = %observation.id,
                statement = %input.statement,
                quotes = assertion.evidence.len(),
                "context extract: no quote found in the turns; held for review"
            );
        }
        let lands_by_rule = !held && resolve::auto_rule(&graph, &relation, &stamp);
        let evidence = verified
            .into_iter()
            .map(|q| Evidence {
                session_id: provenance.session_id.clone(),
                turn_id: Some(q.turn_id),
                quote: q.quote,
            })
            .collect();
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

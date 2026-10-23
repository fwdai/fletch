//! Entity resolution and the conflict check: pure over a [`Graph`].
//!
//! `entity` turns what a caller wrote (an id, a slug, an alias, a name — any
//! case) into one active entity or an error. `classify` says whether a
//! candidate assertion restates a head already recorded about the same
//! entities; `related_heads` lists what a writer must look at first.

use std::collections::HashSet;

use super::model::*;
use super::{ContextError, Result};

/// Resolve one reference. Exact id, then slug (case-insensitive), then alias,
/// then name; `AmbiguousEntity` when a name/alias matches several,
/// `UnknownEntity` when nothing matches. Archived and merged entities do not
/// match (a merged one resolves to its `merged_into`).
pub fn entity<'a>(graph: &'a Graph, reference: &str) -> Result<&'a Entity> {
    if let Some(e) = graph.entity(reference) {
        return redirect(graph, e);
    }
    let wanted = reference.to_lowercase();
    let candidates = || {
        graph
            .entities
            .iter()
            .filter(|e| e.status != EntityStatus::Archived)
    };
    if let Some(e) = candidates().find(|e| e.slug.to_lowercase() == wanted) {
        return redirect(graph, e);
    }
    let tiers: [&dyn Fn(&Entity) -> bool; 2] = [
        &|e| e.aliases.iter().any(|a| a.to_lowercase() == wanted),
        &|e| e.name.to_lowercase() == wanted,
    ];
    for matches in tiers {
        let hits: Vec<&Entity> = candidates().filter(|e| matches(e)).collect();
        match hits.as_slice() {
            [] => continue,
            [one] => return redirect(graph, one),
            many => {
                return Err(ContextError::AmbiguousEntity(
                    reference.to_string(),
                    many.iter().map(|e| e.slug.clone()).collect(),
                ))
            }
        }
    }
    Err(ContextError::UnknownEntity(reference.to_string()))
}

/// A merged entity stands for the one it was merged into. The store keeps a
/// chain one hop long (it compresses on merge); the bound is for a
/// projection it did not get to.
fn redirect<'a>(graph: &'a Graph, e: &'a Entity) -> Result<&'a Entity> {
    let mut current = e;
    for _ in 0..MAX_MERGE_HOPS {
        match (current.status, &current.merged_into) {
            (EntityStatus::Active, _) => return Ok(current),
            (EntityStatus::Merged, Some(into)) => match graph.entity(into) {
                Some(next) => current = next,
                None => break,
            },
            _ => break,
        }
    }
    Err(ContextError::UnknownEntity(e.slug.clone()))
}

const MAX_MERGE_HOPS: usize = 8;

/// Resolve many; unknown references are returned in the second list rather
/// than failing the call (the caller reports them as misses).
pub fn entities<'a>(graph: &'a Graph, references: &[String]) -> (Vec<&'a Entity>, Vec<String>) {
    let mut found = Vec::new();
    let mut unknown = Vec::new();
    for reference in references {
        match entity(graph, reference) {
            Ok(e) => found.push(e),
            Err(_) => unknown.push(reference.clone()),
        }
    }
    (found, unknown)
}

/// The live heads a candidate assertion sits next to: same domain, about at
/// least one of the same entities. What a writer must look at before it can
/// say whether the candidate replaces, contradicts or merely joins them.
pub fn related_heads<'a>(graph: &'a Graph, candidate: &AssertionInput) -> Vec<&'a Assertion> {
    graph
        .assertions
        .iter()
        .filter(|a| super::compile::is_current(graph, a) && a.domain == candidate.domain)
        .filter(|a| a.about.iter().any(|id| candidate.about.contains(id)))
        .collect()
}

/// Word-set overlap at or above which two statements read as one decision in
/// two wordings: Jaccard over the lowercase alphanumeric words of four
/// letters or more, so typing, punctuation and the small words do not count.
pub const TWIN_SIMILARITY: f64 = 0.5;

/// Whether `a` and `b` say the same thing in different words, by
/// [`TWIN_SIMILARITY`]. Text alone: the caller decides what else must match.
pub fn similar(a: &str, b: &str) -> bool {
    let words = |s: &str| -> HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() >= 4)
            .map(str::to_string)
            .collect()
    };
    let (a, b) = (words(a), words(b));
    let union = a.union(&b).count();
    union > 0 && a.intersection(&b).count() as f64 / union as f64 >= TWIN_SIMILARITY
}

/// Whether `a` is the same claim as `candidate` in shape: same kind, domain
/// and stance, about at least one of the same entities. What a twin or a
/// restatement must share before its words are compared, so a decision
/// about one component never stands in for the same words about another.
pub fn same_claim(a: &Assertion, candidate: &AssertionInput) -> bool {
    a.kind == candidate.kind
        && a.domain == candidate.domain
        && a.stance == candidate.stance
        && a.about.iter().any(|id| candidate.about.contains(id))
}

/// A live assertion a later record replaced, that `candidate` would bring
/// back: the [`same_claim`], in [`similar`] words. Retracted and abandoned
/// ones were withdrawn, not replaced, and do not count.
pub fn retired_restatement<'a>(
    graph: &'a Graph,
    candidate: &AssertionInput,
) -> Option<&'a Assertion> {
    graph.assertions.iter().find(|a| {
        is_live(a.status)
            && !super::compile::is_current(graph, a)
            && same_claim(a, candidate)
            && similar(&a.statement, &candidate.statement)
    })
}

/// How `candidate` relates to the existing heads about its entities, as far
/// as text alone can tell: `Duplicate` when a live head of the same kind,
/// domain and stance has the same statement (normalised: case, whitespace,
/// trailing punctuation), `New` otherwise. Whether a different statement
/// *replaces* or *contradicts* a head is a judgment — two decisions about one
/// entity are usually both true — so `Supersedes` and `Contradicts` come only
/// from a caller that says so (an agent's `supersedes`, the extractor's
/// relation) and never from here. See [`related_heads`] for what to show them.
pub fn classify(graph: &Graph, candidate: &AssertionInput) -> ProposedRelation {
    let statement = normalise(&candidate.statement);
    let duplicate = related_heads(graph, candidate).into_iter().find(|a| {
        a.kind == candidate.kind
            && a.stance == candidate.stance
            && normalise(&a.statement) == statement
    });
    match duplicate {
        Some(dup) => relation(RelationKind::Duplicate, dup),
        None => ProposedRelation {
            kind: RelationKind::New,
            target: None,
            reasoning: None,
        },
    }
}

fn relation(kind: RelationKind, target: &Assertion) -> ProposedRelation {
    ProposedRelation {
        kind,
        target: Some(target.id.clone()),
        reasoning: None,
    }
}

/// Retracted and abandoned assertions are out of play for classification.
pub(super) fn is_live(status: AssertionStatus) -> bool {
    !matches!(
        status,
        AssertionStatus::Retracted | AssertionStatus::Abandoned
    )
}

/// Lowercase, single-spaced, without trailing `.`/`!`, so restatements that
/// differ only in typing compare equal.
pub(super) fn normalise(statement: &str) -> String {
    statement
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_end_matches(['.', '!'])
        .to_string()
}

#[cfg(test)]
#[path = "tests/resolve.rs"]
mod tests;

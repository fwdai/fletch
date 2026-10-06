//! Compilation: one project's [`Graph`] → the [`Bundle`] an agent or the UI
//! reads. Pure over the graph, so every later signal (anchors, embeddings,
//! verification passes) is one more flag or ranking input here.
//!
//! Steps:
//! 1. Entry entities: the vision always; those named in `query.entities`
//!    (resolved by `resolve::entity`), matched by `query.query` (name,
//!    summary, aliases, statement, rationale — plain substring / token match
//!    for now), or anchored under `query.paths`. Names that match nothing go
//!    to `misses`. An empty query (nothing named, no text, no paths) means
//!    the whole map: every active entity.
//! 2. Expand one hop over `relates` (both directions).
//! 3. Assertions: heads `about` the assembled entities, current as of
//!    `query.as_of` (`valid_from <= as_of` and no superseder with
//!    `valid_from <= as_of`), excluding `retracted` and `abandoned` and
//!    those about archived entities only. Flag `provisional`, contradictions
//!    among heads, and `has_history`. With `include_history`, attach
//!    predecessors newest first.
//! 4. Rank entities (named/path > query > neighbour; then edge count, then
//!    recency) and drop from the bottom until the rendered size fits
//!    `budget_chars`; the vision and the warnings are never dropped.
//!    Dropped ids go to `truncated`.
//! 5. `warnings`: human-readable lines for contradictions, provisional
//!    decisions, and misses.

use std::collections::HashSet;

use super::model::*;
use super::render::render_markdown;
use super::resolve::{self, is_live};
use super::DEFAULT_BUDGET_CHARS;

/// How many text-query matches enter the bundle.
const QUERY_HITS: usize = 10;

/// Compile `query` against `graph`. `vision_fallback` is the roadmap brief,
/// shown as the vision when no vision entity exists.
pub fn compile(graph: &Graph, query: &CompileQuery, vision_fallback: Option<String>) -> Bundle {
    let mut entries = Entries::default();
    let vision = graph.vision();
    if let Some(v) = vision {
        entries.add(v, EntryReason::Vision);
    }

    let has_text = query.query.as_deref().is_some_and(|q| !q.trim().is_empty());
    let is_map = query.entities.is_empty() && query.paths.is_empty() && !has_text;
    let mut misses = Vec::new();
    if is_map {
        for e in active(graph) {
            entries.add(e, EntryReason::Named);
        }
    } else {
        let (named, unknown) = resolve::entities(graph, &query.entities);
        misses = unknown;
        for e in named {
            entries.add(e, EntryReason::Named);
        }
        for e in by_text(graph, query) {
            entries.add(e, EntryReason::Query);
        }
        for e in by_paths(graph, &query.paths) {
            entries.add(e, EntryReason::Path);
        }
        let seeds: Vec<&Entity> = entries.seeds();
        for seed in seeds {
            for other in neighbours(graph, &seed.id) {
                entries.add(other, EntryReason::Neighbour);
            }
        }
    }

    let mut ranked = entries.ranked(graph);
    let budget = match query.budget_chars {
        0 => DEFAULT_BUDGET_CHARS,
        n => n,
    };
    let mut truncated = Vec::new();
    loop {
        let bundle = assemble(
            graph,
            query,
            &ranked,
            vision_fallback.clone(),
            &misses,
            &truncated,
        );
        if render_markdown(&bundle).len() <= budget {
            return bundle;
        }
        match ranked.last() {
            Some((e, reason)) if *reason != EntryReason::Vision => {
                truncated.push(e.id.clone());
                ranked.pop();
            }
            _ => return bundle,
        }
    }
}

/// Head assertions about `entity_id`, current as of `as_of` (`None` = now).
pub fn heads_about<'a>(
    graph: &'a Graph,
    entity_id: &str,
    as_of: Option<i64>,
) -> Vec<&'a Assertion> {
    graph
        .assertions
        .iter()
        .filter(|a| a.about.iter().any(|id| id == entity_id) && is_current(graph, a, as_of))
        .collect()
}

/// The supersession chain behind `assertion_id`, newest predecessor first.
pub fn history<'a>(graph: &'a Graph, assertion_id: &str) -> Vec<&'a Assertion> {
    let mut chain = Vec::new();
    let mut current = graph.assertion(assertion_id);
    while let Some(prev) = current
        .and_then(|a| a.supersedes.as_ref())
        .and_then(|s| graph.assertion(&s.id))
    {
        if chain.len() >= graph.assertions.len() {
            break;
        }
        chain.push(prev);
        current = Some(prev);
    }
    chain
}

/// Live, in force at `as_of`, and not yet replaced at `as_of`. A superseder
/// that is itself retracted or abandoned replaces nothing: the head it stood
/// on stays current, so an abandoned branch's rewrite cannot erase what it
/// rewrote.
fn is_current(graph: &Graph, a: &Assertion, as_of: Option<i64>) -> bool {
    let as_of = as_of.unwrap_or(i64::MAX);
    let replaced = a
        .superseded_by
        .as_deref()
        .and_then(|id| graph.assertion(id))
        .is_some_and(|s| is_live(s.status) && s.valid_from <= as_of);
    is_live(a.status) && a.valid_from <= as_of && !replaced
}

// ---------------------------------------------------------------------------
// Entry selection

/// Entities picked so far, each with the strongest reason seen for it.
#[derive(Default)]
struct Entries<'a>(Vec<(&'a Entity, EntryReason)>);

impl<'a> Entries<'a> {
    fn add(&mut self, entity: &'a Entity, reason: EntryReason) {
        match self.0.iter_mut().find(|(e, _)| e.id == entity.id) {
            Some(slot) if tier(reason) < tier(slot.1) => slot.1 = reason,
            Some(_) => {}
            None => self.0.push((entity, reason)),
        }
    }

    /// Hop sources: everything but the vision, which would pull in the map.
    fn seeds(&self) -> Vec<&'a Entity> {
        self.0
            .iter()
            .filter(|(_, r)| *r != EntryReason::Vision)
            .map(|(e, _)| *e)
            .collect()
    }

    fn ranked(self, graph: &Graph) -> Vec<(&'a Entity, EntryReason)> {
        let mut v = self.0;
        v.sort_by_key(|(e, r)| {
            (
                tier(*r),
                std::cmp::Reverse(degree(graph, &e.id)),
                std::cmp::Reverse(e.recorded_at),
            )
        });
        v
    }
}

fn tier(reason: EntryReason) -> u8 {
    match reason {
        EntryReason::Vision => 0,
        EntryReason::Named | EntryReason::Path => 1,
        EntryReason::Query => 2,
        EntryReason::Neighbour => 3,
    }
}

fn degree(graph: &Graph, id: &str) -> usize {
    graph
        .relations
        .iter()
        .filter(|r| r.from == id || r.to == id)
        .count()
}

fn active(graph: &Graph) -> impl Iterator<Item = &Entity> {
    graph
        .entities
        .iter()
        .filter(|e| e.status == EntityStatus::Active)
}

fn neighbours<'a>(graph: &'a Graph, id: &str) -> Vec<&'a Entity> {
    graph
        .relations
        .iter()
        .filter_map(|r| match (r.from == id, r.to == id) {
            (true, _) => Some(r.to.as_str()),
            (_, true) => Some(r.from.as_str()),
            _ => None,
        })
        .filter_map(|other| graph.entity(other))
        .filter(|e| e.status == EntityStatus::Active)
        .collect()
}

fn text_tokens(query: &CompileQuery) -> Vec<String> {
    query
        .query
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|t| t.len() >= 3)
        .collect()
}

/// Entities scored by token hits in their own text and in the statements
/// about them; the top [`QUERY_HITS`].
fn by_text<'a>(graph: &'a Graph, query: &CompileQuery) -> Vec<&'a Entity> {
    let tokens = text_tokens(query);
    if tokens.is_empty() {
        return Vec::new();
    }
    let hits = |text: &str| {
        let text = text.to_lowercase();
        tokens.iter().filter(|t| text.contains(t.as_str())).count()
    };
    let mut scored: Vec<(&Entity, usize)> = active(graph)
        .map(|e| {
            let own = hits(&format!(
                "{} {} {} {}",
                e.slug,
                e.name,
                e.aliases.join(" "),
                e.summary
            ));
            let about = graph
                .assertions
                .iter()
                .filter(|a| a.is_head() && is_live(a.status) && a.about.contains(&e.id))
                .map(|a| hits(&format!("{} {}", a.statement, a.rationale)))
                .sum::<usize>();
            (e, own + about)
        })
        .filter(|(_, score)| *score > 0)
        .collect();
    scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    scored
        .into_iter()
        .take(QUERY_HITS)
        .map(|(e, _)| e)
        .collect()
}

/// Entities anchored under a query path (or anchoring one), directly or
/// through an assertion about them.
fn by_paths<'a>(graph: &'a Graph, paths: &[String]) -> Vec<&'a Entity> {
    let wanted: Vec<String> = paths.iter().map(|p| trim_path(p)).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let anchored = |paths: &[String]| {
        paths
            .iter()
            .map(|p| trim_path(p))
            .any(|p| wanted.iter().any(|w| path_overlaps(&p, w)))
    };
    let mut out: Vec<&Entity> = active(graph).filter(|e| anchored(&e.paths)).collect();
    for a in graph
        .assertions
        .iter()
        .filter(|a| a.is_head() && is_live(a.status) && anchored(&a.paths))
    {
        for e in a.about.iter().filter_map(|id| graph.entity(id)) {
            if e.status == EntityStatus::Active && !out.iter().any(|o| o.id == e.id) {
                out.push(e);
            }
        }
    }
    out
}

fn trim_path(p: &str) -> String {
    p.trim_start_matches("./").trim_end_matches('/').to_string()
}

/// One is the other or a directory above it.
fn path_overlaps(a: &str, b: &str) -> bool {
    let under = |short: &str, long: &str| {
        short.is_empty() || long == short || long.starts_with(&format!("{short}/"))
    };
    under(a, b) || under(b, a)
}

// ---------------------------------------------------------------------------
// Assembly

fn assemble(
    graph: &Graph,
    query: &CompileQuery,
    ranked: &[(&Entity, EntryReason)],
    vision_fallback: Option<String>,
    misses: &[String],
    truncated: &[Id],
) -> Bundle {
    let ids: HashSet<&str> = ranked.iter().map(|(e, _)| e.id.as_str()).collect();
    let entities = ranked
        .iter()
        .map(|(e, reason)| BundleEntity {
            entity: (*e).clone(),
            reason: *reason,
            relations: graph
                .relations
                .iter()
                .filter(|r| {
                    (r.from == e.id && ids.contains(r.to.as_str()))
                        || (r.to == e.id && ids.contains(r.from.as_str()))
                })
                .cloned()
                .collect(),
        })
        .collect();

    let mut heads: Vec<&Assertion> = Vec::new();
    for (e, _) in ranked {
        for a in heads_about(graph, &e.id, query.as_of) {
            if !heads.iter().any(|h| h.id == a.id) {
                heads.push(a);
            }
        }
    }

    let mut seen_pairs = HashSet::new();
    let mut contradictions = Vec::new();
    let assertions: Vec<BundleAssertion> = heads
        .iter()
        .map(|a| {
            let contradicted_by = contradicted_by(graph, a, query.as_of);
            for other in &contradicted_by {
                let (x, y) = if a.id < *other {
                    (&a.id, other)
                } else {
                    (other, &a.id)
                };
                if seen_pairs.insert((x.clone(), y.clone())) {
                    contradictions.push(Contradiction {
                        a: x.clone(),
                        b: y.clone(),
                        reasoning: None,
                    });
                }
            }
            BundleAssertion {
                assertion: (*a).clone(),
                flags: AssertionFlags {
                    provisional: a.status == AssertionStatus::Provisional,
                    contradicted_by,
                    has_history: a.supersedes.is_some(),
                },
                history: if query.include_history {
                    history(graph, &a.id).into_iter().cloned().collect()
                } else {
                    Vec::new()
                },
            }
        })
        .collect();

    let vision = graph.vision().cloned();
    let vision_fallback = vision.is_none().then_some(vision_fallback).flatten();
    let warnings = warnings(graph, &assertions, &contradictions, misses, truncated);
    Bundle {
        project_id: graph.project_id.clone(),
        vision,
        vision_fallback,
        entities,
        assertions,
        contradictions,
        misses: misses.to_vec(),
        warnings,
        truncated: truncated.to_vec(),
    }
}

/// Current heads `a` is in tension with, whichever side recorded the link.
fn contradicted_by(graph: &Graph, a: &Assertion, as_of: Option<i64>) -> Vec<Id> {
    let mut out: Vec<Id> = Vec::new();
    let linked = a.contradicts.iter().cloned().chain(
        graph
            .assertions
            .iter()
            .filter(|o| o.contradicts.contains(&a.id))
            .map(|o| o.id.clone()),
    );
    for id in linked {
        let current = graph
            .assertion(&id)
            .is_some_and(|o| is_current(graph, o, as_of));
        if id != a.id && current && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn warnings(
    graph: &Graph,
    assertions: &[BundleAssertion],
    contradictions: &[Contradiction],
    misses: &[String],
    truncated: &[Id],
) -> Vec<String> {
    let statement = |id: &str| {
        graph
            .assertion(id)
            .map_or_else(|| id.to_string(), |a| a.statement.clone())
    };
    let mut out: Vec<String> = contradictions
        .iter()
        .map(|c| format!("Unresolved: {} vs {}", statement(&c.a), statement(&c.b)))
        .collect();
    let provisional = assertions.iter().filter(|a| a.flags.provisional).count();
    if provisional > 0 {
        out.push(format!(
            "{provisional} decision(s) are provisional: recorded from a branch that has not merged"
        ));
    }
    if !misses.is_empty() {
        out.push(format!("No context recorded for: {}", misses.join(", ")));
    }
    if !truncated.is_empty() {
        out.push(format!(
            "Truncated to fit the budget: {} entities dropped",
            truncated.len()
        ));
    }
    out
}

#[cfg(test)]
#[path = "tests/compile.rs"]
mod tests;

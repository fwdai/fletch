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
//! 3. Assertions: the current heads `about` the assembled entities
//!    (`is_current`: live, and nothing live further down the supersession
//!    chain), excluding those about archived entities only. Flag `provisional`, contradictions
//!    among heads, and `has_history`. With `include_history`, attach
//!    predecessors newest first.
//! 4. Rank entities (named/path > query > neighbour; then edge count, then
//!    recency) and drop from the bottom until the rendered size fits
//!    `budget_chars`; the vision and the warnings are never dropped.
//!    Dropped ids go to `truncated`.
//! 5. `warnings`: human-readable lines for contradictions, provisional
//!    decisions, misses, and — given the reader's checkout — every path
//!    anchor of what is served that the checkout does not have.
//!
//! The checkout is the one input that is not the graph: whether an anchor
//! still exists is a fact about a working tree, read at retrieval and never
//! stored.
//!
//! [`overview`] is the other mode (`query.overview`): the fixed picture every
//! agent on the project gets at spawn, fitted by the same loop.

use std::collections::HashSet;
use std::path::Path;

use super::model::*;
use super::render::render_markdown;
use super::resolve::{self, is_live};
use super::{DEFAULT_BUDGET_CHARS, OVERVIEW_BUDGET_CHARS};

/// How many text-query matches enter the bundle.
const QUERY_HITS: usize = 10;

/// Compile `query` against `graph`. `checkout` is the reader's working tree,
/// that path anchors are checked against.
pub fn compile(graph: &Graph, query: &CompileQuery, checkout: Option<&Path>) -> Bundle {
    if query.overview {
        return overview(graph, query.budget_chars);
    }
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

    let ranked = entries.ranked(graph);
    let missing = checkout.map(|root| missing_anchors(graph, &ranked, root));
    let budget = match query.budget_chars {
        0 => DEFAULT_BUDGET_CHARS,
        n => n,
    };
    fit(ranked, 0, budget, |ranked, truncated, _| {
        assemble(graph, query, ranked, &misses, truncated, missing.as_ref())
    })
}

/// The spawn-time overview, the same for every agent on the project: the
/// vision, the adopted current constraints in the business and architectural
/// domains, and every other active entity (rendered as a module legend and a
/// slug index). Entities the extractor minted are left out until something
/// else revised them: model output never writes itself into the next model's
/// instructions. Fitted to `budget_chars` (0 means [`OVERVIEW_BUDGET_CHARS`])
/// by the loop [`compile`] uses. The vision is never dropped; the constraints
/// go only once every other entity has, lowest-ranked first
/// ([`constraint_rank`]), so the overview meets its budget however many
/// there are.
pub fn overview(graph: &Graph, budget_chars: usize) -> Bundle {
    let mut entries = Entries::default();
    for e in active(graph).filter(|e| e.author.kind != AuthorKind::Extractor) {
        match e.kind {
            EntityKind::Vision if graph.vision().is_some_and(|v| v.id == e.id) => {
                entries.add(e, EntryReason::Vision)
            }
            EntityKind::Vision => {}
            _ => entries.add(e, EntryReason::Named),
        }
    }
    let mut constraints: Vec<&Assertion> = graph
        .assertions
        .iter()
        .filter(|a| {
            a.kind == AssertionKind::Constraint
                && a.stance == Stance::Adopted
                && a.domain != Domain::Implementation
                && is_current(graph, a)
        })
        .collect();
    constraints.sort_by_key(|a| constraint_rank(a));
    let (assertions, contradictions) = bundle_assertions(graph, &constraints, false);
    let budget = match budget_chars {
        0 => OVERVIEW_BUDGET_CHARS,
        n => n,
    };
    let sheddable = assertions.len();
    fit(
        entries.ranked(graph),
        sheddable,
        budget,
        |ranked, truncated, kept| {
            let (shown, shed) = assertions.split_at(kept);
            let ids: HashSet<&str> = shown.iter().map(|a| a.assertion.id.as_str()).collect();
            let contradictions: Vec<Contradiction> = contradictions
                .iter()
                .filter(|c| ids.contains(c.a.as_str()) || ids.contains(c.b.as_str()))
                .cloned()
                .collect();
            let mut warnings = warnings(graph, shown, &contradictions, &[], truncated);
            if !shed.is_empty() {
                warnings.push(format!(
                    "Truncated to fit the budget: {} constraints not shown; \
                     call context_get for the full set",
                    shed.len()
                ));
            }
            Bundle {
                project_id: graph.project_id.clone(),
                vision: ranked
                    .iter()
                    .find(|(_, r)| *r == EntryReason::Vision)
                    .map(|(e, _)| (*e).clone()),
                overview: true,
                entities: bundle_entities(graph, ranked),
                warnings,
                assertions: shown.to_vec(),
                contradictions,
                misses: Vec::new(),
                truncated: truncated
                    .iter()
                    .cloned()
                    .chain(shed.iter().rev().map(|a| a.assertion.id.clone()))
                    .collect(),
            }
        },
    )
}

/// Overview order: what the user stated before what anyone else did, merged
/// before provisional, then newest first. "The user stated" is the user
/// writing it or a verified quote of theirs (`user_turn` source), whoever
/// recorded it.
fn constraint_rank(a: &Assertion) -> (bool, bool, std::cmp::Reverse<i64>) {
    let user_stated = a.author.kind == AuthorKind::User || a.source.kind == SourceKind::UserTurn;
    (
        !user_stated,
        a.status == AssertionStatus::Provisional,
        std::cmp::Reverse(a.recorded_at),
    )
}

/// Drops the lowest-ranked entity until the rendered bundle fits `budget`;
/// the vision is never dropped. Dropped ids go to `truncated`. Once only the
/// vision is left, `build` is asked to keep one fewer of its `sheddable`
/// lower-priority items per pass (the overview's constraints), down to none.
fn fit<'a>(
    mut ranked: Vec<(&'a Entity, EntryReason)>,
    sheddable: usize,
    budget: usize,
    build: impl Fn(&[(&'a Entity, EntryReason)], &[Id], usize) -> Bundle,
) -> Bundle {
    let mut truncated = Vec::new();
    let mut kept = sheddable;
    loop {
        let bundle = build(&ranked, &truncated, kept);
        if render_markdown(&bundle).len() <= budget {
            return bundle;
        }
        match ranked.last() {
            Some((e, reason)) if *reason != EntryReason::Vision => {
                truncated.push(e.id.clone());
                ranked.pop();
            }
            _ if kept > 0 => kept -= 1,
            _ => return bundle,
        }
    }
}

/// Current assertions about `entity_id`.
pub fn heads_about<'a>(graph: &'a Graph, entity_id: &str) -> Vec<&'a Assertion> {
    graph
        .assertions
        .iter()
        .filter(|a| a.about.iter().any(|id| id == entity_id) && is_current(graph, a))
        .collect()
}

/// The ids of every current assertion — the one answer to "what stands now"
/// that compile, the conflict check, the stats and the UI all share.
pub fn current_heads(graph: &Graph) -> HashSet<Id> {
    graph
        .assertions
        .iter()
        .filter(|a| is_current(graph, a))
        .map(|a| a.id.clone())
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

/// Whether an assertion stands now: it is live (not retracted or abandoned)
/// and nothing live has replaced it. "Replaced" follows the supersession
/// chain forward: a retracted or abandoned superseder replaces nothing on its
/// own (an abandoned branch's rewrite cannot erase what it rewrote), but a
/// live assertion further down the chain does.
///
/// This is the single definition of a current head. `Assertion::is_head`
/// is the structural fact (no successor at all) and must not stand in for it.
pub fn is_current(graph: &Graph, a: &Assertion) -> bool {
    if !is_live(a.status) {
        return false;
    }
    let mut next = a
        .superseded_by
        .as_deref()
        .and_then(|id| graph.assertion(id));
    let mut hops = 0;
    while let Some(s) = next {
        if is_live(s.status) {
            return false;
        }
        hops += 1;
        if hops > graph.assertions.len() {
            break;
        }
        next = s
            .superseded_by
            .as_deref()
            .and_then(|id| graph.assertion(id));
    }
    true
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
                .filter(|a| is_current(graph, a) && a.about.contains(&e.id))
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
        .filter(|a| is_current(graph, a) && anchored(&a.paths))
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

/// The anchors of the ranked entities and of the heads about them that
/// `root` does not have. Read once per compile, before the budget loop. An
/// anchor that is not a plain repo-relative path (absolute, climbing out with
/// `..`, a glob) is not checked.
fn missing_anchors(
    graph: &Graph,
    ranked: &[(&Entity, EntryReason)],
    root: &Path,
) -> HashSet<String> {
    let checkable = |p: &str| {
        !p.is_empty()
            && !Path::new(p).is_absolute()
            && !p.split('/').any(|s| s == "..")
            && !p.contains(['*', '?', '['])
    };
    ranked
        .iter()
        .flat_map(|(e, _)| {
            e.paths.iter().chain(
                heads_about(graph, &e.id)
                    .into_iter()
                    .flat_map(|a| a.paths.iter()),
            )
        })
        .map(|p| trim_path(p))
        .filter(|p| checkable(p) && !root.join(p).exists())
        .collect()
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
    misses: &[String],
    truncated: &[Id],
    missing: Option<&HashSet<String>>,
) -> Bundle {
    let mut heads: Vec<&Assertion> = Vec::new();
    for (e, _) in ranked {
        for a in heads_about(graph, &e.id) {
            if !heads.iter().any(|h| h.id == a.id) {
                heads.push(a);
            }
        }
    }
    let (assertions, contradictions) = bundle_assertions(graph, &heads, query.include_history);
    let mut warnings = warnings(graph, &assertions, &contradictions, misses, truncated);
    if let Some(missing) = missing {
        warnings.extend(stale_anchors(ranked, &assertions, missing));
    }
    Bundle {
        project_id: graph.project_id.clone(),
        vision: graph.vision().cloned(),
        overview: false,
        entities: bundle_entities(graph, ranked),
        assertions,
        contradictions,
        misses: misses.to_vec(),
        warnings,
        truncated: truncated.to_vec(),
    }
}

/// The ranked entities, each with its relations to the others in the bundle.
fn bundle_entities(graph: &Graph, ranked: &[(&Entity, EntryReason)]) -> Vec<BundleEntity> {
    let ids: HashSet<&str> = ranked.iter().map(|(e, _)| e.id.as_str()).collect();
    ranked
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
        .collect()
}

/// `heads` flagged, with the contradictions among the current heads they
/// touch, each pair once.
fn bundle_assertions(
    graph: &Graph,
    heads: &[&Assertion],
    include_history: bool,
) -> (Vec<BundleAssertion>, Vec<Contradiction>) {
    let mut seen_pairs = HashSet::new();
    let mut contradictions = Vec::new();
    let assertions: Vec<BundleAssertion> = heads
        .iter()
        .map(|a| {
            let contradicted_by = contradicted_by(graph, a);
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
                history: if include_history {
                    history(graph, &a.id).into_iter().cloned().collect()
                } else {
                    Vec::new()
                },
            }
        })
        .collect();
    (assertions, contradictions)
}

/// Current heads `a` is in tension with, whichever side recorded the link.
fn contradicted_by(graph: &Graph, a: &Assertion) -> Vec<Id> {
    let mut out: Vec<Id> = Vec::new();
    let linked = a.contradicts.iter().cloned().chain(
        graph
            .assertions
            .iter()
            .filter(|o| o.contradicts.contains(&a.id))
            .map(|o| o.id.clone()),
    );
    for id in linked {
        let current = graph.assertion(&id).is_some_and(|o| is_current(graph, o));
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

/// One line per served anchor in `missing`: the code it pointed at has moved
/// or gone since it was recorded.
fn stale_anchors(
    ranked: &[(&Entity, EntryReason)],
    assertions: &[BundleAssertion],
    missing: &HashSet<String>,
) -> Vec<String> {
    let gone = |paths: &[String]| -> Vec<String> {
        paths
            .iter()
            .map(|p| trim_path(p))
            .filter(|p| missing.contains(p))
            .collect()
    };
    let entities = ranked.iter().flat_map(|(e, _)| {
        gone(&e.paths).into_iter().map(|p| {
            format!(
                "`{p}` (anchor of `{}`) does not exist in this checkout",
                e.slug
            )
        })
    });
    let assertions = assertions.iter().flat_map(|a| {
        gone(&a.assertion.paths).into_iter().map(|p| {
            format!(
                "`{p}` (anchor of \"{}\") does not exist in this checkout",
                a.assertion.statement
            )
        })
    });
    entities.chain(assertions).collect()
}

#[cfg(test)]
#[path = "tests/compile.rs"]
mod tests;

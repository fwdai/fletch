//! The tidy pass: what a project's log already holds that the ingester's
//! rules of today would not have landed. Two shapes, both left behind by the
//! merged PR's `## Decisions` section before `capture::ingest` learnt to
//! skip them:
//!
//! - a **twin**: the PR's line restating, in other words, the record the
//!   agent had already made from the same workspace (same kind, domain and
//!   stance, about the same entities) — two heads for one decision;
//! - a **restatement of a retired assertion**: a head whose statement is one
//!   a later record had replaced — the replaced statement brought back as
//!   current next to its own successor.
//!
//! Pure over the graph. Nothing here writes: the command lists the
//! candidates, and retracts each one the user confirms as one `retracted`
//! event carrying the reason, so the log keeps what was tidied and why.

use serde::{Deserialize, Serialize};

use super::compile::{history, is_current};
use super::model::*;
use super::resolve::{is_live, normalise};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TidyReason {
    TwinOf,
    RestatesRetired,
}

/// One assertion the pass would retract, and the one it is judged against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TidyCandidate {
    pub retract: Id,
    pub statement: String,
    pub reason: TidyReason,
    pub of: Id,
    pub of_statement: String,
}

impl TidyCandidate {
    /// The reason the `retracted` event records.
    pub fn reasoning(&self) -> String {
        match self.reason {
            TidyReason::TwinOf => format!("tidy: twin of {}", self.of),
            TidyReason::RestatesRetired => format!("tidy: restates retired {}", self.of),
        }
    }
}

/// Every current assertion the pass would retract, with why. An assertion
/// is listed once, for the first rule it meets.
pub fn candidates(graph: &Graph) -> Vec<TidyCandidate> {
    let current: Vec<&Assertion> = graph
        .assertions
        .iter()
        .filter(|a| is_current(graph, a))
        .collect();
    current
        .iter()
        .filter_map(|a| {
            if let Some(old) = retired_by_a_successor(graph, a) {
                return Some(candidate(a, TidyReason::RestatesRetired, old));
            }
            if a.source.kind == SourceKind::Pr {
                if let Some(b) = current.iter().find(|b| twin_of(a, b)) {
                    return Some(candidate(a, TidyReason::TwinOf, b));
                }
            }
            None
        })
        .collect()
}

/// A live assertion a later record replaced, whose statement `a` restates.
/// Retracted and abandoned ones do not count: their statement was withdrawn,
/// not replaced, and a retracted twin's wording must not make its original
/// the next candidate. Nor does anything in `a`'s own chain: a head that
/// restates what it itself replaced is a rewording with a new rationale.
fn retired_by_a_successor<'a>(graph: &'a Graph, a: &Assertion) -> Option<&'a Assertion> {
    let wanted = normalise(&a.statement);
    let own: Vec<&str> = history(graph, &a.id)
        .iter()
        .map(|h| h.id.as_str())
        .collect();
    graph.assertions.iter().find(|old| {
        old.id != a.id
            && is_live(old.status)
            && !is_current(graph, old)
            && !own.contains(&old.id.as_str())
            && normalise(&old.statement) == wanted
    })
}

/// `b` is the record `a`, a merged PR's line, restated: not from a PR
/// itself, from the same workspace, earlier, of the same kind, domain and
/// stance, about at least one of the same entities.
fn twin_of(a: &Assertion, b: &Assertion) -> bool {
    b.id != a.id
        && b.source.kind != SourceKind::Pr
        && b.provenance.workspace_id.is_some()
        && b.provenance.workspace_id == a.provenance.workspace_id
        && b.recorded_at <= a.recorded_at
        && b.kind == a.kind
        && b.domain == a.domain
        && b.stance == a.stance
        && b.about.iter().any(|id| a.about.contains(id))
}

fn candidate(a: &Assertion, reason: TidyReason, of: &Assertion) -> TidyCandidate {
    TidyCandidate {
        retract: a.id.clone(),
        statement: a.statement.clone(),
        reason,
        of: of.id.clone(),
        of_statement: of.statement.clone(),
    }
}

#[cfg(test)]
#[path = "tests/tidy.rs"]
mod tests;

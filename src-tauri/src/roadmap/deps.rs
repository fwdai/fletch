//! Dependency-graph rules: may this item depend on these codes?
//!
//! Unknown/rejected codes and cycles must fail before write — a loop wedges the queue.


use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::types::{ItemStatus, RoadmapItem};

pub type Graph = BTreeMap<String, Vec<String>>;

pub const BATCH_PREFIX: char = '#';

const LISTED: usize = 12;

pub fn graph_of(items: &[RoadmapItem]) -> Graph {
    items
        .iter()
        .map(|i| (i.code.clone(), i.deps.clone()))
        .collect()
}

pub fn rejected_of(items: &[RoadmapItem]) -> BTreeSet<String> {
    items
        .iter()
        .filter(|i| i.status == ItemStatus::Rejected)
        .map(|i| i.code.clone())
        .collect()
}

fn placeholder(index: usize) -> String {
    format!("{BATCH_PREFIX}{}", index + 1)
}

pub fn batch_index(dep: &str, batch_len: usize) -> Option<usize> {
    let n: usize = dep.strip_prefix(BATCH_PREFIX)?.trim().parse().ok()?;
    (1..=batch_len).contains(&n).then(|| n - 1)
}

pub fn validate_new(
    graph: &Graph,
    rejected: &BTreeSet<String>,
    deps: &[String],
) -> Result<(), String> {
    check_codes(graph, rejected, None, deps, "")
}

pub fn validate_edit(
    graph: &Graph,
    rejected: &BTreeSet<String>,
    code: &str,
    deps: &[String],
) -> Result<(), String> {
    check_codes(graph, rejected, Some(code), deps, "")?;
    let mut after = graph.clone();
    after.insert(code.to_string(), deps.to_vec());
    match find_cycle(&after, code) {
        Some(cycle) => Err(loop_message(code, &cycle)),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatchRefusal {
    pub at: usize,
    pub message: String,
}

pub fn validate_batch(
    graph: &Graph,
    rejected: &BTreeSet<String>,
    batch: &[Vec<String>],
) -> Result<(), BatchRefusal> {
    let refuse = |at: usize, message: String| BatchRefusal { at, message };
    let batch_note = ", and not a \"#n\" reference to an item in this batch";
    let mut merged = graph.clone();
    for (n, deps) in batch.iter().enumerate() {
        for d in deps {
            if d.starts_with(BATCH_PREFIX) {
                let position = batch_index(d, batch.len()).ok_or_else(|| {
                    refuse(
                        n,
                        format!(
                            "`deps` names {d:?}, which is not an item in this batch — a batch \
                             reference is \"#1\" to \"#{}\", counting the items in the order you \
                             sent them",
                            batch.len()
                        ),
                    )
                })?;
                if position == n {
                    return Err(refuse(
                        n,
                        format!("{d} is this item — an item cannot depend on itself"),
                    ));
                }
            } else if !graph.contains_key(d.as_str()) {
                return Err(refuse(n, unknown_code(d, graph, batch_note)));
            } else if rejected.contains(d.as_str()) {
                return Err(refuse(n, rejected_code(d)));
            }
        }
        merged.insert(placeholder(n), deps.clone());
    }
    for n in 0..batch.len() {
        let from = placeholder(n);
        if let Some(cycle) = find_cycle(&merged, &from) {
            return Err(refuse(n, loop_message(&from, &cycle)));
        }
    }
    Ok(())
}

pub fn find_cycle(graph: &Graph, start: &str) -> Option<Vec<String>> {
    let mut path: Vec<String> = Vec::new();
    let mut settled: HashSet<String> = HashSet::new();
    walk(graph, start, &mut path, &mut settled)
}

fn walk(
    graph: &Graph,
    at: &str,
    path: &mut Vec<String>,
    settled: &mut HashSet<String>,
) -> Option<Vec<String>> {
    if let Some(from) = path.iter().position(|p| p == at) {
        let mut cycle = path[from..].to_vec();
        cycle.push(at.to_string());
        return Some(cycle);
    }
    if settled.contains(at) {
        return None;
    }
    path.push(at.to_string());
    if let Some(deps) = graph.get(at) {
        for dep in deps {
            if let Some(cycle) = walk(graph, dep, path, settled) {
                return Some(cycle);
            }
        }
    }
    path.pop();
    settled.insert(at.to_string());
    None
}

pub fn loop_path(cycle: &[String]) -> String {
    cycle.join(" → ")
}

fn loop_message(code: &str, cycle: &[String]) -> String {
    let path = loop_path(cycle);
    match cycle.first().map(String::as_str) {
        Some(first) if first == code => format!(
            "{code} can't depend on {}: that closes a dependency loop — {path}. Nothing in a \
             loop is ever built, because each item waits on the next.",
            cycle.get(1).map(String::as_str).unwrap_or(code)
        ),
        _ => format!(
            "{code} would wait on a dependency loop — {path}. Nothing in a loop is ever built, \
             so that chain has to be broken first."
        ),
    }
}

fn check_codes(
    graph: &Graph,
    rejected: &BTreeSet<String>,
    code: Option<&str>,
    deps: &[String],
    batch_note: &str,
) -> Result<(), String> {
    for d in deps {
        if code == Some(d.as_str()) {
            return Err(format!("{d} cannot depend on itself"));
        }
        if !graph.contains_key(d.as_str()) {
            return Err(unknown_code(d, graph, batch_note));
        }
        if rejected.contains(d.as_str()) {
            return Err(rejected_code(d));
        }
    }
    Ok(())
}

fn rejected_code(dep: &str) -> String {
    format!(
        "{dep} was rejected — remove it or reopen it first; a rejected item never satisfies a \
         dependency, so anything waiting on it would sit wedged"
    )
}

fn unknown_code(dep: &str, graph: &Graph, batch_note: &str) -> String {
    if graph.is_empty() {
        return format!("`deps` names {dep:?}, and this board has no items to depend on yet");
    }
    let mut codes: Vec<&str> = graph.keys().map(String::as_str).collect();
    let extra = codes.len().saturating_sub(LISTED);
    codes.truncate(LISTED);
    let mut listed = codes.join(", ");
    if extra > 0 {
        listed.push_str(&format!(", and {extra} more"));
    }
    format!(
        "`deps` names {dep:?}, which is not an item on this board{batch_note} — the codes on it \
         are {listed}"
    )
}

#[cfg(test)]
#[path = "tests/deps.rs"]
mod tests;

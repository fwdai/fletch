//! Dependency-graph rules: the one place that answers "may this item depend on
//! these codes?".
//!
//! Why a module of its own. `deps` is written from five surfaces — the item
//! dialog's create and update commands, the PM's `roadmap_propose` batch, its
//! `roadmap_propose_update` patch, and the user's ruling that applies one — and
//! every one of them can close a loop. A loop is the worst failure this board
//! has: nothing in it is ever `done`, so [`super::drainer::unsatisfied_deps`]
//! never resolves for any of its members, the drainer skips the whole chain on
//! every tick forever, and the only trace is a transient note nobody was
//! watching. That was reachable through an accepted dep patch until this module
//! existed. So the rule lives here once, runs before every dep write, runs
//! *again* when a stored ask is applied (the board moves between the ask and the
//! click), and answers the drainer's "is this queue head wedged for good?".
//!
//! Pure by construction: a `code → deps` map in, a refusal naming the problem
//! out. No connection, no clock, no app handle — so the table below is the
//! whole specification of the rule.

use std::collections::{BTreeMap, HashSet};

use super::types::RoadmapItem;

/// Every code on a board mapped to the codes it must land after.
///
/// A `BTreeMap` rather than a `HashMap` so a refusal that lists the board's
/// codes lists them in a stable order — an error message that reshuffles
/// between two identical calls reads like two different problems.
pub type Graph = BTreeMap<String, Vec<String>>;

/// How a batch item names another item in the same batch: `"#2"` is the second
/// item in the array. Codes are allocated inside the insert transaction, so at
/// validation time a batch's own items are known only by position.
pub const BATCH_PREFIX: char = '#';

/// How many codes a refusal lists before it says "and n more". A message the
/// PM has to read (or the dialog has to fit) stops helping past a dozen.
const LISTED: usize = 12;

/// The dependency graph of a project's board, exactly as stored.
pub fn graph_of(items: &[RoadmapItem]) -> Graph {
    items
        .iter()
        .map(|i| (i.code.clone(), i.deps.clone()))
        .collect()
}

/// The node name a batch item takes while it has no code yet: `"#1"` for the
/// first item. Cannot collide with a real code (`PREFIX-nnn`), which is what
/// lets the batch's edges and the board's edges live in one graph.
fn placeholder(index: usize) -> String {
    format!("{BATCH_PREFIX}{}", index + 1)
}

/// The 0-based batch position a `"#n"` dep resolves to, for the caller that
/// rewrites it into a real code once the batch is inserted. `None` for a plain
/// code, and for anything [`validate_batch`] would have refused — so a caller
/// that validated first can treat `None` as "this is a code".
pub fn batch_index(dep: &str, batch_len: usize) -> Option<usize> {
    let n: usize = dep.strip_prefix(BATCH_PREFIX)?.trim().parse().ok()?;
    (1..=batch_len).contains(&n).then(|| n - 1)
}

/// Validate the deps of an item that does not exist yet — the create path.
///
/// Only the codes are checked: nothing can depend on a row that has no code, so
/// a brand-new item cannot close a loop no matter what it names.
pub fn validate_new(graph: &Graph, deps: &[String]) -> Result<(), String> {
    check_codes(graph, None, deps, "")
}

/// Validate a new dep list for an item that already exists — the dialog's edit,
/// the PM's patch, and the ruling that applies one.
///
/// Refuses an unknown code, a self-reference, and any loop the edit would leave
/// reachable from this item.
pub fn validate_edit(graph: &Graph, code: &str, deps: &[String]) -> Result<(), String> {
    check_codes(graph, Some(code), deps, "")?;
    // The graph as it would be *after* the write: the check has to be about the
    // board this edit produces, not the one it was computed from.
    let mut after = graph.clone();
    after.insert(code.to_string(), deps.to_vec());
    match find_cycle(&after, code) {
        Some(cycle) => Err(loop_message(code, &cycle)),
        None => Ok(()),
    }
}

/// Which item in a batch was refused, and why. The batch's own validator knows
/// the position; only its caller knows the title, so the two are handed back
/// separately rather than baked into one string here.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchRefusal {
    /// 0-based index into the batch.
    pub at: usize,
    pub message: String,
}

/// Validate a whole batch of new items as one graph merged with the board's.
///
/// `batch` is each proposed item's dep list, in batch order; an entry may name a
/// real code or another item in the same batch as `"#n"`. Refuses an unknown
/// code, an out-of-range or self `"#n"`, and any loop — including one made
/// entirely of the batch's own edges, which is the whole reason intra-batch
/// references need checking at all.
pub fn validate_batch(graph: &Graph, batch: &[Vec<String>]) -> Result<(), BatchRefusal> {
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
            }
        }
        merged.insert(placeholder(n), deps.clone());
    }
    // Every new node, not just the ones with batch references: an item whose dep
    // chain runs into a loop already on the board is just as unbuildable as one
    // that closes a fresh loop with its neighbour.
    for n in 0..batch.len() {
        let from = placeholder(n);
        if let Some(cycle) = find_cycle(&merged, &from) {
            return Err(refuse(n, loop_message(&from, &cycle)));
        }
    }
    Ok(())
}

/// The first dependency loop reachable from `start`: the codes in it, first and
/// last the same node, so rendering it reads as a closed walk
/// (`MCA-101 → MCA-104 → MCA-101`). `None` when every chain under `start` ends.
///
/// Reachability rather than "is `start` itself in a loop", on purpose: an item
/// whose dep chain *ends* in a loop can never be built either, and that is the
/// usual shape of a wedged queue head — the loop is two items further down.
/// A code with no node (an item that was deleted) is a leaf, matching
/// [`super::drainer::unsatisfied_deps`], where a stale code counts as satisfied.
pub fn find_cycle(graph: &Graph, start: &str) -> Option<Vec<String>> {
    let mut path: Vec<String> = Vec::new();
    let mut settled: HashSet<String> = HashSet::new();
    walk(graph, start, &mut path, &mut settled)
}

/// Depth-first walk carrying its own path, so hitting a node already on the
/// path *is* the loop and the path spells it out. `settled` holds the nodes
/// already proven loop-free, which is what keeps a diamond-shaped board from
/// being re-walked once per route into it.
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

/// A loop as one line: `MCA-101 → MCA-104 → MCA-101`. The rendering every
/// surface uses — the refusal, the card's note, and the durable `blocked`
/// event's detail — so the same loop reads the same everywhere.
pub fn loop_path(cycle: &[String]) -> String {
    cycle.join(" → ")
}

/// Why a loop is a refusal and not a warning, in the words of whichever item is
/// being written. Two shapes, because "you just closed this" and "you are
/// depending on something already broken" are different things to fix.
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

/// Check every dep resolves, and that the item isn't naming itself. `code` is
/// `None` for an item that has none yet (the create path).
fn check_codes(
    graph: &Graph,
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
    }
    Ok(())
}

/// A dep naming nothing on the board, with what it could have named instead —
/// the difference between an error the caller can fix and one it has to guess at.
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
mod tests {
    use super::*;

    /// A board, as `(code, deps)` pairs — the whole input this module reasons
    /// over, so the tests state the graph and nothing else.
    fn graph(edges: &[(&str, &[&str])]) -> Graph {
        edges
            .iter()
            .map(|(code, deps)| {
                (
                    (*code).to_string(),
                    deps.iter().map(|d| (*d).to_string()).collect(),
                )
            })
            .collect()
    }

    fn list(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| (*c).to_string()).collect()
    }

    #[test]
    fn an_item_with_no_deps_is_always_fine() {
        let g = graph(&[("MCA-100", &[]), ("MCA-101", &[])]);
        assert_eq!(validate_edit(&g, "MCA-100", &[]), Ok(()));
        assert_eq!(validate_new(&g, &[]), Ok(()));
        assert_eq!(find_cycle(&g, "MCA-100"), None);
    }

    #[test]
    fn a_chain_however_long_is_fine() {
        // 102 → 101 → 100. Adding 103 → 102 keeps it a chain.
        let g = graph(&[
            ("MCA-100", &[]),
            ("MCA-101", &["MCA-100"]),
            ("MCA-102", &["MCA-101"]),
            ("MCA-103", &[]),
        ]);
        assert_eq!(validate_edit(&g, "MCA-103", &list(&["MCA-102"])), Ok(()));
        // Two routes into the same node is not a loop either.
        assert_eq!(
            validate_edit(&g, "MCA-103", &list(&["MCA-102", "MCA-100"])),
            Ok(())
        );
        assert_eq!(find_cycle(&g, "MCA-102"), None);
    }

    #[test]
    fn a_direct_cycle_is_refused_and_spelled_out() {
        // 104 already depends on 101; making 101 depend on 104 closes the loop.
        let g = graph(&[("MCA-101", &[]), ("MCA-104", &["MCA-101"])]);
        let err = validate_edit(&g, "MCA-101", &list(&["MCA-104"])).unwrap_err();
        assert!(
            err.contains("MCA-101 → MCA-104 → MCA-101"),
            "the refusal must name the loop: {err}"
        );
        assert!(err.contains("loop"), "{err}");
    }

    #[test]
    fn a_transitive_cycle_is_refused_with_the_whole_path() {
        // 101 → 102 → 103, and now 103 → 101.
        let g = graph(&[
            ("MCA-101", &["MCA-102"]),
            ("MCA-102", &["MCA-103"]),
            ("MCA-103", &[]),
        ]);
        let err = validate_edit(&g, "MCA-103", &list(&["MCA-101"])).unwrap_err();
        assert!(
            err.contains("MCA-103 → MCA-101 → MCA-102 → MCA-103"),
            "{err}"
        );
    }

    #[test]
    fn depending_on_a_loop_you_are_not_in_is_refused_too() {
        // The loop is 101 ⇄ 102; 200 merely waits on it — and so waits forever.
        let g = graph(&[
            ("MCA-101", &["MCA-102"]),
            ("MCA-102", &["MCA-101"]),
            ("MCA-200", &[]),
        ]);
        let err = validate_edit(&g, "MCA-200", &list(&["MCA-101"])).unwrap_err();
        assert!(
            err.contains("MCA-200 would wait on a dependency loop"),
            "{err}"
        );
        assert!(err.contains("MCA-101 → MCA-102 → MCA-101"), "{err}");
    }

    #[test]
    fn an_item_cannot_depend_on_itself() {
        let g = graph(&[("MCA-100", &[])]);
        let err = validate_edit(&g, "MCA-100", &list(&["MCA-100"])).unwrap_err();
        assert!(err.contains("depend on itself"), "{err}");
    }

    #[test]
    fn an_unknown_code_is_refused_and_the_board_is_listed() {
        let g = graph(&[("MCA-100", &[]), ("MCA-101", &[])]);
        let err = validate_edit(&g, "MCA-100", &list(&["MCA-999"])).unwrap_err();
        assert!(err.contains("MCA-999"), "{err}");
        assert!(
            err.contains("MCA-100, MCA-101"),
            "the codes it could name: {err}"
        );
        // The create path applies the same rule without a code of its own.
        assert!(validate_new(&g, &list(&["MCA-999"])).is_err());
        // An empty board says so rather than offering an empty list.
        let err = validate_new(&Graph::new(), &list(&["MCA-999"])).unwrap_err();
        assert!(err.contains("no items to depend on yet"), "{err}");
    }

    #[test]
    fn a_long_board_lists_only_the_first_codes() {
        let codes: Vec<String> = (0..20).map(|n| format!("MCA-1{n:02}")).collect();
        let g: Graph = codes.iter().map(|c| (c.clone(), Vec::new())).collect();
        let err = validate_edit(&g, "MCA-100", &list(&["nope"])).unwrap_err();
        assert!(err.contains("and 8 more"), "{err}");
    }

    // ─────────────────────────── batches ────────────────────────────────

    #[test]
    fn a_batch_may_order_itself_with_hash_references() {
        let g = graph(&[("MCA-100", &[])]);
        // Item 2 lands after item 1; item 3 after item 2 and after the board's
        // own MCA-100. Forward references are fine too — item 1 on item 3.
        let batch = vec![
            list(&["#3"]),
            list(&["#1"]),
            list(&["MCA-100", "#4"]),
            Vec::new(),
        ];
        assert_eq!(validate_batch(&g, &batch), Ok(()));
        // And the positions resolve for the caller that rewrites them.
        assert_eq!(batch_index("#3", 4), Some(2));
        assert_eq!(batch_index("MCA-100", 4), None);
        assert_eq!(batch_index("#9", 4), None);
    }

    #[test]
    fn a_batch_internal_cycle_is_refused() {
        let batch = vec![list(&["#2"]), list(&["#3"]), list(&["#1"])];
        let refusal = validate_batch(&Graph::new(), &batch).unwrap_err();
        assert_eq!(refusal.at, 0);
        assert!(refusal.message.contains("#1 → #2 → #3 → #1"), "{refusal:?}");
    }

    #[test]
    fn a_batch_item_that_waits_on_a_board_loop_is_refused() {
        // The loop is entirely the board's (a pair from before this check
        // existed); the batch item merely hangs off it, which is still a ticket
        // that can never be built.
        let g = graph(&[("MCA-101", &["MCA-102"]), ("MCA-102", &["MCA-101"])]);
        let batch = vec![Vec::new(), list(&["MCA-101"])];
        let refusal = validate_batch(&g, &batch).unwrap_err();
        assert_eq!(refusal.at, 1);
        assert!(
            refusal.message.contains("MCA-101 → MCA-102 → MCA-101"),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_batch_reference_must_be_in_range_and_not_itself() {
        let batch = vec![Vec::new(), Vec::new()];
        for (deps, at, needle) in [
            (list(&["#3"]), 0, "not an item in this batch"),
            (list(&["#0"]), 0, "not an item in this batch"),
            (list(&["#two"]), 0, "not an item in this batch"),
            (list(&["#1"]), 0, "depend on itself"),
        ] {
            let mut batch = batch.clone();
            batch[at] = deps;
            let refusal = validate_batch(&Graph::new(), &batch).unwrap_err();
            assert_eq!(refusal.at, at);
            assert!(refusal.message.contains(needle), "{refusal:?}");
        }
    }

    #[test]
    fn a_batch_dep_on_an_unknown_code_names_the_batch_syntax_too() {
        let g = graph(&[("MCA-100", &[])]);
        let batch = vec![list(&["MCA-999"])];
        let refusal = validate_batch(&g, &batch).unwrap_err();
        assert_eq!(refusal.at, 0);
        assert!(refusal.message.contains("MCA-999"), "{refusal:?}");
        assert!(refusal.message.contains("#n"), "{refusal:?}");
    }

    #[test]
    fn a_deleted_dependency_is_a_leaf_not_a_loop() {
        // `unsatisfied_deps` treats a code with no row as satisfied; the walk
        // has to agree, or a board with a stale code would read as wedged.
        let g = graph(&[("MCA-100", &["MCA-099"])]);
        assert_eq!(find_cycle(&g, "MCA-100"), None);
    }

    #[test]
    fn the_graph_comes_straight_off_the_board() {
        let g = graph_of(&[]);
        assert!(g.is_empty());
    }
}

use std::collections::HashSet;

use crate::roadmap::types::{ItemStatus, NewItem, RoadmapItem};

/// Words that carry no signal about what a ticket is for. Deliberately tiny:
/// this list exists so "Add the queue drainer" and "Add a queue drainer" read
/// as the same idea, not to do linguistics.
const STOPWORDS: [&str; 9] = ["a", "an", "the", "of", "for", "to", "in", "on", "and"];

/// A title as a bag of meaningful words: lowercased, split on anything that
/// isn't alphanumeric, stopwords dropped.
fn title_words(title: &str) -> HashSet<String> {
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !STOPWORDS.contains(w))
        .map(str::to_string)
        .collect()
}

/// Do two titles look like the same idea? True when the smaller title's word
/// set is substantially contained in the other's: at least two shared words,
/// covering at least 60% of the smaller set.
///
/// Deliberately dumb and dependency-free — no stemming, no embeddings, no
/// dials. It only has to catch the PM re-typing an idea in slightly different
/// words; a miss costs nothing (the user still rules), and a false hit costs
/// one advisory line.
fn similar_titles(a: &str, b: &str) -> bool {
    let (a, b) = (title_words(a), title_words(b));
    let shared = a.intersection(&b).count();
    let smaller = a.len().min(b.len());
    // Integer form of `shared / smaller >= 0.6`, so no float ever decides.
    shared >= 2 && 10 * shared >= 6 * smaller
}

/// Advisory lines for a batch whose titles look like items already on the
/// board — every item, `rejected` included, because the killed idea is exactly
/// the one worth flagging. Never a refusal: the proposal lands either way, the
/// user's ruling is the real gate, and each line is worded for the PM to relay.
pub(super) fn duplicate_warnings(news: &[NewItem], board: &[RoadmapItem]) -> Vec<String> {
    let mut warnings = Vec::new();
    for (n, new) in news.iter().enumerate() {
        for item in board {
            if !similar_titles(&new.title, &item.title) {
                continue;
            }
            let at = n + 1;
            warnings.push(match item.status {
                ItemStatus::Rejected => format!(
                    "item {at} ({:?}) is similar to {}, which was REJECTED — {}",
                    new.title,
                    item.code,
                    item.close_reason.as_deref().unwrap_or("no reason recorded")
                ),
                _ => format!(
                    "item {at} ({:?}) is similar to {} — {:?}",
                    new.title, item.code, item.title
                ),
            });
        }
    }
    warnings
}

#[cfg(test)]
#[path = "tests/duplicates.rs"]
mod tests;

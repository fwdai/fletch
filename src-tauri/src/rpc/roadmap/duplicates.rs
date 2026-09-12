use std::collections::HashSet;

use crate::roadmap::types::{ItemStatus, NewItem, RoadmapItem};

const STOPWORDS: [&str; 9] = ["a", "an", "the", "of", "for", "to", "in", "on", "and"];

fn title_words(title: &str) -> HashSet<String> {
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !STOPWORDS.contains(w))
        .map(str::to_string)
        .collect()
}

/// At least two shared words covering 60% of the smaller set. Deliberately dumb:
/// a miss costs nothing, a false hit one advisory line.
fn similar_titles(a: &str, b: &str) -> bool {
    let (a, b) = (title_words(a), title_words(b));
    let shared = a.intersection(&b).count();
    let smaller = a.len().min(b.len());
    // Integer form of `shared / smaller >= 0.6`, so no float ever decides.
    shared >= 2 && 10 * shared >= 6 * smaller
}

/// `rejected` items included: the killed idea is exactly the one worth flagging.
/// Never a refusal.
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

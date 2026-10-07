//! The board's decision log: what the user ruled off the roadmap, newest
//! ruling first. `roadmap_list` returns it as rows; [`digest`] is the same
//! list as the markdown the PM reads at spawn.

use rusqlite::Connection;

use super::store;
use super::types::{ItemStatus, RoadmapItem};

pub const NOT_DOING_MAX: usize = 30;

/// Rejected items, newest ruling first, capped at [`NOT_DOING_MAX`]; the
/// second value is how many older ones the cap dropped.
pub fn not_doing(items: &[RoadmapItem]) -> (Vec<&RoadmapItem>, usize) {
    let mut rejected: Vec<&RoadmapItem> = items
        .iter()
        .filter(|i| i.status == ItemStatus::Rejected)
        .collect();
    rejected.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
    let dropped = rejected.len().saturating_sub(NOT_DOING_MAX);
    rejected.truncate(NOT_DOING_MAX);
    (rejected, dropped)
}

/// One line per rejected item with its reason; `None` when nothing was ruled
/// off the board.
pub fn digest(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<String>> {
    let items = store::list(conn, project_id)?;
    let (rejected, dropped) = not_doing(&items);
    if rejected.is_empty() {
        return Ok(None);
    }
    let mut lines: Vec<String> = rejected.iter().map(|i| not_doing_line(i)).collect();
    if dropped > 0 {
        lines.push(format!(
            "…and {dropped} older rejected item(s) not shown — the {NOT_DOING_MAX} newest are"
        ));
    }
    Ok(Some(lines.join("\n")))
}

fn not_doing_line(item: &RoadmapItem) -> String {
    match item.close_reason.as_deref() {
        Some(reason) => format!(
            "- {} — {} — {}",
            item.code,
            one_line(&item.title),
            one_line(reason)
        ),
        None => format!("- {} — {}", item.code, one_line(&item.title)),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[path = "tests/not_doing.rs"]
mod tests;

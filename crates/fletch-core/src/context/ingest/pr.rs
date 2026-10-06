//! The `## Decisions` section of a PR body, as the git-action playbooks tell
//! agents to write it:
//!
//! ```text
//! ## Decisions
//! - adopted · implementation · [auth-session, login] — Sessions refresh server-side. — Client refresh raced the token cache.
//! - constraint · business · [billing] — Invoices are never deleted, only voided. — Accounting requirement.
//! - none
//! ```
//!
//! Parsed tolerantly: the author is a model, and a line that is nearly right
//! (a `*` bullet, a plain dash for the em dash, no rationale) should land
//! rather than be lost. What cannot be read is warned about and dropped; a
//! line with no entity list is returned with an empty `about` so the caller
//! can report it against the PR.

use crate::context::model::{AssertionKind, Domain, Stance};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDecision {
    pub kind: AssertionKind,
    pub stance: Stance,
    pub domain: Domain,
    /// Entity references as written (slugs, usually); empty when the line
    /// carried no bracket list.
    pub about: Vec<String>,
    pub statement: String,
    /// Empty when the line stopped after the statement.
    pub rationale: String,
    /// The line as written, bullet stripped — the evidence a proposal quotes.
    pub line: String,
}

const HEADING: &str = "## decisions";

/// Every decision line of the first `## Decisions` section. The section runs
/// to the next `## ` heading or the end of the body; a body without one, or
/// one holding only `- none`, is empty.
pub fn parse_decisions(body: &str) -> Vec<ParsedDecision> {
    let mut lines = body.lines().map(str::trim);
    if !lines.any(|l| l.to_lowercase().trim_end_matches(':') == HEADING) {
        return Vec::new();
    }
    lines
        .take_while(|l| !l.starts_with("## "))
        .filter_map(parse_line)
        .collect()
}

fn parse_line(raw: &str) -> Option<ParsedDecision> {
    let line = raw
        .strip_prefix('-')
        .or_else(|| raw.strip_prefix('*'))
        .unwrap_or(raw)
        .trim();
    if line.is_empty() || line.trim_end_matches('.').eq_ignore_ascii_case("none") {
        return None;
    }
    let (head, rest) = split_head(line);
    let fields: Vec<&str> = head.split('·').map(str::trim).collect();
    let (kind, stance) = match fields.first().map(|f| f.to_lowercase()).as_deref() {
        Some("adopted") => (AssertionKind::Decision, Stance::Adopted),
        Some("rejected") => (AssertionKind::Decision, Stance::Rejected),
        Some("constraint") => (AssertionKind::Constraint, Stance::Adopted),
        Some("fact") => (AssertionKind::Fact, Stance::Adopted),
        _ => return skip(line, "no stance or kind"),
    };
    let domain = match fields.get(1).map(|f| f.to_lowercase()).as_deref() {
        Some("business") => Domain::Business,
        Some("architectural") => Domain::Architectural,
        Some("implementation") => Domain::Implementation,
        _ => return skip(line, "no domain"),
    };
    let about = fields
        .iter()
        .find_map(|f| f.strip_prefix('[').and_then(|f| f.strip_suffix(']')))
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let rest = strip_leading_separator(rest);
    let (statement, rationale) = match split_separator(rest) {
        Some((s, r)) => (s.trim(), r.trim()),
        None => (rest.trim(), ""),
    };
    if statement.is_empty() {
        return skip(line, "no statement");
    }
    Some(ParsedDecision {
        kind,
        stance,
        domain,
        about,
        statement: statement.to_string(),
        rationale: rationale.to_string(),
        line: line.to_string(),
    })
}

fn skip(line: &str, why: &str) -> Option<ParsedDecision> {
    tracing::warn!(line, why, "pr decisions: unreadable line skipped");
    None
}

/// The classification fields and the rest. The bracket list is the anchor
/// when there is one — a statement may contain a dash, a slug list may not
/// contain `]` — and the first separator otherwise.
fn split_head(line: &str) -> (&str, &str) {
    match (line.find('['), line.find(']')) {
        (Some(open), Some(close)) if open < close => line.split_at(close + 1),
        _ => split_separator(line).unwrap_or((line, "")),
    }
}

/// Split at the first ` — ` or ` - `; failing those, at a bare em dash.
fn split_separator(text: &str) -> Option<(&str, &str)> {
    [" — ", " - "]
        .iter()
        .filter_map(|sep| text.find(sep).map(|at| (at, sep.len())))
        .min_by_key(|(at, _)| *at)
        .or_else(|| text.find('—').map(|at| (at, '—'.len_utf8())))
        .map(|(at, len)| (&text[..at], &text[at + len..]))
}

/// What sits between the bracket list and the statement: whitespace and one
/// dash of either kind (or a colon).
fn strip_leading_separator(rest: &str) -> &str {
    let rest = rest.trim_start();
    rest.strip_prefix('—')
        .or_else(|| rest.strip_prefix('-'))
        .or_else(|| rest.strip_prefix(':'))
        .unwrap_or(rest)
        .trim_start()
}

#[cfg(test)]
#[path = "tests/parse.rs"]
mod tests;

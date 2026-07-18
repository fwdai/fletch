//! Shared `Closes #<n>` PR-body trailer helpers.
//!
//! An issue-originated workspace (a Home-inbox "Start work") gets its PR body
//! stamped with a `Closes #<n>` trailer so merging the PR closes the issue —
//! reliably, without relying on the agent to remember. Two open-PR paths need
//! the identical, strict semantics: the ad-hoc agent dispatcher
//! (`crate::rpc::git`) and the workflow finalize publish (`crate::workflow`).
//! The helpers live here so both re-use one implementation.

/// GitHub's closing keywords (case-insensitive). Only one of these directly
/// preceding the reference makes merging the PR close the issue — a bare
/// mention (`see #12`) links but never closes.
const CLOSING_KEYWORDS: [&str; 9] = [
    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
];

/// True when `body` already contains a CLOSING reference to issue `#number`
/// (`fixes #12`, `Closes: #12`, …) — bounded so `#12` doesn't match `#123` (a
/// trailing digit means a different issue). A mere mention does NOT count: it
/// wouldn't close the issue, so the trailer is still required. Used to keep
/// the `Closes` trailer idempotent when the agent already wrote one.
pub(crate) fn closes_issue(body: &str, number: u32) -> bool {
    let needle = format!("#{number}");
    let mut offset = 0;
    while let Some(pos) = body[offset..].find(&needle) {
        let at = offset + pos;
        let after = &body[at + needle.len()..];
        let bounded = after.chars().next().map_or(true, |c| !c.is_ascii_digit());
        if bounded && ends_with_closing_keyword(&body[..at]) {
            return true;
        }
        offset = at + needle.len();
    }
    false
}

/// Does the text leading up to a `#N` reference end with a closing keyword
/// plus a real separator (`fixes #12`, `Fixes: #12`)? Strict on both the
/// keyword (`prefixes #12` is not a match) and the separator (GitHub does not
/// document the glued `fixes#12` form) — a false negative merely appends a
/// redundant trailer, while a false positive would leave the issue open.
fn ends_with_closing_keyword(before: &str) -> bool {
    let trimmed = before.trim_end();
    if trimmed.len() == before.len() {
        return false;
    }
    let trimmed = trimmed.strip_suffix(':').unwrap_or(trimmed);
    let word_start = trimmed
        .rfind(|c: char| !c.is_ascii_alphabetic())
        .map_or(0, |i| i + 1);
    let word = &trimmed[word_start..];
    CLOSING_KEYWORDS
        .iter()
        .any(|k| word.eq_ignore_ascii_case(k))
}

/// Append a `Closes #<n>` trailer to a PR body for an issue-originated
/// workspace, unless the body already CLOSES the issue (a bare mention is not
/// enough). `None` leaves the body untouched (the normal, non-issue spawn). A
/// blank body becomes just the trailer.
pub(crate) fn with_closes_trailer(body: &str, close_issue: Option<u32>) -> String {
    let Some(number) = close_issue else {
        return body.to_string();
    };
    if closes_issue(body, number) {
        return body.to_string();
    }
    let trailer = format!("Closes #{number}");
    let trimmed = body.trim_end();
    if trimmed.is_empty() {
        trailer
    } else {
        format!("{trimmed}\n\n{trailer}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closes_trailer_appends_only_when_issue_set() {
        // No issue → body untouched.
        assert_eq!(
            with_closes_trailer("Fixes the thing", None),
            "Fixes the thing"
        );
        // Issue set → trailer appended after a blank line.
        assert_eq!(
            with_closes_trailer("Fixes the thing", Some(42)),
            "Fixes the thing\n\nCloses #42"
        );
        // Blank body → just the trailer.
        assert_eq!(with_closes_trailer("   ", Some(7)), "Closes #7");
    }

    #[test]
    fn closes_trailer_is_idempotent_and_number_bounded() {
        // Already CLOSED by the body → left as-is (no duplicate).
        assert_eq!(
            with_closes_trailer("Work.\n\nCloses #42", Some(42)),
            "Work.\n\nCloses #42"
        );
        // A superstring number must NOT count (#12 vs #123).
        assert!(!closes_issue("Closes #123", 12));
        assert!(closes_issue("Closes #123", 123));
        assert_eq!(
            with_closes_trailer("Refs #120 and #121", Some(12)),
            "Refs #120 and #121\n\nCloses #12"
        );
    }

    #[test]
    fn closes_trailer_requires_a_closing_keyword_not_a_mention() {
        // A bare mention links but doesn't close — the trailer must still land.
        assert!(!closes_issue("see #42, thanks", 42));
        assert_eq!(
            with_closes_trailer("Follow-up to the report in #42.", Some(42)),
            "Follow-up to the report in #42.\n\nCloses #42"
        );
        // GitHub's documented closing spellings count (case, optional colon).
        for body in [
            "fixes #42",
            "Fixes: #42",
            "RESOLVED #42",
            "This should fix #42 for good",
        ] {
            assert!(closes_issue(body, 42), "should close: {body}");
        }
        // A keyword-suffixed word is not a keyword, and a keyword glued to the
        // reference (no separator) is not a documented closing form — both must
        // still receive the trailer.
        assert!(!closes_issue("prefixes #42", 42));
        assert!(!closes_issue("sunfixed #42", 42));
        assert!(!closes_issue("close#42", 42));
        assert!(!closes_issue("Fixes:#42", 42));
    }
}

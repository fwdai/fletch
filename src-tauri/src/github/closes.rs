//! Shared closing-trailer helpers for PR bodies.
//!
//! An issue-originated workspace (a Home-inbox "Start work", a composer
//! issue pick) gets its PR body stamped with a closing trailer so merging
//! the PR closes the issue — reliably, without relying on the agent to
//! remember. The workspace's `issue_ref` names the issue in its source's
//! canonical form, and the trailer follows suit:
//!
//! - a bare number (`"123"`, GitHub) → `Closes #123`, GitHub's own closing
//!   keyword;
//! - a tracker key (`"ENG-123"`, Linear) → `Fixes ENG-123`, the magic word
//!   Linear's GitHub integration recognizes in PR descriptions (Jira's
//!   GitHub app reads the same shape).
//!
//! Two open-PR paths need the identical, strict semantics: the ad-hoc agent
//! dispatcher (`crate::rpc::git`) and the workflow finalize publish
//! (`crate::workflow`). The helpers live here so both re-use one
//! implementation.

/// GitHub's closing keywords (case-insensitive). Only one of these directly
/// preceding the reference makes merging the PR close the issue — a bare
/// mention (`see #12`) links but never closes. Linear's magic words are the
/// same set, so one keyword check serves both trailer forms.
const CLOSING_KEYWORDS: [&str; 9] = [
    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
];

/// An `issue_ref` parsed into its trailer form.
enum IssueRef {
    /// GitHub issue number → `Closes #<n>`.
    Number(u32),
    /// Tracker key like `ENG-123` → `Fixes <key>`.
    Key(String),
}

/// Parse a stored `issue_ref` into its trailer form: a bare number is a
/// GitHub issue; letters-dash-digits (`ENG-123`) is a tracker key. Anything
/// else is unrecognized and gets no trailer — a malformed ref must degrade
/// to a plain PR body, never a broken trailer.
fn parse_issue_ref(issue_ref: &str) -> Option<IssueRef> {
    let r = issue_ref.trim();
    if let Ok(n) = r.parse::<u32>() {
        return Some(IssueRef::Number(n));
    }
    let (prefix, digits) = r.rsplit_once('-')?;
    let prefix_ok = !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_alphanumeric());
    let digits_ok = !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
    (prefix_ok && digits_ok).then(|| IssueRef::Key(r.to_string()))
}

/// True when `body` already contains a CLOSING reference to issue `#number`
/// (`fixes #12`, `Closes: #12`, …) — bounded so `#12` doesn't match `#123` (a
/// trailing digit means a different issue). A mere mention does NOT count: it
/// wouldn't close the issue, so the trailer is still required. Used to keep
/// the `Closes` trailer idempotent when the agent already wrote one.
pub(crate) fn closes_issue(body: &str, number: u32) -> bool {
    closes_needle(body, &format!("#{number}"))
}

/// True when `body` already contains a CLOSING reference to a tracker key
/// (`Fixes ENG-123`) — the key match is case-insensitive (Linear accepts
/// `eng-123`) and digit-bounded like [`closes_issue`].
fn closes_key(body: &str, key: &str) -> bool {
    // Case-insensitive scan via lowercase copies: keys and keywords are
    // ASCII, so byte offsets line up between the copies and the originals.
    closes_needle(&body.to_ascii_lowercase(), &key.to_ascii_lowercase())
}

/// Shared scan: does a closing keyword directly precede an occurrence of
/// `needle` (digit-bounded on the right, so `#12`/`ENG-12` never match
/// `#123`/`ENG-123`)?
fn closes_needle(body: &str, needle: &str) -> bool {
    let mut offset = 0;
    while let Some(pos) = body[offset..].find(needle) {
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

/// Does the text leading up to a reference end with a closing keyword
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

/// Append the closing trailer for an issue-originated workspace, unless the
/// body already CLOSES the issue (a bare mention is not enough). `None` — or
/// an unrecognized ref shape — leaves the body untouched (the normal,
/// non-issue spawn). A blank body becomes just the trailer.
pub(crate) fn with_closes_trailer(body: &str, issue_ref: Option<&str>) -> String {
    let Some(parsed) = issue_ref.and_then(parse_issue_ref) else {
        return body.to_string();
    };
    let trailer = match parsed {
        IssueRef::Number(n) => {
            if closes_issue(body, n) {
                return body.to_string();
            }
            format!("Closes #{n}")
        }
        IssueRef::Key(key) => {
            if closes_key(body, &key) {
                return body.to_string();
            }
            format!("Fixes {key}")
        }
    };
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
            with_closes_trailer("Fixes the thing", Some("42")),
            "Fixes the thing\n\nCloses #42"
        );
        // Blank body → just the trailer.
        assert_eq!(with_closes_trailer("   ", Some("7")), "Closes #7");
        // An unrecognized ref shape must degrade to no trailer.
        assert_eq!(with_closes_trailer("Work.", Some("not a ref")), "Work.");
        assert_eq!(with_closes_trailer("Work.", Some("")), "Work.");
    }

    #[test]
    fn closes_trailer_is_idempotent_and_number_bounded() {
        // Already CLOSED by the body → left as-is (no duplicate).
        assert_eq!(
            with_closes_trailer("Work.\n\nCloses #42", Some("42")),
            "Work.\n\nCloses #42"
        );
        // A superstring number must NOT count (#12 vs #123).
        assert!(!closes_issue("Closes #123", 12));
        assert!(closes_issue("Closes #123", 123));
        assert_eq!(
            with_closes_trailer("Refs #120 and #121", Some("12")),
            "Refs #120 and #121\n\nCloses #12"
        );
    }

    #[test]
    fn closes_trailer_requires_a_closing_keyword_not_a_mention() {
        // A bare mention links but doesn't close — the trailer must still land.
        assert!(!closes_issue("see #42, thanks", 42));
        assert_eq!(
            with_closes_trailer("Follow-up to the report in #42.", Some("42")),
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

    #[test]
    fn tracker_key_gets_fixes_trailer() {
        // A Linear-style key → `Fixes <key>` (Linear's magic word).
        assert_eq!(
            with_closes_trailer("Work.", Some("ENG-123")),
            "Work.\n\nFixes ENG-123"
        );
        assert_eq!(with_closes_trailer("", Some("ENG-7")), "Fixes ENG-7");
        // Idempotent, case-insensitively (Linear accepts `eng-123`), and only
        // for a CLOSING reference — a mention still gets the trailer.
        assert_eq!(
            with_closes_trailer("Done.\n\ncloses eng-123", Some("ENG-123")),
            "Done.\n\ncloses eng-123"
        );
        assert_eq!(
            with_closes_trailer("Follow-up to ENG-123.", Some("ENG-123")),
            "Follow-up to ENG-123.\n\nFixes ENG-123"
        );
        // Digit-bounded: a closing reference to ENG-1234 is a different issue.
        assert_eq!(
            with_closes_trailer("Fixes ENG-1234", Some("ENG-123")),
            "Fixes ENG-1234\n\nFixes ENG-123"
        );
        // Malformed keys degrade to no trailer.
        for bad in ["ENG-", "-123", "ENG-12a", "E N G-12"] {
            assert_eq!(with_closes_trailer("Work.", Some(bad)), "Work.");
        }
    }
}

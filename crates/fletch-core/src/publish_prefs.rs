//! User preferences for how agents' work is published: a prefix for every
//! branch an agent creates, and whether pull requests open as drafts.
//!
//! Both are settings-table keys mirrored in memory — the same idiom as
//! `rpc::approval::ENABLED` — because the readers (the git RPC dispatcher and
//! the PR-creation path) run where there is no DB handle. Seeded at startup,
//! updated by their set-commands.

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

/// Settings key: the prefix prepended to agent-created branch names. Absent or
/// empty means none.
pub const BRANCH_PREFIX_SETTING: &str = "git_branch_prefix";

/// Settings key: open pull requests as drafts. Opt-in — only `"true"` enables.
pub const DRAFT_PRS_SETTING: &str = "github_draft_prs";

static BRANCH_PREFIX: Mutex<String> = Mutex::new(String::new());
static DRAFT_PRS: AtomicBool = AtomicBool::new(false);

pub fn set_branch_prefix(prefix: &str) {
    *BRANCH_PREFIX.lock() = prefix.to_string();
}

pub fn branch_prefix() -> String {
    BRANCH_PREFIX.lock().clone()
}

pub fn parse_draft_prs(raw: Option<&str>) -> bool {
    raw == Some("true")
}

pub fn set_draft_prs(enabled: bool) {
    DRAFT_PRS.store(enabled, Ordering::Relaxed);
}

pub fn draft_prs() -> bool {
    DRAFT_PRS.load(Ordering::Relaxed)
}

/// Normalise a prefix the user typed and refuse one that would make every
/// branch name invalid. Trimmed; empty means none.
///
/// Checked as git will see it: the prefix in front of a name, against the
/// rules of `git check-ref-format --branch`. A prefix is only ever the front
/// of a name, so what matters is that *some* name can follow it — hence the
/// probe against a one-character continuation rather than the prefix alone
/// (`feature-` is fine; `alex.lock/` and `.hidden/` are not).
pub fn validate_branch_prefix(raw: &str) -> Result<String, String> {
    let prefix = raw.trim();
    if prefix.is_empty() {
        return Ok(String::new());
    }
    if is_valid_branch_name(&format!("{prefix}x")) {
        Ok(prefix.to_string())
    } else {
        Err("Not a valid Git branch prefix: no spaces, .., .lock, or a leading dot or dash.".into())
    }
}

/// `git check-ref-format --branch`, in Rust, so a prefix is refused when it is
/// typed rather than when the first push fails. Every rule git applies to a
/// branch name: per slash-separated component, none empty (so no leading,
/// trailing or doubled slashes), none starting with `.` or ending in `.lock`;
/// overall, no `..`, `@{`, control characters, space or `~ ^ : ? * [ \`, not a
/// lone `@`, no trailing `.`, and — the `--branch` extra — no leading `-`.
fn is_valid_branch_name(name: &str) -> bool {
    if name.is_empty() || name == "@" || name.starts_with('-') || name.ends_with('.') {
        return false;
    }
    if name.contains("..") || name.contains("@{") {
        return false;
    }
    if name
        .chars()
        .any(|c| c.is_control() || c == ' ' || "~^:?*[\\".contains(c))
    {
        return false;
    }
    name.split('/')
        .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// The name a branch is actually born under: the configured prefix in front
/// of what the agent (or the title fallback) chose.
pub fn apply_branch_prefix(desired: &str) -> String {
    with_prefix(&branch_prefix(), desired)
}

/// A name that already carries the prefix is left alone, so an agent that
/// echoed the prefix into its own choice doesn't get it twice.
fn with_prefix(prefix: &str, desired: &str) -> String {
    if prefix.is_empty() || desired.starts_with(prefix) {
        desired.to_string()
    } else {
        format!("{prefix}{desired}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_is_prepended_once() {
        assert_eq!(with_prefix("alex/", "fix/login"), "alex/fix/login");
        assert_eq!(with_prefix("alex/", "alex/fix/login"), "alex/fix/login");
        assert_eq!(with_prefix("", "fix/login"), "fix/login");
        assert_eq!(with_prefix("feature-", "login"), "feature-login");
    }

    #[test]
    fn validation_trims_and_accepts_what_git_accepts() {
        assert_eq!(validate_branch_prefix("  alex/ ").unwrap(), "alex/");
        assert_eq!(validate_branch_prefix("   ").unwrap(), "");
        for ok in ["alex/", "feature-", "team/alex/", "v1.2/", "a.lock", "wip_"] {
            assert!(
                validate_branch_prefix(ok).is_ok(),
                "{ok:?} should be accepted"
            );
        }
    }

    /// Each of these makes `git check-ref-format --branch "<prefix>x"` fail.
    #[test]
    fn validation_rejects_what_git_rejects() {
        for bad in [
            "-x/",
            "/x/",
            "a b/",
            "a..b/",
            "a//b",
            "a~b",
            "a@{b",
            "a:b",
            "a?b",
            "a*b",
            "a[b",
            "a\\b",
            "a^b",
            ".hidden/",
            "alex.lock/",
            "a/.b/",
            "a\tb",
        ] {
            assert!(
                validate_branch_prefix(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn branch_name_rules_match_git() {
        assert!(is_valid_branch_name("fix/login"));
        assert!(is_valid_branch_name("alex/fix/login-2"));
        assert!(!is_valid_branch_name("@"));
        assert!(!is_valid_branch_name("fix/login."));
        assert!(!is_valid_branch_name("fix/"));
        assert!(!is_valid_branch_name("fix.lock/x"));
    }
}

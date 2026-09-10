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

/// Normalise a prefix the user typed and refuse anything git would reject as
/// a ref component (or reinterpret as an option). Trimmed; empty means none.
pub fn validate_branch_prefix(raw: &str) -> Result<String, String> {
    let prefix = raw.trim();
    if prefix.is_empty() {
        return Ok(String::new());
    }
    if prefix.starts_with('-') || prefix.starts_with('/') {
        return Err("A branch prefix can't start with - or /.".into());
    }
    if prefix.contains("..") || prefix.contains("//") || prefix.contains("@{") {
        return Err("A branch prefix can't contain .., // or @{.".into());
    }
    if prefix
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
    {
        return Err("A branch prefix can't contain spaces or ~ ^ : ? * [ \\.".into());
    }
    Ok(prefix.to_string())
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
    fn validation_trims_and_rejects_unsafe_prefixes() {
        assert_eq!(validate_branch_prefix("  alex/ ").unwrap(), "alex/");
        assert_eq!(validate_branch_prefix("   ").unwrap(), "");
        for bad in ["-x/", "/x/", "a b/", "a..b/", "a//b", "a~b", "a@{b", "a:b"] {
            assert!(
                validate_branch_prefix(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }
}

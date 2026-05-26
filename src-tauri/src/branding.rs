//! Single source of truth for the app's branded identifiers.
//!
//! Anything that bakes the app name into user-facing or persisted
//! strings (git branch prefix today, possibly more later) goes through
//! this module. Renaming the app is then "change APP_NAME, recompile" —
//! no callers to track down across the codebase.

/// Display / branding name. Used as the git branch namespace and
/// available to anything else that needs to identify the app.
pub const APP_NAME: &str = "quorum";

/// Build the full agent branch name from a slug. Always namespaced
/// under APP_NAME so the branches are easy to spot and filter:
/// `git branch | grep ^quorum/`.
pub fn branch_for(slug: &str) -> String {
    format!("{APP_NAME}/{slug}")
}

/// Turn an arbitrary task description into an ASCII slug suitable for
/// a git branch name. Returns an empty string for inputs with no
/// representable characters (callers should fall back to the agent's
/// place id in that case).
///
/// Rules: lowercase, ASCII alphanumerics kept, every other run of
/// chars becomes a single `-`, leading/trailing `-` trimmed. Capped at
/// 32 chars, truncating at the last word boundary so we don't leave a
/// half-word at the end.
pub fn slugify_task(task: &str) -> String {
    const MAX_LEN: usize = 32;
    let mut out = String::new();
    let mut prev_sep = true;
    for ch in task.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_sep = false;
        } else if !prev_sep {
            out.push('-');
            prev_sep = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() <= MAX_LEN {
        return out;
    }
    let cut = out[..MAX_LEN].rfind('-').unwrap_or(MAX_LEN);
    out.truncate(cut);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_uses_app_name() {
        assert_eq!(branch_for("foo"), "quorum/foo");
    }

    #[test]
    fn slug_basic() {
        assert_eq!(slugify_task("Fix the bug"), "fix-the-bug");
    }

    #[test]
    fn slug_caps_at_word_boundary() {
        let s = slugify_task("Refactor the auth flow to use middleware");
        assert_eq!(s, "refactor-the-auth-flow-to-use");
        assert!(s.len() <= 32);
    }

    #[test]
    fn slug_strips_punctuation_and_unicode() {
        assert_eq!(slugify_task("Add @user/foo as a dep!"), "add-user-foo-as-a-dep");
        assert_eq!(slugify_task("修复 login bug"), "login-bug");
    }

    #[test]
    fn slug_empty_for_pure_non_ascii() {
        assert_eq!(slugify_task("修复登录问题"), "");
        assert_eq!(slugify_task("🐛🔥"), "");
        assert_eq!(slugify_task(""), "");
        assert_eq!(slugify_task("   "), "");
    }

    #[test]
    fn slug_long_single_word_truncates_hard() {
        let s = slugify_task("supercalifragilisticexpialidociousness");
        assert_eq!(s.len(), 32);
    }
}

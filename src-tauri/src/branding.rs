//! Single source of truth for the app's branded identifiers.
//!
//! Anything that bakes the app name into user-facing or persisted
//! strings (git branch prefix today, possibly more later) goes through
//! this module. Renaming the app is then "change APP_NAME, recompile" —
//! no callers to track down across the codebase.

/// Display / branding name. Used as the git branch namespace and
/// available to anything else that needs to identify the app.
pub const APP_NAME: &str = "quorum";

/// Build the full agent branch name from a name component (the agent's
/// place id, e.g. `everest`). Always namespaced under APP_NAME so the
/// branches are easy to spot and filter: `git branch | grep ^quorum/`.
/// The branch deliberately mirrors the workspace name so the worktree,
/// sandbox, and branch all share one identifier.
pub fn branch_for(name: &str) -> String {
    format!("{APP_NAME}/{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_uses_app_name() {
        assert_eq!(branch_for("everest"), "quorum/everest");
    }
}

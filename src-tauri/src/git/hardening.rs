//! Keeps a repository's own config from steering Fletch's host-side git.
//!
//! A git repo's config names programs git executes — hooks, clean/smudge
//! filters, textconv, merge drivers, fsmonitor. In an agent's checkout that
//! config is agent-written, so any host-side git invocation there is a
//! code-execution primitive: Fletch runs git on agent checkouts constantly (diff
//! polling, workflow boundary commits, merge integration).
//!
//! Applied once at [`crate::git_dist`]'s single spawn point, never per call site.
//! That is the point — the guard used to be an opt-in `no_hooks_env` each caller
//! had to remember, and the workflow git path shipped without it. A new call site
//! now inherits it by construction.
//!
//! **Defence in depth, not the boundary.** Only *fixed-name* keys can be
//! overridden here. `filter.<name>.clean`, `diff.<name>.textconv` and
//! `merge.<name>.driver` take their name from the (tracked, agent-editable)
//! `.gitattributes`, so they cannot be enumerated — a planted `filter.*.clean`
//! fires on `git add` regardless of this module. Those are closed by withholding
//! write access to the config itself: [`crate::sandbox::policy::GIT_EXEC_CONFIG_FILES`].

/// Fixed-name git config keys naming a program git would execute, each with the
/// value that neutralises it.
///
/// Applied as `-c` rather than `GIT_CONFIG_*` for two reasons: `-c` outranks
/// both repo config and any `GIT_CONFIG_*` a caller passes, and it cannot
/// collide with the `GIT_CONFIG_COUNT` protocol — where a second set silently
/// shadows the first, which is exactly how this guard would go missing.
///
/// Applied to **every** invocation, the user's own repositories included, which
/// is why membership is limited to keys Fletch's automated git never legitimately
/// needs. Deliberately absent: `credential.helper`, `core.sshCommand` and
/// `gpg.program`. Each carries a user's real remote auth or signing setup, so
/// clearing it would break `fetch` against a non-GitHub remote or an initial
/// commit under `commit.gpgsign` — and each needs a config *write* to exploit,
/// which policy invariant 3 denies. The boundary belongs there, not here.
///
/// Verified against git 2.50: without its row, each of `core.hooksPath`,
/// `core.fsmonitor` and `diff.external` executes agent-authored code host-side.
const NEUTRALISED: &[(&str, &str)] = &[
    ("core.hooksPath", "/dev/null"), // not a directory, so no hook resolves
    ("core.fsmonitor", "false"),     // fires on nearly every index refresh
    ("diff.external", ""),           // replaces the diff engine — hit by diff polling
    ("core.pager", "cat"),           // Fletch captures output; a pager only adds a program
    ("core.editor", "false"),        // an interactive editor would hang an automated op
    ("sequence.editor", "false"),    // `rebase -i`'s todo-list editor, same hazard
];

/// The `-c key=value` arguments that make a host-side git invocation safe to run
/// against a repository Fletch does not control the config of.
pub(crate) fn config_overrides() -> Vec<String> {
    NEUTRALISED
        .iter()
        .flat_map(|(key, value)| ["-c".to_string(), format!("{key}={value}")])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value_of(overrides: &[String], key: &str) -> Option<String> {
        let prefix = format!("{key}=");
        overrides
            .iter()
            .find_map(|a| a.strip_prefix(&prefix))
            .map(str::to_string)
    }

    /// Every row must reach git as a well-formed `-c key=value` pair, and the
    /// three empirically-confirmed executable keys must be present.
    #[test]
    fn overrides_are_well_formed_c_flags() {
        let overrides = config_overrides();
        assert_eq!(overrides.len(), NEUTRALISED.len() * 2);
        for pair in overrides.chunks(2) {
            assert_eq!(pair[0], "-c");
            assert!(pair[1].contains('='), "{} is not key=value", pair[1]);
        }
        assert_eq!(
            value_of(&overrides, "core.hooksPath").as_deref(),
            Some("/dev/null")
        );
        assert_eq!(
            value_of(&overrides, "core.fsmonitor").as_deref(),
            Some("false")
        );
        assert_eq!(value_of(&overrides, "diff.external").as_deref(), Some(""));
    }

    /// The keys that carry a *user's* real remote auth and signing config must
    /// stay out: clearing them would break `fetch` against a non-GitHub remote
    /// or a signed commit, and policy invariant 3 already denies the config
    /// write needed to abuse them.
    #[test]
    fn user_remote_and_signing_config_is_never_overridden() {
        for key in ["credential.helper", "core.sshCommand", "gpg.program"] {
            assert!(
                !NEUTRALISED.iter().any(|(k, _)| *k == key),
                "{key} must not be neutralised — see the const's doc"
            );
        }
    }

    /// The guard end to end, against a real repo carrying a real payload: the
    /// hook and fsmonitor a prompt-injected agent would plant must not run.
    #[test]
    fn overrides_stop_a_planted_hook_and_fsmonitor() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        let fired = td.path().join("fired");
        std::fs::create_dir_all(&repo).unwrap();

        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .expect("git")
        };
        // Every invocation past the plant carries the overrides — an unhardened
        // `add` would trip the fsmonitor itself and mask the result.
        let overrides = config_overrides();
        let hardened = |args: &[&str]| {
            let mut all: Vec<&str> = overrides.iter().map(String::as_str).collect();
            all.extend(args);
            git(&all)
        };

        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);

        // One payload serving as both a post-commit hook and an fsmonitor hook
        // (which must answer "everything is dirty" to be invoked as one).
        let hook = repo.join(".git/hooks/post-commit");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\ntouch {}\nprintf /\n", fired.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        git(&["config", "core.fsmonitor", hook.to_str().unwrap()]);

        std::fs::write(repo.join("f.txt"), "a").unwrap();
        hardened(&["add", "-A"]);
        hardened(&["commit", "-m", "one"]);
        hardened(&["status", "--porcelain"]);

        assert!(
            !fired.exists(),
            "a planted hook/fsmonitor executed despite the overrides"
        );
    }
}

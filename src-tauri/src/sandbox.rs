//! Per-agent macOS sandbox profile — the single, unified isolation layer for
//! every agent Quorum runs.
//!
//! The app launches each agent (Claude *and* the per-turn agents — codex,
//! cursor, opencode, pi, antigravity) under `sandbox-exec` with this profile,
//! rather than relying on each CLI's own sandbox. `sandbox-exec` is just the
//! process wrapper around the PTY/exec child, so terminal streaming and startup
//! timing are unchanged while *writes* are constrained to the agent's parent dir
//! (under `~/.quorum/worktrees/<id>/`) plus standard state/cache locations and
//! each agent's own on-disk session store. The agent's per-repo worktrees live
//! as subdirs of that parent, so each inherits the writable allowance.
//!
//! Because confinement is by *write* path (reads and network stay open via
//! `allow default`), each agent that the wrapper covers must have its
//! out-of-worktree write locations (session transcripts, config, auth refresh)
//! on the allow-list below — otherwise it can't persist its own state. That
//! covers the agents' own dot-dir stores plus the standard per-user
//! cache/state dirs in both XDG (`~/.cache`, `~/.config`, `~/.local`) and
//! macOS-native (`~/Library/Caches`, `~/Library/Application Support`) form,
//! since the agents' subprocess toolchains and macOS frameworks write to the
//! latter. The agent CLIs' own sandboxes are disabled (e.g. codex runs
//! `danger-full-access`) so the two don't fight, leaving `sandbox-exec` as the
//! sole boundary.

use std::path::Path;

use crate::error::{Error, Result};

/// Build the SBPL profile. `writable_root` is the agent's parent dir;
/// `rpc_dir` is its private file-mailbox (`~/.quorum/rpc/<id>/`), which lives
/// outside the worktree tree and so needs its own allow entry.
pub fn build_profile(writable_root: &Path, rpc_dir: &Path, home: &Path) -> Result<String> {
    let writable_root = canonical(writable_root)?;
    let rpc_root = canonical(rpc_dir)?;
    let home = canonical(home)?;

    let writable_root_s = sbpl_string(&writable_root.to_string_lossy());
    let rpc_root_s = sbpl_string(&rpc_root.to_string_lossy());
    let home_s = home.to_string_lossy();

    let claude_state = sbpl_string(&format!("{home_s}/.claude"));
    let claude_json = sbpl_string(&format!("{home_s}/.claude.json"));
    let npm_state = sbpl_string(&format!("{home_s}/.npm"));
    let cache_state = sbpl_string(&format!("{home_s}/.cache"));
    let config_state = sbpl_string(&format!("{home_s}/.config"));
    let local_state = sbpl_string(&format!("{home_s}/.local"));
    // macOS-native equivalents of the XDG cache/state dirs above. Native
    // toolchains the agents invoke (node/npm tooling, git, language SDKs) and
    // macOS framework caches (CFNetwork, fonts, per-bundle state) write here; a
    // denied write ranges from a harmless cache miss to a fatal auth-token
    // write, so allow them on the same "per-user app state, not source/system"
    // basis as `~/.cache`/`~/.config`.
    let library_caches = sbpl_string(&format!("{home_s}/Library/Caches"));
    let library_app_support = sbpl_string(&format!("{home_s}/Library/Application Support"));
    // Per-agent on-disk session stores (transcripts, config, auth) for the
    // per-turn agents now covered by this profile. OpenCode's store lives under
    // `~/.local/share/opencode`, already covered by `local_state`.
    let codex_state = sbpl_string(&format!("{home_s}/.codex"));
    let cursor_state = sbpl_string(&format!("{home_s}/.cursor"));
    let gemini_state = sbpl_string(&format!("{home_s}/.gemini"));
    let pi_state = sbpl_string(&format!("{home_s}/.pi"));

    Ok(format!(
        r#"(version 1)
(allow default)

;; Block writes everywhere by default, then re-allow specific subpaths.
(deny file-write*)
(allow file-write*
  (subpath {writable_root_s})
  (subpath {rpc_root_s})
  (subpath "/private/tmp")
  (subpath "/private/var/folders")
  (subpath "/private/var/tmp")
  (subpath {claude_state})
  (literal {claude_json})
  (subpath {npm_state})
  (subpath {cache_state})
  (subpath {config_state})
  (subpath {local_state})
  (subpath {library_caches})
  (subpath {library_app_support})
  (subpath {codex_state})
  (subpath {cursor_state})
  (subpath {gemini_state})
  (subpath {pi_state}))

;; PTYs and basic device files are required for terminal programs.
(allow file-write* (literal "/dev/null") (literal "/dev/zero"))
(allow file-write*
  (regex #"^/dev/tty[^/]*$")
  (regex #"^/dev/ptmx$")
  (regex #"^/dev/pts/[0-9]+$"))
"#
    ))
}

fn sbpl_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn canonical(p: &Path) -> Result<std::path::PathBuf> {
    std::fs::canonicalize(p)
        .map_err(|e| Error::Other(format!("canonicalize {}: {e}", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_includes_writable_root_and_denies_writes_by_default() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("agent-parent");
        let rpc = td.path().join("rpc");
        let home = td.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&rpc).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let profile = build_profile(&root, &rpc, &home).unwrap();
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        let canonical_rpc = std::fs::canonicalize(&rpc).unwrap();

        assert!(profile.contains("(deny file-write*)"));
        assert!(profile.contains(&format!("\"{}\"", canonical_root.display())));
        // The mailbox lives outside the worktree tree, so it needs its own entry.
        assert!(profile.contains(&format!("\"{}\"", canonical_rpc.display())));
        // macOS-native per-user state dirs, needed by the agents' toolchains.
        assert!(profile.contains("/Library/Caches"));
        assert!(profile.contains("/Library/Application Support"));
    }

    #[test]
    fn escapes_quotes_in_paths() {
        assert_eq!(sbpl_string(r#"/path/with"quote"#), r#""/path/with\"quote""#);
    }
}

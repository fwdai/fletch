//! Per-agent macOS sandbox profile for yolo-mode Claude.
//!
//! The app still launches Claude in a normal PTY. `sandbox-exec` is only
//! the process wrapper around that PTY child, so terminal streaming and
//! startup timing stay the same while writes are constrained to the agent
//! worktree plus standard state/cache locations.

use std::path::Path;

use crate::error::{Error, Result};

pub fn build_profile(worktree: &Path, home: &Path) -> Result<String> {
    let worktree = canonical(worktree)?;
    let home = canonical(home)?;

    let worktree_s = sbpl_string(&worktree.to_string_lossy());
    let home_s = home.to_string_lossy();

    let claude_state = sbpl_string(&format!("{home_s}/.claude"));
    let claude_json = sbpl_string(&format!("{home_s}/.claude.json"));
    let npm_state = sbpl_string(&format!("{home_s}/.npm"));
    let cache_state = sbpl_string(&format!("{home_s}/.cache"));
    let config_state = sbpl_string(&format!("{home_s}/.config"));
    let local_state = sbpl_string(&format!("{home_s}/.local"));

    Ok(format!(
        r#"(version 1)
(allow default)

;; Block writes everywhere by default, then re-allow specific subpaths.
(deny file-write*)
(allow file-write*
  (subpath {worktree_s})
  (subpath "/private/tmp")
  (subpath "/private/var/folders")
  (subpath "/private/var/tmp")
  (subpath {claude_state})
  (literal {claude_json})
  (subpath {npm_state})
  (subpath {cache_state})
  (subpath {config_state})
  (subpath {local_state}))

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
    fn profile_includes_worktree_path_and_denies_writes_by_default() {
        let td = tempfile::tempdir().unwrap();
        let worktree = td.path().join("worktree");
        let home = td.path().join("home");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let profile = build_profile(&worktree, &home).unwrap();
        let canonical_worktree = std::fs::canonicalize(&worktree).unwrap();

        assert!(profile.contains("(deny file-write*)"));
        assert!(profile.contains(&format!("\"{}\"", canonical_worktree.display())));
    }

    #[test]
    fn escapes_quotes_in_paths() {
        assert_eq!(sbpl_string(r#"/path/with"quote"#), r#""/path/with\"quote""#);
    }
}

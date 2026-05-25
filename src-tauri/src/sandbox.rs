//! Per-agent macOS sandbox profile + `sandbox-exec` invocation.
//!
//! Each agent runs claude (and any subprocesses it spawns) inside a
//! kernel-enforced sandbox that's tight on writes but otherwise
//! permissive — the threat model is "don't let yolo-mode claude
//! destroy the user's machine", not "isolate it like a VM".
//!
//! Strategy:
//!   - `allow default` — start permissive so node, git, bash, etc. all
//!     still work without us enumerating every capability.
//!   - `deny file-write*` — then revoke writes globally,
//!   - `allow file-write*` for a narrow set of subpaths the agent
//!     legitimately needs (its worktree, temp dirs, claude state).
//!
//! Net effect: claude can read your machine, can make network calls,
//! can spawn subprocesses — but can ONLY write inside its assigned
//! worktree (plus temp + its own state dir). `rm -rf /`, edits to
//! `~/.zshrc`, writes to other agents' worktrees: all blocked at the
//! kernel.

use std::path::Path;

use crate::error::{Error, Result};

/// Build the SBPL profile for one agent.
///
/// `worktree` is the only path inside the user's repo that the agent can
/// modify. `home` is the user's home directory (we narrowly re-allow
/// writes to `~/.claude` so claude code can persist its session state,
/// `~/.npm` and `~/.cache` so package managers don't error out).
pub fn build_profile(worktree: &Path, home: &Path) -> Result<String> {
    let worktree = canonical(worktree)?;
    let home = canonical(home)?;

    let worktree_s = sbpl_string(&worktree.to_string_lossy());
    let home_s = home.to_string_lossy();

    let claude_state = sbpl_string(&format!("{home_s}/.claude"));
    let npm_state = sbpl_string(&format!("{home_s}/.npm"));
    let cache_state = sbpl_string(&format!("{home_s}/.cache"));
    let config_state = sbpl_string(&format!("{home_s}/.config"));
    let local_state = sbpl_string(&format!("{home_s}/.local"));

    // Notes on the SBPL bits below:
    // - (allow default) is a permissive baseline; we narrow writes only.
    // - "/private/tmp" + "/private/var/folders" are macOS's standard temp
    //   locations; tons of tools spew there.
    // - We allow writes to ~/.{claude,npm,cache,config,local} because
    //   claude code + package managers expect them. Without this, npm
    //   install + git config + claude session writes all fail.
    let _ = home_s; // already baked into the *_state vars above
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
  (subpath {npm_state})
  (subpath {cache_state})
  (subpath {config_state})
  (subpath {local_state}))

;; Allow writes to /dev/null, /dev/tty, ptys — required for shells and
;; redirection to behave normally.
(allow file-write* (literal "/dev/null") (literal "/dev/zero"))
(allow file-write*
  (regex #"^/dev/tty[^/]*$")
  (regex #"^/dev/ptmx$")
  (regex #"^/dev/pts/[0-9]+$"))
"#
    ))
}

/// SBPL string literal — must be wrapped in double quotes and have
/// embedded quotes/backslashes escaped.
fn sbpl_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Resolve symlinks and normalize. The sandbox kernel matches paths
/// literally — if the user's repo is at `/Users/foo/code` but `/Users`
/// is symlinked, the sandbox profile must use the canonical path or it
/// silently denies writes.
fn canonical(p: &Path) -> Result<std::path::PathBuf> {
    std::fs::canonicalize(p)
        .map_err(|e| Error::Other(format!("canonicalize {}: {e}", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_includes_worktree_path() {
        let td = tempfile::tempdir().unwrap();
        let wt = td.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let home = td.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        let prof = build_profile(&wt, &home).unwrap();
        // Worktree must appear as a literal SBPL string in the profile,
        // with the canonical absolute path.
        let canon = std::fs::canonicalize(&wt).unwrap();
        assert!(
            prof.contains(&format!("\"{}\"", canon.display())),
            "profile missing worktree path: {prof}"
        );
        assert!(prof.contains("(allow default)"));
        assert!(prof.contains("(deny file-write*)"));
    }

    #[test]
    fn escapes_quotes_in_paths() {
        // Hypothetical (unusual) path with a quote in it
        let s = sbpl_string(r#"/path/with"quote"#);
        assert_eq!(s, r#""/path/with\"quote""#);
    }
}

//! Cursor's home in the guest: `~/.cursor` under the user's home, where
//! `cursor-agent` keeps its state and writes its session transcripts
//! (`projects/<slug>/agent-transcripts/<id>/<id>.jsonl`, which
//! `agent::providers::cursor` reads). Its login lives in the host's OS
//! keychain, which no guest can read, so — as in a container — the one
//! credential is `CURSOR_API_KEY`, taken from the login shell and put in the
//! launch's env file; without it the launch is refused up front rather than
//! failing on the first turn.

use std::path::{Path, PathBuf};

use super::{login_shell_var, Credential, GuestEntry, ProviderHome};
use crate::error::{Error, Result};
use crate::sandbox::engine::AgentLaunchCtx;

/// Where the host mirrors `~/.cursor/projects`, under the box dir.
pub const MIRROR_DIR: &str = "cursor-projects";

const API_KEY: &str = "CURSOR_API_KEY";

/// Launch-blocking message when there is no key: the keychain login cannot
/// reach a guest, so nothing else could authenticate there.
pub const NO_AUTH_MSG: &str = "No Cursor credentials for boxes — set CURSOR_API_KEY (create one at \
     cursor.com/dashboard). `cursor-agent login` stores its token in the host keychain, which a box \
     can't read.";

pub struct Cursor;

impl Cursor {
    /// `~/.cursor/projects` in a guest whose `HOME` is `home`.
    pub fn projects_dir(home: &Path) -> PathBuf {
        home.join(".cursor").join("projects")
    }
}

impl ProviderHome for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn sessions_dir(&self, home: &Path, _writable_root: &Path) -> PathBuf {
        Self::projects_dir(home)
    }

    fn mirror_dirname(&self) -> &'static str {
        MIRROR_DIR
    }

    /// `~/.cursor` itself, as the container launch creates it; cursor lays
    /// the rest out on first run. No shared config: `accounts::shared_items`
    /// names none for cursor.
    fn seed(&self, home: &Path, _writable_root: &Path) -> Result<Vec<GuestEntry>> {
        Ok(vec![GuestEntry::Dir(home.join(".cursor"))])
    }

    fn credential(&self, _ctx: &AgentLaunchCtx) -> Result<Credential> {
        let key = login_shell_var(API_KEY).ok_or_else(|| Error::Other(NO_AUTH_MSG.into()))?;
        Ok(Credential {
            env: vec![(API_KEY.to_string(), key)],
            file: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_live_under_the_home_and_the_seed_is_the_state_dir() {
        let home = Path::new("/Users/u");
        let root = Path::new("/Users/u/.fletch/workspaces/a");
        assert_eq!(
            Cursor.sessions_dir(home, root),
            PathBuf::from("/Users/u/.cursor/projects")
        );
        assert_eq!(Cursor.mirror_dirname(), "cursor-projects");
        assert_eq!(
            Cursor.seed(home, root).unwrap(),
            vec![GuestEntry::Dir(PathBuf::from("/Users/u/.cursor"))]
        );
    }
}

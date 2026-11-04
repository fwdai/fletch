//! Claude Code's home in the guest: `~/.claude` under the user's home (the
//! guest's `HOME` by path identity), with no `CLAUDE_CONFIG_DIR`, so the CLI
//! lays it out itself on first run. Seeded with the onboarding state the
//! managed account dirs seed, so the first turn is not a setup wizard; the
//! login is the access token the host resolved for the launch
//! (`agent::host_login::claude`), in the env file.

use std::path::{Path, PathBuf};

use super::{auth, Credential, GuestEntry, ProviderHome, SharedState};
use crate::error::{Error, Result};
use crate::sandbox::engine::AgentLaunchCtx;
use crate::sandbox::policy;

/// Where the host mirrors `~/.claude/projects`, under the box dir.
pub const MIRROR_DIR: &str = "claude-projects";

/// Why a launch with no credential is refused. Bedrock, Vertex and
/// `apiKeyHelper` setups pass the host's login check but reach no guest.
pub const NO_AUTH_MSG: &str =
    "No Claude Code credentials for Linux VMs. Sign in under Settings → Providers → Claude Code, \
     or set ANTHROPIC_API_KEY in your shell. Bedrock, Vertex and apiKeyHelper setups only work \
     in the macOS sandbox.";

pub struct Claude;

impl Claude {
    /// `~/.claude/projects` in a guest whose `HOME` is `home`.
    pub fn projects_dir(home: &Path) -> PathBuf {
        home.join(".claude").join("projects")
    }
}

impl ProviderHome for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn sessions_dir(&self, home: &Path, _writable_root: &Path) -> PathBuf {
        Self::projects_dir(home)
    }

    fn mirror_dirname(&self) -> &'static str {
        MIRROR_DIR
    }

    /// The projects dir the transcript tail follows, and `~/.claude.json`
    /// with onboarding done (and the user's theme, read from their own state
    /// file — the one `CLAUDE_CONFIG_DIR` names when it is set) unless claude
    /// already wrote one in the guest, which it goes on to change.
    fn seed(&self, home: &Path, _writable_root: &Path) -> Result<Vec<GuestEntry>> {
        let own = crate::agent::accounts::claude_config_override()
            .unwrap_or_else(|| home.to_path_buf())
            .join(".claude.json");
        let state = crate::agent::accounts::claude_state_seed(std::fs::read(&own).ok().as_deref());
        Ok(vec![
            GuestEntry::Dir(Self::projects_dir(home)),
            GuestEntry::File {
                path: home.join(".claude.json"),
                bytes: state.to_string().into_bytes(),
                mode: 0o600,
                keep_existing: true,
            },
        ])
    }

    /// The claude login the host resolved (`agent::host_login`), else the
    /// shell's own key.
    fn credential(&self, ctx: &AgentLaunchCtx) -> Result<Credential> {
        credential_from(auth::claude_env(ctx.oauth_token))
    }

    /// The writable islands of `~/.claude` seatbelt grants
    /// (`policy::provider_state_dirs`), the projects dir among them, and
    /// `~/.claude.json`.
    fn shared_state(&self, home: &Path) -> SharedState {
        SharedState {
            dirs: policy::provider_state_dirs("claude", home),
            files: vec![home.join(".claude.json")],
        }
    }
}

/// [`Claude::credential`] with the env given.
fn credential_from(env: Vec<(String, String)>) -> Result<Credential> {
    if env.is_empty() {
        return Err(Error::Other(NO_AUTH_MSG.into()));
    }
    Ok(Credential {
        env,
        files: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A claude that could only fail on its first turn is refused at launch.
    #[test]
    fn a_launch_with_nothing_to_sign_in_with_is_refused() {
        assert_eq!(
            credential_from(Vec::new()).unwrap_err().to_string(),
            Error::Other(NO_AUTH_MSG.into()).to_string()
        );
        let env = vec![("ANTHROPIC_API_KEY".to_string(), "sk".to_string())];
        assert_eq!(credential_from(env.clone()).unwrap().env, env);
    }

    #[test]
    fn sessions_live_under_the_home_whatever_the_root() {
        let home = Path::new("/Users/u");
        assert_eq!(
            Claude.sessions_dir(home, Path::new("/Users/u/.fletch/workspaces/a")),
            PathBuf::from("/Users/u/.claude/projects")
        );
        assert_eq!(Claude.mirror_dirname(), "claude-projects");
    }

    /// The seed is the projects dir and an onboarding-done state file that
    /// never clobbers one claude wrote.
    #[test]
    fn the_seed_skips_onboarding_without_clobbering_claudes_own_state() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path();
        let entries = Claude.seed(home, home).unwrap();
        assert_eq!(entries[0], GuestEntry::Dir(home.join(".claude/projects")));
        let GuestEntry::File {
            path,
            bytes,
            mode,
            keep_existing,
        } = &entries[1]
        else {
            panic!("{:?}", entries[1]);
        };
        assert_eq!(path, &home.join(".claude.json"));
        assert_eq!(*mode, 0o600);
        assert!(*keep_existing);
        let state: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(state["hasCompletedOnboarding"], true);
    }
}

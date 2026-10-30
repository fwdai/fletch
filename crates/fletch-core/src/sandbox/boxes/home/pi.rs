//! Pi's home in the guest: `~/.pi/agent` under the user's home, holding the
//! login (`auth.json`, a provider-keyed credential map), the settings and
//! the sessions (`sessions/<cwd slug>/<time>_<id>.jsonl`, append-only JSONL
//! that `agent::providers::pi` reads and the relay tails). The settings file
//! is copied in from the user's before every launch; the login is shipped as
//! the credential, and a provider API key from the login shell does as well,
//! as for a container. Extensions a `pi install` put under the user's
//! `~/.pi` are not copied: their layout is not enumerated (see the seatbelt
//! policy's residual note), so a guest pi runs with the built-in tools.

use std::path::{Path, PathBuf};

use super::{copy_tree, login_shell_var, Credential, GuestEntry, ProviderHome, ShippedFile};
use crate::error::Result;
use crate::sandbox::container::launch_auth::{multi_provider_auth_env, present_api_keys};
use crate::sandbox::engine::AgentLaunchCtx;

/// Where the host mirrors `~/.pi/agent/sessions`, under the box dir.
pub const MIRROR_DIR: &str = "pi-sessions";

/// Launch-blocking message when there is neither a login nor a provider key.
pub const NO_AUTH_MSG: &str =
    "No Pi credentials for boxes — sign in with `pi` on the host (writes \
     ~/.pi/agent/auth.json) or set a provider API key (e.g. ANTHROPIC_API_KEY or OPENAI_API_KEY).";

pub struct Pi;

impl Pi {
    /// `~/.pi/agent` in a guest whose `HOME` is `home` — and on the host, by
    /// path identity: pi honours no XDG variable.
    pub fn agent_dir(home: &Path) -> PathBuf {
        home.join(".pi").join("agent")
    }

    /// `~/.pi/agent/sessions`.
    pub fn sessions_dir(home: &Path) -> PathBuf {
        Self::agent_dir(home).join("sessions")
    }
}

impl ProviderHome for Pi {
    fn id(&self) -> &'static str {
        "pi"
    }

    fn sessions_dir(&self, home: &Path, _writable_root: &Path) -> PathBuf {
        Self::sessions_dir(home)
    }

    fn mirror_dirname(&self) -> &'static str {
        MIRROR_DIR
    }

    /// The agent dir, and the user's settings file as a copy, replaced whole
    /// (nothing when the user has none).
    fn seed(&self, home: &Path, _writable_root: &Path) -> Result<Vec<GuestEntry>> {
        let settings = Self::agent_dir(home).join("settings.json");
        let mut entries = vec![
            GuestEntry::Dir(Self::agent_dir(home)),
            GuestEntry::Absent(settings.clone()),
        ];
        entries.extend(copy_tree(&settings, &settings)?);
        Ok(entries)
    }

    /// The login file when the user has one, and every provider API key the
    /// login shell exports; one of the two is required.
    fn credential(&self, ctx: &AgentLaunchCtx) -> Result<Credential> {
        credential_from(ctx.home, present_api_keys(login_shell_var))
    }
}

/// [`Pi::credential`] with the shell's keys given.
fn credential_from(home: &Path, api_keys: Vec<(String, String)>) -> Result<Credential> {
    let auth = Pi::agent_dir(home).join("auth.json");
    let files: Vec<ShippedFile> = auth
        .is_file()
        .then(|| ShippedFile::at(auth))
        .into_iter()
        .collect();
    let env = multi_provider_auth_env(api_keys, !files.is_empty(), NO_AUTH_MSG)?;
    Ok(Credential { env, files })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_live_under_the_agent_dir_and_are_tailed() {
        let home = Path::new("/Users/u");
        assert_eq!(
            Pi.sessions_dir(home, Path::new("/Users/u/.fletch/workspaces/a")),
            PathBuf::from("/Users/u/.pi/agent/sessions")
        );
        assert_eq!(Pi.mirror_dirname(), "pi-sessions");
        assert_eq!(Pi.transport(), super::super::Transport::Tail);
    }

    /// The seed is the agent dir and the settings file copied; the login is
    /// the credential, at the same path on both sides, never seeded.
    #[test]
    fn the_seed_copies_the_settings_and_the_login_is_the_credential() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path();
        let agent = home.join(".pi/agent");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(agent.join("settings.json"), b"{\"theme\":\"dark\"}").unwrap();
        std::fs::write(agent.join("auth.json"), b"{\"anthropic\":{}}").unwrap();

        assert_eq!(
            Pi.seed(home, home).unwrap(),
            vec![
                GuestEntry::Dir(agent.clone()),
                GuestEntry::Absent(agent.join("settings.json")),
                GuestEntry::File {
                    path: agent.join("settings.json"),
                    bytes: b"{\"theme\":\"dark\"}".to_vec(),
                    mode: 0o644,
                    keep_existing: false,
                },
            ]
        );
        let credential = credential_from(home, Vec::new()).unwrap();
        assert!(credential.env.is_empty());
        assert_eq!(
            credential.files,
            vec![ShippedFile::at(agent.join("auth.json"))]
        );
    }

    /// With no login file, a provider key is enough; with neither, the
    /// launch is refused with what to do.
    #[test]
    fn a_login_or_a_key_is_required() {
        let td = tempfile::tempdir().unwrap();
        let key = vec![("ANTHROPIC_API_KEY".to_string(), "sk-test".to_string())];
        let credential = credential_from(td.path(), key.clone()).unwrap();
        assert_eq!(credential.env, key);
        assert!(credential.files.is_empty());
        assert_eq!(
            credential_from(td.path(), Vec::new())
                .unwrap_err()
                .to_string(),
            crate::error::Error::Other(NO_AUTH_MSG.into()).to_string()
        );
    }
}

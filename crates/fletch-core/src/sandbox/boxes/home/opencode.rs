//! OpenCode's home in the guest: its XDG data dir (`~/.local/share/opencode`,
//! the guest sets no XDG variable) holding the login and the session blob
//! store (`storage/message/<ses>/<msg>.json`, `storage/part/<msg>/<part>.json`,
//! which `agent::providers::opencode` reads), and its XDG config dir
//! (`~/.config/opencode`: custom providers, plugins), copied in from the
//! user's before every launch. The host dirs are the user's own, wherever
//! their XDG variables put them (`sandbox::policy`); the guest's are the
//! defaults under the same home.
//!
//! The login is whichever of `auth.json` (legacy) and `opencode.db` (the
//! accounts database) the user's data dir holds, shipped to the guest's data
//! dir before every launch and turn, as the container engines mount it; a
//! provider API key from the login shell does as well. The session blobs are
//! whole files opencode rewrites as a turn proceeds, so the relay's
//! append-only tail cannot carry them: the host pulls the store instead
//! ([`super::Transport::Pull`]) when it reads the transcript and before the
//! box stops.

use std::path::{Path, PathBuf};

use super::{
    copy_tree, login_shell_var, Credential, GuestEntry, ProviderHome, ShippedFile, Transport,
};
use crate::error::Result;
use crate::sandbox::container::launch_auth::{multi_provider_auth_env, present_api_keys};
use crate::sandbox::engine::AgentLaunchCtx;
use crate::sandbox::policy;

/// Where the host mirrors the guest's `storage` dir, under the box dir.
pub const MIRROR_DIR: &str = "opencode-storage";

/// The files of the user's data dir that hold the login.
const LOGIN_FILES: &[&str] = &["auth.json", "opencode.db"];

/// Launch-blocking message when there is neither a login nor a provider key.
pub const NO_AUTH_MSG: &str =
    "No OpenCode credentials for boxes — sign in with `opencode auth login` on \
     the host or set a provider API key (e.g. ANTHROPIC_API_KEY or OPENAI_API_KEY).";

pub struct OpenCode;

impl OpenCode {
    /// The guest's data dir: the XDG default under `home`.
    pub fn data_dir(home: &Path) -> PathBuf {
        home.join(".local").join("share").join("opencode")
    }

    /// The guest's config dir: the XDG default under `home`.
    pub fn config_dir(home: &Path) -> PathBuf {
        home.join(".config").join("opencode")
    }
}

impl ProviderHome for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn sessions_dir(&self, home: &Path, _writable_root: &Path) -> PathBuf {
        Self::data_dir(home).join("storage")
    }

    fn mirror_dirname(&self) -> &'static str {
        MIRROR_DIR
    }

    fn transport(&self) -> Transport {
        Transport::Pull
    }

    /// The data dir, and the config dir as a copy of the user's, replaced
    /// whole (nothing when the user has none: opencode runs without it).
    fn seed(&self, home: &Path, _writable_root: &Path) -> Result<Vec<GuestEntry>> {
        let config = Self::config_dir(home);
        let mut entries = vec![
            GuestEntry::Dir(Self::data_dir(home)),
            GuestEntry::Absent(config.clone()),
        ];
        entries.extend(copy_tree(&policy::opencode_config_dir(home), &config)?);
        Ok(entries)
    }

    /// The login files the user's data dir holds, for the guest's data dir,
    /// and every provider API key the login shell exports; one of the two is
    /// required, as for a container.
    fn credential(&self, ctx: &AgentLaunchCtx) -> Result<Credential> {
        let source = policy::opencode_data_dir(ctx.home);
        let guest = Self::data_dir(ctx.home);
        let files: Vec<ShippedFile> = LOGIN_FILES
            .iter()
            .filter(|name| source.join(name).is_file())
            .map(|name| ShippedFile {
                host: source.join(name),
                guest: guest.join(name),
            })
            .collect();
        let env = multi_provider_auth_env(
            present_api_keys(login_shell_var),
            !files.is_empty(),
            NO_AUTH_MSG,
        )?;
        Ok(Credential { env, files })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(home: &Path) -> AgentLaunchCtx<'_> {
        AgentLaunchCtx {
            agent_id: "a",
            project_id: "p",
            provider: "opencode",
            writable_root: home,
            source_repos: &[],
            rpc_dir: Path::new("/rpc/a"),
            cwd: home,
            home,
            interactive: false,
            blackboard: None,
            account_dir: None,
            oauth_token: None,
            codex_home: None,
            adopted_read_only: false,
            folder_grants: &[],
        }
    }

    #[test]
    fn sessions_are_the_data_dirs_store_and_the_host_pulls_them() {
        let home = Path::new("/Users/u");
        assert_eq!(
            OpenCode.sessions_dir(home, Path::new("/Users/u/.fletch/workspaces/a")),
            PathBuf::from("/Users/u/.local/share/opencode/storage")
        );
        assert_eq!(OpenCode.mirror_dirname(), "opencode-storage");
        assert_eq!(OpenCode.transport(), Transport::Pull);
    }

    /// The seed is the data dir and the config dir copied whole; the login
    /// files are the credential, never part of the seed.
    #[test]
    fn the_seed_copies_the_config_dir_and_leaves_the_login_to_the_credential() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path();
        let config = home.join(".config/opencode");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("opencode.json"), b"{}").unwrap();
        let data = home.join(".local/share/opencode");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("auth.json"), b"{\"anthropic\":{}}").unwrap();

        let entries = OpenCode.seed(home, home).unwrap();
        assert_eq!(
            entries,
            vec![
                GuestEntry::Dir(data.clone()),
                GuestEntry::Absent(config.clone()),
                GuestEntry::Dir(config.clone()),
                GuestEntry::File {
                    path: config.join("opencode.json"),
                    bytes: b"{}".to_vec(),
                    mode: 0o644,
                    keep_existing: false,
                },
            ]
        );

        let credential = OpenCode.credential(&ctx(home)).unwrap();
        assert_eq!(
            credential.files,
            vec![ShippedFile {
                host: data.join("auth.json"),
                guest: data.join("auth.json"),
            }]
        );
    }

    /// With no login file, a provider key from the shell is enough; with
    /// neither, the launch is refused with what to do.
    #[test]
    fn a_login_or_a_key_is_required() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path();
        let keys = present_api_keys(login_shell_var);
        let credential = OpenCode.credential(&ctx(home));
        if keys.is_empty() {
            assert_eq!(
                credential.unwrap_err().to_string(),
                crate::error::Error::Other(NO_AUTH_MSG.into()).to_string()
            );
        } else {
            let credential = credential.unwrap();
            assert_eq!(credential.env, keys);
            assert!(credential.files.is_empty());
        }
    }
}

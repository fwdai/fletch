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

    /// The data dir, and the config dir as a copy of the user's (wherever
    /// their `XDG_CONFIG_HOME` puts it), replaced whole — nothing when the
    /// user has none: opencode runs without it.
    fn seed(&self, home: &Path, _writable_root: &Path) -> Result<Vec<GuestEntry>> {
        seed_from(home, &policy::opencode_config_dir(home))
    }

    /// The login files the user's data dir (wherever their `XDG_DATA_HOME`
    /// puts it) holds, for the guest's data dir, and every provider API key
    /// the login shell exports; one of the two is required, as for a
    /// container.
    fn credential(&self, ctx: &AgentLaunchCtx) -> Result<Credential> {
        credential_from(
            ctx.home,
            &policy::opencode_data_dir(ctx.home),
            present_api_keys(login_shell_var),
        )
    }
}

/// [`OpenCode::seed`] with the user's config dir given.
fn seed_from(home: &Path, host_config: &Path) -> Result<Vec<GuestEntry>> {
    let config = OpenCode::config_dir(home);
    let mut entries = vec![
        GuestEntry::Dir(OpenCode::data_dir(home)),
        GuestEntry::Absent(config.clone()),
    ];
    entries.extend(copy_tree(host_config, &config)?);
    Ok(entries)
}

/// [`OpenCode::credential`] with the user's data dir and the shell's keys
/// given.
fn credential_from(
    home: &Path,
    host_data: &Path,
    api_keys: Vec<(String, String)>,
) -> Result<Credential> {
    let guest = OpenCode::data_dir(home);
    let files: Vec<ShippedFile> = LOGIN_FILES
        .iter()
        .filter(|name| host_data.join(name).is_file())
        .map(|name| ShippedFile {
            host: host_data.join(name),
            guest: guest.join(name),
        })
        .collect();
    let env = multi_provider_auth_env(api_keys, !files.is_empty(), NO_AUTH_MSG)?;
    Ok(Credential { env, files })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The seed is the data dir and the user's config dir — wherever it is on
    /// the host — copied whole to the guest's default place; the login files
    /// are the credential, never part of the seed.
    #[test]
    fn the_seed_copies_the_config_dir_and_leaves_the_login_to_the_credential() {
        let td = tempfile::tempdir().unwrap();
        let home = Path::new("/Users/u");
        let host_config = td.path().join("xdg-config/opencode");
        std::fs::create_dir_all(&host_config).unwrap();
        std::fs::write(host_config.join("opencode.json"), b"{}").unwrap();
        let host_data = td.path().join("xdg-data/opencode");
        std::fs::create_dir_all(&host_data).unwrap();
        std::fs::write(host_data.join("auth.json"), b"{\"anthropic\":{}}").unwrap();

        let config = home.join(".config/opencode");
        assert_eq!(
            seed_from(home, &host_config).unwrap(),
            vec![
                GuestEntry::Dir(home.join(".local/share/opencode")),
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

        let credential = credential_from(home, &host_data, Vec::new()).unwrap();
        assert!(credential.env.is_empty());
        assert_eq!(
            credential.files,
            vec![ShippedFile {
                host: host_data.join("auth.json"),
                guest: home.join(".local/share/opencode/auth.json"),
            }]
        );
    }

    /// With no login file, a provider key is enough and travels in the env;
    /// with neither, the launch is refused with what to do.
    #[test]
    fn a_login_or_a_key_is_required() {
        let td = tempfile::tempdir().unwrap();
        let home = Path::new("/Users/u");
        let key = vec![("OPENAI_API_KEY".to_string(), "sk-test".to_string())];
        let credential = credential_from(home, td.path(), key.clone()).unwrap();
        assert_eq!(credential.env, key);
        assert!(credential.files.is_empty());

        assert_eq!(
            credential_from(home, td.path(), Vec::new())
                .unwrap_err()
                .to_string(),
            crate::error::Error::Other(NO_AUTH_MSG.into()).to_string()
        );
    }
}

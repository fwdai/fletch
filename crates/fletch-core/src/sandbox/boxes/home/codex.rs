//! Codex's home in the guest: the per-agent `CODEX_HOME` overlay the host
//! assembles for every codex launch (`agent::codex_home`), at its own path
//! under the agent's writable root — in the guest by path identity, the
//! `CODEX_HOME` the launch env names. On seatbelt the overlay links the
//! user's shared config in and the host writes the launch credential into it
//! before every launch and turn; in a box the shared config is copied in
//! before every launch, and the credential file the host wrote is shipped to
//! the same guest path whenever the host rewrites it. The refresh token never
//! leaves the host either way.

use std::path::{Path, PathBuf};

use super::{copy_tree, login_shell_var, Credential, CredentialFile, GuestEntry, ProviderHome};
use crate::agent::{accounts, codex_home};
use crate::error::Result;
use crate::sandbox::engine::AgentLaunchCtx;

/// Where the host mirrors `<CODEX_HOME>/sessions`, under the box dir.
pub const MIRROR_DIR: &str = "codex-sessions";

pub struct Codex;

impl Codex {
    /// `<CODEX_HOME>/sessions` for the agent whose writable root is
    /// `writable_root`: the overlay's sessions dir, where `codex exec` writes
    /// its rollouts and `codex exec resume` looks for them.
    pub fn sessions_dir_in(writable_root: &Path) -> PathBuf {
        codex_home::overlay_in(writable_root).join("sessions")
    }
}

impl ProviderHome for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn sessions_dir(&self, _home: &Path, writable_root: &Path) -> PathBuf {
        Self::sessions_dir_in(writable_root)
    }

    fn mirror_dirname(&self) -> &'static str {
        MIRROR_DIR
    }

    /// The overlay as `codex_home::prepare_overlay` lays it out on the host:
    /// its `sessions` dir, and each shared item (`accounts::shared_items`)
    /// as a copy of the user's — replaced whole, so an item the user removed
    /// or renamed since the last launch does not linger.
    fn seed(&self, home: &Path, writable_root: &Path) -> Result<Vec<GuestEntry>> {
        Ok(seed_from(
            writable_root,
            &accounts::shared_source_dir("codex", home),
        ))
    }

    /// The launch credential the host wrote into the overlay
    /// (`codex_home::write_launch_credential`): a copy of the account's login
    /// with the refresh token blanked, rewritten before every launch and
    /// turn. For the default account, the login shell's `OPENAI_API_KEY` as
    /// well, which seatbelt lets through with the rest of the shell env and
    /// the container engines forward (`container::launch_auth`); a managed
    /// account runs on its stored login alone, as it does there.
    fn credential(&self, ctx: &AgentLaunchCtx) -> Result<Credential> {
        let files = ctx
            .codex_home
            .map(|overlay| CredentialFile::host_owned(codex_home::launch_credential_path(overlay)))
            .into_iter()
            .collect();
        let env = match ctx.account_dir {
            None => login_shell_var(OPENAI_API_KEY)
                .map(|key| (OPENAI_API_KEY.to_string(), key))
                .into_iter()
                .collect(),
            Some(_) => Vec::new(),
        };
        Ok(Credential { env, files })
    }
}

const OPENAI_API_KEY: &str = "OPENAI_API_KEY";

/// [`Codex::seed`] with the user's codex home given (wherever their
/// `CODEX_HOME` puts it).
fn seed_from(writable_root: &Path, source: &Path) -> Vec<GuestEntry> {
    let overlay = codex_home::overlay_in(writable_root);
    let mut entries = vec![GuestEntry::Dir(overlay.join("sessions"))];
    for item in accounts::shared_items("codex") {
        entries.push(GuestEntry::Absent(overlay.join(item)));
        entries.extend(copy_tree(&source.join(item), &overlay.join(item)));
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(root: &'a Path, codex_home: Option<&'a Path>) -> AgentLaunchCtx<'a> {
        AgentLaunchCtx {
            agent_id: "a",
            project_id: "p",
            provider: "codex",
            writable_root: root,
            source_repos: &[],
            rpc_dir: Path::new("/rpc/a"),
            cwd: root,
            home: Path::new("/Users/u"),
            interactive: false,
            blackboard: None,
            account_dir: None,
            oauth_token: None,
            codex_home,
            adopted_read_only: false,
            folder_grants: &[],
        }
    }

    #[test]
    fn sessions_live_in_the_agents_overlay() {
        let root = Path::new("/Users/u/.fletch/workspaces/a");
        assert_eq!(
            Codex.sessions_dir(Path::new("/Users/u"), root),
            root.join(".fletch-codex-home/sessions")
        );
        assert_eq!(Codex.mirror_dirname(), "codex-sessions");
    }

    /// The seed lays the overlay out: the sessions dir, then each shared
    /// item cleared and copied from the user's codex home — a file as a
    /// file, a directory as its tree, a missing one cleared only.
    #[test]
    fn the_seed_copies_the_shared_config_in_and_clears_what_is_gone() {
        let td = tempfile::tempdir().unwrap();
        // The user's codex home, wherever CODEX_HOME puts it on the host.
        let codex = td.path().join("codex-home");
        std::fs::create_dir_all(codex.join("prompts")).unwrap();
        std::fs::write(codex.join("config.toml"), b"model = \"gpt\"").unwrap();
        std::fs::write(codex.join("prompts/fix.md"), b"fix it").unwrap();
        std::fs::write(codex.join("auth.json"), b"{\"secret\":1}").unwrap();
        let root = Path::new("/Users/u/.fletch/workspaces/a");
        let overlay = root.join(".fletch-codex-home");

        let entries = seed_from(root, &codex);

        assert_eq!(entries[0], GuestEntry::Dir(overlay.join("sessions")));
        assert!(entries.contains(&GuestEntry::Absent(overlay.join("config.toml"))));
        assert!(entries.contains(&GuestEntry::File {
            path: overlay.join("config.toml"),
            bytes: b"model = \"gpt\"".to_vec(),
            mode: 0o644,
            keep_existing: false,
        }));
        assert!(entries.contains(&GuestEntry::Absent(overlay.join("prompts"))));
        assert!(entries.contains(&GuestEntry::Dir(overlay.join("prompts"))));
        assert!(entries.contains(&GuestEntry::File {
            path: overlay.join("prompts/fix.md"),
            bytes: b"fix it".to_vec(),
            mode: 0o644,
            keep_existing: false,
        }));
        // Absent on the host: cleared in the guest, nothing copied.
        assert!(entries.contains(&GuestEntry::Absent(overlay.join("skills"))));
        assert!(!entries
            .iter()
            .any(|e| matches!(e, GuestEntry::Dir(p) if p == &overlay.join("skills"))));
        // Never the login: the credential travels on its own.
        assert!(!entries.iter().any(|e| match e {
            GuestEntry::File { path, .. } => path.ends_with("auth.json"),
            _ => false,
        }));
        // Clearing precedes copying for every item.
        let index = |want: &GuestEntry| entries.iter().position(|e| e == want).unwrap();
        assert!(
            index(&GuestEntry::Absent(overlay.join("prompts")))
                < index(&GuestEntry::Dir(overlay.join("prompts")))
        );
    }

    /// The credential file is the overlay's launch file; a managed account
    /// gets nothing from the shell, whatever it exports.
    #[test]
    fn the_credential_is_the_overlays_launch_file() {
        let root = Path::new("/Users/u/.fletch/workspaces/a");
        let overlay = root.join(".fletch-codex-home");
        let launch_file = vec![CredentialFile::host_owned(overlay.join("auth.json"))];
        let default = Codex
            .credential(&ctx(root, Some(overlay.as_path())))
            .unwrap();
        assert_eq!(default.files, launch_file);
        assert!(default.env.iter().all(|(k, _)| k == OPENAI_API_KEY));

        let mut managed = ctx(root, Some(overlay.as_path()));
        managed.account_dir = Some(Path::new("/Users/u/.fletch/accounts/codex/work"));
        assert_eq!(
            Codex.credential(&managed).unwrap(),
            Credential {
                env: Vec::new(),
                files: launch_file,
            }
        );
        assert!(Codex.credential(&ctx(root, None)).unwrap().files.is_empty());
    }
}

//! Fletch-managed provider accounts: one sign-in per config directory.
//!
//! A CLI keeps one login per config directory — claude's `CLAUDE_CONFIG_DIR`,
//! codex's `CODEX_HOME` — so "another account" is another directory. Fletch
//! owns these under `~/.fletch/accounts/<provider>/<id>/`. The directory
//! listing *is* the registry (nothing in the database to drift from the disk),
//! and one `settings` key per provider names which account new agents use.
//!
//! The user's own CLI directory (`~/.claude`, `~/.codex`, or wherever their
//! shell's env points) stays the **default** account: no directory here, no
//! env override, so a user with one login sees nothing change. It is also the
//! **shared source**: the config a managed directory should inherit —
//! settings, instructions, commands, skills — is symlinked from it, so editing
//! it from the terminal reaches every account and nothing is ever copied. The
//! login itself, the session transcripts and the CLI's own identity file stay
//! per directory, which is the whole point.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::auth_probe::{self, AuthStatus};
use crate::error::{Error, Result};

/// Env var overriding the accounts root (default `~/.fletch/accounts`). Same
/// style as `workspace::paths::TOOLS_ROOT_ENV` — set in tests so nothing
/// touches a developer's real `~/.fletch`.
pub const ACCOUNTS_ROOT_ENV: &str = "FLETCH_ACCOUNTS_ROOT";

/// The id of the account that is the CLI's own config dir. Never a directory
/// under the root; reserved so a managed account can't shadow it.
pub const DEFAULT_ACCOUNT: &str = "default";

/// `settings` key prefix naming the active account per provider
/// (`provider_account_claude`). Absent, blank or [`DEFAULT_ACCOUNT`] all mean
/// the default.
pub const ACTIVE_SETTING_PREFIX: &str = "provider_account_";

/// The providers whose CLI relocates its whole state with one env var. Only
/// these have accounts; the rest keep their single sign-in.
pub const ACCOUNT_PROVIDERS: [&str; 2] = ["claude", "codex"];

pub fn active_setting_key(provider: &str) -> String {
    format!("{ACTIVE_SETTING_PREFIX}{provider}")
}

/// The env var that points `provider`'s CLI at a config directory, or `None`
/// for a provider without one.
pub fn config_dir_env(provider: &str) -> Option<&'static str> {
    match provider {
        "claude" => Some("CLAUDE_CONFIG_DIR"),
        "codex" => Some("CODEX_HOME"),
        _ => None,
    }
}

pub fn supports_accounts(provider: &str) -> bool {
    config_dir_env(provider).is_some()
}

pub fn is_default(id: &str) -> bool {
    id.is_empty() || id == DEFAULT_ACCOUNT
}

/// What a managed directory inherits from the shared source, by link. Config
/// only — never the login, the identity file (`.claude.json`, which also holds
/// onboarding state) or the session stores, which are exactly what differs per
/// account.
fn shared_items(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &[
            "settings.json",
            "CLAUDE.md",
            "commands",
            "skills",
            "agents",
            "plugins",
        ],
        "codex" => &["config.toml", "AGENTS.md", "prompts", "skills"],
        _ => &[],
    }
}

/// `~/.fletch/accounts/`, a sibling of the workspaces and tools roots;
/// `$FLETCH_ACCOUNTS_ROOT` overrides it when set and non-empty.
pub fn accounts_root() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os(ACCOUNTS_ROOT_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    Ok(home.join(".fletch").join("accounts"))
}

/// An account id doubles as its directory name and its label: lowercase
/// ASCII letters, digits and hyphens, starting alphanumeric, at most 32 chars,
/// and never the reserved default.
pub fn validate_account_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    if !ok {
        return Err(Error::Other(
            "Account names use lowercase letters, digits and hyphens (up to 32 characters).".into(),
        ));
    }
    if is_default(id) {
        return Err(Error::Other(format!(
            "`{DEFAULT_ACCOUNT}` is the CLI's own login."
        )));
    }
    Ok(())
}

fn validate_provider(provider: &str) -> Result<()> {
    if supports_accounts(provider) {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "`{provider}` has no account directories."
        )))
    }
}

/// The directory of a managed account, validated but not necessarily present.
pub fn account_dir(provider: &str, id: &str) -> Result<PathBuf> {
    validate_provider(provider)?;
    validate_account_id(id)?;
    Ok(accounts_root()?.join(provider).join(id))
}

/// Every managed account of `provider`, by id, sorted. A directory whose name
/// isn't a valid id (something the user dropped there by hand) is skipped.
pub fn list_account_ids(provider: &str) -> Vec<String> {
    let Ok(root) = accounts_root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root.join(provider)) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|id| validate_account_id(id).is_ok())
        .collect();
    ids.sort();
    ids
}

/// The CLI's own config dir on this host — the default account, and the source
/// every managed directory links its shared config from. Honours the user's
/// own env override so a terminal that already relocates the dir is followed.
pub fn shared_source_dir(provider: &str, home: &Path) -> PathBuf {
    match provider {
        "claude" => std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude")),
        "codex" => crate::sandbox::policy::codex_home_dir(home),
        _ => home.to_path_buf(),
    }
}

/// Create (or repair) a managed account directory and return it. Idempotent
/// and cheap, so callers run it before every login and every spawn: a shared
/// item that appeared in the source since last time gets linked now; one the
/// CLI has since replaced with a real file (temp-then-rename does that) is left
/// alone rather than clobbered — a forked config is the user's to resolve.
pub fn ensure_account_dir(provider: &str, id: &str) -> Result<PathBuf> {
    let dir = account_dir(provider, id)?;
    std::fs::create_dir_all(&dir)?;
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    link_shared(
        &shared_source_dir(provider, &home),
        &dir,
        shared_items(provider),
    );
    Ok(dir)
}

/// Link each of `items` from `source` into `dir` where the source exists and
/// nothing sits at the target yet. Best-effort per item: one failure is logged
/// and the rest proceed, since a missing link costs a shared setting, not the
/// login.
fn link_shared(source: &Path, dir: &Path, items: &[&str]) {
    for item in items {
        let src = source.join(item);
        let dst = dir.join(item);
        if dst.symlink_metadata().is_ok() || !src.exists() {
            continue;
        }
        if let Err(e) = symlink(&src, &dst) {
            tracing::warn!(item, error = %e, "could not link shared config into account dir");
        }
    }
}

#[cfg(unix)]
fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::os::windows::fs::symlink_dir(src, dst)
    } else {
        std::os::windows::fs::symlink_file(src, dst)
    }
}

/// Delete a managed account directory, links and all. The login it held in
/// the macOS Keychain is the CLI's item, not ours, and is left in place.
pub fn remove_account_dir(provider: &str, id: &str) -> Result<()> {
    let dir = account_dir(provider, id)?;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The env that points `provider`'s CLI at account `id`: empty for the
/// default, which runs with whatever the CLI resolves on its own.
pub fn account_env(provider: &str, id: &str) -> Result<Vec<(String, String)>> {
    if is_default(id) {
        return Ok(Vec::new());
    }
    let var = config_dir_env(provider)
        .ok_or_else(|| Error::Other(format!("`{provider}` has no account directories.")))?;
    let dir = account_dir(provider, id)?;
    Ok(vec![(var.to_string(), dir.to_string_lossy().into_owned())])
}

/// One account of one provider, as Settings lists it.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderAccount {
    pub provider: String,
    pub id: String,
    /// False for the default: the CLI's own directory, which Fletch neither
    /// created nor can remove.
    pub managed: bool,
    /// Whether new agents of this provider use this account.
    pub active: bool,
    pub status: AuthStatus,
    /// The probe's fixed reason for a non-signed-in status; see
    /// [`super::ProviderAuthProbe::detail`].
    pub detail: Option<String>,
}

/// Every account of every account-capable provider, the default first, each
/// probed for its login. `active_id` answers "which account does `provider`'s
/// setting name?"; a name with no directory behind it falls back to the
/// default, the same way the spawn path does. Never errors: a provider whose
/// root can't be read just lists its default.
pub fn list_accounts(active_id: impl Fn(&str) -> Option<String>) -> Vec<ProviderAccount> {
    let mut out = Vec::new();
    for provider in ACCOUNT_PROVIDERS {
        let ids = list_account_ids(provider);
        let active = active_id(provider)
            .filter(|id| ids.iter().any(|known| known == id))
            .unwrap_or_else(|| DEFAULT_ACCOUNT.to_string());

        let probe = auth_probe::probe_default(provider);
        out.push(ProviderAccount {
            provider: provider.to_string(),
            id: DEFAULT_ACCOUNT.to_string(),
            managed: false,
            active: active == DEFAULT_ACCOUNT,
            status: probe.status,
            detail: probe.detail,
        });

        for id in ids {
            let Ok(dir) = account_dir(provider, &id) else {
                continue;
            };
            let probe = auth_probe::probe_dir(provider, &dir);
            out.push(ProviderAccount {
                provider: provider.to_string(),
                active: active == id,
                id,
                managed: true,
                status: probe.status,
                detail: probe.detail,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the root env var: every test here points it at its own
    /// tempdir, and tests in one binary run in parallel.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_root<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let td = tempfile::tempdir().unwrap();
        std::env::set_var(ACCOUNTS_ROOT_ENV, td.path());
        let out = f(td.path());
        std::env::remove_var(ACCOUNTS_ROOT_ENV);
        out
    }

    #[test]
    fn ids_are_lowercase_slugs_and_never_the_default() {
        assert!(validate_account_id("work").is_ok());
        assert!(validate_account_id("team-2").is_ok());
        assert!(validate_account_id("Work").is_err());
        assert!(validate_account_id("-x").is_err());
        assert!(validate_account_id("a b").is_err());
        assert!(validate_account_id("").is_err());
        assert!(validate_account_id(DEFAULT_ACCOUNT).is_err());
        assert!(validate_account_id(&"a".repeat(33)).is_err());
    }

    #[test]
    fn only_env_relocatable_providers_have_accounts() {
        assert_eq!(config_dir_env("claude"), Some("CLAUDE_CONFIG_DIR"));
        assert_eq!(config_dir_env("codex"), Some("CODEX_HOME"));
        assert_eq!(config_dir_env("cursor"), None);
        assert!(account_dir("cursor", "work").is_err());
    }

    #[test]
    fn the_default_account_sets_no_env_and_a_managed_one_points_at_its_dir() {
        with_root(|root| {
            assert!(account_env("claude", DEFAULT_ACCOUNT).unwrap().is_empty());
            assert!(account_env("claude", "").unwrap().is_empty());
            let env = account_env("codex", "work").unwrap();
            assert_eq!(
                env,
                vec![(
                    "CODEX_HOME".to_string(),
                    root.join("codex")
                        .join("work")
                        .to_string_lossy()
                        .into_owned()
                )]
            );
        });
    }

    #[test]
    fn listing_reads_the_directories_and_skips_strays() {
        with_root(|root| {
            assert!(list_account_ids("claude").is_empty());
            std::fs::create_dir_all(root.join("claude").join("work")).unwrap();
            std::fs::create_dir_all(root.join("claude").join("personal")).unwrap();
            std::fs::create_dir_all(root.join("claude").join("Not A Slug")).unwrap();
            std::fs::write(root.join("claude").join("stray.txt"), "x").unwrap();
            assert_eq!(list_account_ids("claude"), vec!["personal", "work"]);
            assert!(list_account_ids("codex").is_empty());
        });
    }

    #[test]
    fn linking_follows_the_source_and_never_clobbers() {
        let td = tempfile::tempdir().unwrap();
        let source = td.path().join("source");
        let dir = td.path().join("account");
        std::fs::create_dir_all(source.join("commands")).unwrap();
        std::fs::write(source.join("settings.json"), "{}").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        // A fork the CLI left behind: a real file where a link would go.
        std::fs::write(dir.join("CLAUDE.md"), "mine").unwrap();
        std::fs::write(source.join("CLAUDE.md"), "shared").unwrap();

        link_shared(&source, &dir, shared_items("claude"));

        assert!(dir
            .join("settings.json")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(dir
            .join("commands")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        // Absent in the source: nothing to link, and no dangling link either.
        assert!(dir.join("skills").symlink_metadata().is_err());
        // The fork survives untouched.
        assert!(!dir
            .join("CLAUDE.md")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            "mine"
        );

        // A source item that appears later is picked up by the next repair.
        std::fs::create_dir_all(source.join("skills")).unwrap();
        link_shared(&source, &dir, shared_items("claude"));
        assert!(dir
            .join("skills")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn removing_is_idempotent_and_scoped_to_the_account() {
        with_root(|root| {
            let dir = root.join("codex").join("work");
            std::fs::create_dir_all(dir.join("sessions")).unwrap();
            std::fs::write(root.join("codex").join("keep"), "x").unwrap();
            remove_account_dir("codex", "work").unwrap();
            assert!(!dir.exists());
            assert!(root.join("codex").join("keep").exists());
            remove_account_dir("codex", "work").unwrap();
        });
    }
}

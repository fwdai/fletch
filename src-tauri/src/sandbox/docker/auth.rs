//! Anthropic auth for containerized agents (slice D1).
//!
//! Host claude logins usually live in the macOS Keychain, which doesn't exist
//! inside a container, and Fletch itself injects no credentials — so docker
//! agents need auth resolved explicitly. [`resolve`] walks a first-hit-wins
//! chain and returns the env vars to set on the *docker CLI process*; the
//! launch path (B2) forwards them with bare `-e VAR` flags, so token values
//! never appear in argv (invariant 3). They must never appear in logs either:
//! [`ContainerAuth`] redacts values in its `Debug` output, and nothing in this
//! module traces a token.
//!
//! The chain:
//! 1. A `claude setup-token` value pasted into settings
//!    ([`TOKEN_SETTING`]) → `CLAUDE_CODE_OAUTH_TOKEN`.
//! 2. The login-shell env exports `ANTHROPIC_API_KEY` or
//!    `CLAUDE_CODE_OAUTH_TOKEN` → forward those (plus `ANTHROPIC_BASE_URL` /
//!    `ANTHROPIC_AUTH_TOKEN` for proxy setups, which ride along but don't
//!    constitute a hit on their own).
//! 3. `<home>/.claude/.credentials.json` exists → nothing to inject: the
//!    `~/.claude` bind mount carries it, and refresh writes land on the host.
//! 4. Nothing → [`ContainerAuth::Unavailable`]; the spawn path fails fast and
//!    the UI shows the "Connect Claude for containers" call-to-action.
//!
//! Seatbelt agents never see any of this — they keep the user's own login.

use std::collections::HashMap;
use std::fmt;

use parking_lot::RwLock;

use crate::bin_resolve;

/// `settings` key holding the user-pasted `claude setup-token` value.
/// Plaintext in sqlite — the same posture as `github::TOKEN_SETTING`
/// (consistency over novelty; a keychain migration would move both).
pub const TOKEN_SETTING: &str = "claude_container_token";

/// Env var claude reads a setup-token (OAuth) credential from.
const OAUTH_TOKEN_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// Shell vars that constitute a chain hit on their own.
const SHELL_KEY_VARS: [&str; 2] = ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"];

/// Everything forwarded from the login shell once one of [`SHELL_KEY_VARS`]
/// is present; the extra two support proxy/gateway setups.
const SHELL_AUTH_VARS: [&str; 4] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
];

/// Expected prefix of a `claude setup-token` credential. Other shapes are
/// accepted with a warning — the format isn't a contract we own.
const SETUP_TOKEN_PREFIX: &str = "sk-ant-oat";

/// The stored token, cached in-process. Seeded from the DB at startup and
/// updated by the `set_container_auth_token` / `clear_container_auth_token`
/// commands, so [`resolve`] — called deep in spawn paths with no DB handle —
/// never touches the DB. Same pattern as `github::set_token`.
static STORED_TOKEN: RwLock<Option<String>> = RwLock::new(None);

/// Replace the in-process stored token. Callers that change the *persisted*
/// token write the DB and then call this; blank counts as none.
pub fn set_stored_token(token: Option<String>) {
    *STORED_TOKEN.write() = sanitize(token);
}

fn stored_token() -> Option<String> {
    STORED_TOKEN.read().clone()
}

/// Blank-to-none normalization for the mirror: a cleared setting is stored as
/// `""` (like `github_disconnect`), which must not count as a token.
fn sanitize(token: Option<String>) -> Option<String> {
    token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Which chain step supplied the credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSource {
    /// The pasted setup-token from settings.
    StoredToken,
    /// Auth vars exported by the user's login shell.
    ShellEnv,
    /// `~/.claude/.credentials.json` — carried by the `~/.claude` mount, so
    /// there is nothing to inject.
    CredentialsFile,
}

/// Outcome of the resolution chain: the env to set on the docker CLI process
/// (forwarded into the container via bare `-e VAR`), or nothing usable.
pub enum ContainerAuth {
    Resolved {
        env: Vec<(String, String)>,
        source: AuthSource,
    },
    Unavailable,
}

/// Manual impl so a stray `{:?}` in a log line can never leak a token: env
/// entries print their var *names* only.
impl fmt::Debug for ContainerAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolved { env, source } => f
                .debug_struct("Resolved")
                .field("source", source)
                .field(
                    "env",
                    &env.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
                )
                .finish(),
            Self::Unavailable => write!(f, "Unavailable"),
        }
    }
}

/// Walk the auth chain (first hit wins). Called by the docker launch path at
/// spawn time and, via [`status`], by the settings UI. May block on the first
/// call: the login-shell env is loaded (a shell runs) if nothing earlier
/// populated `bin_resolve`'s cache.
pub fn resolve() -> ContainerAuth {
    let credentials_file =
        dirs::home_dir().is_some_and(|home| home.join(".claude/.credentials.json").exists());
    resolve_from(
        stored_token(),
        bin_resolve::login_shell_env(),
        credentials_file,
    )
}

/// The chain itself, pure over its inputs so tests can exercise the ordering
/// without touching process globals or the filesystem.
fn resolve_from(
    stored: Option<String>,
    shell_env: Option<&HashMap<String, String>>,
    credentials_file: bool,
) -> ContainerAuth {
    if let Some(token) = stored {
        return ContainerAuth::Resolved {
            env: vec![(OAUTH_TOKEN_VAR.to_string(), token)],
            source: AuthSource::StoredToken,
        };
    }
    if let Some(env) = shell_env {
        let set = |var: &str| env.get(var).map(String::as_str).map(str::trim);
        if SHELL_KEY_VARS
            .iter()
            .any(|var| set(var).is_some_and(|v| !v.is_empty()))
        {
            let forwarded = SHELL_AUTH_VARS
                .iter()
                .filter(|var| set(var).is_some_and(|v| !v.is_empty()))
                .map(|var| (var.to_string(), env[*var].clone()))
                .collect();
            return ContainerAuth::Resolved {
                env: forwarded,
                source: AuthSource::ShellEnv,
            };
        }
    }
    if credentials_file {
        return ContainerAuth::Resolved {
            env: Vec::new(),
            source: AuthSource::CredentialsFile,
        };
    }
    ContainerAuth::Unavailable
}

/// Wire shape of the `get_container_auth_status` command — [`resolve`]'s
/// outcome for the settings status row. Serializes like `DockerAvailability`:
/// `{ "status": "stored-token" | "shell-env" | "credentials-file" | "none" }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ContainerAuthStatus {
    StoredToken,
    ShellEnv,
    CredentialsFile,
    None,
}

/// Which chain step is active right now (settings UI polling).
pub fn status() -> ContainerAuthStatus {
    match resolve() {
        ContainerAuth::Resolved { source, .. } => match source {
            AuthSource::StoredToken => ContainerAuthStatus::StoredToken,
            AuthSource::ShellEnv => ContainerAuthStatus::ShellEnv,
            AuthSource::CredentialsFile => ContainerAuthStatus::CredentialsFile,
        },
        ContainerAuth::Unavailable => ContainerAuthStatus::None,
    }
}

/// Normalize a pasted token for storage: trimmed, non-empty, plus whether it
/// matches the expected setup-token shape (callers warn-but-accept on a
/// mismatch). The error string never contains the input.
pub fn normalize_token(raw: &str) -> Result<(String, bool), String> {
    let token = raw.trim();
    if token.is_empty() {
        return Err("Token is empty — run `claude setup-token` and paste its output.".into());
    }
    Ok((token.to_string(), token.starts_with(SETUP_TOKEN_PREFIX)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn resolved(auth: ContainerAuth) -> (Vec<(String, String)>, AuthSource) {
        match auth {
            ContainerAuth::Resolved { env, source } => (env, source),
            ContainerAuth::Unavailable => panic!("expected Resolved"),
        }
    }

    #[test]
    fn stored_token_beats_shell_env_and_credentials_file() {
        let shell = shell_env(&[("ANTHROPIC_API_KEY", "sk-ant-api-key")]);
        let auth = resolve_from(Some("sk-ant-oat-stored".into()), Some(&shell), true);
        let (env, source) = resolved(auth);
        assert_eq!(source, AuthSource::StoredToken);
        assert_eq!(
            env,
            vec![(
                "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
                "sk-ant-oat-stored".to_string()
            )]
        );
    }

    #[test]
    fn shell_env_beats_credentials_file_and_forwards_proxy_vars() {
        let shell = shell_env(&[
            ("ANTHROPIC_API_KEY", "sk-ant-api-key"),
            ("ANTHROPIC_BASE_URL", "https://proxy.example.com"),
            ("PATH", "/usr/bin"),
        ]);
        let (env, source) = resolved(resolve_from(None, Some(&shell), true));
        assert_eq!(source, AuthSource::ShellEnv);
        let mut keys: Vec<_> = env.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"]);
    }

    #[test]
    fn proxy_vars_alone_are_not_a_hit() {
        // BASE_URL without a key var must fall through — it can't authenticate.
        let shell = shell_env(&[("ANTHROPIC_BASE_URL", "https://proxy.example.com")]);
        let (env, source) = resolved(resolve_from(None, Some(&shell), true));
        assert_eq!(source, AuthSource::CredentialsFile);
        assert!(env.is_empty());
    }

    #[test]
    fn blank_shell_values_are_ignored() {
        let shell = shell_env(&[("ANTHROPIC_API_KEY", "  ")]);
        assert!(matches!(
            resolve_from(None, Some(&shell), false),
            ContainerAuth::Unavailable
        ));
    }

    #[test]
    fn credentials_file_resolves_with_empty_env() {
        // The ~/.claude mount carries the file; nothing to inject.
        let (env, source) = resolved(resolve_from(None, None, true));
        assert_eq!(source, AuthSource::CredentialsFile);
        assert!(env.is_empty());
    }

    #[test]
    fn nothing_resolves_to_unavailable() {
        assert!(matches!(
            resolve_from(None, None, false),
            ContainerAuth::Unavailable
        ));
        assert!(matches!(
            resolve_from(None, Some(&shell_env(&[("PATH", "/usr/bin")])), false),
            ContainerAuth::Unavailable
        ));
    }

    #[test]
    fn debug_output_redacts_token_values() {
        let auth = resolve_from(Some("sk-ant-oat-SECRET-VALUE".into()), None, false);
        let printed = format!("{auth:?}");
        assert!(printed.contains("CLAUDE_CODE_OAUTH_TOKEN"), "{printed}");
        assert!(printed.contains("StoredToken"), "{printed}");
        assert!(!printed.contains("SECRET"), "token leaked: {printed}");
    }

    #[test]
    fn sanitize_drops_blank_and_trims() {
        assert_eq!(sanitize(None), None);
        assert_eq!(sanitize(Some("".into())), None);
        assert_eq!(sanitize(Some("   ".into())), None);
        assert_eq!(sanitize(Some(" tok ".into())), Some("tok".into()));
    }

    #[test]
    fn normalize_token_rejects_empty_and_flags_shape() {
        assert!(normalize_token("").is_err());
        assert!(normalize_token("  \n ").is_err());
        assert_eq!(
            normalize_token(" sk-ant-oat01-abc \n"),
            Ok(("sk-ant-oat01-abc".to_string(), true))
        );
        // Unknown shapes are accepted but flagged so the command can warn.
        assert_eq!(
            normalize_token("some-proxy-token"),
            Ok(("some-proxy-token".to_string(), false))
        );
    }

    #[test]
    fn status_serializes_to_the_wire_shape() {
        for (status, wire) in [
            (ContainerAuthStatus::StoredToken, "stored-token"),
            (ContainerAuthStatus::ShellEnv, "shell-env"),
            (ContainerAuthStatus::CredentialsFile, "credentials-file"),
            (ContainerAuthStatus::None, "none"),
        ] {
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::json!({ "status": wire })
            );
        }
    }
}

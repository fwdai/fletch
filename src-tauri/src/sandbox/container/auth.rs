//! Anthropic auth for containerized agents. [`resolve`] walks a first-hit-wins
//! chain — Keychain login, stored setup-token, shell/process env, a usable
//! `~/.claude/.credentials.json`, else [`ContainerAuth::Unavailable`] — and is
//! re-evaluated on every spawn, so a `claude` re-login lands immediately. It
//! returns env for the *container CLI process*, which bare `-e VAR` flags
//! forward, so token values never appear in argv (invariant 3); nor in logs,
//! since [`ContainerAuth`]'s `Debug` prints var names only.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;

use crate::bin_resolve;

/// `crate::secrets` key holding the user-pasted `claude setup-token` value.
pub const TOKEN_SETTING: &str = "claude_container_token";

/// Env var claude reads a setup-token (OAuth) credential from.
const OAUTH_TOKEN_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// macOS Keychain service Claude Code stores its login under; the password
/// payload is the same JSON as `~/.claude/.credentials.json`.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Shell vars that constitute a chain hit on their own — the gateway bearer
/// `ANTHROPIC_AUTH_TOKEN` included, so a gateway-only host isn't refused.
const SHELL_KEY_VARS: [&str; 3] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
];

/// Everything forwarded once one of [`SHELL_KEY_VARS`] is present.
const SHELL_AUTH_VARS: [&str; 4] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
];

/// Endpoint vars forwarded alongside a credential resolved from a *higher* chain
/// step, to point that login at a custom proxy. Credential vars must never be
/// listed here: an ambient one would override the resolved login.
const PROXY_RIDE_ALONG: [&str; 1] = ["ANTHROPIC_BASE_URL"];

/// Expected prefix of a `claude setup-token` credential. Other shapes are
/// accepted with a warning — the format isn't a contract we own.
const SETUP_TOKEN_PREFIX: &str = "sk-ant-oat";

/// The stored token, mirrored in-process so [`resolve`] — called deep in spawn
/// paths with no DB handle — never touches the DB.
static STORED_TOKEN: RwLock<Option<String>> = RwLock::new(None);

/// True once an explicit [`set_stored_token`] has run, so a delayed startup-seed
/// retry can't overwrite newer user action with the stale value it read.
static SEALED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Replace the in-process stored token (blank counts as none) and seal the
/// mirror against any still-pending startup seed.
pub fn set_stored_token(token: Option<String>) {
    let mut w = STORED_TOKEN.write();
    SEALED.store(true, std::sync::atomic::Ordering::SeqCst);
    *w = sanitize(token);
}

/// Startup-seed variant of [`set_stored_token`]: applies only while no explicit
/// set has run. The seal is read under the mirror's write lock, so a racing
/// paste/clear wins in either order.
pub fn seed_stored_token(token: Option<String>) {
    let mut w = STORED_TOKEN.write();
    if SEALED.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    *w = sanitize(token);
}

fn stored_token() -> Option<String> {
    STORED_TOKEN.read().clone()
}

/// Blank-to-none: a cleared setting is persisted as `""`, which must not count
/// as a token.
fn sanitize(token: Option<String>) -> Option<String> {
    token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Which chain step supplied the credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSource {
    /// The live macOS Keychain login, read fresh each spawn.
    Keychain,
    /// The pasted/captured setup-token from settings.
    StoredToken,
    /// Auth vars from the app's process env or the user's login shell.
    ShellEnv,
    /// `~/.claude/.credentials.json` — carried by the mount, nothing to inject.
    CredentialsFile,
}

/// Outcome of the chain: the env to set on the container CLI process (forwarded
/// via bare `-e VAR`), or nothing usable.
pub enum ContainerAuth {
    Resolved {
        env: Vec<(String, String)>,
        source: AuthSource,
    },
    Unavailable,
}

/// Manual impl so a stray `{:?}` can never leak a token: env entries print their
/// var *names* only.
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

/// The config dir whose `.credentials.json` claude reads: `CLAUDE_CONFIG_DIR`
/// if set, else `~/.claude`.
fn credentials_config_dir(config_dir_env: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    config_dir_env
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".claude")))
}

/// Walk the auth chain (first hit wins). May block on the first call: loading
/// the login-shell env runs a shell if nothing populated `bin_resolve`'s cache.
pub fn resolve() -> ContainerAuth {
    let keychain = keychain_token();
    // The dir claude will actually read — and the one the engine mounts (see
    // `nondefault_claude_config_dir`); hardcoding `~/.claude` would refuse a
    // container whose only credential lives in a custom config dir.
    let credentials_file = credentials_config_dir(
        std::env::var_os("CLAUDE_CONFIG_DIR").as_deref(),
        dirs::home_dir().as_deref(),
    )
    .is_some_and(|dir| {
        credentials_file_usable(std::fs::read(dir.join(".credentials.json")).ok().as_deref())
    });
    let process_env: HashMap<String, String> = SHELL_AUTH_VARS
        .iter()
        .filter_map(|var| std::env::var(var).ok().map(|v| (var.to_string(), v)))
        .collect();
    let env = merge_auth_env(&process_env, bin_resolve::login_shell_env());
    resolve_from(keychain, stored_token(), env.as_ref(), credentials_file)
}

/// The live host login token from the macOS Keychain, read fresh on every
/// [`resolve`] so a `claude` re-login lands on the next spawn. `None` when
/// there's no readable/usable login (Keychain locked or empty, non-macOS host).
#[cfg(target_os = "macos")]
fn keychain_token() -> Option<String> {
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    usable_oauth_token(Some(&out.stdout))
}

#[cfg(not(target_os = "macos"))]
fn keychain_token() -> Option<String> {
    None
}

/// Fold the app's own process environment together with the login-shell probe
/// into one auth view for the env chain step (login-shell wins on collision);
/// `None` when neither carries an auth var. Both are consulted because the
/// container CLI child inherits the process env: a token visible only there —
/// e.g. on a bash-only host the `/bin/zsh -lc` probe can't read — would
/// otherwise resolve `Unavailable` and abort a launch that would have worked.
fn merge_auth_env(
    process_env: &HashMap<String, String>,
    shell_env: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let mut merged = HashMap::new();
    for var in SHELL_AUTH_VARS {
        if let Some(v) = process_env.get(var) {
            merged.insert(var.to_string(), v.clone());
        }
    }
    if let Some(shell) = shell_env {
        for var in SHELL_AUTH_VARS {
            if let Some(v) = shell.get(var) {
                merged.insert(var.to_string(), v.clone());
            }
        }
    }
    (!merged.is_empty()).then_some(merged)
}

/// Whether `.credentials.json` carries a credential the container can
/// authenticate with — see [`usable_oauth_token`] for the usability bar.
fn credentials_file_usable(contents: Option<&[u8]>) -> bool {
    usable_oauth_token(contents).is_some()
}

/// Extract a container-usable OAuth access token from a credentials JSON blob —
/// the `.credentials.json` file *or* the macOS Keychain password, which share
/// the shape. Requires a non-empty token and `expiresAt > 0`: a macOS Keychain
/// login leaves an `expiresAt: 0` placeholder on disk, and treating it as a hit
/// boots the container into a login prompt it can't answer. Expired-but-positive
/// is accepted — the container refreshes and the write lands on the mount.
fn usable_oauth_token(contents: Option<&[u8]>) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(contents?).ok()?;
    let oauth = &json["claudeAiOauth"];
    let token = oauth["accessToken"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())?;
    let expires_ok = oauth["expiresAt"].as_i64().is_some_and(|e| e > 0);
    expires_ok.then(|| token.to_string())
}

/// The chain itself, pure over its inputs so tests can exercise the ordering.
fn resolve_from(
    keychain: Option<String>,
    stored: Option<String>,
    shell_env: Option<&HashMap<String, String>>,
    credentials_file: bool,
) -> ContainerAuth {
    // Only `PROXY_RIDE_ALONG` endpoints follow a credential taken from a higher
    // step; forwarding an ambient credential var would override that login.
    let with_proxy = |mut env: Vec<(String, String)>| -> Vec<(String, String)> {
        if let Some(shell) = shell_env {
            for var in PROXY_RIDE_ALONG {
                if let Some(value) = shell.get(var).map(|v| v.trim()).filter(|v| !v.is_empty()) {
                    env.push((var.to_string(), value.to_string()));
                }
            }
        }
        env
    };

    if let Some(token) = keychain {
        return ContainerAuth::Resolved {
            env: with_proxy(vec![(OAUTH_TOKEN_VAR.to_string(), token)]),
            source: AuthSource::Keychain,
        };
    }
    if let Some(token) = stored {
        return ContainerAuth::Resolved {
            env: with_proxy(vec![(OAUTH_TOKEN_VAR.to_string(), token)]),
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
                .filter_map(|var| {
                    let value = set(var)?;
                    if value.is_empty() {
                        None
                    } else {
                        Some((var.to_string(), value.to_string()))
                    }
                })
                .collect();
            return ContainerAuth::Resolved {
                env: forwarded,
                source: AuthSource::ShellEnv,
            };
        }
    }
    if credentials_file {
        return ContainerAuth::Resolved {
            env: with_proxy(Vec::new()),
            source: AuthSource::CredentialsFile,
        };
    }
    ContainerAuth::Unavailable
}

/// Wire shape of the `get_container_auth_status` command:
/// `{ "status": "keychain" | "stored-token" | "shell-env" | "credentials-file" | "none" }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ContainerAuthStatus {
    Keychain,
    StoredToken,
    ShellEnv,
    CredentialsFile,
    None,
}

/// Which chain step is active right now (settings UI polling).
pub fn status() -> ContainerAuthStatus {
    match resolve() {
        ContainerAuth::Resolved { source, .. } => match source {
            AuthSource::Keychain => ContainerAuthStatus::Keychain,
            AuthSource::StoredToken => ContainerAuthStatus::StoredToken,
            AuthSource::ShellEnv => ContainerAuthStatus::ShellEnv,
            AuthSource::CredentialsFile => ContainerAuthStatus::CredentialsFile,
        },
        ContainerAuth::Unavailable => ContainerAuthStatus::None,
    }
}

/// Normalize a pasted token for storage: trimmed and non-empty, plus whether it
/// matches the expected shape (callers warn-but-accept). Errors never echo input.
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
    fn merge_auth_env_honors_process_env_and_prefers_shell() {
        let process = shell_env(&[
            ("ANTHROPIC_API_KEY", "proc-key"),
            ("ANTHROPIC_BASE_URL", "https://proc-proxy"),
        ]);
        let m = merge_auth_env(&process, None).unwrap();
        assert_eq!(m.get("ANTHROPIC_API_KEY").unwrap(), "proc-key");
        // Login-shell wins on collision; process-only vars still survive.
        let shell = shell_env(&[("ANTHROPIC_API_KEY", "shell-key")]);
        let m = merge_auth_env(&process, Some(&shell)).unwrap();
        assert_eq!(m.get("ANTHROPIC_API_KEY").unwrap(), "shell-key");
        assert_eq!(m.get("ANTHROPIC_BASE_URL").unwrap(), "https://proc-proxy");
    }

    #[test]
    fn merge_auth_env_none_when_no_auth_vars() {
        assert!(merge_auth_env(&HashMap::new(), None).is_none());
        let junk = shell_env(&[("PATH", "/usr/bin")]);
        assert!(merge_auth_env(&junk, Some(&junk)).is_none());
    }

    #[test]
    fn process_env_token_resolves_instead_of_aborting() {
        // A token only the process env carries must resolve, not abort the launch.
        let process = shell_env(&[("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat-proc")]);
        let merged = merge_auth_env(&process, None);
        let (env, source) = resolved(resolve_from(None, None, merged.as_ref(), false));
        assert_eq!(source, AuthSource::ShellEnv);
        assert_eq!(
            env,
            vec![(
                "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
                "sk-ant-oat-proc".to_string()
            )]
        );
    }

    #[test]
    fn keychain_beats_stored_shell_and_credentials_file() {
        let shell = shell_env(&[("ANTHROPIC_API_KEY", "sk-ant-api-key")]);
        let auth = resolve_from(
            Some("sk-ant-oat-keychain".into()),
            Some("sk-ant-oat-stored".into()),
            Some(&shell),
            true,
        );
        let (env, source) = resolved(auth);
        assert_eq!(source, AuthSource::Keychain);
        assert_eq!(
            env,
            vec![(
                "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
                "sk-ant-oat-keychain".to_string()
            )]
        );
    }

    #[test]
    fn proxy_config_rides_along_but_ambient_credentials_do_not() {
        // Forwarding the ambient key would let claude prefer it over the
        // Keychain login the chain resolved.
        let shell = shell_env(&[
            ("ANTHROPIC_API_KEY", "sk-ant-ambient-key"),
            ("ANTHROPIC_BASE_URL", "https://proxy.example.com"),
        ]);
        let (env, source) = resolved(resolve_from(
            Some("sk-ant-oat-keychain".into()),
            None,
            Some(&shell),
            false,
        ));
        assert_eq!(source, AuthSource::Keychain);
        let mut keys: Vec<_> = env.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["ANTHROPIC_BASE_URL", "CLAUDE_CODE_OAUTH_TOKEN"]);
        assert!(env
            .iter()
            .any(|(k, v)| k == "CLAUDE_CODE_OAUTH_TOKEN" && v == "sk-ant-oat-keychain"));
    }

    #[test]
    fn gateway_token_alone_resolves_via_shell_env() {
        // A gateway-only host still authenticates, matching seatbelt.
        let shell = shell_env(&[
            ("ANTHROPIC_AUTH_TOKEN", "gw-secret"),
            ("ANTHROPIC_BASE_URL", "https://gateway.example.com"),
        ]);
        let (env, source) = resolved(resolve_from(None, None, Some(&shell), false));
        assert_eq!(source, AuthSource::ShellEnv);
        let mut keys: Vec<_> = env.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"]);
    }

    #[test]
    fn endpoint_rides_along_but_ambient_gateway_token_does_not() {
        // The endpoint points the resolved login at its proxy; the ambient
        // gateway token would override that login, so it must not ride along.
        let shell = shell_env(&[
            ("ANTHROPIC_AUTH_TOKEN", "gw-secret"),
            ("ANTHROPIC_BASE_URL", "https://proxy.example.com"),
        ]);
        let (env, source) = resolved(resolve_from(
            Some("sk-ant-oat-keychain".into()),
            None,
            Some(&shell),
            false,
        ));
        assert_eq!(source, AuthSource::Keychain);
        let mut keys: Vec<_> = env.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["ANTHROPIC_BASE_URL", "CLAUDE_CODE_OAUTH_TOKEN"]);
    }

    #[test]
    fn usable_oauth_token_extracts_or_rejects() {
        assert_eq!(
            usable_oauth_token(Some(
                br#"{"claudeAiOauth":{"accessToken":"  sk-ant-oat-x \n","expiresAt":1893456000000}}"#
            )),
            Some("sk-ant-oat-x".to_string()),
            "trimmed token extracted from a usable blob"
        );
        // Placeholder, empty token, wrong shape, unparseable, absent — all reject.
        for blob in [
            &br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-x","expiresAt":0}}"#[..],
            &br#"{"claudeAiOauth":{"accessToken":"","expiresAt":1893456000000}}"#[..],
            &br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-x"}}"#[..],
            &br#"{"somethingElse":true}"#[..],
            &b"not json"[..],
        ] {
            assert_eq!(
                usable_oauth_token(Some(blob)),
                None,
                "must reject: {blob:?}"
            );
        }
        assert_eq!(usable_oauth_token(None), None);
    }

    #[test]
    fn stored_token_beats_shell_env_and_credentials_file() {
        let shell = shell_env(&[("ANTHROPIC_API_KEY", "sk-ant-api-key")]);
        let auth = resolve_from(None, Some("sk-ant-oat-stored".into()), Some(&shell), true);
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
        let (env, source) = resolved(resolve_from(None, None, Some(&shell), true));
        assert_eq!(source, AuthSource::ShellEnv);
        let mut keys: Vec<_> = env.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"]);
    }

    #[test]
    fn shell_values_are_trimmed_before_forwarding() {
        // The hit check trims, so the forwarded value must trim too or auth
        // fails in-container.
        let shell = shell_env(&[("ANTHROPIC_API_KEY", "  sk-ant-api-key\n")]);
        let (env, source) = resolved(resolve_from(None, None, Some(&shell), false));
        assert_eq!(source, AuthSource::ShellEnv);
        assert_eq!(
            env,
            vec![(
                "ANTHROPIC_API_KEY".to_string(),
                "sk-ant-api-key".to_string()
            )]
        );
    }

    #[test]
    fn proxy_vars_alone_are_not_a_hit_but_ride_along() {
        // BASE_URL alone can't authenticate, so resolution falls through.
        let shell = shell_env(&[("ANTHROPIC_BASE_URL", "https://proxy.example.com")]);
        let (env, source) = resolved(resolve_from(None, None, Some(&shell), true));
        assert_eq!(source, AuthSource::CredentialsFile);
        assert_eq!(
            env,
            vec![(
                "ANTHROPIC_BASE_URL".to_string(),
                "https://proxy.example.com".to_string()
            )]
        );
    }

    #[test]
    fn blank_shell_values_are_ignored() {
        let shell = shell_env(&[("ANTHROPIC_API_KEY", "  ")]);
        assert!(matches!(
            resolve_from(None, None, Some(&shell), false),
            ContainerAuth::Unavailable
        ));
    }

    #[test]
    fn credentials_config_dir_honors_claude_config_dir() {
        let home = Path::new("/Users/u");
        // Unset → the default `~/.claude`.
        assert_eq!(
            credentials_config_dir(None, Some(home)),
            Some(PathBuf::from("/Users/u/.claude"))
        );
        // A custom `CLAUDE_CONFIG_DIR` is used verbatim.
        assert_eq!(
            credentials_config_dir(Some(OsStr::new("/cfg/eve")), Some(home)),
            Some(PathBuf::from("/cfg/eve"))
        );
        // No env and no home → nothing to check.
        assert_eq!(credentials_config_dir(None, None), None);
    }

    #[test]
    fn credentials_file_resolves_with_empty_env() {
        // The ~/.claude mount carries the file; nothing to inject.
        let (env, source) = resolved(resolve_from(None, None, None, true));
        assert_eq!(source, AuthSource::CredentialsFile);
        assert!(env.is_empty());
    }

    #[test]
    fn credentials_file_usable_accepts_a_real_oauth_token() {
        assert!(credentials_file_usable(Some(
            br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-x","refreshToken":"r","expiresAt":1893456000000}}"#
        )));
        // Expired-but-nonzero stays usable: the container refreshes via the mount.
        assert!(credentials_file_usable(Some(
            br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-x","refreshToken":"r","expiresAt":1}}"#
        )));
    }

    #[test]
    fn credentials_file_usable_rejects_stale_and_malformed() {
        // A macOS Keychain login leaves a placeholder on disk.
        assert!(!credentials_file_usable(Some(
            br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-x","refreshToken":"r","expiresAt":0}}"#
        )));
        // Empty access token, missing expiry, wrong shape, unparseable, absent.
        assert!(!credentials_file_usable(Some(
            br#"{"claudeAiOauth":{"accessToken":"","expiresAt":1893456000000}}"#
        )));
        assert!(!credentials_file_usable(Some(
            br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-x"}}"#
        )));
        assert!(!credentials_file_usable(Some(br#"{"somethingElse":true}"#)));
        assert!(!credentials_file_usable(Some(b"not json")));
        assert!(!credentials_file_usable(None));
    }

    #[test]
    fn nothing_resolves_to_unavailable() {
        assert!(matches!(
            resolve_from(None, None, None, false),
            ContainerAuth::Unavailable
        ));
        assert!(matches!(
            resolve_from(None, None, Some(&shell_env(&[("PATH", "/usr/bin")])), false),
            ContainerAuth::Unavailable
        ));
    }

    #[test]
    fn debug_output_redacts_token_values() {
        let auth = resolve_from(None, Some("sk-ant-oat-SECRET-VALUE".into()), None, false);
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
            (ContainerAuthStatus::Keychain, "keychain"),
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

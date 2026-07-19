//! Anthropic auth for containerized agents.
//!
//! Host claude logins usually live in the macOS Keychain, which doesn't exist
//! inside a container, and Fletch itself injects no credentials — so docker
//! agents need auth resolved explicitly. [`resolve`] walks a first-hit-wins
//! chain and returns the env vars to set on the *docker CLI process*; the
//! launch path forwards them with bare `-e VAR` flags, so token values
//! never appear in argv (invariant 3). They must never appear in logs either:
//! [`ContainerAuth`] redacts values in its `Debug` output, and nothing in this
//! module traces a token.
//!
//! The chain (first hit wins), re-evaluated on every spawn:
//! 1. The **macOS Keychain** login (`security find-generic-password -s
//!    "Claude Code-credentials"`) → its `claudeAiOauth.accessToken` forwarded as
//!    `CLAUDE_CODE_OAUTH_TOKEN`. This is where an interactive `claude` login
//!    stores the live credential on macOS, so reading it fresh each spawn tracks
//!    the *currently authenticated account* — the same one a seatbelt agent
//!    would use — with no pasting and no staleness when the user switches
//!    accounts. Keychain-primary: it sits ahead of the stored token so a
//!    re-login (e.g. after hitting a rate limit) takes effect immediately. Only
//!    a usable token counts (see [`usable_oauth_token`]); no Keychain / no login
//!    (Linux, CI) falls through. See [`keychain_token`].
//! 2. A `claude setup-token` value captured into the app's secret store
//!    ([`TOKEN_SETTING`], auto-populated by [`super::setup_token`]) →
//!    `CLAUDE_CODE_OAUTH_TOKEN`. The fallback for hosts without a readable
//!    Keychain login.
//! 3. The app's process env or the login-shell probe exports a credential —
//!    `ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN`, or `ANTHROPIC_AUTH_TOKEN`
//!    (a custom-gateway bearer) → forward it plus `ANTHROPIC_BASE_URL` (the
//!    gateway/proxy endpoint). `ANTHROPIC_BASE_URL` alone is not a credential.
//!    This step sits below Keychain/stored, so an ambient credential here never
//!    overrides a resolved login — but it means a gateway-only host (no
//!    Keychain, no stored token) authenticates on docker just as it does under
//!    seatbelt. Both sources are consulted (login-shell wins on collision) so a
//!    token in the launching terminal's env works even when the `/bin/zsh -lc`
//!    probe can't see it — see [`merge_auth_env`].
//! 4. `<home>/.claude/.credentials.json` holds a usable OAuth token (non-empty
//!    access token, non-placeholder `expiresAt`) → nothing to inject: the
//!    `~/.claude` bind mount carries it, and refresh writes land on the host. A
//!    stale placeholder (macOS Keychain logins leave `"expiresAt": 0` on disk)
//!    does *not* count — see [`credentials_file_usable`].
//! 5. Nothing → [`ContainerAuth::Unavailable`]; the spawn path fails fast and
//!    the UI shows the "Connect Claude for containers" call-to-action.
//!
//! Seatbelt agents never see any of this — they keep the user's own login.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;

use crate::bin_resolve;

/// `crate::secrets` key holding the user-pasted `claude setup-token` value —
/// the OS keychain on release macOS builds, the same posture as
/// `github::TOKEN_SETTING`.
pub const TOKEN_SETTING: &str = "claude_container_token";

/// Env var claude reads a setup-token (OAuth) credential from.
const OAUTH_TOKEN_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// macOS Keychain generic-password service name Claude Code stores its login
/// credential under. The password payload is the same `{"claudeAiOauth":{…}}`
/// JSON as `~/.claude/.credentials.json`, so [`usable_oauth_token`] parses both.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Shell vars that constitute a chain hit on their own. `ANTHROPIC_AUTH_TOKEN`
/// (a custom-gateway bearer credential) counts too, so a host that authenticates
/// *only* via a gateway isn't refused on docker while it works under seatbelt —
/// but it sits in the `ShellEnv` step (below Keychain/stored), so it never
/// overrides a resolved login, matching how `ANTHROPIC_API_KEY` is treated.
const SHELL_KEY_VARS: [&str; 3] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
];

/// Everything forwarded from the login shell once one of [`SHELL_KEY_VARS`]
/// is present; `ANTHROPIC_BASE_URL` is the endpoint for the proxy/gateway.
const SHELL_AUTH_VARS: [&str; 4] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
];

/// Endpoint var forwarded alongside a credential resolved from a *higher* chain
/// step (Keychain/stored/credentials-file). It's an endpoint, not a credential —
/// it points the resolved login at a custom `ANTHROPIC_BASE_URL` (e.g. a proxy
/// in front of Anthropic that accepts that login) — so it always rides along.
/// `ANTHROPIC_AUTH_TOKEN` is a credential, not an endpoint, so it is NOT here: an
/// ambient one must never ride along and override the resolved login (it's only
/// forwarded when the shell env is itself the resolved credential — the
/// `ShellEnv` step, via [`SHELL_AUTH_VARS`]).
const PROXY_RIDE_ALONG: [&str; 1] = ["ANTHROPIC_BASE_URL"];

/// Expected prefix of a `claude setup-token` credential. Other shapes are
/// accepted with a warning — the format isn't a contract we own.
const SETUP_TOKEN_PREFIX: &str = "sk-ant-oat";

/// The stored token, cached in-process. Seeded from the DB at startup and
/// updated by the `set_container_auth_token` / `clear_container_auth_token`
/// commands, so [`resolve`] — called deep in spawn paths with no DB handle —
/// never touches the DB. Same pattern as `github::set_token`.
static STORED_TOKEN: RwLock<Option<String>> = RwLock::new(None);

/// True once an explicit [`set_stored_token`] (paste/clear command) has run.
/// Guards [`seed_stored_token`]: the startup seed retries in the background
/// while the keychain is locked, and a delayed retry must never overwrite
/// newer user action with the stale value it read.
static SEALED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Replace the in-process stored token. Callers that change the *persisted*
/// token write the DB and then call this; blank counts as none. Seals the
/// mirror against any still-pending startup seed.
pub fn set_stored_token(token: Option<String>) {
    let mut w = STORED_TOKEN.write();
    SEALED.store(true, std::sync::atomic::Ordering::SeqCst);
    *w = sanitize(token);
}

/// Startup-seed variant of [`set_stored_token`]: applies only while no
/// explicit set has run. The seal is checked under the mirror's write lock,
/// so a racing paste/clear either lands after this (and overwrites the seed)
/// or before it (and the seed no-ops) — the fresher value wins in both
/// orders.
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
    /// The live macOS Keychain login (`claude`'s own credential), read fresh
    /// each spawn and forwarded — tracks the currently authenticated account.
    Keychain,
    /// The pasted/captured setup-token from settings.
    StoredToken,
    /// Auth vars from the app's process env or the user's login shell.
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

/// The config dir whose `.credentials.json` claude reads: the explicit
/// `CLAUDE_CONFIG_DIR` if set, else `~/.claude`. Pure over its inputs so the
/// resolution is unit-testable without touching process env or `$HOME`.
fn credentials_config_dir(config_dir_env: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    config_dir_env
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".claude")))
}

/// Walk the auth chain (first hit wins). Called by the docker launch path at
/// spawn time and, via [`status`], by the settings UI. May block on the first
/// call: the login-shell env is loaded (a shell runs) if nothing earlier
/// populated `bin_resolve`'s cache.
pub fn resolve() -> ContainerAuth {
    let keychain = keychain_token();
    // Check the config dir claude will actually read from — the explicit
    // `CLAUDE_CONFIG_DIR` if set, else `~/.claude` — which is also the dir the
    // engine mounts (see `nondefault_claude_config_dir`). Hardcoding `~/.claude`
    // would refuse a container whose only credential is a `.credentials.json`
    // living in a custom config dir.
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

/// The live host login token from the macOS Keychain, or `None` when there's no
/// readable/usable login (Keychain locked or empty, non-macOS host). Shells out
/// to `security find-generic-password -s <service> -w`, which prints the stored
/// password — the `{"claudeAiOauth":{…}}` JSON — to stdout. Read fresh on every
/// [`resolve`] so a `claude` re-login is reflected in the very next spawn. The
/// process may surface a one-time Keychain access prompt the first time a new
/// Fletch build reads the item; "Always Allow" persists it.
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
/// into one auth view for the env chain step (login-shell wins on collision).
///
/// The docker CLI child inherits the app's process env, so a token exported in
/// the terminal that launched Fletch — or on a bash-only host, where the
/// `/bin/zsh -lc` probe in [`bin_resolve::login_shell_env`] can't see it — was
/// always forwarded into the container by the bare `-e VAR` flags. Consulting
/// only the login-shell probe here would make [`resolve`] report `Unavailable`
/// for exactly that setup, and the fail-fast launch path would then abort a
/// container that would otherwise have authenticated fine. Reading the process
/// env too keeps that path working. `None` when neither source carries an auth
/// var.
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

/// Whether `~/.claude/.credentials.json` carries an OAuth credential the
/// container can actually authenticate with. Mere existence is *not* enough:
/// on a macOS host the live token lives in the Keychain and the on-disk file is
/// commonly a stale placeholder (`"expiresAt": 0`) — counting that as a hit
/// boots the container straight into "Not logged in · Please run /login", which
/// it can't recover from (no interactive login inside the sandbox).
///
/// A file counts only when it holds a non-empty `claudeAiOauth.accessToken` and
/// a positive `expiresAt`. We deliberately do *not* require `expiresAt` to be
/// in the future: an expired-but-refreshable token is the documented refresh
/// flow (the container refreshes and the write lands on the mounted file), so
/// rejecting it would break working Linux-host setups. The `expiresAt <= 0`
/// (or missing/empty-token) placeholder is the only shape we reject.
fn credentials_file_usable(contents: Option<&[u8]>) -> bool {
    usable_oauth_token(contents).is_some()
}

/// Extract a container-usable OAuth access token from a credentials JSON blob —
/// the `~/.claude/.credentials.json` file *or* the macOS Keychain password,
/// which share the `{"claudeAiOauth":{accessToken,expiresAt}}` shape. Returns
/// the trimmed access token only when it's non-empty and `expiresAt > 0`, the
/// same usability bar [`credentials_file_usable`] documents: the `expiresAt <= 0`
/// (or empty-token / wrong-shape / unparseable) placeholder is rejected, while
/// an expired-but-refreshable positive `expiresAt` is accepted.
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

/// The chain itself, pure over its inputs so tests can exercise the ordering
/// without touching process globals or the filesystem.
fn resolve_from(
    keychain: Option<String>,
    stored: Option<String>,
    shell_env: Option<&HashMap<String, String>>,
    credentials_file: bool,
) -> ContainerAuth {
    // Append the endpoint ride-along (see [`PROXY_RIDE_ALONG`]) to a credential
    // the chain picked from a *higher* step than the shell env, so a custom
    // `ANTHROPIC_BASE_URL` still points the resolved login at its proxy. Only the
    // endpoint rides along — never a credential var — so an ambient
    // `ANTHROPIC_AUTH_TOKEN`/`ANTHROPIC_API_KEY` can't override the resolved
    // login. (The endpoint is forwarded regardless of whether a gateway token is
    // also set: the user set `BASE_URL` deliberately, often for network egress,
    // so honoring it beats silently falling back to the default endpoint.)
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

/// Wire shape of the `get_container_auth_status` command — [`resolve`]'s
/// outcome for the settings status row. Serializes like `DockerAvailability`:
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
    fn merge_auth_env_honors_process_env_and_prefers_shell() {
        let process = shell_env(&[
            ("ANTHROPIC_API_KEY", "proc-key"),
            ("ANTHROPIC_BASE_URL", "https://proc-proxy"),
        ]);
        // Process env alone is honored.
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
        // Non-auth vars are ignored, so a shell with only PATH is not a source.
        let junk = shell_env(&[("PATH", "/usr/bin")]);
        assert!(merge_auth_env(&junk, Some(&junk)).is_none());
    }

    #[test]
    fn process_env_token_resolves_instead_of_aborting() {
        // The regression: a token in the app's process env but not the
        // login-shell probe must resolve (not fall through to Unavailable and
        // abort the launch). merge_auth_env feeds it into the env chain step.
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
        // Keychain-primary: the live host login wins over a (possibly stale)
        // stored setup-token, shell env, and the mounted credentials file — so a
        // `claude` re-login is reflected on the very next spawn.
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
        // Keychain wins, but the shell also exports a proxy base URL and a
        // competing API key. The base URL rides along (endpoint config); the
        // API key must NOT — forwarding it would let claude prefer it over the
        // Keychain login the chain actually resolved.
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
        // A host whose only credential is a custom-gateway bearer (+ its
        // endpoint) — no Keychain, no stored token, no credentials file — still
        // authenticates on docker, matching seatbelt. It resolves in the
        // shell-env step, forwarding both the token and the endpoint.
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
        // Keychain wins. A custom BASE_URL still rides along — it's an endpoint,
        // so it just points the resolved OAuth login at its proxy (honoring an
        // explicit endpoint beats falling back to the default). But the ambient
        // gateway AUTH_TOKEN must NOT ride along — forwarding it would let it
        // override the Keychain login.
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
        // Same shape for the file and the Keychain password: extract the token
        // when usable, reject the macOS placeholder and malformed blobs.
        assert_eq!(
            usable_oauth_token(Some(
                br#"{"claudeAiOauth":{"accessToken":"  sk-ant-oat-x \n","expiresAt":1893456000000}}"#
            )),
            Some("sk-ant-oat-x".to_string()),
            "trimmed token extracted from a usable blob"
        );
        // expiresAt:0 placeholder (Keychain login on disk), empty token, wrong
        // shape, unparseable, absent — all reject.
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
        // A profile that exports a padded credential must reach the container
        // trimmed — the status check trims when deciding it's a hit, so the
        // forwarded value has to match or Claude auth fails in-container.
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
        // BASE_URL without a key var can't authenticate, so it doesn't make the
        // shell env a credential hit — resolution falls through to the
        // credentials file. The proxy endpoint still rides along so the
        // container honors it.
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
        // A custom `CLAUDE_CONFIG_DIR` is used verbatim — this is the dir claude
        // reads its `.credentials.json` from and the one the engine mounts.
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
        // Expired-but-nonzero is still usable: the container can refresh via the
        // mounted file (the documented refresh flow), so we must not reject it.
        assert!(credentials_file_usable(Some(
            br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-x","refreshToken":"r","expiresAt":1}}"#
        )));
    }

    #[test]
    fn credentials_file_usable_rejects_stale_and_malformed() {
        // The reported macOS bug: a Keychain login leaves a placeholder on disk.
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

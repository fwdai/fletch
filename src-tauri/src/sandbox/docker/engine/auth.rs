//! Per-provider container auth: folding the resolved credential(s) into the
//! docker CLI's process env (from which [`super::run_args::run_args`]' bare
//! `-e NAME` flags forward them — invariant 3, values never touch argv) and the
//! fail-fast messages when a provider has no usable credential.

use std::path::Path;

use crate::error::{Error, Result};
use crate::sandbox::docker::auth::ContainerAuth;

/// Launch-blocking message when the container auth chain resolves nothing.
/// Kept as one stable, matchable string the frontend keys its Settings
/// call-to-action on; the wording tells the user exactly what to do.
pub(super) const NO_CONTAINER_AUTH_MSG: &str = "No Anthropic credentials for containers — open Settings → General → Sandbox and connect Claude for containers (claude setup-token).";

/// Fold the D1 auth-chain outcome ([`resolve`]) into the docker CLI's
/// process env, from which [`run_args`]' bare `-e NAME` flags forward it into
/// the container (invariant 3 — values never touch argv). An [`AuthSource`] is
/// logged (the enum variant only, never a token value); [`ContainerAuth`]'s
/// `Debug` redacts values so even `?source` cannot leak one. When the chain
/// yields nothing the launch fails fast with [`NO_CONTAINER_AUTH_MSG`].
///
/// [`resolve`]: crate::sandbox::docker::auth::resolve
/// [`run_args`]: super::run_args::run_args
/// [`AuthSource`]: crate::sandbox::docker::auth::AuthSource
pub(super) fn apply_container_auth(
    env: &mut Vec<(String, String)>,
    auth: ContainerAuth,
) -> Result<()> {
    match auth {
        ContainerAuth::Resolved {
            env: auth_env,
            source,
        } => {
            tracing::info!(target: "fletch::docker", ?source, "container auth resolved");
            env.extend(auth_env);
            Ok(())
        }
        ContainerAuth::Unavailable => Err(Error::Other(NO_CONTAINER_AUTH_MSG.to_string())),
    }
}

/// Launch-blocking message when codex has no usable credential: no
/// `auth.json` in its config dir and `OPENAI_API_KEY` unset. Mirrors
/// [`NO_CONTAINER_AUTH_MSG`]'s fail-fast: an unauthenticated container boots
/// straight into a login prompt it can't answer inside the sandbox.
pub(super) const NO_CODEX_AUTH_MSG: &str =
    "No Codex credentials for containers — sign in with `codex` on the host (writes ~/.codex/auth.json) or set OPENAI_API_KEY.";

/// Launch-blocking message when opencode has no usable credential: no accounts DB
/// / auth.json on its data-dir mount and no known provider API key set. Same
/// fail-fast rationale as [`NO_CODEX_AUTH_MSG`].
pub(super) const NO_OPENCODE_AUTH_MSG: &str =
    "No OpenCode credentials for containers — sign in with `opencode auth login` on the host or set a provider API key (e.g. ANTHROPIC_API_KEY or OPENAI_API_KEY).";

/// Launch-blocking message when pi has no usable credential: no
/// `~/.pi/agent/auth.json` on its mount and no known provider API key set.
pub(super) const NO_PI_AUTH_MSG: &str =
    "No Pi credentials for containers — sign in with `pi` on the host (writes ~/.pi/agent/auth.json) or set a provider API key (e.g. ANTHROPIC_API_KEY or OPENAI_API_KEY).";

/// Launch-blocking message when cursor has no usable credential. Unlike the other
/// providers there is no mount-based fallback: `cursor-agent login` stores its
/// access/refresh tokens in the host OS keychain (macOS "Cursor Safe Storage"),
/// which a Linux container can't read, and `~/.cursor` carries only identity
/// metadata — not a bearer token. So `CURSOR_API_KEY` is the sole container
/// credential; fail fast (before touching the filesystem) when it's unset.
pub(super) const NO_CURSOR_AUTH_MSG: &str =
    "No Cursor credentials for containers — set CURSOR_API_KEY (create one at cursor.com/dashboard). `cursor-agent login` stores its token in the host keychain, which containers can't read.";

/// Provider API-key env vars the multi-provider CLIs (opencode, pi) read to
/// authenticate. Whichever are set in the app's process env are forwarded by bare
/// `-e NAME` (invariant 3) so the in-container CLI can use them, and a set key
/// satisfies the auth requirement on its own. Curated to the mainstream providers
/// both CLIs honor (verified against each CLI's binary) — not exhaustive, but
/// enough that a user with any common key set can launch. Codex is excluded: it's
/// single-provider (OpenAI) and resolved separately.
const MULTI_PROVIDER_API_KEY_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "GEMINI_API_KEY",
    "GROQ_API_KEY",
    "XAI_API_KEY",
    "DEEPSEEK_API_KEY",
    "MISTRAL_API_KEY",
];

/// Fold codex's container auth into the docker CLI's process env, then make
/// sure the config dir exists so the read-write bind has a host source. Codex's
/// primary credential is the mounted `~/.codex/auth.json` (the read-write mount
/// carries it and token refresh persists to the host); an `OPENAI_API_KEY` in
/// the app's process env is forwarded when set (by bare `-e`, so its value never
/// touches argv — invariant 3). Either alone suffices: a key-only user may
/// never have run codex on the host, so the dir is created rather than
/// required — mounting a fresh dir keeps session rollouts landing where
/// `find_codex_rollouts` reads them. Fails the launch when neither credential
/// is present (before touching the filesystem), so an unauthenticated
/// container never boots into an unanswerable login prompt.
///
/// Unlike claude, no ANTHROPIC_*/CLAUDE_* var is injected: codex authenticates
/// against OpenAI, and forwarding those would be dead weight at best.
pub(super) fn prepare_codex_launch(
    env: &mut Vec<(String, String)>,
    config_dir: &Path,
    api_key: Option<&str>,
) -> Result<()> {
    let auth_file = config_dir.join("auth.json").is_file();
    let resolved = codex_auth_env(api_key, auth_file)?;
    // Booleans only — never a token value.
    tracing::info!(
        target: "fletch::docker",
        auth_file,
        api_key = !resolved.is_empty(),
        "codex container auth resolved"
    );
    env.extend(resolved);
    std::fs::create_dir_all(config_dir).map_err(|e| {
        Error::Other(format!(
            "Couldn't create Codex config dir {}: {e}",
            config_dir.display()
        ))
    })?;
    Ok(())
}

/// Pure core of [`prepare_codex_launch`]: the auth env to forward given the process
/// `OPENAI_API_KEY` (if any) and whether `auth.json` exists on the mount. A
/// non-blank key is forwarded (trimmed); the mounted `auth.json` carries auth on
/// its own with nothing to inject. Neither present → the launch-blocking error.
pub(super) fn codex_auth_env(
    api_key: Option<&str>,
    auth_file: bool,
) -> Result<Vec<(String, String)>> {
    let api_key = api_key.map(str::trim).filter(|k| !k.is_empty());
    if let Some(key) = api_key {
        return Ok(vec![("OPENAI_API_KEY".to_string(), key.to_string())]);
    }
    if auth_file {
        return Ok(Vec::new());
    }
    Err(Error::Other(NO_CODEX_AUTH_MSG.to_string()))
}

/// The subset of [`MULTI_PROVIDER_API_KEY_ENV`] present and non-blank via
/// `lookup` (a var name → value resolver, `std::env::var` in production, a fixture
/// in tests), each as a `(name, value)` to forward. Order follows the constant so
/// forwarding is deterministic.
pub(super) fn present_api_keys(lookup: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    MULTI_PROVIDER_API_KEY_ENV
        .iter()
        .filter_map(|&name| {
            let value = lookup(name)?;
            let value = value.trim();
            (!value.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

/// Shared auth rule for the multi-provider CLIs (opencode, pi): a forwarded
/// provider key OR a credential carried on the read-write mount suffices; neither
/// present → the caller's launch-blocking message. Returns the keys to forward
/// (empty when the mount carries the login and no key is set), mirroring
/// [`codex_auth_env`]'s shape.
pub(super) fn multi_provider_auth_env(
    api_keys: Vec<(String, String)>,
    credential_on_mount: bool,
    no_auth_msg: &str,
) -> Result<Vec<(String, String)>> {
    if !api_keys.is_empty() || credential_on_mount {
        Ok(api_keys)
    } else {
        Err(Error::Other(no_auth_msg.to_string()))
    }
}

/// Fold opencode's container auth into the docker CLI's process env, then make
/// sure the data dir exists so the read-write bind has a host source. OpenCode's
/// login lives in its data-dir mount (the accounts DB `opencode.db`, or a legacy
/// `auth.json`); a provider API key in the app's env is forwarded when set. Either
/// alone suffices, so — like codex — a key-only user who never ran opencode gets
/// the dir created rather than required. Neither present → fail the launch before
/// touching the filesystem. Auth values ride the process env and forward by bare
/// `-e` (invariant 3); only booleans are logged.
pub(super) fn prepare_opencode_launch(
    env: &mut Vec<(String, String)>,
    data_dir: &Path,
    api_keys: Vec<(String, String)>,
) -> Result<()> {
    let auth_file = data_dir.join("auth.json").is_file();
    let auth_db = data_dir.join("opencode.db").is_file();
    let has_keys = !api_keys.is_empty();
    let resolved = multi_provider_auth_env(api_keys, auth_file || auth_db, NO_OPENCODE_AUTH_MSG)?;
    tracing::info!(
        target: "fletch::docker",
        auth_file,
        auth_db,
        api_keys = has_keys,
        "opencode container auth resolved"
    );
    env.extend(resolved);
    std::fs::create_dir_all(data_dir).map_err(|e| {
        Error::Other(format!(
            "Couldn't create OpenCode data dir {}: {e}",
            data_dir.display()
        ))
    })?;
    Ok(())
}

/// Fold pi's container auth into the docker CLI's process env, then make sure
/// `~/.pi` exists so the read-write bind has a host source. Pi's login lives in
/// `~/.pi/agent/auth.json` on that mount; a provider API key in the app's env is
/// forwarded when set. Either alone suffices, so a key-only user who never ran pi
/// gets `~/.pi` created rather than required. Neither present → fail the launch
/// before touching the filesystem. Only booleans are logged.
pub(super) fn prepare_pi_launch(
    env: &mut Vec<(String, String)>,
    data_dir: &Path,
    api_keys: Vec<(String, String)>,
) -> Result<()> {
    let auth_file = data_dir.join("agent/auth.json").is_file();
    let has_keys = !api_keys.is_empty();
    let resolved = multi_provider_auth_env(api_keys, auth_file, NO_PI_AUTH_MSG)?;
    tracing::info!(
        target: "fletch::docker",
        auth_file,
        api_keys = has_keys,
        "pi container auth resolved"
    );
    env.extend(resolved);
    std::fs::create_dir_all(data_dir).map_err(|e| {
        Error::Other(format!(
            "Couldn't create Pi data dir {}: {e}",
            data_dir.display()
        ))
    })?;
    Ok(())
}

/// Fold cursor's container auth into the docker CLI's process env, then make sure
/// `~/.cursor` exists so the read-write bind has a host source. Cursor is a
/// single-provider CLI, so — like codex — no cross-provider key set applies;
/// unlike every other provider, though, its credential can't ride the mount:
/// `cursor-agent login` writes its tokens to the host OS keychain (see
/// [`NO_CURSOR_AUTH_MSG`]), so `CURSOR_API_KEY` (forwarded by bare `-e` — invariant
/// 3) is the only container credential. The `~/.cursor` mount still matters: it's
/// where cursor writes session transcripts (`agent::cursor_locate` reads them at
/// the identical host path), so the dir is created for the bind even though it
/// carries no auth. Fails the launch when `CURSOR_API_KEY` is unset, before
/// touching the filesystem. Only a boolean is logged, never the key.
pub(super) fn prepare_cursor_launch(
    env: &mut Vec<(String, String)>,
    config_dir: &Path,
    api_key: Option<&str>,
) -> Result<()> {
    let resolved = cursor_auth_env(api_key)?;
    tracing::info!(
        target: "fletch::docker",
        api_key = !resolved.is_empty(),
        "cursor container auth resolved"
    );
    env.extend(resolved);
    std::fs::create_dir_all(config_dir).map_err(|e| {
        Error::Other(format!(
            "Couldn't create Cursor config dir {}: {e}",
            config_dir.display()
        ))
    })?;
    Ok(())
}

/// Pure core of [`prepare_cursor_launch`]: the auth env to forward given the
/// process `CURSOR_API_KEY`. A non-blank key is forwarded (trimmed); anything
/// else — unset or blank — is the launch-blocking error, because cursor's login
/// token lives in the host keychain and can't reach the container by any other
/// path (see [`NO_CURSOR_AUTH_MSG`]).
pub(super) fn cursor_auth_env(api_key: Option<&str>) -> Result<Vec<(String, String)>> {
    match api_key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => Ok(vec![("CURSOR_API_KEY".to_string(), key.to_string())]),
        None => Err(Error::Other(NO_CURSOR_AUTH_MSG.to_string())),
    }
}

//! Per-provider container auth: folding the resolved credential(s) into the
//! container CLI's process env, from which [`super::run_args::run_args`]' bare
//! `-e NAME` flags forward them (invariant 3 — values never touch argv), plus
//! the fail-fast messages when a provider has no usable credential.

use std::path::Path;

use super::auth::ContainerAuth;
use crate::error::{Error, Result};

/// Launch-blocking message when the container auth chain resolves nothing. One
/// stable string — the frontend keys its Settings call-to-action on it.
pub(crate) const NO_CONTAINER_AUTH_MSG: &str = "No Anthropic credentials for containers — open Settings → General → Sandbox and connect Claude for containers (claude setup-token).";

/// Fold the claude auth-chain outcome ([`resolve`]) into the container CLI's
/// process env. Only the [`AuthSource`] variant is logged, never a value.
/// Nothing resolved → fail fast with [`NO_CONTAINER_AUTH_MSG`].
///
/// [`resolve`]: crate::sandbox::container::auth::resolve
/// [`AuthSource`]: crate::sandbox::container::auth::AuthSource
pub(crate) fn apply_container_auth(
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

/// Launch-blocking message when codex has no credential: no `auth.json` in its
/// config dir, no `OPENAI_API_KEY`. Fail fast like [`NO_CONTAINER_AUTH_MSG`] —
/// an unauthenticated container boots into a login prompt it can't answer.
pub(crate) const NO_CODEX_AUTH_MSG: &str =
    "No Codex credentials for containers — sign in with `codex` on the host (writes ~/.codex/auth.json) or set OPENAI_API_KEY.";

/// Launch-blocking message when opencode has no credential: no accounts DB /
/// auth.json on its data-dir mount and no provider API key set.
pub(crate) const NO_OPENCODE_AUTH_MSG: &str =
    "No OpenCode credentials for containers — sign in with `opencode auth login` on the host or set a provider API key (e.g. ANTHROPIC_API_KEY or OPENAI_API_KEY).";

/// Launch-blocking message when pi has no credential: no
/// `~/.pi/agent/auth.json` on its mount and no provider API key set.
pub(crate) const NO_PI_AUTH_MSG: &str =
    "No Pi credentials for containers — sign in with `pi` on the host (writes ~/.pi/agent/auth.json) or set a provider API key (e.g. ANTHROPIC_API_KEY or OPENAI_API_KEY).";

/// Launch-blocking message when cursor has no credential. There is no
/// mount-based fallback: `cursor-agent login` stores its tokens in the host OS
/// keychain, which a Linux container can't read, and `~/.cursor` carries only
/// identity metadata — so `CURSOR_API_KEY` is the sole container credential.
pub(crate) const NO_CURSOR_AUTH_MSG: &str =
    "No Cursor credentials for containers — set CURSOR_API_KEY (create one at cursor.com/dashboard). `cursor-agent login` stores its token in the host keychain, which containers can't read.";

/// Provider API-key env vars the multi-provider CLIs (opencode, pi) read to
/// authenticate; any one set in the app's process env satisfies auth on its own.
/// Curated to the mainstream providers both CLIs honor, not exhaustive. Codex is
/// excluded — it's single-provider and resolved separately.
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

/// Fold codex's container auth into the container CLI's process env, then make
/// sure the config dir exists so the read-write bind has a host source. Either
/// the mounted `auth.json` or an `OPENAI_API_KEY` suffices, so a key-only user
/// who never ran codex gets the dir created rather than required; neither
/// present fails the launch before any filesystem work.
pub(crate) fn prepare_codex_launch(
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

/// Pure core of [`prepare_codex_launch`]: a non-blank key is forwarded trimmed,
/// a mounted `auth.json` needs nothing injected, neither is the blocking error.
pub(crate) fn codex_auth_env(
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
/// `lookup`, in constant order so forwarding is deterministic.
pub(crate) fn present_api_keys(lookup: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
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
/// provider key OR a credential on the read-write mount suffices; neither is the
/// caller's launch-blocking message.
pub(crate) fn multi_provider_auth_env(
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

/// Fold opencode's container auth into the container CLI's process env, then
/// make sure the data dir exists so the read-write bind has a host source. Its
/// login lives on that mount (accounts DB `opencode.db`, or a legacy
/// `auth.json`); a provider API key does just as well, so — like codex — the dir
/// is created rather than required. Only booleans are logged, never a value.
pub(crate) fn prepare_opencode_launch(
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

/// Fold pi's container auth into the container CLI's process env, then make sure
/// `~/.pi` exists so the read-write bind has a host source. Its login lives in
/// `agent/auth.json` on that mount; a provider API key does just as well, so the
/// dir is created rather than required. Only booleans are logged.
pub(crate) fn prepare_pi_launch(
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

/// Fold cursor's container auth into the container CLI's process env, then make
/// sure `~/.cursor` exists so the read-write bind has a host source. Only
/// `CURSOR_API_KEY` can authenticate (see [`NO_CURSOR_AUTH_MSG`]); the mount is
/// still created because cursor writes session transcripts there, at the
/// identical host path `agent::cursor_locate` reads.
pub(crate) fn prepare_cursor_launch(
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

/// Pure core of [`prepare_cursor_launch`]: a non-blank `CURSOR_API_KEY` is
/// forwarded trimmed, unset or blank is the launch-blocking error.
pub(crate) fn cursor_auth_env(api_key: Option<&str>) -> Result<Vec<(String, String)>> {
    match api_key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => Ok(vec![("CURSOR_API_KEY".to_string(), key.to_string())]),
        None => Err(Error::Other(NO_CURSOR_AUTH_MSG.to_string())),
    }
}

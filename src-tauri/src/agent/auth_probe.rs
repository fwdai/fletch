//! Per-provider "is this CLI signed in?" probing for Settings › Providers.
//!
//! [`super::probe`] answers whether a provider's CLI is *installed*; this module
//! answers the next question — has the user actually logged it in? Nothing tells
//! them otherwise today, so an installed-but-signed-out CLI just fails at spawn
//! time.
//!
//! Every check is derived from **structure only**: a file's existence and
//! non-emptiness, a JSON blob's shape, or a Keychain item's presence. Keychain
//! lookups all go through [`crate::keychain::item_present`], which never passes
//! `-w` — the password is never read and macOS never prompts, which matters
//! because the providers pane re-runs these probes on a timer while it is open.
//! No credential value is returned, logged, or stored in a
//! [`ProviderAuthProbe`] — `detail` is always one of the `&'static str` reasons
//! in this file.
//!
//! The layouts are the ones the container launch path already depends on (see
//! [`crate::sandbox::container::launch_auth`]), so the two can't drift. Where a
//! layout isn't knowable from a cheap check we return [`AuthStatus::Unknown`]
//! rather than guess: a wrong "Not signed in" sends the user chasing a login
//! they already have.

use std::path::Path;

use serde_json::Value;

use super::capabilities::PER_TURN_AGENTS;

/// cursor-agent's Keychain services are `<domain>-{access,refresh}-token` with
/// account `<domain>-user`, where the domain is the compiled-in `"cursor"` (see
/// the `@cursor/keychain` credential store in the `cursor-agent` bundle). We
/// probe the access token's *presence* only.
#[cfg(target_os = "macos")]
const CURSOR_KEYCHAIN_SERVICE: &str = "cursor-access-token";
#[cfg(target_os = "macos")]
const CURSOR_KEYCHAIN_ACCOUNT: &str = "cursor-user";

/// Whether a provider's CLI has a usable login on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    SignedIn,
    SignedOut,
    /// We can't tell — no cheap check exists for this provider's credential
    /// store, or the store exists but says nothing about login state. The UI
    /// shows nothing rather than a claim it can't back up.
    Unknown,
}

/// Wire shape of `probe_provider_auth`: one entry per known provider. `detail`
/// is a short human reason for a non-`SignedIn` status (`None` when signed in)
/// and is always a fixed string — never a path with a username in it, and never
/// anything read out of a credential file.
#[derive(Debug, serde::Serialize)]
pub struct ProviderAuthProbe {
    pub id: String,
    pub status: AuthStatus,
    pub detail: Option<String>,
}

/// Probe every known provider's login state in parallel. Never errors — a
/// provider we can't classify contributes [`AuthStatus::Unknown`].
pub async fn probe_all_provider_auth() -> Vec<ProviderAuthProbe> {
    // Same id set as `probe_all_providers`: claude (the lone persistent-runner
    // agent, which has no descriptor row) plus every per-turn descriptor.
    let mut ids: Vec<&'static str> = vec!["claude"];
    ids.extend(PER_TURN_AGENTS.iter().map(|d| d.id));

    let mut handles = Vec::new();
    for id in ids {
        handles.push(tokio::task::spawn_blocking(move || probe_one(id)));
    }

    let mut results = Vec::new();
    for handle in handles {
        if let Ok(probe) = handle.await {
            results.push(probe);
        }
    }
    results
}

fn probe_one(id: &'static str) -> ProviderAuthProbe {
    let Some(home) = dirs::home_dir() else {
        return entry(id, AuthStatus::Unknown, "no home directory");
    };
    match id {
        "claude" => claude_probe(),
        "codex" => codex_probe(&home),
        "opencode" => opencode_probe(&home),
        "pi" => pi_probe(&home),
        "cursor" => cursor_probe(),
        "antigravity" => antigravity_probe(&home),
        _ => entry(id, AuthStatus::Unknown, "no credential layout wired"),
    }
}

// ── Per-provider probes ───────────────────────────────────────────────────────

/// claude reuses the host half of the container auth chain (Keychain login,
/// `~/.claude/.credentials.json`, or an Anthropic key in the login shell). The
/// container-only pasted setup token is excluded there, and the Keychain step is
/// a presence check that never reads the credential — see
/// [`crate::sandbox::container::auth::host_login_present`].
fn claude_probe() -> ProviderAuthProbe {
    if crate::sandbox::container::auth::host_login_present() {
        entry_ok("claude")
    } else {
        entry(
            "claude",
            AuthStatus::SignedOut,
            "no Keychain login, credentials file, or Anthropic key in your shell",
        )
    }
}

/// codex writes `$CODEX_HOME/auth.json` (default `~/.codex/auth.json`) on login;
/// an `OPENAI_API_KEY` in the login shell authenticates it just as well.
fn codex_probe(home: &Path) -> ProviderAuthProbe {
    let auth = read_file(&crate::sandbox::policy::codex_home_dir(home).join("auth.json"));
    let status = classify_codex_auth(auth.as_deref(), env_key_present("OPENAI_API_KEY"));
    detailed(
        "codex",
        status,
        "no credential in $CODEX_HOME/auth.json and no OPENAI_API_KEY",
    )
}

/// opencode's credentials live in `auth.json` under its XDG data dir — the path
/// its own `opencode auth list` prints as "Credentials". Note the `opencode.db`
/// sitting beside it is the session store, *not* an account store, so its
/// presence is deliberately ignored here (a machine that has merely run
/// opencode has one).
fn opencode_probe(home: &Path) -> ProviderAuthProbe {
    let auth = read_file(&crate::sandbox::policy::opencode_data_dir(home).join("auth.json"));
    detailed(
        "opencode",
        classify_provider_map_auth(auth.as_deref()),
        "no credentials in the opencode data dir's auth.json",
    )
}

/// pi writes one provider-keyed credential map at `~/.pi/agent/auth.json` — the
/// same file the container launch path treats as its login.
fn pi_probe(home: &Path) -> ProviderAuthProbe {
    let auth = read_file(&home.join(".pi/agent/auth.json"));
    detailed(
        "pi",
        classify_provider_map_auth(auth.as_deref()),
        "no ~/.pi/agent/auth.json",
    )
}

/// `cursor-agent login` stores its tokens in the OS keychain, not on disk
/// (`~/.cursor` carries only identity metadata) — so on macOS we check for the
/// Keychain item's *presence*, and elsewhere we can't tell. `CURSOR_API_KEY`
/// is the keyless alternative, same as the container path uses.
fn cursor_probe() -> ProviderAuthProbe {
    if env_key_present("CURSOR_API_KEY") {
        return entry_ok("cursor");
    }
    cursor_keychain_probe()
}

#[cfg(target_os = "macos")]
fn cursor_keychain_probe() -> ProviderAuthProbe {
    if crate::keychain::item_present(CURSOR_KEYCHAIN_SERVICE, Some(CURSOR_KEYCHAIN_ACCOUNT)) {
        entry_ok("cursor")
    } else {
        entry(
            "cursor",
            AuthStatus::SignedOut,
            "no cursor-agent login in the macOS Keychain and no CURSOR_API_KEY",
        )
    }
}

/// Off macOS there is no cheap check: the login is in whatever keychain the
/// platform offers, and nothing lands on disk. `cursor-agent status` would
/// answer it, but it makes a network call (~4s here) — too slow and too
/// offline-fragile for a probe that runs on every providers rescan.
#[cfg(not(target_os = "macos"))]
fn cursor_keychain_probe() -> ProviderAuthProbe {
    entry(
        "cursor",
        AuthStatus::Unknown,
        "cursor-agent keeps its login in the OS keychain; only macOS is probed",
    )
}

/// The antigravity CLI drops an OAuth token beside its state in
/// `~/.gemini/antigravity-cli`. Presence is conclusive; absence is *not* — a
/// Google-account login may live elsewhere under `~/.gemini`, so we say
/// `Unknown` rather than claim the user is signed out.
fn antigravity_probe(home: &Path) -> ProviderAuthProbe {
    if non_empty_file(&home.join(".gemini/antigravity-cli/antigravity-oauth-token")) {
        entry_ok("antigravity")
    } else {
        entry(
            "antigravity",
            AuthStatus::Unknown,
            "no antigravity-cli OAuth token file; other login paths unverified",
        )
    }
}

// ── Classifiers (pure, so the shapes are unit-testable) ───────────────────────

/// codex's `auth.json`: an OAuth login populates a `tokens` object, an
/// API-key login populates `OPENAI_API_KEY`. A key in the environment
/// authenticates the CLI without any file at all, hence the separate flag.
/// Malformed or missing JSON is a signed-out state, not an error — a half-written
/// file is indistinguishable from no login as far as the CLI is concerned.
fn classify_codex_auth(json: Option<&[u8]>, env_key_present: bool) -> AuthStatus {
    if env_key_present {
        return AuthStatus::SignedIn;
    }
    let Some(value) = parse_json(json) else {
        return AuthStatus::SignedOut;
    };
    let has_tokens = value
        .get("tokens")
        .and_then(Value::as_object)
        .is_some_and(|t| !t.is_empty());
    // Emptiness only — the value is never copied out or logged.
    let has_key = value
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .is_some_and(|k| !k.trim().is_empty());
    signed_in_if(has_tokens || has_key)
}

/// The `auth.json` shape both multi-provider CLIs use (opencode, pi): a map of
/// provider id → credential object. One entry is a login; an empty object is
/// what a logged-out CLI leaves behind. Only the map's *arity* is inspected —
/// the entries are never looked inside.
fn classify_provider_map_auth(json: Option<&[u8]>) -> AuthStatus {
    match parse_json(json) {
        Some(Value::Object(map)) => signed_in_if(!map.is_empty()),
        _ => AuthStatus::SignedOut,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_json(bytes: Option<&[u8]>) -> Option<Value> {
    serde_json::from_slice(bytes?).ok()
}

fn signed_in_if(yes: bool) -> AuthStatus {
    if yes {
        AuthStatus::SignedIn
    } else {
        AuthStatus::SignedOut
    }
}

fn read_file(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn non_empty_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// Whether `var` is set non-blank in the app's process env or the user's login
/// shell — the same two sources the container auth chain consults, so a key that
/// only exists in `~/.zshrc` still counts. The value is tested for emptiness and
/// dropped; it is never returned or logged.
fn env_key_present(var: &str) -> bool {
    let non_blank = |v: &String| !v.trim().is_empty();
    std::env::var(var).ok().is_some_and(|v| non_blank(&v))
        || crate::bin_resolve::login_shell_env()
            .and_then(|env| env.get(var))
            .is_some_and(non_blank)
}

fn entry_ok(id: &str) -> ProviderAuthProbe {
    ProviderAuthProbe {
        id: id.to_string(),
        status: AuthStatus::SignedIn,
        detail: None,
    }
}

fn entry(id: &str, status: AuthStatus, detail: &'static str) -> ProviderAuthProbe {
    ProviderAuthProbe {
        id: id.to_string(),
        status,
        detail: Some(detail.to_string()),
    }
}

/// `entry`/`entry_ok` in one: a signed-in probe carries no detail, anything else
/// carries `detail`.
fn detailed(id: &str, status: AuthStatus, detail: &'static str) -> ProviderAuthProbe {
    match status {
        AuthStatus::SignedIn => entry_ok(id),
        _ => entry(id, status, detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic fixtures only — no real credential file is ever read in tests.
    const CODEX_OAUTH: &[u8] = br#"{"OPENAI_API_KEY":null,"tokens":{"access_token":"x","refresh_token":"y"},"auth_mode":"chatgpt"}"#;
    const CODEX_API_KEY: &[u8] = br#"{"OPENAI_API_KEY":"sk-fake","tokens":null}"#;
    const CODEX_LOGGED_OUT: &[u8] = br#"{"OPENAI_API_KEY":"","tokens":{}}"#;

    #[test]
    fn codex_oauth_tokens_count_as_signed_in() {
        assert_eq!(
            classify_codex_auth(Some(CODEX_OAUTH), false),
            AuthStatus::SignedIn
        );
    }

    #[test]
    fn codex_api_key_in_file_counts_as_signed_in() {
        assert_eq!(
            classify_codex_auth(Some(CODEX_API_KEY), false),
            AuthStatus::SignedIn
        );
    }

    #[test]
    fn codex_blank_key_and_empty_tokens_are_signed_out() {
        assert_eq!(
            classify_codex_auth(Some(CODEX_LOGGED_OUT), false),
            AuthStatus::SignedOut
        );
        assert_eq!(
            classify_codex_auth(Some(b"{}"), false),
            AuthStatus::SignedOut
        );
    }

    #[test]
    fn codex_malformed_or_missing_file_is_signed_out() {
        assert_eq!(
            classify_codex_auth(Some(b"{not json"), false),
            AuthStatus::SignedOut
        );
        assert_eq!(classify_codex_auth(None, false), AuthStatus::SignedOut);
        // A JSON scalar is well-formed but not the expected object.
        assert_eq!(
            classify_codex_auth(Some(b"7"), false),
            AuthStatus::SignedOut
        );
    }

    #[test]
    fn codex_env_key_alone_is_signed_in() {
        assert_eq!(classify_codex_auth(None, true), AuthStatus::SignedIn);
        // The env key wins even over a file that says logged out.
        assert_eq!(
            classify_codex_auth(Some(CODEX_LOGGED_OUT), true),
            AuthStatus::SignedIn
        );
    }

    #[test]
    fn provider_map_needs_at_least_one_entry() {
        assert_eq!(
            classify_provider_map_auth(Some(br#"{"anthropic":{"type":"oauth"}}"#)),
            AuthStatus::SignedIn
        );
        assert_eq!(
            classify_provider_map_auth(Some(b"{}")),
            AuthStatus::SignedOut
        );
    }

    #[test]
    fn provider_map_rejects_malformed_missing_and_non_object() {
        assert_eq!(
            classify_provider_map_auth(Some(b"{oops")),
            AuthStatus::SignedOut
        );
        assert_eq!(classify_provider_map_auth(None), AuthStatus::SignedOut);
        assert_eq!(
            classify_provider_map_auth(Some(b"[\"anthropic\"]")),
            AuthStatus::SignedOut
        );
    }

    /// Real shapes from both CLIs that share [`classify_provider_map_auth`]:
    /// opencode's `~/.local/share/opencode/auth.json` and pi's
    /// `~/.pi/agent/auth.json`. The multi-entry case is what a user with two
    /// providers configured has.
    #[test]
    fn multi_provider_auth_files_classify_from_their_entry_count() {
        assert_eq!(
            classify_provider_map_auth(Some(
                br#"{"anthropic":{"type":"oauth","refresh":"x"},"openai":{"type":"api","key":"y"}}"#
            )),
            AuthStatus::SignedIn
        );
        // What `opencode auth logout` of the last provider leaves behind.
        assert_eq!(
            classify_provider_map_auth(Some(b"{}")),
            AuthStatus::SignedOut
        );
    }

    #[test]
    fn status_serializes_snake_case_for_the_frontend_union() {
        let json = serde_json::to_string(&entry(
            "codex",
            AuthStatus::SignedOut,
            "no credential in $CODEX_HOME/auth.json and no OPENAI_API_KEY",
        ))
        .unwrap();
        assert!(json.contains(r#""status":"signed_out""#), "{json}");
        assert_eq!(
            serde_json::to_string(&AuthStatus::SignedIn).unwrap(),
            r#""signed_in""#
        );
        assert_eq!(
            serde_json::to_string(&AuthStatus::Unknown).unwrap(),
            r#""unknown""#
        );
    }

    #[test]
    fn signed_in_probes_carry_no_detail() {
        let probe = detailed("pi", AuthStatus::SignedIn, "no ~/.pi/agent/auth.json");
        assert_eq!(probe.status, AuthStatus::SignedIn);
        assert!(probe.detail.is_none());
    }

    #[test]
    fn non_empty_file_rejects_dirs_empty_files_and_absent_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!non_empty_file(dir.path()));
        assert!(!non_empty_file(&dir.path().join("nope")));
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        assert!(!non_empty_file(&empty));
        let token = dir.path().join("antigravity-oauth-token");
        std::fs::write(&token, b"not-a-real-token").unwrap();
        assert!(non_empty_file(&token));
    }

    #[test]
    fn every_known_provider_gets_exactly_one_entry() {
        let mut ids: Vec<&str> = vec!["claude"];
        ids.extend(PER_TURN_AGENTS.iter().map(|d| d.id));
        // Nothing in the id set falls through to the "no layout wired" arm by
        // accident — a new provider descriptor should be a deliberate decision.
        for id in ["claude", "codex", "cursor", "opencode", "pi", "antigravity"] {
            assert!(ids.contains(&id), "{id} missing from the probe id set");
        }
        assert_eq!(
            ids.len(),
            6,
            "provider set changed: wire up its auth layout"
        );
    }
}

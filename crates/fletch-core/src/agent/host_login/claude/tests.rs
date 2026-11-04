use serde_json::{json, Value};

use super::*;
use crate::agent::host_login::now_ms;

const HOUR_MS: i64 = 60 * 60 * 1000;

fn login(access: &str, refresh: &str, expires_at_ms: i64) -> Value {
    json!({
        "claudeAiOauth": {
            "accessToken": access,
            "refreshToken": refresh,
            "expiresAt": expires_at_ms,
            "refreshTokenExpiresAt": expires_at_ms + 700 * HOUR_MS,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "team",
            "rateLimitTier": "default_claude_max_5x",
            "futureField": {"kept": true},
        },
        "mcpOAuth": {"server": {"accessToken": "mcp-secret"}},
    })
}

fn grant(access: &str, refresh: Option<&str>) -> Value {
    let mut body = json!({
        "token_type": "Bearer",
        "access_token": access,
        "expires_in": 28800,
        "scope": "user:inference user:profile",
    });
    if let Some(refresh) = refresh {
        body["refresh_token"] = refresh.into();
    }
    body
}

fn test_store(file: PathBuf) -> HostStore {
    HostStore {
        service: format!("fletch-test-absent-{}", uuid::Uuid::new_v4()),
        file,
    }
}

#[test]
fn a_refresh_preserves_every_field_it_does_not_own() {
    let before = login("a1", "r1", now_ms() - HOUR_MS);
    let mut after = before.clone();
    let expires = apply_grant(
        &mut after,
        &parse_grant(&grant("a2", Some("r2"))).unwrap(),
        1_000,
    );
    assert_eq!(expires, Some(1_000 + 28_800_000));
    assert_eq!(after["claudeAiOauth"]["accessToken"], "a2");
    assert_eq!(after["claudeAiOauth"]["refreshToken"], "r2");
    assert_eq!(after["mcpOAuth"], before["mcpOAuth"]);
    for field in [
        "scopes",
        "subscriptionType",
        "rateLimitTier",
        "futureField",
        "refreshTokenExpiresAt",
    ] {
        assert_eq!(
            after["claudeAiOauth"][field], before["claudeAiOauth"][field],
            "{field}"
        );
    }
}

#[test]
fn a_refresh_without_rotation_keeps_the_stored_refresh_token() {
    let mut json = login("a1", "r1", now_ms() - HOUR_MS);
    apply_grant(&mut json, &parse_grant(&grant("a2", None)).unwrap(), 1_000);
    assert_eq!(json["claudeAiOauth"]["refreshToken"], "r1");
}

/// A Keychain login over 4 KB can't be written back through `security -i`,
/// growth slack included; a file always can.
#[test]
fn only_a_keychain_login_with_room_fits_back() {
    let td = tempfile::tempdir().unwrap();
    let login = ClaudeLogin {
        store: test_store(td.path().join(".credentials.json")),
        token_url: TOKEN_URL.into(),
    };
    let keychain = Place::Keychain {
        account: "alex".into(),
    };
    assert!(login.fits(&keychain, 1_000));
    assert!(!login.fits(&keychain, 4_500));
    assert!(login.fits(&Place::File, 1_000_000));
}

#[test]
fn the_margin_is_inclusive_and_covers_expired_tokens() {
    let now = 1_000_000_000;
    assert!(!needs_refresh(now + REFRESH_MARGIN_MS + 1, now));
    assert!(needs_refresh(now + REFRESH_MARGIN_MS, now));
    assert!(needs_refresh(now - 1, now));
}

#[test]
fn a_placeholder_login_holds_no_token() {
    let placeholder = br#"{"claudeAiOauth":{"accessToken":"x","expiresAt":0}}"#;
    assert_eq!(stored_access_token(placeholder), None);
    assert_eq!(stored_access_token(b"not json"), None);
    let real = login("a1", "r1", 5).to_string();
    assert_eq!(stored_access_token(real.as_bytes()).as_deref(), Some("a1"));
}

#[test]
fn a_401_or_an_invalid_grant_refuses_the_refresh_token() {
    assert!(matches!(
        interpret_refresh(400, Some(&json!({"error": "invalid_grant"}))),
        Err(RefreshFailure::Rejected)
    ));
    assert!(matches!(
        interpret_refresh(401, None),
        Err(RefreshFailure::Rejected)
    ));
}

/// Any other 400 is a request problem a new sign-in wouldn't fix: it must not
/// mark the login refused, which now outlasts a restart.
#[test]
fn any_other_400_is_a_failure_not_a_refusal() {
    for body in [
        Some(json!({"error": "invalid_request"})),
        Some(json!({"error": "unsupported_grant_type"})),
        None,
    ] {
        assert!(
            matches!(
                interpret_refresh(400, body.as_ref()),
                Err(RefreshFailure::Failed(_))
            ),
            "{body:?}"
        );
    }
}

/// Only a standard OAuth error code reaches the message; anything else the
/// server put in `error` is rendered as an unexpected 400.
#[test]
fn a_400_message_names_only_a_known_error_code() {
    let message = |body: Value| match interpret_refresh(400, Some(&body)) {
        Err(RefreshFailure::Failed(m)) => m,
        other => panic!("{other:?}"),
    };
    assert!(message(json!({"error": "invalid_client"})).contains("(invalid_client)"));
    let odd = message(json!({"error": "sk-ant-ort01-echoed-back"}));
    assert!(!odd.contains("sk-ant"), "{odd}");
    assert!(odd.contains("unexpected 400"), "{odd}");
}

#[test]
fn other_answers_map_to_failed_or_a_grant() {
    assert!(matches!(
        interpret_refresh(503, None),
        Err(RefreshFailure::Failed(_))
    ));
    assert!(matches!(
        interpret_refresh(200, Some(&json!({}))),
        Err(RefreshFailure::Failed(_))
    ));
    let ok = interpret_refresh(200, Some(&grant("a2", Some("r2")))).unwrap();
    assert_eq!(ok.expires_in_s, 28800);
}

#[test]
fn debug_output_never_carries_a_token() {
    let token = AccessToken {
        secret: "sk-ant-oat-secret".into(),
        expires_at_ms: 7,
    };
    let printed = format!("{token:?}");
    assert!(!printed.contains("secret"), "{printed}");
    let grant = parse_grant(&grant("sk-ant-oat-secret", Some("sk-ant-ort-secret"))).unwrap();
    let printed = format!("{grant:?}");
    assert!(!printed.contains("sk-ant"), "{printed}");
}

#[test]
fn a_file_login_is_loaded_and_written_back_to_the_file() {
    let td = tempfile::tempdir().unwrap();
    let store = test_store(td.path().join(".credentials.json"));
    assert_eq!(store.stamp(), None);
    let blob = login("a1", "r1", 5).to_string();
    std::fs::write(&store.file, &blob).unwrap();
    let before = store.stamp().expect("a file login has a stamp");
    let (loaded, place) = store.load().unwrap().unwrap();
    assert_eq!((loaded.as_str(), &place), (blob.as_str(), &Place::File));
    store
        .save(&Place::File, &login("a2-longer", "r2", 6).to_string())
        .unwrap();
    assert_ne!(store.stamp().unwrap(), before);
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&store.file).unwrap()).unwrap();
    assert_eq!(saved["claudeAiOauth"]["accessToken"], "a2-longer");
}

/// A refused claude login's mark lives under the accounts root, so "sign in
/// again" survives a restart.
#[test]
fn a_refusal_is_recorded_under_the_accounts_root() {
    accounts::with_test_root(|root| {
        let td = tempfile::tempdir().unwrap();
        let login = ClaudeLogin {
            store: test_store(td.path().join(".credentials.json")),
            token_url: TOKEN_URL.into(),
        };
        let path = login.mark_path().unwrap();
        assert!(
            path.starts_with(root.join(".state").join("claude-revoked")),
            "{}",
            path.display()
        );
    });
}

/// A GUI launch doesn't inherit the shell's env, but the claude it starts
/// does: a `CLAUDE_CONFIG_DIR` only the login shell exports still names the
/// default account's Keychain item and credentials file.
#[test]
fn the_default_login_is_read_from_a_config_dir_the_login_shell_exports() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("relocated");
    let store = crate::bin_resolve::with_login_shell_env(
        &[("CLAUDE_CONFIG_DIR", dir.to_str().unwrap())],
        || HostStore::for_account(None),
    )
    .unwrap();
    assert_eq!(store.service, claude_keychain_service(Some(&dir)));
    assert_ne!(store.service, claude_keychain_service(None));
    assert_eq!(store.file, dir.join(".credentials.json"));
}

/// A default-account launch with no host login fails with where the login
/// was looked for, unless the shell can sign it in.
#[test]
fn a_default_launch_without_a_login_is_refused_unless_something_else_signs_in() {
    let store =
        || "the Keychain item `Claude Code-credentials` or ~/.claude/.credentials.json".to_string();
    assert_eq!(
        default_launch_refusal(&LoginError::SignedOut, true, store),
        None
    );
    let refusal = default_launch_refusal(&LoginError::SignedOut, false, store).unwrap();
    assert!(refusal.contains("`Claude Code-credentials`"), "{refusal}");
    assert!(
        refusal.contains("Settings → Providers → Claude Code"),
        "{refusal}"
    );
    let revoked = default_launch_refusal(&LoginError::Revoked, false, store).unwrap();
    assert!(revoked.contains("revoked"), "{revoked}");
    let unavailable =
        default_launch_refusal(&LoginError::Unavailable("locked".into()), false, store).unwrap();
    assert!(unavailable.contains("locked"), "{unavailable}");
    assert!(unavailable.contains("Keychain"), "{unavailable}");
}

#[cfg(target_os = "macos")]
mod live;

/// The suffix is the first eight hex chars of SHA-256 over the directory
/// path as the env var carries it, verified against a live Keychain item
/// written by claude 2.1.x for a custom `CLAUDE_CONFIG_DIR`.
#[test]
fn keychain_service_is_suffixed_per_config_dir() {
    assert_eq!(claude_keychain_service(None), "Claude Code-credentials");
    assert_eq!(
        claude_keychain_service(Some(Path::new("/cfg/eve"))),
        "Claude Code-credentials-b634a9ea"
    );
    assert_eq!(
        claude_keychain_service(Some(Path::new("/Users/u/.fletch/accounts/claude/work"))),
        "Claude Code-credentials-98eed843"
    );
}

/// Settings' presence probe looks for the Keychain item claude names for a
/// config dir only the login shell exports, not the default dir's.
#[test]
fn the_status_probe_looks_at_the_login_shells_config_dir_keychain_item() {
    let service = crate::bin_resolve::with_login_shell_env(
        &[("CLAUDE_CONFIG_DIR", "/cfg/eve")],
        default_keychain_service,
    );
    assert_eq!(service, "Claude Code-credentials-b634a9ea");
}

#[test]
fn an_api_key_helper_counts_only_when_named_non_blank() {
    assert!(settings_name_an_api_key_helper(Some(
        br#"{"apiKeyHelper": "~/bin/key.sh", "model": "opus"}"#
    )));
    assert!(!settings_name_an_api_key_helper(Some(
        br#"{"apiKeyHelper": "  "}"#
    )));
    assert!(!settings_name_an_api_key_helper(Some(
        br#"{"model": "opus"}"#
    )));
    assert!(!settings_name_an_api_key_helper(Some(b"{not json")));
    assert!(!settings_name_an_api_key_helper(None));
}

/// The credentials file is read under a config dir only the login shell
/// exports.
#[test]
fn the_credentials_file_check_reads_under_the_login_shells_config_dir() {
    let td = tempfile::tempdir().unwrap();
    std::fs::write(
        td.path().join(".credentials.json"),
        br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-x","refreshToken":"r","expiresAt":1893456000000}}"#,
    )
    .unwrap();
    assert!(crate::bin_resolve::with_login_shell_env(
        &[("CLAUDE_CONFIG_DIR", td.path().to_str().unwrap())],
        default_credentials_file_usable,
    ));
}

#[test]
fn credentials_config_dir_honors_claude_config_dir() {
    let home = Path::new("/Users/u");
    assert_eq!(
        credentials_config_dir(None, Some(home)),
        Some(PathBuf::from("/Users/u/.claude"))
    );
    assert_eq!(
        credentials_config_dir(Some(PathBuf::from("/cfg/eve")), Some(home)),
        Some(PathBuf::from("/cfg/eve"))
    );
    assert_eq!(credentials_config_dir(None, None), None);
}

#[test]
fn credentials_file_usable_accepts_a_real_oauth_token() {
    assert!(credentials_file_usable(Some(
        br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-x","refreshToken":"r","expiresAt":1893456000000}}"#
    )));
    // Expired-but-nonzero is still a login: the host refreshes it.
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
fn stored_access_token_extracts_or_rejects() {
    assert_eq!(
        stored_access_token(
            br#"{"claudeAiOauth":{"accessToken":"  sk-ant-oat-x \n","expiresAt":1893456000000}}"#
        ),
        Some("sk-ant-oat-x".to_string()),
        "trimmed token extracted from a usable blob"
    );
}

//! Whose login an account directory holds: enough to tell that two
//! directories are signed in as the same person on the same plan. That is the
//! case where the CLI's own login and a Fletch-managed account are one account
//! listed twice, and the listing folds them (`accounts::list_accounts`).
//!
//! Read from the files each CLI keeps beside its login, never from the
//! credential itself: claude's state file names the OAuth account, and codex's
//! `auth.json` carries an id token whose claims do. The value is only ever
//! compared here in the engine; it is not listed, logged or sent anywhere.

use std::path::Path;

use serde_json::Value;

use super::accounts;
use super::host_login::codex::jwt_claims;

/// The identity of `provider`'s login in `dir` (a managed account), or in the
/// CLI's own directory when `dir` is `None`. `None` when the files don't say:
/// signed out, an API-key login, a shape this doesn't know. Unknown never
/// matches anything, so the worst a miss costs is two rows where one would do.
pub(crate) fn of_account(provider: &str, dir: Option<&Path>, home: &Path) -> Option<String> {
    match provider {
        "claude" => {
            let state = accounts::claude_state_file(dir, home);
            claude_identity(std::fs::read(state).ok().as_deref())
        }
        "codex" => {
            let home_dir;
            let dir = match dir {
                Some(dir) => dir,
                None => {
                    home_dir = crate::sandbox::policy::codex_home_dir(home);
                    &home_dir
                }
            };
            codex_identity(std::fs::read(dir.join("auth.json")).ok().as_deref())
        }
        _ => None,
    }
}

/// Claude's state file names the signed-in OAuth account. The organization is
/// part of the identity: one person's two organizations are two plans, with
/// limits of their own.
pub(crate) fn claude_identity(state: Option<&[u8]>) -> Option<String> {
    let state: Value = serde_json::from_slice(state?).ok()?;
    let account = state.get("oauthAccount")?;
    let user = non_empty(account.get("accountUuid"))?;
    let org = non_empty(account.get("organizationUuid")).unwrap_or_default();
    Some(format!("{org}/{user}"))
}

/// Codex's ChatGPT login: the person (the id token's subject) within the
/// workspace the login is for. The workspace alone would fold two teammates
/// into one. An API-key login has neither and is never matched.
pub(crate) fn codex_identity(auth: Option<&[u8]>) -> Option<String> {
    let auth: Value = serde_json::from_slice(auth?).ok()?;
    let tokens = auth.get("tokens")?;
    let claims = jwt_claims(tokens.get("id_token")?.as_str()?)?;
    let user = non_empty(claims.get("sub"))?;
    let workspace = non_empty(tokens.get("account_id")).unwrap_or_default();
    Some(format!("{workspace}/{user}"))
}

fn non_empty(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// The managed account that holds the same login as the default, if any: the
/// first in listing order. `None` when the default's identity is unknown.
pub(crate) fn duplicate_of<'a>(
    default: Option<&str>,
    managed: impl IntoIterator<Item = (&'a str, Option<String>)>,
) -> Option<String> {
    let default = default?;
    managed
        .into_iter()
        .find(|(_, identity)| identity.as_deref() == Some(default))
        .map(|(id, _)| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::json;

    fn jwt(claims: Value) -> String {
        let part = |v: Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
        };
        format!("{}.{}.sig", part(json!({"alg": "none"})), part(claims))
    }

    fn claude(user: &str, org: &str) -> Vec<u8> {
        json!({"theme": "dark", "oauthAccount": {"accountUuid": user, "organizationUuid": org, "emailAddress": "a@b"}})
            .to_string()
            .into_bytes()
    }

    fn codex(sub: &str, workspace: &str) -> Vec<u8> {
        json!({"tokens": {"id_token": jwt(json!({"sub": sub})), "access_token": "at", "account_id": workspace}})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn claude_is_the_person_within_the_organization() {
        let a = claude_identity(Some(&claude("u1", "o1")));
        assert!(a.is_some());
        assert_eq!(a, claude_identity(Some(&claude("u1", "o1"))));
        assert_ne!(a, claude_identity(Some(&claude("u1", "o2"))));
        assert_ne!(a, claude_identity(Some(&claude("u2", "o1"))));
    }

    #[test]
    fn claude_without_an_oauth_account_is_unknown() {
        assert_eq!(claude_identity(None), None);
        assert_eq!(claude_identity(Some(b"not json")), None);
        // What a fresh managed directory is seeded with.
        assert_eq!(
            claude_identity(Some(br#"{"hasCompletedOnboarding":true}"#)),
            None
        );
        assert_eq!(
            claude_identity(Some(br#"{"oauthAccount":{"accountUuid":""}}"#)),
            None
        );
    }

    #[test]
    fn codex_is_the_person_within_the_workspace() {
        let a = codex_identity(Some(&codex("user-1", "ws-1")));
        assert!(a.is_some());
        assert_eq!(a, codex_identity(Some(&codex("user-1", "ws-1"))));
        // Two teammates in one workspace are not one account.
        assert_ne!(a, codex_identity(Some(&codex("user-2", "ws-1"))));
        assert_ne!(a, codex_identity(Some(&codex("user-1", "ws-2"))));
    }

    #[test]
    fn codex_without_an_id_token_is_unknown() {
        assert_eq!(codex_identity(None), None);
        assert_eq!(
            codex_identity(Some(br#"{"OPENAI_API_KEY":"sk-x","tokens":null}"#)),
            None
        );
        assert_eq!(
            codex_identity(Some(
                br#"{"tokens":{"id_token":"garbage","account_id":"ws"}}"#
            )),
            None
        );
    }

    #[test]
    fn the_default_duplicates_the_first_managed_account_with_its_login() {
        let managed = || {
            vec![
                ("personal", Some("o/u2".to_string())),
                ("signed-out", None),
                ("work", Some("o/u1".to_string())),
                ("work-again", Some("o/u1".to_string())),
            ]
        };
        assert_eq!(duplicate_of(Some("o/u1"), managed()), Some("work".into()));
        assert_eq!(duplicate_of(Some("o/u9"), managed()), None);
        // An unknown login matches nothing, another unknown included.
        assert_eq!(duplicate_of(None, managed()), None);
    }
}

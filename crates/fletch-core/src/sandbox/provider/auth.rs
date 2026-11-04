//! What the adapters share to sign a box launch in. A guest inherits none of
//! the host's env and cannot read its Keychain, so every credential is
//! resolved on the host: claude's from the access token the host hands over,
//! else the shell; opencode's and pi's from a login file, or the shell's keys.

use super::{login_shell_var, Credential, CredentialFile};
use crate::agent::accounts::ambient_credential_vars;
use crate::agent::host_login::claude::AccessToken;
use crate::error::{Error, Result};

/// Env var claude reads an OAuth access token from.
const OAUTH_TOKEN_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// Points claude at a proxy. Not a login, so it follows whichever signs the
/// launch in.
const BASE_URL_VAR: &str = "ANTHROPIC_BASE_URL";

/// claude's env for a box launch: the access token the host resolved for it,
/// else the auth vars in the user's shell. Those belong to the default account,
/// and a managed account always arrives with a token. Empty when neither
/// exists.
pub(crate) fn claude_env(host_login: Option<&AccessToken>) -> Vec<(String, String)> {
    claude_env_from(host_login.map(AccessToken::secret), login_shell_var)
}

fn claude_env_from(
    host_login: Option<&str>,
    shell: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let var = |name: &str| shell(name).map(|value| (name.to_string(), value));
    let mut env: Vec<(String, String)> = match host_login {
        // An ambient credential would outrank the token, so none rides along.
        Some(token) => vec![(OAUTH_TOKEN_VAR.to_string(), token.to_string())],
        None => ambient_credential_vars("claude")
            .iter()
            .filter_map(|name| var(name))
            .collect(),
    };
    if !env.is_empty() {
        env.extend(var(BASE_URL_VAR));
    }
    env
}

/// Provider API-key env vars the multi-provider CLIs (opencode, pi) read to
/// authenticate; any one set satisfies auth on its own. Curated to the
/// mainstream providers both CLIs honor, not exhaustive.
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

/// The keys in [`MULTI_PROVIDER_API_KEY_ENV`] the user's shell exports, in
/// constant order so forwarding is deterministic.
pub(crate) fn present_api_keys() -> Vec<(String, String)> {
    MULTI_PROVIDER_API_KEY_ENV
        .iter()
        .filter_map(|&name| Some((name.to_string(), login_shell_var(name)?)))
        .collect()
}

/// The credential of a multi-provider CLI (opencode, pi): a forwarded key or a
/// login file suffices; with neither, the launch is refused with `no_auth_msg`.
pub(crate) fn multi_provider_credential(
    api_keys: Vec<(String, String)>,
    files: Vec<CredentialFile>,
    no_auth_msg: &str,
) -> Result<Credential> {
    if api_keys.is_empty() && files.is_empty() {
        return Err(Error::Other(no_auth_msg.to_string()));
    }
    Ok(Credential {
        env: api_keys,
        files,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn shell<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    fn names(env: &[(String, String)]) -> Vec<&str> {
        let mut names: Vec<_> = env.iter().map(|(name, _)| name.as_str()).collect();
        names.sort_unstable();
        names
    }

    /// The token wins, and only the endpoint follows it: forwarding an ambient
    /// credential would let claude prefer it over the login.
    #[test]
    fn a_host_login_signs_in_and_only_the_endpoint_follows_it() {
        let vars = [
            ("ANTHROPIC_API_KEY", "sk-ant-ambient"),
            ("ANTHROPIC_AUTH_TOKEN", "gw-secret"),
            ("ANTHROPIC_BASE_URL", "https://proxy"),
        ];
        let env = claude_env_from(Some("sk-ant-oat-login"), shell(&vars));
        assert_eq!(
            env,
            vec![
                (
                    "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
                    "sk-ant-oat-login".to_string()
                ),
                (
                    "ANTHROPIC_BASE_URL".to_string(),
                    "https://proxy".to_string()
                ),
            ]
        );
    }

    #[test]
    fn without_a_host_login_the_shells_auth_vars_sign_in() {
        let vars = [
            ("ANTHROPIC_API_KEY", "sk-ant-key"),
            ("ANTHROPIC_BASE_URL", "https://proxy"),
            ("PATH", "/usr/bin"),
        ];
        assert_eq!(
            names(&claude_env_from(None, shell(&vars))),
            ["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"]
        );
    }

    /// A gateway-only host still authenticates, matching seatbelt.
    #[test]
    fn a_gateway_token_alone_signs_in() {
        let vars = [
            ("ANTHROPIC_AUTH_TOKEN", "gw-secret"),
            ("ANTHROPIC_BASE_URL", "https://gateway"),
        ];
        assert_eq!(
            names(&claude_env_from(None, shell(&vars))),
            ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"]
        );
    }

    #[test]
    fn an_endpoint_alone_signs_nothing_in() {
        let vars = [("ANTHROPIC_BASE_URL", "https://proxy")];
        assert!(claude_env_from(None, shell(&vars)).is_empty());
        assert!(claude_env_from(None, shell(&[])).is_empty());
    }

    /// The value forwarded is the trimmed one, or auth fails in the guest, and
    /// a blank one counts as unset.
    #[test]
    fn the_shell_is_read_trimmed_and_a_blank_value_is_unset() {
        let read = |value| {
            crate::bin_resolve::with_login_shell_env(&[("ANTHROPIC_API_KEY", value)], || {
                login_shell_var("ANTHROPIC_API_KEY")
            })
        };
        assert_eq!(read("  sk-ant-key\n").as_deref(), Some("sk-ant-key"));
        assert_eq!(read("  "), None);
    }

    #[test]
    fn a_multi_provider_cli_needs_a_key_or_a_login_file() {
        let key = vec![("OPENAI_API_KEY".to_string(), "sk".to_string())];
        let file = vec![CredentialFile::cli_owned(PathBuf::from("/h/auth.json"))];
        assert_eq!(
            multi_provider_credential(key.clone(), Vec::new(), "no")
                .unwrap()
                .env,
            key
        );
        assert_eq!(
            multi_provider_credential(Vec::new(), file.clone(), "no")
                .unwrap()
                .files,
            file
        );
        assert_eq!(
            multi_provider_credential(Vec::new(), Vec::new(), "no")
                .unwrap_err()
                .to_string(),
            Error::Other("no".into()).to_string()
        );
    }
}

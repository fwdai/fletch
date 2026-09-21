//! What a host can actually run: the provider CLIs it has, and which of them
//! are signed in.
//!
//! A client that spawns onto a remote host spawns a provider *there*, so the
//! only provider list that matters for that decision is the host's. This
//! module folds the two engine probes — [`super::probe_all_providers`] for the
//! binary, [`super::probe_all_provider_auth`] for the login — into one
//! read-only row per provider, which is what the `host_providers` op answers
//! (see `remote::dispatch`).
//!
//! Read-only by design. Installing a provider and signing one in are the
//! operator's, done on the host itself (`fletch-host provider login <id>`);
//! this says only what the state is, plus the command that would change it, so
//! the client can print the fix instead of offering a button it must not have.
//!
//! Nothing here can carry a secret or a path: the fields are an id, a label, a
//! version string, a fixed status word, and the pinned login command from
//! [`super::login_command`].

use super::{login_command, probe_all_provider_auth, probe_all_providers, provider_bin_label};
use crate::agent::AuthStatus;

/// One provider on the host, as `host_providers` answers it.
///
/// `auth` is `None` when the CLI is not installed — there is no login state to
/// report about a binary that is not there — and otherwise one of `signed_in`,
/// `signed_out`, `unknown`. `unknown` is not a claim: the client must not block
/// on it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProvider {
    pub id: String,
    pub label: String,
    pub installed: bool,
    pub version: Option<String>,
    pub auth: Option<&'static str>,
    /// The vendor's own sign-in command (`claude auth login`), or `None` for a
    /// provider that authenticates out of band. What the client quotes when it
    /// says a provider is signed out; never a button it can press.
    pub login_command: Option<String>,
}

/// Probe every provider on this host: installed + version, then login state for
/// the ones that are installed. Never errors — a provider that cannot be
/// classified is reported as `unknown` rather than dropped.
pub async fn host_providers() -> Vec<HostProvider> {
    let installed = probe_all_providers().await;
    let auth = probe_all_provider_auth().await;

    installed
        .into_iter()
        .map(|probe| {
            let (bin, label) = provider_bin_label(&probe.id).unwrap_or(("", ""));
            let is_installed = probe.path.is_some();
            HostProvider {
                label: label.to_string(),
                installed: is_installed,
                version: probe.version,
                // A login state only means something for a binary that is here.
                auth: is_installed.then(|| {
                    auth.iter()
                        .find(|a| a.id == probe.id)
                        .map_or("unknown", |a| auth_name(a.status))
                }),
                login_command: login_command(&probe.id)
                    .map(|args| format!("{bin} {}", args.join(" "))),
                id: probe.id,
            }
        })
        .collect()
}

/// The wire word for a status. The same three `AuthStatus` serializes as, so a
/// client reads one vocabulary whatever answered it.
fn auth_name(status: AuthStatus) -> &'static str {
    match status {
        AuthStatus::SignedIn => "signed_in",
        AuthStatus::SignedOut => "signed_out",
        AuthStatus::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probes read this machine, so the contents vary — the shape does
    /// not. Every row names a known provider, says whether it is here, and
    /// reports a login state only when it is.
    #[tokio::test]
    async fn every_row_is_a_known_provider_with_a_state_it_can_back_up() {
        let rows = host_providers().await;
        assert!(!rows.is_empty(), "the provider table is never empty");

        for row in &rows {
            assert!(
                provider_bin_label(&row.id).is_some(),
                "{} is not a provider this engine knows",
                row.id
            );
            assert!(!row.label.is_empty(), "{} has no label", row.id);
            match (row.installed, row.auth) {
                (true, Some(auth)) => assert!(
                    matches!(auth, "signed_in" | "signed_out" | "unknown"),
                    "{} reported {auth}",
                    row.id
                ),
                (false, None) => assert!(
                    row.version.is_none(),
                    "{} is not installed but has a version",
                    row.id
                ),
                _ => panic!(
                    "{} reports auth {:?} for installed={}",
                    row.id, row.auth, row.installed
                ),
            }
        }
    }

    /// The row carries a command to *quote*, not a path and not a credential.
    /// `ProviderProbe.path` is the one field the engine's own probe holds that
    /// would name the host's filesystem, and it is deliberately not on this
    /// row — so neither the resolved binary nor the operator's home directory
    /// can ride out to a paired device.
    #[tokio::test]
    async fn no_row_leaks_a_path_or_a_secret() {
        let home = dirs::home_dir().unwrap_or_default();
        let home = home.to_string_lossy().to_string();
        for row in host_providers().await {
            let json = serde_json::to_string(&row).unwrap();
            assert!(
                !json.contains("path"),
                "{} gained a path field: {json}",
                row.id
            );
            assert!(
                home.is_empty() || !json.contains(&home),
                "{} carries the home directory: {json}",
                row.id
            );
            // The login command is the pinned one, verbatim: `<bin> <argv>`,
            // never a resolved path and never anything a caller supplied.
            let pinned = login_command(&row.id).map(|args| {
                format!(
                    "{} {}",
                    provider_bin_label(&row.id).unwrap().0,
                    args.join(" ")
                )
            });
            assert_eq!(
                row.login_command, pinned,
                "{} invented a login command",
                row.id
            );
        }
    }
}

//! Presence-only macOS Keychain lookups.
//!
//! [`item_present`] answers "does an item exist for this service?" by running
//! `security find-generic-password` **without** `-w`. Omitting `-w` is the whole
//! point: `security` then reports metadata and an exit status only, so the
//! password is never read, never returned, and macOS never raises the "… wants
//! to use your confidential information stored in your keychain" prompt. That
//! matters because the providers settings pane polls the auth probe every few
//! seconds while it is open.
//!
//! A caller that genuinely needs the secret must ask for it explicitly and
//! elsewhere — see [`crate::sandbox::container::auth::resolve`], which reads the
//! credential once per container launch. Nothing on a polling path may do that.
//!
//! Off macOS there is no equally cheap check, so the answer is always `false`;
//! callers treat that as "no Keychain login to find here", not "signed out".

/// Whether a generic-password item exists for `service`, optionally narrowed to
/// `account`. Exit status 0 means the item exists; a missing item, a locked
/// keychain, or an unavailable `security` binary all read as `false`.
///
/// Never passes `-w`, so no secret is read and no Keychain prompt appears.
#[cfg(target_os = "macos")]
pub(crate) fn item_present(service: &str, account: Option<&str>) -> bool {
    let mut command = std::process::Command::new("security");
    command.args(["find-generic-password", "-s", service]);
    if let Some(account) = account {
        command.args(["-a", account]);
    }
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn item_present(_service: &str, _account: Option<&str>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real Keychain *hit* isn't unit-testable — it would need an item
    /// planted in the developer's login keychain — so only the miss is covered.
    /// A service nobody has registered exits non-zero and, because no `-w` is
    /// passed, does so without touching or prompting for any secret.
    #[test]
    fn absent_item_reads_as_missing() {
        assert!(!item_present(
            "fletch-keychain-probe-service-that-does-not-exist",
            None
        ));
        assert!(!item_present(
            "fletch-keychain-probe-service-that-does-not-exist",
            Some("nobody")
        ));
    }
}

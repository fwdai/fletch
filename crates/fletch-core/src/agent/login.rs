//! The pinned sign-in command per provider.
//!
//! Detecting a binary is not the same as being signed in, and every CLI does
//! auth differently (browser OAuth, device codes, an interactive provider
//! picker). Rather than model each flow, every caller runs the vendor's own
//! login command and lets the user answer whatever it asks — the desktop under
//! a PTY in an embedded terminal, the headless host straight in the operator's
//! own terminal (`fletch-host provider login <id>`).
//!
//! The commands are pinned here rather than passed in by a caller, so neither
//! the renderer nor the admin socket can launch anything but a known vendor
//! login. Each mirrors the string shown beside the terminal in the desktop UI
//! (`src/data/providerDetail.ts`) — keep the two in sync.

/// argv (after the binary itself) for a provider's interactive sign-in, or
/// `None` when its CLI has no login command — antigravity and pi authenticate
/// out of band, so their callers offer the docs link and the hint only.
///
/// Verified against the installed CLIs' `--help` output; the binary each runs
/// is whatever `resolve_agent_bin` resolves for the provider, so a custom
/// binary path override is honoured here too.
pub fn login_command(id: &str) -> Option<&'static [&'static str]> {
    match id {
        "claude" => Some(&["auth", "login"]),
        // A headless variant (`codex login --device-auth`) is a follow-up, once
        // it is verified against the installed CLI — this stays the one command
        // both the desktop and the host run.
        "codex" => Some(&["login"]),
        "cursor" => Some(&["login"]),
        "opencode" => Some(&["auth", "login"]),
        _ => None,
    }
}

/// argv (after the binary itself) that signs a provider's CLI out of the
/// config dir it runs against, or `None` where Fletch offers no sign-out. Only
/// the account-capable CLIs, whose logout asks nothing — so it runs without a
/// terminal. Verified against `claude auth --help` and `codex --help`.
pub fn logout_command(id: &str) -> Option<&'static [&'static str]> {
    match id {
        "claude" => Some(&["auth", "logout"]),
        "codex" => Some(&["logout"]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_the_logout_command_for_the_account_providers_only() {
        assert_eq!(logout_command("claude"), Some(&["auth", "logout"][..]));
        assert_eq!(logout_command("codex"), Some(&["logout"][..]));
        assert_eq!(logout_command("cursor"), None);
        assert_eq!(logout_command("not-a-provider"), None);
    }

    #[test]
    fn pins_the_login_command_for_every_signable_provider() {
        assert_eq!(login_command("claude"), Some(&["auth", "login"][..]));
        assert_eq!(login_command("codex"), Some(&["login"][..]));
        assert_eq!(login_command("cursor"), Some(&["login"][..]));
        assert_eq!(login_command("opencode"), Some(&["auth", "login"][..]));
    }

    #[test]
    fn has_no_login_for_providers_that_authenticate_out_of_band() {
        assert_eq!(login_command("antigravity"), None);
        assert_eq!(login_command("pi"), None);
    }

    #[test]
    fn has_no_login_for_an_unknown_provider() {
        assert_eq!(login_command(""), None);
        assert_eq!(login_command("not-a-provider"), None);
    }
}

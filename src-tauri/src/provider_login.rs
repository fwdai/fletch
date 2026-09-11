//! Signing an agent CLI in from inside the app: the pinned login command per
//! provider, plus the live PTY sessions driving them.
//!
//! Detecting a binary is not the same as being signed in, and every CLI does
//! auth differently (browser OAuth, device codes, an interactive provider
//! picker). Rather than model each flow, we run the vendor's own login command
//! under a PTY and show it in an embedded terminal — whatever it asks for, the
//! user answers in place.
//!
//! The commands are pinned here rather than passed from the renderer, so the UI
//! can only ever launch a known vendor login. Each mirrors the string shown
//! beside the terminal in the UI (`src/data/providerDetail.ts`) — keep the two
//! in sync.

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::pty_session::PtySession;

/// argv (after the binary itself) for a provider's interactive sign-in, or
/// `None` when its CLI has no login command — antigravity and pi authenticate
/// out of band, so their rows offer the docs link and the hint only.
///
/// Verified against the installed CLIs' `--help` output; the binary each runs
/// is whatever `resolve_agent_bin` resolves for the provider, so a custom
/// binary path override is honoured here too.
pub fn login_args(id: &str) -> Option<&'static [&'static str]> {
    match id {
        "claude" => Some(&["auth", "login"]),
        "codex" => Some(&["login"]),
        "cursor" => Some(&["login"]),
        "opencode" => Some(&["auth", "login"]),
        _ => None,
    }
}

/// The sign-in PTYs running right now, keyed by provider id — at most one per
/// provider, so a second click on "Sign in" attaches to the flow already in
/// progress instead of racing a second one. Removing an entry drops the
/// [`PtySession`], which kills the PTY.
pub type ProviderLoginSessions = Mutex<HashMap<String, PtySession>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_the_login_command_for_every_signable_provider() {
        assert_eq!(login_args("claude"), Some(&["auth", "login"][..]));
        assert_eq!(login_args("codex"), Some(&["login"][..]));
        assert_eq!(login_args("cursor"), Some(&["login"][..]));
        assert_eq!(login_args("opencode"), Some(&["auth", "login"][..]));
    }

    #[test]
    fn has_no_login_for_providers_that_authenticate_out_of_band() {
        assert_eq!(login_args("antigravity"), None);
        assert_eq!(login_args("pi"), None);
    }

    #[test]
    fn has_no_login_for_an_unknown_provider() {
        assert_eq!(login_args(""), None);
        assert_eq!(login_args("not-a-provider"), None);
    }
}

//! Signing an agent CLI in from inside the app: the live PTY sessions driving
//! the vendors' own login commands.
//!
//! Detecting a binary is not the same as being signed in, and every CLI does
//! auth differently (browser OAuth, device codes, an interactive provider
//! picker). Rather than model each flow, we run the vendor's own login command
//! under a PTY and show it in an embedded terminal — whatever it asks for, the
//! user answers in place.
//!
//! Which command that is lives in the engine ([`crate::agent::login_command`]),
//! pinned there so the desktop and the headless host (`fletch-host provider
//! login`) sign the same CLI in the same way.

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::pty_session::PtySession;

/// argv (after the binary itself) for a provider's interactive sign-in. The
/// engine's table, under the name this crate has always called it.
pub use crate::agent::login_command as login_args;

/// The sign-in PTYs running right now, keyed by provider id — at most one per
/// provider, so a second click on "Sign in" attaches to the flow already in
/// progress instead of racing a second one. Removing an entry drops the
/// [`PtySession`], which kills the PTY.
pub type ProviderLoginSessions = Mutex<HashMap<String, PtySession>>;

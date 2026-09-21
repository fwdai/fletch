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
//!
//! The answer is memoised for [`FRESH_FOR`] — see [`Memo`] for why the op
//! needs it.

use std::future::Future;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

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

/// How long an answer stays good.
///
/// A paired device may call `host_providers` as often as it likes, while what
/// it reports only changes at operator cadence — an install or a sign-in, done
/// on the host by hand — so a few seconds of staleness costs nothing and stops
/// a client loop turning into a subprocess-spawn loop on someone's machine.
const FRESH_FOR: Duration = Duration::from_secs(10);

/// The last answer, and when it was taken.
///
/// Probing means resolving six binaries and running `--version` on each, so
/// the op is cheap to *ask* and expensive to *answer* — the asymmetry a remote
/// caller is on the wrong side of. The memo closes the gap twice over: a hit
/// costs nothing, and a *miss* is single-flight.
///
/// Single-flight is the point of holding the lock across the probe rather than
/// around the two halves of it. A connection may have eight requests in
/// flight, and without it eight simultaneous misses mean eight full probes —
/// dozens of subprocesses for one answer they all want anyway. Waiting is
/// bounded by the probe itself (`VERSION_TIMEOUT` per binary, run
/// concurrently), and a caller that waits gets the answer it came for.
///
/// The lock is [`tokio::sync::Mutex`] because it is held across an await.
/// Clippy's `await_holding_lock` is about the std and `parking_lot` guards,
/// which block the executor thread; this one yields.
struct Memo(Mutex<Option<(Instant, Vec<HostProvider>)>>);

impl Memo {
    const fn new() -> Self {
        Self(Mutex::const_new(None))
    }

    /// The memoised answer if it is still fresh at `now`, otherwise `probe`'s,
    /// which is remembered on the way out. `now` is a parameter so the memo is
    /// testable without sleeping.
    ///
    /// Callers that arrive during a probe queue on the lock and take its
    /// result, which by then is fresh — so `probe` runs once per expiry, not
    /// once per caller.
    async fn get<F, Fut>(&self, now: Instant, probe: F) -> Vec<HostProvider>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Vec<HostProvider>>,
    {
        let mut held = self.0.lock().await;
        if let Some((taken, rows)) = held.as_ref() {
            if now.saturating_duration_since(*taken) < FRESH_FOR {
                return rows.clone();
            }
        }
        let rows = probe().await;
        *held = Some((now, rows.clone()));
        rows
    }
}

/// Every caller in this process shares one answer — the wire op, and whatever
/// else comes to want the same list.
static MEMO: Memo = Memo::new();

/// Which provider CLIs this host has, and which of them are signed in.
///
/// Memoised for [`FRESH_FOR`]; [`probe`] is the uncached read.
pub async fn host_providers() -> Vec<HostProvider> {
    MEMO.get(Instant::now(), probe).await
}

/// Probe every provider on this host: installed + version, then login state for
/// the ones that are installed. Never errors — a provider that cannot be
/// classified is reported as `unknown` rather than dropped.
async fn probe() -> Vec<HostProvider> {
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A memo of its own with a counted stand-in for the probe, so nothing
    /// here touches [`MEMO`] — these tests must not decide what a later one
    /// sees, and the real probes read whatever is installed on the machine.
    fn counted() -> (Memo, AtomicUsize) {
        (Memo::new(), AtomicUsize::new(0))
    }

    fn row(id: &str) -> Vec<HostProvider> {
        vec![HostProvider {
            id: id.to_string(),
            label: id.to_string(),
            installed: true,
            version: Some("1.0.0".to_string()),
            auth: Some("signed_in"),
            login_command: None,
        }]
    }

    /// The point of the memo: a client that loops the op gets the same answer
    /// back without six more vendor binaries being run for it.
    #[tokio::test]
    async fn a_second_ask_inside_the_window_does_not_probe_again() {
        let (memo, probes) = counted();
        let at = Instant::now();
        let probe = || async {
            probes.fetch_add(1, Ordering::SeqCst);
            row("claude")
        };

        let first = memo.get(at, probe).await;
        let second = memo.get(at + FRESH_FOR / 2, probe).await;

        assert_eq!(probes.load(Ordering::SeqCst), 1, "it probed twice");
        assert_eq!(first.len(), second.len());
        assert_eq!(first[0].id, second[0].id);
    }

    /// Stale is stale: the window is short precisely so that a sign-in done on
    /// the host shows up on the client's next look, not on its next restart.
    #[tokio::test]
    async fn an_ask_past_the_window_probes_again_and_reports_the_change() {
        let (memo, probes) = counted();
        let at = Instant::now();
        let probe = || async {
            let nth = probes.fetch_add(1, Ordering::SeqCst);
            row(if nth == 0 { "claude" } else { "codex" })
        };

        assert_eq!(memo.get(at, probe).await[0].id, "claude");
        // Exactly at the boundary counts as expired — `fresh` is a strict `<`.
        let after = memo.get(at + FRESH_FOR, probe).await;

        assert_eq!(probes.load(Ordering::SeqCst), 2);
        assert_eq!(after[0].id, "codex", "it served the stale answer");
    }

    /// Eight callers arriving on an empty memo — one connection's whole
    /// in-flight allowance — probe once between them, not eight times. Without
    /// this the op would let a remote caller multiply one ask into dozens of
    /// subprocesses on the host.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_misses_probe_once_and_share_the_answer() {
        let memo = Memo::new();
        let probes = AtomicUsize::new(0);
        let at = Instant::now();
        let probe = || async {
            probes.fetch_add(1, Ordering::SeqCst);
            // Long enough that the other seven are certainly waiting on the
            // lock rather than arriving after the first one finished.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            row("claude")
        };

        let answers = futures_util::future::join_all((0..8).map(|_| memo.get(at, probe))).await;

        assert_eq!(probes.load(Ordering::SeqCst), 1, "the probe was not shared");
        assert_eq!(answers.len(), 8);
        for answer in &answers {
            assert_eq!(answer[0].id, "claude");
        }
    }

    /// An empty answer is an answer. Memoising `None`-as-miss instead would
    /// re-probe every call on a machine with no provider installed — the one
    /// machine where probing is pure waste.
    #[tokio::test]
    async fn an_empty_answer_is_remembered_like_any_other() {
        let (memo, probes) = counted();
        let at = Instant::now();
        let probe = || async {
            probes.fetch_add(1, Ordering::SeqCst);
            Vec::new()
        };

        assert!(memo.get(at, probe).await.is_empty());
        assert!(memo.get(at + FRESH_FOR / 2, probe).await.is_empty());
        assert_eq!(probes.load(Ordering::SeqCst), 1);
    }

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

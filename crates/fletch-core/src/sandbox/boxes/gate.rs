//! The gate between making cloud machines and sweeping them away.
//!
//! A machine is made at a provider in exactly two places, both in
//! [`super::cloud`]: a box's first boot (`Client::create` in `machine_up`)
//! and a lost machine's replacement on its disk (`Client::replace_machine`
//! in `to_running`). Spares, workspaces' boxes and a runner's own machine
//! all pass through them. Each such **make** registers here before the
//! provider is asked ([`make`]) and stays registered until the machine's
//! handle is on disk, or the cleanup after a handle that could not be kept
//! has run.
//!
//! A **sweep** destroys every machine of a scope (one provider, or all of
//! them) that a root it reads has a handle for: a provider's disconnect
//! destroys its spares (`commands::cloud_disconnect_impl`), a host's
//! wind-down destroys its spares and its workspaces' boxes
//! (`super::destroy_cloud_boxes`). It takes a [`Hold`] first ([`hold`]): from
//! that moment a make in its scope is refused at once, with why, and the hold
//! waits (bounded) until no make in its scope is in flight. Only then does
//! the sweep read its roots.
//!
//! So this holds: **a machine at a provider has its handle on disk under a
//! root a sweep reads, or its making is in flight and a hold waits for it.**
//! A sweep that has waited misses none, and none is made behind it while its
//! hold stands. Work that goes on past a make (a spare's preparation still
//! installing) is the sweep's to destroy by its handle like any other; a
//! spare is not marked ready while a hold covers its provider
//! (`spares::write_ready`), so no taker has a machine being swept. Dropping a
//! hold releases its scope; a hold kept ([`Hold::keep`]) refuses makes for the
//! rest of the process, for a host about to be deleted. Process-wide, as the
//! box registry is.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::cloud::ProviderId;
use crate::error::{Error, Result};

/// How long a hold waits for the makes in flight in its scope: making a
/// machine waits for the provider to start it, which can take minutes.
const HOLD_WAIT: Duration = Duration::from_secs(10 * 60);
/// The pause between two looks at the makes in flight.
const HOLD_PACE: Duration = Duration::from_millis(100);

/// Who a hold covers: one provider, or every provider (`None`).
pub type Scope = Option<ProviderId>;

/// The makes in flight and the holds standing.
#[derive(Default)]
pub struct Gate {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The provider of each make in flight, one entry per make.
    making: Vec<ProviderId>,
    /// Each standing hold, by its id.
    holds: Vec<(u64, Scope)>,
    next_hold: u64,
}

/// A make in flight, registered until dropped.
pub struct Making {
    gate: Arc<Gate>,
    provider: ProviderId,
}

impl Drop for Making {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock();
        if let Some(at) = state.making.iter().position(|p| *p == self.provider) {
            state.making.swap_remove(at);
        }
    }
}

/// A sweep's hold on its scope: makes there are refused while it stands.
pub struct Hold {
    gate: Arc<Gate>,
    id: u64,
    settled: bool,
}

impl Hold {
    /// Whether no make in the scope was in flight any more when the hold's
    /// wait ended; `false` when the wait ran out with one still in flight,
    /// whose machine may then be left at its provider.
    pub fn settled(&self) -> bool {
        self.settled
    }

    /// Keep the scope held for the rest of the process.
    pub fn keep(self) {
        std::mem::forget(self);
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.gate
            .state
            .lock()
            .holds
            .retain(|(id, _)| *id != self.id);
    }
}

/// Whether `scope` covers `provider`.
fn covers<P: PartialEq>(scope: Option<P>, provider: &P) -> bool {
    scope.as_ref().map_or(true, |p| p == provider)
}

impl Gate {
    /// Register a make at `provider`, refused while a hold covers it.
    fn make(self: &Arc<Self>, provider: ProviderId) -> Result<Making> {
        let mut state = self.state.lock();
        if let Some((_, scope)) = state.holds.iter().find(|(_, s)| covers(*s, &provider)) {
            return Err(Error::SandboxUnavailable(match scope {
                Some(_) => format!(
                    "{} is being disconnected on this machine and its spare machines deleted, so \
                     no machine is made there now. Try again once it is done, or connect it again.",
                    provider.label()
                ),
                None => "the cloud machines this host made are being deleted, so no machine is \
                         made now"
                    .to_string(),
            }));
        }
        state.making.push(provider);
        Ok(Making {
            gate: self.clone(),
            provider,
        })
    }

    /// Hold `scope`, then wait at most `wait` until no make in it is in
    /// flight.
    async fn hold(self: &Arc<Self>, scope: Scope, wait: Duration) -> Hold {
        let id = {
            let mut state = self.state.lock();
            state.next_hold += 1;
            let id = state.next_hold;
            state.holds.push((id, scope));
            id
        };
        let mut hold = Hold {
            gate: self.clone(),
            id,
            settled: false,
        };
        let deadline = Instant::now() + wait;
        loop {
            let in_flight = self.state.lock().making.iter().any(|p| covers(scope, p));
            if !in_flight {
                hold.settled = true;
                return hold;
            }
            if Instant::now() >= deadline {
                tracing::warn!(?scope, "a sweep's wait for the machines being made ran out");
                return hold;
            }
            tokio::time::sleep(HOLD_PACE).await;
        }
    }

    fn held(&self, provider: ProviderId) -> bool {
        self.state
            .lock()
            .holds
            .iter()
            .any(|(_, s)| covers(*s, &provider))
    }
}

/// The process's gate, or under a test the one it set
/// ([`with_own_gate`]).
fn current() -> Arc<Gate> {
    #[cfg(test)]
    if let Ok(gate) = TEST_GATE.try_with(Arc::clone) {
        return gate;
    }
    static GATE: OnceLock<Arc<Gate>> = OnceLock::new();
    GATE.get_or_init(Arc::default).clone()
}

/// Register a make at `provider` before its machine is asked of the
/// provider, and keep what this answers until the machine's handle is on
/// disk. Refused at once, with why, while a sweep holds the provider.
pub fn make(provider: ProviderId) -> Result<Making> {
    current().make(provider)
}

/// Hold `scope` for a sweep: makes in it are refused from now on, and this
/// answers once none is in flight, or the wait ran out ([`Hold::settled`]).
pub async fn hold(scope: Scope) -> Hold {
    current().hold(scope, HOLD_WAIT).await
}

/// Whether a sweep holds `provider` now.
pub fn held(provider: ProviderId) -> bool {
    current().held(provider)
}

#[cfg(test)]
tokio::task_local! {
    static TEST_GATE: Arc<Gate>;
}

/// Run `work` with a gate of its own, so that a test's holds refuse nothing
/// of another test's.
#[cfg(test)]
pub async fn with_own_gate<F: std::future::Future>(work: F) -> F::Output {
    TEST_GATE.scope(Arc::default(), work).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: ProviderId = ProviderId::ALL[0];

    /// A hold for one provider covers it and nothing else; a hold for all
    /// covers every one.
    #[test]
    fn a_scope_covers_its_provider_or_all_of_them() {
        assert!(covers(Some(1u8), &1));
        assert!(!covers(Some(1u8), &2));
        assert!(covers(None, &2u8));
        assert!(covers(Some(P), &P));
        assert!(covers(None, &P));
    }

    /// A make while a hold stands is refused with why; once the hold is
    /// dropped, makes go ahead; a kept hold refuses them for good.
    #[tokio::test]
    async fn a_make_under_a_hold_is_refused_and_one_after_it_is_not() {
        let gate = Arc::new(Gate::default());
        let hold = gate.hold(Some(P), Duration::from_millis(10)).await;
        assert!(hold.settled());
        let refusal = gate.make(P).err().expect("refused").to_string();
        assert!(refusal.contains("being disconnected"), "{refusal}");
        assert!(gate.held(P));
        drop(hold);
        assert!(!gate.held(P));
        drop(gate.make(P).expect("no hold now"));

        gate.hold(None, Duration::from_millis(10)).await.keep();
        let refusal = gate.make(P).err().expect("refused").to_string();
        assert!(refusal.contains("being deleted"), "{refusal}");
    }

    /// A hold waits for a make in flight in its scope, and settles once its
    /// handle is down (the make dropped); one that outlasts the wait leaves
    /// the hold unsettled.
    #[tokio::test]
    async fn a_hold_waits_for_the_makes_in_flight() {
        let gate = Arc::new(Gate::default());
        let making = gate.make(P).unwrap();
        let unsettled = gate.hold(Some(P), Duration::from_millis(1)).await;
        assert!(!unsettled.settled(), "the make is still in flight");
        drop(unsettled);

        let waited = gate.hold(None, Duration::from_secs(5));
        let finished = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(gate.make(P).is_err(), "refused while the hold waits");
            drop(making);
        };
        let (hold, ()) = tokio::join!(waited, finished);
        assert!(hold.settled());
    }
}

//! Host-side watch over every bound PR, for the ship loop and for every client.
//!
//! Clients do not poll GitHub: they seed from one read (`get_all_pr_status`,
//! and `get_pr_live` / `get_pr_threads` for a PR on screen with nothing
//! cached) and then follow the events this task emits, so GitHub is read once
//! a minute however many windows and phones are open — and still read with
//! every one of them shut. This task is the host's own poll: once a minute
//! it runs the same batched sweep the sidebar uses
//! ([`resolve_all_pr_status`] — one GraphQL query for every bound PR, snapshot
//! persistence and the backoff gate included), every other tick also reads the
//! open PRs' review threads through `get_pr_threads_impl`, diffs each read
//! against what it last saw, and emits one event per change —
//! `pr:state_changed`, `pr:checks_changed`, `pr:threads_changed` — for the
//! remote forwarder and the push triggers (`remote::push`) to act on.
//!
//! Modelled on `roadmap::merge_sweep`: a `Notify` nudge, each pass on its own
//! task so a panic cannot end the loop. The first read of an open PR seeds the
//! memory and emits only its state — a PR reopened, or opened outside Fletch,
//! must reach clients that no longer poll — never its checks or threads, so a
//! restart does not re-announce last week's threads.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::github::{MergeState, PrChecks, PrComments, PrState, PrStatus};
use crate::host::{EngineCtx, EventSink};

use super::events::{emit_pr_checks, emit_pr_state, emit_pr_threads};
use super::session_sync::resolve_all_pr_status;
use super::Supervisor;

const TICK: Duration = Duration::from_secs(60);

fn signal() -> &'static Notify {
    static SIGNAL: OnceLock<Notify> = OnceLock::new();
    SIGNAL.get_or_init(Notify::new)
}

/// Wake the watcher now. Called when a PR has just been resolved for an agent
/// (a push, a turn end), so a freshly opened PR is seeded while its checks are
/// still pending — the seed emits no checks, and a PR first seen already
/// failing would otherwise never announce that failure.
pub(crate) fn nudge() {
    signal().notify_one();
}

/// What the watcher last saw of one PR, keyed by [`pr_map_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    /// Which PR this memory is about. A checkout's key outlives its PR — a
    /// follow-up is adopted onto the same branch after a merge — and the old
    /// PR's rollup and threads say nothing about the new one.
    number: u32,
    state: PrStatus,
    /// `None` until a tick resolved checks.
    rollup: Option<String>,
    required_failing: BTreeSet<String>,
    /// The merge gate, `None` until a read reported a known one: GitHub
    /// computes it lazily, and an `unknown` in between two real answers is not
    /// a change.
    merge_state: Option<MergeState>,
    threads: BTreeSet<String>,
}

impl Seen {
    fn first(read: &Read) -> Self {
        Self {
            number: read.state.number,
            state: read.state.state,
            rollup: read.checks.as_ref().map(|c| c.rollup.clone()),
            required_failing: read.checks.as_ref().map(failing_set).unwrap_or_default(),
            merge_state: read.checks.as_ref().and_then(known_merge_state),
            threads: read.threads.as_ref().map(thread_ids).unwrap_or_default(),
        }
    }
}

type Last = Mutex<HashMap<String, Seen>>;

/// One tick's read of a PR. `checks: None` is "the CI read did not resolve"
/// and `threads: None` "not polled this tick" — both leave the last value
/// alone rather than reporting a change.
pub(crate) struct Read {
    pub state: PrState,
    pub checks: Option<PrChecks>,
    pub threads: Option<PrComments>,
}

/// An event the diff decided to emit.
#[derive(Debug, Clone)]
pub(crate) enum Change {
    State(PrState),
    Checks(PrChecks),
    Threads {
        comments: PrComments,
        new_ids: Vec<String>,
    },
}

fn failing_set(checks: &PrChecks) -> BTreeSet<String> {
    checks.required_failing.iter().cloned().collect()
}

fn known_merge_state(checks: &PrChecks) -> Option<MergeState> {
    (checks.merge_state != MergeState::Unknown).then_some(checks.merge_state)
}

fn thread_ids(comments: &PrComments) -> BTreeSet<String> {
    comments.unresolved.iter().map(|c| c.id.clone()).collect()
}

/// Fold one read into `last` and say what changed. Pure, so the rules are
/// testable without GitHub:
///
/// - An open PR not in `last` is seeded and reports its state alone: clients
///   follow these events instead of polling, so a PR reopened, opened outside
///   Fletch or opened before the first look has to reach them. Its checks and
///   threads are the baseline, not news — a restart re-announces no thread. A
///   settled PR not in `last` reports nothing: the sweep serves it from its
///   snapshot every tick and there is nothing to watch.
/// - A different PR number under the same key is a new PR: its state is
///   reported once and it is seeded afresh, since the old memory is about a
///   PR that is gone.
/// - A state change is the only thing reported for it, and the key goes —
///   a settled PR has nothing further to watch.
/// - Checks report when the rollup, the set of failing names or the (known)
///   merge gate moves.
/// - Threads report whenever the unresolved set changes, naming the ids not
///   seen before — none when a thread was only resolved. Clients follow these
///   events instead of polling, so a resolution has to reach them too; the
///   alerts key off the new ids and stay quiet for it.
pub(crate) fn diff(last: &mut HashMap<String, Seen>, key: &str, read: &Read) -> Vec<Change> {
    let Some(seen) = last.get_mut(key) else {
        if read.state.state != PrStatus::Open {
            return Vec::new();
        }
        last.insert(key.to_string(), Seen::first(read));
        return vec![Change::State(read.state.clone())];
    };
    if read.state.number != seen.number {
        last.remove(key);
        if read.state.state == PrStatus::Open {
            last.insert(key.to_string(), Seen::first(read));
        }
        return vec![Change::State(read.state.clone())];
    }
    if read.state.state != seen.state {
        last.remove(key);
        return vec![Change::State(read.state.clone())];
    }
    let mut changes = Vec::new();
    if let Some(checks) = &read.checks {
        let failing = failing_set(checks);
        let merge_state = known_merge_state(checks).or(seen.merge_state);
        if seen.rollup.as_deref() != Some(checks.rollup.as_str())
            || seen.required_failing != failing
            || seen.merge_state != merge_state
        {
            seen.rollup = Some(checks.rollup.clone());
            seen.required_failing = failing;
            seen.merge_state = merge_state;
            changes.push(Change::Checks(checks.clone()));
        }
    }
    if let Some(comments) = &read.threads {
        let ids = thread_ids(comments);
        if ids != seen.threads {
            let new_ids: Vec<String> = ids.difference(&seen.threads).cloned().collect();
            seen.threads = ids;
            changes.push(Change::Threads {
                comments: comments.clone(),
                new_ids,
            });
        }
    }
    changes
}

/// Put the changes on the wire, each addressed to its checkout: `subdir` is
/// `None` for the primary repo and names a secondary, so a secondary's merge
/// lands on its own key rather than the agent's primary PR.
fn publish(
    sink: &dyn EventSink,
    agent_id: &str,
    subdir: Option<&str>,
    number: u32,
    changes: Vec<Change>,
) {
    for change in changes {
        match change {
            Change::State(state) => emit_pr_state(sink, agent_id, subdir, Some(state)),
            Change::Checks(checks) => emit_pr_checks(sink, agent_id, subdir, number, checks),
            Change::Threads { comments, new_ids } => {
                emit_pr_threads(sink, agent_id, subdir, comments, new_ids)
            }
        }
    }
}

// Background watch over every bound PR. Reads once now (to seed), then every
// minute; threads every other tick.
pub fn spawn(ctx: Arc<EngineCtx>, supervisor: Arc<Supervisor>) {
    crate::host::spawn(async move {
        let last: Arc<Last> = Arc::new(Mutex::new(HashMap::new()));
        let mut ticks: u64 = 0;
        loop {
            let with_threads = ticks % 2 == 1;
            ticks += 1;
            let pass = {
                let (ctx, supervisor, last) = (ctx.clone(), supervisor.clone(), last.clone());
                crate::host::spawn(
                    async move { tick(&ctx, &supervisor, &last, with_threads).await },
                )
                .await
            };
            if let Err(e) = pass {
                tracing::error!(error = %e, "pr watch tick panicked — watching continues");
            }
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {}
                _ = signal().notified() => {}
            }
        }
    });
}

/// One pass: the sidebar's batched sweep, then threads for the open PRs. A PR
/// that merged through any other path comes back in the same map as `merged`
/// (served from its snapshot), so the diff's state path announces it once and
/// forgets it — no bookkeeping of its own.
async fn tick(ctx: &Arc<EngineCtx>, supervisor: &Arc<Supervisor>, last: &Last, with_threads: bool) {
    let statuses = resolve_all_pr_status(&supervisor.workspace, false).await;
    for (key, status) in statuses {
        // The inverse of `pr_map_key`: no `::` is the primary repo.
        let (agent_id, subdir) = match key.split_once("::") {
            Some((agent_id, subdir)) => (agent_id, Some(subdir)),
            None => (key.as_str(), None),
        };
        let threads = if with_threads && status.state.state == PrStatus::Open {
            match crate::commands::get_pr_threads_impl(supervisor, agent_id, subdir).await {
                Ok(threads) => threads,
                Err(e) => {
                    tracing::debug!(agent_id, error = %e, "pr watch: thread read failed");
                    None
                }
            }
        } else {
            None
        };
        let read = Read {
            state: status.state,
            checks: status.checks,
            threads,
        };
        let number = read.state.number;
        let changes = diff(&mut last.lock(), &key, &read);
        publish(ctx.sink.as_ref(), agent_id, subdir, number, changes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{MergeableState, PrComment};
    use crate::supervisor::pr_map_key;

    fn pr(status: PrStatus) -> PrState {
        PrState {
            number: 650,
            url: "https://github.com/o/r/pull/650".to_string(),
            state: status,
            title: "t".to_string(),
            mergeable: MergeableState::Unknown,
            opened_at: None,
            merged_at: None,
        }
    }

    fn checks(rollup: &str, failing: &[&str]) -> PrChecks {
        PrChecks {
            merge_state: MergeState::Unknown,
            rollup: rollup.to_string(),
            total: 2,
            passed: 0,
            failed: failing.len() as u32,
            pending: 0,
            required_failing: failing.iter().map(|s| s.to_string()).collect(),
            runs: Vec::new(),
        }
    }

    fn threads(ids: &[&str]) -> PrComments {
        PrComments {
            unresolved: ids
                .iter()
                .map(|id| PrComment {
                    id: id.to_string(),
                    author: "greptile".to_string(),
                    is_bot: true,
                    body: String::new(),
                    path: None,
                    line: None,
                    url: String::new(),
                    replies: 0,
                    we_replied_last: false,
                })
                .collect(),
        }
    }

    fn read(status: PrStatus, checks: Option<PrChecks>, threads: Option<PrComments>) -> Read {
        Read {
            state: pr(status),
            checks,
            threads,
        }
    }

    /// Diff one read of the primary repo's PR for agent `arabia`.
    fn step(last: &mut HashMap<String, Seen>, read: &Read) -> Vec<Change> {
        diff(last, &pr_map_key("arabia", "", true), read)
    }

    /// The first look at an open PR reports its state — clients follow these
    /// events and would otherwise never learn of a PR opened outside Fletch —
    /// and only its state: its checks and threads are the baseline. That is
    /// also what a host restart looks like for an already-open PR, so the
    /// restart announces no old thread or failing check.
    #[test]
    fn the_first_observation_seeds_and_emits_only_the_state() {
        let mut last = HashMap::new();
        let first = read(
            PrStatus::Open,
            Some(checks("failing", &["unit"])),
            Some(threads(&["t1"])),
        );
        let changes = step(&mut last, &first);
        assert!(
            matches!(&changes[..], [Change::State(s)] if s.state == PrStatus::Open),
            "{changes:?}"
        );
        // A second identical read is nothing.
        assert!(step(&mut last, &first).is_empty());
        let seen = &last["arabia"];
        assert_eq!(seen.rollup.as_deref(), Some("failing"));
        assert_eq!(seen.required_failing, BTreeSet::from(["unit".to_string()]));
        assert_eq!(seen.threads, BTreeSet::from(["t1".to_string()]));
    }

    /// A PR that is already settled when first read — the sweep serves merged
    /// and closed ones from their snapshot every tick — is nobody's to watch.
    #[test]
    fn a_settled_pr_is_not_seeded() {
        let mut last = HashMap::new();
        assert!(step(&mut last, &read(PrStatus::Merged, None, None)).is_empty());
        assert!(step(&mut last, &read(PrStatus::Merged, None, None)).is_empty());
        assert!(last.is_empty());
    }

    #[test]
    fn a_rollup_moving_from_pending_to_failing_emits_checks() {
        let mut last = HashMap::new();
        step(
            &mut last,
            &read(PrStatus::Open, Some(checks("pending", &[])), None),
        );
        let changes = step(
            &mut last,
            &read(PrStatus::Open, Some(checks("failing", &["unit"])), None),
        );
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert!(matches!(&changes[0], Change::Checks(c) if c.rollup == "failing"));
        assert_eq!(last["arabia"].rollup.as_deref(), Some("failing"));
    }

    /// The names matter as much as the colour: a second check joining the
    /// failing set is a change even though the rollup stays `failing`.
    #[test]
    fn a_changed_failing_set_emits_checks_under_the_same_rollup() {
        let mut last = HashMap::new();
        step(
            &mut last,
            &read(PrStatus::Open, Some(checks("failing", &["unit"])), None),
        );
        let changes = step(
            &mut last,
            &read(
                PrStatus::Open,
                Some(checks("failing", &["unit", "lint"])),
                None,
            ),
        );
        assert!(matches!(&changes[..], [Change::Checks(_)]), "{changes:?}");
    }

    /// An unresolved CI read is not "no checks": it leaves the last rollup
    /// alone instead of flapping it.
    #[test]
    fn an_unresolved_checks_read_changes_nothing() {
        let mut last = HashMap::new();
        step(
            &mut last,
            &read(PrStatus::Open, Some(checks("passing", &[])), None),
        );
        assert!(step(&mut last, &read(PrStatus::Open, None, None)).is_empty());
        assert_eq!(last["arabia"].rollup.as_deref(), Some("passing"));
    }

    #[test]
    fn a_new_thread_emits_exactly_the_new_ids() {
        let mut last = HashMap::new();
        step(
            &mut last,
            &read(PrStatus::Open, None, Some(threads(&["t1"]))),
        );
        let changes = step(
            &mut last,
            &read(PrStatus::Open, None, Some(threads(&["t1", "t2", "t3"]))),
        );
        match &changes[..] {
            [Change::Threads { comments, new_ids }] => {
                assert_eq!(new_ids, &["t2".to_string(), "t3".to_string()]);
                assert_eq!(comments.unresolved.len(), 3, "the whole set travels");
            }
            other => panic!("expected one threads change: {other:?}"),
        }
        // A thread resolving travels too, so a client following the events
        // drops it — but it names nothing new, which is what keeps it quiet.
        match &step(
            &mut last,
            &read(PrStatus::Open, None, Some(threads(&["t3"]))),
        )[..]
        {
            [Change::Threads { comments, new_ids }] => {
                assert!(new_ids.is_empty(), "{new_ids:?}");
                assert_eq!(comments.unresolved.len(), 1);
            }
            other => panic!("expected one threads change: {other:?}"),
        }
        // The same set again is nothing.
        assert!(step(
            &mut last,
            &read(PrStatus::Open, None, Some(threads(&["t3"])))
        )
        .is_empty());
    }

    /// The merge gate is what the panel's buttons read, so its moving is news —
    /// but GitHub's lazy `unknown` between two real answers is not.
    #[test]
    fn a_moved_merge_gate_emits_checks_and_unknown_does_not() {
        let mut last = HashMap::new();
        let gate = |state: MergeState| {
            let mut c = checks("passing", &[]);
            c.merge_state = state;
            read(PrStatus::Open, Some(c), None)
        };
        step(&mut last, &gate(MergeState::Clean));
        assert!(step(&mut last, &gate(MergeState::Unknown)).is_empty());
        assert!(step(&mut last, &gate(MergeState::Clean)).is_empty());
        let changes = step(&mut last, &gate(MergeState::Behind));
        assert!(matches!(&changes[..], [Change::Checks(_)]), "{changes:?}");
    }

    /// A reopened PR is unseen again (its key went when it closed), and the
    /// clients that no longer poll have to hear it is open.
    #[test]
    fn open_closed_open_emits_closed_then_open() {
        let mut last = HashMap::new();
        let states = |changes: Vec<Change>| -> Vec<PrStatus> {
            changes
                .into_iter()
                .map(|c| match c {
                    Change::State(s) => s.state,
                    other => panic!("expected a state change: {other:?}"),
                })
                .collect()
        };
        let open = read(PrStatus::Open, Some(checks("passing", &[])), None);
        assert_eq!(states(step(&mut last, &open)), [PrStatus::Open]);
        assert_eq!(
            states(step(&mut last, &read(PrStatus::Closed, None, None))),
            [PrStatus::Closed]
        );
        assert!(last.is_empty());
        // Closed again is not news; the reopen is, once.
        assert!(step(&mut last, &read(PrStatus::Closed, None, None)).is_empty());
        assert_eq!(states(step(&mut last, &open)), [PrStatus::Open]);
        assert!(step(&mut last, &open).is_empty());
    }

    #[test]
    fn open_to_merged_emits_state_and_drops_the_key() {
        let mut last = HashMap::new();
        step(
            &mut last,
            &read(
                PrStatus::Open,
                Some(checks("passing", &[])),
                Some(threads(&["t1"])),
            ),
        );
        // Whatever else the final read carries, the merge is the only news.
        let changes = step(
            &mut last,
            &read(
                PrStatus::Merged,
                Some(checks("failing", &["unit"])),
                Some(threads(&["t9"])),
            ),
        );
        assert!(
            matches!(&changes[..], [Change::State(s)] if s.state == PrStatus::Merged),
            "{changes:?}"
        );
        assert!(last.is_empty(), "a settled PR has nothing further to watch");
    }

    /// A follow-up PR adopted onto the same branch is a different PR: its state
    /// is announced once and the memory starts over, so the old PR's green
    /// rollup and read threads are not held against the new one.
    #[test]
    fn a_different_pr_number_under_the_same_key_starts_over() {
        let mut last = HashMap::new();
        step(
            &mut last,
            &read(
                PrStatus::Open,
                Some(checks("passing", &[])),
                Some(threads(&["t1"])),
            ),
        );
        let mut next = read(
            PrStatus::Open,
            Some(checks("passing", &[])),
            Some(threads(&["t1"])),
        );
        next.state.number = 651;
        let changes = step(&mut last, &next);
        assert!(
            matches!(&changes[..], [Change::State(s)] if s.number == 651),
            "{changes:?}"
        );
        let seen = &last["arabia"];
        assert_eq!(seen.number, 651);
        assert_eq!(seen.rollup.as_deref(), Some("passing"));
        // The new PR's own first settle is news when it moves, like any PR's.
        let mut later = read(PrStatus::Open, Some(checks("failing", &["unit"])), None);
        later.state.number = 651;
        let changes = step(&mut last, &later);
        assert!(matches!(&changes[..], [Change::Checks(_)]), "{changes:?}");
    }

    #[test]
    fn an_identical_second_read_emits_nothing() {
        let mut last = HashMap::new();
        let same = read(
            PrStatus::Open,
            Some(checks("failing", &["unit"])),
            Some(threads(&["t1", "t2"])),
        );
        assert_eq!(step(&mut last, &same).len(), 1, "the seed's state");
        assert!(step(&mut last, &same).is_empty());
        assert_eq!(last.len(), 1);
    }

    /// Secondary repos are watched under their own key, beside the primary's.
    #[test]
    fn repos_are_watched_per_key() {
        let mut last = HashMap::new();
        step(
            &mut last,
            &read(PrStatus::Open, Some(checks("pending", &[])), None),
        );
        let secondary = pr_map_key("arabia", "api", false);
        let pending = read(PrStatus::Open, Some(checks("pending", &[])), None);
        assert!(matches!(
            &diff(&mut last, &secondary, &pending)[..],
            [Change::State(_)]
        ));
        let changes = diff(
            &mut last,
            &secondary,
            &read(PrStatus::Open, Some(checks("passing", &[])), None),
        );
        assert!(matches!(&changes[..], [Change::Checks(_)]));
        assert_eq!(last["arabia"].rollup.as_deref(), Some("pending"));
        assert_eq!(last["arabia::api"].rollup.as_deref(), Some("passing"));
    }

    /// A secondary's PR settling is reported under its own key, like the
    /// primary's — it is not hidden as per-repo panel state.
    #[test]
    fn a_secondary_pr_merging_emits_state_under_its_key() {
        let mut last = HashMap::new();
        let secondary = pr_map_key("arabia", "api", false);
        step(&mut last, &read(PrStatus::Open, None, None));
        diff(&mut last, &secondary, &read(PrStatus::Open, None, None));
        let changes = diff(&mut last, &secondary, &read(PrStatus::Merged, None, None));
        assert!(
            matches!(&changes[..], [Change::State(s)] if s.state == PrStatus::Merged),
            "{changes:?}"
        );
        assert!(!last.contains_key("arabia::api"));
        assert!(last.contains_key("arabia"), "the primary is still watched");
    }

    /// `publish` puts a secondary's state on the wire with its subdir and the
    /// primary's with `subdir: null`, so a client can key each by checkout.
    #[test]
    fn publish_addresses_state_to_its_checkout() {
        use crate::host::sink::RecordingSink;

        let sink = RecordingSink::new();
        let merged = pr(PrStatus::Merged);
        publish(
            &sink,
            "arabia",
            Some("api"),
            650,
            vec![Change::State(merged.clone())],
        );
        publish(&sink, "arabia", None, 650, vec![Change::State(merged)]);
        let events = sink.events();
        let subdirs: Vec<_> = events
            .iter()
            .map(|(name, payload)| {
                assert_eq!(name, "pr:state_changed");
                assert_eq!(payload["agent_id"], "arabia");
                assert_eq!(payload["state"]["state"], "merged");
                payload["subdir"].clone()
            })
            .collect();
        assert_eq!(subdirs, [serde_json::json!("api"), serde_json::Value::Null]);
    }
}

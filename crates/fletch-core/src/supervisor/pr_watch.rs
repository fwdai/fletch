//! Host-side watch over every bound PR, for the ship loop.
//!
//! The desktop polls a PR's state, checks and review threads only while its
//! Git panel is open, and `pr:state_changed` fires only when something else
//! triggers a resolve (a turn end, a push, a client poll). A phone with the
//! Mac's window closed therefore hears nothing when checks fail, a reviewer
//! comments or the PR merges. This task is the host's own poll: once a minute
//! it runs the same batched sweep the sidebar uses
//! ([`resolve_all_pr_status`] — one GraphQL query for every bound PR, snapshot
//! persistence and the backoff gate included), every other tick also reads the
//! open PRs' review threads through `get_pr_threads_impl`, diffs each read
//! against what it last saw, and emits one event per change —
//! `pr:state_changed`, `pr:checks_changed`, `pr:threads_changed` — for the
//! remote forwarder and the push triggers (`remote::push`) to act on.
//!
//! Modelled on `roadmap::merge_sweep`: a `Notify` nudge, each pass on its own
//! task so a panic cannot end the loop. The first read of a PR seeds the memory
//! and emits nothing, so a restart does not re-announce last week's threads.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::github::{PrChecks, PrComments, PrState, PrStatus};
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
/// still pending — the seed emits nothing, and a PR first seen already failing
/// would otherwise never announce that failure.
pub(crate) fn nudge() {
    signal().notify_one();
}

/// What the watcher last saw of one PR, keyed by [`pr_map_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    state: PrStatus,
    /// `None` until a tick resolved checks.
    rollup: Option<String>,
    required_failing: BTreeSet<String>,
    threads: BTreeSet<String>,
}

impl Seen {
    fn first(read: &Read) -> Self {
        Self {
            state: read.state.state,
            rollup: read.checks.as_ref().map(|c| c.rollup.clone()),
            required_failing: read.checks.as_ref().map(failing_set).unwrap_or_default(),
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

fn thread_ids(comments: &PrComments) -> BTreeSet<String> {
    comments.unresolved.iter().map(|c| c.id.clone()).collect()
}

/// Fold one read into `last` and say what changed. Pure, so the rules are
/// testable without GitHub:
///
/// - A PR not in `last` is seeded (if open) and reports nothing.
/// - A state change is the only thing reported for it, and the key goes —
///   a settled PR has nothing further to watch.
/// - Checks report when the rollup or the set of failing names moves.
/// - Threads report the ids not seen before; a resolved one just leaves.
pub(crate) fn diff(last: &mut HashMap<String, Seen>, key: &str, read: &Read) -> Vec<Change> {
    let Some(seen) = last.get_mut(key) else {
        if read.state.state == PrStatus::Open {
            last.insert(key.to_string(), Seen::first(read));
        }
        return Vec::new();
    };
    if read.state.state != seen.state {
        last.remove(key);
        return vec![Change::State(read.state.clone())];
    }
    let mut changes = Vec::new();
    if let Some(checks) = &read.checks {
        let failing = failing_set(checks);
        if seen.rollup.as_deref() != Some(checks.rollup.as_str())
            || seen.required_failing != failing
        {
            seen.rollup = Some(checks.rollup.clone());
            seen.required_failing = failing;
            changes.push(Change::Checks(checks.clone()));
        }
    }
    if let Some(comments) = &read.threads {
        let ids = thread_ids(comments);
        let new_ids: Vec<String> = ids.difference(&seen.threads).cloned().collect();
        seen.threads = ids;
        if !new_ids.is_empty() {
            changes.push(Change::Threads {
                comments: comments.clone(),
                new_ids,
            });
        }
    }
    changes
}

/// Put the changes on the wire. A state change is app-wide only for the
/// primary repo, as `fetch_and_emit_pr_state` emits it; a secondary's PR is
/// per-repo panel state and only its checks and threads are announced.
fn publish(sink: &dyn EventSink, agent_id: &str, subdir: Option<&str>, changes: Vec<Change>) {
    for change in changes {
        match change {
            Change::State(state) => {
                if subdir.is_none() {
                    emit_pr_state(sink, agent_id, Some(state));
                }
            }
            Change::Checks(checks) => emit_pr_checks(sink, agent_id, subdir, checks),
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
        let changes = diff(&mut last.lock(), &key, &read);
        publish(ctx.sink.as_ref(), agent_id, subdir, changes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{MergeState, MergeableState, PrComment};
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

    #[test]
    fn the_first_observation_seeds_and_emits_nothing() {
        let mut last = HashMap::new();
        let changes = step(
            &mut last,
            &read(
                PrStatus::Open,
                Some(checks("failing", &["unit"])),
                Some(threads(&["t1"])),
            ),
        );
        assert!(changes.is_empty(), "{changes:?}");
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
        // A thread resolving is not news; the one that stays is not new.
        assert!(step(
            &mut last,
            &read(PrStatus::Open, None, Some(threads(&["t3"])))
        )
        .is_empty());
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

    #[test]
    fn an_identical_second_read_emits_nothing() {
        let mut last = HashMap::new();
        let same = read(
            PrStatus::Open,
            Some(checks("failing", &["unit"])),
            Some(threads(&["t1", "t2"])),
        );
        step(&mut last, &same);
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
        assert!(diff(&mut last, &secondary, &pending).is_empty());
        let changes = diff(
            &mut last,
            &secondary,
            &read(PrStatus::Open, Some(checks("passing", &[])), None),
        );
        assert!(matches!(&changes[..], [Change::Checks(_)]));
        assert_eq!(last["arabia"].rollup.as_deref(), Some("pending"));
        assert_eq!(last["arabia::api"].rollup.as_deref(), Some("passing"));
    }
}

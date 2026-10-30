//! Git state surfaces shared with the remote dispatcher: one checkout's full
//! state, the fleet-wide shortstats poll behind the sidebar badges, and the
//! base-freshness fetch the host runs on its own clock.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::error::Result;
use crate::git_state::{self, GitMeta, GitState, ShortStats};
use crate::supervisor::Supervisor;

use super::files::{agent_repo_checkout_opt, checkout_pending};

/// Git state for one of the agent's checkouts — the repo whose `subdir`
/// matches, or the primary when none is given.
///
/// Shared with the remote dispatcher.
pub async fn get_git_state_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<GitState>> {
    // Still cloning: no checkout to describe yet. `None` is what an unresolvable
    // agent already returns, and the panel renders it as Loading… rather than
    // as a status full of phantom deletions.
    if checkout_pending(supervisor, agent_id) {
        return Ok(None);
    }
    let Some((repo, checkout)) = agent_repo_checkout_opt(supervisor, agent_id, subdir)? else {
        return Ok(None);
    };
    // A refused checkout answers with the keys to remove rather than an error:
    // the panel polls this, and a failing poll just freezes it on stale state.
    // Checked before resolving the base, which runs git there too.
    let blocked = crate::git::hardening::steerable_config(&checkout).await?;
    if !blocked.is_empty() {
        let parent = repo.parent_branch.clone().unwrap_or_default();
        return Ok(Some(GitState::blocked(parent, blocked)));
    }
    let base = repo.resolve_base(&checkout).await;
    let state = git_state::query(&checkout, &base).await?;
    Ok(Some(state))
}

/// [`get_git_state_impl`] for a timer poll (the Git panel's interval tick,
/// autopilot): `None` for a sleeping box, which it does not wake, so the panel
/// keeps what it showed. Anything a user asked for takes `get_git_state_impl`.
///
/// Shared with the remote dispatcher.
pub async fn poll_git_state_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<GitState>> {
    let asleep = || checkout_asleep(supervisor, agent_id, subdir);
    poll_unless_asleep(asleep, get_git_state_impl(supervisor, agent_id, subdir))
        .await
        .unwrap_or(Ok(None))
}

/// Whether the agent's checkout for `subdir` is in a box that is not running.
pub(crate) fn checkout_asleep(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> bool {
    agent_repo_checkout_opt(supervisor, agent_id, subdir)
        .ok()
        .flatten()
        .is_some_and(|(_, checkout)| crate::sandbox::boxes::asleep_for_path(&checkout))
}

/// `read` as a [passive](crate::exec::passive) poll: `None` when `asleep`
/// holds before it or after it. Checked after too because a box that stops
/// mid-read fails its git commands, which `git_state` absorbs into an empty,
/// clean-looking answer.
pub(crate) async fn poll_unless_asleep<T>(
    asleep: impl Fn() -> bool,
    read: impl Future<Output = T>,
) -> Option<T> {
    if asleep() {
        return None;
    }
    let value = crate::exec::passive(read).await;
    (!asleep()).then_some(value)
}

/// What [`poll_checkout`] last read from each box checkout, per poll. Each
/// sweep drops the checkouts it no longer reads.
static LAST_SHORTSTATS: Mutex<BTreeMap<PathBuf, ShortStats>> = Mutex::new(BTreeMap::new());
static LAST_GIT_META: Mutex<BTreeMap<PathBuf, GitMeta>> = Mutex::new(BTreeMap::new());

/// `read` of `checkout` for a bulk poll ([`poll_unless_asleep`]). A sleeping
/// box answers its last read, which its stopped guest cannot have changed, so
/// the sidebar keeps its numbers; `None` before a first read.
async fn poll_checkout<T: Clone>(
    last: &Mutex<BTreeMap<PathBuf, T>>,
    checkout: &Path,
    read: impl Future<Output = T>,
) -> Option<T> {
    use crate::sandbox::boxes;
    let Some(value) = poll_unless_asleep(|| boxes::asleep_for_path(checkout), read).await else {
        return last.lock().get(checkout).cloned();
    };
    if boxes::box_holding(checkout).is_some() {
        last.lock().insert(checkout.to_path_buf(), value.clone());
    }
    Some(value)
}

/// A compact shortstat (additions / deletions / file count) for every live
/// agent's primary repo, keyed by agent id. Used by the app-wide background
/// poll that powers per-agent shortstats in the sidebar and the right-rail
/// file-count badge. Archived agents and agents with no resolvable repo are
/// omitted; a git error degrades to zeroes. A sleeping box is not woken to be
/// counted ([`poll_checkout`]).
///
/// Each agent's stats come from `git_state::shortstats`, which spawns just the
/// two git processes the badge reads (status + numstat) rather than the ~7 a
/// full `GitState` needs. Agents are queried in parallel, so total latency is
/// bounded by the slowest agent's git invocation, not the sum. The reply
/// carries only the three numbers per agent — no file list — to keep the IPC
/// payload flat as the agent count grows.
///
/// Shared with the remote dispatcher.
pub async fn get_all_shortstats_impl(
    supervisor: &Supervisor,
) -> Result<std::collections::HashMap<String, ShortStats>> {
    let workspace = match supervisor.workspace.current() {
        Some(w) => w,
        None => return Ok(Default::default()),
    };
    let mut set = tokio::task::JoinSet::new();
    let mut seen = BTreeSet::new();
    for agent in workspace.agents {
        // Omitted while provisioning for the same reason as archived agents:
        // there is nothing to count yet, and counting a half-written clone
        // would flash a phantom file count on the badge.
        if agent.archive.is_some() || checkout_pending(supervisor, &agent.id) {
            continue;
        }
        // One shortstat per checkout; a multi-repo agent's badge shows the
        // sum across all of its repos (matching the archive metadata, which
        // also aggregates). Single-repo agents behave exactly as before.
        for repo in &agent.repos {
            let Ok(checkout) = repo.checkout_path(&agent.id) else {
                continue;
            };
            let agent_id = agent.id.clone();
            seen.insert(checkout.clone());
            set.spawn(async move {
                let read = git_state::shortstats(&checkout);
                let stats = poll_checkout(&LAST_SHORTSTATS, &checkout, read).await;
                (agent_id, stats)
            });
        }
    }
    let mut out: std::collections::HashMap<String, ShortStats> = std::collections::HashMap::new();
    while let Some(res) = set.join_next().await {
        if let Ok((id, Some(stats))) = res {
            let entry = out.entry(id).or_insert(ShortStats {
                additions: 0,
                deletions: 0,
                file_count: 0,
            });
            entry.additions += stats.additions;
            entry.deletions += stats.deletions;
            entry.file_count += stats.file_count;
        }
    }
    LAST_SHORTSTATS
        .lock()
        .retain(|checkout, _| seen.contains(checkout));
    Ok(out)
}

/// Advisory fleet-wide git metadata for every live agent's checkouts, keyed by
/// the frontend's `gitKey` convention (plain agent id for the primary repo,
/// `"{agent_id}::{subdir}"` for secondaries — same as the PR maps). Feeds the
/// always-visible "base moved · N behind" chips and the cross-agent
/// file-overlap hints.
///
/// Purely local git — no network. Each checkout's `behind` is measured against
/// its resolved base tip (`git::resolve_base`), preferring the SOURCE repo's
/// `refs/remotes/origin/<base>` that the host's base-freshness loop
/// ([`refresh_base_freshness_impl`]) advances, whenever that commit is readable from the checkout. Without a
/// GitHub connection the source ref never advances, so `behind` stays
/// unknown/zero and no chip shows — the intended silent degrade. File paths
/// always resolve (local status), so overlap hints work with or without GitHub.
///
/// Queried in parallel; a git error degrades that checkout to a bare
/// unknown-behind / empty-files entry rather than dropping it. A sleeping box
/// is not woken to be read ([`poll_checkout`]).
///
/// Shared with the remote dispatcher.
pub async fn get_all_git_meta_impl(
    supervisor: &Supervisor,
) -> Result<std::collections::HashMap<String, GitMeta>> {
    let workspace = match supervisor.workspace.current() {
        Some(w) => w,
        None => return Ok(Default::default()),
    };
    let mut set = tokio::task::JoinSet::new();
    let mut seen = BTreeSet::new();
    for agent in workspace.agents {
        if agent.archive.is_some() || checkout_pending(supervisor, &agent.id) {
            continue;
        }
        for (i, repo) in agent.repos.iter().enumerate() {
            let Ok(checkout) = repo.checkout_path(&agent.id) else {
                continue;
            };
            let key = crate::supervisor::pr_map_key(&agent.id, &repo.subdir, i == 0);
            // Resolved inside the task, not in this loop: resolving a base
            // shells out to git, and doing that serially would make the poll's
            // latency scale with the fleet size instead of the slowest checkout.
            let repo = repo.clone();
            seen.insert(checkout.clone());
            set.spawn(async move {
                let read = async {
                    let base = repo.resolve_base(&checkout).await;
                    git_state::git_meta(&checkout, &base).await
                };
                (key, poll_checkout(&LAST_GIT_META, &checkout, read).await)
            });
        }
    }
    let mut out = std::collections::HashMap::new();
    while let Some(res) = set.join_next().await {
        if let Ok((key, Some(meta))) = res {
            out.insert(key, meta);
        }
    }
    LAST_GIT_META
        .lock()
        .retain(|checkout, _| seen.contains(checkout));
    Ok(out)
}

/// What one base-freshness pass did, for the host's log line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BaseFreshness {
    /// Distinct `(source repo, base)` pairs fetched.
    pub fetched: usize,
    /// Fetches that failed and were stepped over.
    pub failed: usize,
    /// Why the pass touched no network, when it did not.
    pub skipped: Option<&'static str>,
}

impl BaseFreshness {
    fn skipped(why: &'static str) -> Self {
        Self {
            skipped: Some(why),
            ..Self::default()
        }
    }
}

/// Fetch each project's base branch on its SOURCE repo, so `get_all_git_meta`
/// can measure staleness against a base that moved on GitHub (a sibling's PR
/// merging, a teammate's push). One fetch per distinct `(source repo, base)` —
/// deduped so a multi-agent project pays a single fetch, and the shared object
/// store propagates it to every clone.
///
/// Best-effort and silent by contract: no GitHub credential or a paused
/// rate-limit backoff skips the whole pass, and each fetch failure is logged
/// and stepped over — a background fetch must never raise a user-facing error.
/// Nothing is emitted: there is no git-meta event, and the clients'
/// `get_all_git_meta` poll reads the moved ref on its next tick.
///
/// Run by the host's own loop (`supervisor::base_freshness`) and by the
/// desktop's `refresh_base_freshness` command.
pub async fn refresh_base_freshness_impl(supervisor: &Supervisor) -> BaseFreshness {
    if crate::github::client::token().is_none() {
        return BaseFreshness::skipped("no GitHub credential");
    }
    // Paused → touch no network; the last-fetched base tips still serve.
    if crate::github::client::is_backing_off() {
        return BaseFreshness::skipped("rate-limit backoff");
    }
    let Some(workspace) = supervisor.workspace.current() else {
        return BaseFreshness::skipped("no workspace");
    };
    // Distinct (source repo, base) across every live agent's repos — one fetch
    // covers all clones that share that source's objects.
    let mut seen: BTreeSet<(PathBuf, String)> = BTreeSet::new();
    for agent in &workspace.agents {
        if agent.archive.is_some() {
            continue;
        }
        for repo in &agent.repos {
            seen.insert((repo.repo_path.clone(), repo.base_branch().await));
        }
    }
    let mut out = BaseFreshness::default();
    for (source, base) in seen {
        match crate::git::fetch_base(&source, &base).await {
            Ok(()) => out.fetched += 1,
            Err(e) => {
                out.failed += 1;
                tracing::debug!(error = %e, source = %source.display(), base, "base freshness fetch skipped");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::boxes::registry::TestBox;

    /// A poll reads a host checkout as before, and never a sleeping box's:
    /// that one answers its last read, or nothing before one.
    #[tokio::test]
    async fn a_poll_answers_a_sleeping_box_from_its_last_read() {
        let id = "poll-checkout-unit-test";
        let checkout = PathBuf::from(format!("/{id}/root/repo"));
        let last: Mutex<BTreeMap<PathBuf, u32>> = Mutex::new(BTreeMap::new());

        assert_eq!(poll_checkout(&last, &checkout, async { 7 }).await, Some(7));
        assert!(last.lock().is_empty(), "a host checkout is not remembered");

        let _test_box = TestBox::register(id);
        let unread = || async { panic!("a sleeping box's checkout is not read") };
        assert_eq!(poll_checkout(&last, &checkout, unread()).await, None);
        last.lock().insert(checkout.clone(), 3);
        assert_eq!(poll_checkout(&last, &checkout, unread()).await, Some(3));
    }

    /// A box that goes to sleep during the read answers `None`, not what the
    /// failed read made of it; the bulk poll answers its last read instead.
    #[tokio::test]
    async fn a_box_asleep_after_the_read_answers_none() {
        let id = "poll-asleep-after-read-unit-test";
        let checkout = PathBuf::from(format!("/{id}/root/repo"));
        let asleep = || crate::sandbox::boxes::asleep_for_path(&checkout);

        let mut test_box = None;
        let read = async {
            test_box = Some(TestBox::register(id));
            "clean"
        };
        assert_eq!(poll_unless_asleep(asleep, read).await, None);

        let last = Mutex::new(BTreeMap::from([(checkout.clone(), "dirty")]));
        drop(test_box.take());
        let read = async {
            test_box = Some(TestBox::register(id));
            "clean"
        };
        assert_eq!(poll_checkout(&last, &checkout, read).await, Some("dirty"));
        assert_eq!(last.lock().get(&checkout), Some(&"dirty"));
    }
}

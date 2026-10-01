//! Hourly sweep that archives sidebar workspaces nobody has touched for a while.
//!
//! Archiving tears the clone down (`disposition::archive_agent`), and restore
//! refetches the pushed branch from origin — so anything uncommitted or
//! unpushed is gone for good. The sweep therefore archives only a checkout
//! that is clean *and* fully pushed, and treats every read failure as "not
//! this pass". The decision itself is [`eligible`], a pure function over a
//! [`Candidate`], so the rules are tested without git or a database.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::database;
use crate::github::PrStatus;
use crate::host::EngineCtx;
use crate::workspace::{AgentRecord, AgentStatus};
use crate::DbState;

use super::Supervisor;

/// Settings key: days a workspace may sit idle before the sweep archives it,
/// as an integer string. `0` turns the sweep off. Written by the desktop's
/// `set_auto_archive_idle_days` command; read here on every pass.
pub const IDLE_DAYS_SETTING: &str = "auto_archive_idle_days";

/// What applies when the setting is unset or unparsable.
pub const DEFAULT_IDLE_DAYS: u32 = 7;

/// A workspace whose PR has merged or closed is finished work: it goes after
/// this fixed grace instead of the configured idle period, which exists for
/// work that may still be picked up.
pub const MERGED_PR_GRACE_DAYS: u32 = 1;

/// First pass waits for the git and PR polls after boot to settle, so the
/// `pr_state` snapshots it reads are current.
const FIRST_PASS_DELAY: Duration = Duration::from_secs(2 * 60);
const SWEEP: Duration = Duration::from_secs(60 * 60);

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Decode the stored setting; anything unparsable is the default.
pub fn parse_idle_days(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_IDLE_DAYS)
}

/// One checkout's state as the sweep saw it. `pr_state` is the record's
/// last-known snapshot, not a live fetch — database truth that survives a
/// GitHub outage, and the same thing the sidebar shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RepoSummary {
    pub clean_tree: bool,
    pub unpushed: u32,
    pub blocked_config: bool,
    pub pr_state: Option<PrStatus>,
    /// When the checkout directory was created — the spawn, or the restore
    /// that rebuilt it. A restored workspace keeps its old session records, so
    /// without this floor its idle clock would still read weeks old and the
    /// next pass would archive it again before the user typed a word.
    /// `None` when the filesystem does not report a birth time.
    pub provisioned_ms: Option<i64>,
}

/// Everything [`eligible`] needs to know about one agent.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub archived: bool,
    pub run_owned: bool,
    pub purpose_tagged: bool,
    /// The supervisor's effective status (live value over the resting one).
    pub status: AgentStatus,
    /// Ingest time of the newest session record, when there is one.
    pub last_activity_ms: Option<i64>,
    /// Stands in for `last_activity_ms` on an agent that never produced a
    /// record. `None` when the record's timestamp could not be read.
    pub created_at_ms: Option<i64>,
    /// Every non-adopted checkout. Empty means nothing could be verified.
    pub repos: Vec<RepoSummary>,
}

/// Why an agent qualifies — what the archive log line reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Eligible {
    pub idle_days: u32,
    /// A repo's PR is merged or closed, so the shorter grace applied.
    pub finished_pr: bool,
}

/// Whether `c` may be archived now. `None` is always the safe answer: the
/// sweep retries next pass, and nothing is lost by waiting.
pub(crate) fn eligible(c: &Candidate, now_ms: i64, threshold_days: u32) -> Option<Eligible> {
    if threshold_days == 0 || c.archived || c.run_owned || c.purpose_tagged {
        return None;
    }
    if matches!(c.status, AgentStatus::Spawning | AgentStatus::Running) {
        return None;
    }
    // An agent with nothing verifiable is not "trivially clean" — it is
    // unknown, and unknown is not eligible.
    if c.repos.is_empty() {
        return None;
    }
    let all_pushed = c
        .repos
        .iter()
        .all(|r| r.clean_tree && r.unpushed == 0 && !r.blocked_config);
    if !all_pushed {
        return None;
    }
    // Idle is measured from whichever is latest: the last session record (or
    // the agent's creation, for one that never produced any) and the newest
    // checkout provisioning. The second floor is what keeps a just-restored
    // workspace from being swept straight back.
    let activity = c.last_activity_ms.or(c.created_at_ms)?;
    let provisioned = c.repos.iter().filter_map(|r| r.provisioned_ms).max();
    let since = provisioned.map_or(activity, |p| p.max(activity));
    let idle_ms = now_ms.saturating_sub(since);
    if idle_ms < 0 {
        return None;
    }
    let finished_pr = c
        .repos
        .iter()
        .any(|r| matches!(r.pr_state, Some(PrStatus::Merged | PrStatus::Closed)));
    let required_days = if finished_pr {
        MERGED_PR_GRACE_DAYS
    } else {
        threshold_days
    };
    if idle_ms < i64::from(required_days) * DAY_MS {
        return None;
    }
    Some(Eligible {
        idle_days: u32::try_from(idle_ms / DAY_MS).unwrap_or(u32::MAX),
        finished_pr,
    })
}

/// Payload of `workspace:auto-archived`, one per pass that archived anything,
/// so the desktop can tell the user what moved to History.
#[derive(Debug, Serialize)]
struct AutoArchivedEvent {
    agent_ids: Vec<String>,
    names: Vec<String>,
}

/// Start the sweep. Each pass runs in its own task so a panic is logged and
/// the loop goes on, as `roadmap::merge_sweep` does.
pub fn spawn(ctx: Arc<EngineCtx>, supervisor: Arc<Supervisor>, db: DbState) {
    crate::host::spawn(async move {
        tokio::time::sleep(FIRST_PASS_DELAY).await;
        loop {
            let pass = {
                let (ctx, supervisor, db) = (ctx.clone(), supervisor.clone(), db.clone());
                crate::host::spawn(async move { sweep(&ctx, &supervisor, &db).await }).await
            };
            if let Err(e) = pass {
                tracing::error!(error = %e, "auto-archive pass panicked — sweeping continues");
            }
            tokio::time::sleep(SWEEP).await;
        }
    });
}

async fn sweep(ctx: &Arc<EngineCtx>, supervisor: &Arc<Supervisor>, db: &DbState) {
    let threshold_days = {
        let conn = db.lock();
        parse_idle_days(database::get_setting(&conn, IDLE_DAYS_SETTING).as_deref())
    };
    if threshold_days == 0 {
        return;
    }
    // The sidebar snapshot already omits run-owned and purpose-tagged agents
    // and overlays the live status; `eligible` re-checks all three anyway.
    let Some(ws) = supervisor.current_workspace() else {
        return;
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut archived: Vec<(String, String)> = Vec::new();
    for record in ws.agents {
        // `eligible` rejects these too; skipping first avoids git reads on a
        // torn-down checkout or on a tree the agent is writing to right now.
        if record.archive.is_some()
            || matches!(record.status, AgentStatus::Spawning | AgentStatus::Running)
        {
            continue;
        }
        let Some(repos) = summarize_repos(&record).await else {
            continue;
        };
        let candidate = Candidate {
            archived: record.archive.is_some(),
            run_owned: record.owner_run_id.is_some(),
            purpose_tagged: record.purpose.is_some(),
            status: record.status.clone(),
            last_activity_ms: supervisor.last_activity(&record.id),
            created_at_ms: chrono::DateTime::parse_from_rfc3339(&record.created_at)
                .ok()
                .map(|t| t.timestamp_millis()),
            repos,
        };
        let Some(verdict) = eligible(&candidate, now_ms, threshold_days) else {
            continue;
        };
        // A turn may have started since the git reads above; the fresh record
        // is what decides.
        let Some(fresh) = supervisor.agent_record(&record.id) else {
            continue;
        };
        if fresh.archive.is_some()
            || matches!(fresh.status, AgentStatus::Spawning | AgentStatus::Running)
        {
            continue;
        }
        match supervisor
            .clone()
            .archive_agent(ctx.clone(), &record.id)
            .await
        {
            Ok(()) => {
                tracing::info!(
                    agent_id = %record.id,
                    name = %record.name,
                    idle_days = verdict.idle_days,
                    finished_pr = verdict.finished_pr,
                    "auto-archived idle workspace"
                );
                archived.push((record.id.clone(), record.name.clone()));
            }
            Err(e) => {
                tracing::warn!(agent_id = %record.id, error = %e, "auto-archive failed");
            }
        }
    }
    if !archived.is_empty() {
        let (agent_ids, names) = archived.into_iter().unzip();
        crate::host::emit(
            ctx.sink.as_ref(),
            "workspace:auto-archived",
            &AutoArchivedEvent { agent_ids, names },
        );
    }
}

/// Read every owned checkout's git state. `None` when any read fails — an
/// unreadable tree might hold anything, so the agent sits this pass out.
async fn summarize_repos(record: &AgentRecord) -> Option<Vec<RepoSummary>> {
    let mut out = Vec::new();
    for repo in record.repos.iter().filter(|r| !r.is_adopted()) {
        let checkout = match repo.checkout_path(&record.id) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(agent_id = %record.id, error = %e, "auto-archive: no checkout path");
                return None;
            }
        };
        let base = repo.resolve_base(&checkout).await;
        let state = match crate::git_state::query(&checkout, &base).await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(agent_id = %record.id, error = %e, "auto-archive: git read failed");
                return None;
            }
        };
        out.push(RepoSummary {
            clean_tree: state.files.is_empty(),
            unpushed: state.unpushed,
            blocked_config: !state.blocked_config.is_empty(),
            pr_state: repo.pr_state.as_deref().and_then(PrStatus::parse),
            provisioned_ms: provisioned_at(&checkout),
        });
    }
    Some(out)
}

/// Birth time of the checkout dir, ms since the epoch. Clone creates the dir,
/// so this is when the current checkout came to exist; later writes inside it
/// leave it alone (unlike mtime, which every `git status` index refresh bumps).
/// `None` where the filesystem has no birth time to report.
fn provisioned_at(checkout: &std::path::Path) -> Option<i64> {
    let created = std::fs::metadata(checkout).ok()?.created().ok()?;
    let since_epoch = created.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since_epoch.as_millis()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    fn pushed(pr_state: Option<PrStatus>) -> RepoSummary {
        RepoSummary {
            clean_tree: true,
            unpushed: 0,
            blocked_config: false,
            pr_state,
            provisioned_ms: Some(NOW - 100 * DAY_MS),
        }
    }

    #[test]
    fn a_restored_checkout_restarts_the_idle_clock() {
        // Thirty idle days of session records, but the checkout itself was
        // (re)built an hour ago — restore, not neglect. Not eligible until the
        // checkout has sat for the full period too.
        let mut c = idle_for(30);
        c.repos[0].provisioned_ms = Some(NOW - 60 * 60 * 1000);
        assert_eq!(eligible(&c, NOW, 7), None);
        c.repos[0].provisioned_ms = Some(NOW - 8 * DAY_MS);
        assert_eq!(eligible(&c, NOW, 7).map(|e| e.idle_days), Some(8));
        // The latest checkout governs for a multi-repo agent.
        c.repos.push(pushed(None));
        c.repos[1].provisioned_ms = Some(NOW - 2 * DAY_MS);
        assert_eq!(eligible(&c, NOW, 7), None);
        // No birth time reported: activity alone decides, as before.
        c.repos = vec![pushed(None)];
        c.repos[0].provisioned_ms = None;
        assert_eq!(eligible(&c, NOW, 7).map(|e| e.idle_days), Some(30));
    }

    /// Idle for `days`, clean, pushed, no PR.
    fn idle_for(days: i64) -> Candidate {
        Candidate {
            archived: false,
            run_owned: false,
            purpose_tagged: false,
            status: AgentStatus::Idle,
            last_activity_ms: Some(NOW - days * DAY_MS),
            created_at_ms: Some(NOW - 100 * DAY_MS),
            repos: vec![pushed(None)],
        }
    }

    #[test]
    fn zero_days_archives_nothing() {
        assert_eq!(eligible(&idle_for(400), NOW, 0), None);
    }

    #[test]
    fn under_the_threshold_waits_and_over_it_qualifies() {
        assert_eq!(eligible(&idle_for(6), NOW, 7), None);
        assert_eq!(
            eligible(&idle_for(8), NOW, 7),
            Some(Eligible {
                idle_days: 8,
                finished_pr: false
            })
        );
    }

    #[test]
    fn an_open_pr_follows_the_configured_threshold() {
        let mut c = idle_for(6);
        c.repos = vec![pushed(Some(PrStatus::Open))];
        assert_eq!(eligible(&c, NOW, 7), None);
        c = idle_for(8);
        c.repos = vec![pushed(Some(PrStatus::Open))];
        assert!(eligible(&c, NOW, 7).is_some());
    }

    #[test]
    fn a_merged_or_closed_pr_shortens_the_wait_to_the_grace() {
        for state in [PrStatus::Merged, PrStatus::Closed] {
            let mut c = idle_for(0);
            c.last_activity_ms = Some(NOW - 23 * 60 * 60 * 1000);
            c.repos = vec![pushed(Some(state))];
            assert_eq!(eligible(&c, NOW, 7), None, "{state:?} under a day");
            c.last_activity_ms = Some(NOW - DAY_MS);
            assert_eq!(
                eligible(&c, NOW, 7),
                Some(Eligible {
                    idle_days: 1,
                    finished_pr: true
                }),
                "{state:?} after a day"
            );
        }
    }

    #[test]
    fn a_dirty_tree_is_never_archived() {
        let mut c = idle_for(30);
        c.repos[0].clean_tree = false;
        assert_eq!(eligible(&c, NOW, 7), None);
    }

    #[test]
    fn unpushed_commits_are_never_archived() {
        let mut c = idle_for(30);
        c.repos[0].unpushed = 1;
        assert_eq!(eligible(&c, NOW, 7), None);
    }

    #[test]
    fn blocked_config_is_an_unreadable_tree() {
        let mut c = idle_for(30);
        c.repos[0].blocked_config = true;
        assert_eq!(eligible(&c, NOW, 7), None);
    }

    #[test]
    fn a_working_agent_is_skipped() {
        for status in [AgentStatus::Spawning, AgentStatus::Running] {
            let mut c = idle_for(30);
            c.status = status;
            assert_eq!(eligible(&c, NOW, 7), None);
        }
        for status in [AgentStatus::Idle, AgentStatus::Stopped, AgentStatus::Error] {
            let mut c = idle_for(30);
            c.status = status;
            assert!(eligible(&c, NOW, 7).is_some());
        }
    }

    #[test]
    fn hidden_and_already_archived_agents_are_skipped() {
        let mut c = idle_for(30);
        c.run_owned = true;
        assert_eq!(eligible(&c, NOW, 7), None);
        let mut c = idle_for(30);
        c.purpose_tagged = true;
        assert_eq!(eligible(&c, NOW, 7), None);
        let mut c = idle_for(30);
        c.archived = true;
        assert_eq!(eligible(&c, NOW, 7), None);
    }

    #[test]
    fn every_repo_must_pass() {
        let mut c = idle_for(30);
        c.repos = vec![pushed(Some(PrStatus::Merged)), pushed(None)];
        assert!(eligible(&c, NOW, 7).is_some());
        c.repos[1].unpushed = 2;
        assert_eq!(eligible(&c, NOW, 7), None);
    }

    #[test]
    fn nothing_verifiable_is_not_eligible() {
        let mut c = idle_for(30);
        c.repos.clear();
        assert_eq!(eligible(&c, NOW, 7), None);
    }

    #[test]
    fn no_session_records_falls_back_to_created_at() {
        let mut c = idle_for(30);
        c.last_activity_ms = None;
        c.created_at_ms = Some(NOW - 8 * DAY_MS);
        assert_eq!(eligible(&c, NOW, 7).map(|e| e.idle_days), Some(8));
        c.created_at_ms = Some(NOW - 6 * DAY_MS);
        assert_eq!(eligible(&c, NOW, 7), None);
        c.created_at_ms = None;
        assert_eq!(eligible(&c, NOW, 7), None);
    }

    #[test]
    fn setting_parses_with_a_default() {
        assert_eq!(parse_idle_days(None), DEFAULT_IDLE_DAYS);
        assert_eq!(parse_idle_days(Some("")), DEFAULT_IDLE_DAYS);
        assert_eq!(parse_idle_days(Some("abc")), DEFAULT_IDLE_DAYS);
        assert_eq!(parse_idle_days(Some("-1")), DEFAULT_IDLE_DAYS);
        assert_eq!(parse_idle_days(Some("0")), 0);
        assert_eq!(parse_idle_days(Some(" 14 ")), 14);
    }
}

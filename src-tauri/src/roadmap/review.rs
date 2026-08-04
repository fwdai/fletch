//! The settle review: one turn into the PM chat every time a roadmap run lands.
//!
//! Why this exists: until now the PM only ever saw the board it *asked* for.
//! It wrote the brief, the drainer built it, and whatever came back — a PR, a
//! silent ship, a failure — was known only to the user staring at the card. A
//! manager who never reads the work it commissioned cannot notice that the
//! implementation answered a different question than the ticket, which is the
//! one deviation nobody else is positioned to catch (the run passed its own
//! acceptance criteria; only the item's *intent* was missed).
//!
//! So the moment [`super::drainer::settle_project`] decides what a run did, it
//! asks here for a review turn: the item's brief plus the outcome, delivered
//! into the project's newest `roadmap-pm` chat as an ordinary user-role message.
//! The PM answers with `roadmap_note`s (attention, invariant 2's conservative
//! direction) and proposals (everything that advances state) — never by editing
//! the board itself.
//!
//! # The delivery seam
//!
//! A PM chat is an ordinary workspace session, so this goes through the exact
//! path the composer and `ChatPane` use: [`Supervisor::send_user_message`]. That
//! one call already owns every case this needs and none of them are ours to
//! re-derive — the agent is mid-turn (live injection or the durable
//! `pending_messages` queue, flushed at the next boundary), or resting after an
//! app restart (revived in `--resume` mode and flushed). Writing into
//! `pending_messages` directly would persist the message and then wait for
//! *something else* to trigger a flush, which is the half of the mechanism that
//! lives in the supervisor anyway.
//!
//! Consequence worth naming: a settled run can wake a resting PM session, which
//! is what makes the review land while the Roadmap tab is closed. It never
//! *spawns* a chat — a project with no PM chat gets a durable `note` instead
//! (see [`Plan::NoChat`]), so the review is deferred to the standup digest
//! rather than lost.
//!
//! # Locking
//!
//! `WorkspaceManager` is built on the same `Arc<Mutex<Connection>>` this module
//! is handed, and `parking_lot::Mutex` is not reentrant — so the decision
//! ([`plan`]) takes the lock, drops it, and only then is anything delivered.
//! Same discipline as every other roadmap write: DB under the lock, effects
//! after.

use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension};
use tauri::{AppHandle, Manager};

use super::drainer::{project_setting, FinalizedPr, Settlement};
use super::events::{self, EventActor, EventKind};
use super::types::RoadmapItem;
use super::{emit_item_event, Db};
use crate::supervisor::Supervisor;

/// `project_settings` key gating the review, read the way the drainer reads
/// `workflow.default`. Absent means on: a user who has never touched the dial
/// gets the oversight loop the product is about, and turning it off is the
/// explicit act.
const SETTLE_REVIEW_KEY: &str = "roadmap.settle_review";

/// The instruction line the review turn ends on — what the PM is being asked to
/// *do* with the outcome, as opposed to acknowledge. Both halves matter: a
/// deviation the user never hears about is a note nobody reads, and a roadmap
/// that should change is a proposal, never a direct edit.
const INSTRUCTION: &str =
    "Review this outcome against the item's intent. If it deviates, record a \
                           roadmap_note on the item and tell the user what you'd change; if the \
                           roadmap should change, propose it.";

/// What a settled run did, in the words the review turn uses. Narrower than
/// [`Settlement`] on purpose: the three things worth reviewing, with the payload
/// the PM needs to go look for itself.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    /// The run finished and opened a pull request, which is where the diff is.
    PrOpened(String),
    /// The run finished without a PR, so the item is done as far as this app can
    /// tell — nothing is coming for the PM to read.
    Shipped,
    /// The run failed, was canceled, or never started. Carries the drainer's own
    /// reason string, so the durable `run_failed` event and this turn agree.
    Failed(String),
}

impl Outcome {
    /// The outcome as one clause, the way the prompt's first line says it.
    fn line(&self) -> String {
        match self {
            Outcome::PrOpened(url) => format!("PR opened at {url}"),
            Outcome::Shipped => "shipped directly (the run finished without opening a PR)".into(),
            Outcome::Failed(why) => format!("failed: {why}"),
        }
    }
}

/// The outcome a settlement is worth reviewing as, or `None` for a run that is
/// still going (nothing has happened yet to review).
///
/// Mirrors [`super::drainer::settlement_event`] one-to-one, off the same two
/// inputs, so the item's durable history and the PM's turn can never describe
/// two different endings for one run.
pub(crate) fn outcome_for(settlement: &Settlement, pr: Option<&FinalizedPr>) -> Option<Outcome> {
    match settlement {
        Settlement::Running => None,
        // A PR-less `in_review` is impossible (`settle` only reaches it with a
        // PR), but the projection stays total rather than unwrapping.
        Settlement::InReview => Some(match pr {
            Some(p) => Outcome::PrOpened(p.url.clone()),
            None => Outcome::Shipped,
        }),
        Settlement::Done => Some(Outcome::Shipped),
        Settlement::Released(why) => Some(Outcome::Failed((*why).to_string())),
    }
}

/// The review turn's text: what was asked for, what came back, and the one
/// instruction.
///
/// Pure over the item and the outcome — no clock, no database, no app handle —
/// so the wording the PM actually receives is what the tests assert on.
///
/// Deliberately the *item's* brief rather than the run's
/// ([`super::drainer::build_brief`]): the question here is "did this answer the
/// ticket", so quoting the ticket is the whole point. The run's own additions
/// (what landed underneath it, the stamp-the-code instruction) would only be
/// noise in a review.
pub(crate) fn review_prompt(item: &RoadmapItem, outcome: &Outcome) -> String {
    let mut lines = vec![
        format!("{} settled — {}.", item.code, outcome.line()),
        String::new(),
        format!("{}: {}", item.code, item.title),
    ];
    if !item.why.trim().is_empty() {
        lines.push(String::new());
        lines.push(item.why.trim().to_string());
    }
    if !item.accept.is_empty() {
        lines.push(String::new());
        lines.push("Done when:".to_string());
        lines.extend(item.accept.iter().map(|a| format!("- {a}")));
    }
    lines.push(String::new());
    lines.push(INSTRUCTION.to_string());
    lines.join("\n")
}

/// What the drainer should do with a settled item's review, decided in one lock
/// scope so the setting and the chat it resolves are read off the same moment.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Plan {
    /// The project turned the review off. Nothing is sent and nothing is
    /// recorded — a dial the user set is not a failure to report.
    Off,
    /// The project has no PM chat to deliver into. The review becomes a durable
    /// note on the item, which the standup digest picks up whenever a chat does
    /// exist. We do *not* spawn one: a chat the user never asked for would start
    /// burning a context window on a board they may not be managing this way.
    NoChat,
    /// Deliver into this chat — the newest one, which is the conversation the
    /// Roadmap tab opens on.
    Deliver { agent_id: String },
}

/// Read the dial and resolve the target chat. Called with the connection lock
/// held; performs no delivery.
pub(crate) fn plan(conn: &Connection, project_id: &str) -> Plan {
    if !enabled(conn, project_id) {
        return Plan::Off;
    }
    match newest_pm_chat(conn, project_id) {
        Some(agent_id) => Plan::Deliver { agent_id },
        None => Plan::NoChat,
    }
}

/// Is the settle review on for this project? Absent is on; the off spellings are
/// the ones a checkbox or a hand-edited row would plausibly write.
fn enabled(conn: &Connection, project_id: &str) -> bool {
    match project_setting(conn, project_id, SETTLE_REVIEW_KEY) {
        None => true,
        Some(v) => !matches!(
            v.to_ascii_lowercase().as_str(),
            "false" | "0" | "off" | "no"
        ),
    }
}

/// The project's newest live PM chat, by the same filter and order the Roadmap
/// tab's picker lists (`purpose`-tagged, not archived, newest first) — so "the
/// chat this review lands in" is the conversation the user would open.
fn newest_pm_chat(conn: &Connection, project_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM workspaces
          WHERE project_id = ?1 AND purpose = ?2 AND archived_at IS NULL
          ORDER BY created_at DESC LIMIT 1",
        rusqlite::params![project_id, crate::workspace::PURPOSE_ROADMAP_PM],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Ask the PM to review one settled item. Best-effort by construction: every
/// path either delivers the turn or leaves a durable line on the card, and
/// neither can fail the settlement that called it.
pub(crate) fn request(app: &AppHandle, db: &Db, item: &RoadmapItem, outcome: &Outcome) {
    // Decided under the lock, acted on after it drops — the workspace manager
    // this delivery goes through holds the same non-reentrant mutex.
    let decision = {
        let conn = db.lock();
        plan(&conn, &item.project_id)
    };
    match decision {
        Plan::Off => {}
        Plan::NoChat => defer(app, db, item, outcome),
        Plan::Deliver { agent_id } => {
            if !deliver(app, &agent_id, &review_prompt(item, outcome)) {
                defer(app, db, item, outcome);
            }
        }
    }
}

/// Hand the turn to the PM's session. `true` means the message is the
/// supervisor's problem now — delivered as a turn, injected into a running one,
/// or persisted in `pending_messages` for the next boundary. Only an outright
/// refusal (no supervisor, no such workspace) is `false`, which is what makes
/// the fallback fire exactly when the review would otherwise vanish.
fn deliver(app: &AppHandle, agent_id: &str, prompt: &str) -> bool {
    let Some(sup) = app
        .try_state::<Arc<Supervisor>>()
        .map(|s| s.inner().clone())
    else {
        tracing::warn!("roadmap settle review: no supervisor to deliver through");
        return false;
    };
    // A fresh turn id: this is a first-class user-role turn in that chat, and
    // `insert_user_turn` is idempotent on it, so it must not collide with one the
    // frontend allocated.
    let turn_id = uuid::Uuid::new_v4().to_string();
    match sup.send_user_message(app, agent_id, &turn_id, prompt, &[]) {
        Ok(held) => {
            tracing::info!(agent_id, held, "roadmap settle review: review turn sent");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, agent_id, "roadmap settle review: delivery refused");
            false
        }
    }
}

/// No chat could take the review: record it on the item instead, so the outcome
/// is still waiting to be reviewed rather than silently unreviewed (invariant 3
/// — a deviation is a durable object, not a message that failed to send). The
/// standup digest reads the same trail, so the next PM session sees it.
fn defer(app: &AppHandle, db: &Db, item: &RoadmapItem, outcome: &Outcome) {
    let detail = format!("PM review pending: {}", outcome.line());
    let recorded = {
        let conn = db.lock();
        events::record(
            &conn,
            &item.id,
            &item.project_id,
            // The drainer is what noticed, and what could not hand it on.
            EventActor::Drainer,
            EventKind::Note,
            Some(&detail),
        )
    };
    match recorded {
        Ok(event) => emit_item_event(app, &event),
        // The row was deleted mid-tick, or the write failed; there is nothing
        // left to review either way.
        Err(e) => tracing::warn!(item = %item.code, error = %e, "roadmap settle review: \
                                  recording the deferred review failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::get_migrations;
    use crate::roadmap::store;
    use crate::roadmap::types::{ItemStatus, NewItem};

    fn test_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        get_migrations().to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'fletch', 0)",
            [],
        )
        .unwrap();
        conn
    }

    fn chat(conn: &Connection, id: &str, created_at: i64, purpose: Option<&str>) {
        conn.execute(
            "INSERT INTO workspaces (id, project_id, name, created_at, purpose)
             VALUES (?1, 'p1', ?1, ?2, ?3)",
            rusqlite::params![id, created_at, purpose],
        )
        .unwrap();
    }

    fn setting(conn: &Connection, value: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO project_settings (project_id, key, value)
             VALUES ('p1', ?1, ?2)",
            rusqlite::params![SETTLE_REVIEW_KEY, value],
        )
        .unwrap();
    }

    fn item() -> RoadmapItem {
        RoadmapItem {
            id: "i1".into(),
            project_id: "p1".into(),
            code: "MCA-104".into(),
            parent_id: None,
            title: "Add the queue drainer".into(),
            why: "queued items sit forever with nothing to launch them".into(),
            horizon: crate::roadmap::types::Horizon::Now,
            status: ItemStatus::InReview,
            rank: 1.0,
            area: None,
            source: crate::roadmap::types::ItemSource::Pm,
            accept: vec!["a queued item launches a run".into()],
            deps: Vec::new(),
            agent_id: None,
            workflow_def_id: None,
            run_id: None,
            pr_url: None,
            pr_number: None,
            hold_reason: None,
            held_by: None,
            held_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn pr() -> FinalizedPr {
        FinalizedPr {
            url: "https://github.com/o/r/pull/42".into(),
            number: Some(42),
        }
    }

    /// Every settlement the drainer can reach maps to the outcome the review
    /// turn names — and a still-running one to nothing at all.
    #[test]
    fn each_settlement_becomes_the_outcome_it_describes() {
        assert_eq!(outcome_for(&Settlement::Running, None), None);
        assert_eq!(
            outcome_for(&Settlement::InReview, Some(&pr())),
            Some(Outcome::PrOpened(
                "https://github.com/o/r/pull/42".to_string()
            ))
        );
        assert_eq!(outcome_for(&Settlement::Done, None), Some(Outcome::Shipped));
        assert_eq!(
            outcome_for(&Settlement::Released("its run failed"), None),
            Some(Outcome::Failed("its run failed".to_string()))
        );
        // The PR is what distinguishes review from shipped, so a review without
        // one degrades rather than panicking.
        assert_eq!(
            outcome_for(&Settlement::InReview, None),
            Some(Outcome::Shipped)
        );
    }

    /// The durable record and the review turn are two projections of one
    /// settlement, so they must agree on *whether* there is one: an outcome the
    /// PM is asked about but the card never records (or the reverse) would be two
    /// stories about one run.
    #[test]
    fn a_settlement_produces_a_review_exactly_when_it_produces_an_event() {
        use crate::roadmap::drainer::settlement_event;
        for (settlement, pr) in [
            (Settlement::Running, None),
            (Settlement::InReview, Some(pr())),
            (Settlement::Done, None),
            (Settlement::Released("its run failed"), None),
            (Settlement::Released("its run never started"), None),
        ] {
            assert_eq!(
                outcome_for(&settlement, pr.as_ref()).is_some(),
                settlement_event(&settlement, pr.as_ref()).is_some(),
                "{settlement:?}"
            );
        }
    }

    /// The PR case: the prompt opens with the outcome and the link, quotes the
    /// ticket the run was built from, and closes on the one instruction.
    #[test]
    fn a_pr_review_quotes_the_ticket_and_the_link() {
        let prompt = review_prompt(
            &item(),
            &Outcome::PrOpened("https://github.com/o/r/pull/42".into()),
        );
        let lines: Vec<&str> = prompt.lines().collect();
        assert_eq!(
            lines[0],
            "MCA-104 settled — PR opened at https://github.com/o/r/pull/42."
        );
        assert_eq!(lines[2], "MCA-104: Add the queue drainer");
        assert!(
            prompt.contains("queued items sit forever"),
            "the why is the intent being reviewed against: {prompt}"
        );
        assert!(
            prompt.contains("Done when:\n- a queued item launches a run"),
            "{prompt}"
        );
        assert!(prompt.ends_with(INSTRUCTION), "{prompt}");
        // The instruction names both durable outputs the PM may produce.
        assert!(prompt.contains("roadmap_note") && prompt.contains("propose it"));
    }

    /// A run that shipped without a PR says so plainly — there is no diff to go
    /// read, which is exactly what the PM needs to know.
    #[test]
    fn a_shipped_review_names_the_missing_pr() {
        let prompt = review_prompt(&item(), &Outcome::Shipped);
        assert_eq!(
            prompt.lines().next().unwrap(),
            "MCA-104 settled — shipped directly (the run finished without opening a PR)."
        );
        assert!(prompt.ends_with(INSTRUCTION));
    }

    /// A failure carries the drainer's own reason string, so this turn and the
    /// item's `run_failed` event tell one story.
    #[test]
    fn a_failed_review_carries_the_reason() {
        let prompt = review_prompt(&item(), &Outcome::Failed("its run was canceled".into()));
        assert_eq!(
            prompt.lines().next().unwrap(),
            "MCA-104 settled — failed: its run was canceled."
        );
    }

    /// A bare ticket (no why, no acceptance criteria) still produces a coherent
    /// turn rather than blank sections.
    #[test]
    fn a_bare_ticket_still_reviews() {
        let mut bare = item();
        bare.why = "   ".into();
        bare.accept = Vec::new();
        let prompt = review_prompt(&bare, &Outcome::Shipped);
        assert!(!prompt.contains("Done when"), "{prompt}");
        assert_eq!(
            prompt,
            format!(
                "MCA-104 settled — shipped directly (the run finished without opening a PR).\n\n\
                 MCA-104: Add the queue drainer\n\n{INSTRUCTION}"
            )
        );
    }

    /// The default is on, and the target is the *newest* PM chat — the one the
    /// Roadmap tab opens on. Chats of other purposes, archived ones, and other
    /// projects' are all invisible.
    #[test]
    fn the_plan_targets_the_newest_live_pm_chat_by_default() {
        let conn = test_conn();
        chat(&conn, "old-pm", 100, Some("roadmap-pm"));
        chat(&conn, "new-pm", 200, Some("roadmap-pm"));
        // A sidebar agent, and an archived PM chat: neither is a conversation
        // the user can be handed a review in.
        chat(&conn, "sidebar", 300, None);
        chat(&conn, "gone-pm", 400, Some("roadmap-pm"));
        conn.execute(
            "UPDATE workspaces SET archived_at = 500 WHERE id = 'gone-pm'",
            [],
        )
        .unwrap();

        assert_eq!(
            plan(&conn, "p1"),
            Plan::Deliver {
                agent_id: "new-pm".into()
            }
        );
    }

    /// No PM chat at all: the review is deferred to a durable note rather than
    /// spawning a conversation the user never asked for.
    #[test]
    fn a_project_without_a_pm_chat_defers() {
        let conn = test_conn();
        chat(&conn, "sidebar", 100, None);
        assert_eq!(plan(&conn, "p1"), Plan::NoChat);
    }

    /// The dial off suppresses the review entirely — no turn, and nothing
    /// recorded either.
    #[test]
    fn the_setting_off_suppresses_the_review() {
        let conn = test_conn();
        chat(&conn, "pm", 100, Some("roadmap-pm"));
        for off in ["false", "0", "off", "no", "FALSE", "Off"] {
            setting(&conn, off);
            assert_eq!(plan(&conn, "p1"), Plan::Off, "{off} should read as off");
        }
        // Anything else — including a blank the drainer's reader treats as
        // absent — leaves it on.
        for on in ["true", "1", "on", "  "] {
            setting(&conn, on);
            assert_eq!(
                plan(&conn, "p1"),
                Plan::Deliver {
                    agent_id: "pm".into()
                },
                "{on} should read as on"
            );
        }
    }

    /// The deferred note is a real event on the item, attributed to the drainer
    /// and naming the outcome — so nothing about the settlement is lost when
    /// there is nobody to review it.
    #[test]
    fn the_deferred_note_names_the_outcome() {
        let conn = test_conn();
        let it = store::create(
            &conn,
            "p1",
            &NewItem {
                title: "one".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let detail = format!(
            "PM review pending: {}",
            Outcome::PrOpened("https://github.com/o/r/pull/42".into()).line()
        );
        let event = events::record(
            &conn,
            &it.id,
            "p1",
            EventActor::Drainer,
            EventKind::Note,
            Some(&detail),
        )
        .unwrap();
        assert_eq!(event.kind, EventKind::Note);
        assert_eq!(event.actor, EventActor::Drainer);
        assert_eq!(
            event.detail.as_deref(),
            Some("PM review pending: PR opened at https://github.com/o/r/pull/42")
        );
    }
}

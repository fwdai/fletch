//! The agent-authored walkthrough of a workspace's changes: steps the agent
//! writes through the `tour_set` mailbox op (validated in `rpc::git`) and the
//! Code panel shows beside the diff, one at a time. One per workspace, newest
//! wins; it goes with the workspace row.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::Result;

use super::{now_millis, WorkspaceManager};

/// One place a step points at: a file of the workspace's changes and a
/// 1-indexed, inclusive line range in its current content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TourAnchor {
    pub path: String,
    pub start: u32,
    pub end: u32,
}

/// One step: a short note and the hunks it is about, across as many files as
/// the idea spans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TourStep {
    pub title: Option<String>,
    pub note: String,
    pub anchors: Vec<TourAnchor>,
}

/// A workspace's tour as the Code panel reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tour {
    pub agent_id: String,
    /// The user turn it was written in, when the host knew it.
    pub turn_id: Option<String>,
    pub steps: Vec<TourStep>,
    /// Epoch ms.
    pub created_at: i64,
}

impl WorkspaceManager {
    /// Store `steps` as workspace `id`'s tour, replacing any earlier one.
    pub fn set_tour(&self, id: &str, turn_id: Option<&str>, steps: &[TourStep]) -> Result<()> {
        let steps = serde_json::to_string(steps)?;
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "INSERT OR REPLACE INTO workspace_tours (workspace_id, turn_id, steps, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, turn_id, steps, now_millis()],
        )?;
        Ok(())
    }

    /// Workspace `id`'s tour, or `None` when its agent has not written one.
    pub fn get_tour(&self, id: &str) -> Result<Option<Tour>> {
        let conn = self.db.lock();
        let row: Option<(Option<String>, String, i64)> = conn
            .query_row(
                "SELECT turn_id, steps, created_at FROM workspace_tours WHERE workspace_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((turn_id, steps, created_at)) = row else {
            return Ok(None);
        };
        Ok(Some(Tour {
            agent_id: id.to_string(),
            turn_id,
            steps: serde_json::from_str(&steps)?,
            created_at,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{agent, exchange, test_db};
    use super::*;

    fn step(note: &str, path: &str) -> TourStep {
        TourStep {
            title: None,
            note: note.into(),
            anchors: vec![TourAnchor {
                path: path.into(),
                start: 1,
                end: 3,
            }],
        }
    }

    /// A tour round-trips with the turn it was written in, a later one replaces
    /// it outright, and it goes when its workspace does.
    #[test]
    fn a_tour_round_trips_is_replaced_by_the_next_and_goes_with_its_workspace() {
        let wm = WorkspaceManager::new(test_db());
        agent(&wm, "yosemite", "/r", None);
        assert_eq!(wm.get_tour("yosemite").unwrap(), None);

        let first = vec![step("The new column", "src/db.rs")];
        wm.set_tour("yosemite", Some("t1"), &first).unwrap();
        let tour = wm.get_tour("yosemite").unwrap().unwrap();
        assert_eq!(tour.agent_id, "yosemite");
        assert_eq!(tour.turn_id.as_deref(), Some("t1"));
        assert_eq!(tour.steps, first);
        assert!(tour.created_at > 0);

        let second = vec![
            TourStep {
                title: Some("Contract".into()),
                ..step("The type both halves share", "src/api.ts")
            },
            step("Its one caller", "src/ui.tsx"),
        ];
        wm.set_tour("yosemite", None, &second).unwrap();
        let tour = wm.get_tour("yosemite").unwrap().unwrap();
        assert_eq!(tour.turn_id, None);
        assert_eq!(tour.steps, second);

        assert!(wm.set_tour("nope", None, &first).is_err());

        wm.remove_agent("yosemite").unwrap();
        let rows: i64 = wm
            .db
            .lock()
            .query_row("SELECT COUNT(*) FROM workspace_tours", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    /// The turn a tour is credited to is the latest one that ran: a follow-up
    /// queued behind it has not, so it is not the one being explained.
    #[test]
    fn the_latest_turn_is_the_newest_that_started() {
        let wm = WorkspaceManager::new(test_db());
        agent(&wm, "yosemite", "/r", None);
        assert_eq!(wm.latest_user_turn_id("yosemite").unwrap(), None);

        exchange(&wm, "yosemite", "t1", "first");
        wm.mark_user_turn_started("t1", 1).unwrap();
        exchange(&wm, "yosemite", "t2", "second");
        wm.mark_user_turn_started("t2", 2).unwrap();
        wm.insert_user_turn("yosemite", "t3", "queued", &[])
            .unwrap();
        assert_eq!(
            wm.latest_user_turn_id("yosemite").unwrap().as_deref(),
            Some("t2")
        );
    }
}

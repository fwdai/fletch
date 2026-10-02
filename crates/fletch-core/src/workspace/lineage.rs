//! Session lineage — where a session's history comes from.
//!
//! A session's history is its parent's history below `parent_cut_seq`
//! (recursively), followed by its own records. History is stored once and
//! referenced, never copied: `session_records` and `session_user_turns` are
//! append-only, so a cut into a parent always means the same rows. Each
//! session keeps its own seq space, ingest offset, positional ids, turn
//! matching and usage, so every internal read stays own-session; only the
//! display reads here stitch a chain together, tagging what an ancestor
//! contributed as `inherited`.
//!
//! The operations: resolving an [`Anchor`] into the [`SessionLineage`] a new
//! session is created with (`insert_agent` writes it with the session row),
//! the stitched reads, and [`detach_children`] for every path that deletes
//! sessions. See docs/fork-and-rewind.md.

use rusqlite::OptionalExtension;

use super::sessions::{current_session_id, query_records};
use super::turns::query_turns;
use super::*;

/// Where a session's history branches off: its parent session, and the
/// exclusive `session_records.seq` (in the parent's own seq space) below which
/// it shows the parent's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLineage {
    pub parent_session_id: String,
    pub cut_seq: i64,
}

/// A point in a workspace's conversation. Named by a user turn
/// (`session_user_turns.turn_id`) rather than a position, so it means the same
/// thing however the history it sits in was assembled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor<'a> {
    /// The end of the current session, as of now.
    End,
    /// Everything before turn T's prompt.
    Before(&'a str),
    /// Everything through turn T and its reply, up to the next turn.
    Through(&'a str),
}

/// One session of a history chain, and the exclusive seq below which the chain
/// shows it: the cut its successor in the chain was created with, or `None` for
/// the chain's own session, which shows everything.
struct Link {
    session_id: String,
    bound: Option<i64>,
}

/// `session_id`'s history chain, root ancestor first and `session_id` last.
fn history_chain(conn: &Connection, session_id: &str) -> Result<Vec<Link>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE chain(id, parent, cut, bound, depth) AS (
             SELECT id, parent_session_id, parent_cut_seq, NULL, 0
               FROM sessions WHERE id = ?1
             UNION ALL
             SELECT s.id, s.parent_session_id, s.parent_cut_seq, c.cut, c.depth + 1
               FROM sessions s JOIN chain c ON s.id = c.parent
         )
         SELECT id, bound FROM chain ORDER BY depth DESC",
    )?;
    let links = stmt
        .query_map([session_id], |r| {
            Ok(Link {
                session_id: r.get(0)?,
                bound: r.get(1)?,
            })
        })?
        .collect::<std::result::Result<_, rusqlite::Error>>()?;
    Ok(links)
}

/// One past the session's last record: the cut that keeps all of it.
fn end_of(conn: &Connection, session_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM session_records WHERE session_id = ?1",
        [session_id],
        |r| r.get(0),
    )?)
}

impl WorkspaceManager {
    /// Resolve `anchor` in `workspace_id`'s conversation into the lineage a new
    /// session takes to continue from that point.
    ///
    /// The parent is the session that owns the anchor turn — an ancestor when
    /// the turn is inherited, which the new session then references directly.
    /// `Before(T)` cuts at T's prompt record. `Through(T)` cuts at the prompt
    /// record of the next turn after T, or at the end of T's session when no
    /// later turn has one: a later turn without a record is still running or
    /// was never delivered, so none of it is in the history to cut away. A cut
    /// never reaches past what this conversation shows of the parent, so
    /// whatever an ancestor did after this branch left it stays out. `End` cuts
    /// at the end of the current session.
    ///
    /// An anchor turn whose prompt hasn't been matched to a record yet is an
    /// error, never "everything": it has no position to cut at.
    pub fn resolve_anchor(&self, workspace_id: &str, anchor: Anchor) -> Result<SessionLineage> {
        let conn = self.db.lock();
        let current = current_session_id(&conn, workspace_id)
            .ok_or_else(|| Error::Other(format!("{workspace_id} has no session")))?;
        let (turn_id, through) = match anchor {
            Anchor::End => {
                return Ok(SessionLineage {
                    cut_seq: end_of(&conn, &current)?,
                    parent_session_id: current,
                })
            }
            Anchor::Before(turn_id) => (turn_id, false),
            Anchor::Through(turn_id) => (turn_id, true),
        };

        // The turn's session and its prompt record's seq (None = unmatched).
        let (origin, prompt_seq): (String, Option<i64>) = conn
            .query_row(
                "SELECT t.session_id, r.seq
                   FROM session_user_turns t
                   LEFT JOIN session_records r
                          ON r.session_id = t.session_id AND r.native_id = t.native_id
                  WHERE t.turn_id = ?1",
                [turn_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| Error::Other(format!("unknown turn {turn_id}")))?;
        let elsewhere = || {
            Error::Other(format!(
                "turn {turn_id} is not part of {workspace_id}'s conversation"
            ))
        };
        let bound = history_chain(&conn, &current)?
            .into_iter()
            .find(|link| link.session_id == origin)
            .ok_or_else(elsewhere)?
            .bound
            .unwrap_or(i64::MAX);
        let prompt_seq = prompt_seq.ok_or_else(|| {
            Error::Other("That message is still syncing. Try again in a moment.".into())
        })?;
        if prompt_seq >= bound {
            return Err(elsewhere());
        }

        let cut_seq = if through {
            let next_prompt: Option<i64> = conn.query_row(
                "SELECT MIN(r.seq)
                   FROM session_user_turns t
                   JOIN session_records r
                     ON r.session_id = t.session_id AND r.native_id = t.native_id
                  WHERE t.session_id = ?1 AND r.seq > ?2",
                rusqlite::params![origin, prompt_seq],
                |r| r.get(0),
            )?;
            match next_prompt {
                Some(seq) => seq,
                None => end_of(&conn, &origin)?,
            }
            .min(bound)
        } else {
            prompt_seq
        };
        Ok(SessionLineage {
            parent_session_id: origin,
            cut_seq,
        })
    }

    /// The current session's display history: each ancestor's records below
    /// its cut, root first, then the session's own, with the ancestors' tagged
    /// `inherited`. What the transcript command and the remote op serve;
    /// everything internal reads the own-session `read_session_records`.
    pub fn read_history_records(&self, workspace_id: &str) -> Result<Vec<SessionRecord>> {
        let conn = self.db.lock();
        let Some(current) = current_session_id(&conn, workspace_id) else {
            return Ok(vec![]);
        };
        let mut records = Vec::new();
        for link in history_chain(&conn, &current)? {
            records.extend(query_records(
                &conn,
                &link.session_id,
                link.bound.unwrap_or(i64::MAX),
                link.bound.is_some(),
            )?);
        }
        Ok(records)
    }

    /// The user turns of [`Self::read_history_records`], in the same order:
    /// each ancestor's turns whose prompt lies below its cut, then every turn
    /// of the session's own, pending ones included.
    pub fn read_history_turns(&self, workspace_id: &str) -> Result<Vec<UserTurn>> {
        let conn = self.db.lock();
        let Some(current) = current_session_id(&conn, workspace_id) else {
            return Ok(vec![]);
        };
        let mut turns = Vec::new();
        for link in history_chain(&conn, &current)? {
            turns.extend(query_turns(
                &conn,
                &link.session_id,
                link.bound,
                link.bound.is_some(),
            )?);
        }
        Ok(turns)
    }
}

/// Keep every surviving session's history whole before the `doomed`
/// workspaces, and by cascade their sessions, are deleted. Every path that
/// deletes workspaces calls this first, inside its own transaction; the parent
/// reference has no ON DELETE action, so a path that doesn't fails on the
/// foreign key instead of cutting a child's history short.
///
/// A doomed session that a surviving session inherits from is handed over to
/// that survivor's workspace rather than deleted, as a superseded session —
/// which is all an ancestor is. Nothing is copied or renumbered, so the
/// survivor's own seqs, positional ids and its children's cuts are untouched,
/// and its inherited turns keep their ids as anchors. This repeats until no
/// survivor points into the doomed set, so a doomed chain moves whole. What no
/// child shows goes with the deletion as it would have: the moved session's
/// records at or past every child's cut, its turns left without a record, and
/// its queued messages, which have no agent left to deliver to.
pub(super) fn detach_children(conn: &Connection, doomed: &[String]) -> Result<()> {
    if doomed.is_empty() {
        return Ok(());
    }
    let ids = (1..=doomed.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let inherited_from_doomed = format!(
        "SELECT parent.id, child.workspace_id
           FROM sessions parent
           JOIN sessions child ON child.parent_session_id = parent.id
          WHERE parent.workspace_id IN ({ids}) AND child.workspace_id NOT IN ({ids})
          ORDER BY child.created_at
          LIMIT 1"
    );
    loop {
        let next: Option<(String, String)> = conn
            .query_row(
                &inherited_from_doomed,
                rusqlite::params_from_iter(doomed),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((session, heir)) = next else {
            return Ok(());
        };
        conn.execute(
            "UPDATE sessions SET workspace_id = ?2, superseded_at = COALESCE(superseded_at, ?3)
             WHERE id = ?1",
            rusqlite::params![session, heir, now_millis()],
        )?;
        conn.execute(
            "DELETE FROM session_records WHERE session_id = ?1
               AND seq >= (SELECT MAX(parent_cut_seq) FROM sessions WHERE parent_session_id = ?1)",
            [&session],
        )?;
        conn.execute(
            "DELETE FROM session_user_turns WHERE session_id = ?1
               AND (native_id IS NULL
                    OR native_id NOT IN (SELECT native_id FROM session_records WHERE session_id = ?1))",
            [&session],
        )?;
        conn.execute(
            "DELETE FROM pending_messages WHERE session_id = ?1",
            [&session],
        )?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::{mark_archived, mk_repo, seed_repo, test_db};
    use serde_json::json;

    /// A workspace `id` (on repo `repo`), whose session continues `lineage`.
    fn agent(wm: &WorkspaceManager, id: &str, repo: &str, lineage: Option<SessionLineage>) {
        let mut rec = new_agent_record(
            id.into(),
            id.into(),
            "claude".into(),
            mk_repo(repo),
            "task".into(),
            AgentView::Custom,
        );
        rec.lineage = lineage;
        wm.add_agent(&mut rec).unwrap();
    }

    /// One exchange in `ws`'s current session, as turn-end ingest leaves it: the
    /// sent turn, its prompt and reply records (`{turn}-u`, `{turn}-a`), matched.
    fn exchange(wm: &WorkspaceManager, ws: &str, turn: &str, text: &str) {
        wm.insert_user_turn(ws, turn, text, &[]).unwrap();
        let prompt = json!({"type": "user", "text": text});
        let reply = json!({"type": "assistant", "text": format!("re {text}")});
        wm.append_session_records(
            ws,
            "claude",
            "transcript",
            None,
            &[
                (format!("{turn}-u").as_str(), &prompt),
                (format!("{turn}-a").as_str(), &reply),
            ],
        )
        .unwrap();
        wm.associate_pending_user_turns(ws).unwrap();
    }

    fn session_of(wm: &WorkspaceManager, ws: &str) -> String {
        current_session_id(&wm.db.lock(), ws).unwrap()
    }

    /// `(native_id, inherited)` of the workspace's stitched records.
    fn history(wm: &WorkspaceManager, ws: &str) -> Vec<(String, bool)> {
        wm.read_history_records(ws)
            .unwrap()
            .into_iter()
            .map(|r| (r.native_id, r.inherited))
            .collect()
    }

    /// `(turn_id, inherited)` of the workspace's stitched turns.
    fn history_turns(wm: &WorkspaceManager, ws: &str) -> Vec<(String, bool)> {
        wm.read_history_turns(ws)
            .unwrap()
            .into_iter()
            .map(|t| (t.turn_id, t.inherited))
            .collect()
    }

    fn owned(ids: &[&str]) -> Vec<(String, bool)> {
        ids.iter().map(|id| (id.to_string(), false)).collect()
    }

    fn inherited(ids: &[&str]) -> Vec<(String, bool)> {
        ids.iter().map(|id| (id.to_string(), true)).collect()
    }

    fn concat(parts: &[Vec<(String, bool)>]) -> Vec<(String, bool)> {
        parts.concat()
    }

    /// Root `a` (alpha, bravo, charlie); `b` forked through alpha, then ran
    /// delta and echo; `c` forked `b` through delta, then ran foxtrot.
    fn three_level_chain() -> WorkspaceManager {
        let db = test_db();
        seed_repo(&db, "/r");
        let wm = WorkspaceManager::new(db);
        agent(&wm, "a", "/r", None);
        exchange(&wm, "a", "a1", "alpha");
        exchange(&wm, "a", "a2", "bravo");
        exchange(&wm, "a", "a3", "charlie");

        let b = wm.resolve_anchor("a", Anchor::Through("a1")).unwrap();
        agent(&wm, "b", "/r", Some(b));
        exchange(&wm, "b", "b1", "delta");
        exchange(&wm, "b", "b2", "echo");

        let c = wm.resolve_anchor("b", Anchor::Through("b1")).unwrap();
        agent(&wm, "c", "/r", Some(c));
        exchange(&wm, "c", "c1", "foxtrot");
        wm
    }

    #[test]
    fn through_cuts_at_the_next_turns_prompt() {
        let wm = three_level_chain();
        // a: a1-u(1) a1-a(2) a2-u(3) a2-a(4) a3-u(5) a3-a(6).
        let cut = wm.resolve_anchor("a", Anchor::Through("a2")).unwrap();
        assert_eq!(
            cut,
            SessionLineage {
                parent_session_id: session_of(&wm, "a"),
                cut_seq: 5
            }
        );
    }

    #[test]
    fn before_cuts_at_the_turns_own_prompt() {
        let wm = three_level_chain();
        let cut = wm.resolve_anchor("a", Anchor::Before("a2")).unwrap();
        assert_eq!(cut.cut_seq, 3);
        assert_eq!(cut.parent_session_id, session_of(&wm, "a"));
    }

    #[test]
    fn through_the_last_turn_keeps_the_whole_session() {
        let wm = three_level_chain();
        assert_eq!(
            wm.resolve_anchor("a", Anchor::Through("a3"))
                .unwrap()
                .cut_seq,
            7
        );
        // A later turn that never reached the transcript (still running, or a
        // failed send) bounds nothing.
        wm.insert_user_turn("a", "a4", "never landed", &[]).unwrap();
        assert_eq!(
            wm.resolve_anchor("a", Anchor::Through("a3"))
                .unwrap()
                .cut_seq,
            7
        );
    }

    #[test]
    fn end_cuts_after_the_current_sessions_last_record() {
        let wm = three_level_chain();
        let cut = wm.resolve_anchor("b", Anchor::End).unwrap();
        assert_eq!(
            cut,
            SessionLineage {
                parent_session_id: session_of(&wm, "b"),
                cut_seq: 5
            }
        );
    }

    #[test]
    fn an_unmatched_turn_is_an_error_not_everything() {
        let wm = three_level_chain();
        wm.insert_user_turn("a", "a4", "still running", &[])
            .unwrap();
        let err = wm
            .resolve_anchor("a", Anchor::Through("a4"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("still syncing"), "{err}");
        assert!(wm.resolve_anchor("a", Anchor::Before("a4")).is_err());
    }

    #[test]
    fn a_turn_outside_the_conversation_is_rejected() {
        let wm = three_level_chain();
        // c1 belongs to a descendant of `a`, not to `a`'s history.
        assert!(wm.resolve_anchor("a", Anchor::Through("c1")).is_err());
        // a2 is in `b`'s parent, but past the cut `b` branched at.
        assert!(wm.resolve_anchor("b", Anchor::Through("a2")).is_err());
        assert!(wm.resolve_anchor("a", Anchor::Through("nope")).is_err());
    }

    #[test]
    fn an_inherited_anchor_references_the_ancestor_that_owns_it() {
        let wm = three_level_chain();
        // From `c`, alpha is inherited from `a` through `b`.
        let cut = wm.resolve_anchor("c", Anchor::Through("a1")).unwrap();
        assert_eq!(
            cut,
            SessionLineage {
                parent_session_id: session_of(&wm, "a"),
                cut_seq: 3
            }
        );
        let cut = wm.resolve_anchor("c", Anchor::Through("b1")).unwrap();
        assert_eq!(cut.parent_session_id, session_of(&wm, "b"));
        assert_eq!(cut.cut_seq, 3);
    }

    #[test]
    fn an_inherited_anchor_never_reaches_past_what_the_branch_shows() {
        let db = test_db();
        seed_repo(&db, "/r");
        let wm = WorkspaceManager::new(db);
        agent(&wm, "a", "/r", None);
        exchange(&wm, "a", "a1", "alpha");
        let at_end = wm.resolve_anchor("a", Anchor::End).unwrap();
        assert_eq!(at_end.cut_seq, 3);
        agent(&wm, "b", "/r", Some(at_end));
        // `a` carries on after `b` branched: a late reply, then a new turn.
        wm.append_session_records("a", "claude", "transcript", None, &[("late", &json!({}))])
            .unwrap();
        exchange(&wm, "a", "a2", "bravo");

        // Through alpha from `a` itself runs up to bravo's prompt …
        assert_eq!(
            wm.resolve_anchor("a", Anchor::Through("a1"))
                .unwrap()
                .cut_seq,
            4
        );
        // … but from `b`, which never showed the late reply, it stops at b's cut.
        assert_eq!(
            wm.resolve_anchor("b", Anchor::Through("a1"))
                .unwrap()
                .cut_seq,
            3
        );
    }

    #[test]
    fn history_stitches_a_three_level_chain() {
        let wm = three_level_chain();
        assert_eq!(
            history(&wm, "c"),
            concat(&[
                inherited(&["a1-u", "a1-a", "b1-u", "b1-a"]),
                owned(&["c1-u", "c1-a"]),
            ])
        );
        assert_eq!(
            history_turns(&wm, "c"),
            concat(&[inherited(&["a1", "b1"]), owned(&["c1"])])
        );
        assert_eq!(
            history(&wm, "b"),
            concat(&[
                inherited(&["a1-u", "a1-a"]),
                owned(&["b1-u", "b1-a", "b2-u", "b2-a"]),
            ])
        );
        // The root's history is just its own.
        assert_eq!(history(&wm, "a").len(), 6);
        assert!(history(&wm, "a").iter().all(|(_, inh)| !inh));
    }

    #[test]
    fn history_turns_keep_own_pending_turns_but_not_an_ancestors() {
        let wm = three_level_chain();
        wm.insert_user_turn("c", "c2", "in flight", &[]).unwrap();
        let end = wm.resolve_anchor("c", Anchor::End).unwrap();
        agent(&wm, "d", "/r", Some(end));
        // `d` shows c's matched turns only; a pending turn has no position.
        assert_eq!(history_turns(&wm, "d"), inherited(&["a1", "b1", "c1"]));
        // `c` itself still shows its pending turn, for retry.
        assert_eq!(
            history_turns(&wm, "c").last().unwrap(),
            &("c2".to_string(), false)
        );
    }

    #[test]
    fn a_child_is_created_by_reference_with_nothing_copied() {
        let wm = three_level_chain();
        // Own-session reads see nothing of the parent: ingestion, positional
        // ids, last_activity and usage all start from zero.
        let lineage = wm.resolve_anchor("b", Anchor::Through("b1")).unwrap();
        agent(&wm, "fresh", "/r", Some(lineage.clone()));
        assert!(wm.read_session_records("fresh").unwrap().is_empty());
        assert_eq!(wm.session_record_count("fresh").unwrap(), 0);
        assert_eq!(wm.last_activity("fresh"), None);
        assert_eq!(wm.agent("fresh").unwrap().lineage, Some(lineage));
        assert_eq!(
            history(&wm, "fresh"),
            inherited(&["a1-u", "a1-a", "b1-u", "b1-a"])
        );

        // Its first own record starts its own seq space.
        exchange(&wm, "fresh", "f1", "golf");
        let own = wm.read_session_records("fresh").unwrap();
        assert_eq!(own.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![1, 2]);
    }

    // ── detaching children on delete ─────────────────────────────────────

    fn workspace_of(wm: &WorkspaceManager, session: &str) -> (String, Option<i64>) {
        wm.db
            .lock()
            .query_row(
                "SELECT workspace_id, superseded_at FROM sessions WHERE id = ?1",
                [session],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }

    #[test]
    fn deleting_an_ancestor_without_detaching_fails_loudly() {
        let wm = three_level_chain();
        let err = wm
            .db
            .lock()
            .execute("DELETE FROM workspaces WHERE id = 'b'", [])
            .unwrap_err();
        assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
    }

    #[test]
    fn discarding_an_ancestor_leaves_its_descendants_whole() {
        let wm = three_level_chain();
        let (a, b, c) = (
            session_of(&wm, "a"),
            session_of(&wm, "b"),
            session_of(&wm, "c"),
        );
        let before = history(&wm, "c");
        let count_c = wm.session_record_count("c").unwrap();

        wm.remove_agent("b").unwrap();

        assert!(wm.agent("b").is_err(), "the workspace is gone");
        assert_eq!(history(&wm, "c"), before, "c's history is untouched");
        let (home, superseded) = workspace_of(&wm, &b);
        assert_eq!(home, "c", "b's session now lives with its heir");
        assert!(
            superseded.is_some(),
            "… as an ancestor, not its current session"
        );
        assert_eq!(session_of(&wm, "c"), c);
        // b's records past every child's cut went with the discard.
        assert_eq!(
            wm.db
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM session_records WHERE session_id = ?1",
                    [&b],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        // Inherited turns keep their ids, so they still anchor.
        assert_eq!(
            wm.resolve_anchor("c", Anchor::Through("b1"))
                .unwrap()
                .parent_session_id,
            b
        );
        // c's own seq space and positional ids are unaffected.
        assert_eq!(wm.session_record_count("c").unwrap(), count_c);
        exchange(&wm, "c", "c2", "golf");
        assert_eq!(
            wm.read_session_records("c").unwrap().last().map(|r| r.seq),
            Some(count_c as i64 + 2)
        );

        // Discarding the root hands it to the workspace that now holds its
        // child.
        wm.remove_agent("a").unwrap();
        assert_eq!(workspace_of(&wm, &a).0, "c");
        assert_eq!(&history(&wm, "c")[..4], &before[..4]);
    }

    #[test]
    fn evicting_a_recycled_name_detaches_its_forks() {
        let db = test_db();
        seed_repo(&db, "/r");
        let wm = WorkspaceManager::new(db.clone());
        agent(&wm, "kilimanjaro", "/r", None);
        exchange(&wm, "kilimanjaro", "k1", "alpha");
        let old = session_of(&wm, "kilimanjaro");
        let lineage = wm.resolve_anchor("kilimanjaro", Anchor::End).unwrap();
        agent(&wm, "fork", "/r", Some(lineage));
        mark_archived(&db, "kilimanjaro");

        // Reusing the archived name evicts its row — but not the fork's history.
        agent(&wm, "kilimanjaro", "/r", None);

        assert_eq!(history(&wm, "fork"), inherited(&["k1-u", "k1-a"]));
        assert_eq!(workspace_of(&wm, &old).0, "fork");
        assert!(
            history(&wm, "kilimanjaro").is_empty(),
            "the new agent starts fresh"
        );
    }

    #[test]
    fn deleting_a_project_detaches_forks_outside_it() {
        let db = test_db();
        seed_repo(&db, "/r");
        seed_repo(&db, "/q");
        let wm = WorkspaceManager::new(db.clone());
        agent(&wm, "a", "/r", None);
        exchange(&wm, "a", "a1", "alpha");
        let b_cut = wm.resolve_anchor("a", Anchor::End).unwrap();
        agent(&wm, "b", "/r", Some(b_cut));
        exchange(&wm, "b", "b1", "bravo");
        // Same project: goes with it. Another project: survives.
        let inside = wm.resolve_anchor("b", Anchor::End).unwrap();
        agent(&wm, "inside", "/r", Some(inside));
        let outside = wm.resolve_anchor("b", Anchor::End).unwrap();
        agent(&wm, "outside", "/q", Some(outside));
        let before = history(&wm, "outside");

        let pid: String = db
            .lock()
            .query_row("SELECT project_id FROM repos WHERE path = '/r'", [], |r| {
                r.get(0)
            })
            .unwrap();
        wm.delete_project(&pid, &[]).unwrap();

        for gone in ["a", "b", "inside"] {
            assert!(wm.agent(gone).is_err(), "{gone} goes with its project");
        }
        assert_eq!(history(&wm, "outside"), before);
        let held: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE workspace_id = 'outside'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(held, 3, "outside's own session plus the chain it inherits");
    }
}

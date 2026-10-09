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
//! session is created with (`insert_agent` and `start_session` write it with
//! the session row), the stitched reads, whether a workspace ran a turn's
//! session itself (`turn_is_own`), and [`detach_children`] for every path that
//! deletes sessions. See docs/fork-and-rewind.md.

use rusqlite::OptionalExtension;

use super::sessions::{current_session_id, query_newest_records};
use super::turns::{query_turns, TURN_POSITION};
use super::*;

/// The records a history page holds when the caller names no `limit`: enough
/// for a phone to show the last few turns, a sliver of a long session.
pub const HISTORY_PAGE_DEFAULT: usize = 200;
/// The most a history page holds whatever the caller asks for, so no request
/// can turn the page read back into the whole-history one.
pub const HISTORY_PAGE_MAX: usize = 500;

/// One page of a display history, newest page first: its records in display
/// order, and the cursor naming the page before it — `None` once nothing
/// older is left. The cursor is opaque to callers.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryPage {
    pub records: Vec<SessionRecord>,
    pub older: Option<String>,
}

/// Where the next older page ends: link `index` of the history chain (root
/// first) and the exclusive seq, in that link's own seq space, below which it
/// reads. Seqs are per session, so a seq alone could not name a position in a
/// stitched history. Indexed from the root because records appended to the
/// current session leave every link's index alone; a session switch between
/// two page reads changes the chain itself, and the client starts over from
/// the newest page then anyway. Spelled `"<index>:<below>"` on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cursor {
    index: usize,
    below: i64,
}

impl Cursor {
    /// Strict: digits on both sides of one colon and nothing else, so a cursor
    /// that was mangled or made up is an error rather than some other page.
    fn parse(text: &str) -> Result<Self> {
        let bad = || Error::Other(format!("bad history cursor {text:?}"));
        let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        let (index, below) = text.split_once(':').ok_or_else(bad)?;
        if !digits(index) || !digits(below) {
            return Err(bad());
        }
        Ok(Self {
            index: index.parse().map_err(|_| bad())?,
            below: below.parse().map_err(|_| bad())?,
        })
    }

    fn encode(self) -> String {
        format!("{}:{}", self.index, self.below)
    }
}

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

impl Link {
    /// What the chain shows of this session below `below`: its records under
    /// both `below` and its bound, tagged `inherited` when it has a bound (an
    /// ancestor) — the newest `limit` of them, or all for `None`, in seq order.
    fn records(
        &self,
        conn: &Connection,
        below: i64,
        limit: Option<usize>,
    ) -> Result<Vec<SessionRecord>> {
        query_newest_records(
            conn,
            &self.session_id,
            self.bound.map_or(below, |bound| bound.min(below)),
            self.bound.is_some(),
            limit,
        )
    }
}

/// The records a chain shows, root first: each session's below its bound,
/// tagged `inherited`, and all of an unbounded one's (the chain's own).
fn chain_records(conn: &Connection, links: &[Link]) -> Result<Vec<SessionRecord>> {
    let mut records = Vec::new();
    for link in links {
        records.extend(link.records(conn, i64::MAX, None)?);
    }
    Ok(records)
}

/// One past the session's last record: the cut that keeps all of it.
fn end_of(conn: &Connection, session_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM transcripts.session_records WHERE session_id = ?1",
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
    /// `Before(T)` cuts at T's position: its prompt record, or for a turn that
    /// ended without the provider logging its prompt, just past the records
    /// that existed when it was sent (`TURN_POSITION`). `Through(T)` cuts at
    /// the position of the next turn after T, or at the end of T's session when
    /// no later turn has one: a later turn without one is still running or was
    /// never delivered, so none of it is in the history to cut away. A cut
    /// never reaches past what this conversation shows of the parent, so
    /// whatever an ancestor did after this branch left it stays out. `End` cuts
    /// at the end of the current session.
    ///
    /// An anchor turn still in flight with no record yet is an error, never
    /// "everything": its echo may still arrive, so it has no position to cut at.
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

        // The turn's session, its position (None = none yet), and whether it
        // has ended.
        let (origin, prompt_seq, ended): (String, Option<i64>, bool) = conn
            .query_row(
                &format!(
                    "SELECT t.session_id, {TURN_POSITION}, t.ended_at IS NOT NULL
                       FROM session_user_turns t
                       LEFT JOIN transcripts.session_records r
                              ON r.session_id = t.session_id AND r.native_id = t.native_id
                      WHERE t.turn_id = ?1"
                ),
                [turn_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
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
            Error::Other(if ended {
                "That message has no place in the history to cut at.".into()
            } else {
                "That message is still syncing. Try again in a moment.".into()
            })
        })?;
        if prompt_seq >= bound {
            return Err(elsewhere());
        }

        let cut_seq = if through {
            // Two turns share a position only when one is placed by its
            // watermark (`TURN_POSITION`). A placed later turn has no record to
            // cut away, so it never ends this one there; a prompt record at this
            // turn's own placed position does, and this turn goes with it.
            let next_prompt: Option<i64> = conn.query_row(
                &format!(
                    "SELECT MIN(pos) FROM (
                         SELECT {TURN_POSITION} AS pos, t.native_id AS nid
                           FROM session_user_turns t
                           LEFT JOIN transcripts.session_records r
                                  ON r.session_id = t.session_id AND r.native_id = t.native_id
                          WHERE t.session_id = ?1 AND t.turn_id != ?3
                     )
                     WHERE pos > ?2 OR (pos = ?2 AND nid IS NOT NULL)"
                ),
                rusqlite::params![origin, prompt_seq, turn_id],
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

    /// The turn delivered after `turn_id` in its own session, as `(workspace,
    /// turn)`: the workspace is the one that session belongs to, so its
    /// checkouts hold that turn's checkpoint — the code `turn_id`'s reply left
    /// behind. `None` when no turn followed it.
    pub fn turn_after(&self, turn_id: &str) -> Result<Option<(String, String)>> {
        let conn = self.db.lock();
        Ok(conn
            .query_row(
                "SELECT s.workspace_id, n.turn_id
                   FROM session_user_turns t
                   JOIN sessions s ON s.id = t.session_id
                   JOIN session_user_turns n ON n.session_id = t.session_id AND n.seq > t.seq
                  WHERE t.turn_id = ?1
                  ORDER BY n.seq
                  LIMIT 1",
                [turn_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Whether `workspace_id` ran the session turn `turn_id` ran in itself —
    /// its current session or one it rewound away from — rather than
    /// inheriting it from another workspace (a fork's parent, or a session
    /// `detach_children` handed it, told apart by age as in
    /// `read_superseded_records`). Only then are the turn's checkpoints in
    /// this workspace's checkouts.
    pub fn turn_is_own(&self, workspace_id: &str, turn_id: &str) -> Result<bool> {
        let conn = self.db.lock();
        conn.query_row(
            "SELECT s.workspace_id = w.id AND s.created_at >= w.created_at
               FROM session_user_turns t
               JOIN sessions s ON s.id = t.session_id
               JOIN workspaces w ON w.id = ?2
              WHERE t.turn_id = ?1",
            [turn_id, workspace_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| Error::Other(format!("unknown turn {turn_id}")))
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
        chain_records(&conn, &history_chain(&conn, &current)?)
    }

    /// One page of [`Self::read_history_records`], walking back from the
    /// newest: the last `limit` records before `before` (from the end when
    /// `None`), in the same order and with the same `inherited` tags, crossing
    /// into the previous link of the chain when one runs out. `limit` defaults
    /// to [`HISTORY_PAGE_DEFAULT`] and is clamped to `1..=`[`HISTORY_PAGE_MAX`].
    /// What lets a phone show the last turn of a session tens of MB long
    /// without shipping the rest of it.
    pub fn read_history_page(
        &self,
        workspace_id: &str,
        before: Option<&str>,
        limit: Option<usize>,
    ) -> Result<HistoryPage> {
        let limit = limit
            .unwrap_or(HISTORY_PAGE_DEFAULT)
            .clamp(1, HISTORY_PAGE_MAX);
        let before = before.map(Cursor::parse).transpose()?;
        let conn = self.db.lock();
        let empty = HistoryPage {
            records: vec![],
            older: None,
        };
        let Some(current) = current_session_id(&conn, workspace_id) else {
            return Ok(empty);
        };
        let links = history_chain(&conn, &current)?;
        let Some(last) = links.len().checked_sub(1) else {
            return Ok(empty);
        };
        let start = match before {
            None => Cursor {
                index: last,
                below: i64::MAX,
            },
            Some(cursor) if cursor.index <= last => cursor,
            Some(cursor) => {
                return Err(Error::Other(format!(
                    "history cursor {:?} is past {workspace_id}'s chain",
                    cursor.encode()
                )))
            }
        };

        // Newest first, one past the page: the extra record is how a full
        // page knows whether anything older is left to point at.
        let mut newest_first: Vec<(usize, SessionRecord)> = Vec::new();
        for index in (0..=start.index).rev() {
            let below = if index == start.index {
                start.below
            } else {
                i64::MAX
            };
            let want = limit + 1 - newest_first.len();
            let records = links[index].records(&conn, below, Some(want))?;
            newest_first.extend(records.into_iter().rev().map(|r| (index, r)));
            if newest_first.len() > limit {
                break;
            }
        }
        let older = if newest_first.len() > limit {
            newest_first.truncate(limit);
            newest_first.last().map(|(index, r)| {
                Cursor {
                    index: *index,
                    below: r.seq,
                }
                .encode()
            })
        } else {
            None
        };
        Ok(HistoryPage {
            records: newest_first.into_iter().rev().map(|(_, r)| r).collect(),
            older,
        })
    }

    /// The history a session continuing `lineage` shows before it has records
    /// of its own: what `read_history_records` reads for it, all inherited.
    /// The conversation a new session continues, read before it exists.
    pub fn read_lineage_records(&self, lineage: &SessionLineage) -> Result<Vec<SessionRecord>> {
        let conn = self.db.lock();
        let mut links = history_chain(&conn, &lineage.parent_session_id)?;
        if let Some(parent) = links.last_mut() {
            parent.bound = Some(lineage.cut_seq);
        }
        chain_records(&conn, &links)
    }

    /// The user turns of [`Self::read_history_records`], in the same order:
    /// each ancestor's turns positioned below its cut (`TURN_POSITION`), then
    /// every turn of the session's own, pending ones included.
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
/// its queued messages, which have no agent left to deliver to. The records
/// are in the attached file and are not touched here: the returned
/// `(session, cut)` pairs are for `TranscriptCleanup`, once the caller's
/// transaction has committed.
pub(super) fn detach_children(conn: &Connection, doomed: &[String]) -> Result<Vec<(String, i64)>> {
    let mut trims = Vec::new();
    if doomed.is_empty() {
        return Ok(trims);
    }
    let ids = sessions::placeholders(doomed.len());
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
            return Ok(trims);
        };
        conn.execute(
            "UPDATE sessions SET workspace_id = ?2, superseded_at = COALESCE(superseded_at, ?3)
             WHERE id = ?1",
            rusqlite::params![session, heir, now_millis()],
        )?;
        let cut: Option<i64> = conn.query_row(
            "SELECT MAX(parent_cut_seq) FROM sessions WHERE parent_session_id = ?1",
            [&session],
            |r| r.get(0),
        )?;
        // Turns are judged against the records that will survive the trim.
        conn.execute(
            "DELETE FROM session_user_turns WHERE session_id = ?1
               AND (native_id IS NULL
                    OR native_id NOT IN (SELECT native_id FROM transcripts.session_records
                                          WHERE session_id = ?1 AND seq < ?2))",
            rusqlite::params![session, cut.unwrap_or(i64::MAX)],
        )?;
        if let Some(cut) = cut {
            trims.push((session.clone(), cut));
        }
        conn.execute(
            "DELETE FROM pending_messages WHERE session_id = ?1",
            [&session],
        )?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::{
        agent, concat, exchange, history, history_turns, inherited, mark_archived, owned,
        seed_repo, session_of, test_db,
    };
    use serde_json::json;

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
    fn the_turn_after_is_the_next_one_delivered_in_the_same_session() {
        let wm = three_level_chain();
        let after = |turn| wm.turn_after(turn).unwrap();
        assert_eq!(after("a1"), Some(("a".into(), "a2".into())));
        assert_eq!(after("b1"), Some(("b".into(), "b2".into())));
        // Nothing followed a3 in `a`, whatever its descendants did.
        assert_eq!(after("a3"), None);
        // A delivered turn counts before its prompt is matched: its checkpoint
        // was taken as it went out.
        wm.insert_user_turn("a", "a4", "in flight", &[]).unwrap();
        assert_eq!(after("a3"), Some(("a".into(), "a4".into())));
        assert_eq!(after("nope"), None);
    }

    #[test]
    fn a_turns_session_is_the_workspaces_own_only_if_it_ran_it() {
        let wm = three_level_chain();
        let own = |ws: &str, turn: &str| wm.turn_is_own(ws, turn).unwrap();
        assert!(own("c", "c1"));
        assert!(!own("c", "b1"), "inherited from a fork's parent");
        assert!(!own("c", "a1"));
        assert!(own("a", "a1"));

        // A session it rewound away from is still its own.
        let before = wm.resolve_anchor("c", Anchor::Before("c1")).unwrap();
        wm.start_session("c", &before, None).unwrap();
        assert!(own("c", "c1"));
        assert!(wm.turn_is_own("c", "nope").is_err());
    }

    /// What a new session continuing a lineage will show, read before it
    /// exists, is what it shows once it does.
    #[test]
    fn a_lineages_records_are_the_history_its_session_shows() {
        let wm = three_level_chain();
        for (ws, anchor) in [
            ("c", Anchor::Through("b1")),
            ("c", Anchor::Before("c1")),
            ("c", Anchor::End),
            ("a", Anchor::Through("a2")),
        ] {
            let lineage = wm.resolve_anchor(ws, anchor).unwrap();
            let read = wm.read_lineage_records(&lineage).unwrap();
            agent(&wm, "probe", "/r", Some(lineage));
            let shown = wm.read_history_records("probe").unwrap();
            let ids = |records: &[SessionRecord]| -> Vec<(String, bool)> {
                records
                    .iter()
                    .map(|r| (r.native_id.clone(), r.inherited))
                    .collect()
            };
            assert_eq!(ids(&read), ids(&shown), "{ws} {anchor:?}");
            assert!(read.iter().all(|r| r.inherited));
            wm.remove_agent("probe").unwrap();
        }
        let c = wm.resolve_anchor("c", Anchor::Before("c1")).unwrap();
        assert_eq!(
            wm.read_lineage_records(&c)
                .unwrap()
                .iter()
                .map(|r| r.native_id.as_str())
                .collect::<Vec<_>>(),
            ["a1-u", "a1-a", "b1-u", "b1-a"]
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

    // ── history pages ────────────────────────────────────────────────────

    /// `(native_id, inherited)` of one page, and its cursor.
    fn page(
        wm: &WorkspaceManager,
        ws: &str,
        before: Option<&str>,
        limit: usize,
    ) -> (Vec<(String, bool)>, Option<String>) {
        let page = wm.read_history_page(ws, before, Some(limit)).unwrap();
        let ids = page
            .records
            .into_iter()
            .map(|r| (r.native_id, r.inherited))
            .collect();
        (ids, page.older)
    }

    #[test]
    fn a_single_sessions_history_pages_back_from_the_newest() {
        let wm = three_level_chain();
        let (newest, older) = page(&wm, "a", None, 4);
        assert_eq!(newest, owned(&["a2-u", "a2-a", "a3-u", "a3-a"]));
        assert_eq!(older.as_deref(), Some("0:3"));
        let (rest, older) = page(&wm, "a", older.as_deref(), 4);
        assert_eq!(rest, owned(&["a1-u", "a1-a"]));
        assert_eq!(older, None, "nothing precedes the first record");
    }

    #[test]
    fn a_page_that_ends_exactly_at_the_start_has_no_older_cursor() {
        let wm = three_level_chain();
        let (all, older) = page(&wm, "a", None, 6);
        assert_eq!(all.len(), 6);
        assert_eq!(older, None);
    }

    /// `b` shows `a` only below its cut (a1), then its own: a page straddling
    /// the boundary walks into `a`, stops at the cut and tags `a`'s records.
    #[test]
    fn a_page_straddles_a_cut_and_tags_the_ancestors_records() {
        let wm = three_level_chain();
        let (newest, older) = page(&wm, "b", None, 3);
        assert_eq!(newest, owned(&["b1-a", "b2-u", "b2-a"]));
        // Link 1 (b itself, root first), below b1-a's seq in b's own space.
        assert_eq!(older.as_deref(), Some("1:2"));
        let (straddle, older) = page(&wm, "b", older.as_deref(), 3);
        assert_eq!(
            straddle,
            concat(&[inherited(&["a1-u", "a1-a"]), owned(&["b1-u"])])
        );
        assert_eq!(older, None, "a2 and later lie past b's cut");
    }

    /// However the history is cut into pages, the pages put back together are
    /// the whole read, across every link of a three-level chain.
    #[test]
    fn pages_put_back_together_are_the_whole_history() {
        let wm = three_level_chain();
        for ws in ["a", "b", "c"] {
            for limit in 1..=7 {
                let mut stitched = Vec::new();
                let mut before = None;
                loop {
                    let (records, older) = page(&wm, ws, before.as_deref(), limit);
                    assert!(records.len() <= limit);
                    stitched.splice(0..0, records);
                    if older.is_none() {
                        break;
                    }
                    before = older;
                }
                assert_eq!(stitched, history(&wm, ws), "{ws} by {limit}");
            }
        }
    }

    #[test]
    fn a_bad_cursor_is_an_error_not_some_other_page() {
        let wm = three_level_chain();
        for cursor in [
            "", "0", "x:1", "0:x", "0:-1", "+0:1", "0:+1", "0: 1", "0:1:2", ":1", "0:",
        ] {
            assert!(
                wm.read_history_page("a", Some(cursor), None).is_err(),
                "{cursor:?}"
            );
        }
        // Well formed, but `a`'s chain has one link.
        let err = wm
            .read_history_page("a", Some("1:3"), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("past"), "{err}");
    }

    #[test]
    fn the_page_size_defaults_and_clamps() {
        let db = test_db();
        seed_repo(&db, "/r");
        let wm = WorkspaceManager::new(db);
        agent(&wm, "long", "/r", None);
        let body = json!({"type": "assistant"});
        let ids: Vec<String> = (0..HISTORY_PAGE_MAX + 10)
            .map(|i| format!("r{i}"))
            .collect();
        let rows: Vec<(&str, &serde_json::Value)> =
            ids.iter().map(|id| (id.as_str(), &body)).collect();
        wm.append_session_records("long", "claude", "transcript", None, &rows)
            .unwrap();

        let read = |limit| wm.read_history_page("long", None, limit).unwrap();
        assert_eq!(read(None).records.len(), HISTORY_PAGE_DEFAULT);
        let capped = read(Some(usize::MAX));
        assert_eq!(capped.records.len(), HISTORY_PAGE_MAX);
        assert_eq!(
            capped.records.last().map(|r| r.native_id.as_str()),
            Some(ids.last().unwrap().as_str()),
            "the newest page ends at the newest record"
        );
        assert!(capped.older.is_some());
        assert_eq!(
            read(Some(0)).records.len(),
            1,
            "a page is never empty by asking"
        );
    }

    #[test]
    fn a_workspace_without_a_session_has_an_empty_last_page() {
        let wm = three_level_chain();
        let page = wm.read_history_page("nope", None, None).unwrap();
        assert!(page.records.is_empty());
        assert_eq!(page.older, None);
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

    /// The trim of a handed-over session's records happens through
    /// `TranscriptCleanup` after the moving transaction has committed, never
    /// inside it: a WAL commit is atomic per file, so an in-transaction delete
    /// in the attached file could outlive a rolled-back move.
    #[test]
    fn detach_children_leaves_the_record_trim_to_cleanup_after_commit() {
        let wm = three_level_chain();
        let b = session_of(&wm, "b");
        let count = |conn: &Connection| {
            conn.query_row(
                "SELECT COUNT(*) FROM transcripts.session_records WHERE session_id = ?1",
                [&b],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
        };
        let conn = wm.db.lock();
        let before = count(&conn);
        let tx = conn.unchecked_transaction().unwrap();
        let trims = detach_children(&tx, &["b".to_string()]).unwrap();
        assert_eq!(
            count(&tx),
            before,
            "the attached file is untouched in the transaction"
        );
        tx.commit().unwrap();
        assert_eq!(trims.len(), 1);
        assert_eq!(trims[0].0, b);
        sessions::TranscriptCleanup {
            sessions: Vec::new(),
            trims,
        }
        .apply(&conn);
        assert_eq!(
            count(&conn),
            2,
            "rows at or past every child's cut are gone"
        );
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
                    "SELECT COUNT(*) FROM transcripts.session_records WHERE session_id = ?1",
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

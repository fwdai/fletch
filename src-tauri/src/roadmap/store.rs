//! The `roadmap_items` DAO: every read and write of the table, as plain
//! functions over a `&Connection`.
//!
//! Kept separate from the command surface in `mod.rs` on purpose. The commands
//! are the frontend's door, but the roadmap's other writers are Rust-side (the
//! RPC dispatcher the PM agent talks to, and the queue drainer that flips items
//! to `queued`/`active`), and they hold the connection lock themselves. Both
//! doors therefore share one implementation of code allocation and JSON
//! marshalling, so neither can drift.
//!
//! Every function here must be called with the connection mutex held. That is
//! what makes [`next_code`]'s read/bump pair atomic: the app has exactly one
//! `Connection` behind one `Mutex`, so no second writer can allocate the same
//! code between reading the counter and bumping it.

use rusqlite::{params, Connection, OptionalExtension};

use super::types::{strings_to_col, ItemPatch, ItemSource, ItemStatus, NewItem};
use super::types::{Horizon, RoadmapItem, COLUMNS};
use crate::database::now_millis;

/// `project_settings` key holding a project's roadmap code prefix. Stored on
/// first allocation so the prefix survives a project rename — the codes already
/// minted under it don't change, and neither should the next one.
const PREFIX_KEY: &str = "roadmap.code_prefix";

/// `project_settings` key holding the *next* number a project will mint. Read
/// and bumped inside the caller's lock scope by [`next_code`], so a number is
/// issued exactly once, ever — see there for why recycling is not an option.
const SEQ_KEY: &str = "roadmap.code_seq";

/// Where a project's numbering starts. Three digits from the outset so codes
/// sort and align in the UI without zero-padding.
const FIRST_NUMBER: i64 = 100;

/// Fallback prefix when a project name yields no usable letters.
const FALLBACK_PREFIX: &str = "PRJ";

/// Every item on a project's roadmap in *board order* — `rank` first, which is
/// the explicit priority order the user drags and the PM proposes (0032), then
/// `created_at, rowid` to break a tie stably.
///
/// The same order the drainer dispatches in: it reads its queue through this
/// function, so the visible order and the dispatch order cannot disagree (see
/// [`super::drainer::pick_next`]). The tiebreak matters because `rank` has no
/// uniqueness constraint — two rows can only share a value through a hand-edit
/// or a fractional split that exhausted the float, and neither may make the
/// board's order depend on SQLite's scan order.
pub fn list(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<RoadmapItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM roadmap_items WHERE project_id = ?1 \
         ORDER BY rank, created_at, rowid"
    ))?;
    let rows = stmt.query_map([project_id], RoadmapItem::from_row)?;
    rows.collect()
}

/// One item by id, or `None` if it's gone.
pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<RoadmapItem>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM roadmap_items WHERE id = ?1"),
        [id],
        RoadmapItem::from_row,
    )
    .optional()
}

/// Insert an item, allocating its `code` and its `rank`. Defaults: `later`
/// horizon, `open` status, `user` source, last in the priority order.
pub fn create(conn: &Connection, project_id: &str, new: &NewItem) -> rusqlite::Result<RoadmapItem> {
    let id = uuid::Uuid::new_v4().to_string();
    let code = next_code(conn, project_id)?;
    let rank = next_rank(conn, project_id)?;
    let now = now_millis();
    conn.execute(
        "INSERT INTO roadmap_items
           (id, project_id, code, title, why, horizon, status, rank, area, source,
            accept_json, deps_json, workflow_def_id, issue_url, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
        params![
            id,
            project_id,
            code,
            new.title,
            new.why,
            new.horizon.unwrap_or(Horizon::Later).as_str(),
            new.status.unwrap_or(ItemStatus::Open).as_str(),
            rank,
            new.area,
            new.source.unwrap_or(ItemSource::User).as_str(),
            strings_to_col(&new.accept),
            strings_to_col(&new.deps),
            new.workflow_def_id,
            new.issue_url,
            now,
        ],
    )?;
    // Read back rather than reconstructing, so the returned row is exactly what
    // a later `list` will produce (column defaults included).
    get(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// Apply a partial update and return the stored row. An empty patch is a no-op
/// that still returns the row (and still bumps nothing), so callers can send a
/// patch built from a form without special-casing "nothing changed".
///
/// Unconditional: the `SET` lands whatever the row currently says. Use
/// [`update_where_status`] for a *transition*, where applying the patch on top of
/// a status somebody else already moved would be wrong.
pub fn update(
    conn: &Connection,
    id: &str,
    patch: &ItemPatch,
) -> rusqlite::Result<Option<RoadmapItem>> {
    apply(conn, id, patch, None)
}

/// Apply a partial update only while the row is still in `expected`, and return
/// the stored row — or `None` when the precondition missed (or the row is gone).
///
/// The precondition rides the `UPDATE`'s own `WHERE`, so the check and the write
/// are one statement and nothing can slip between them. That is what makes a
/// status *transition* safe to express from a client holding a stale snapshot:
/// an unqueue (`queued → open`) issued a moment after the drainer claimed the
/// item (`queued → active`) matches no row and is dropped, rather than flipping a
/// live run's item back onto the board.
pub fn update_where_status(
    conn: &Connection,
    id: &str,
    expected: ItemStatus,
    patch: &ItemPatch,
) -> rusqlite::Result<Option<RoadmapItem>> {
    apply(conn, id, patch, Some(expected))
}

/// The one implementation behind [`update`] and [`update_where_status`]: build
/// the `SET` list from the patch, optionally carry a status precondition, and
/// read the row back.
fn apply(
    conn: &Connection,
    id: &str,
    patch: &ItemPatch,
    expected: Option<ItemStatus>,
) -> rusqlite::Result<Option<RoadmapItem>> {
    // Built as (column, value) pairs so the SQL only ever contains literal
    // column names written here — never caller-supplied identifiers.
    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut set = |col: &'static str, v: Box<dyn rusqlite::ToSql>| {
        sets.push(col);
        vals.push(v);
    };

    if let Some(v) = &patch.title {
        set("title", Box::new(v.clone()));
    }
    if let Some(v) = &patch.why {
        set("why", Box::new(v.clone()));
    }
    if let Some(v) = patch.horizon {
        set("horizon", Box::new(v.as_str()));
    }
    if let Some(v) = patch.status {
        set("status", Box::new(v.as_str()));
    }
    if let Some(v) = patch.source {
        set("source", Box::new(v.as_str()));
    }
    if let Some(v) = patch.rank {
        set("rank", Box::new(v));
    }
    if let Some(v) = &patch.accept {
        set("accept_json", Box::new(strings_to_col(v)));
    }
    if let Some(v) = &patch.deps {
        set("deps_json", Box::new(strings_to_col(v)));
    }
    if let Some(v) = &patch.area {
        set("area", Box::new(v.clone()));
    }
    if let Some(v) = &patch.agent_id {
        set("agent_id", Box::new(v.clone()));
    }
    if let Some(v) = &patch.workflow_def_id {
        set("workflow_def_id", Box::new(v.clone()));
    }
    if let Some(v) = &patch.run_id {
        set("run_id", Box::new(v.clone()));
    }
    if let Some(v) = &patch.pr_url {
        set("pr_url", Box::new(v.clone()));
    }
    if let Some(v) = patch.pr_number {
        set("pr_number", Box::new(v));
    }

    if sets.is_empty() {
        // Nothing to write, but a precondition still has to be honoured: an
        // empty patch against a row that has moved on is a miss, not a no-op.
        let row = get(conn, id)?;
        return Ok(match expected {
            Some(status) => row.filter(|r| r.status == status),
            None => row,
        });
    }

    let assignments: Vec<String> = sets
        .iter()
        .enumerate()
        .map(|(i, col)| format!("{col} = ?{}", i + 1))
        .collect();
    let n = vals.len();
    vals.push(Box::new(now_millis()));
    vals.push(Box::new(id.to_string()));
    let guard = match expected {
        Some(status) => {
            vals.push(Box::new(status.as_str()));
            format!(" AND status = ?{}", n + 3)
        }
        None => String::new(),
    };
    let sql = format!(
        "UPDATE roadmap_items SET {}, updated_at = ?{} WHERE id = ?{}{guard}",
        assignments.join(", "),
        n + 1,
        n + 2
    );
    let refs: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| v.as_ref()).collect();
    let changed = conn.execute(&sql, refs.as_slice())?;
    // A guarded update that matched nothing is a missed precondition — the
    // caller must be able to tell that from "applied", so don't hand back a row.
    if expected.is_some() && changed == 0 {
        return Ok(None);
    }
    get(conn, id)
}

/// Delete an item. Returns whether a row was actually removed, so a caller
/// doesn't announce a deletion that didn't happen.
pub fn delete(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute("DELETE FROM roadmap_items WHERE id = ?1", [id])?;
    Ok(n > 0)
}

/// Rule an item off the board: `status = rejected`, `close_reason` = why, and
/// every claim on the row's future cleared in the same statement — the hold
/// trio (a rejection supersedes a pause; the trail keeps the hold's history)
/// and the agent stamp (nobody builds a rejected item). `None` when the row is
/// gone.
///
/// Deliberately its own SQL rather than [`ItemPatch`] fields, for the same
/// reason as [`super::holds::hold_item`]: `close_reason` is absent from the
/// patch surface, so no generic edit can rule an item off the board — only the
/// typed reject command and the discard ruling can express this write. The
/// status gate (which statuses may be rejected at all) is the caller's, checked
/// under the same connection lock this runs in.
pub fn reject(conn: &Connection, id: &str, reason: &str) -> rusqlite::Result<Option<RoadmapItem>> {
    conn.execute(
        "UPDATE roadmap_items
            SET status = ?1, close_reason = ?2,
                hold_reason = NULL, held_by = NULL, held_at = NULL,
                agent_id = NULL, updated_at = ?3
          WHERE id = ?4",
        params![ItemStatus::Rejected.as_str(), reason, now_millis(), id],
    )?;
    get(conn, id)
}

/// Put a rejected item back on the board: `rejected → open`, `close_reason`
/// cleared — an item back in play owes nobody an epitaph; the trail keeps the
/// reason it just shed.
///
/// The `status = rejected` precondition rides the `UPDATE`'s own `WHERE`, like
/// [`update_where_status`], so the check and the write are one statement:
/// `None` means the row moved on (or was never rejected, or is gone), and the
/// caller treats that as a miss rather than reopening something else.
pub fn reopen(conn: &Connection, id: &str) -> rusqlite::Result<Option<RoadmapItem>> {
    let changed = conn.execute(
        "UPDATE roadmap_items
            SET status = ?1, close_reason = NULL, updated_at = ?2
          WHERE id = ?3 AND status = ?4",
        params![
            ItemStatus::Open.as_str(),
            now_millis(),
            id,
            ItemStatus::Rejected.as_str()
        ],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    get(conn, id)
}

/// `project_settings` key holding the tracker issues the user has *turned down* —
/// a JSON array of URLs.
///
/// Not a column and not a table, for one reason: the fact it records is about a
/// row that no longer exists. Discarding a ghost deletes it (that is what a
/// discard is, and what makes the trail cascade away with it), so the "declined"
/// half of a routing record has nowhere on `roadmap_items` to live. A table would
/// be the tidier home, but a roadmap table is off the generic CRUD allow-list by
/// invariant and would therefore need a typed read command of its own for one
/// list of strings; `project_settings` is already the project-scoped key/value
/// store the frontend can read (`getProjectSettings`), and already holds JSON
/// documents this way (see `run_env`).
pub(crate) const DECLINED_ISSUES_KEY: &str = "roadmap.declined_issues";

/// How many declined URLs a project keeps. Oldest are dropped first: the list
/// exists to stop the inbox re-offering something you just said no to, and an
/// issue you declined a thousand tickets ago being offered once more is a far
/// smaller sin than an unbounded settings value re-read on every inbox render.
const DECLINED_MAX: usize = 500;

/// Remember that an imported issue was turned down, so the inbox stops offering
/// it. Idempotent: a URL already on the list is left where it is rather than
/// re-appended, so re-declining can't push the rest of the list out.
///
/// Called from the delete path when the row being removed is a *proposed* row
/// carrying an `issue_url` — the exact shape of "the user discarded the ghost
/// this issue was routed as". Deleting a row that was already *accepted* records
/// nothing: that issue reached the roadmap and was then removed from it, which is
/// a different decision from refusing it at the door.
pub fn decline_issue(conn: &Connection, project_id: &str, issue_url: &str) -> rusqlite::Result<()> {
    let mut urls = declined_issues(conn, project_id)?;
    if urls.iter().any(|u| u == issue_url) {
        return Ok(());
    }
    urls.push(issue_url.to_string());
    if urls.len() > DECLINED_MAX {
        urls.drain(..urls.len() - DECLINED_MAX);
    }
    let json = serde_json::to_string(&urls).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3) \
         ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
        params![project_id, DECLINED_ISSUES_KEY, json],
    )?;
    Ok(())
}

/// The stored declined list, oldest first. A missing or unparseable value reads
/// as empty — a corrupt setting must cost the dedup, never the discard.
fn declined_issues(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<String>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
            params![project_id, DECLINED_ISSUES_KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(stored
        .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
        .unwrap_or_default())
}

/// The next free code for a project ("FLT-142").
///
/// The number comes off a persisted per-project counter ([`SEQ_KEY`]), read and
/// bumped here, so a number is issued **once, ever** — never recycled. Must be
/// called with the connection lock held, in the same scope as the insert that
/// consumes it, which is what makes read-and-bump atomic (see the module docs).
///
/// Recycling was the earlier behaviour (`MAX(live suffix) + 1`) and is rejected:
/// a code outlives its row in places the table can't see. A stored PM proposal
/// naming `FLT-142` as a dependency would silently rebind to whatever unrelated
/// item next took that number, and a code the PM quoted in a live transcript
/// would start pointing somewhere else. A stateless allocator isn't worth
/// either.
pub fn next_code(conn: &Connection, project_id: &str) -> rusqlite::Result<String> {
    let prefix = code_prefix(conn, project_id)?;
    let number = next_number(conn, project_id)?;
    conn.execute(
        "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3) \
         ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
        params![project_id, SEQ_KEY, (number + 1).to_string()],
    )?;
    Ok(format!("{prefix}-{number}"))
}

/// The number [`next_code`] will mint: the stored counter, floored by the
/// highest live code so it can never collide with a row that is already there.
///
/// The floor covers the two ways the counter can be behind the table: a project
/// numbered before the counter existed (no stored value — the floor *is* the
/// seed), and a row inserted with a hand-written code. `MAX` is computed in Rust
/// rather than SQL so codes that don't match the project's own prefix (an
/// imported `#207`) are skipped instead of poisoning it.
fn next_number(conn: &Connection, project_id: &str) -> rusqlite::Result<i64> {
    let stored: Option<i64> = conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
            params![project_id, SEQ_KEY],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| v.trim().parse().ok());
    let mut stmt = conn.prepare("SELECT code FROM roadmap_items WHERE project_id = ?1")?;
    let highest = stmt
        .query_map([project_id], |r| r.get::<_, String>(0))?
        .filter_map(|c| c.ok())
        .filter_map(|c| code_number(&c))
        .max()
        .unwrap_or(FIRST_NUMBER - 1);
    Ok(stored.unwrap_or(FIRST_NUMBER).max(highest + 1))
}

/// The rank a new item takes: `MAX(rank) + 1` for the project, so anything
/// added lands *last* in the priority order rather than silently jumping the
/// queue. `1.0` on an empty board.
///
/// Read-max/insert, atomic for the same reason [`next_code`]'s read/bump is: one
/// connection behind one mutex, held by the caller. Unlike a code, a rank is not
/// an identity — reusing one is meaningless, so there is no counter to keep.
/// Two rows sharing a rank would still
/// order deterministically (the list query tiebreaks on `created_at, rowid`),
/// but the drag would have no gap to split.
pub fn next_rank(conn: &Connection, project_id: &str) -> rusqlite::Result<f64> {
    conn.query_row(
        "SELECT COALESCE(MAX(rank), 0.0) + 1.0 FROM roadmap_items WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )
}

/// Rewrite a whole set of ranks as `1.0, 2.0, …` in the given id order, in one
/// transaction — the accepted-order path, where the new sequence is the whole
/// ask and a partially applied one would be an order nobody proposed.
///
/// Returns the rewritten rows in their new order, for emitting after the lock
/// drops. An id that no longer resolves is skipped rather than failing the
/// batch: the caller has already validated the set, and a row deleted in the
/// microseconds since is not a reason to refuse the other twelve.
pub fn set_ranks(conn: &Connection, ids: &[String]) -> rusqlite::Result<Vec<RoadmapItem>> {
    let tx = conn.unchecked_transaction()?;
    let now = now_millis();
    let mut out = Vec::with_capacity(ids.len());
    for (n, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE roadmap_items SET rank = ?1, updated_at = ?2 WHERE id = ?3",
            params![(n + 1) as f64, now, id],
        )?;
        if let Some(row) = get(&tx, id)? {
            out.push(row);
        }
    }
    tx.commit()?;
    Ok(out)
}

/// The numeric tail of a code, if it has one. `"FLT-142"` → `142`.
fn code_number(code: &str) -> Option<i64> {
    code.rsplit('-').next()?.parse().ok()
}

/// A project's code prefix, deriving and persisting one on first use so it
/// survives a rename.
fn code_prefix(conn: &Connection, project_id: &str) -> rusqlite::Result<String> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
            params![project_id, PREFIX_KEY],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(p) = stored.filter(|p| !p.trim().is_empty()) {
        return Ok(p);
    }

    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM projects WHERE id = ?1",
            [project_id],
            |r| r.get(0),
        )
        .optional()?;
    let prefix = derive_prefix(name.as_deref().unwrap_or_default());
    conn.execute(
        "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3) \
         ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
        params![project_id, PREFIX_KEY, prefix],
    )?;
    Ok(prefix)
}

/// Turn a project name into a 2–4 letter code prefix: initials for a
/// multi-word name ("my cool app" → `MCA`), the leading letters otherwise
/// ("fletch" → `FLE`). Falls back to `PRJ` when there is nothing to work with,
/// because a project with an unusable name still needs codes.
fn derive_prefix(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let candidate: String = if words.len() >= 2 {
        words
            .iter()
            .take(4)
            .filter_map(|w| w.chars().next())
            .collect()
    } else {
        words
            .first()
            .map(|w| w.chars().take(3).collect())
            .unwrap_or_default()
    };
    let candidate = candidate.to_ascii_uppercase();
    if candidate.len() >= 2 {
        candidate
    } else {
        FALLBACK_PREFIX.to_string()
    }
}

#[cfg(test)]
mod tests {
    use rusqlite_migration::{Migrations, M};

    use super::*;
    use crate::database::get_migrations;

    /// A migrated in-memory DB with foreign keys on, matching how the app opens
    /// the real file — the cascade and the FK to `projects` are part of what
    /// these tests exercise.
    fn test_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        get_migrations().to_latest(&mut conn).unwrap();
        conn
    }

    fn project(conn: &Connection, id: &str, name: &str) -> String {
        conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES (?1, ?2, 0)",
            params![id, name],
        )
        .unwrap();
        id.to_string()
    }

    fn titled(title: &str) -> NewItem {
        NewItem {
            title: title.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn codes_run_sequentially_from_100_with_a_derived_prefix() {
        let conn = test_conn();
        let p = project(&conn, "p1", "my-cool-app");

        let first = create(&conn, &p, &titled("one")).unwrap();
        let second = create(&conn, &p, &titled("two")).unwrap();

        assert_eq!(first.code, "MCA-100");
        assert_eq!(second.code, "MCA-101");
        // The prefix is persisted on first allocation, so renaming the project
        // later can't renumber anything.
        let stored: String = conn
            .query_row(
                "SELECT value FROM project_settings WHERE project_id = 'p1' AND key = ?1",
                [PREFIX_KEY],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, "MCA");
    }

    #[test]
    fn code_allocation_is_per_project() {
        let conn = test_conn();
        let a = project(&conn, "pa", "alpha");
        let b = project(&conn, "pb", "beta service");

        let a1 = create(&conn, &a, &titled("a1")).unwrap();
        let b1 = create(&conn, &b, &titled("b1")).unwrap();
        let a2 = create(&conn, &a, &titled("a2")).unwrap();

        // Each project numbers independently and carries its own prefix.
        assert_eq!(a1.code, "ALP-100");
        assert_eq!(a2.code, "ALP-101");
        assert_eq!(b1.code, "BS-100");
    }

    #[test]
    fn a_number_is_issued_once_ever_even_if_its_item_is_deleted() {
        // A code is an item's identity — the PM quotes it in a transcript, a
        // stored proposal names it as a dependency, and later slices put it in
        // branch names and PR titles. None of those live in this table, so
        // handing a deleted item's number to a *different* item would silently
        // rebind every one of them: an accepted dep patch saying "after
        // FLE-102" would start pointing at work nobody sequenced. Earlier
        // versions recycled the highest number as a deliberate trade for a
        // stateless allocator; the trade is rejected — the counter is persisted.
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        let one = create(&conn, &p, &titled("one")).unwrap();
        let two = create(&conn, &p, &titled("two")).unwrap();
        let three = create(&conn, &p, &titled("three")).unwrap();
        // Both a middle item and the highest one, which is the case the old
        // `MAX(live) + 1` allocator got wrong.
        assert!(delete(&conn, &two.id).unwrap());
        assert!(delete(&conn, &three.id).unwrap());

        let four = create(&conn, &p, &titled("four")).unwrap();
        assert_eq!(one.code, "FLE-100", "survivors keep their codes");
        assert_eq!(two.code, "FLE-101");
        assert_eq!(three.code, "FLE-102");
        assert_eq!(four.code, "FLE-103", "no gap is ever backfilled");
        // And the counter is where the numbering lives, not the rows.
        let seq: String = conn
            .query_row(
                "SELECT value FROM project_settings WHERE project_id = 'p1' AND key = ?1",
                [SEQ_KEY],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(seq, "104");
    }

    #[test]
    fn a_project_numbered_before_the_counter_existed_picks_up_after_its_rows() {
        // The seed path: rows minted by the old stateless allocator, and no
        // stored counter. The first allocation must clear them rather than
        // collide (there is no UNIQUE constraint to catch it).
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        conn.execute(
            "INSERT INTO roadmap_items (id, project_id, code, title, horizon, status,
                                        created_at, updated_at)
             VALUES ('legacy', 'p1', 'FLE-142', 'from before', 'later', 'open', 0, 0)",
            [],
        )
        .unwrap();
        assert_eq!(create(&conn, &p, &titled("next")).unwrap().code, "FLE-143");
        assert_eq!(create(&conn, &p, &titled("after")).unwrap().code, "FLE-144");
    }

    #[test]
    fn foreign_codes_do_not_poison_the_next_number() {
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        // An imported row, as a Linear/GitHub import would leave it.
        conn.execute(
            "INSERT INTO roadmap_items (id, project_id, code, title, horizon, status,
                                        created_at, updated_at)
             VALUES ('imported', 'p1', '#207', 'from github', 'later', 'open', 0, 0)",
            [],
        )
        .unwrap();
        assert_eq!(create(&conn, &p, &titled("x")).unwrap().code, "FLE-100");
    }

    /// A new item lands *last* in the priority order — `MAX(rank) + 1`, per
    /// project, so adding a ticket never jumps the queue.
    #[test]
    fn a_new_item_ranks_last_within_its_own_project() {
        let conn = test_conn();
        let a = project(&conn, "pa", "alpha");
        let b = project(&conn, "pb", "beta");

        let first = create(&conn, &a, &titled("one")).unwrap();
        let second = create(&conn, &a, &titled("two")).unwrap();
        let other = create(&conn, &b, &titled("theirs")).unwrap();
        assert_eq!(first.rank, 1.0);
        assert_eq!(second.rank, 2.0);
        assert_eq!(other.rank, 1.0, "each project has its own sequence");

        // A drag drops the second item above the first (the midpoint of "before
        // 1.0" and nothing), and the next new item still lands last.
        update(
            &conn,
            &second.id,
            &ItemPatch {
                rank: Some(0.5),
                ..Default::default()
            },
        )
        .unwrap();
        let third = create(&conn, &a, &titled("three")).unwrap();
        assert_eq!(third.rank, 2.0, "MAX + 1 over what is left");
        assert_eq!(
            list(&conn, &a)
                .unwrap()
                .iter()
                .map(|i| i.code.clone())
                .collect::<Vec<_>>(),
            vec![second.code, first.code, third.code],
            "the board draws rank order"
        );
    }

    /// The 0032 backfill: rows written before the column existed keep exactly
    /// the order the board used to draw them (`created_at, rowid`), as 1.0, 2.0,
    /// … per project. Seeded through raw inserts because that is what a
    /// pre-migration database holds.
    #[test]
    fn the_migration_backfills_existing_rows_in_board_order() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        // Everything up to and including 0031 — the state a shipped install was
        // in before the rank slice. Pinned by count rather than as "all but the
        // last", so a migration added after 0032 doesn't quietly change which
        // schema this test rebuilds (it would then apply 0032 itself, and there
        // would be no backfill left to assert).
        const BEFORE_RANK: usize = 31;
        let before = BEFORE_RANK;
        Migrations::new(
            crate::database::MIGRATIONS[..before]
                .iter()
                .map(|&sql| M::up(sql))
                .collect(),
        )
        .to_latest(&mut conn)
        .unwrap();
        conn.execute_batch(
            "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'one', 0), ('p2', 'two', 0);
             INSERT INTO roadmap_items (id, project_id, code, title, horizon, status,
                                        created_at, updated_at) VALUES
               ('b', 'p1', 'ONE-101', 'second', 'later', 'open', 20, 20),
               ('a', 'p1', 'ONE-100', 'first',  'later', 'open', 10, 10),
               ('c', 'p1', 'ONE-102', 'tied',   'later', 'open', 20, 20),
               ('d', 'p2', 'TWO-100', 'theirs', 'later', 'open', 99, 99);",
        )
        .unwrap();

        get_migrations().to_latest(&mut conn).unwrap();

        // Per project, and in the pre-0032 order: `created_at` first, then
        // insertion order for the two rows that share a timestamp.
        let ranks: Vec<(String, f64)> = list(&conn, "p1")
            .unwrap()
            .iter()
            .map(|i| (i.id.clone(), i.rank))
            .collect();
        assert_eq!(
            ranks,
            vec![
                ("a".to_string(), 1.0),
                ("b".to_string(), 2.0),
                ("c".to_string(), 3.0)
            ]
        );
        assert_eq!(list(&conn, "p2").unwrap()[0].rank, 1.0);
        // And the allocator picks up from the backfilled values.
        assert_eq!(create(&conn, "p1", &titled("next")).unwrap().rank, 4.0);
    }

    /// The accepted-order path: one transaction rewrites the whole sequence as
    /// 1.0, 2.0, …, and hands back the rows in that order.
    #[test]
    fn setting_a_whole_sequence_renumbers_it_from_one() {
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        let one = create(&conn, &p, &titled("one")).unwrap();
        let two = create(&conn, &p, &titled("two")).unwrap();
        let three = create(&conn, &p, &titled("three")).unwrap();

        let rows = set_ranks(&conn, &[three.id.clone(), one.id.clone(), two.id.clone()]).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.rank).collect::<Vec<_>>(),
            vec![1.0, 2.0, 3.0]
        );
        assert_eq!(
            list(&conn, &p)
                .unwrap()
                .iter()
                .map(|i| i.id.clone())
                .collect::<Vec<_>>(),
            vec![three.id, one.id, two.id]
        );

        // An id that vanished between validation and the write is skipped, not
        // fatal — the rest of the sequence still lands.
        let ghost = "no-such-item".to_string();
        assert_eq!(set_ranks(&conn, &[ghost]).unwrap().len(), 0);
    }

    #[test]
    fn create_defaults_and_round_trips_json_arrays() {
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");

        let bare = create(&conn, &p, &titled("bare")).unwrap();
        assert_eq!(bare.horizon, Horizon::Later, "an unplaced item is backlog");
        assert_eq!(bare.status, ItemStatus::Open);
        assert_eq!(bare.source, ItemSource::User);
        assert!(bare.accept.is_empty() && bare.deps.is_empty());
        assert_eq!(bare.why, "");

        let full = create(
            &conn,
            &p,
            &NewItem {
                title: "shaped".into(),
                why: "because".into(),
                horizon: Some(Horizon::Now),
                status: Some(ItemStatus::Proposed),
                area: Some("runtime".into()),
                source: Some(ItemSource::Pm),
                accept: vec!["survives a quit".into(), "reattaches".into()],
                deps: vec![bare.code.clone()],
                workflow_def_id: Some("wf-pipeline".into()),
                issue_url: None,
            },
        )
        .unwrap();

        // Read back through `list`, so the JSON columns went through both the
        // write and the read path.
        let rows = list(&conn, &p).unwrap();
        assert_eq!(rows.len(), 2);
        let stored = rows.iter().find(|i| i.id == full.id).unwrap();
        assert_eq!(stored, &full);
        assert_eq!(stored.accept, vec!["survives a quit", "reattaches"]);
        assert_eq!(stored.deps, vec![bare.code]);
        assert_eq!(stored.status, ItemStatus::Proposed);
        // Assignable at creation, so the item form can create-and-assign in one
        // round-trip; unset on the bare row, which means "the project default".
        assert_eq!(stored.workflow_def_id.as_deref(), Some("wf-pipeline"));
        assert_eq!(bare.workflow_def_id, None);
    }

    #[test]
    fn a_wire_borne_null_clears_the_column() {
        // The other update tests build `ItemPatch` in Rust and bypass serde;
        // the frontend's patches arrive as JSON through the command layer.
        // This is the edit dialog's cleared-area path, end to end.
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        let item = create(
            &conn,
            &p,
            &NewItem {
                title: "labelled".into(),
                area: Some("runtime".into()),
                workflow_def_id: Some("wf-pipeline".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let patch: ItemPatch = serde_json::from_str(r#"{"area": null}"#).unwrap();
        let row = update(&conn, &item.id, &patch).unwrap().unwrap();
        assert_eq!(row.area, None, "the dialog's clear must stick");
        assert_eq!(
            row.workflow_def_id.as_deref(),
            Some("wf-pipeline"),
            "absent keys stay untouched"
        );
    }

    #[test]
    fn update_patches_only_named_fields_and_clears_with_null() {
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        let item = create(
            &conn,
            &p,
            &NewItem {
                title: "move me".into(),
                horizon: Some(Horizon::Later),
                area: Some("runtime".into()),
                accept: vec!["one".into()],
                ..Default::default()
            },
        )
        .unwrap();

        // The board's drag-to-horizon: one field, everything else untouched.
        let moved = update(
            &conn,
            &item.id,
            &ItemPatch {
                horizon: Some(Horizon::Now),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(moved.horizon, Horizon::Now);
        assert_eq!(
            moved.area.as_deref(),
            Some("runtime"),
            "an absent field is left alone"
        );
        assert_eq!(moved.accept, vec!["one"]);
        assert_eq!(moved.code, item.code, "a code never moves");
        assert_eq!(moved.created_at, item.created_at);

        // An explicit null clears a nullable column; an empty list clears a
        // JSON one.
        let cleared = update(
            &conn,
            &item.id,
            &ItemPatch {
                area: Some(None),
                accept: Some(vec![]),
                title: Some("retitled".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(cleared.area, None);
        assert!(cleared.accept.is_empty());
        assert_eq!(cleared.title, "retitled");

        // Patching a row that no longer exists is not an error — it reports
        // that there is nothing there.
        assert!(delete(&conn, &item.id).unwrap());
        assert!(update(&conn, &item.id, &ItemPatch::default())
            .unwrap()
            .is_none());
        assert!(
            !delete(&conn, &item.id).unwrap(),
            "second delete is a no-op"
        );
        assert!(list(&conn, &p).unwrap().is_empty());
    }

    #[test]
    fn a_conditional_update_applies_only_while_the_status_still_holds() {
        // The unqueue race: the drainer claims `queued → active` under the
        // connection lock, and the click that says `queued → open` arrives a
        // moment later off a stale board. A blind SET would flip the row back to
        // `open` while a run is being launched against it.
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        let item = create(
            &conn,
            &p,
            &NewItem {
                title: "queue me".into(),
                status: Some(ItemStatus::Queued),
                ..Default::default()
            },
        )
        .unwrap();

        // Hit: the row still says `queued`, so the transition lands.
        let unqueued = update_where_status(
            &conn,
            &item.id,
            ItemStatus::Queued,
            &ItemPatch {
                status: Some(ItemStatus::Open),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(unqueued.status, ItemStatus::Open);

        // The drainer claims it.
        let claimed = update(
            &conn,
            &item.id,
            &ItemPatch {
                status: Some(ItemStatus::Active),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();

        // Miss: the same click, replayed against the claimed row. Nothing is
        // written — not the status, not `updated_at` — and `None` is what tells
        // the command layer to emit nothing and report the row as it is.
        assert!(update_where_status(
            &conn,
            &item.id,
            ItemStatus::Queued,
            &ItemPatch {
                status: Some(ItemStatus::Open),
                ..Default::default()
            },
        )
        .unwrap()
        .is_none());
        assert_eq!(get(&conn, &item.id).unwrap().unwrap(), claimed);

        // An empty patch honours the precondition too, rather than reporting a
        // success the caller would read as "the transition happened".
        assert!(
            update_where_status(&conn, &item.id, ItemStatus::Queued, &ItemPatch::default())
                .unwrap()
                .is_none()
        );
        assert!(
            update_where_status(&conn, &item.id, ItemStatus::Active, &ItemPatch::default())
                .unwrap()
                .is_some()
        );

        // And a row that is gone is a miss, not an error.
        assert!(delete(&conn, &item.id).unwrap());
        assert!(update_where_status(
            &conn,
            &item.id,
            ItemStatus::Active,
            &ItemPatch {
                status: Some(ItemStatus::Open),
                ..Default::default()
            },
        )
        .unwrap()
        .is_none());
    }

    /// The funnel's dedup key is a column, so it survives every edit to the
    /// prose that used to carry it.
    #[test]
    fn a_routed_row_carries_its_issue_url_and_a_hand_typed_one_does_not() {
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        let routed = create(
            &conn,
            &p,
            &NewItem {
                title: "Crash on save".into(),
                why: "https://github.com/o/r/issues/7\nSaving drops the body".into(),
                status: Some(ItemStatus::Proposed),
                source: Some(ItemSource::Github),
                issue_url: Some("https://github.com/o/r/issues/7".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let typed = create(&conn, &p, &titled("my own idea")).unwrap();
        assert_eq!(
            routed.issue_url.as_deref(),
            Some("https://github.com/o/r/issues/7")
        );
        assert_eq!(typed.issue_url, None, "nothing imported this one");

        // The whole point of the column: the `why` it used to be read out of is
        // the user's to rewrite, and dedup must not notice.
        let edited = update(
            &conn,
            &routed.id,
            &ItemPatch {
                why: Some("Because three people asked".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            edited.issue_url.as_deref(),
            Some("https://github.com/o/r/issues/7"),
            "an edited rationale is still the same issue"
        );
        // And there is no way to patch it: `ItemPatch` has no such field, so a
        // wire patch naming it changes nothing.
        let patch: ItemPatch = serde_json::from_str(r#"{"issue_url": "https://evil/1"}"#).unwrap();
        let after = update(&conn, &routed.id, &patch).unwrap().unwrap();
        assert_eq!(
            after.issue_url.as_deref(),
            Some("https://github.com/o/r/issues/7"),
            "provenance is not editable"
        );
        assert_eq!(list(&conn, &p).unwrap().len(), 2);
    }

    /// The other half of the routing record: a refusal, which has to outlive the
    /// row it was expressed on.
    #[test]
    fn a_declined_issue_is_remembered_once_per_project() {
        let conn = test_conn();
        let a = project(&conn, "pa", "alpha");
        let b = project(&conn, "pb", "beta");
        let url = "https://github.com/o/r/issues/7";

        decline_issue(&conn, &a, url).unwrap();
        // Idempotent — declining twice is one entry, not two.
        decline_issue(&conn, &a, url).unwrap();
        decline_issue(&conn, &a, "https://github.com/o/r/issues/9").unwrap();
        // Per project: the same origin repo pinned in two projects means routing
        // (and refusing) in one says nothing about the other.
        assert!(declined_issues(&conn, &b).unwrap().is_empty());
        assert_eq!(
            declined_issues(&conn, &a).unwrap(),
            vec![
                url.to_string(),
                "https://github.com/o/r/issues/9".to_string()
            ]
        );

        // Stored as a JSON array under the shared key, which is how the frontend
        // reads it (`getProjectSettings`) — no command of its own.
        let raw: String = conn
            .query_row(
                "SELECT value FROM project_settings WHERE project_id = 'pa' AND key = ?1",
                [DECLINED_ISSUES_KEY],
                |r| r.get(0),
            )
            .unwrap();
        assert!(raw.starts_with('['), "{raw}");
        assert!(raw.contains(url));

        // A corrupt value costs the dedup, never a panic.
        conn.execute(
            "UPDATE project_settings SET value = 'not json' WHERE project_id = 'pa'",
            [],
        )
        .unwrap();
        assert!(declined_issues(&conn, &a).unwrap().is_empty());
        decline_issue(&conn, &a, url).unwrap();
        assert_eq!(declined_issues(&conn, &a).unwrap(), vec![url.to_string()]);
    }

    /// 0036 is a plain `ADD COLUMN`, so the rows a shipped install already holds
    /// must cross it untouched — no rebuild, no cascade, and the new column
    /// reading NULL rather than absent.
    #[test]
    fn the_issue_url_migration_leaves_existing_rows_alone() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        // Everything up to and including 0035 — pinned by count for the same
        // reason the rank backfill test pins its own: a later migration must not
        // quietly change which schema this rebuilds.
        const BEFORE_ISSUE_URL: usize = 35;
        Migrations::new(
            crate::database::MIGRATIONS[..BEFORE_ISSUE_URL]
                .iter()
                .map(|&sql| M::up(sql))
                .collect(),
        )
        .to_latest(&mut conn)
        .unwrap();
        conn.execute_batch(
            "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'one', 0);
             INSERT INTO roadmap_items (id, project_id, code, title, why, horizon, status,
                                        rank, created_at, updated_at) VALUES
               ('old', 'p1', 'ONE-100', 'from before',
                'https://github.com/o/r/issues/1\nrouted the old way',
                'later', 'proposed', 1.0, 10, 10);",
        )
        .unwrap();

        get_migrations().to_latest(&mut conn).unwrap();

        let rows = list(&conn, "p1").unwrap();
        assert_eq!(rows.len(), 1, "the row survived the migration");
        assert_eq!(rows[0].code, "ONE-100");
        assert_eq!(
            rows[0].issue_url, None,
            "not backfilled — the legacy URL stays in the `why`, where the \
             frontend's fallback reader finds it"
        );
        assert!(rows[0].why.starts_with("https://github.com/o/r/issues/1"));
        // And the column really is there to write.
        decline_issue(&conn, "p1", "https://github.com/o/r/issues/1").unwrap();
        let fresh = create(
            &conn,
            "p1",
            &NewItem {
                title: "new import".into(),
                issue_url: Some("https://github.com/o/r/issues/2".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            fresh.issue_url.as_deref(),
            Some("https://github.com/o/r/issues/2")
        );
    }

    #[test]
    fn deleting_a_project_takes_its_roadmap() {
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        create(&conn, &p, &titled("one")).unwrap();
        conn.execute("DELETE FROM projects WHERE id = 'p1'", [])
            .unwrap();
        assert!(list(&conn, &p).unwrap().is_empty());
    }

    #[test]
    fn prefixes_are_derived_from_whatever_the_name_offers() {
        assert_eq!(derive_prefix("fletch"), "FLE");
        assert_eq!(derive_prefix("my-cool-app"), "MCA");
        assert_eq!(derive_prefix("A very long product name here"), "AVLP");
        assert_eq!(derive_prefix("q"), "PRJ", "one letter is not a prefix");
        assert_eq!(derive_prefix("  "), "PRJ");
        assert_eq!(
            derive_prefix("🚀"),
            "PRJ",
            "non-ascii yields nothing usable"
        );
        assert_eq!(derive_prefix("2fa"), "2FA");
    }
}

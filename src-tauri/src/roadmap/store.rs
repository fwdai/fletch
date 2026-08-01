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
//! what makes [`next_code`]'s read-max/insert pair atomic: the app has exactly
//! one `Connection` behind one `Mutex`, so no second writer can allocate the
//! same code between the `MAX` and the `INSERT`.

use rusqlite::{params, Connection, OptionalExtension};

use super::types::{strings_to_col, ItemPatch, ItemSource, ItemStatus, NewItem};
use super::types::{Horizon, RoadmapItem, COLUMNS};
use crate::database::now_millis;

/// `project_settings` key holding a project's roadmap code prefix. Stored on
/// first allocation so the prefix survives a project rename — the codes already
/// minted under it don't change, and neither should the next one.
const PREFIX_KEY: &str = "roadmap.code_prefix";

/// Where a project's numbering starts. Three digits from the outset so codes
/// sort and align in the UI without zero-padding.
const FIRST_NUMBER: i64 = 100;

/// Fallback prefix when a project name yields no usable letters.
const FALLBACK_PREFIX: &str = "PRJ";

/// Every item on a project's roadmap, oldest first — the order rows were added
/// within a horizon is the order the board draws them, and it's stable across
/// edits (unlike `updated_at`, which would make a row jump when it's touched).
pub fn list(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<RoadmapItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM roadmap_items WHERE project_id = ?1 ORDER BY created_at, rowid"
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

/// Insert an item, allocating its `code`. Defaults: `later` horizon, `open`
/// status, `user` source.
pub fn create(conn: &Connection, project_id: &str, new: &NewItem) -> rusqlite::Result<RoadmapItem> {
    let id = uuid::Uuid::new_v4().to_string();
    let code = next_code(conn, project_id)?;
    let now = now_millis();
    conn.execute(
        "INSERT INTO roadmap_items
           (id, project_id, code, parent_id, title, why, horizon, status, size, area, source,
            epic, accept_json, deps_json, workflow_def_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
        params![
            id,
            project_id,
            code,
            new.title,
            new.why,
            new.horizon.unwrap_or(Horizon::Later).as_str(),
            new.status.unwrap_or(ItemStatus::Open).as_str(),
            new.size.map(|s| s.as_str()),
            new.area,
            new.source.unwrap_or(ItemSource::User).as_str(),
            new.epic,
            strings_to_col(&new.accept),
            strings_to_col(&new.deps),
            new.workflow_def_id,
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
pub fn update(
    conn: &Connection,
    id: &str,
    patch: &ItemPatch,
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
    if let Some(v) = &patch.accept {
        set("accept_json", Box::new(strings_to_col(v)));
    }
    if let Some(v) = &patch.deps {
        set("deps_json", Box::new(strings_to_col(v)));
    }
    if let Some(v) = patch.size {
        set("size", Box::new(v.map(|s| s.as_str())));
    }
    if let Some(v) = &patch.area {
        set("area", Box::new(v.clone()));
    }
    if let Some(v) = &patch.epic {
        set("epic", Box::new(v.clone()));
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

    if !sets.is_empty() {
        let assignments: Vec<String> = sets
            .iter()
            .enumerate()
            .map(|(i, col)| format!("{col} = ?{}", i + 1))
            .collect();
        let n = vals.len();
        vals.push(Box::new(now_millis()));
        vals.push(Box::new(id.to_string()));
        let sql = format!(
            "UPDATE roadmap_items SET {}, updated_at = ?{} WHERE id = ?{}",
            assignments.join(", "),
            n + 1,
            n + 2
        );
        let refs: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| v.as_ref()).collect();
        conn.execute(&sql, refs.as_slice())?;
    }
    get(conn, id)
}

/// Delete an item. Returns whether a row was actually removed, so a caller
/// doesn't announce a deletion that didn't happen.
pub fn delete(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute("DELETE FROM roadmap_items WHERE id = ?1", [id])?;
    Ok(n > 0)
}

/// The next free code for a project ("FLT-142").
///
/// The number is `MAX(existing suffix) + 1`, computed in Rust rather than SQL so
/// codes that don't match the project's own prefix (an imported `#207`, a
/// hand-edited row) are skipped instead of poisoning the max. Must be called
/// with the connection lock held — see the module docs.
///
/// The max is taken over *live* rows, so deleting the highest-numbered item
/// hands its number back to the next one. That is the accepted cost of keeping
/// the allocator stateless (no counter to drift from the rows): an item deleted
/// off the board never shipped, so nothing outside the table quotes its code.
pub fn next_code(conn: &Connection, project_id: &str) -> rusqlite::Result<String> {
    let prefix = code_prefix(conn, project_id)?;
    let mut stmt = conn.prepare("SELECT code FROM roadmap_items WHERE project_id = ?1")?;
    let highest = stmt
        .query_map([project_id], |r| r.get::<_, String>(0))?
        .filter_map(|c| c.ok())
        .filter_map(|c| code_number(&c))
        .max()
        .unwrap_or(FIRST_NUMBER - 1);
    Ok(format!("{prefix}-{}", highest + 1))
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
    use super::*;
    use crate::database::get_migrations;
    use crate::roadmap::types::ItemSize;

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
    fn deleting_an_item_never_renumbers_the_survivors() {
        // A code is an item's identity — the PM quotes it, and later slices put
        // it in branch names and PR titles. Removing a neighbour must not move
        // anyone else's, and the next allocation must not collide with a live
        // one. (Deleting the *highest* item does free its number back: see
        // `next_code`.)
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        let one = create(&conn, &p, &titled("one")).unwrap();
        let two = create(&conn, &p, &titled("two")).unwrap();
        let three = create(&conn, &p, &titled("three")).unwrap();
        assert!(delete(&conn, &two.id).unwrap());

        let four = create(&conn, &p, &titled("four")).unwrap();
        assert_eq!(one.code, "FLE-100");
        assert_eq!(three.code, "FLE-102", "survivors keep their codes");
        assert_eq!(four.code, "FLE-103", "and the gap is not backfilled");
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

    #[test]
    fn create_defaults_and_round_trips_json_arrays() {
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");

        let bare = create(&conn, &p, &titled("bare")).unwrap();
        assert_eq!(bare.horizon, Horizon::Later, "an unplaced item is backlog");
        assert_eq!(bare.status, ItemStatus::Open);
        assert_eq!(bare.source, ItemSource::User);
        assert_eq!(bare.size, None);
        assert!(bare.accept.is_empty() && bare.deps.is_empty());
        assert_eq!(bare.why, "");
        assert!(bare.parent_id.is_none(), "v1 never writes a parent");

        let full = create(
            &conn,
            &p,
            &NewItem {
                title: "shaped".into(),
                why: "because".into(),
                horizon: Some(Horizon::Now),
                status: Some(ItemStatus::Proposed),
                size: Some(ItemSize::L),
                area: Some("runtime".into()),
                source: Some(ItemSource::Pm),
                epic: Some("persistence".into()),
                accept: vec!["survives a quit".into(), "reattaches".into()],
                deps: vec![bare.code.clone()],
                workflow_def_id: Some("wf-pipeline".into()),
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
        assert_eq!(stored.size, Some(ItemSize::L));
        // Assignable at creation, so the item form can create-and-assign in one
        // round-trip; unset on the bare row, which means "the project default".
        assert_eq!(stored.workflow_def_id.as_deref(), Some("wf-pipeline"));
        assert_eq!(bare.workflow_def_id, None);
    }

    #[test]
    fn a_wire_borne_null_clears_the_column() {
        // The other update tests build `ItemPatch` in Rust and bypass serde;
        // the frontend's patches arrive as JSON through the command layer.
        // This is the edit dialog's "Unsized" path, end to end.
        let conn = test_conn();
        let p = project(&conn, "p1", "fletch");
        let item = create(
            &conn,
            &p,
            &NewItem {
                title: "sized".into(),
                size: Some(ItemSize::M),
                area: Some("runtime".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let patch: ItemPatch = serde_json::from_str(r#"{"size": null}"#).unwrap();
        let row = update(&conn, &item.id, &patch).unwrap().unwrap();
        assert_eq!(row.size, None, "the dialog's clear must stick");
        assert_eq!(
            row.area.as_deref(),
            Some("runtime"),
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
                size: Some(ItemSize::M),
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
            moved.size,
            Some(ItemSize::M),
            "an absent field is left alone"
        );
        assert_eq!(moved.area.as_deref(), Some("runtime"));
        assert_eq!(moved.accept, vec!["one"]);
        assert_eq!(moved.code, item.code, "a code never moves");
        assert_eq!(moved.created_at, item.created_at);

        // An explicit null clears a nullable column; an empty list clears a
        // JSON one.
        let cleared = update(
            &conn,
            &item.id,
            &ItemPatch {
                size: Some(None),
                area: Some(None),
                accept: Some(vec![]),
                title: Some("retitled".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(cleared.size, None);
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

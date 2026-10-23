//! The `review_threads` table: the local reviewer's findings per checkout.
//! Plain rusqlite over the host database, in the style of `autopilot::store`;
//! the callers hold the connection.

use rusqlite::{params, Connection, OptionalExtension};

use super::model::{Finding, Reply, ReplyAuthor, ThreadRow};
use super::{now_ms, Key};
use crate::error::Result;
use crate::github::{PrComment, ReviewSeverity};

/// Every local thread id starts with this; GitHub node ids never do.
pub const LOCAL_ID_PREFIX: &str = "local:";

/// Whether a thread id names a local thread rather than a GitHub node.
pub fn is_local(id: &str) -> bool {
    id.starts_with(LOCAL_ID_PREFIX)
}

const COLUMNS: &str = "id, agent_id, subdir, round, severity, path, line, title, body, author, \
                       created_at, resolved_at, resolved_by, replies";

fn row(r: &rusqlite::Row) -> rusqlite::Result<ThreadRow> {
    let severity: Option<String> = r.get(4)?;
    let resolved_by: Option<String> = r.get(12)?;
    let replies: String = r.get(13)?;
    Ok(ThreadRow {
        id: r.get(0)?,
        agent_id: r.get(1)?,
        subdir: r.get(2)?,
        round: r.get::<_, i64>(3)? as u32,
        severity: severity.as_deref().and_then(ReviewSeverity::parse),
        path: r.get(5)?,
        line: r.get::<_, Option<i64>>(6)?.map(|n| n as u32),
        title: r.get(7)?,
        body: r.get(8)?,
        author: r.get(9)?,
        created_at: r.get(10)?,
        resolved_at: r.get(11)?,
        resolved_by: match resolved_by.as_deref() {
            Some("agent") => Some(ReplyAuthor::Agent),
            Some("user") => Some(ReplyAuthor::User),
            _ => None,
        },
        // A malformed column reads as no replies rather than failing the
        // whole read: the thread is still worth showing.
        replies: serde_json::from_str::<Vec<Reply>>(&replies).unwrap_or_default(),
    })
}

/// Store one review round's findings for `key`; returns the new thread ids in
/// the findings' order.
pub fn insert_findings(
    conn: &Connection,
    key: &Key,
    round: u32,
    author: &str,
    findings: &[Finding],
) -> Result<Vec<String>> {
    let now = now_ms();
    let mut ids = Vec::with_capacity(findings.len());
    for finding in findings {
        let id = format!("{LOCAL_ID_PREFIX}{}", uuid::Uuid::new_v4());
        conn.execute(
            "INSERT INTO review_threads (id, agent_id, subdir, round, severity, path, line, \
             title, body, author, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                key.0,
                key.1,
                round as i64,
                finding.severity.map(ReviewSeverity::as_str),
                finding.path,
                finding.line.map(|n| n as i64),
                finding.title.trim(),
                finding.body.trim(),
                author,
                now,
            ],
        )?;
        ids.push(id);
    }
    Ok(ids)
}

/// One thread by id, resolved or not.
pub fn get(conn: &Connection, id: &str) -> Result<Option<ThreadRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM review_threads WHERE id = ?1"),
            [id],
            row,
        )
        .optional()?)
}

/// The open threads of `key`, oldest first, as the panel renders them.
pub fn unresolved(conn: &Connection, key: &Key) -> Result<Vec<PrComment>> {
    Ok(unresolved_rows(conn, key)?
        .iter()
        .map(ThreadRow::to_comment)
        .collect())
}

/// The open threads of `key` as rows, oldest first.
pub fn unresolved_rows(conn: &Connection, key: &Key) -> Result<Vec<ThreadRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM review_threads \
         WHERE agent_id = ?1 AND subdir IS ?2 AND resolved_at IS NULL \
         ORDER BY created_at, rowid"
    ))?;
    let rows = stmt
        .query_map(params![key.0, key.1], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Append a reply. `false` when no such thread exists.
pub fn reply(conn: &Connection, id: &str, author: ReplyAuthor, body: &str) -> Result<bool> {
    let Some(mut thread) = get(conn, id)? else {
        return Ok(false);
    };
    thread.replies.push(Reply {
        author,
        body: body.trim().to_string(),
        at: now_ms(),
    });
    conn.execute(
        "UPDATE review_threads SET replies = ?2 WHERE id = ?1",
        params![id, serde_json::to_string(&thread.replies)?],
    )?;
    Ok(true)
}

/// Mark a thread resolved by `by`. `false` when no such thread exists;
/// resolving twice keeps the first resolution.
pub fn resolve(conn: &Connection, id: &str, by: ReplyAuthor) -> Result<bool> {
    if get(conn, id)?.is_none() {
        return Ok(false);
    }
    conn.execute(
        "UPDATE review_threads SET resolved_at = ?2, resolved_by = ?3 \
         WHERE id = ?1 AND resolved_at IS NULL",
        params![id, now_ms(), by.as_str()],
    )?;
    Ok(true)
}

/// `(open, resolved)` thread counts for `key`.
pub fn counts(conn: &Connection, key: &Key) -> Result<(u32, u32)> {
    let (open, resolved): (i64, i64) = conn.query_row(
        "SELECT COUNT(*) FILTER (WHERE resolved_at IS NULL), \
                COUNT(*) FILTER (WHERE resolved_at IS NOT NULL) \
         FROM review_threads WHERE agent_id = ?1 AND subdir IS ?2",
        params![key.0, key.1],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok((open as u32, resolved as u32))
}

/// Every thread about any checkout of `agent_id`, with the workspace.
pub fn delete_for_agent(conn: &Connection, agent_id: &str) -> Result<()> {
    conn.execute("DELETE FROM review_threads WHERE agent_id = ?1", [agent_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::database::get_migrations()
            .to_latest(&mut conn)
            .unwrap();
        conn
    }

    fn key(agent: &str, subdir: Option<&str>) -> Key {
        (agent.to_string(), subdir.map(str::to_string))
    }

    fn finding(title: &str, severity: Option<ReviewSeverity>) -> Finding {
        Finding {
            path: Some("src/a.rs".into()),
            line: Some(12),
            severity,
            title: title.into(),
            body: "detail".into(),
        }
    }

    #[test]
    fn findings_become_open_local_threads_in_order() {
        let c = conn();
        let k = key("arabia", None);
        let ids = insert_findings(
            &c,
            &k,
            1,
            "Reviewer",
            &[
                finding("first", Some(ReviewSeverity::High)),
                finding("second", None),
            ],
        )
        .unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.iter().all(|id| is_local(id)));

        let threads = unresolved(&c, &k).unwrap();
        assert_eq!(threads.len(), 2);
        let first = &threads[0];
        assert_eq!(first.id, ids[0]);
        assert_eq!(first.source, crate::github::ThreadSource::Local);
        assert_eq!(first.severity, Some(ReviewSeverity::High));
        assert_eq!(first.round, Some(1));
        assert_eq!(first.author, "Reviewer");
        assert!(first.is_bot);
        assert_eq!(first.body, "first\n\ndetail");
        assert_eq!(first.path.as_deref(), Some("src/a.rs"));
        assert_eq!(first.line, Some(12));
        assert_eq!(first.url, None);
        assert_eq!(first.replies, 0);
        assert!(!first.we_replied_last);
        assert_eq!(threads[1].severity, None);
        assert_eq!(counts(&c, &k).unwrap(), (2, 0));

        // Another checkout of the same agent, and another agent, see nothing.
        assert!(unresolved(&c, &key("arabia", Some("api")))
            .unwrap()
            .is_empty());
        assert!(unresolved(&c, &key("dolomites", None)).unwrap().is_empty());
    }

    #[test]
    fn a_coders_reply_is_a_push_back_until_the_user_answers_or_it_resolves() {
        let c = conn();
        let k = key("arabia", None);
        let id = insert_findings(&c, &k, 1, "Reviewer", &[finding("f", None)]).unwrap()[0].clone();

        assert!(reply(&c, &id, ReplyAuthor::Agent, "disagree: by design").unwrap());
        let t = &unresolved(&c, &k).unwrap()[0];
        assert_eq!(t.replies, 1);
        assert!(t.we_replied_last, "the coder had the last word");

        assert!(reply(&c, &id, ReplyAuthor::User, "no, fix it").unwrap());
        let t = &unresolved(&c, &k).unwrap()[0];
        assert_eq!(t.replies, 2);
        assert!(!t.we_replied_last, "the user answered: back to the coder");

        assert!(resolve(&c, &id, ReplyAuthor::Agent).unwrap());
        assert!(unresolved(&c, &k).unwrap().is_empty());
        assert_eq!(counts(&c, &k).unwrap(), (0, 1));
        let row = get(&c, &id).unwrap().unwrap();
        assert!(row.resolved());
        assert_eq!(row.resolved_by, Some(ReplyAuthor::Agent));
        assert!(!row.we_replied_last(), "a resolved thread awaits nobody");

        // Unknown ids are reported, not invented.
        assert!(!reply(&c, "local:nope", ReplyAuthor::Agent, "x").unwrap());
        assert!(!resolve(&c, "local:nope", ReplyAuthor::User).unwrap());
    }

    #[test]
    fn a_deleted_agent_takes_its_threads_along() {
        let c = conn();
        insert_findings(&c, &key("arabia", None), 1, "R", &[finding("a", None)]).unwrap();
        insert_findings(
            &c,
            &key("arabia", Some("api")),
            1,
            "R",
            &[finding("b", None)],
        )
        .unwrap();
        insert_findings(&c, &key("dolomites", None), 1, "R", &[finding("c", None)]).unwrap();
        delete_for_agent(&c, "arabia").unwrap();
        assert_eq!(counts(&c, &key("arabia", None)).unwrap(), (0, 0));
        assert_eq!(counts(&c, &key("arabia", Some("api"))).unwrap(), (0, 0));
        assert_eq!(counts(&c, &key("dolomites", None)).unwrap(), (1, 0));
    }

    #[test]
    fn local_ids_are_told_from_github_node_ids() {
        assert!(is_local("local:4d0c"));
        assert!(!is_local("PRRT_kwDOA"));
        assert!(!is_local(""));
    }
}

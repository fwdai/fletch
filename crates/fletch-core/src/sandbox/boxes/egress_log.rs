//! The record of every connection a box's egress proxy answered, kept in the
//! host's database (`egress_log`, migration 0056). What the Network lists
//! read, grouped by host: what agents reach, what they were refused, and how
//! much went up. Under the open posture it is the one thing that says what
//! left the VM, and the ground a later look at the hosts themselves would
//! stand on.
//!
//! Written from the proxy's event path, which has no engine context, so the
//! database handle is installed once at boot like the event sink
//! (`boxes::set_event_sink`). A write that fails is logged and dropped: the
//! record is advisory, the connection was already answered.

use std::sync::OnceLock;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::Result;
use crate::roadmap::Db;

/// Rows older than this are pruned at boot.
pub const RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// The most hosts one read answers.
const MAX_HOSTS: usize = 200;

static DB: OnceLock<Db> = OnceLock::new();

/// Where the records go. Installed once at boot.
pub fn install(db: Db) {
    if DB.set(db).is_err() {
        tracing::warn!("egress log database already set");
    }
}

/// One answered connection.
#[derive(Debug, Clone)]
pub struct Record<'a> {
    pub agent_id: &'a str,
    pub project_id: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub allowed: bool,
    /// `listed` | `open` | `turn` for a tunnel; `host` | `port` | `quiet` for
    /// a refusal (the migration's comment).
    pub via: &'static str,
    pub bytes_up: u64,
    pub bytes_down: u64,
}

/// Append `record` now.
pub fn record(record: &Record<'_>) {
    let Some(db) = DB.get() else {
        return;
    };
    let conn = db.lock();
    if let Err(e) = insert(&conn, record) {
        tracing::warn!(
            agent_id = record.agent_id,
            host = record.host,
            error = %e,
            "egress log write failed"
        );
    }
}

fn insert(conn: &Connection, r: &Record<'_>) -> Result<()> {
    conn.execute(
        "INSERT INTO egress_log \
         (at, agent_id, project_id, host, port, allowed, via, bytes_up, bytes_down) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            crate::database::now_millis(),
            r.agent_id,
            r.project_id,
            r.host,
            r.port,
            r.allowed as i64,
            r.via,
            r.bytes_up as i64,
            r.bytes_down as i64,
        ],
    )?;
    Ok(())
}

/// Drop rows past [`RETENTION_MS`].
pub fn prune(conn: &Connection) {
    let cutoff = crate::database::now_millis() - RETENTION_MS;
    if let Err(e) = conn.execute("DELETE FROM egress_log WHERE at < ?1", [cutoff]) {
        tracing::warn!(error = %e, "egress log prune failed");
    }
}

/// One host as the Network lists show it: every connection to it in the
/// window, across the workspaces that reached it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostActivity {
    pub host: String,
    /// Tunnels opened.
    pub allowed: u64,
    /// Connections refused.
    pub refused: u64,
    /// When the newest connection was answered, unix ms.
    pub last_at: i64,
    /// How the newest connection was answered (`Record::via`).
    pub last_via: String,
    pub bytes_up: u64,
    pub bytes_down: u64,
    /// The workspaces that reached for it, newest first.
    pub agent_ids: Vec<String>,
    /// Known telemetry (`fletch_box::egress::QUIET`).
    pub quiet: bool,
}

/// The hosts reached since `since` (unix ms), newest first; with
/// `project_id`, those project's boxes only.
pub fn activity(
    conn: &Connection,
    project_id: Option<&str>,
    since: i64,
) -> Result<Vec<HostActivity>> {
    // The filter is spelled once; `?2` is ignored when every project is asked.
    let mut stmt = conn.prepare(
        "SELECT host, \
                SUM(allowed), SUM(1 - allowed), MAX(at), SUM(bytes_up), SUM(bytes_down), \
                (SELECT via FROM egress_log n \
                   WHERE n.host = e.host AND n.at >= ?1 AND (?2 IS NULL OR n.project_id = ?2) \
                   ORDER BY n.at DESC, n.id DESC LIMIT 1), \
                (SELECT GROUP_CONCAT(agent_id, '\n') FROM \
                   (SELECT agent_id FROM egress_log a \
                     WHERE a.host = e.host AND a.at >= ?1 AND (?2 IS NULL OR a.project_id = ?2) \
                     GROUP BY agent_id ORDER BY MAX(a.at) DESC)) \
         FROM egress_log e \
         WHERE at >= ?1 AND (?2 IS NULL OR project_id = ?2) \
         GROUP BY host \
         ORDER BY MAX(at) DESC \
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![since, project_id, MAX_HOSTS as i64], |r| {
        let host: String = r.get(0)?;
        let agents: Option<String> = r.get(7)?;
        Ok(HostActivity {
            quiet: fletch_box::egress::is_quiet(&host),
            host,
            allowed: r.get::<_, i64>(1)?.max(0) as u64,
            refused: r.get::<_, i64>(2)?.max(0) as u64,
            last_at: r.get(3)?,
            bytes_up: r.get::<_, i64>(4)?.max(0) as u64,
            bytes_down: r.get::<_, i64>(5)?.max(0) as u64,
            last_via: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            agent_ids: agents
                .unwrap_or_default()
                .split('\n')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(agent: &'a str, project: &'a str, host: &'a str, allowed: bool) -> Record<'a> {
        Record {
            agent_id: agent,
            project_id: project,
            host,
            port: 443,
            allowed,
            via: if allowed { "open" } else { "host" },
            bytes_up: if allowed { 10 } else { 0 },
            bytes_down: if allowed { 100 } else { 0 },
        }
    }

    /// Rows group by host, newest first, with the counts, the bytes, the
    /// workspaces and how the last one was answered; a project filter keeps
    /// its own; a prune drops what is past the window.
    #[test]
    fn activity_groups_by_host_and_a_project_sees_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        let conn = db.lock();
        insert(&conn, &row("fuji", "p1", "api.stripe.com", true)).unwrap();
        insert(&conn, &row("etna", "p1", "api.stripe.com", true)).unwrap();
        insert(&conn, &row("fuji", "p1", "evil.test", false)).unwrap();
        insert(&conn, &row("rainier", "p2", "api.stripe.com", true)).unwrap();
        insert(
            &conn,
            &Record {
                via: "listed",
                ..row("rainier", "p2", "registry.npmjs.org", true)
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO egress_log (at, agent_id, project_id, host, port, allowed, via) \
             VALUES (1, 'old', 'p1', 'old.test', 443, 1, 'open')",
            [],
        )
        .unwrap();

        let all = activity(&conn, None, 0).unwrap();
        let stripe = all.iter().find(|h| h.host == "api.stripe.com").unwrap();
        assert_eq!((stripe.allowed, stripe.refused), (3, 0));
        assert_eq!((stripe.bytes_up, stripe.bytes_down), (30, 300));
        assert_eq!(stripe.agent_ids.len(), 3);
        assert_eq!(stripe.agent_ids[0], "rainier", "newest first");
        assert_eq!(stripe.last_via, "open");
        assert!(!stripe.quiet);
        let evil = all.iter().find(|h| h.host == "evil.test").unwrap();
        assert_eq!((evil.allowed, evil.refused), (0, 1));
        assert_eq!(evil.last_via, "host");
        assert_eq!(all.last().unwrap().host, "old.test", "oldest last");

        let p2 = activity(&conn, Some("p2"), 0).unwrap();
        let hosts: Vec<&str> = p2.iter().map(|h| h.host.as_str()).collect();
        assert_eq!(hosts, ["registry.npmjs.org", "api.stripe.com"]);
        assert_eq!(p2[1].agent_ids, vec!["rainier".to_string()]);
        assert_eq!(p2[0].last_via, "listed");

        prune(&conn);
        assert!(activity(&conn, None, 0)
            .unwrap()
            .iter()
            .all(|h| h.host != "old.test"));

        insert(&conn, &row("fuji", "p1", "o1.ingest.sentry.io", false)).unwrap();
        let quiet = activity(&conn, Some("p1"), 0).unwrap();
        assert!(
            quiet
                .iter()
                .find(|h| h.host == "o1.ingest.sentry.io")
                .unwrap()
                .quiet
        );
    }

    /// With no database installed a record is dropped, not a panic: the
    /// connection it describes was already answered.
    #[test]
    fn recording_without_a_database_is_a_no_op() {
        record(&row("fuji", "p", "x.test", true));
    }
}

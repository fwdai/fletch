//! Autopilot's durable half: the two switches and the history log.
//!
//! The switches keep the keys and encodings the desktop wrote while it ran the
//! loop, so an existing database keeps its opt-outs: `project_settings`
//! `autopilot.enabled` (`"0"`/`"false"` = off, anything else or no row = on)
//! and `settings` `autopilotPausedAgents` (a JSON array of agent ids). Only the
//! host writes them now (`autopilot_set`).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::step::GiveUpReason;
use crate::error::{Error, Result};
use crate::supervisor::delegation::DelegationKind;

/// Per-project switch. On by default: the row exists only once switched off.
pub const PROJECT_ENABLED_KEY: &str = "autopilot.enabled";

/// Agents (workspaces) paused from a client, as one JSON array.
pub const PAUSED_AGENTS_KEY: &str = "autopilotPausedAgents";

/// Rows kept per checkout. Enough for several complete ladder exhaustions (the
/// four budgets sum to 9 cycles, each a dispatch plus its outcome); beyond
/// that the oldest rows answer a question nobody is asking.
pub const LOG_LIMIT: usize = 50;

/// Whether a stored `autopilot.enabled` value means off. Anything but an
/// explicit off reads as on — a loop that is on by default must not switch off
/// on a typo. Same rule the desktop used.
fn project_off(value: &str) -> bool {
    value == "0" || value == "false"
}

/// The two switches, as one read; also the `autopilot:switches` payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Switches {
    pub disabled_projects: Vec<String>,
    pub paused_agents: Vec<String>,
}

impl Switches {
    pub fn project_on(&self, project_id: &str) -> bool {
        !self.disabled_projects.iter().any(|p| p == project_id)
    }

    pub fn paused(&self, agent_id: &str) -> bool {
        self.paused_agents.iter().any(|a| a == agent_id)
    }

    /// Whether autopilot may act on this agent's checkouts at all.
    pub fn agent_on(&self, project_id: &str, agent_id: &str) -> bool {
        self.project_on(project_id) && !self.paused(agent_id)
    }
}

/// A stored pause list. A value that is not a JSON array of strings reads as
/// empty, as the desktop's parser did.
fn parse_paused(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|r| serde_json::from_str::<Vec<String>>(r).ok())
        .unwrap_or_default()
}

/// Read both switches. An error here must stop the caller from acting: on by
/// default without knowing who opted out would act on exactly the projects
/// that said no.
pub fn read_switches(conn: &Connection) -> rusqlite::Result<Switches> {
    let mut stmt = conn.prepare("SELECT project_id, value FROM project_settings WHERE key = ?1")?;
    let rows = stmt.query_map([PROJECT_ENABLED_KEY], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut disabled_projects = Vec::new();
    for row in rows {
        let (project_id, value) = row?;
        if project_off(&value) {
            disabled_projects.push(project_id);
        }
    }
    disabled_projects.sort();
    let paused: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [PAUSED_AGENTS_KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(Switches {
        disabled_projects,
        paused_agents: parse_paused(paused.as_deref()),
    })
}

/// Flip a project's switch. On is the default, so "on" deletes the row.
pub fn set_project(conn: &Connection, project_id: &str, enabled: bool) -> Result<()> {
    if enabled {
        conn.execute(
            "DELETE FROM project_settings WHERE project_id = ?1 AND key = ?2",
            params![project_id, PROJECT_ENABLED_KEY],
        )?;
    } else {
        conn.execute(
            "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, '0')
             ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
            params![project_id, PROJECT_ENABLED_KEY],
        )?;
    }
    Ok(())
}

/// Pause (or resume) one agent. Written whole; ids for which `exists` says no
/// are pruned on the way out, so the list can't grow with every discarded agent
/// that was ever paused.
pub fn set_agent_paused(
    conn: &Connection,
    agent_id: &str,
    paused: bool,
    exists: impl Fn(&str) -> bool,
) -> Result<()> {
    let current: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [PAUSED_AGENTS_KEY],
            |r| r.get(0),
        )
        .optional()?;
    let mut list: Vec<String> = parse_paused(current.as_deref())
        .into_iter()
        .filter(|id| id != agent_id && exists(id))
        .collect();
    if paused {
        list.push(agent_id.to_string());
    }
    let encoded = serde_json::to_string(&list).map_err(|e| Error::Other(e.to_string()))?;
    crate::database::set_setting(conn, PAUSED_AGENTS_KEY, &encoded)
}

/// What a log row records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Dispatch,
    Settle,
    Retry,
    GiveUp,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Dispatch => "dispatch",
            Outcome::Settle => "settle",
            Outcome::Retry => "retry",
            Outcome::GiveUp => "give-up",
        }
    }
}

/// One thing autopilot did: the `autopilot:event` payload and an
/// `autopilot_log` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: String,
    pub agent_id: String,
    pub subdir: Option<String>,
    /// Epoch ms of the decision behind it.
    pub at: i64,
    pub outcome: Outcome,
    pub rung: DelegationKind,
    /// 1-based cycle attempt.
    pub attempt: u32,
    /// Only ever set on a `give-up`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<GiveUpReason>,
}

fn json_str<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

fn from_str<T: for<'de> Deserialize<'de>>(raw: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

/// Append one row and drop that checkout's rows beyond [`LOG_LIMIT`].
pub fn append_log(conn: &Connection, entry: &LogEntry) -> Result<()> {
    conn.execute(
        "INSERT INTO autopilot_log (id, agent_id, subdir, at, outcome, rung, attempt, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            entry.id,
            entry.agent_id,
            entry.subdir,
            entry.at,
            entry.outcome.as_str(),
            json_str(&entry.rung),
            entry.attempt,
            entry.reason.map(GiveUpReason::as_str),
        ],
    )?;
    conn.execute(
        "DELETE FROM autopilot_log WHERE agent_id = ?1 AND subdir IS ?2 AND rowid NOT IN (
             SELECT rowid FROM autopilot_log WHERE agent_id = ?1 AND subdir IS ?2
             ORDER BY at DESC, rowid DESC LIMIT ?3)",
        params![entry.agent_id, entry.subdir, LOG_LIMIT as i64],
    )?;
    Ok(())
}

/// Which rows [`read_log`] returns.
pub enum LogScope<'a> {
    All,
    Agent(&'a str),
    Checkout(&'a str, Option<&'a str>),
}

/// Rows newest first. A row whose rung or outcome this build can't read is
/// skipped rather than failing the read.
pub fn read_log(conn: &Connection, scope: LogScope) -> Result<Vec<LogEntry>> {
    let base = "SELECT id, agent_id, subdir, at, outcome, rung, attempt, reason FROM autopilot_log";
    let order = "ORDER BY at DESC, rowid DESC";
    let map = |r: &rusqlite::Row| -> rusqlite::Result<Option<LogEntry>> {
        let outcome: String = r.get(4)?;
        let rung: String = r.get(5)?;
        let reason: Option<String> = r.get(7)?;
        let (Some(outcome), Some(rung)) = (from_str(&outcome), from_str(&rung)) else {
            return Ok(None);
        };
        Ok(Some(LogEntry {
            id: r.get(0)?,
            agent_id: r.get(1)?,
            subdir: r.get(2)?,
            at: r.get(3)?,
            outcome,
            rung,
            attempt: r.get(6)?,
            reason: reason.as_deref().and_then(from_str),
        }))
    };
    let rows: Vec<Option<LogEntry>> = match scope {
        LogScope::All => conn
            .prepare(&format!("{base} {order}"))?
            .query_map([], map)?
            .collect::<rusqlite::Result<_>>()?,
        LogScope::Agent(agent_id) => conn
            .prepare(&format!("{base} WHERE agent_id = ?1 {order}"))?
            .query_map([agent_id], map)?
            .collect::<rusqlite::Result<_>>()?,
        LogScope::Checkout(agent_id, subdir) => conn
            .prepare(&format!(
                "{base} WHERE agent_id = ?1 AND subdir IS ?2 {order}"
            ))?
            .query_map(params![agent_id, subdir], map)?
            .collect::<rusqlite::Result<_>>()?,
    };
    Ok(rows.into_iter().flatten().collect())
}

/// Drop the history of agents that no longer exist (discarded). Archived
/// agents keep theirs: they can be restored.
pub fn prune_orphans(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM autopilot_log WHERE agent_id NOT IN (SELECT id FROM workspaces)",
        [],
    )?;
    Ok(())
}

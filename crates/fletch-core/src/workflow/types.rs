//! Workflow v1 domain types — the run / attempt / event rows and their status
//! enums, with serde for the command surface and `from_row` constructors for the
//! journal + read commands.
//!
//! The spec types (`Spec`, `Step`, `Gate`, …) deliberately live in `spec.rs`:
//! this module only models what is persisted in the v1 tables. JSON-typed columns (`spec_json`, `payload_json`, …) are parsed
//! into `serde_json::Value` so the frontend receives real objects, not strings.

use rusqlite::types::Type;
use rusqlite::Row;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Declares a string-backed enum with a single source of truth for the
/// on-disk / on-wire spelling: `as_str` (DB write + serialize) and `from_db`
/// (DB read + deserialize) can never drift apart.
///
/// Exported at the crate root (`crate::db_enum!`) because every module with
/// status-string columns wants the same guarantee — see `roadmap::types`.
#[macro_export]
macro_rules! db_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self { $(Self::$variant => $s),+ }
            }
            pub fn from_db(s: &str) -> Option<Self> {
                match s { $($s => Some(Self::$variant),)+ _ => None }
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
                ser.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                let s = String::deserialize(de)?;
                Self::from_db(&s).ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        concat!("invalid ", stringify!($name), " value: {}"),
                        s
                    ))
                })
            }
        }
    };
}

db_enum! {
    /// Run lifecycle (§6.2): `pending → running → done | failed | canceled`.
    RunStatus {
        Pending  => "pending",
        Running  => "running",
        Done     => "done",
        Failed   => "failed",
        Canceled => "canceled",
    }
}

db_enum! {
    /// Step-attempt lifecycle (§6.2). The kernel writes `spawning → running →
    /// done | blocked | error | abandoned`; `pending`, `gating` and
    /// `awaiting_approval` appear only on rows an earlier engine wrote, and stay
    /// readable so those runs still open. Once terminal, a row is never mutated
    /// again.
    AttemptStatus {
        Pending           => "pending",
        Spawning          => "spawning",
        Running           => "running",
        Gating            => "gating",
        Done              => "done",
        Blocked           => "blocked",
        AwaitingApproval  => "awaiting_approval",
        Error             => "error",
        Abandoned         => "abandoned",
    }
}

/// The journal event type names (§7.1). Kept as string constants rather than an
/// enum so a layer can attach payloads and add types without a central match to
/// touch. These are what the kernel writes; a run an earlier engine journaled
/// may carry other names, which `Event` passes through as plain strings.
pub mod event_type {
    pub const RUN_LAUNCHED: &str = "run_launched";
    pub const RUN_DONE: &str = "run_done";
    pub const RUN_FAILED: &str = "run_failed";
    pub const RUN_CANCELED: &str = "run_canceled";
    pub const ATTEMPT_SPAWNED: &str = "attempt_spawned";
    /// A step spawned without skills its definition requested — they no longer
    /// resolve against the local library (deleted since the save/import).
    /// Carries the unresolved names/ids; the step still runs (warn-don't-fail,
    /// the same policy as import).
    pub const SKILLS_MISSING: &str = "skills_missing";
    /// A step spawned without MCP servers its custom agent assigned — the
    /// server rows no longer resolve (deleted since the save). Carries the
    /// unresolved ids; the step still runs. Provider-filtered transports are
    /// not reported (by-design gating, mirrors the agent editor).
    pub const MCP_SERVERS_MISSING: &str = "mcp_servers_missing";
    /// A step's `custom_agent` no longer resolves (deleted since the save) —
    /// the step spawned without that agent's skills and MCP servers. Carries
    /// the unresolved id; the step still runs.
    pub const CUSTOM_AGENT_MISSING: &str = "custom_agent_missing";
    /// A by-name skill reference matched more than one library row (names are
    /// not unique). The step still runs against a deterministic pick (lowest
    /// id); the event carries the ambiguous names so the timeline is honest
    /// about which of several same-named skills was chosen.
    pub const SKILLS_AMBIGUOUS: &str = "skills_ambiguous";
    /// A by-name MCP server reference matched more than one library row (names
    /// are not unique). The step still runs against a deterministic pick
    /// (lowest id); the event carries the ambiguous names. Only the by-name
    /// path (imported/spec-level servers) can be ambiguous — custom-agent
    /// assignments resolve by unique id.
    pub const MCP_SERVERS_AMBIGUOUS: &str = "mcp_servers_ambiguous";
    pub const ATTEMPT_READY: &str = "attempt_ready";
    pub const PROMPT_SENT: &str = "prompt_sent";
    pub const TURN_ENDED: &str = "turn_ended";
    pub const GATE_EVALUATED: &str = "gate_evaluated";
    pub const BOUNDARY_COMMIT: &str = "boundary_commit";
    pub const ATTEMPT_ABANDONED: &str = "attempt_abandoned";
    pub const ATTEMPT_ERROR: &str = "attempt_error";
    pub const FINALIZE_PUSHED: &str = "finalize_pushed";
    pub const FINALIZE_PR: &str = "finalize_pr";
}

/// A `wf_run` row (§4). JSON columns are exposed as parsed objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub definition_id: Option<String>,
    pub name: String,
    pub spec: Value,
    pub task: String,
    pub project_id: String,
    pub repo_path: String,
    pub run_dir: String,
    pub branch: String,
    pub base_sha: String,
    pub status: RunStatus,
    pub error: Option<String>,
    /// The PR this run's finalize opened (0029). Both `None` until a finalize
    /// actually proposes one — a `push`-only spec, a `pr create` that failed,
    /// or a run that never reached finalize all leave them unset. Persisted so
    /// "did this run's PR merge?" is a column read rather than a journal scan;
    /// the roadmap merge sweep is the caller that needs it.
    pub pr_number: Option<i64>,
    pub pr_url: Option<String>,
    /// The roadmap item this run was dispatched for (0028), or `None` for a run
    /// launched by hand. Exposed on the row so the monitor can show which item a
    /// run is building without a second query — the back-link the roadmap
    /// drainer already relies on, read from the other end.
    pub roadmap_item_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Run {
    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get("id")?,
            definition_id: r.get("definition_id")?,
            name: r.get("name")?,
            spec: json_col(r, "spec_json")?,
            task: r.get("task")?,
            project_id: r.get("project_id")?,
            repo_path: r.get("repo_path")?,
            run_dir: r.get("run_dir")?,
            branch: r.get("branch")?,
            base_sha: r.get("base_sha")?,
            status: enum_col(r, "status", RunStatus::from_db)?,
            error: r.get("error")?,
            pr_number: r.get("pr_number")?,
            pr_url: r.get("pr_url")?,
            roadmap_item_id: r.get("roadmap_item_id")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        })
    }
}

/// A `wf_step_exec` row (§4) — one execution of a step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepExec {
    pub id: String,
    pub run_id: String,
    pub step_id: String,
    pub attempt: i64,
    pub iteration: i64,
    pub agent_id: Option<String>,
    pub status: AttemptStatus,
    pub gate_mode: String,
    pub head_start: Option<String>,
    pub head_end: Option<String>,
    pub verdict: Option<Value>,
    pub error: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

impl StepExec {
    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get("id")?,
            run_id: r.get("run_id")?,
            step_id: r.get("step_id")?,
            attempt: r.get("attempt")?,
            iteration: r.get("iteration")?,
            agent_id: r.get("agent_id")?,
            status: enum_col(r, "status", AttemptStatus::from_db)?,
            gate_mode: r.get("gate_mode")?,
            head_start: r.get("head_start")?,
            head_end: r.get("head_end")?,
            verdict: opt_json_col(r, "verdict_json")?,
            error: r.get("error")?,
            started_at: r.get("started_at")?,
            ended_at: r.get("ended_at")?,
        })
    }
}

/// A `wf_event` row (§4, §7.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub run_id: String,
    pub seq: i64,
    pub ts: i64,
    pub step_exec_id: Option<String>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: Value,
}

impl Event {
    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            run_id: r.get("run_id")?,
            seq: r.get("seq")?,
            ts: r.get("ts")?,
            step_exec_id: r.get("step_exec_id")?,
            event_type: r.get("type")?,
            payload: json_col(r, "payload_json")?,
        })
    }
}

/// `wf_get_run` payload (§7.2): a run plus its attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDetail {
    pub run: Run,
    pub attempts: Vec<StepExec>,
}

// ───────────────────────────── row helpers ──────────────────────────────

fn conversion_err(col: &str, detail: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, Type::Text, format!("{col}: {detail}").into())
}

/// Parse a required JSON TEXT column into a `Value`.
fn json_col(r: &Row, col: &str) -> rusqlite::Result<Value> {
    let raw: String = r.get(col)?;
    serde_json::from_str(&raw).map_err(|e| conversion_err(col, e.to_string()))
}

/// Parse a nullable JSON TEXT column into `Option<Value>`.
fn opt_json_col(r: &Row, col: &str) -> rusqlite::Result<Option<Value>> {
    let raw: Option<String> = r.get(col)?;
    match raw {
        None => Ok(None),
        Some(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| conversion_err(col, e.to_string())),
    }
}

/// Parse a required enum TEXT column via its `from_db`.
fn enum_col<T>(r: &Row, col: &str, parse: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    let raw: String = r.get(col)?;
    parse(&raw).ok_or_else(|| conversion_err(col, format!("unexpected value {raw:?}")))
}

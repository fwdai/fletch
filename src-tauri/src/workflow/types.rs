//! Workflow v1 domain types (TECH_SPEC §4, §6.2, §7.1) — the run / attempt /
//! event / message rows and their status enums, with serde for the command
//! surface and `from_row` constructors for the journal + read commands.
//!
//! The spec/block types (`Spec`, `Block`, `Gate`, …) deliberately live in
//! `spec.rs` (a separate slice): this module only models what is persisted in
//! the v1 tables. JSON-typed columns (`spec_json`, `payload_json`, …) are parsed
//! into `serde_json::Value` so the frontend receives real objects, not strings.

use rusqlite::types::Type;
use rusqlite::Row;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Declares a string-backed enum with a single source of truth for the
/// on-disk / on-wire spelling: `as_str` (DB write + serialize) and `from_db`
/// (DB read + deserialize) can never drift apart.
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
    /// Run lifecycle (§6.2): `pending → running → { paused ⇄ running } →
    /// done | failed | canceled`.
    RunStatus {
        Pending  => "pending",
        Running  => "running",
        Paused   => "paused",
        Done     => "done",
        Failed   => "failed",
        Canceled => "canceled",
    }
}

db_enum! {
    /// Why a run is paused (§6.2). Set only while `RunStatus::Paused`.
    PausedReason {
        Approval       => "approval",
        Question       => "question",
        BlockedGate    => "blocked_gate",
        BudgetExceeded => "budget_exceeded",
        Conflict       => "conflict",
        Stalled        => "stalled",
    }
}

db_enum! {
    /// Step-attempt lifecycle (§6.2). The last five are terminal and, once
    /// reached, the attempt row is never mutated again.
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

db_enum! {
    /// `wf_message.kind` (§10).
    MessageKind {
        Report   => "report",
        Ask      => "ask",
        Answer   => "answer",
        Notify   => "notify",
        Decision => "decision",
    }
}

db_enum! {
    /// `wf_message.status` (§10.4).
    MessageStatus {
        Queued    => "queued",
        Delivered => "delivered",
        Answered  => "answered",
        Expired   => "expired",
    }
}

/// The journal event type names (§7.1). Kept as string constants rather than an
/// enum so later slices can attach payloads and add types without a central
/// match to touch; the journal's `type` column is always one of these.
pub mod event_type {
    pub const RUN_LAUNCHED: &str = "run_launched";
    pub const RUN_RESUMED: &str = "run_resumed";
    pub const RUN_PAUSED: &str = "run_paused";
    pub const RUN_DONE: &str = "run_done";
    pub const RUN_FAILED: &str = "run_failed";
    pub const RUN_CANCELED: &str = "run_canceled";
    pub const ATTEMPT_SPAWNED: &str = "attempt_spawned";
    pub const ATTEMPT_READY: &str = "attempt_ready";
    pub const PROMPT_SENT: &str = "prompt_sent";
    pub const TURN_ENDED: &str = "turn_ended";
    pub const GATE_EVALUATED: &str = "gate_evaluated";
    pub const BOUNDARY_COMMIT: &str = "boundary_commit";
    pub const ATTEMPT_ABANDONED: &str = "attempt_abandoned";
    pub const ATTEMPT_ERROR: &str = "attempt_error";
    pub const WATCHDOG_STALLED: &str = "watchdog_stalled";
    pub const BUDGET_TICK: &str = "budget_tick";
    pub const BUDGET_EXCEEDED: &str = "budget_exceeded";
    pub const MESSAGE_ROUTED: &str = "message_routed";
    pub const DECISION: &str = "decision";
    pub const CHILD_SPAWN_REQUESTED: &str = "child_spawn_requested";
    pub const CHILD_SPAWN_APPROVED: &str = "child_spawn_approved";
    pub const CHILD_SPAWN_DENIED: &str = "child_spawn_denied";
    pub const SUBRUN_LAUNCHED: &str = "subrun_launched";
    pub const SUBRUN_FINISHED: &str = "subrun_finished";
    pub const MERGE_STARTED: &str = "merge_started";
    pub const MERGE_CONFLICT: &str = "merge_conflict";
    pub const MERGE_DONE: &str = "merge_done";
    pub const FINALIZE_PUSHED: &str = "finalize_pushed";
    pub const FINALIZE_PR: &str = "finalize_pr";
}

/// A `wf_run` row (§4). JSON columns are exposed as parsed objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub definition_id: Option<String>,
    pub parent_run_id: Option<String>,
    pub name: String,
    pub spec: Value,
    pub task: String,
    pub project_id: String,
    pub repo_path: String,
    pub run_dir: String,
    pub branch: String,
    pub base_sha: String,
    pub status: RunStatus,
    pub paused_reason: Option<PausedReason>,
    pub cursor: Option<Value>,
    pub budgets: Value,
    pub spent: Value,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Run {
    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get("id")?,
            definition_id: r.get("definition_id")?,
            parent_run_id: r.get("parent_run_id")?,
            name: r.get("name")?,
            spec: json_col(r, "spec_json")?,
            task: r.get("task")?,
            project_id: r.get("project_id")?,
            repo_path: r.get("repo_path")?,
            run_dir: r.get("run_dir")?,
            branch: r.get("branch")?,
            base_sha: r.get("base_sha")?,
            status: enum_col(r, "status", RunStatus::from_db)?,
            paused_reason: opt_enum_col(r, "paused_reason", PausedReason::from_db)?,
            cursor: opt_json_col(r, "cursor_json")?,
            budgets: json_col(r, "budgets_json")?,
            spent: json_col(r, "spent_json")?,
            error: r.get("error")?,
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

/// A `wf_message` row (§4, §10).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub run_id: String,
    pub from_step_exec_id: Option<String>,
    pub to_step_exec_id: Option<String>,
    pub kind: MessageKind,
    pub body: Value,
    pub status: MessageStatus,
    pub created_at: i64,
    pub delivered_at: Option<i64>,
}

impl Message {
    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get("id")?,
            run_id: r.get("run_id")?,
            from_step_exec_id: r.get("from_step_exec_id")?,
            to_step_exec_id: r.get("to_step_exec_id")?,
            kind: enum_col(r, "kind", MessageKind::from_db)?,
            body: json_col(r, "body_json")?,
            status: enum_col(r, "status", MessageStatus::from_db)?,
            created_at: r.get("created_at")?,
            delivered_at: r.get("delivered_at")?,
        })
    }
}

/// `wf_get_run` payload (§7.2): a run plus its attempts and messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDetail {
    pub run: Run,
    pub attempts: Vec<StepExec>,
    pub messages: Vec<Message>,
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

/// Parse a nullable enum TEXT column via its `from_db`.
fn opt_enum_col<T>(
    r: &Row,
    col: &str,
    parse: fn(&str) -> Option<T>,
) -> rusqlite::Result<Option<T>> {
    let raw: Option<String> = r.get(col)?;
    match raw {
        None => Ok(None),
        Some(s) => parse(&s)
            .map(Some)
            .ok_or_else(|| conversion_err(col, format!("unexpected value {s:?}"))),
    }
}

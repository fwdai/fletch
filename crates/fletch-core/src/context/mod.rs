//! The project context layer: what a project is made of, what has been decided
//! about it, and where each piece of that came from — the knowledge that is
//! not in the code, served to agents so they do not re-explore the project to
//! reconstruct it.
//!
//! Shape: an insert-only event log in `context.db` (attached to the main
//! connection as schema `context`), projected into entity / assertion / edge
//! tables that a replay can rebuild at any time; a pure [`compile`] step that
//! turns one project's [`Graph`] into the [`Bundle`] an agent or the UI reads;
//! and a pipeline of observations → proposals → events through which agents,
//! ingesters and the user all write.
//!
//! Rules:
//! - `events` is never updated or deleted; assertions are immutable. Changing
//!   a decision records a new assertion that supersedes the old one, with
//!   reasoning. Hiding one that was never right is a `retracted` event.
//! - Every id is minted by the writer (UUIDv7); every event carries
//!   `(host_id, seq)`, so a log can merge with another host's later.
//! - All SQL against `context.*` lives in this module.
//! - Actuality is decided in [`compile`], not in storage: staleness, provisional
//!   status, contradictions and ranking are flags computed there.

pub mod compile;
pub mod model;
pub mod render;
pub mod resolve;
pub mod service;
pub mod store;
pub mod trust;

/// Hand-built graphs shared by the pure-logic tests (compile, render).
#[cfg(test)]
#[path = "tests/fixtures.rs"]
pub(crate) mod fixtures;

pub use model::*;
pub use service::{ContextService, Project};
pub use store::{context_project_id, ContextStore};

/// Schema name `context.db` is attached under on the main connection.
pub const SCHEMA: &str = "context";

/// `project_settings` key holding a project's context id. Minted once per
/// project by [`context_project_id`]; never the host-local `projects.id`.
pub const PROJECT_ID_KEY: &str = "context.id";

/// `settings` key holding this host's writer id for the event log. Minted once
/// per data dir by [`ContextStore::new`].
pub const HOST_ID_KEY: &str = "context.host_id";

/// The developer gate (`settings`, host-owned): the whole layer — ops,
/// instruction block, ingesters, extractor and the project page's Context tab
/// — is off until this is the literal `"true"`. Flipped from Settings ›
/// Developer only, while the feature is piloted.
pub const DEV_SETTING: &str = "context_layer_enabled";

/// Per-project opt-out (`project_settings`): `"false"` disables the ops, the
/// instruction block and the pipeline for that project.
pub const ENABLED_KEY: &str = "context.enabled";

/// Per-project opt-out of the background extractor only (`project_settings`).
pub const EXTRACT_KEY: &str = "context.extract";

/// Default character budget for a rendered bundle.
pub const DEFAULT_BUDGET_CHARS: usize = 12_000;

/// Fired with `{ project_id }` (the Fletch project id) by the service after
/// every write that changes what a project's context says — whichever
/// writer made it: the UI, an agent op, the ingester or the extractor. It
/// carries no row: a client re-reads `context_overview`, which is small by
/// design. Forwarded to remote clients.
pub const CHANGED_EVENT: &str = "context:changed";

/// Both project toggles are opt-out: absent or anything but the literal
/// `"false"` means on (the `code_indexing_enabled` convention).
fn project_flag(conn: &rusqlite::Connection, fletch_project_id: &str, key: &str) -> bool {
    conn.query_row(
        "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
        [fletch_project_id, key],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .as_deref()
        != Some("false")
}

/// Opt-in, unlike the project flags: only the literal `"true"` turns the
/// layer on at all.
pub fn parse_dev_enabled(raw: Option<&str>) -> bool {
    raw == Some("true")
}

/// Whether the developer gate is open on this host.
pub fn dev_enabled(conn: &rusqlite::Connection) -> bool {
    parse_dev_enabled(crate::database::get_setting(conn, DEV_SETTING).as_deref())
}

/// Whether the context layer is on for a project (ops, instruction block,
/// ingesters and extractor alike): the developer gate, then the project's
/// own flag.
pub fn enabled(conn: &rusqlite::Connection, fletch_project_id: &str) -> bool {
    dev_enabled(conn) && project_flag(conn, fletch_project_id, ENABLED_KEY)
}

/// Whether the background extractor runs for a project. Implies [`enabled`].
pub fn extract_enabled(conn: &rusqlite::Connection, fletch_project_id: &str) -> bool {
    enabled(conn, fletch_project_id) && project_flag(conn, fletch_project_id, EXTRACT_KEY)
}

/// The Fletch project a *context* id was minted for, if any.
fn fletch_project_of(conn: &rusqlite::Connection, context_project_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT project_id FROM project_settings WHERE key = ?1 AND value = ?2",
        [PROJECT_ID_KEY, context_project_id],
        |r| r.get(0),
    )
    .ok()
}

/// The gate as the store reads it inside every write transaction, keyed by
/// the context id the write names: the developer gate, then the flag of the
/// Fletch project that owns the id. A `Project` resolved through the service
/// is a name, not a lasting permission — an agent spawned while the layer
/// was on, or a pipeline that opened the project before an `await`, is
/// refused the moment either switch is off. An id no project owns (a test's)
/// has no project flag, so only the developer gate applies.
pub(crate) fn require_enabled_for(
    conn: &rusqlite::Connection,
    context_project_id: &str,
) -> Result<()> {
    if !dev_enabled(conn) {
        return Err(ContextError::Disabled);
    }
    match fletch_project_of(conn, context_project_id) {
        Some(fletch) if !project_flag(conn, &fletch, ENABLED_KEY) => Err(ContextError::Disabled),
        _ => Ok(()),
    }
}

/// Two rules a reading of the code cannot be trusted to keep: a `user_turn`
/// source is minted only by `trust`, and "what stands now" is only ever
/// `compile::is_current` — the structural `Assertion::is_head` is not a stand-in.
#[cfg(test)]
mod invariants {
    use std::path::Path;

    fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && !path.components().any(|c| c.as_os_str() == "tests")
            {
                out.push((
                    path.to_string_lossy().into_owned(),
                    std::fs::read_to_string(&path).unwrap(),
                ));
            }
        }
    }

    fn crate_sources() -> Vec<(String, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        for dir in ["context", "capture", "rpc/context", "commands"] {
            sources(&root.join(dir), &mut out);
        }
        out
    }

    #[test]
    fn a_user_turn_source_comes_only_from_trust() {
        for (path, text) in crate_sources() {
            if path.ends_with("/trust.rs")
                || path.ends_with("/model.rs")
                || path.ends_with("/context/mod.rs")
            {
                continue;
            }
            let code: String = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            let minted = code.contains("kind: SourceKind::UserTurn")
                || code.contains("Source::new(SourceKind::UserTurn");
            assert!(
                !minted,
                "{path} mints a user_turn source; only trust::UserStated may (matching the kind is fine)"
            );
        }
    }

    #[test]
    fn current_view_decisions_never_use_is_head() {
        for (path, text) in crate_sources() {
            if path.ends_with("/compile.rs")
                || path.ends_with("/model.rs")
                || path.ends_with("/context/mod.rs")
            {
                continue;
            }
            assert!(
                !text.contains(".is_head()"),
                "{path} uses Assertion::is_head; use compile::is_current / current_heads"
            );
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("the project context layer is off for this project")]
    Disabled,
    #[error("assertion {0} does not stand now: it was superseded, retracted or abandoned")]
    NotHead(Id),
    #[error("superseding an assertion needs non-empty reasoning")]
    MissingReasoning,
    #[error("an assertion must be about at least one entity")]
    NoSubject,
    #[error("unknown entity `{0}`")]
    UnknownEntity(String),
    #[error("ambiguous entity `{0}`: matches {1:?}")]
    AmbiguousEntity(String, Vec<String>),
    #[error("slug `{0}` is already taken")]
    SlugTaken(String),
    #[error("a project has exactly one vision entity; archive or revise the existing one")]
    VisionExists,
    #[error("unknown assertion `{0}`")]
    UnknownAssertion(Id),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ContextError>;

impl From<ContextError> for crate::error::Error {
    fn from(e: ContextError) -> Self {
        match e {
            ContextError::Db(e) => crate::error::Error::Database(e),
            other => crate::error::Error::Other(other.to_string()),
        }
    }
}

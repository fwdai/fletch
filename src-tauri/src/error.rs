use serde::{Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("git command failed: {0}")]
    Git(String),

    #[error("github: {0}")]
    Gh(String),

    #[error("agent not found: {0}")]
    AgentNotFound(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("database schema is newer than this app version")]
    SchemaTooNew,

    #[error("{0}")]
    Other(String),
}

/// Tauri requires command return errors to be `Serialize` so they can cross
/// the IPC boundary. We serialize as the display string.
impl Serialize for Error {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

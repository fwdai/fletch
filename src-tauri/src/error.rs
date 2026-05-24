use serde::{Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("tart command failed: {0}")]
    Tart(String),

    #[error("git command failed: {0}")]
    Git(String),

    #[error("ssh error: {0}")]
    Ssh(String),

    #[error("vm boot timeout after {0}s")]
    VmBootTimeout(u64),

    #[error("agent not found: {0}")]
    AgentNotFound(String),

    #[error("workspace not loaded")]
    WorkspaceNotLoaded,

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Other(e.to_string())
    }
}

/// Tauri requires command return errors to be `Serialize` so they can cross
/// the IPC boundary. We serialize as the display string.
impl Serialize for Error {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

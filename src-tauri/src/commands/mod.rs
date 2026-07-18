//! Tauri IPC command handlers — the thin frontend-facing surface.
//!
//! Grouped into feature-oriented submodules mirroring the frontend surfaces
//! that call them. Every handler is re-exported here so `lib.rs`'s
//! `generate_handler![commands::x, ...]` list keeps resolving unchanged.

mod agent;
mod app;
mod files;
mod git_ops;
mod git_state;
mod github;
mod run;
mod session;
mod shell;
mod tooling;
mod workspace;

pub use agent::*;
pub use app::*;
pub use files::*;
pub use git_ops::*;
pub use git_state::*;
pub use github::*;
pub use run::*;
pub use session::*;
pub use shell::*;
pub use tooling::*;
pub use workspace::*;

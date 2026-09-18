//! The engine halves of the desktop's command surface.
//!
//! Each `*_impl` here is the body a `#[tauri::command]` wrapper in
//! `src-tauri/src/commands/` calls, and the one the remote dispatcher
//! (`remote::dispatch`) calls directly — so a phone and the desktop window take
//! the same code path. The wrappers stay in the shell because only they know
//! about `tauri::State`; everything that touches the engine lives here.
//!
//! Re-exported flat, like the shell's own `commands` module, so
//! `commands::commit_agent_impl` resolves without naming the submodule.

pub mod agent;
pub mod files;
pub mod git_ops;
pub mod git_state;
pub mod github;
pub mod workspace;

pub use agent::*;
pub use files::*;
pub use git_ops::*;
pub use git_state::*;
pub use github::*;
pub use workspace::*;

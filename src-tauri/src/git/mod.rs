//! Thin wrapper around `git worktree`.
//!
//! Kept deliberately minimal — the v1 supervisor only needs to add a
//! worktree on a fresh branch and remove it later.

mod branch;
mod cmd;
mod diff;
mod files;
mod transport;
mod worktree;

pub use branch::*;
pub use diff::*;
pub use files::*;
pub use transport::*;
pub use worktree::*;
pub(crate) use cmd::*;

//! Thin wrapper around `git worktree`.
//!
//! Kept deliberately minimal — the v1 supervisor only needs to add a
//! worktree on a fresh branch and remove it later.

mod base;
mod branch;
pub mod checkpoint;
mod cmd;
mod diff;
mod files;
pub(crate) mod hardening;
mod identity;
mod publish;
mod signing;
#[cfg(test)]
pub(crate) mod testing;
mod transport;
mod worktree;

pub use base::*;
pub use branch::*;
pub(crate) use cmd::*;
pub use diff::*;
pub use files::*;
pub use identity::*;
pub use publish::*;
pub use signing::*;
pub use transport::*;
pub use worktree::*;

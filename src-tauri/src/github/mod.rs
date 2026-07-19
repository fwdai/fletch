//! GitHub operations over the REST/GraphQL API with the app's own OAuth
//! token — the successor to shelling out to the `gh` CLI, so GitHub features
//! work without any extra tool installed or terminal auth dance.
//!
//! Same public surface (functions and types) the `gh` module exposed; the
//! HTTP/auth plumbing lives in [`client`]. Read ops degrade gracefully:
//! no token, a non-GitHub origin, or a missing PR all yield `Ok(None)` /
//! empty — matching how callers treated gh's "no pull requests found".
//! Mutating ops (create/merge/clone/publish) error loudly instead, telling
//! the user to connect GitHub.
//!
//! GraphQL is used for the read ops because that's what `gh` used under the
//! hood — the payload shapes (UPPERCASE enums, `statusCheckRollup` contexts,
//! review threads) carry over verbatim, and the pure parsers below are the
//! same ones that parsed gh's output.

pub mod client;
pub mod closes;

mod checks;
mod comments;
mod issues;
mod pr;
mod query;
mod repo;
mod types;

pub use client::{git_auth_env, seed_token, set_token, TOKEN_SETTING};
pub(crate) use closes::with_closes_trailer;

pub use checks::*;
pub use comments::*;
pub use issues::*;
pub use pr::*;
pub use repo::*;
pub use types::*;

pub(crate) use pr::{pr_body, pr_update_body};
pub(crate) use query::{pr_checks_batch, pr_states_batch, resolve_slug, PrRef};

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

mod actions;
mod checks;
mod comments;
mod issues;
mod live;
mod pr;
mod query;
mod repo;
mod types;

pub use client::{git_auth_env, seed_token, set_token, TOKEN_SETTING};
pub(crate) use closes::with_closes_trailer;

pub use actions::{
    check_log, run_jobs, run_list, CHECK_LOG_DEFAULT_LINES, CHECK_LOG_MAX_LINES, RUN_LIST_MAX,
};
pub use checks::*;
pub use comments::{
    pr_conversation_comments, pr_reply_thread, pr_resolve_thread, pr_threads_number,
    CONVERSATION_COMMENTS_MAX,
};
pub use issues::*;
pub use live::*;
pub use pr::*;
pub use query::github_https_remote;
pub use repo::*;
pub use types::*;

pub(crate) use pr::{pr_body, pr_body_and_merge, pr_files, pr_update_body};
pub(crate) use query::{pr_status_batch, repo_ref, resolve_slug, PrBatchRow, PrRef};

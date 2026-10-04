//! The host's base-freshness loop: every five minutes, fetch each project's
//! base branch on its source repo (`commands::refresh_base_freshness_impl`).
//!
//! This used to be a webview timer, which fetched once per open window and not
//! at all with every window shut. The fetch touches repos the host owns, so it
//! runs here, once, however many clients are watching. Nothing is emitted: the
//! clients' `get_all_git_meta` poll reads the advanced ref on its next tick.
//!
//! Modelled on `auto_archive`: each pass runs on its own task so a panic is
//! logged and the loop goes on.

use std::sync::Arc;
use std::time::Duration;

use super::Supervisor;

/// Let boot seed the GitHub token and load the workspace before the first pass.
const FIRST_PASS_DELAY: Duration = Duration::from_secs(30);
const EVERY: Duration = Duration::from_secs(5 * 60);

pub fn spawn(supervisor: Arc<Supervisor>) {
    crate::host::spawn(async move {
        tokio::time::sleep(FIRST_PASS_DELAY).await;
        loop {
            let pass = {
                let supervisor = supervisor.clone();
                crate::host::spawn(async move {
                    crate::commands::refresh_base_freshness_impl(&supervisor).await
                })
                .await
            };
            match pass {
                Ok(done) => match done.skipped {
                    Some(why) => tracing::debug!(why, "base freshness: pass skipped"),
                    None => tracing::info!(
                        fetched = done.fetched,
                        failed = done.failed,
                        "base freshness: fetched project bases"
                    ),
                },
                Err(e) => {
                    tracing::error!(error = %e, "base freshness pass panicked — fetching continues")
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

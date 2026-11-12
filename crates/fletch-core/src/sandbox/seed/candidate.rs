use std::path::{Path, PathBuf};

use super::request::{standing_request, withdraw, Request};
use super::store::{self, Made, CANDIDATE_PREFIX};
use super::{AtRest, STORE, WORTH_INHERITING_KIB};
use crate::error::{Error, Result};
use crate::project_file::SeedSettings;

/// One candidate is reduced at a time; the others wait their turn in the
/// order they were taken.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Makes a workspace at rest its project's seed, when it is wanted
/// ([`super::want`]) or the project has none yet. What a seed is made of is
/// taken here, while the workspace is at rest; the rest happens in the
/// candidate's turn, off the caller's path.
pub fn offer(at_rest: impl AtRest) {
    if let Some(candidate) = Candidate::take(at_rest) {
        crate::host::spawn(candidate.settle());
    }
}

/// What was taken of a workspace at rest, on its way to being its project's
/// seed. What was taken goes when this does.
pub(super) struct Candidate<W> {
    pub(super) at_rest: W,
    pub(super) dir: PathBuf,
    /// The request it was taken under, when one stood.
    pub(super) request: Option<Request>,
}

impl<W: AtRest> Candidate<W> {
    fn take(at_rest: W) -> Option<Candidate<W>> {
        let settings = crate::project_file::seed_settings(at_rest.project_id())?;
        let candidate = Candidate {
            dir: at_rest
                .store()
                .join(format!("{CANDIDATE_PREFIX}{}", uuid::Uuid::new_v4())),
            request: standing_request(at_rest.request_file()),
            at_rest,
        };
        if !candidate.is_needed(&settings) {
            candidate.withdraw_request();
            return None;
        }
        match candidate.at_rest.take(&candidate.dir) {
            Ok(()) => Some(candidate),
            Err(e) => {
                tracing::warn!(error = %e, "could not take a workspace to make a seed of it");
                None
            }
        }
    }

    /// Whether the project has use for this workspace under `settings`: it
    /// answers a request newer than the one the seed answers, or the project
    /// has no seed, may have one, and the workspace was asked for or may be
    /// its first.
    pub(super) fn is_needed(&self, settings: &SeedSettings) -> bool {
        let _store = STORE.lock();
        let usable = |seed: &Path| self.at_rest.usable(seed);
        match store::seed_in_force(self.at_rest.store(), settings, usable) {
            Some(seed) => self.request > seed.for_request,
            None => {
                let may_be_the_first = self.at_rest.may_be_first();
                settings.enabled && (self.request.is_some() || may_be_the_first)
            }
        }
    }

    /// Reduces the candidate in its turn, under the settings that stand
    /// then. The request it was taken under is withdrawn once it is
    /// answered: the project has no use for the workspace, the engine could
    /// not start a workspace from what was taken, or what it kept has been
    /// looked at and what was found is settled. After any failure the
    /// request stands, and the workspace is offered again.
    pub(super) async fn settle(self) {
        let _turn = TURN.lock().await;
        let agent_id = self.at_rest.agent_id();
        let Some(settings) = crate::project_file::seed_settings(self.at_rest.project_id()) else {
            return;
        };
        if !self.is_needed(&settings) || !self.at_rest.usable(&self.dir) {
            self.withdraw_request();
            return;
        }
        match self.examine(&settings).await {
            Ok(kept_kib) => {
                tracing::info!(
                    agent_id,
                    kept_kib,
                    "looked at a workspace for its project's seed"
                );
                self.withdraw_request();
            }
            Err(e) => {
                tracing::warn!(agent_id, error = %e, "could not make a seed of a workspace");
            }
        }
    }

    /// Reduces the candidate under `settings` and publishes it when it kept
    /// something worth inheriting and the settings still stand. Answers the
    /// KiB it kept.
    async fn examine(&self, settings: &SeedSettings) -> Result<u64> {
        let kept_kib = self.at_rest.reduce(&self.dir, &settings.exclude).await?;
        if kept_kib < WORTH_INHERITING_KIB {
            self.at_rest.kept_nothing();
            return Ok(kept_kib);
        }
        let settings_now = crate::project_file::seed_settings(self.at_rest.project_id());
        if settings_now.as_ref() != Some(settings) {
            return Err(Error::Other(
                "the project's seed settings changed while its seed was made".into(),
            ));
        }
        let made = Made {
            without: settings.exclude.clone(),
            for_request: self.request,
        };
        store::publish(self.at_rest.store(), &self.dir, &made, || {
            self.at_rest.publishing()
        })?;
        Ok(kept_kib)
    }

    fn withdraw_request(&self) {
        if let Some(request) = self.request {
            withdraw(self.at_rest.request_file(), request);
        }
    }
}

impl<W> Drop for Candidate<W> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

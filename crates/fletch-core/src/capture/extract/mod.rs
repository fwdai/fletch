//! The background extractor: the main way context gets captured. After a
//! session's turn, a small headless run of the session's own provider reads
//! the new user turns and the agent's final message of each, compares them
//! against the workspace's plan and what is already recorded, and proposes
//! entities and assertions into the pipeline. The working agent is never
//! asked to remember anything.
//!
//! Shape: the host hooks ([`on_turn_completed`], [`on_archive`]) decide
//! whether a run is due (`schedule`), gather the input (`input`) and hand it
//! with an [`Extractor`] to `pipeline`, which does the rest over the store.
//! The real extractor runs the provider CLI once, tool-less, through the same
//! one-shot the handoff summarizer uses; tests pass a fake returning canned
//! JSON. Nothing here blocks the supervisor: each hook spawns onto the engine
//! runtime, at most one run is in flight per workspace (a turn-end that finds
//! one drops out; an archive waits for it), and no error reaches the caller —
//! every failure is logged, and a failed run stays on `extractor_runs` for
//! the trail.

pub mod input;
pub mod pipeline;
pub mod prompt;
pub mod schedule;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::context::model::*;
use crate::context::ContextError;
use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::workspace::WorkspaceManager;

pub use input::{ExtractInput, TurnText};
pub use pipeline::Summary;

/// How long one run may take; the handoff summary's bound, since it runs the
/// same way.
pub const TIMEOUT: Duration = Duration::from_secs(180);

/// Fewer user characters than this is not worth a model run; the turns wait
/// for more to accumulate.
pub const MIN_USER_CHARS: usize = 200;

/// The source kind of a run's observation: the conversation as a whole, the
/// agent's replies included (`user_turn` is reserved for what the user
/// said). The pipeline writes it and the watermark reads it back; one
/// constant, so the two cannot drift apart again.
pub const OBSERVATION_SOURCE: SourceKind = SourceKind::AgentTurn;

/// What the model answered: its raw text and, when the CLI reports it, usage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractOutput {
    pub text: String,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
}

/// One model run over an input: the single seam between the pipeline and a
/// provider CLI, so tests can stand in a fake.
pub trait Extractor: Send + Sync {
    fn extract(&self, input: &ExtractInput) -> Result<ExtractOutput>;
    /// What the run is recorded as having used.
    fn model(&self) -> String;
}

/// The session's provider CLI run once, tool-less and in an empty scratch
/// directory, with the prompt on stdin (`crate::agent::OneShot`). The text
/// output format reports no usage, so tokens stay unknown.
pub struct OneShotExtractor {
    provider: String,
    model: Option<String>,
    /// The runtime the subprocess is awaited on; `extract` is called from a
    /// blocking thread, where `block_on` is allowed.
    handle: tokio::runtime::Handle,
}

impl OneShotExtractor {
    pub fn new(provider: String, model: Option<String>, handle: tokio::runtime::Handle) -> Self {
        Self {
            provider,
            model,
            handle,
        }
    }
}

impl Extractor for OneShotExtractor {
    fn extract(&self, input: &ExtractInput) -> Result<ExtractOutput> {
        let provider = self.provider.as_str();
        let shot = crate::agent::one_shot(provider)
            .ok_or_else(|| Error::Other(format!("{provider} can't run as a tool-less one-shot")))?;
        let (bin, label) = crate::agent::provider_bin_label(provider)
            .ok_or_else(|| Error::Other(format!("unknown provider {provider}")))?;
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let program = crate::agent::resolve_agent_bin(provider, bin, label, &home)?;
        let text = self.handle.block_on(crate::handoff::run::once(
            &program,
            &shot,
            self.model.as_deref(),
            &prompt::render(input),
            TIMEOUT,
        ))?;
        Ok(ExtractOutput {
            text,
            tokens_in: None,
            tokens_out: None,
        })
    }

    fn model(&self) -> String {
        format!(
            "{}/{}",
            self.provider,
            self.model.as_deref().unwrap_or("default")
        )
    }
}

/// Workspaces with a run in flight. A turn-end trigger for one of them is
/// dropped (its turns wait for the next); an archive trigger waits for it.
static INFLIGHT: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// How long an archive waits for a turn-end run to finish before giving up on
/// extracting the workspace's last turns. Well past [`TIMEOUT`], so only a
/// run stuck past its own bound is ever abandoned.
pub const CLAIM_WAIT: Duration = Duration::from_secs(5 * 60);

const CLAIM_POLL: Duration = Duration::from_millis(500);

fn claim(agent_id: &str) -> bool {
    INFLIGHT
        .lock()
        .get_or_insert_with(HashSet::new)
        .insert(agent_id.to_string())
}

fn release(agent_id: &str) {
    if let Some(set) = INFLIGHT.lock().as_mut() {
        set.remove(agent_id);
    }
}

/// [`claim`], retried until it succeeds or `limit` has passed.
async fn wait_for_claim(agent_id: &str, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if claim(agent_id) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(CLAIM_POLL).await;
    }
}

/// Turn-end hook: run when the debounce allows and there is something new.
pub fn on_turn_completed(ctx: Arc<EngineCtx>, agent_id: String) {
    trigger(ctx, agent_id, false);
}

/// Archive hook: whatever is unextracted goes now, debounce or not. A
/// turn-end run still in flight is waited for (up to [`CLAIM_WAIT`]) rather
/// than dropped. The run only proposes, so it settles nothing: the branch's
/// fate is the ingester's to apply (`ingest::on_workspace_archived`).
pub fn on_archive(ctx: Arc<EngineCtx>, agent_id: String) {
    trigger(ctx, agent_id, true);
}

fn trigger(ctx: Arc<EngineCtx>, agent_id: String, is_archive: bool) {
    crate::host::spawn(async move {
        let claimed = if is_archive {
            wait_for_claim(&agent_id, CLAIM_WAIT).await
        } else {
            claim(&agent_id)
        };
        if !claimed {
            if is_archive {
                tracing::warn!(
                    agent_id,
                    "context extract: archive gave up waiting for the run in flight"
                );
            } else {
                tracing::debug!(agent_id, "context extract: a run is already in flight");
            }
            return;
        }
        if let Err(e) = run_for_workspace(ctx, agent_id.clone(), is_archive).await {
            tracing::warn!(agent_id, error = %e, "context extract: run failed");
        }
        release(&agent_id);
    });
}

/// Decide, gather and run for one workspace. The debounce is checked first,
/// off the watermark alone, so a turn-end inside it costs no transcript read;
/// the rest of the gathering is a few short reads on the engine thread, and
/// the model run goes to a blocking thread.
async fn run_for_workspace(ctx: Arc<EngineCtx>, agent_id: String, is_archive: bool) -> Result<()> {
    let workspace = WorkspaceManager::new(ctx.db.clone());
    let record = workspace.agent(&agent_id)?;
    if record.project_id.is_empty() {
        return Ok(());
    }
    let service = ctx.context()?.clone();
    let project = match service.open_for_extraction(&record.project_id) {
        Ok(Some(project)) => project,
        Ok(None) | Err(ContextError::Disabled) => return Ok(()),
        Err(e) => return Err(e.into()),
    };

    let watermark = schedule::watermark(service.store(), &project.id, &agent_id)?;
    let last_run_at = watermark.as_ref().map(|w| w.last_run_at);
    if !schedule::due(last_run_at, crate::database::now_millis(), is_archive) {
        return Ok(());
    }
    let turns = workspace.read_history_turns(&agent_id)?;
    let records = workspace.read_session_records(&agent_id)?;
    let new_turns = input::turns_since(
        &turns,
        &records,
        watermark.as_ref().and_then(|w| w.turn_id.as_deref()),
    );
    if new_turns.is_empty() {
        return Ok(());
    }

    let graph = service.store().load(&project.id)?;
    let plan = (!record.task.trim().is_empty()).then(|| record.task.clone());
    let input = ExtractInput::new(plan, new_turns, &graph);
    if input.user_chars() < MIN_USER_CHARS {
        tracing::debug!(
            agent_id,
            chars = input.user_chars(),
            "context extract: too little user text; waiting"
        );
        return Ok(());
    }

    let primary = record.repos.first();
    let commit_sha = match primary.and_then(|r| r.checkout_path(&agent_id).ok()) {
        Some(checkout) => crate::git::rev_parse(&checkout, "HEAD").await.ok(),
        None => None,
    };
    let provenance = Provenance {
        workspace_id: Some(agent_id.clone()),
        branch: primary.and_then(|r| r.branch.clone()),
        commit_sha,
        session_id: record.session_id.clone(),
        turn_id: input.last_turn_id().map(str::to_string),
        repo: primary.map(|r| r.subdir.clone()),
    };
    let author = Author::extractor(&agent_id, &record.provider);
    let extractor = OneShotExtractor::new(
        record.provider.clone(),
        record.model.clone(),
        tokio::runtime::Handle::current(),
    );
    tracing::info!(
        agent_id,
        turns = input.turns.len(),
        is_archive,
        "context extract: running"
    );
    let summary = tokio::task::spawn_blocking(move || {
        pipeline::process(&service, &project, author, provenance, input, &extractor)
    })
    .await
    .map_err(|e| Error::Other(format!("extraction task failed: {e}")))??;
    match &summary.error {
        Some(error) => {
            tracing::warn!(agent_id, observation = %summary.observation_id, error, "context extract: run kept with error")
        }
        None => tracing::info!(agent_id, ?summary, "context extract: done"),
    }
    Ok(())
}

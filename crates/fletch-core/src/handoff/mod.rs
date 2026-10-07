//! The handoff: what a new session's agent is told about the conversation it
//! continues (docs/fork-and-rewind.md, concept 4).
//!
//! The caller renders that conversation as a plain-text transcript (the
//! frontend does, since it has every provider's chat adapter). [`context`]
//! condenses it into a terse brief by running the provider's CLI once,
//! headless and tool-less, and falls back to the transcript's bounded tail
//! when that can't be done. The result is stored on the new session
//! (`sessions.handoff_context`) and composed into its instructions by
//! `agent_profile::effective_instructions`.

pub(crate) mod run;
mod tail;

use std::path::PathBuf;

use crate::error::{Error, Result};

pub use run::TIMEOUT;

/// What the summarizer is asked for. The transcript follows it on stdin.
const PROMPT: &str = "\
You are handing an ongoing piece of work over to another coding agent. Below is the \
transcript of the conversation so far between the user and the previous agent. The next \
agent will read only what you write, so it must stand on its own.

Write a terse handoff brief with these sections:
- Goal: what the user wants to achieve.
- Decisions: what was decided, and why.
- Current state: the files and components touched, and what is done.
- Open questions: anything still unresolved.
- Next steps: what remains to be done.

Keep exact identifiers as they appear: file paths, symbols, commands, branch names, error \
messages. Don't speculate or add anything the transcript doesn't support. Reply with the \
brief only, as plain text.";

/// The handoff context for `transcript`: a summary written by `provider`'s
/// CLI with `model` (`None` = its default), or, when the provider has no
/// tool-less mode or the run fails or times out, the transcript's most recent
/// part behind a notice. Never fails: a fork doesn't hang on its summary.
pub async fn context(provider: &str, model: Option<&str>, transcript: &str) -> String {
    match summarize(provider, model, transcript).await {
        Ok(summary) => summary,
        Err(e) => {
            tracing::warn!(provider, error = %e, "handoff summary failed; carrying the transcript's tail");
            tail::fallback(transcript)
        }
    }
}

/// Summarize `transcript` by running `provider`'s CLI once (see
/// [`crate::agent::OneShot`]), within [`TIMEOUT`].
pub async fn summarize(provider: &str, model: Option<&str>, transcript: &str) -> Result<String> {
    let shot = crate::agent::one_shot(provider)
        .ok_or_else(|| Error::Other(format!("{provider} can't run as a tool-less one-shot")))?;
    let (bin, label) = crate::agent::provider_bin_label(provider)
        .ok_or_else(|| Error::Other(format!("unknown provider {provider}")))?;
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let program = crate::agent::resolve_agent_bin(provider, bin, label, &home)?;
    let input = format!("{PROMPT}\n\n<transcript>\n{transcript}\n</transcript>\n");
    run::once(&program, &shot, model, &input, TIMEOUT).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_provider_that_cant_run_tool_less_carries_the_transcript() {
        let transcript = "User: fix the build\n\nAssistant: done";
        let carried = context("opencode", None, transcript).await;
        assert!(carried.starts_with(tail::FALLBACK_NOTICE), "{carried}");
        assert!(carried.ends_with(transcript));
    }

    #[tokio::test]
    async fn summarize_refuses_a_provider_without_a_one_shot() {
        for provider in ["antigravity", "nonesuch"] {
            assert!(summarize(provider, None, "x").await.is_err(), "{provider}");
        }
    }
}

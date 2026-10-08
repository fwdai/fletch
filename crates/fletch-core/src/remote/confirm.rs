//! The Mac's half of a confirmed pairing (docs/remote-protocol.md, "Confirmed
//! pairing"): the one question it can have open at a time — "Alex's iPhone
//! wants to connect · 482 913 · Accept or Decline?" — and the hook that puts
//! that question in front of a person.
//!
//! Only a host with someone at its screen installs the hook. The desktop does;
//! the headless host does not, and refuses confirmed pairings so the phone
//! falls back to the code.

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::oneshot;

use crate::host::sink::{emit, Sink};

/// Raised with a [`PairPrompt`] when a device asks, and the prompt to show.
pub const PAIR_REQUEST_EVENT: &str = "remote:pair-request";
/// Raised with `{ id }` when that prompt is over — answered, withdrawn, timed
/// out — so every window showing it takes it down.
pub const PAIR_REQUEST_ENDED_EVENT: &str = "remote:pair-request-ended";
/// Raised with `{ reason }` when the host closes the pairing window itself —
/// today only `"too_many_requests"` — so the pairing card stops showing a code
/// that no longer works. Anyone on the LAN can cause it, which is why the card
/// has to be told rather than left counting down.
pub const PAIRING_CLOSED_EVENT: &str = "remote:pairing-closed";

/// One pending question, as the desktop shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PairPrompt {
    pub id: String,
    pub device_name: String,
    pub platform: String,
    /// The six digits the device is showing too.
    pub code: String,
}

/// Why a prompt could not be opened, in words the phone shows. `NO_CONFIRMER`
/// is also a contract: the phone matches it exactly to switch to the code
/// (`NO_CONFIRMER_ERROR` in src/remote/types.ts; docs/remote-protocol.md,
/// "Confirmed pairing"), so it changes in all three places or none.
pub const NO_CONFIRMER: &str =
    "This host can't confirm a pairing on its screen. Enter the code it shows instead.";
pub const BUSY: &str = "Your Mac is answering another pairing request. Try again in a moment.";

struct Open {
    prompt: PairPrompt,
    answer: oneshot::Sender<bool>,
}

#[derive(Default)]
pub struct PairPrompts {
    confirmer: Mutex<Option<Sink>>,
    open: Mutex<Option<Open>>,
}

impl PairPrompts {
    /// Where prompts are shown. Set once, by a host that has a screen.
    pub fn set_confirmer(&self, sink: Sink) {
        *self.confirmer.lock() = Some(sink);
    }

    pub fn can_confirm(&self) -> bool {
        self.confirmer.lock().is_some()
    }

    /// Ask. The receiver yields the answer; it fails if the prompt is withdrawn
    /// unanswered.
    pub fn open(
        &self,
        device_name: &str,
        platform: &str,
        code: &str,
    ) -> Result<(String, oneshot::Receiver<bool>), &'static str> {
        let sink = self.confirmer.lock().clone().ok_or(NO_CONFIRMER)?;
        let mut open = self.open.lock();
        if open.is_some() {
            return Err(BUSY);
        }
        let prompt = PairPrompt {
            id: uuid::Uuid::new_v4().to_string(),
            device_name: device_name.to_string(),
            platform: platform.to_string(),
            code: code.to_string(),
        };
        let (answer, rx) = oneshot::channel();
        let id = prompt.id.clone();
        emit(sink.as_ref(), PAIR_REQUEST_EVENT, &prompt);
        *open = Some(Open { prompt, answer });
        Ok((id, rx))
    }

    /// Tell the screen the pairing window was closed by the host, not by the
    /// person at it.
    pub fn window_closed(&self, reason: &str) {
        if let Some(sink) = self.confirmer.lock().clone() {
            emit(
                sink.as_ref(),
                PAIRING_CLOSED_EVENT,
                &serde_json::json!({ "reason": reason }),
            );
        }
    }

    /// The question still waiting, for a window that missed the event.
    pub fn current(&self) -> Option<PairPrompt> {
        self.open.lock().as_ref().map(|o| o.prompt.clone())
    }

    /// The person's answer. `false` when `id` is not the open prompt — already
    /// answered elsewhere, or withdrawn.
    pub fn answer(&self, id: &str, accept: bool) -> bool {
        let Some(open) = self.take(id) else {
            return false;
        };
        let _ = open.answer.send(accept);
        true
    }

    /// Take the prompt down unanswered: the device hung up, or nobody answered
    /// in time. A no-op when `id` is not the open prompt.
    pub fn withdraw(&self, id: &str) {
        drop(self.take(id));
    }

    fn take(&self, id: &str) -> Option<Open> {
        let open = {
            let mut slot = self.open.lock();
            if slot.as_ref().map(|o| o.prompt.id.as_str()) != Some(id) {
                return None;
            }
            slot.take()
        };
        if let Some(sink) = self.confirmer.lock().clone() {
            emit(
                sink.as_ref(),
                PAIR_REQUEST_ENDED_EVENT,
                &serde_json::json!({ "id": id }),
            );
        }
        open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::sink::RecordingSink;
    use std::sync::Arc;

    fn prompts() -> (PairPrompts, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink::new());
        let prompts = PairPrompts::default();
        prompts.set_confirmer(sink.clone());
        (prompts, sink)
    }

    /// Pinned: the phone compares against this exact string.
    #[test]
    fn the_no_confirmer_refusal_is_the_documented_one() {
        assert_eq!(
            NO_CONFIRMER,
            "This host can't confirm a pairing on its screen. Enter the code it shows instead."
        );
    }

    #[test]
    fn a_host_without_a_screen_cannot_ask() {
        let prompts = PairPrompts::default();
        assert!(!prompts.can_confirm());
        assert_eq!(
            prompts.open("phone", "ios", "123456").unwrap_err(),
            NO_CONFIRMER
        );
    }

    #[test]
    fn one_question_at_a_time_and_the_answer_reaches_the_asker() {
        let (prompts, sink) = prompts();
        let (id, rx) = prompts.open("Alex's iPhone", "ios", "482913").unwrap();
        assert_eq!(prompts.open("Other", "ios", "000000").unwrap_err(), BUSY);
        assert_eq!(prompts.current().unwrap().code, "482913");

        assert!(!prompts.answer("not-it", true));
        assert!(prompts.answer(&id, true));
        assert_eq!(rx.blocking_recv(), Ok(true));
        assert!(prompts.current().is_none());
        // Answered once: a second click elsewhere finds nothing.
        assert!(!prompts.answer(&id, false));

        let names: Vec<String> = sink.events().into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, [PAIR_REQUEST_EVENT, PAIR_REQUEST_ENDED_EVENT]);
    }

    #[test]
    fn a_withdrawn_prompt_fails_the_wait_and_frees_the_slot() {
        let (prompts, _sink) = prompts();
        let (id, rx) = prompts.open("phone", "ios", "111111").unwrap();
        prompts.withdraw(&id);
        assert!(rx.blocking_recv().is_err());
        assert!(prompts.open("phone", "ios", "222222").is_ok());
    }
}

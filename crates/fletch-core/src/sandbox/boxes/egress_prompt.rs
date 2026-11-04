//! The questions a box's held connections raise, for the user to answer.
//!
//! Under the `ask` posture (`settings::EgressMode::Ask`) the proxy holds a
//! connection to a host off the allow-list and says so once per hold
//! (`fletch_box::egress::EgressEvent::Held`, with the hold's id). That
//! becomes one prompt here, one per workspace and host at a time, announced
//! as [`EVENT_REQUESTED`] and listed for clients that connect later
//! (`box_egress_pending`). An answer (`commands::box_egress_answer_impl`)
//! allows the host somewhere above the box, declines it on the box, or opens
//! the box's network for the turn; the proxy then admits or refuses what it
//! held and reports the hold's end (`HoldEnded`, the same id), which closes
//! the prompt. The id is what ties the two: a retry of the host starts a new
//! hold, and the end of the old one, however late it is delivered, closes
//! only the prompt of the hold it belongs to.
//!
//! Only the UI's op answers. Nothing an agent can reach — its mailbox, the
//! guest — leads here, so a box cannot widen its own allow-list.
//!
//! Shaped like `rpc::approval`: the two events, the pending list, the
//! fail-closed default.

use std::collections::HashMap;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::json;

/// Payload: a [`PendingEgressPrompt`].
pub const EVENT_REQUESTED: &str = "box:egress-requested";
/// Payload `{ id, outcome }`, one per [`EVENT_REQUESTED`] that stops waiting,
/// whoever ended it, so a card on a client that did not answer comes down.
pub const EVENT_RESOLVED: &str = "box:egress-resolved";

/// One host a workspace's box is waiting to reach. The field names match the
/// event payload, so a card renders the list and the event with one code.
#[derive(Debug, Clone, Serialize)]
pub struct PendingEgressPrompt {
    pub id: String,
    pub agent_id: String,
    pub project_id: String,
    pub host: String,
    pub port: u16,
    pub requested_at: String,
    /// The proxy's hold this prompt stands for: the engine's own, not a
    /// client's.
    #[serde(skip)]
    pub hold: u64,
}

/// How a prompt stopped waiting, as [`EVENT_RESOLVED`] spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The host was allowed, or the box's network opened for the turn.
    Allowed,
    /// The user blocked it.
    Blocked,
    /// Nobody answered before the hold ended, or the box went away.
    Expired,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Allowed => "allowed",
            Outcome::Blocked => "blocked",
            Outcome::Expired => "expired",
        }
    }
}

static PENDING: Mutex<Option<HashMap<String, PendingEgressPrompt>>> = Mutex::new(None);

/// A box holds a connection to `host` under hold `hold`: raise the prompt.
/// A prompt already up for this workspace and host stands for an earlier
/// hold (the proxy announces each hold once), so it is closed as expired
/// first: its hold is over, whether or not its end has been delivered yet,
/// and one card per host is what the user sees. The same hold announced
/// twice raises nothing.
pub fn request(agent_id: &str, project_id: &str, host: &str, port: u16, hold: u64) {
    let (stale, prompt) = {
        let mut guard = PENDING.lock();
        let map = guard.get_or_insert_with(HashMap::new);
        let standing = map
            .values()
            .find(|p| p.agent_id == agent_id && p.host == host)
            .map(|p| (p.id.clone(), p.hold));
        let stale = match standing {
            Some((_, standing_hold)) if standing_hold == hold => return,
            Some((id, _)) => map.remove(&id).map(|p| p.id),
            None => None,
        };
        let prompt = PendingEgressPrompt {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            project_id: project_id.to_string(),
            host: host.to_string(),
            port,
            requested_at: chrono::Utc::now().to_rfc3339(),
            hold,
        };
        map.insert(prompt.id.clone(), prompt.clone());
        (stale, prompt)
    };
    if let Some(id) = stale {
        super::emit(
            EVENT_RESOLVED,
            &json!({ "id": id, "outcome": Outcome::Expired.as_str() }),
        );
    }
    super::emit(EVENT_REQUESTED, &prompt);
}

/// Every prompt still waiting, oldest first.
pub fn pending() -> Vec<PendingEgressPrompt> {
    let guard = PENDING.lock();
    let Some(map) = guard.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<PendingEgressPrompt> = map.values().cloned().collect();
    out.sort_by(|a, b| a.requested_at.cmp(&b.requested_at));
    out
}

/// The prompt `id`, still waiting.
pub fn get(id: &str) -> Option<PendingEgressPrompt> {
    PENDING.lock().as_ref()?.get(id).cloned()
}

/// Close prompt `id` with `outcome`, telling every client. Unknown ids are
/// ignored: a late answer closes nothing twice.
pub fn resolve(id: &str, outcome: Outcome) {
    let taken = PENDING.lock().as_mut().and_then(|m| m.remove(id));
    if taken.is_some() {
        super::emit(
            EVENT_RESOLVED,
            &json!({ "id": id, "outcome": outcome.as_str() }),
        );
    }
}

/// The proxy's hold `hold` for `agent_id`'s box is over, whichever way: the
/// host came to be allowed (`admitted`, by an answer here or by a list or
/// posture changed elsewhere), or it was refused (a decline, the deadline,
/// the box going away). The prompt standing for that hold, if it is still
/// up, closes with the matching outcome; one an answer here already closed
/// is not closed twice, and a newer hold's prompt for the same host is not
/// this hold's to close.
pub fn ended(agent_id: &str, hold: u64, admitted: bool) {
    let id = PENDING.lock().as_ref().and_then(|m| {
        m.values()
            .find(|p| p.agent_id == agent_id && p.hold == hold)
            .map(|p| p.id.clone())
    });
    if let Some(id) = id {
        resolve(
            &id,
            if admitted {
                Outcome::Allowed
            } else {
                Outcome::Expired
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One prompt per workspace and host, however many times a hold is
    /// announced; closing it once is final.
    #[test]
    fn a_host_prompts_once_per_workspace_and_resolves_once() {
        request("fuji-prompt", "p", "api.stripe.com", 443, 1);
        request("fuji-prompt", "p", "api.stripe.com", 443, 1);
        request("etna-prompt", "p", "api.stripe.com", 443, 1);
        let mine: Vec<_> = pending()
            .into_iter()
            .filter(|p| p.agent_id == "fuji-prompt" || p.agent_id == "etna-prompt")
            .collect();
        assert_eq!(mine.len(), 2);
        let fuji = mine.iter().find(|p| p.agent_id == "fuji-prompt").unwrap();
        assert!(get(&fuji.id).is_some());

        ended("fuji-prompt", 1, false);
        assert!(get(&fuji.id).is_none(), "a refused hold closes the prompt");
        ended("fuji-prompt", 1, false);
        let etna = mine.iter().find(|p| p.agent_id == "etna-prompt").unwrap();
        resolve(&etna.id, Outcome::Blocked);
        assert!(get(&etna.id).is_none());
        resolve("never-registered", Outcome::Allowed);

        // A hold admitted elsewhere (the posture opened, a list gained the
        // host) closes its prompt too, so no stale card can be answered.
        request("vesuvio-prompt", "p", "docs.python.org", 443, 7);
        let vesuvio = pending()
            .into_iter()
            .find(|p| p.agent_id == "vesuvio-prompt")
            .unwrap();
        ended("vesuvio-prompt", 7, true);
        assert!(
            get(&vesuvio.id).is_none(),
            "an admitted hold closes the prompt"
        );
    }

    /// A retry's hold announced before the previous hold's end is delivered:
    /// the new hold gets its prompt (the old one is closed for it), and the
    /// old end, arriving late, closes nothing of the new hold's.
    #[test]
    fn a_late_end_of_an_earlier_hold_leaves_the_retry_its_prompt() {
        request("stromboli-prompt", "p", "api.stripe.com", 443, 1);
        let first = pending()
            .into_iter()
            .find(|p| p.agent_id == "stromboli-prompt")
            .unwrap();
        // The retry starts a new hold while the first's end is still in flight.
        request("stromboli-prompt", "p", "api.stripe.com", 443, 2);
        let second = pending()
            .into_iter()
            .find(|p| p.agent_id == "stromboli-prompt")
            .unwrap();
        assert_ne!(first.id, second.id, "a new hold is a new prompt");
        assert!(
            get(&first.id).is_none(),
            "the earlier hold's prompt is over"
        );
        assert_eq!(second.hold, 2);

        ended("stromboli-prompt", 1, false);
        assert!(get(&second.id).is_some(), "the late end is not the retry's");
        ended("stromboli-prompt", 2, true);
        assert!(get(&second.id).is_none());
    }

    /// The wire spellings are the documented ones (docs/remote-protocol.md).
    #[test]
    fn the_wire_names_are_the_documented_ones() {
        assert_eq!(EVENT_REQUESTED, "box:egress-requested");
        assert_eq!(EVENT_RESOLVED, "box:egress-resolved");
        assert_eq!(Outcome::Allowed.as_str(), "allowed");
        assert_eq!(Outcome::Blocked.as_str(), "blocked");
        assert_eq!(Outcome::Expired.as_str(), "expired");
    }
}

// Whether an agent should read as "working" right now, for the chat surfaces:
// the backend's status plus this client's own unacknowledged send (see
// `isAgentBusy`), minus a turn that is paused on a question for the user.
//
// Shared by the main chat, the Roadmap tab's PM chat and the run monitor's
// thread so every surface settles on the same beat as the sidebar — one
// derivation, no local timing to drift.

import type { AgentRecord } from "@/api";
import { isAgentBusy } from "@/helpers";
import { useAppStore } from "@/store";

/** `awaitingInput` (from the transcript) means the last row is an unanswered
 *  question widget: the agent is waiting on the user, not working, so the
 *  spinner must be suppressed even though the turn is technically open. */
export function useLiveBusy(
  agent: Pick<AgentRecord, "id" | "status">,
  awaitingInput: boolean,
): boolean {
  const busy = useAppStore((s) => isAgentBusy(s, agent.id, agent.status));
  return busy && !awaitingInput;
}

// The order of `get_workspace` reads, and what happened to the records while
// one was out. A read is answered off the host's records as they were when it
// was read; a live event (`agent:status`, `agent:model`, …) or the answer to a
// per-agent account switch can reach the phone first, and a read started
// earlier can answer later than one started after it. So only the newest read
// lands, and the patches made to records while it was out are replayed over
// its answer — the same rule store/chats applies to the chat lists, and
// `approvals_list` to its events. Its own module so the slices that patch
// records can note it without importing the store back.

import type { AgentRecord } from "@desktop/api/types/agent";
import { newestWins } from "@desktop/util/newestWins";

type Replay = (agents: AgentRecord[]) => AgentRecord[];

const reads = newestWins();
const inFlight = new Set<Replay[]>();

/** Start a read: its claim on the order, and the replays it will apply. The
 *  caller `finish`es it however the read ends. */
export function beginWorkspaceRead() {
  const claim = reads.claim();
  const replays: Replay[] = [];
  inFlight.add(replays);
  return {
    /** False once a later read was started: this answer is not the one to apply. */
    current: () => claim.current(),
    /** The answer's records with every change made meanwhile replayed over them. */
    apply: (agents: AgentRecord[]) => replays.reduce((list, replay) => replay(list), agents),
    finish: () => {
      inFlight.delete(replays);
    },
  };
}

/** Record a change to one agent's record for every read in flight: `change`
 *  is applied to the record *as the answer holds it*, not to a copy of the
 *  phone's — a change that edits part of a list (one repo's branch) must not
 *  carry the rest of the phone's possibly older list over the answer's. */
export function noteAgentChange(
  agentId: string,
  change: (record: AgentRecord) => AgentRecord,
): void {
  for (const replays of inFlight) {
    replays.push((agents) =>
      agents.some((a) => a.id === agentId)
        ? agents.map((a) => (a.id === agentId ? change(a) : a))
        : agents,
    );
  }
}

/** Record a patch of whole fields to one agent's record for every read in
 *  flight. For a field that is a list the answer may know more about, use
 *  [`noteAgentChange`] and edit the answer's list instead. */
export function noteAgentPatch(agentId: string, fields: Partial<AgentRecord>): void {
  noteAgentChange(agentId, (a) => ({ ...a, ...fields }));
}

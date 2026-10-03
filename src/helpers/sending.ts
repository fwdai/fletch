// The `sending` flag: a send this client made that the backend has not answered
// with a status yet. The agent's status is the busy signal; this only bridges
// the gap between the click and the `running` it produces — or the spawn it
// triggers first — so the UI reads "working" from the click on.
//
// Two rules decide when the bridge comes down, and they live here, once, for
// the desktop and the phone alike (the phone imports this module): the status
// rule, for the live event stream, and the snapshot rule, for a client that
// missed events while backgrounded or disconnected. Pure, so both apps' stores
// fold them the same way and a test can pin them down without a store.

import type { AgentStatus } from "../api";

export type SendingMap = Record<string, boolean>;

/** The flag after an `agent:status` for `agentId` moved it from `prev` to
 *  `next`. A send is answered by the status it produced: `running` (it
 *  landed), `error` / `stopped` (it won't), or an `idle` the send did not
 *  account for. Not by the spawn the send itself triggered — a dead agent
 *  revives as `spawning`, then rests at `idle` before the held message becomes
 *  its first turn — so those two keep it. Returns the same object when nothing
 *  changes, so a store can skip the write. */
export function dischargeSending(
  sending: SendingMap,
  agentId: string,
  prev: AgentStatus | undefined,
  next: AgentStatus,
): SendingMap {
  if (!sending[agentId]) return sending;
  const spawnResting = next === "idle" && prev === "spawning";
  if (next === "spawning" || spawnResting) return sending;
  const { [agentId]: _done, ...rest } = sending;
  return rest;
}

/** Sends still on the wire from this client. A flag set by one of these is
 *  younger than any snapshot the backend can answer with, so the snapshot rule
 *  leaves it alone. Module-wide: one client, one wire. */
export const inFlightSends = new Set<string>();

/** Run a send with its agent marked in-flight, so a snapshot landing in the
 *  optimistic window can't clear the flag the send just set. Covers the whole
 *  hand-off a caller passes, including any wait for the agent to come up. */
export async function whileSending<T>(agentId: string, fn: () => Promise<T>): Promise<T> {
  inFlightSends.add(agentId);
  try {
    return await fn();
  } finally {
    inFlightSends.delete(agentId);
  }
}

/** The flags after a fresh snapshot of `records`. The status rule runs on live
 *  events, and a backgrounded webview or a dropped socket misses those
 *  silently — which would strand a flag on `true` for the rest of the session,
 *  every surface reading "working" for an agent that finished. A snapshot
 *  saying the agent is at rest is the authority that the gap is over; one
 *  saying it is busy, or a send still on the wire, is not. Returns the same
 *  object when nothing changes. */
export function reconcileSending(
  sending: SendingMap,
  records: readonly { id: string; status: AgentStatus }[],
  inFlight: ReadonlySet<string> = inFlightSends,
): SendingMap {
  let next = sending;
  for (const a of records) {
    if (!sending[a.id] || a.status === "running" || a.status === "spawning") continue;
    if (inFlight.has(a.id)) continue;
    if (next === sending) next = { ...sending };
    delete next[a.id];
  }
  return next;
}

// The open turn: the one a host says an agent owes its user, by when it began.
//
// A turn, as its user knows it, runs from the moment the message is accepted
// until the agent is back at rest. The host owns that span and reports it as
// `turn_since` on every `agent:status` and on each record of a snapshot: a
// start time while a turn is open, null while none is. A client reads both
// "working" and the clock from it (`isAgentBusy`, the status by the composer),
// so the wait for a resting session's process, the checkpoint and the run are
// one status with one count, with nothing to stitch together on this side.
//
// The two rules below fold what a host says into the map of open turns: one
// for the live event stream, one for a snapshot. Pure, like `helpers/sending`,
// so a store folds them the same way and a test pins them down without one.
//
// A host from before `turn_since` says nothing (the field is absent, not
// null). For it the old inference stands: a turn opens on `turn:started` and
// is over at the first status that is not the turn running.

import type { AgentStatus } from "../api";

/** When each agent's open turn began, epoch millis, by agent id. */
export type OpenTurns = Record<string, number>;

const without = (turns: OpenTurns, agentId: string): OpenTurns => {
  if (!(agentId in turns)) return turns;
  const { [agentId]: _closed, ...rest } = turns;
  return rest;
};

const withTurn = (turns: OpenTurns, agentId: string, since: number): OpenTurns =>
  turns[agentId] === since ? turns : { ...turns, [agentId]: since };

/** The open turns after an `agent:status` for `agentId`: `since` is what the
 *  host said of its turn, undefined from a host that says nothing. Returns the
 *  same object when nothing changes, so a store can skip the write. */
export function followTurn(
  turns: OpenTurns,
  agentId: string,
  status: AgentStatus,
  since: number | null | undefined,
): OpenTurns {
  if (typeof since === "number") return withTurn(turns, agentId, since);
  if (since === null) return without(turns, agentId);
  return status === "running" || status === "spawning" ? turns : without(turns, agentId);
}

/** The open turns after a fresh snapshot of `records`: each record that says
 *  (`turn_since` present) is the authority on its agent, for a client that
 *  missed events while backgrounded or disconnected. One that does not say
 *  leaves its agent as the events left it. Returns the same object when
 *  nothing changes. */
export function reconcileTurns(
  turns: OpenTurns,
  records: readonly { id: string; turn_since?: number | null }[],
): OpenTurns {
  let next = turns;
  for (const a of records) {
    if (a.turn_since === undefined) continue;
    next = a.turn_since === null ? without(next, a.id) : withTurn(next, a.id, a.turn_since);
  }
  return next;
}

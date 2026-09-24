import type { AgentRecord } from "@/api";

/** A workspace created within this window counts as recent. */
export const RECENT_WINDOW_MS = 7 * 86_400_000;
/** At most this many recent workspaces stay in view per project. */
export const RECENT_CAP = 5;

export interface FoldOptions {
  now: number;
  /** The selected agent is never folded away — the row the transcript belongs
   *  to has to stay reachable by eye and by the arrow keys. */
  selectedId?: string | null;
  cap?: number;
  windowMs?: number;
}

export interface FoldedAgents {
  shown: AgentRecord[];
  hidden: AgentRecord[];
}

/** Whether a row must stay visible whatever its age: it is selected, or it is
 *  live. An old error is not pinned — a failure from weeks ago is history, not
 *  something to act on, and a recent one is shown by age anyway. */
function pinned(a: AgentRecord, selectedId: string | null | undefined): boolean {
  return a.id === selectedId || a.status === "running" || a.status === "spawning";
}

function isRecent(a: AgentRecord, now: number, windowMs: number): boolean {
  const t = new Date(a.created_at).getTime();
  // An undatable row is kept rather than hidden on a guess.
  return Number.isNaN(t) || now - t <= windowMs;
}

/** Split a project's workspaces (newest first) into the rows worth keeping in
 *  front of the user and the rest, which fold behind a single "older" row.
 *
 *  A row stays out when it is recent AND fewer than `cap` rows are already
 *  out — so a project with three recent workspaces shows three, not five, and
 *  one with twelve recent shows five. Selected and live rows are always out
 *  and count toward the cap. Folding a single row saves nothing, so a group
 *  that would hide exactly one shows everything.
 *
 *  Both lists keep the input order. The caller renders `hidden` after `shown`
 *  when unfolded rather than merging them back by date, so the rows already
 *  on screen never move. */
export function foldAgents(agents: readonly AgentRecord[], opts: FoldOptions): FoldedAgents {
  const { now, selectedId, cap = RECENT_CAP, windowMs = RECENT_WINDOW_MS } = opts;
  const shown: AgentRecord[] = [];
  const hidden: AgentRecord[] = [];
  for (const a of agents) {
    if (pinned(a, selectedId) || (shown.length < cap && isRecent(a, now, windowMs))) {
      shown.push(a);
    } else {
      hidden.push(a);
    }
  }
  if (hidden.length <= 1) return { shown: agents.slice(), hidden: [] };
  return { shown, hidden };
}

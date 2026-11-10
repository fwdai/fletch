import type { AgentRecord } from "@/api";

/** A workspace used within this window counts as recent. */
export const RECENT_WINDOW_MS = 7 * 86_400_000;
/** At most this many recent workspaces stay in view per project. */
export const RECENT_CAP = 5;

export interface FoldOptions {
  now: number;
  /** The selected agent is never folded away — the row the transcript belongs
   *  to has to stay reachable by eye and by the arrow keys. */
  selectedId?: string | null;
  /** When a turn last started in each workspace, by agent id (see
   *  `useTurnActivity`). Absent for a workspace nobody has messaged since it
   *  was launched. */
  activity?: Readonly<Record<string, number>>;
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

/** When the workspace was last used: its launch, or the last turn started in
 *  it, whichever is later. A workspace created a month ago and resumed this
 *  morning is this morning's. */
export function lastUsed(a: AgentRecord, activity: Readonly<Record<string, number>> = {}): number {
  const created = new Date(a.created_at).getTime();
  const turn = activity[a.id] ?? 0;
  // An undatable row with no turn on record is kept rather than hidden on a
  // guess, so it ranks as just used.
  if (Number.isNaN(created)) return turn || Number.POSITIVE_INFINITY;
  return Math.max(created, turn);
}

/** Split a project's workspaces into the rows worth keeping in front of the
 *  user and the rest, which fold behind a single "older" row.
 *
 *  The rows kept are the `cap` most recently used among those used within
 *  the window (see `lastUsed`) — so a project with three recent workspaces
 *  shows three, not five, and one with twelve shows the five last touched.
 *  Selected and live rows are always out, on top of the cap: selecting a
 *  folded row brings it into view without pushing another one out. Folding a
 *  single row saves nothing, so a group that would hide exactly one shows
 *  everything.
 *
 *  Both lists keep the input order: recency decides which rows stay, never
 *  where they sit. The caller renders `hidden` after `shown` when unfolded
 *  rather than merging them back, so the rows already on screen never move. */
export function foldAgents(agents: readonly AgentRecord[], opts: FoldOptions): FoldedAgents {
  const { now, selectedId, activity, cap = RECENT_CAP, windowMs = RECENT_WINDOW_MS } = opts;
  const kept = new Set(
    agents
      .map((a, i) => ({ a, i, t: lastUsed(a, activity) }))
      .filter((x) => now - x.t <= windowMs)
      .sort((x, y) => y.t - x.t || x.i - y.i)
      .slice(0, cap)
      .map((x) => x.a),
  );
  const shown: AgentRecord[] = [];
  const hidden: AgentRecord[] = [];
  for (const a of agents) (kept.has(a) || pinned(a, selectedId) ? shown : hidden).push(a);
  if (hidden.length <= 1) return { shown: agents.slice(), hidden: [] };
  return { shown, hidden };
}

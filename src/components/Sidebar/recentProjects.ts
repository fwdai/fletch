import type { AgentRecord, WfRun } from "@/api";
import type { ProjectActivity } from "@/storage/projectActivity";

/** A project whose last activity is older than this has gone quiet and
 *  settles back into the manual order. Three days: yesterday's project is
 *  still near the top today, last week's is not. */
export const ACTIVE_WINDOW_MS = 3 * 86_400_000;

/** The slice of a sidebar group the ranking needs. */
export interface ActiveCandidate {
  key: string;
  agents: readonly AgentRecord[];
  runs: readonly WfRun[];
}

/** When the user last did something in the project: the newest agent or
 *  workflow launch, or the last turn started in it (`turns`, see
 *  `storage/projectActivity`) — whichever is later. Zero for an empty,
 *  never-used project.
 *
 *  That moment is the only rank: the project you just spawned into, or just
 *  sent a message in, goes to the top and stays put when the agent finishes.
 *  Whether something is still running doesn't move a project (see `isLive`),
 *  otherwise every project with a running agent would tie for first and the
 *  fresh one would land under them by manual order.
 *
 *  A draft does not count. Pressing "+" opens a blank form; the project moves
 *  only once that form spawns a real agent, so the list doesn't jump under the
 *  click that opened it. */
export function lastActivity(g: ActiveCandidate, turns: ProjectActivity): number {
  let t = turns[g.key] ?? 0;
  for (const a of g.agents) {
    const c = new Date(a.created_at).getTime();
    if (!Number.isNaN(c) && c > t) t = c;
  }
  for (const r of g.runs) if (r.created_at > t) t = r.created_at;
  return t;
}

/** Whether an agent or a workflow run is running in the project right now. A
 *  live project stays floated however old its launch — the window only
 *  retires the quiet ones. */
export function isLive(g: ActiveCandidate): boolean {
  return (
    g.agents.some((a) => a.status === "running" || a.status === "spawning") ||
    g.runs.some((r) => r.status === "running")
  );
}

/** Float the projects being worked on above the manually ordered list.
 *
 *  `active` holds every group used within the window, or live, most recent
 *  activity first (a tie keeps the manual order); `rest` holds the others in
 *  their manual order. The caller renders them back to back as one list —
 *  the point is that the quiet tail never moves, so it stays scannable from
 *  memory. */
export function bubbleActive<G extends ActiveCandidate>(
  groups: readonly G[],
  now: number,
  turns: ProjectActivity = {},
  windowMs = ACTIVE_WINDOW_MS,
): { active: G[]; rest: G[] } {
  const active = groups
    .map((g, i) => ({ g, i, t: lastActivity(g, turns) }))
    .filter((x) => now - x.t <= windowMs || isLive(x.g))
    .sort((a, b) => b.t - a.t || a.i - b.i)
    .map((x) => x.g);
  const lifted = new Set(active);
  return { active, rest: groups.filter((g) => !lifted.has(g)) };
}

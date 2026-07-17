// Sidebar/stepChildren.ts — derive a run's step-agent children for the sidebar.
// Run-owned step agents are ordinary AgentRecords fetched via `wf_run_agents`;
// this collapses them into the ordered, live subset the RunRow expands under
// itself, each carrying the same status-rail vocabulary as a regular AgentRow.

import type { AgentRecord } from "@/api";

/** Left-spine rail state, matching `AgentRow`'s vocabulary (minus the PR-merged
 *  purple — step agents are capability-restricted and never own a PR). */
export type StepRail = "run" | "err" | "idle";

export interface StepChild {
  agent: AgentRecord;
  rail: StepRail;
  /** Live turn in flight — drives the name shimmer + loader, like AgentRow. */
  working: boolean;
}

function railFor(status: AgentRecord["status"]): { rail: StepRail; working: boolean } {
  if (status === "running" || status === "spawning") return { rail: "run", working: true };
  if (status === "error") return { rail: "err", working: false };
  return { rail: "idle", working: false };
}

/** The live step agents of a run, in step (spawn) order. Archived step agents
 *  are dropped — a finished run's expander shows nothing rather than a wall of
 *  tombstones — and a run with no live step agent yields an empty list so the
 *  RunRow can skip the expander stub entirely. */
export function deriveStepChildren(agents: AgentRecord[]): StepChild[] {
  return agents
    .filter((a) => !a.archive)
    .slice()
    .sort((a, b) => {
      const at = new Date(a.created_at).getTime();
      const bt = new Date(b.created_at).getTime();
      if (at !== bt) return at - bt;
      return a.id.localeCompare(b.id);
    })
    .map((agent) => ({ agent, ...railFor(agent.status) }));
}

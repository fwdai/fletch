import type { AgentRecord, WfRun } from "@/api";
import { agentDotStatus } from "@/components/TitleBar/WorkspaceStatus/derive";

/** What is going on in a project that its rows would show if they were on
 *  screen: an agent waiting on the user, finished results nobody has opened,
 *  or work in progress. The project selector carries it for the projects a
 *  focus hides, so narrowing the list never costs the user a question. */
export type ProjectSignal = "waiting" | "unseen" | "running";

/** Strongest first: what needs the user outranks what is merely busy. */
const BY_STRENGTH: readonly ProjectSignal[] = ["waiting", "unseen", "running"];

/** The row tooltips' own wording, so a dot and the row it stands for agree. */
export const SIGNAL_LABEL: Record<ProjectSignal, string> = {
  waiting: "Waiting for input",
  unseen: "New results to review",
  running: "Working",
};

export function strongestSignal(
  signals: readonly (ProjectSignal | null | undefined)[],
): ProjectSignal | null {
  return BY_STRENGTH.find((s) => signals.includes(s)) ?? null;
}

/** The store maps the agent rows read their state from. */
export interface SignalSources {
  /** `pendingToolUse`: an entry while an agent is paused on a question. */
  pending: Record<string, Record<string, string> | undefined>;
  /** `unseenResults`: set when a turn ended on an agent that wasn't open. */
  unseen: Record<string, boolean | undefined>;
}

/** The strongest signal among a project's agents and workflow runs, read the
 *  way `AgentRow` reads each one. */
export function projectSignal(
  group: { agents: readonly AgentRecord[]; runs: readonly WfRun[] },
  { pending, unseen }: SignalSources,
): ProjectSignal | null {
  const signals: (ProjectSignal | null)[] = group.agents.map((a) => {
    const dot = agentDotStatus(a.status, pending[a.id]);
    if (dot === "waiting" || dot === "running") return dot;
    return a.status === "idle" && unseen[a.id] ? "unseen" : null;
  });
  if (group.runs.some((r) => r.status === "running")) signals.push("running");
  return strongestSignal(signals);
}

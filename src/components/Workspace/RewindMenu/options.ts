import type { AgentRecord, RestoreReport, RewindScope } from "@/api";

/** One rewind a message offers, and why it can't be picked now (null when it
 *  can). */
export interface RewindOption {
  scope: RewindScope;
  label: string;
  reason: string | null;
}

/** What the menu knows of the message's code: what restoring it would do,
 *  why it can't be restored (it ran in another workspace, or no snapshot of it
 *  was kept), or nothing yet. */
export type CodeState = { report: RestoreReport } | { unavailable: string } | "checking";

/** Why no rewind can start now, if none can: the agent is mid-turn. The view
 *  is no obstacle: the rewound session launches in it like any other. */
export function rewindBlocker(agent: Pick<AgentRecord, "status">): string | null {
  if (agent.status === "running" || agent.status === "spawning") return "Stop the agent first.";
  return null;
}

/** The rewinds a message offers, in menu order, each with why it is
 *  unavailable. Restoring the code also needs the message's code (`code`);
 *  restoring the conversation needs nothing more. */
export function rewindOptions(blocker: string | null, code: CodeState): RewindOption[] {
  const codeReason =
    blocker ??
    (code === "checking" ? "Checking the code…" : "unavailable" in code ? code.unavailable : null);
  return [
    { scope: "conversation", label: "Restore conversation", reason: blocker },
    { scope: "code", label: "Restore code", reason: codeReason },
    { scope: "both", label: "Restore conversation and code", reason: codeReason },
  ];
}

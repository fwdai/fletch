// data.ts — static descriptors for the builder UI.

import type { IconName } from "../components/Icon";
import type { AdvanceMode } from "./storage";

export interface AdvanceModeDef {
  id: AdvanceMode;
  label: string;
  short: string;
  icon: IconName;
  note: string;
}

/** How a step decides it's finished and hands off. Mirrors the design
 *  prototype; only the copy/labels live here — the run engine (a later PR)
 *  interprets the semantics. */
export const ADVANCE_MODES: AdvanceModeDef[] = [
  {
    id: "signal",
    label: "Agent signals done",
    short: "signals done",
    icon: "check",
    note: "The agent decides it's finished and explicitly hands off. Simplest; each agent is told the full workflow shape and its place in it.",
  },
  {
    id: "commit",
    label: "On commit",
    short: "on commit",
    icon: "commit",
    note: "Advance as soon as the step makes a git commit.",
  },
  {
    id: "tests",
    label: "When tests pass",
    short: "tests pass",
    icon: "check",
    note: "Run the project's test command; advance only when it's green.",
  },
  {
    id: "artifact",
    label: "When file is written",
    short: "file written",
    icon: "file",
    note: "Advance once a named file (e.g. PLAN.md) is committed to the worktree.",
  },
  {
    id: "approval",
    label: "Manual approval",
    short: "you approve",
    icon: "user",
    note: "Pause and wait for you to approve the handoff.",
  },
];

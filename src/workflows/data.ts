// data.ts — static descriptors for the block-editor UI (labels, icons, copy).
// The semantics live in the Rust engine (spec §9/§6); only presentation here.

import type { IconName } from "../components/Icon";
import type { CommsCap, Gate } from "./spec";

export type GateKind = Gate["type"];

export interface GateModeDef {
  id: GateKind;
  label: string;
  short: string;
  icon: IconName;
  note: string;
}

/** How a step attempt is judged done (spec §9). `verdict` is the default. */
export const GATE_MODES: GateModeDef[] = [
  {
    id: "verdict",
    label: "Writes a verdict",
    short: "verdict",
    icon: "check",
    note: 'The agent writes verdict.json with result "done". The default, and the only gate a loop can exit on.',
  },
  {
    id: "commit",
    label: "On commit",
    short: "commit",
    icon: "commit",
    note: "Done as soon as the step moves HEAD (makes a git commit).",
  },
  {
    id: "artifact",
    label: "A file is written",
    short: "file",
    icon: "file",
    note: "Done once a named repo-relative file exists in the checkout (e.g. PLAN.md).",
  },
  {
    id: "tests",
    label: "Tests pass",
    short: "tests",
    icon: "flask",
    note: "Runs the project's test command in the step's checkout; done only when it exits 0.",
  },
  {
    id: "approval",
    label: "You approve",
    short: "approval",
    icon: "hand",
    note: "Pauses the run for you to approve the handoff — no gate the agent can satisfy itself.",
  },
];

export interface CommsCapDef {
  id: CommsCap;
  label: string;
  note: string;
}

/** The comms permissions a plain step / orchestrate child may hold. `notify` is
 *  orchestrator-only (spec §5.1) and never offered here. */
export const STEP_COMMS: CommsCapDef[] = [
  {
    id: "report",
    label: "Report",
    note: "Post progress/done notes to the orchestrator (or timeline).",
  },
  {
    id: "ask",
    label: "Ask",
    note: "Ask a question — routed to the orchestrator, else pauses for you.",
  },
];

export interface BlockTypeDef {
  id: "step" | "parallel" | "loop" | "orchestrate";
  label: string;
  icon: IconName;
  note: string;
}

/** The block kinds the sequence "add" menu offers. */
export const BLOCK_TYPES: BlockTypeDef[] = [
  { id: "step", label: "Step", icon: "arrowR", note: "One agent works, then hands off." },
  {
    id: "parallel",
    label: "Parallel",
    icon: "layers",
    note: "Several agents work at once, then join.",
  },
  {
    id: "loop",
    label: "Loop",
    icon: "loop",
    note: "Repeat a body until a step's verdict says done.",
  },
  {
    id: "orchestrate",
    label: "Orchestrate",
    icon: "combine",
    note: "A lead agent assigns work to children and answers their questions.",
  },
];

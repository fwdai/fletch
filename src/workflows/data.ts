// data.ts — static descriptors for the builder UI (labels, icons, copy).
// The semantics live in the Rust engine (spec §9); only presentation here.

import type { IconName } from "../components/Icon";
import type { Gate } from "./spec";

export type GateKind = Gate["type"];

export interface GateModeDef {
  id: GateKind;
  label: string;
  short: string;
  icon: IconName;
  note: string;
}

/** How a step is judged done (spec §9). `verdict` is the default. */
export const GATE_MODES: GateModeDef[] = [
  {
    id: "verdict",
    label: "Writes a verdict",
    short: "verdict",
    icon: "check",
    note: 'The agent writes verdict.json with result "done"; any other result stops the run. The default.',
  },
  {
    id: "commit",
    label: "On commit",
    short: "commit",
    icon: "commit",
    note: "Done when the agent's turn ends; whatever it changed is committed for the next step.",
  },
];

// ctx.ts — the shared editing surface threaded through the recursive block tree,
// so a StepCard nested three containers deep can dispatch an edit or open a
// popover without prop-drilling every callback.

import type { BlockTypeDef } from "../data";
import type { AgentResolver } from "../shared";
import type { AgentRole } from "./edits";
import type { EBlock, EStep, NodeId } from "./model";

export interface PopRect {
  top: number;
  left: number;
  right: number;
  bottom: number;
}

/** The one open popover, positioned from a measured trigger rect. */
export type Pop =
  | { type: "agent"; nid: NodeId; role: AgentRole; rect: PopRect }
  | { type: "gate"; nid: NodeId; rect: PopRect }
  | { type: "budgets"; target: NodeId | "run"; rect: PopRect };

export interface BuilderCtx {
  resolve: AgentResolver;
  /** Inline validation messages for a node, if any. */
  errorsFor: (nid: NodeId) => string[] | undefined;
  patchStep: (nid: NodeId, patch: Partial<EStep>) => void;
  patchBlock: (nid: NodeId, patch: Partial<EBlock>) => void;
  removeNode: (nid: NodeId) => void;
  addStepToContainer: (nid: NodeId) => void;
  /** Append a block to a sequence: the top level (`null`) or a loop body's nid. */
  addBlock: (seqNid: NodeId | null, type: BlockTypeDef["id"]) => void;
  openAgent: (nid: NodeId, role: AgentRole, e: React.MouseEvent) => void;
  openGate: (nid: NodeId, e: React.MouseEvent) => void;
  openBudgets: (target: NodeId | "run", e: React.MouseEvent) => void;
}

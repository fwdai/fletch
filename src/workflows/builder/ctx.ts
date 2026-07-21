// ctx.ts — the shared editing surface threaded through the recursive block tree,
// so a StepCard nested three containers deep can dispatch an edit or open a
// popover without prop-drilling every callback.
//
// v2 (vertical editor): the canvas renders collapsed cards and the right-hand
// inspector edits the selected node, so the ctx also carries the selection.

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

/** The one open popover, positioned from a measured trigger rect. Gate and
 *  budget editing live inline in the inspector now — only the agent picker
 *  remains a popover. */
export type Pop = { type: "agent"; nid: NodeId; role: AgentRole; rect: PopRect };

export interface BuilderCtx {
  resolve: AgentResolver;
  /** Inline validation messages for a node, if any. */
  errorsFor: (nid: NodeId) => string[] | undefined;
  /** The selected node (step or container) the inspector is editing. */
  selectedNid: NodeId | null;
  /** Select a node (`null` returns the inspector to the workflow overview). */
  select: (nid: NodeId | null) => void;
  patchStep: (nid: NodeId, patch: Partial<EStep>) => void;
  patchBlock: (nid: NodeId, patch: Partial<EBlock>) => void;
  removeNode: (nid: NodeId) => void;
  addStepToContainer: (nid: NodeId) => void;
  /** Insert a block into a sequence — the top level (`null`) or a loop body's
   *  nid — at `index` (appended when omitted). Selects the new block. */
  addBlock: (seqNid: NodeId | null, type: BlockTypeDef["id"], index?: number) => void;
  openAgent: (nid: NodeId, role: AgentRole, e: React.MouseEvent) => void;
}

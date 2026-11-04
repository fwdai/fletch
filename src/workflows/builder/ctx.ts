// ctx.ts — the shared editing surface handed to the canvas cards and the
// inspector, so a StepCard can dispatch an edit, select itself or open the agent
// picker without prop-drilling every callback.
//
// The canvas renders collapsed cards and the right-hand inspector edits the
// selected step, so the ctx also carries the selection.

import type { AgentResolver } from "../shared";
import type { EStep, NodeId } from "./model";

export interface PopRect {
  top: number;
  left: number;
  right: number;
  bottom: number;
}

/** The one open popover, positioned from a measured trigger rect. Gate editing
 *  lives inline in the inspector — only the agent picker is a popover. */
export type Pop = { type: "agent"; nid: NodeId; rect: PopRect };

export interface BuilderCtx {
  resolve: AgentResolver;
  /** Inline validation messages for a step, if any. */
  errorsFor: (nid: NodeId) => string[] | undefined;
  /** The selected step the inspector is editing. */
  selectedNid: NodeId | null;
  /** Select a step (`null` returns the inspector to the workflow overview). */
  select: (nid: NodeId | null) => void;
  patchStep: (nid: NodeId, patch: Partial<EStep>) => void;
  removeStep: (nid: NodeId) => void;
  /** Insert a fresh step at `index` (appended when omitted) and select it. */
  addStep: (index?: number) => void;
  openAgent: (nid: NodeId, e: React.MouseEvent) => void;
}

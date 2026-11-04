// edits.ts — pure operations over EditorState's step list. Components dispatch
// these and get a fresh state back; nothing here mutates its input, so React
// sees new refs.

import type { CustomAgent } from "../../storage/customAgents";
import { type EditorState, type EStep, ensureAlias, type NodeId } from "./model";

/** Every spec step id currently in use (so new steps avoid collisions). */
export function editorStepIds(state: EditorState): Set<string> {
  return new Set(state.blocks.map((s) => s.stepId));
}

export function setField(state: EditorState, patch: Partial<EditorState>): EditorState {
  return { ...state, ...patch };
}

export function patchStep(state: EditorState, nid: NodeId, patch: Partial<EStep>): EditorState {
  return { ...state, blocks: state.blocks.map((s) => (s.nid === nid ? { ...s, ...patch } : s)) };
}

export function removeStep(state: EditorState, nid: NodeId): EditorState {
  return { ...state, blocks: state.blocks.filter((s) => s.nid !== nid) };
}

/** Insert a prebuilt step at `index` (appended when omitted or out of range).
 *  The step is constructed by the caller (see `newStep`) so this stays a pure
 *  state→state op, safe to run inside a React functional update. */
export function insertStep(state: EditorState, step: EStep, index?: number): EditorState {
  const { blocks } = state;
  const at = index == null || index < 0 || index > blocks.length ? blocks.length : index;
  return { ...state, blocks: [...blocks.slice(0, at), step, ...blocks.slice(at)] };
}

export function findStep(state: EditorState, nid: NodeId): EStep | null {
  return state.blocks.find((s) => s.nid === nid) ?? null;
}

/** Point a step's agent at an alias for the picked agent, creating/reusing the
 *  alias in `state.agents`. */
export function setAgent(
  state: EditorState,
  nid: NodeId,
  agentId: string,
  customAgents: CustomAgent[],
): EditorState {
  const { agents, alias } = ensureAlias(state.agents, agentId, customAgents);
  return patchStep({ ...state, agents }, nid, { agent: alias });
}

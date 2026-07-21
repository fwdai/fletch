// edits.ts — pure tree operations over EditorState. Components dispatch these and
// get a fresh state back; nothing here mutates its input, so React sees new refs
// and the recursive block tree stays cheap to update by `nid`.

import type { CustomAgent } from "../../storage/customAgents";
import {
  type EBlock,
  type EditorState,
  type EStep,
  ensureAlias,
  type NodeId,
  newLoop,
  newOrchestrate,
  newParallel,
  newStep,
} from "./model";

/** Every spec step id currently in the tree (so new steps avoid collisions). */
function editorStepIds(state: EditorState): Set<string> {
  const ids = new Set<string>();
  const walk = (blocks: EBlock[]) => {
    for (const b of blocks) {
      if (b.kind === "step") ids.add(b.stepId);
      else if (b.kind === "parallel") for (const s of b.steps) ids.add(s.stepId);
      else if (b.kind === "loop") walk(b.body);
      else for (const s of b.body) ids.add(s.stepId);
    }
  };
  walk(state.blocks);
  return ids;
}

/** A loop whose exit step was deleted can't gate; clear the dangling reference so
 *  validation reports "choose an exit step" rather than a phantom id. */
function fixLoops(blocks: EBlock[]): EBlock[] {
  return blocks.map((b) => {
    if (b.kind !== "loop") return b;
    const body = fixLoops(b.body);
    const stillThere = b.untilNid && findStepNid(body, b.untilNid);
    return { ...b, body, untilNid: stillThere ? b.untilNid : null };
  });
}

function findStepNid(blocks: EBlock[], nid: NodeId): boolean {
  for (const b of blocks) {
    if (b.kind === "step") {
      if (b.nid === nid) return true;
    } else if (b.kind === "parallel") {
      if (b.steps.some((s) => s.nid === nid)) return true;
    } else if (b.kind === "loop") {
      if (findStepNid(b.body, nid)) return true;
    } else if (b.body.some((s) => s.nid === nid)) return true;
  }
  return false;
}

// ───────────────────────────── field edits ─────────────────────────────

export function setField(state: EditorState, patch: Partial<EditorState>): EditorState {
  return { ...state, ...patch };
}

/** Patch a step anywhere in the tree (top-level, parallel child, loop body,
 *  orchestrate child). */
export function patchStep(state: EditorState, nid: NodeId, patch: Partial<EStep>): EditorState {
  const apply = (blocks: EBlock[]): EBlock[] =>
    blocks.map((b) => {
      if (b.kind === "step") return b.nid === nid ? { ...b, ...patch } : b;
      if (b.kind === "parallel") {
        return { ...b, steps: b.steps.map((s) => (s.nid === nid ? { ...s, ...patch } : s)) };
      }
      if (b.kind === "loop") return { ...b, body: apply(b.body) };
      return { ...b, body: b.body.map((s) => (s.nid === nid ? { ...s, ...patch } : s)) };
    });
  return { ...state, blocks: apply(state.blocks) };
}

/** Patch a container/step block's own fields by nid (not its children). */
export function patchBlock(state: EditorState, nid: NodeId, patch: Partial<EBlock>): EditorState {
  const apply = (blocks: EBlock[]): EBlock[] =>
    blocks.map((b) => {
      if (b.nid === nid) return { ...b, ...patch } as EBlock;
      if (b.kind === "loop") return { ...b, body: apply(b.body) };
      return b;
    });
  return { ...state, blocks: fixLoops(apply(state.blocks)) };
}

/** Remove a block (top-level or loop body) or a step (parallel/orchestrate
 *  child) by nid. */
export function removeNode(state: EditorState, nid: NodeId): EditorState {
  const apply = (blocks: EBlock[]): EBlock[] => {
    const out: EBlock[] = [];
    for (const b of blocks) {
      if (b.nid === nid) continue;
      if (b.kind === "parallel") out.push({ ...b, steps: b.steps.filter((s) => s.nid !== nid) });
      else if (b.kind === "orchestrate") {
        out.push({ ...b, body: b.body.filter((s) => s.nid !== nid) });
      } else if (b.kind === "loop") out.push({ ...b, body: apply(b.body) });
      else out.push(b);
    }
    return out;
  };
  return { ...state, blocks: fixLoops(apply(state.blocks)) };
}

// ───────────────────────────── structural edits ────────────────────────────

/** Insert a block into a sequence — the top level (`seqNid = null`) or a loop
 *  body — at `index` (appended when omitted or out of range). */
export function addBlock(
  state: EditorState,
  seqNid: NodeId | null,
  block: EBlock,
  index?: number,
): EditorState {
  const insert = (blocks: EBlock[]): EBlock[] => {
    const at = index == null || index < 0 || index > blocks.length ? blocks.length : index;
    return [...blocks.slice(0, at), block, ...blocks.slice(at)];
  };
  if (seqNid === null) return { ...state, blocks: insert(state.blocks) };
  const apply = (blocks: EBlock[]): EBlock[] =>
    blocks.map((b) => {
      if (b.kind !== "loop") return b;
      if (b.nid === seqNid) return { ...b, body: insert(b.body) };
      return { ...b, body: apply(b.body) };
    });
  return { ...state, blocks: apply(state.blocks) };
}

/** Add a child step to a parallel or orchestrate container. Returns the new
 *  state and the child's nid so the caller can select it. */
export function addStepToContainer(
  state: EditorState,
  containerNid: NodeId,
): { state: EditorState; nid: NodeId } {
  const step = newStep(editorStepIds(state));
  const apply = (blocks: EBlock[]): EBlock[] =>
    blocks.map((b) => {
      if (b.nid === containerNid && b.kind === "parallel") {
        return { ...b, steps: [...b.steps, step] };
      }
      if (b.nid === containerNid && b.kind === "orchestrate") {
        return { ...b, body: [...b.body, step] };
      }
      if (b.kind === "loop") return { ...b, body: apply(b.body) };
      return b;
    });
  return { state: { ...state, blocks: apply(state.blocks) }, nid: step.nid };
}

/** Insert a fresh block of the chosen kind into a sequence (top level or loop),
 *  at `index` (appended when omitted). Returns the new state and the block's nid
 *  so the caller can select it. */
export function addBlockOfType(
  state: EditorState,
  seqNid: NodeId | null,
  type: "step" | "parallel" | "loop" | "orchestrate",
  index?: number,
): { state: EditorState; nid: NodeId } {
  const taken = editorStepIds(state);
  const block =
    type === "step"
      ? newStep(taken)
      : type === "parallel"
        ? newParallel(taken)
        : type === "loop"
          ? newLoop(taken)
          : newOrchestrate(taken);
  return { state: addBlock(state, seqNid, block, index), nid: block.nid };
}

/** Find any node — step or container — by nid anywhere in the tree. Used by the
 *  inspector to render the selected node and to drop a stale selection. */
export function findNode(state: EditorState, nid: NodeId): EBlock | null {
  let found: EBlock | null = null;
  const walk = (blocks: EBlock[]) => {
    for (const b of blocks) {
      if (found) return;
      if (b.nid === nid) {
        found = b;
        return;
      }
      if (b.kind === "parallel") {
        found = b.steps.find((s) => s.nid === nid) ?? null;
      } else if (b.kind === "loop") {
        walk(b.body);
      } else if (b.kind === "orchestrate") {
        found = b.body.find((s) => s.nid === nid) ?? null;
      }
    }
  };
  walk(state.blocks);
  return found;
}

/** Every step in a loop body, flattened, as loop-exit candidates. */
export function loopExitCandidates(blocks: EBlock[]): { nid: NodeId; label: string }[] {
  const out: { nid: NodeId; label: string }[] = [];
  const walk = (bs: EBlock[]) => {
    for (const b of bs) {
      if (b.kind === "step") out.push({ nid: b.nid, label: b.stepId });
      else if (b.kind === "parallel")
        for (const s of b.steps) out.push({ nid: s.nid, label: s.stepId });
      else if (b.kind === "loop") walk(b.body);
      else for (const s of b.body) out.push({ nid: s.nid, label: s.stepId });
    }
  };
  walk(blocks);
  return out;
}

// ───────────────────────────── agent assignment ────────────────────────────

export type AgentRole = "step" | "orchestrator" | "child";

/** Point a node's agent at an alias for the picked agent, creating/reusing the
 *  alias in `state.agents`. `role` selects which slot on an orchestrate node. */
export function setAgent(
  state: EditorState,
  nid: NodeId,
  role: AgentRole,
  agentId: string,
  customAgents: CustomAgent[],
): EditorState {
  const { agents, alias } = ensureAlias(state.agents, agentId, customAgents);
  const withAgents = { ...state, agents };
  if (role === "step") return patchStep(withAgents, nid, { agent: alias });
  const apply = (blocks: EBlock[]): EBlock[] =>
    blocks.map((b) => {
      if (b.kind === "loop") return { ...b, body: apply(b.body) };
      if (b.nid !== nid || b.kind !== "orchestrate") return b;
      if (role === "orchestrator") return { ...b, agent: alias };
      const children = b.children ?? { agent: null, max: 3 };
      return { ...b, children: { ...children, agent: alias } };
    });
  return { ...withAgents, blocks: apply(withAgents.blocks) };
}

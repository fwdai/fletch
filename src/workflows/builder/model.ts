// model.ts — the builder's editor state and its bidirectional mapping to the
// canonical `Spec` (src/workflows/spec.ts, mirror of the Rust `workflow::spec`).
//
// Why an editor state distinct from `Spec`:
//  - React needs a stable per-step id (`nid`) to key and update the list; spec
//    steps carry no such handle.
//  - Agents: the builder lets each step pick a custom agent or a base provider
//    (the v0 UX). `Spec.agents` is an alias→AgentSpec map. We keep that map
//    verbatim in the editor (so any imported spec round-trips losslessly) and
//    synthesize/reuse an alias whenever the user makes a pick.
//
// `toSpec(fromSpec(spec))` is value-equal to `spec` for any spec our builder or
// the importer can produce — this is the property the vitest pins.

import type { CustomAgent } from "../../storage/customAgents";
import { slugify } from "../shared";
import type { AgentSpec, Definition, Finalize, Gate, Spec, Step } from "../spec";
import { SPEC_VERSION } from "../spec";

// ───────────────────────────── editor state ─────────────────────────────

export type NodeId = string;

export interface EStep {
  nid: NodeId;
  /** The spec step id (unique across the whole spec). */
  stepId: string;
  /** A custom-agent id, a base-provider id, or null (unassigned). Aliases in
   *  `EditorState.agents` are derived from this at save time. */
  agent: string | null;
  goal: string;
  gate: Gate;
}

export interface EditorState {
  /** Definition id; null for an unsaved workflow. */
  id: string | null;
  name: string;
  description: string;
  hue: number;
  /** Alias → agent spec, kept verbatim from the loaded spec and extended by
   *  picks. Pruned to referenced aliases at `toSpec`. */
  agents: Record<string, AgentSpec>;
  /** The steps, in run order. */
  blocks: EStep[];
  finalize: Finalize | null;
}

// ───────────────────────────── id generation ─────────────────────────────

let nidSeq = 0;
/** A stable UI handle for a step (never serialized). */
export function newNid(): NodeId {
  nidSeq += 1;
  return `n-${nidSeq}`;
}

let stepSeq = 0;
/** A fresh spec step id, unique against `taken`. */
export function newStepId(taken: Set<string>): string {
  do {
    stepSeq += 1;
  } while (taken.has(`step-${stepSeq}`));
  const id = `step-${stepSeq}`;
  taken.add(id);
  return id;
}

export function newStep(taken: Set<string>): EStep {
  return {
    nid: newNid(),
    stepId: newStepId(taken),
    agent: null,
    goal: "",
    gate: { type: "verdict" },
  };
}

// ───────────────────────────── blank / defaults ───────────────────────────

export const WF_HUES = [265, 150, 25, 215, 320, 95, 175, 50];

export function blankEditor(seed: number): EditorState {
  const taken = new Set<string>();
  return {
    id: null,
    name: "",
    description: "",
    hue: WF_HUES[seed % WF_HUES.length],
    agents: {},
    blocks: [newStep(taken)],
    finalize: { push: false, open_pr: false },
  };
}

// ───────────────────────────── spec → editor ──────────────────────────────

function stepToEditor(s: Step): EStep {
  return {
    nid: newNid(),
    stepId: s.id,
    agent: s.agent || null,
    goal: s.goal,
    gate: s.gate ?? { type: "verdict" },
  };
}

/** Build editor state from a stored definition (edit/reload path). */
export function fromDefinition(def: Definition): EditorState {
  const spec = def.spec;
  return {
    id: def.id,
    name: def.name,
    description: def.description ?? "",
    hue: def.hue ?? WF_HUES[0],
    agents: structuredClone(spec.agents ?? {}),
    blocks: (spec.workflow ?? []).map((b) => stepToEditor(b.step)),
    finalize: spec.finalize ?? null,
  };
}

// ───────────────────────────── editor → spec ──────────────────────────────

function stepToSpec(s: EStep): Step {
  return { id: s.stepId, agent: s.agent ?? "", goal: s.goal, gate: s.gate };
}

/** Serialize editor state to a canonical `Spec` (the shape `wf_def_save` takes).
 *  Aliases are pruned to those actually referenced so deleting a step never
 *  leaves an orphan agent behind. */
export function toSpec(state: EditorState): Spec {
  const agents: Record<string, AgentSpec> = {};
  for (const s of state.blocks) {
    if (s.agent && state.agents[s.agent]) agents[s.agent] = state.agents[s.agent];
  }

  const spec: Spec = {
    version: SPEC_VERSION,
    name: state.name,
    agents,
    workflow: state.blocks.map((s) => ({ step: stepToSpec(s) })),
  };
  if (state.description.trim()) spec.description = state.description;
  if (state.finalize) spec.finalize = state.finalize;
  return spec;
}

// ───────────────────────────── agent aliasing ─────────────────────────────

/** Does an existing alias already stand for exactly this pick? Reusing it keeps
 *  the agents map small and stable across edits. */
function aliasMatchesPick(spec: AgentSpec, agentId: string, customAgents: CustomAgent[]): boolean {
  const ca = customAgents.find((a) => a.id === agentId);
  if (ca) return spec.custom_agent === agentId;
  // Base provider: a bare alias with no overrides.
  return (
    spec.base === agentId &&
    !spec.custom_agent &&
    spec.model == null &&
    spec.instructions == null &&
    !spec.skills?.length
  );
}

/** Resolve a picked agent (custom-agent id or base-provider id) to an alias,
 *  reusing a matching one or creating a fresh entry. Returns the next agents map
 *  and the alias to point the step at. Pure — never mutates the input. */
export function ensureAlias(
  agents: Record<string, AgentSpec>,
  agentId: string,
  customAgents: CustomAgent[],
): { agents: Record<string, AgentSpec>; alias: string } {
  for (const [alias, spec] of Object.entries(agents)) {
    if (aliasMatchesPick(spec, agentId, customAgents)) return { agents, alias };
  }
  const ca = customAgents.find((a) => a.id === agentId);
  const base = slugify(ca ? ca.name : agentId, "agent");
  let alias = base;
  let n = 1;
  while (agents[alias]) alias = `${base}-${++n}`;
  const spec: AgentSpec = ca ? { base: ca.base, custom_agent: ca.id } : { base: agentId };
  return { agents: { ...agents, [alias]: spec }, alias };
}

// ───────────────────────────── validation ─────────────────────────────

/** Per-step inline errors plus form-level messages. Mirrors the load-bearing
 *  §5.2 rules so invalid state is caught and shown before `wf_def_save` — the
 *  backend re-validates and stays the source of truth. */
export interface Validation {
  /** nid → messages for that step. */
  byNode: Record<NodeId, string[]>;
  /** Not tied to one step (e.g. the missing name). */
  form: string[];
  ok: boolean;
}

/** Validate editor state for inline display and save-gating. */
export function validateEditor(state: EditorState): Validation {
  const v: Validation = { byNode: {}, form: [], ok: true };
  if (!state.name.trim()) v.form.push("name this workflow");
  const seen = new Set<string>();
  for (const s of state.blocks) {
    const errs: string[] = [];
    if (!s.agent) errs.push("assign an agent");
    if (!s.stepId.trim()) errs.push("step id must not be empty");
    else if (seen.has(s.stepId)) errs.push(`duplicate step id '${s.stepId}'`);
    else seen.add(s.stepId);
    if (errs.length) v.byNode[s.nid] = errs;
  }
  v.ok = v.form.length === 0 && Object.keys(v.byNode).length === 0;
  return v;
}

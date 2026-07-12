// model.ts — the builder's editor state and its bidirectional mapping to the
// canonical `Spec` (src/workflows/spec.ts, mirror of src-tauri/.../spec.rs).
//
// Why an editor state distinct from `Spec`:
//  - React needs a stable per-node id (`nid`) to key and update the recursive
//    block tree; `Spec` blocks carry no such handle.
//  - Loop exit conditions reference a step by its *spec id*; the editor holds a
//    live `nid` reference instead so renaming/reordering can't dangle it.
//  - Agents: the builder lets each step pick a custom agent or a base provider
//    (the v0 UX). `Spec.agents` is an alias→AgentSpec map. We keep that map
//    verbatim in the editor (so any imported spec round-trips losslessly) and
//    synthesize/reuse an alias whenever the user makes a pick.
//
// `toSpec(fromSpec(spec))` is value-equal to `spec` for any spec our builder or
// the importer can produce — this is the property the vitest pins.

import type { CustomAgent } from "../../storage/customAgents";
import type {
  AgentSpec,
  Block,
  Budgets,
  CommsCap,
  Definition,
  Finalize,
  Gate,
  Integrate,
  Join,
  Orchestrate,
  Parallel,
  Spec,
  Step,
} from "../spec";
import { SPEC_VERSION } from "../spec";

// ───────────────────────────── editor state ─────────────────────────────

export type NodeId = string;

export interface EStep {
  kind: "step";
  nid: NodeId;
  /** The spec step id (unique across the whole spec). */
  stepId: string;
  /** A custom-agent id, a base-provider id, or null (unassigned). Aliases in
   *  `EditorState.agents` are derived from this at save time. */
  agent: string | null;
  goal: string;
  gate: Gate;
  budgets?: Budgets;
  /** `report` / `ask` only — `notify` is orchestrator-only (spec §5.1). */
  comms: CommsCap[];
}

export interface EParallel {
  kind: "parallel";
  nid: NodeId;
  join: Join;
  integrate: Integrate;
  maxConcurrent: number | null;
  /** v1: children are plain steps. */
  steps: EStep[];
}

export interface ELoop {
  kind: "loop";
  nid: NodeId;
  max: number;
  /** The body step whose `done` verdict exits the loop (spec §6.6). */
  untilNid: NodeId | null;
  body: EBlock[];
}

export interface EChildTemplate {
  agent: string | null;
  max: number;
}

export interface EComposeLimits {
  maxSubRuns: number;
  maxDepth: number;
}

export interface EOrchestrate {
  kind: "orchestrate";
  nid: NodeId;
  agent: string | null;
  goal: string;
  children: EChildTemplate | null;
  body: EStep[];
  join: Join;
  integrate: Integrate;
  comms: CommsCap[];
  compose: EComposeLimits | null;
}

export type EBlock = EStep | EParallel | ELoop | EOrchestrate;

export interface EditorState {
  /** Definition id; null for an unsaved workflow. */
  id: string | null;
  name: string;
  description: string;
  hue: number;
  budgets?: Budgets;
  /** Alias → agent spec, kept verbatim from the loaded spec and extended by
   *  picks. Pruned to referenced aliases at `toSpec`. */
  agents: Record<string, AgentSpec>;
  blocks: EBlock[];
  finalize: Finalize | null;
}

// ───────────────────────────── id generation ─────────────────────────────

let nidSeq = 0;
/** A stable UI handle for a tree node (never serialized). */
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

// ───────────────────────────── node constructors ──────────────────────────

export function newStep(taken: Set<string>): EStep {
  return {
    kind: "step",
    nid: newNid(),
    stepId: newStepId(taken),
    agent: null,
    goal: "",
    gate: { type: "verdict" },
    comms: [],
  };
}

export function newParallel(taken: Set<string>): EParallel {
  return {
    kind: "parallel",
    nid: newNid(),
    join: "all",
    integrate: "none",
    maxConcurrent: null,
    steps: [newStep(taken), newStep(taken)],
  };
}

export function newLoop(taken: Set<string>): ELoop {
  const body = newStep(taken);
  return { kind: "loop", nid: newNid(), max: 3, untilNid: body.nid, body: [body] };
}

export function newOrchestrate(taken: Set<string>): EOrchestrate {
  return {
    kind: "orchestrate",
    nid: newNid(),
    agent: null,
    goal: "",
    children: null,
    body: [newStep(taken)],
    join: "all",
    integrate: "none",
    comms: [],
    compose: null,
  };
}

// ───────────────────────────── blank / defaults ───────────────────────────

const WF_HUES = [265, 150, 25, 215, 320, 95, 175, 50];

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
    kind: "step",
    nid: newNid(),
    stepId: s.id,
    agent: s.agent || null,
    goal: s.goal,
    gate: s.gate ?? { type: "verdict" },
    budgets: s.budgets,
    comms: (s.comms ?? []).filter((c): c is "report" | "ask" => c !== "notify"),
  };
}

function blockToEditor(b: Block): EBlock {
  if ("step" in b) return stepToEditor(b.step);
  if ("parallel" in b) {
    return {
      kind: "parallel",
      nid: newNid(),
      join: b.parallel.join,
      integrate: b.parallel.integrate,
      maxConcurrent: b.parallel.max_concurrent ?? null,
      steps: b.parallel.steps.map(stepToEditor),
    };
  }
  if ("loop" in b) {
    const body = b.loop.body.map(blockToEditor);
    const until = findStepByStepId(body, b.loop.until.step);
    return {
      kind: "loop",
      nid: newNid(),
      max: b.loop.max,
      untilNid: until?.nid ?? null,
      body,
    };
  }
  // orchestrate
  const o = b.orchestrate;
  return {
    kind: "orchestrate",
    nid: newNid(),
    agent: o.agent || null,
    goal: o.goal,
    children: o.children ? { agent: o.children.agent || null, max: o.children.max } : null,
    body: (o.body ?? []).map(stepToEditor),
    join: o.join,
    integrate: o.integrate,
    comms: (o.comms ?? []).filter((c): c is "report" | "ask" => c !== "notify"),
    compose: o.compose
      ? { maxSubRuns: o.compose.max_sub_runs, maxDepth: o.compose.max_depth }
      : null,
  };
}

/** Depth-first search for an editor step by its spec step id. */
function findStepByStepId(blocks: EBlock[], stepId: string): EStep | null {
  for (const b of blocks) {
    if (b.kind === "step") {
      if (b.stepId === stepId) return b;
    } else if (b.kind === "parallel") {
      const hit = b.steps.find((s) => s.stepId === stepId);
      if (hit) return hit;
    } else if (b.kind === "loop") {
      const hit = findStepByStepId(b.body, stepId);
      if (hit) return hit;
    } else {
      const hit = b.body.find((s) => s.stepId === stepId);
      if (hit) return hit;
    }
  }
  return null;
}

/** Look up an editor step by its `nid` anywhere in the tree. */
function findStepByNid(blocks: EBlock[], nid: NodeId): EStep | null {
  for (const b of blocks) {
    if (b.kind === "step") {
      if (b.nid === nid) return b;
    } else if (b.kind === "parallel") {
      const hit = b.steps.find((s) => s.nid === nid);
      if (hit) return hit;
    } else if (b.kind === "loop") {
      const hit = findStepByNid(b.body, nid);
      if (hit) return hit;
    } else {
      const hit = b.body.find((s) => s.nid === nid);
      if (hit) return hit;
    }
  }
  return null;
}

/** Build editor state from a stored definition (edit/reload path). */
export function fromDefinition(def: Definition): EditorState {
  const spec = def.spec;
  return {
    id: def.id,
    name: def.name,
    description: def.description ?? "",
    hue: def.hue ?? WF_HUES[0],
    budgets: spec.budgets,
    agents: structuredClone(spec.agents ?? {}),
    blocks: (spec.workflow ?? []).map(blockToEditor),
    finalize: spec.finalize ?? null,
  };
}

// ───────────────────────────── editor → spec ──────────────────────────────

function stepToSpec(s: EStep): Step {
  const out: Step = { id: s.stepId, agent: s.agent ?? "", goal: s.goal, gate: s.gate };
  if (s.budgets && Object.keys(pruneBudgets(s.budgets)).length)
    out.budgets = pruneBudgets(s.budgets);
  if (s.comms.length) out.comms = s.comms;
  return out;
}

function blockToSpec(b: EBlock): Block {
  if (b.kind === "step") return { step: stepToSpec(b) };
  if (b.kind === "parallel") {
    const par: Parallel = {
      join: b.join,
      integrate: b.integrate,
      steps: b.steps.map(stepToSpec),
    };
    if (b.maxConcurrent != null) par.max_concurrent = b.maxConcurrent;
    return { parallel: par };
  }
  if (b.kind === "loop") {
    const until = b.untilNid ? findStepByNid(b.body, b.untilNid) : null;
    return {
      loop: {
        max: b.max,
        until: { step: until?.stepId ?? "", verdict: "done" },
        body: b.body.map(blockToSpec),
      },
    };
  }
  const orch: Orchestrate = {
    agent: b.agent ?? "",
    goal: b.goal,
    join: b.join,
    integrate: b.integrate,
  };
  if (b.children) orch.children = { agent: b.children.agent ?? "", max: b.children.max };
  if (b.body.length) orch.body = b.body.map(stepToSpec);
  if (b.comms.length) orch.comms = b.comms;
  if (b.compose)
    orch.compose = { max_sub_runs: b.compose.maxSubRuns, max_depth: b.compose.maxDepth };
  return { orchestrate: orch };
}

/** Every alias referenced by a step, orchestrator, or child template. */
function referencedAliases(blocks: EBlock[], into: Set<string>): void {
  for (const b of blocks) {
    if (b.kind === "step") {
      if (b.agent) into.add(b.agent);
    } else if (b.kind === "parallel") {
      for (const s of b.steps) if (s.agent) into.add(s.agent);
    } else if (b.kind === "loop") {
      referencedAliases(b.body, into);
    } else {
      if (b.agent) into.add(b.agent);
      if (b.children?.agent) into.add(b.children.agent);
      for (const s of b.body) if (s.agent) into.add(s.agent);
    }
  }
}

/** Drop budget fields left blank in the UI so an all-empty budgets object never
 *  serializes (and never trips the positivity validator with a stray null). */
function pruneBudgets(b: Budgets): Budgets {
  const out: Budgets = {};
  for (const [k, v] of Object.entries(b)) {
    if (v != null) (out as Record<string, number>)[k] = v as number;
  }
  return out;
}

/** Serialize editor state to a canonical `Spec` (the shape `wf_def_save` takes).
 *  Aliases are pruned to those actually referenced so deleting a step never
 *  leaves an orphan agent behind. */
export function toSpec(state: EditorState): Spec {
  const refs = new Set<string>();
  referencedAliases(state.blocks, refs);
  const agents: Record<string, AgentSpec> = {};
  for (const alias of refs) {
    if (state.agents[alias]) agents[alias] = state.agents[alias];
  }

  const spec: Spec = {
    version: SPEC_VERSION,
    name: state.name,
    agents,
    workflow: state.blocks.map(blockToSpec),
  };
  if (state.description.trim()) spec.description = state.description;
  if (state.budgets && Object.keys(pruneBudgets(state.budgets)).length) {
    spec.budgets = pruneBudgets(state.budgets);
  }
  if (state.finalize) spec.finalize = state.finalize;
  return spec;
}

// ───────────────────────────── agent aliasing ─────────────────────────────

function slugify(s: string): string {
  return (
    s
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "agent"
  );
}

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
  const base = slugify(ca ? ca.name : agentId);
  let alias = base;
  let n = 1;
  while (agents[alias]) alias = `${base}-${++n}`;
  const spec: AgentSpec = ca ? { base: ca.base, custom_agent: ca.id } : { base: agentId };
  return { agents: { ...agents, [alias]: spec }, alias };
}

// ───────────────────────────── validation ─────────────────────────────

/** Per-node inline errors plus form-level messages. Mirrors the load-bearing
 *  §5.2 rules so invalid state is caught and shown before `wf_def_save` — the
 *  backend re-validates and stays the source of truth. */
export interface Validation {
  /** nid → messages for that node. */
  byNode: Record<NodeId, string[]>;
  /** Not tied to one node (e.g. the missing name). */
  form: string[];
  ok: boolean;
}

const PATH_ABS = /^\//;
function badArtifactPath(path: string): boolean {
  const p = path.trim();
  if (!p) return true;
  return PATH_ABS.test(p) || p.split("/").some((seg) => seg === "..");
}

function validateBudgets(b: Budgets | undefined, push: (m: string) => void): void {
  if (!b) return;
  for (const [field, value] of Object.entries(b)) {
    if (value != null && (value as number) <= 0) {
      push(`budget '${field}' must be a positive number`);
    }
  }
}

function validateStep(s: EStep, v: Validation, seen: Map<string, EStep>): void {
  const errs: string[] = [];
  const push = (m: string) => errs.push(m);
  if (!s.agent) push("assign an agent");
  if (!s.stepId.trim()) push("step id must not be empty");
  else if (seen.has(s.stepId)) push(`duplicate step id '${s.stepId}'`);
  else seen.set(s.stepId, s);
  if (s.gate.type === "artifact" && badArtifactPath(s.gate.path)) {
    push("artifact path must be repo-relative (no leading '/' and no '..')");
  }
  validateBudgets(s.budgets, push);
  if (errs.length) v.byNode[s.nid] = errs;
}

function validateBlocks(blocks: EBlock[], v: Validation, seen: Map<string, EStep>): void {
  for (const b of blocks) {
    if (b.kind === "step") {
      validateStep(b, v, seen);
    } else if (b.kind === "parallel") {
      const errs: string[] = [];
      if (b.steps.length === 0) errs.push("add at least one step");
      if (b.maxConcurrent != null && b.maxConcurrent < 1) {
        errs.push("max concurrent must be ≥ 1");
      }
      if (errs.length) v.byNode[b.nid] = errs;
      for (const s of b.steps) validateStep(s, v, seen);
    } else if (b.kind === "loop") {
      const errs: string[] = [];
      if (b.max < 1) errs.push("max iterations must be ≥ 1");
      const until = b.untilNid ? findStepByNid(b.body, b.untilNid) : null;
      if (!until) errs.push("choose which step's verdict ends the loop");
      else if (until.gate.type !== "verdict") {
        errs.push(`the exit step must use a verdict gate (it is '${until.gate.type}')`);
      }
      if (errs.length) v.byNode[b.nid] = errs;
      validateBlocks(b.body, v, seen);
    } else {
      const errs: string[] = [];
      if (!b.agent) errs.push("assign an orchestrator agent");
      if (b.children && b.children.max < 1) errs.push("children max must be ≥ 1");
      if (b.children && !b.children.agent) errs.push("choose the child agent");
      if (b.compose) {
        if (b.compose.maxDepth < 1 || b.compose.maxDepth > 2) {
          errs.push("compose depth must be 1 or 2");
        }
        if (b.compose.maxSubRuns < 1) errs.push("compose sub-runs must be ≥ 1");
      }
      if (errs.length) v.byNode[b.nid] = errs;
      for (const s of b.body) validateStep(s, v, seen);
    }
  }
}

/** Validate editor state for inline display and save-gating. */
export function validateEditor(state: EditorState): Validation {
  const v: Validation = { byNode: {}, form: [], ok: true };
  if (!state.name.trim()) v.form.push("name this workflow");
  validateBudgets(state.budgets, (m) => v.form.push(`run ${m}`));
  validateBlocks(state.blocks, v, new Map());
  v.ok = v.form.length === 0 && Object.keys(v.byNode).length === 0;
  return v;
}

// Starter pack installer — seeds the four specialist custom agents and the
// "Feature pipeline" workflow. Idempotent: it matches existing items by name
// and skips them, so re-installing never duplicates. Deliberately explicit
// (invoked from a Settings affordance), never run silently on launch.

import type { CustomAgent, NewCustomAgent } from "@/storage/customAgents";
import type { Definition, Spec } from "@/workflows/spec";
import {
  buildFeaturePipelineSpec,
  STARTER_AGENTS,
  STARTER_WORKFLOW_HUE,
  STARTER_WORKFLOW_NAME,
} from "./presets";

/** The library surface the installer needs — passed in so the flow stays
 *  testable and decoupled from the store/api singletons. */
export interface InstallDeps {
  /** Current custom agents (for the by-name skip). */
  existingAgents: CustomAgent[];
  /** Persist one new custom agent, returning the created row (with its id). */
  createAgent: (agent: NewCustomAgent) => Promise<CustomAgent>;
  /** Current workflow definitions (for the by-name skip). */
  existingDefinitions: Definition[];
  /** Persist a workflow definition. */
  saveDefinition: (spec: Spec, hue: number) => Promise<unknown>;
}

export interface InstallResult {
  agentsCreated: string[];
  agentsSkipped: string[];
  workflowCreated: boolean;
}

/** Seed the starter pack. Creates any of the four specialists missing by name,
 *  reuses those already present, then creates the "Feature pipeline" workflow
 *  (wired to whichever agent ids resulted) unless a workflow of that name
 *  already exists. */
export async function installStarterPack(deps: InstallDeps): Promise<InstallResult> {
  const agentsCreated: string[] = [];
  const agentsSkipped: string[] = [];
  const idByRole: Record<string, string> = {};

  for (const { role, preset } of STARTER_AGENTS) {
    const existing = deps.existingAgents.find((a) => a.name === preset.name);
    if (existing) {
      idByRole[role] = existing.id;
      agentsSkipped.push(preset.name);
      continue;
    }
    const created = await deps.createAgent(preset);
    idByRole[role] = created.id;
    agentsCreated.push(preset.name);
  }

  let workflowCreated = false;
  const workflowExists = deps.existingDefinitions.some((d) => d.name === STARTER_WORKFLOW_NAME);
  if (!workflowExists) {
    await deps.saveDefinition(buildFeaturePipelineSpec(idByRole), STARTER_WORKFLOW_HUE);
    workflowCreated = true;
  }

  return { agentsCreated, agentsSkipped, workflowCreated };
}

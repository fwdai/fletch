import { describe, expect, it, vi } from "vitest";
import type { CustomAgent, NewCustomAgent } from "@/storage/customAgents";
import type { Definition, Spec } from "@/workflows/spec";
import { installStarterPack } from "./install";
import { STARTER_AGENTS, STARTER_WORKFLOW_NAME } from "./presets";

function fakeAgent(name: string, id: string): CustomAgent {
  return {
    id,
    name,
    description: "",
    color: 0,
    base: "claude",
    model: null,
    effort: null,
    instructions: "",
    skillIds: [],
    mcpServerIds: [],
    created_at: 0,
    updated_at: 0,
  };
}

function deps(existingAgents: CustomAgent[], existingDefinitions: Definition[]) {
  const saved: Spec[] = [];
  let seq = 0;
  const createAgent = vi.fn(async (a: NewCustomAgent) => fakeAgent(a.name, `new-${seq++}`));
  const saveDefinition = vi.fn(async (spec: Spec) => {
    saved.push(spec);
  });
  return {
    d: { existingAgents, createAgent, existingDefinitions, saveDefinition },
    saved,
  };
}

describe("installStarterPack", () => {
  it("seeds all four agents and the workflow on a clean library", async () => {
    const { d, saved } = deps([], []);
    const res = await installStarterPack(d);

    expect(res.agentsCreated).toEqual(["Architect", "Coder", "Reviewer", "Tester"]);
    expect(res.agentsSkipped).toEqual([]);
    expect(res.workflowCreated).toBe(true);
    expect(d.createAgent).toHaveBeenCalledTimes(4);

    const spec = saved[0];
    expect(spec.name).toBe(STARTER_WORKFLOW_NAME);
    // Every alias is wired to a freshly created custom-agent id.
    for (const alias of Object.values(spec.agents)) {
      expect(alias.custom_agent).toMatch(/^new-\d+$/);
    }
    // Architect → implement → parallel[review, test] → ship.
    expect(spec.workflow.length).toBe(4);
    expect("parallel" in spec.workflow[2]).toBe(true);
  });

  it("is idempotent: reuses existing agents by name and skips an existing workflow", async () => {
    const existingAgents = STARTER_AGENTS.map((a, i) => fakeAgent(a.preset.name, `old-${i}`));
    const existingDef = {
      id: "wf-1",
      name: STARTER_WORKFLOW_NAME,
      description: "",
      hue: null,
      spec: {} as Spec,
      run_count: 0,
      created_at: 0,
      updated_at: 0,
    } satisfies Definition;

    const { d } = deps(existingAgents, [existingDef]);
    const res = await installStarterPack(d);

    expect(res.agentsCreated).toEqual([]);
    expect(res.agentsSkipped).toEqual(["Architect", "Coder", "Reviewer", "Tester"]);
    expect(res.workflowCreated).toBe(false);
    expect(d.createAgent).not.toHaveBeenCalled();
    expect(d.saveDefinition).not.toHaveBeenCalled();
  });

  it("wires the workflow to existing agent ids even when only the workflow is missing", async () => {
    const existingAgents = STARTER_AGENTS.map((a, i) => fakeAgent(a.preset.name, `old-${i}`));
    const { d, saved } = deps(existingAgents, []);
    const res = await installStarterPack(d);

    expect(res.agentsSkipped.length).toBe(4);
    expect(res.workflowCreated).toBe(true);
    const ids = Object.values(saved[0].agents).map((a) => a.custom_agent);
    expect(ids).toEqual(["old-0", "old-1", "old-2", "old-3"]);
  });
});

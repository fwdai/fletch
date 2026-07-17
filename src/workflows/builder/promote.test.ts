import { describe, expect, it } from "vitest";
import type { CustomAgent } from "../../storage/customAgents";
import type { PromoteSeed } from "../../store/types";
import { seedEditorFromPromotion } from "./promote";

function seed(over: Partial<PromoteSeed> = {}): PromoteSeed {
  return {
    agentId: "ag-1",
    agentName: "Fuji",
    agentPick: "claude",
    task: "Add retry/backoff to the sync worker",
    baseSha: "deadbeefcafef00d",
    baseLabel: "deadbeef",
    repoPath: "/repo",
    projectId: "p-1",
    ...over,
  };
}

describe("seedEditorFromPromotion", () => {
  it("seeds a single step assigned to the session's provider, goaled with the brief", () => {
    const ed = seedEditorFromPromotion(seed(), [], 0, "Fuji");
    expect(ed.blocks).toHaveLength(1);
    const step = ed.blocks[0];
    expect(step.kind).toBe("step");
    if (step.kind !== "step") throw new Error("expected step");
    expect(step.goal).toBe("Add retry/backoff to the sync worker");
    // The step's alias resolves to the picked base provider.
    expect(ed.agents[step.agent as string]).toEqual({ base: "claude" });
  });

  it("names the workflow from the brief's first line", () => {
    const ed = seedEditorFromPromotion(
      seed({ task: "Fix the flaky login test\nmore detail" }),
      [],
      0,
      "Fuji",
    );
    expect(ed.name).toBe("Fix the flaky login test");
  });

  it("resolves a custom-agent pick to a custom-agent alias spec", () => {
    const custom: CustomAgent[] = [
      { id: "ca-1", name: "Reviewer", base: "codex", color: 200 } as CustomAgent,
    ];
    const ed = seedEditorFromPromotion(seed({ agentPick: "ca-1" }), custom, 0, "Fuji");
    const step = ed.blocks[0];
    if (step.kind !== "step") throw new Error("expected step");
    expect(ed.agents[step.agent as string]).toEqual({ base: "codex", custom_agent: "ca-1" });
  });

  it("keeps finalize off — promotion never lifts the run publish boundary", () => {
    const ed = seedEditorFromPromotion(seed(), [], 0, "Fuji");
    expect(ed.finalize).toEqual({ push: false, open_pr: false });
  });

  it("falls back to an agent-named title when the brief is empty", () => {
    const ed = seedEditorFromPromotion(seed({ task: "" }), [], 0, "Fuji");
    expect(ed.name).toBe("Promoted from Fuji");
  });
});

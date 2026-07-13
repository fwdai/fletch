// Import-mapping logic (spec §14.3 / SLICES F3 acceptance). The load-bearing
// property: "map to yours" attaches the local custom-agent id (so the workflow
// runs against the user's agent) while "use embedded" keeps the file's portable
// spec — which is what makes the §5.3 example runnable on a machine without the
// custom agents.

import { describe, expect, it } from "vitest";
import type { ImportReport } from "../../spec";
import { applyResolutions, defaultChoice, initialChoices } from "./resolve";

/** A report with one alias that has a local match and one that does not. */
function report(): ImportReport {
  return {
    spec: {
      version: 1,
      name: "feature-pipeline",
      agents: {
        planner: { base: "claude", model: "opus", instructions: "architect" },
        coder: { base: "codex" },
      },
      workflow: [{ step: { id: "plan", agent: "planner", goal: "plan it" } }],
    },
    agents: [
      {
        alias: "planner",
        base: "claude",
        local_match: { id: "ca-123", name: "planner" },
        embedded: { base: "claude", model: "opus", instructions: "architect" },
      },
      {
        alias: "coder",
        base: "codex",
        local_match: null,
        embedded: { base: "codex" },
      },
    ],
    warnings: [],
  };
}

describe("import resolution", () => {
  it("defaults to mapping when a local agent matches, else embedding", () => {
    const r = report();
    expect(defaultChoice(r.agents[0])).toBe("map");
    expect(defaultChoice(r.agents[1])).toBe("embed");
    expect(initialChoices(r)).toEqual({ planner: "map", coder: "embed" });
  });

  it("mapping attaches the local custom-agent id, keeping embedded fields as fallback", () => {
    const spec = applyResolutions(report(), initialChoices(report()));
    expect(spec.agents.planner).toEqual({
      base: "claude",
      model: "opus",
      instructions: "architect",
      custom_agent: "ca-123",
    });
    // No local match → embedded spec, never a custom_agent.
    expect(spec.agents.coder).toEqual({ base: "codex" });
    expect(spec.agents.coder.custom_agent).toBeUndefined();
  });

  it("embedding a matched alias drops the local id (runs on a machine without it)", () => {
    const spec = applyResolutions(report(), { planner: "embed", coder: "embed" });
    expect(spec.agents.planner).toEqual({
      base: "claude",
      model: "opus",
      instructions: "architect",
    });
    expect(spec.agents.planner.custom_agent).toBeUndefined();
  });

  it("a 'map' choice with no local match falls back to the embedded spec", () => {
    const spec = applyResolutions(report(), { planner: "map", coder: "map" });
    expect(spec.agents.coder).toEqual({ base: "codex" });
  });

  it("leaves the rest of the spec untouched", () => {
    const spec = applyResolutions(report(), initialChoices(report()));
    expect(spec.name).toBe("feature-pipeline");
    expect(spec.workflow).toEqual([{ step: { id: "plan", agent: "planner", goal: "plan it" } }]);
  });
});

// flattenSteps is the rail's backbone: the stepper, the thread's step numbering,
// the composer's flow strip and the sidebar row's lead agent all read it.

import { describe, expect, it } from "vitest";
import type { Block, Spec } from "../../spec";
import { flattenSteps } from "./flatten";

function spec(workflow: Block[]): Spec {
  return { version: 1, name: "s", agents: { a: { base: "claude" } }, workflow };
}

/** A block only a retired run's frozen snapshot still holds. */
function legacy(block: unknown): Block {
  return block as Block;
}

describe("flattenSteps", () => {
  it("lists the steps in document order with their gate kind", () => {
    const steps = flattenSteps(
      spec([
        { step: { id: "plan", agent: "a", goal: "plan it", gate: { type: "verdict" } } },
        { step: { id: "code", agent: "a", goal: "build it", gate: { type: "commit" } } },
        { step: { id: "check", agent: "a", goal: "check it" } },
      ]),
    );
    expect(steps).toEqual([
      { id: "plan", agentAlias: "a", goal: "plan it", gate: "verdict", block: 0, position: 0 },
      { id: "code", agentAlias: "a", goal: "build it", gate: "commit", block: 1, position: 0 },
      { id: "check", agentAlias: "a", goal: "check it", gate: undefined, block: 2, position: 0 },
    ]);
  });

  it("is empty without a spec", () => {
    expect(flattenSteps(null)).toEqual([]);
    expect(flattenSteps(spec([]))).toEqual([]);
  });

  it("walks the containers a retired engine's run snapshot still holds, in document order", () => {
    // What the clone-per-step engine could launch: a run row carries it
    // verbatim, and its nested steps have attempts to label and order.
    const old = spec([
      { step: { id: "plan", agent: "a", goal: "g" } },
      legacy({
        loop: {
          max: 2,
          until: { step: "check" },
          body: [
            { step: { id: "code", agent: "a", goal: "g" } },
            { step: { id: "check", agent: "a", goal: "g" } },
          ],
        },
      }),
      legacy({
        parallel: {
          steps: [
            { id: "review", agent: "a", goal: "g" },
            { id: "test", agent: "a", goal: "g" },
          ],
        },
      }),
      { step: { id: "ship", agent: "a", goal: "g" } },
    ]);
    const steps = flattenSteps(old);
    expect(steps.map((s) => s.id)).toEqual(["plan", "code", "check", "review", "test", "ship"]);
    // Block and position carry the container structure the thread orders by.
    expect(steps.map((s) => [s.block, s.position])).toEqual([
      [0, 0],
      [1, 0],
      [1, 1],
      [2, 0],
      [2, 1],
      [3, 0],
    ]);
  });

  it("names the stages a retired run created beside its spec, only when they ran", () => {
    const old = spec([
      legacy({
        parallel: {
          steps: [{ id: "review", agent: "a", goal: "g" }],
        },
      }),
      legacy({
        orchestrate: {
          agent: "lead",
          goal: "coordinate",
          body: [{ id: "child-a", agent: "a", goal: "g" }],
          children: { agent: "worker", max: 3 },
        },
      }),
    ]);
    // Without attempts only the spec's own steps and the orchestrator exist.
    expect(flattenSteps(old).map((s) => s.id)).toEqual(["review", "orchestrate-1", "child-a"]);

    const attempts = [
      { step_id: "orchestrate-1::dyn-1" },
      { step_id: "orchestrate-1::dyn-0" },
      { step_id: "__merge_0" },
      { step_id: "review" },
    ];
    const steps = flattenSteps(old, attempts);
    expect(steps.map((s) => s.id)).toEqual([
      "review",
      "__merge_0",
      "orchestrate-1",
      "child-a",
      "orchestrate-1::dyn-0",
      "orchestrate-1::dyn-1",
    ]);
    expect(steps.find((s) => s.id === "orchestrate-1")?.agentAlias).toBe("lead");
    expect(steps.find((s) => s.id === "orchestrate-1::dyn-0")?.agentAlias).toBe("worker");
    expect(steps.map((s) => [s.block, s.position])).toEqual([
      [0, 0],
      [0, 1],
      [1, 0],
      [1, 1],
      [1, 2],
      [1, 3],
    ]);
  });

  it("seats a retired run's conflict resolver after its block's work and before its merge", () => {
    const old = spec([
      legacy({
        parallel: {
          steps: [
            { id: "review", agent: "a", goal: "g" },
            { id: "test", agent: "a", goal: "g" },
          ],
        },
      }),
      legacy({
        orchestrate: {
          agent: "lead",
          goal: "coordinate",
          body: [{ id: "child-a", agent: "a", goal: "g" }],
        },
      }),
      { step: { id: "ship", agent: "a", goal: "g" } },
    ]);
    const attempts = [
      { step_id: "ship" },
      { step_id: "__merge_1" },
      { step_id: "__resolve_1" },
      { step_id: "__merge_0" },
      { step_id: "__resolve_0" },
      { step_id: "review" },
    ];
    const steps = flattenSteps(old, attempts);
    // Both block kinds integrated through the same machinery: resolver, then
    // merge, then whatever block came next.
    expect(steps.map((s) => s.id)).toEqual([
      "review",
      "test",
      "__resolve_0",
      "__merge_0",
      "orchestrate-1",
      "child-a",
      "__resolve_1",
      "__merge_1",
      "ship",
    ]);
    // The parallel resolver ran as the conflicted child's agent, the
    // orchestrate one as the orchestrator; both sit inside their block.
    expect(steps.find((s) => s.id === "__resolve_0")?.agentAlias).toBe("a");
    expect(steps.find((s) => s.id === "__resolve_1")?.agentAlias).toBe("lead");
    expect(steps.map((s) => [s.block, s.position])).toEqual([
      [0, 0],
      [0, 1],
      [0, 2],
      [0, 3],
      [1, 0],
      [1, 1],
      [1, 2],
      [1, 3],
      [2, 0],
    ]);
  });

  it("attributes a parallel resolver to the branch whose agent actually ran it", () => {
    // The engine sent the conflicted child's own agent; the exec row only
    // names the agent record, whose provider or custom agent tells the
    // branches apart.
    const old: Spec = {
      version: 1,
      name: "s",
      agents: {
        reviewer: { base: "claude", custom_agent: "ca-reviewer" },
        tester: { base: "codex" },
        coder: { base: "claude" },
      },
      workflow: [
        legacy({
          parallel: {
            steps: [
              { id: "review", agent: "reviewer", goal: "g" },
              { id: "test", agent: "tester", goal: "g" },
              { id: "code", agent: "coder", goal: "g" },
            ],
          },
        }),
      ],
    };
    const resolver = (agentId: string) =>
      flattenSteps(
        old,
        [{ step_id: "__resolve_0", agent_id: agentId }],
        [
          { id: "ag-tester", provider: "codex" },
          { id: "ag-coder", provider: "claude", custom_agent_id: null },
          { id: "ag-reviewer", provider: "claude", custom_agent_id: "ca-reviewer" },
        ],
      ).find((s) => s.id === "__resolve_0")?.agentAlias;
    expect(resolver("ag-tester")).toBe("tester");
    expect(resolver("ag-coder")).toBe("coder");
    expect(resolver("ag-reviewer")).toBe("reviewer");
    // No record to go on: the first branch stands in.
    expect(resolver("ag-gone")).toBe("reviewer");
  });
});

// flattenSteps is the rail's backbone: the stepper, the thread's step numbering,
// the composer's flow strip and the sidebar row's lead agent all read it.

import { describe, expect, it } from "vitest";
import type { Block, Spec } from "../../spec";
import { flattenSteps } from "./flatten";

function spec(workflow: Block[]): Spec {
  return { version: 1, name: "s", agents: { a: { base: "claude" } }, workflow };
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
      { id: "plan", agentAlias: "a", goal: "plan it", gate: "verdict" },
      { id: "code", agentAlias: "a", goal: "build it", gate: "commit" },
      { id: "check", agentAlias: "a", goal: "check it", gate: undefined },
    ]);
  });

  it("is empty without a spec", () => {
    expect(flattenSteps(null)).toEqual([]);
    expect(flattenSteps(spec([]))).toEqual([]);
  });

  it("skips entries with no step in a run's frozen launch snapshot", () => {
    // What the retired engine could launch: a run row still carries it verbatim.
    const old = spec([
      { step: { id: "plan", agent: "a", goal: "g" } },
      { loop: { max: 2, body: [] } } as unknown as Block,
    ]);
    expect(flattenSteps(old).map((s) => s.id)).toEqual(["plan"]);
  });
});

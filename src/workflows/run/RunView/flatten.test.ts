// isSequentialSpec mirrors the kernel runner's `kernel_eligible`
// (src-tauri/src/workflow/runner/mod.rs). If the two drift, the monitor renders
// a thread for a run that isn't one — or hides the thread from one that is.

import { describe, expect, it } from "vitest";
import type { Block, Gate, Spec } from "../../spec";
import { isSequentialSpec } from "./flatten";

function spec(workflow: Block[]): Spec {
  return { version: 1, name: "s", agents: { a: { base: "claude" } }, workflow };
}

function step(id: string, gate?: Gate): Block {
  return { step: { id, agent: "a", goal: "g", ...(gate ? { gate } : {}) } };
}

describe("isSequentialSpec", () => {
  it("accepts a flat sequence of commit/verdict steps", () => {
    expect(
      isSequentialSpec(spec([step("plan", { type: "verdict" }), step("code", { type: "commit" })])),
    ).toBe(true);
  });

  it("accepts a step with no gate — serde defaults it to verdict", () => {
    expect(isSequentialSpec(spec([step("plan")]))).toBe(true);
  });

  it("rejects an empty workflow", () => {
    expect(isSequentialSpec(spec([]))).toBe(false);
    expect(isSequentialSpec(null)).toBe(false);
  });

  it("rejects gates that suspend the run or need external verification", () => {
    for (const gate of [
      { type: "approval" },
      { type: "tests" },
      { type: "artifact", path: "out.md" },
    ] as Gate[]) {
      expect(isSequentialSpec(spec([step("plan"), step("check", gate)]))).toBe(false);
    }
  });

  it("rejects parallel, loop and orchestrate blocks", () => {
    const parallel: Block = { parallel: { join: "all", integrate: "merge", steps: [] } };
    const loop: Block = { loop: { max: 2, until: { step: "plan" }, body: [step("plan")] } };
    const orchestrate: Block = {
      orchestrate: { agent: "a", goal: "g", join: "all", integrate: "none" },
    };
    for (const block of [parallel, loop, orchestrate]) {
      expect(isSequentialSpec(spec([step("plan"), block]))).toBe(false);
    }
  });
});

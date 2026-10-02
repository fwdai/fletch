import { describe, expect, it } from "vitest";
import type { AgentStatus, AgentView, RestoreReport } from "@/api";
import {
  type CodeState,
  rewindBlocker,
  rewindOptions,
} from "@/components/Workspace/RewindMenu/options";

const report: RestoreReport = { repos: [] };
const idle = { status: "idle" as AgentStatus, view: "custom" as AgentView };

/** `[scope, reason]` per option, in menu order. */
const reasons = (blocker: string | null, code: CodeState) =>
  rewindOptions(blocker, code).map((o) => [o.scope, o.reason]);

describe("rewind menu availability", () => {
  it("offers every rewind of an idle agent in the chat view whose code is restorable", () => {
    expect(rewindBlocker(idle)).toBeNull();
    expect(rewindOptions(null, { report }).map((o) => [o.scope, o.label, o.reason])).toEqual([
      ["conversation", "Restore conversation", null],
      ["code", "Restore code", null],
      ["both", "Restore conversation and code", null],
    ]);
  });

  it("offers nothing while the agent works or starts", () => {
    for (const status of ["running", "spawning"] as AgentStatus[]) {
      const blocker = rewindBlocker({ ...idle, status });
      expect(blocker).toBe("Stop the agent first.");
      expect(reasons(blocker, { report })).toEqual([
        ["conversation", blocker],
        ["code", blocker],
        ["both", blocker],
      ]);
    }
    // A stopped or failed agent can be rewound.
    for (const status of ["stopped", "error"] as AgentStatus[]) {
      expect(rewindBlocker({ ...idle, status })).toBeNull();
    }
  });

  it("offers every rewind in the native view too", () => {
    const native = { ...idle, view: "native" as AgentView };
    const blocker = rewindBlocker(native);
    expect(blocker).toBeNull();
    expect(reasons(blocker, { report })).toEqual([
      ["conversation", null],
      ["code", null],
      ["both", null],
    ]);
  });

  it("keeps the conversation when the code can't be restored, and says why", () => {
    for (const why of [
      "The code as of this message isn't available: it ran in another workspace.",
      "The code as of this message isn't available: no snapshot of it was kept.",
    ]) {
      expect(reasons(null, { unavailable: why })).toEqual([
        ["conversation", null],
        ["code", why],
        ["both", why],
      ]);
    }
  });

  it("holds the code back until its preview is in", () => {
    expect(reasons(null, "checking")).toEqual([
      ["conversation", null],
      ["code", "Checking the code…"],
      ["both", "Checking the code…"],
    ]);
  });
});

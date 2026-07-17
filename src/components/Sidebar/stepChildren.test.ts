import { describe, expect, it } from "vitest";
import type { AgentRecord, AgentStatus } from "@/api";
import { deriveStepChildren } from "./stepChildren";

function agent(over: Partial<AgentRecord> & { id: string }): AgentRecord {
  return {
    project_id: "p",
    name: over.id,
    provider: "claude",
    repos: [],
    task: "",
    status: "idle" as AgentStatus,
    view: "custom",
    created_at: "2026-01-01T00:00:00.000Z",
    ...over,
  } as AgentRecord;
}

describe("deriveStepChildren", () => {
  it("orders by spawn time, then id for stable ties", () => {
    const out = deriveStepChildren([
      agent({ id: "b", created_at: "2026-01-01T00:00:02.000Z" }),
      agent({ id: "a", created_at: "2026-01-01T00:00:01.000Z" }),
      agent({ id: "c", created_at: "2026-01-01T00:00:02.000Z" }),
    ]);
    expect(out.map((c) => c.agent.id)).toEqual(["a", "b", "c"]);
  });

  it("maps status to the AgentRow rail vocabulary", () => {
    const out = deriveStepChildren([
      agent({ id: "run", status: "running" }),
      agent({ id: "spawn", status: "spawning" }),
      agent({ id: "err", status: "error" }),
      agent({ id: "idle", status: "idle" }),
    ]);
    const by = Object.fromEntries(out.map((c) => [c.agent.id, c]));
    expect(by.run).toMatchObject({ rail: "run", working: true });
    expect(by.spawn).toMatchObject({ rail: "run", working: true });
    expect(by.err).toMatchObject({ rail: "err", working: false });
    expect(by.idle).toMatchObject({ rail: "idle", working: false });
  });

  it("drops archived step agents so a finished run shows no tombstones", () => {
    const out = deriveStepChildren([
      agent({ id: "live" }),
      agent({
        id: "gone",
        archive: { archived_at: "x", repos: [], diff_stats: { additions: 0, deletions: 0 } },
      }),
    ]);
    expect(out.map((c) => c.agent.id)).toEqual(["live"]);
  });

  it("yields an empty list when there are no live step agents", () => {
    expect(deriveStepChildren([])).toEqual([]);
  });
});

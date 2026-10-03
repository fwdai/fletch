import { describe, expect, it } from "vitest";
import type { AgentRecord } from "../api";
import type { AppState } from "../store";
import { isAgentBusy, patchAgentRecord } from "./agentLookups";

const record = (id: string, status: AgentRecord["status"]): AgentRecord =>
  ({ id, status, provider: "claude", repos: [] }) as unknown as AgentRecord;

/** A store with one sidebar agent and one Roadmap chat the snapshot omits. */
const state = (over: Partial<AppState> = {}): AppState =>
  ({
    workspace: { agents: [record("fuji", "idle")], projects: [] },
    offSidebarAgents: { sakura: record("sakura", "idle") },
    sending: {},
    ...over,
  }) as unknown as AppState;

describe("patchAgentRecord", () => {
  it("patches a sidebar agent in the snapshot only", () => {
    const patch = patchAgentRecord(state(), "fuji", { status: "running" });
    expect(patch.workspace?.agents[0].status).toBe("running");
    expect(patch.offSidebarAgents).toBeUndefined();
  });

  it("patches an off-sidebar chat in its registry only", () => {
    const patch = patchAgentRecord(state(), "sakura", (a) => ({
      status: "running",
      last_error: a.last_error ?? null,
    }));
    expect(patch.offSidebarAgents?.sakura.status).toBe("running");
    expect(patch.workspace).toBeUndefined();
  });

  it("is a no-op for a record the store does not hold", () => {
    expect(patchAgentRecord(state(), "nowhere", { status: "running" })).toEqual({});
  });
});

describe("isAgentBusy", () => {
  it("reads the store's record ahead of a copy a component holds", () => {
    const s = state({ offSidebarAgents: { sakura: record("sakura", "running") } });
    // The Roadmap pane renders from its own stale list; the registry is fresher.
    expect(isAgentBusy(s, "sakura", "idle")).toBe(true);
  });

  it("falls back to the caller's status for a record the store does not carry", () => {
    expect(isAgentBusy(state(), "step-1", "running")).toBe(true);
    expect(isAgentBusy(state(), "step-1", "idle")).toBe(false);
  });

  it("counts this client's own send as busy", () => {
    expect(isAgentBusy(state({ sending: { fuji: true } }), "fuji")).toBe(true);
    expect(isAgentBusy(state(), "fuji")).toBe(false);
  });
});

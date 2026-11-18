import { describe, expect, it } from "vitest";
import type { AgentRecord, Workspace } from "../api";
import type { AppState } from "../store";
import { endedAgentIds, isAgentBusy, patchAgentRecord } from "./agentLookups";

const record = (id: string, status: AgentRecord["status"]): AgentRecord =>
  ({ id, status, provider: "claude", repos: [] }) as unknown as AgentRecord;

/** A store with one sidebar agent and one Roadmap chat the snapshot omits. */
const state = (over: Partial<AppState> = {}): AppState =>
  ({
    workspace: { agents: [record("fuji", "idle")], projects: [] },
    offSidebarAgents: { sakura: record("sakura", "idle") },
    sending: {},
    turnStartedAt: {},
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

  it("counts the turn the host has open, through a status that reads idle", () => {
    // A resting session's message: its process has come up and rests, and the
    // turn the message is owed is still open.
    expect(isAgentBusy(state({ turnStartedAt: { fuji: 100 } }), "fuji")).toBe(true);
    expect(isAgentBusy(state({ turnStartedAt: { sakura: 100 } }), "fuji")).toBe(false);
  });

  it("counts this client's own send as busy", () => {
    expect(isAgentBusy(state({ sending: { fuji: true } }), "fuji")).toBe(true);
    expect(isAgentBusy(state(), "fuji")).toBe(false);
  });
});

describe("endedAgentIds", () => {
  const archived = (id: string): AgentRecord =>
    ({ ...record(id, "idle"), archive: { archived_at: "2026-01-02T00:00:00Z" } }) as AgentRecord;
  const snapshot = (...agents: AgentRecord[]) => ({ agents, projects: [] }) as unknown as Workspace;

  it("names an agent a snapshot shows archived, whoever archived it", () => {
    const prev = snapshot(record("fuji", "idle"), record("etna", "idle"));
    expect(endedAgentIds(prev, snapshot(archived("fuji"), record("etna", "running")))).toEqual([
      "fuji",
    ]);
  });

  it("names an agent that is gone from the snapshot", () => {
    const prev = snapshot(record("fuji", "idle"), record("etna", "idle"));
    expect(endedAgentIds(prev, snapshot(record("etna", "idle")))).toEqual(["fuji"]);
  });

  it("leaves an agent that was archived already, and one restored", () => {
    // History reads an archived agent's transcript into the same maps: a
    // snapshot that still shows it archived must not clear it again.
    const prev = snapshot(archived("fuji"), archived("etna"));
    expect(endedAgentIds(prev, snapshot(archived("fuji"), record("etna", "spawning")))).toEqual([]);
  });

  it("ends an agent this window archived on the snapshot that confirms it", () => {
    // The click only hides the row (a placeholder with no `archived_at`).
    const hidden = { ...record("fuji", "idle"), archive: { archived_at: "" } } as AgentRecord;
    expect(endedAgentIds(snapshot(hidden), snapshot(hidden))).toEqual([]);
    expect(endedAgentIds(snapshot(hidden), snapshot(archived("fuji")))).toEqual(["fuji"]);
  });

  it("leaves a spawning agent a snapshot read before its spawn doesn't know yet", () => {
    const prev = snapshot(record("fuji", "spawning"));
    expect(endedAgentIds(prev, snapshot())).toEqual([]);
  });

  it("has nothing to say before the first snapshot", () => {
    expect(endedAgentIds(null, snapshot(archived("fuji")))).toEqual([]);
  });
});

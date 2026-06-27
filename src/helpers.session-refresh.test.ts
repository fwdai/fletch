import { describe, expect, it } from "vitest";
import type { AgentRecord, Workspace } from "./api";
import { needsSessionIdRefresh } from "./helpers";

function agent(overrides: Partial<AgentRecord> = {}): AgentRecord {
  return {
    id: "a1",
    name: "agent",
    provider: "antigravity",
    view: "custom",
    status: "idle",
    repos: [],
    ...overrides,
  } as AgentRecord;
}

function ws(agents: AgentRecord[]): Workspace {
  return { agents } as Workspace;
}

describe("needsSessionIdRefresh", () => {
  it("is true when the agent has no session id yet (first turn just landed)", () => {
    expect(needsSessionIdRefresh(ws([agent({ session_id: null })]), "a1")).toBe(true);
    expect(needsSessionIdRefresh(ws([agent({ session_id: undefined })]), "a1")).toBe(true);
    expect(needsSessionIdRefresh(ws([agent({ session_id: "" })]), "a1")).toBe(true);
  });

  it("is false once the agent already has a session id (avoids re-fetch churn)", () => {
    expect(needsSessionIdRefresh(ws([agent({ session_id: "215e97ad" })]), "a1")).toBe(false);
  });

  it("is false for an unknown agent or a null workspace", () => {
    expect(needsSessionIdRefresh(ws([agent({ session_id: null })]), "nope")).toBe(false);
    expect(needsSessionIdRefresh(null, "a1")).toBe(false);
  });
});

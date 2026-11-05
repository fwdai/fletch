import { describe, expect, it } from "vitest";
import type { AgentRecord } from "@/api";
import { compactBlocker, isCompacting, supportsCompaction } from "./compaction";
import type { AppState } from "./types";

const agent = (over: Partial<AgentRecord> = {}): AgentRecord =>
  ({ id: "a1", provider: "claude", status: "idle", view: "custom", ...over }) as AgentRecord;

const stateWith = (a: AgentRecord, over: Partial<AppState> = {}): AppState =>
  ({
    workspace: { agents: [a] },
    offSidebarAgents: {},
    sending: {},
    busyLabel: {},
    compactRequested: {},
    transcriptLoading: {},
    switchInFlight: {},
    features: { nativeView: false },
    ...over,
  }) as unknown as AppState;

describe("supportsCompaction", () => {
  it("is the providers with a /compact Fletch passes through", () => {
    expect(supportsCompaction("claude")).toBe(true);
    // codex runs under `exec`, where /compact would reach the model as text.
    for (const p of ["codex", "cursor", "antigravity", "opencode", "pi", undefined]) {
      expect(supportsCompaction(p)).toBe(false);
    }
  });
});

describe("isCompacting", () => {
  it("covers the hand-off, before the send has raised busy", () => {
    const s = stateWith(agent(), { compactRequested: { a1: true } });
    expect(isCompacting(s, "a1")).toBe(true);
  });

  it("follows a turn running /compact, however it was sent", () => {
    const s = stateWith(agent({ status: "running" }), { busyLabel: { a1: "Compacting" } });
    expect(isCompacting(s, "a1")).toBe(true);
  });

  it("ignores other turns, and a label outliving its turn", () => {
    expect(isCompacting(stateWith(agent({ status: "running" })), "a1")).toBe(false);
    expect(isCompacting(stateWith(agent(), { busyLabel: { a1: "Compacting" } }), "a1")).toBe(false);
  });
});

describe("compactBlocker", () => {
  it("lets an idle Claude agent in the chat view compact", () => {
    expect(compactBlocker(stateWith(agent()), "a1")).toBeNull();
  });

  it("refuses mid-turn rather than queueing", () => {
    expect(compactBlocker(stateWith(agent({ status: "running" })), "a1")).toBe(
      "Available when the agent is idle",
    );
  });

  it("refuses the native view, whose terminal takes /compact itself", () => {
    const s = stateWith(agent({ view: "native" }), {
      features: { nativeView: true } as AppState["features"],
    });
    expect(compactBlocker(s, "a1")).toBe("Type /compact in the terminal");
  });

  it("refuses an agent that isn't ready, and one whose provider can't", () => {
    expect(compactBlocker(stateWith(agent({ status: "stopped" })), "a1")).toBe(
      "Agent is not ready",
    );
    expect(compactBlocker(stateWith(agent({ provider: "codex" })), "a1")).toBe(
      "This agent can't compact",
    );
  });
});

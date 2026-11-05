import { describe, expect, it } from "vitest";
import type { RawEvent } from "@/adapters/types";
import type { AgentRecord } from "@/api";
import { compactBlocker, compactionSignal, supportsCompaction } from "./compaction";
import type { AppState } from "./types";

const agent = (over: Partial<AgentRecord> = {}): AgentRecord =>
  ({ id: "a1", provider: "claude", status: "idle", view: "custom", ...over }) as AgentRecord;

const stateWith = (a: AgentRecord, over: Partial<AppState> = {}): AppState =>
  ({
    workspace: { agents: [a] },
    offSidebarAgents: {},
    sending: {},
    busyLabel: {},
    compactions: {},
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

describe("compactionSignal", () => {
  const sys = (fields: Record<string, unknown>) => ({ type: "system", ...fields }) as RawEvent;

  it("reads Claude's compacting status as started, repeats included", () => {
    expect(compactionSignal(sys({ subtype: "status", status: "compacting" }))).toBe("started");
  });

  it("reads the closing status and the boundary as ended", () => {
    expect(
      compactionSignal(sys({ subtype: "status", status: null, compact_result: "success" })),
    ).toBe("ended");
    expect(compactionSignal(sys({ subtype: "compact_boundary" }))).toBe("ended");
  });

  it("reads a compaction error as failed", () => {
    expect(
      compactionSignal(sys({ subtype: "status", status: null, compact_error: "too short" })),
    ).toBe("failed");
  });

  it("ignores statuses that say nothing about a compaction", () => {
    // The summary request itself can report "requesting".
    expect(compactionSignal(sys({ subtype: "status", status: "requesting" }))).toBeNull();
    expect(
      compactionSignal(sys({ subtype: "status", status: null, permissionMode: "default" })),
    ).toBeNull();
    expect(compactionSignal({ type: "assistant" } as RawEvent)).toBeNull();
  });
});

describe("compactBlocker", () => {
  it("refuses while a compaction is in flight", () => {
    expect(compactBlocker(stateWith(agent(), { compactions: { a1: "running" } }), "a1")).toBe(
      "Compacting…",
    );
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

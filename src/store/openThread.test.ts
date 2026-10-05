// The sub-agent thread selection (`openThread`, store/ui) against the agent
// selection it rides on (store/workspace): opening a thread selects its
// agent; any selection change closes it; closing it leaves a reveal request
// for the card that launched it, so the chat lands back where the user left.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import type { AppState } from "./types";
import { createUiSlice } from "./ui";
import { createWorkspaceSlice } from "./workspace";

const makeStore = () =>
  create<AppState>()(
    (...a) => ({ ...createWorkspaceSlice(...a), ...createUiSlice(...a) }) as AppState,
  );

describe("openSubagentThread", () => {
  it("selects the agent and shows the thread, dropping a stale reveal", () => {
    const store = makeStore();
    store.setState({ selectedAgentId: "other", chatFocus: { agentId: "a1", toolUseId: "old" } });
    store.getState().openSubagentThread("a1", ["t1"]);
    expect(store.getState().selectedAgentId).toBe("a1");
    expect(store.getState().openThread).toEqual({ agentId: "a1", path: ["t1"] });
    expect(store.getState().chatFocus).toBeNull();
  });

  it("treats an empty path as the conversation itself", () => {
    const store = makeStore();
    store.getState().openSubagentThread("a1", ["t1", "t2"]);
    store.getState().openSubagentThread("a1", []);
    expect(store.getState().openThread).toBeNull();
    // Back from a nested thread reveals the top-level launch.
    expect(store.getState().chatFocus).toEqual({ agentId: "a1", toolUseId: "t1" });
  });
});

describe("closeSubagentThread", () => {
  it("closes and asks the chat to reveal the launching card", () => {
    const store = makeStore();
    store.getState().openSubagentThread("a1", ["t1"]);
    store.getState().closeSubagentThread();
    expect(store.getState().openThread).toBeNull();
    expect(store.getState().chatFocus).toEqual({ agentId: "a1", toolUseId: "t1" });
  });

  it("is a no-op with nothing open", () => {
    const store = makeStore();
    const before = store.getState();
    store.getState().closeSubagentThread();
    expect(store.getState()).toBe(before);
  });
});

describe("selection changes close the thread without a reveal", () => {
  it("selectAgent", () => {
    const store = makeStore();
    store.getState().openSubagentThread("a1", ["t1"]);
    store.getState().selectAgent("a2");
    expect(store.getState().openThread).toBeNull();
    expect(store.getState().chatFocus).toBeNull();
  });

  it("selectRun and selectRunStep", () => {
    const store = makeStore();
    store.getState().openSubagentThread("a1", ["t1"]);
    store.getState().selectRun("r1");
    expect(store.getState().openThread).toBeNull();
    store.getState().openSubagentThread("a1", ["t1"]);
    store.getState().selectRunStep("r1", "s1");
    expect(store.getState().openThread).toBeNull();
  });
});

// Regression tests for the discard/restore optimistic workspace edits. The bug
// these pin: when the guarded workspace refresh returns null (fetch failed) or
// is superseded, the store must still leave the workspace itself consistent —
// a discarded agent gone from the list (its side-state was cleared), and a
// restored agent un-archived and selectable — rather than relying on a later
// refresh to heal it. A null refresh is the exact worst case, so we force it.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { discardAgent, restoreAgent, getWorkspace } = vi.hoisted(() => ({
  discardAgent: vi.fn(),
  restoreAgent: vi.fn(),
  getWorkspace: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { discardAgent, restoreAgent, getWorkspace } }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn() }));

import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

// dropAgentEntries destructures every per-agent side map, so they must exist.
const EMPTY_MAPS = {
  managedLogs: {},
  transcriptLoading: {},
  transcriptLoaded: {},
  managedBusy: {},
  turnStartedAt: {},
  usage: {},
  gitStates: {},
  prStates: {},
  prChecks: {},
  prComments: {},
  gitShortstats: {},
  composerSeeds: {},
  composerDrafts: {},
  gitDelegations: {},
  unseenResults: {},
  rightPanelTabs: {},
};

// biome-ignore lint/suspicious/noExplicitAny: test fixtures use minimal shapes
const agent = (id: string, archive: any = null) => ({ id, archive }) as any;

// biome-ignore lint/suspicious/noExplicitAny: test builds a partial store
const makeStore = (agents: any[]) => {
  const store = create<AppState>()((...a) => ({ ...createWorkspaceSlice(...a) }) as AppState);
  store.setState({
    ...EMPTY_MAPS,
    // biome-ignore lint/suspicious/noExplicitAny: minimal workspace fixture
    workspace: { agents } as any,
    selectedAgentId: null,
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  } as any);
  return store;
};

describe("discard/restore stay consistent when the refresh returns null", () => {
  beforeEach(() => {
    discardAgent.mockReset().mockResolvedValue(undefined);
    restoreAgent.mockReset().mockResolvedValue(undefined);
    getWorkspace.mockReset().mockResolvedValue(null); // force the failed-refresh case
  });

  it("discard removes the row and clears the selection", async () => {
    const store = makeStore([agent("a"), agent("b")]);
    store.setState({ selectedAgentId: "a" });

    await store.getState().discard("a");

    expect(store.getState().workspace?.agents.map((x) => x.id)).toEqual(["b"]);
    expect(store.getState().selectedAgentId).toBeNull();
  });

  it("restore un-archives the row and selects it", async () => {
    const store = makeStore([agent("a", { archived_at: "t" }), agent("b")]);

    await store.getState().restore("a");

    const restored = store.getState().workspace?.agents.find((x) => x.id === "a");
    expect(restored?.archive).toBeNull();
    expect(store.getState().selectedAgentId).toBe("a");
    expect(store.getState().historyOpen).toBe(false);
  });
});

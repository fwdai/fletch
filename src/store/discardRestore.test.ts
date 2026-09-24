// Regression tests for the discard/restore/archive workspace edits. The bug
// these pin: when the guarded workspace refresh returns null (fetch failed) or
// is superseded, the store must still leave the workspace consistent — a
// discarded agent gone from the list (side-state cleared), a restored agent
// un-archived and selectable, and an archived agent whose destructive side-map
// cleanup only runs atomically with a snapshot that hides the row (so a
// transiently re-exposed row is never left emptied). A null refresh is the
// exact worst case, so we force it.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { discardAgent, restoreAgent, archiveAgent, getWorkspace } = vi.hoisted(() => ({
  discardAgent: vi.fn(),
  restoreAgent: vi.fn(),
  archiveAgent: vi.fn(),
  getWorkspace: vi.fn(),
}));
vi.mock("@/api", () => ({
  api: { discardAgent, restoreAgent, archiveAgent, getWorkspace },
}));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));

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
  gitBlocked: {},
  prStates: {},
  prChecks: {},
  prComments: {},
  gitShortstats: {},
  composerSeeds: {},
  composerDrafts: {},
  delegations: {},
  delegationNotices: {},
  autopilot: {},
  autopilotVerdicts: {},
  autopilotLog: {},
  unseenResults: {},
  rightPanelTabs: {},
  offSidebarAgents: {},
  backgroundTasks: {},
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

describe("discard/restore/archive stay consistent when the refresh returns null", () => {
  beforeEach(() => {
    discardAgent.mockReset().mockResolvedValue(undefined);
    restoreAgent.mockReset().mockResolvedValue(undefined);
    archiveAgent.mockReset().mockResolvedValue(undefined);
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

  it("archive keeps the agent's side state when the refresh fails", async () => {
    // A failed refresh leaves only the optimistic placeholder hiding the row —
    // which a stale refresh could transiently re-expose. The destructive side-map
    // cleanup must therefore NOT have run, so a re-exposed row is never emptied.
    const store = makeStore([agent("a"), agent("b")]);
    store.setState({ managedLogs: { a: [{ kind: "user_message", text: "hi" }] } });

    await store.getState().archive("a");

    expect(store.getState().managedLogs.a).toBeDefined();
    // The optimistic placeholder still hides the row from the live sidebar.
    const a = store.getState().workspace?.agents.find((x) => x.id === "a");
    expect(a?.archive).not.toBeNull();
  });

  it("archive drops the side state atomically with a snapshot that hides the row", async () => {
    getWorkspace.mockResolvedValue({
      agents: [
        { id: "a", archive: { archived_at: "t" } },
        { id: "b", archive: null },
      ],
      // biome-ignore lint/suspicious/noExplicitAny: minimal workspace fixture
    } as any);
    const store = makeStore([agent("a"), agent("b")]);
    store.setState({ managedLogs: { a: [{ kind: "user_message", text: "hi" }] } });

    await store.getState().archive("a");

    // The winning snapshot archives the row, so the cleanup runs with it.
    expect(store.getState().managedLogs.a).toBeUndefined();
    const a = store.getState().workspace?.agents.find((x) => x.id === "a");
    expect(a?.archive).not.toBeNull();
  });

  it("a snapshot fetched before the archive committed cannot put the row back", async () => {
    // The race: archive "a" is in flight; meanwhile a refresh issued by
    // something else (the previous click's `workspace:changed`) resolves with
    // a snapshot taken before the backend stamped `archived_at`, so it still
    // lists "a" as live. It is the newest generation, so the guard applies it.
    let finishArchive!: () => void;
    archiveAgent.mockReturnValue(
      new Promise<void>((r) => {
        finishArchive = r;
      }),
    );
    const store = makeStore([agent("a"), agent("b")]);

    const archiving = store.getState().archive("a");
    expect(store.getState().workspace?.agents.find((x) => x.id === "a")?.archive).not.toBeNull();

    getWorkspace.mockResolvedValueOnce({ agents: [agent("a"), agent("b")] });
    const { refreshWorkspace } = await import("./refreshWorkspace");
    await refreshWorkspace(store.setState);
    expect(
      store.getState().workspace?.agents.find((x) => x.id === "a")?.archive,
      "pre-commit snapshot must not re-expose the row",
    ).not.toBeNull();

    // The archive's own refresh then confirms it with real metadata.
    getWorkspace.mockResolvedValueOnce({
      agents: [{ id: "a", archive: { archived_at: "t" } }, agent("b")],
    });
    finishArchive();
    await archiving;
    const a = store.getState().workspace?.agents.find((x) => x.id === "a");
    expect(a?.archive).toEqual({ archived_at: "t" });
    expect(store.getState().managedLogs.a).toBeUndefined();
  });

  it("a refused archive withdraws the hide and puts the row back", async () => {
    archiveAgent.mockRejectedValue(new Error("agent must be idle"));
    const store = makeStore([agent("a"), agent("b")]);
    // The drafts slice (not part of this fixture) rests activeDraftId at null.
    store.setState({ selectedAgentId: "a", activeDraftId: null });

    await store.getState().archive("a");

    expect(store.getState().workspace?.agents.find((x) => x.id === "a")?.archive).toBeNull();
    expect(store.getState().selectedAgentId).toBe("a");
    expect(store.getState().lastError).toContain("must be idle");
    // Withdrawn: a later live snapshot is applied as-is.
    getWorkspace.mockResolvedValueOnce({ agents: [agent("a")] });
    const { refreshWorkspace } = await import("./refreshWorkspace");
    await refreshWorkspace(store.setState);
    expect(store.getState().workspace?.agents.find((x) => x.id === "a")?.archive).toBeNull();
  });

  it("discard keeps the row out of a snapshot fetched before the delete committed", async () => {
    const store = makeStore([agent("a"), agent("b")]);
    await store.getState().discard("a");

    getWorkspace.mockResolvedValueOnce({ agents: [agent("a"), agent("b")] });
    const { refreshWorkspace } = await import("./refreshWorkspace");
    await refreshWorkspace(store.setState);
    expect(store.getState().workspace?.agents.map((x) => x.id)).toEqual(["b"]);

    // Confirmed once a snapshot no longer lists it.
    getWorkspace.mockResolvedValueOnce({ agents: [agent("b")] });
    await refreshWorkspace(store.setState);
    expect(store.getState().workspace?.agents.map((x) => x.id)).toEqual(["b"]);
  });
});

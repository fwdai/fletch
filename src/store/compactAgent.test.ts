// `/compact` must open a turn of its own. A send waiting in line has not raised
// `sending` yet, so the agent still reads idle; these pin that compaction is
// refused behind one, and that a `/compact` whose agent turned busy before its
// place in line came is dropped rather than delivered as a mid-turn follow-up.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { sendUserMessage, grantAgentFolder } = vi.hoisted(() => ({
  sendUserMessage: vi.fn(),
  grantAgentFolder: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { sendUserMessage, grantAgentFolder } }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/data/slashCommands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/data/slashCommands")>()),
  discoverCommands: vi.fn(),
}));

import type { AgentRecord, FolderGrant } from "@/api";
import { createCompactionSlice } from "./compaction";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const agent = {
  id: "a1",
  status: "idle",
  provider: "claude",
  view: "custom",
  repos: [],
  folder_grants: [],
} as unknown as AgentRecord;

const makeStore = () => {
  const store = create<AppState>()(
    (...a) => ({ ...createWorkspaceSlice(...a), ...createCompactionSlice(...a) }) as AppState,
  );
  store.setState({
    workspace: { agents: [agent], projects: [] },
    offSidebarAgents: {},
    managedLogs: {},
    sending: {},
    busyLabel: {},
    transcriptLoading: {},
    switchInFlight: {},
    features: { nativeView: false },
  } as unknown as Partial<AppState>);
  return store;
};

const sentTexts = () => sendUserMessage.mock.calls.map((c) => c[2]);
const lib: FolderGrant = { path: "/Users/u/Projects/lib", writable: false };

beforeEach(() => {
  vi.clearAllMocks();
  sendUserMessage.mockResolvedValue(false);
});

describe("compactAgent", () => {
  it("sends /compact to an idle agent", async () => {
    const store = makeStore();
    await store.getState().compactAgent("a1");
    expect(sentTexts()).toEqual(["/compact"]);
  });

  it("refuses behind an earlier send still waiting in line", async () => {
    const grant = deferred<unknown>();
    grantAgentFolder.mockReturnValue(grant.promise);
    const store = makeStore();

    // Stalled on its folder grant, before it raises `sending`: the agent
    // still reads idle.
    const first = store.getState().sendUserMessage("a1", "read the lib", [], [lib]);
    await store.getState().compactAgent("a1");

    grant.resolve({
      granted: lib.path,
      folder_grants: [{ path: lib.path, writable: false }],
      applies_at_turn_end: false,
    });
    await first;
    expect(sentTexts()).toEqual(["read the lib"]);
  });
});

describe("sendUserMessage idleOnly", () => {
  it("drops the message, bubble and all, if the agent is busy when its turn comes", async () => {
    const grant = deferred<unknown>();
    grantAgentFolder.mockReturnValue(grant.promise);
    const store = makeStore();

    const first = store.getState().sendUserMessage("a1", "read the lib", [], [lib]);
    const compact = store.getState().sendUserMessage("a1", "/compact", [], [], { idleOnly: true });

    grant.resolve({
      granted: lib.path,
      folder_grants: [{ path: lib.path, writable: false }],
      applies_at_turn_end: false,
    });
    await first;
    await expect(compact).resolves.toBe(false);
    expect(sentTexts()).toEqual(["read the lib"]);
    const log = store.getState().managedLogs.a1 ?? [];
    expect(log.some((it) => "text" in it && it.text === "/compact")).toBe(false);
    expect(store.getState().lastError).toMatch(/wasn't sent/);
  });
});

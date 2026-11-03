// A message's place in its agent's line is fixed when it is sent; what it
// waits on before reaching the host (its folders resolving and being granted,
// the agent being spawned) happens inside that place. These pin that a later
// send never reaches the host first, and that a send made while a draft's
// agent is spawning follows the first prompt rather than spawning again.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { sendUserMessage, grantAgentFolder, spawnAgent, getWorkspace } = vi.hoisted(() => ({
  sendUserMessage: vi.fn(),
  grantAgentFolder: vi.fn(),
  spawnAgent: vi.fn(),
  getWorkspace: vi.fn(),
}));
vi.mock("@/api", () => ({
  api: { sendUserMessage, grantAgentFolder, spawnAgent, getWorkspace },
}));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/data/slashCommands", () => ({ discoverCommands: vi.fn() }));

import type { AgentRecord, FolderGrant } from "@/api";
import { createDraftsSlice } from "./drafts";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";
import { createWorkspaceTypesSlice } from "./workspaceTypes";

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Lets every queued microtask (and the store's awaits) run. */
const settle = () => new Promise((r) => setTimeout(r, 0));

const agent = {
  id: "a1",
  status: "idle",
  provider: "claude",
  repos: [],
  folder_grants: [],
} as unknown as AgentRecord;

const makeStore = () => {
  const store = create<AppState>()(
    (...a) =>
      ({
        ...createWorkspaceSlice(...a),
        ...createDraftsSlice(...a),
        ...createWorkspaceTypesSlice(...a),
      }) as AppState,
  );
  store.setState({
    workspace: { agents: [agent], projects: [] },
    drafts: [{ id: "d1", repoPath: "/repos/app", name: "chimborazo", provider: "claude" }],
    composerDrafts: {},
    customAgents: [],
    modelsByAgent: {},
    skills: [],
    managedLogs: {},
    sending: {},
  } as unknown as Partial<AppState>);
  return store;
};

/** The texts the host received, in order. */
const sentTexts = () => sendUserMessage.mock.calls.map((c) => c[2]);

const lib: FolderGrant = { path: "/Users/u/Projects/lib", writable: false };
const granted = (path: string) => ({
  granted: path,
  folder_grants: [{ path, writable: false }],
  applies_at_turn_end: false,
});

beforeEach(() => {
  vi.clearAllMocks();
  sendUserMessage.mockResolvedValue(false);
  getWorkspace.mockResolvedValue(null);
});

describe("sendUserMessage", () => {
  it("holds a later message until an earlier one's folder is granted and sent", async () => {
    const grant = deferred<ReturnType<typeof granted>>();
    grantAgentFolder.mockReturnValue(grant.promise);
    const store = makeStore();

    const first = store.getState().sendUserMessage("a1", "read the lib", [], [lib]);
    const second = store.getState().sendUserMessage("a1", "and then this");
    await settle();
    expect(sendUserMessage).not.toHaveBeenCalled();

    grant.resolve(granted(lib.path));
    await expect(first).resolves.toBe(true);
    await expect(second).resolves.toBe(true);
    expect(sentTexts()).toEqual(["read the lib", "and then this"]);
    // The folder rides the message by the path the host stored.
    expect(sendUserMessage.mock.calls[0][3]).toEqual([lib.path]);
  });

  it("holds a later message while an earlier one's path mentions resolve", async () => {
    const lookup = deferred<FolderGrant[]>();
    grantAgentFolder.mockResolvedValue(granted(lib.path));
    const store = makeStore();

    const first = store
      .getState()
      .sendUserMessage("a1", "look at @~/Projects/lib", [], lookup.promise);
    const second = store.getState().sendUserMessage("a1", "thanks");
    await settle();
    expect(sendUserMessage).not.toHaveBeenCalled();

    lookup.resolve([lib]);
    await Promise.all([first, second]);
    expect(sentTexts()).toEqual(["look at @~/Projects/lib", "thanks"]);
  });

  it("draws each message at once, and redraws it in place when it is delivered", async () => {
    const grant = deferred<ReturnType<typeof granted>>();
    grantAgentFolder.mockReturnValue(grant.promise);
    const store = makeStore();
    const log = () => store.getState().managedLogs.a1 ?? [];

    const first = store.getState().sendUserMessage("a1", "read the lib", [], [lib]);
    const second = store.getState().sendUserMessage("a1", "and then this");
    // Both up before anything has reached the host.
    expect(log().map((it) => [it.kind, "text" in it ? it.text : ""])).toEqual([
      ["queued_message", "read the lib"],
      ["queued_message", "and then this"],
    ]);

    grant.resolve(granted(lib.path));
    await Promise.all([first, second]);
    // The first opened the turn, in its own place and now with its folder;
    // the second, delivered behind it, is that turn's follow-up.
    expect(log()).toEqual([
      expect.objectContaining({
        kind: "user_message",
        text: "read the lib",
        attachments: [lib.path],
      }),
      expect.objectContaining({ kind: "queued_message", text: "and then this" }),
    ]);
  });

  it("takes a refused message's bubble down", async () => {
    grantAgentFolder.mockRejectedValue(new Error("Fletch won't share it"));
    const store = makeStore();

    await store.getState().sendUserMessage("a1", "read ~/.ssh", [], [lib]);

    expect(store.getState().managedLogs.a1 ?? []).toEqual([]);
  });

  it("sends nothing for a refused folder and lets the next message through", async () => {
    grantAgentFolder.mockRejectedValue(new Error("Fletch won't share it"));
    const store = makeStore();

    const first = store.getState().sendUserMessage("a1", "read ~/.ssh", [], [lib]);
    const second = store.getState().sendUserMessage("a1", "never mind");

    await expect(first).resolves.toBe(false);
    await expect(second).resolves.toBe(true);
    expect(sentTexts()).toEqual(["never mind"]);
    expect(store.getState().lastError).toContain("won't share");
  });
});

describe("spawnFromDraft", () => {
  it("sends a message made during the spawn to that agent, after the first prompt", async () => {
    const spawn = deferred<AgentRecord>();
    spawnAgent.mockReturnValue(spawn.promise);
    // The refresh before the first prompt goes out is held open, so a second
    // message not kept behind it would reach the host first.
    const refresh = deferred<null>();
    getWorkspace.mockReturnValue(refresh.promise);
    const store = makeStore();

    const first = store.getState().spawnFromDraft("d1", "set up the project", "claude");
    const second = store.getState().spawnFromDraft("d1", "also add a README", "claude");
    await settle();

    spawn.resolve({ ...agent, id: "a2" } as AgentRecord);
    await settle();
    expect(sendUserMessage).not.toHaveBeenCalled();
    refresh.resolve(null);
    await expect(first).resolves.toBe(true);
    await expect(second).resolves.toBe(true);
    expect(spawnAgent).toHaveBeenCalledTimes(1);
    expect(sendUserMessage.mock.calls.map((c) => [c[0], c[2]])).toEqual([
      ["a2", "set up the project"],
      ["a2", "also add a README"],
    ]);
  });

  it("gives a message made during a failed spawn back", async () => {
    spawnAgent.mockRejectedValue(new Error("no such repo"));
    const store = makeStore();

    const first = store.getState().spawnFromDraft("d1", "set up the project", "claude");
    const second = store.getState().spawnFromDraft("d1", "also add a README", "claude");

    await expect(first).resolves.toBe(false);
    await expect(second).resolves.toBe(false);
    expect(spawnAgent).toHaveBeenCalledTimes(1);
    expect(sendUserMessage).not.toHaveBeenCalled();
  });
});

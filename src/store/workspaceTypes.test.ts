// The composer's workspace-type answer: one read at a time, keyed by its
// inputs, refreshed on the events that move the default, and the spawn sending
// exactly what the chip declares from it.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import type { WorkspaceKind, WorkspaceTypeOption } from "@/api";
import { workspaceTypePickDropped } from "@/components/Composer/workspaceType";

const { availableWorkspaceTypes, spawnAgent, sendUserMessage, getWorkspace } = vi.hoisted(() => ({
  availableWorkspaceTypes: vi.fn(),
  spawnAgent: vi.fn(),
  sendUserMessage: vi.fn(),
  getWorkspace: vi.fn(),
}));
vi.mock("@/api", () => ({
  api: { availableWorkspaceTypes, spawnAgent, sendUserMessage, getWorkspace },
}));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/data/slashCommands", () => ({ discoverCommands: vi.fn() }));

import { createDraftsSlice } from "./drafts";
import { createSandboxSlice } from "./sandbox";
import type { AppState } from "./types";
import {
  applyBoxSetupEvent,
  createWorkspaceTypesSlice,
  workspaceTypeOptionsFor,
  workspaceTypesFailedFor,
} from "./workspaceTypes";

const option = (kind: WorkspaceKind, over: Partial<WorkspaceTypeOption> = {}) => ({
  kind,
  available: true,
  reason: null,
  default: false,
  ...over,
});
const HOST_DEFAULT = [option("host", { default: true }), option("box")];
const BOX_DEFAULT = [option("host"), option("box", { default: true })];
const PINNED = [option("host"), option("box")];

/** A read the test answers by hand, in any order. */
function deferred() {
  let resolve!: (v: WorkspaceTypeOption[]) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<WorkspaceTypeOption[]>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const makeStore = () => {
  const store = create<AppState>()(
    (...a) =>
      ({
        ...createSandboxSlice(...a),
        ...createWorkspaceTypesSlice(...a),
        ...createDraftsSlice(...a),
      }) as AppState,
  );
  store.setState({
    drafts: [{ id: "d1", repoPath: "/r", name: "chimborazo", provider: "claude", base: "main" }],
    composerDrafts: {},
    customAgents: [],
    modelsByAgent: {},
    skills: [],
    managedLogs: {},
    sending: {},
    admin: true,
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  } as any);
  return store;
};

type Store = ReturnType<typeof makeStore>;
const optionsFor = (s: Store, provider = "claude") =>
  workspaceTypeOptionsFor(s.getState(), "/r", provider);
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  vi.clearAllMocks();
  spawnAgent.mockResolvedValue({ id: "a1" });
  sendUserMessage.mockResolvedValue(false);
  getWorkspace.mockResolvedValue(null);
});

describe("the workspace-type answer", () => {
  it("lands only the latest read", async () => {
    const s = makeStore();
    const first = deferred();
    const second = deferred();
    availableWorkspaceTypes.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const a = s.getState().loadWorkspaceTypes("/r", "claude");
    // The chip opened: a second read for the same inputs.
    const b = s.getState().loadWorkspaceTypes("/r", "claude");
    second.resolve(BOX_DEFAULT);
    await b;
    first.resolve(HOST_DEFAULT);
    await a;
    expect(optionsFor(s)).toBe(BOX_DEFAULT);
  });

  it("never shows an answer for other inputs", async () => {
    const s = makeStore();
    availableWorkspaceTypes.mockResolvedValueOnce(BOX_DEFAULT);
    await s.getState().loadWorkspaceTypes("/r", "claude");
    expect(optionsFor(s)).toBe(BOX_DEFAULT);

    // The provider changes: the previous answer goes at once, so the chip is
    // back to loading and a send carries nothing until the new one lands.
    const pending = deferred();
    availableWorkspaceTypes.mockReturnValueOnce(pending.promise);
    const load = s.getState().loadWorkspaceTypes("/r", "codex");
    expect(optionsFor(s, "codex")).toBeNull();
    expect(optionsFor(s, "claude")).toBeNull();
    expect(s.getState().workspaceTypes.answer).toBeNull();
    pending.resolve(HOST_DEFAULT);
    await load;
    expect(optionsFor(s, "codex")).toBe(HOST_DEFAULT);
  });

  it("drops a response for inputs the picker no longer has", async () => {
    const s = makeStore();
    const old = deferred();
    availableWorkspaceTypes.mockReturnValueOnce(old.promise);
    const a = s.getState().loadWorkspaceTypes("/r", "claude");
    const fresh = deferred();
    availableWorkspaceTypes.mockReturnValueOnce(fresh.promise);
    const b = s.getState().loadWorkspaceTypes("/r", "codex");
    old.resolve(BOX_DEFAULT);
    await a;
    expect(s.getState().workspaceTypes.answer).toBeNull();
    fresh.resolve(HOST_DEFAULT);
    await b;
    expect(optionsFor(s, "codex")).toBe(HOST_DEFAULT);
  });

  it("drops the answer when a setting that moves the default changes", async () => {
    const s = makeStore();
    availableWorkspaceTypes.mockResolvedValueOnce(HOST_DEFAULT);
    await s.getState().loadWorkspaceTypes("/r", "claude");
    s.setState({ sandboxEngine: "docker" });
    expect(optionsFor(s)).toBeNull();
    // And a read started before the change does not land after it.
    s.setState({ sandboxEngine: "auto" });
    const inFlight = deferred();
    availableWorkspaceTypes.mockReturnValueOnce(inFlight.promise);
    const load = s.getState().loadWorkspaceTypes("/r", "claude");
    s.setState({ boxesEnabled: !s.getState().boxesEnabled });
    inFlight.resolve(BOX_DEFAULT);
    await load;
    expect(optionsFor(s)).toBeNull();
  });

  it("keeps the answer across a failed read, and notes a never-answered failure", async () => {
    const s = makeStore();
    availableWorkspaceTypes.mockRejectedValueOnce(new Error("busy"));
    await s.getState().loadWorkspaceTypes("/r", "claude");
    expect(optionsFor(s)).toBeNull();
    expect(workspaceTypesFailedFor(s.getState(), "/r", "claude")).toBe(true);
    // Not for other inputs.
    expect(workspaceTypesFailedFor(s.getState(), "/r", "codex")).toBe(false);

    availableWorkspaceTypes.mockResolvedValueOnce(HOST_DEFAULT);
    await s.getState().loadWorkspaceTypes("/r", "claude");
    expect(workspaceTypesFailedFor(s.getState(), "/r", "claude")).toBe(false);
    availableWorkspaceTypes.mockRejectedValueOnce(new Error("busy"));
    await s.getState().loadWorkspaceTypes("/r", "claude");
    expect(optionsFor(s)).toBe(HOST_DEFAULT);
  });

  it("forgets the picker that left the screen", async () => {
    const s = makeStore();
    const pending = deferred();
    availableWorkspaceTypes.mockReturnValueOnce(pending.promise);
    const load = s.getState().loadWorkspaceTypes("/r", "claude");
    s.getState().releaseWorkspaceTypes();
    pending.resolve(HOST_DEFAULT);
    await load;
    expect(s.getState().workspaceTypes.answer).toBeNull();
    // Nothing on screen: an event reads nothing.
    s.getState().reloadWorkspaceTypes();
    expect(availableWorkspaceTypes).toHaveBeenCalledTimes(1);
  });
});

describe("box setup events", () => {
  it("re-read the picker on screen when a phase moves, not on a progress tick", async () => {
    const s = makeStore();
    availableWorkspaceTypes.mockResolvedValue(HOST_DEFAULT);
    await s.getState().loadWorkspaceTypes("/r", "claude");
    expect(availableWorkspaceTypes).toHaveBeenCalledTimes(1);

    const image = { phase: "downloading" as const, progress: 0.1, pinned_version: "1", url: "u" };
    applyBoxSetupEvent(s.getState, { image });
    expect(availableWorkspaceTypes).toHaveBeenCalledTimes(2);
    applyBoxSetupEvent(s.getState, { image: { ...image, progress: 0.5 } });
    expect(availableWorkspaceTypes).toHaveBeenCalledTimes(2);
    expect(s.getState().boxImage?.progress).toBe(0.5);

    // The image installed: the default moves to the VM.
    availableWorkspaceTypes.mockResolvedValue(BOX_DEFAULT);
    applyBoxSetupEvent(s.getState, { image: { ...image, phase: "installed" } });
    expect(availableWorkspaceTypes).toHaveBeenLastCalledWith("/r", "claude");
    await flush();
    expect(optionsFor(s)).toBe(BOX_DEFAULT);

    applyBoxSetupEvent(s.getState, {
      tools: { provider: "claude", version: "1", phase: "ready" },
    });
    applyBoxSetupEvent(s.getState, { state: { agent_id: "a9", phase: "running" } });
    expect(availableWorkspaceTypes).toHaveBeenCalledTimes(5);
  });
});

describe("spawning from a draft", () => {
  const sent = () => spawnAgent.mock.calls[0].at(-1);

  it("sends the default the chip shows for an untouched draft", async () => {
    const s = makeStore();
    availableWorkspaceTypes.mockResolvedValueOnce(BOX_DEFAULT);
    await s.getState().loadWorkspaceTypes("/r", "claude");
    await s.getState().spawnFromDraft("d1", "go", "claude");
    expect(sent()).toBe("box");
  });

  it("sends the explicit pick the chip shows", async () => {
    const s = makeStore();
    availableWorkspaceTypes.mockResolvedValueOnce(BOX_DEFAULT);
    await s.getState().loadWorkspaceTypes("/r", "claude");
    s.getState().updateDraft("d1", { workspaceType: "host" });
    await s.getState().spawnFromDraft("d1", "go", "claude");
    expect(sent()).toBe("host");
  });

  it("sends nothing while the answer for its inputs is loading", async () => {
    const s = makeStore();
    availableWorkspaceTypes.mockReturnValueOnce(deferred().promise);
    void s.getState().loadWorkspaceTypes("/r", "claude");
    s.getState().updateDraft("d1", { workspaceType: "box" });
    await s.getState().spawnFromDraft("d1", "go", "claude");
    expect(sent()).toBeUndefined();
  });

  it("sends nothing when the answer is for other inputs", async () => {
    const s = makeStore();
    availableWorkspaceTypes.mockResolvedValueOnce(BOX_DEFAULT);
    await s.getState().loadWorkspaceTypes("/r", "claude");
    // The send is for another provider than the answer was read for.
    await s.getState().spawnFromDraft("d1", "go", "codex");
    expect(sent()).toBeUndefined();
  });

  it("drops the pick and sends nothing when the host pins a container engine", async () => {
    const s = makeStore();
    availableWorkspaceTypes.mockResolvedValueOnce(PINNED);
    await s.getState().loadWorkspaceTypes("/r", "claude");
    // The picker clears the pick on this answer, and only on it.
    expect(workspaceTypePickDropped(optionsFor(s))).toBe(true);
    s.getState().updateDraft("d1", { workspaceType: "box" });
    await s.getState().spawnFromDraft("d1", "go", "claude");
    expect(sent()).toBeUndefined();
  });
});

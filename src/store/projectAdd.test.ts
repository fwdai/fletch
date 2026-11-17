// The hand-off after a project is added: the user lands in a draft on the new
// project — seeded with the suggested first task for their first project,
// empty after that — instead of back where they were with a new sidebar row.
// Pins which project the draft targets, the seed rule, the opt-out, and that a
// draft that cannot open never reads as a failed add.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { api, track } = vi.hoisted(() => ({
  api: {
    addWorkspaceRepo: vi.fn(),
    cloneRepo: vi.fn(),
    createRepo: vi.fn(),
    allocateDraftName: vi.fn(),
    repoDefaultBranch: vi.fn(),
    listRepoBranches: vi.fn(),
  },
  track: vi.fn(),
}));
vi.mock("@/api", () => ({ api }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/util/track", () => ({ track }));
// The gate reads the whole store module, which would pull the real slices in
// through a cycle; every add here runs on the local environment, where it is
// open.
vi.mock("./capabilities", () => ({ hostGateReason: () => null }));

import type { Workspace } from "@/api";
import { activeEngine } from "@/api/engine";
import { FIRST_TASK_PROMPT } from "@/data/firstTask";
import { createComposerSlice } from "./composer";
import { createDraftsSlice } from "./drafts";
import { type EnvironmentEntry, LOCAL_ENVIRONMENT_ID, setEnvironmentsSource } from "./environments";
import { createReposSlice } from "./repos";
import type { AppState } from "./types";
import { createWorkspaceTypesSlice } from "./workspaceTypes";

const workspaceOf = (paths: string[]): Workspace => ({
  repos: paths,
  projects: paths.map((path, i) => ({ path, name: `p${i}`, project_id: `id-${i}`, label: null })),
  agents: [],
});

const makeStore = (workspace: Workspace | null) => {
  const closeProjectScreen = vi.fn();
  const setLastError = vi.fn();
  const requestFocusJoin = vi.fn();
  const store = create<AppState>()(
    (set, get) =>
      ({
        ...createReposSlice(set, get, activeEngine),
        ...createDraftsSlice(set, get, activeEngine),
        ...createWorkspaceTypesSlice(set, get, activeEngine),
        ...createComposerSlice(set, get, activeEngine),
        closeProjectScreen,
        setLastError,
        requestFocusJoin,
      }) as unknown as AppState,
  );
  // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  store.setState({ workspace, customAgents: [], modelsByAgent: {} } as any);
  return { store, closeProjectScreen, setLastError, requestFocusJoin };
};

/** Let the fire-and-forget hand-off settle. */
const settled = () => new Promise((r) => setTimeout(r, 0));

const draftOf = (store: ReturnType<typeof makeStore>["store"]) => {
  const s = store.getState();
  const draft = s.drafts.find((d) => d.id === s.activeDraftId);
  return draft ? { ...draft, text: s.composerDrafts[draft.id] } : null;
};

/** Put `entry` (or the local environment) on screen, as a switch does. */
const activeIs = (entry?: EnvironmentEntry) =>
  setEnvironmentsSource(() => ({
    activeEnvironmentId: entry?.id ?? LOCAL_ENVIRONMENT_ID,
    environments: entry ? { [entry.id]: entry } : {},
  }));

const remote: EnvironmentEntry = {
  id: "host-1",
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
  protocol: { version: 2, ops: [], events: [], features: [] },
};

beforeEach(() => {
  vi.clearAllMocks();
  activeIs();
  api.allocateDraftName.mockResolvedValue("everest");
  api.repoDefaultBranch.mockResolvedValue("main");
  api.listRepoBranches.mockResolvedValue(["main"]);
});

describe("a project added while the sidebar is focused", () => {
  it("asks to join the focus, by the key the sidebar groups it under", async () => {
    const { store, requestFocusJoin } = makeStore(workspaceOf(["/repos/app", "/repos/other"]));
    api.cloneRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/other", "/repos/new"]));

    await store.getState().cloneRepo("x/new", "/repos");

    expect(requestFocusJoin).toHaveBeenCalledWith("id-2");
  });

  it("asks for nothing when the add pinned nothing new", async () => {
    const { store, requestFocusJoin } = makeStore(workspaceOf(["/repos/app"]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));

    await store.getState().addWorkspaceRepo("/repos/app");

    expect(requestFocusJoin).not.toHaveBeenCalled();
  });
});

describe("the draft after a project is added", () => {
  it("opens on the first project with the suggested first task", async () => {
    const { store, closeProjectScreen } = makeStore(workspaceOf([]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));

    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();

    expect(draftOf(store)).toMatchObject({
      repoPath: "/repos/app",
      text: FIRST_TASK_PROMPT,
      afterProjectAdd: { seeded: true },
    });
    expect(closeProjectScreen).toHaveBeenCalled();
    expect(track).toHaveBeenCalledWith("project_add_draft_opened", { seeded: true });
  });

  it("opens empty on a later project", async () => {
    const { store } = makeStore(workspaceOf(["/repos/app"]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/other"]));

    await store.getState().addWorkspaceRepo("/repos/other");
    await settled();

    expect(draftOf(store)).toMatchObject({
      repoPath: "/repos/other",
      afterProjectAdd: { seeded: false },
    });
    expect(draftOf(store)?.text).toBeUndefined();
    expect(track).toHaveBeenCalledWith("project_add_draft_opened", { seeded: false });
  });

  it("targets the project a clone just added, not one already pinned", async () => {
    const { store } = makeStore(workspaceOf(["/repos/app"]));
    api.cloneRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/cloned"]));

    await store.getState().cloneRepo("owner/cloned", "/repos");
    await settled();

    expect(draftOf(store)?.repoPath).toBe("/repos/cloned");
  });

  it("targets the project a create just added", async () => {
    const { store } = makeStore(workspaceOf([]));
    api.createRepo.mockResolvedValue(workspaceOf(["/repos/fresh"]));

    await store.getState().createRepo("fresh", "/repos", true);
    await settled();

    expect(draftOf(store)).toMatchObject({ repoPath: "/repos/fresh", text: FIRST_TASK_PROMPT });
  });

  it("can be declined by a caller whose screen the user should stay on", async () => {
    const { store, closeProjectScreen } = makeStore(workspaceOf([]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));

    await store.getState().addWorkspaceRepo("/repos/app", { draft: false });
    await settled();

    expect(store.getState().drafts).toEqual([]);
    expect(closeProjectScreen).not.toHaveBeenCalled();
    expect(track).toHaveBeenCalledTimes(1);
    expect(track).toHaveBeenCalledWith("project_added", { method: "existing", first: true });
  });

  it("does not open when the add failed", async () => {
    const { store } = makeStore(workspaceOf([]));
    api.addWorkspaceRepo.mockRejectedValue(new Error("not a git repository"));

    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();

    expect(store.getState().drafts).toEqual([]);
    expect(store.getState().lastError).toBe("Error: not a git repository");
  });

  it("opens on the host it was asked of when another host is put on screen", async () => {
    // Drafts are the host's own. The add has returned, but the draft's reads
    // are still in flight when the user puts another host on screen: they
    // carry on against this host, and the draft opens in this host's state.
    const { store } = makeStore(workspaceOf([]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));
    api.allocateDraftName.mockImplementation(async () => {
      activeIs(remote);
      return "everest";
    });

    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();

    expect(api.repoDefaultBranch).toHaveBeenCalledWith("/repos/app");
    expect(draftOf(store)).toMatchObject({ repoPath: "/repos/app", name: "everest" });
  });

  it("puts its own host on screen when another host was", async () => {
    const { store } = makeStore(workspaceOf(["/repos/app"]));
    // A paired host's workspace is in the main pane while a project is added
    // to this Mac from the sidebar.
    store.setState({ activeEnvironmentId: remote.id });
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/new"]));

    await store.getState().addWorkspaceRepo("/repos/new");
    await settled();

    expect(draftOf(store)?.repoPath).toBe("/repos/new");
    expect(store.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
  });

  it("leaves the screen alone when the user put another host on it meanwhile", async () => {
    const { store } = makeStore(workspaceOf(["/repos/app"]));
    store.setState({ activeEnvironmentId: LOCAL_ENVIRONMENT_ID });
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/new"]));
    api.allocateDraftName.mockImplementation(async () => {
      store.setState({ activeEnvironmentId: remote.id });
      return "everest";
    });

    await store.getState().addWorkspaceRepo("/repos/new");
    await settled();

    // Open in its own host's state, for when the user comes back to it.
    expect(draftOf(store)?.repoPath).toBe("/repos/new");
    expect(store.getState().activeEnvironmentId).toBe(remote.id);
  });

  it("yields to a full-screen page the user opened while it was opening", async () => {
    // Settings sits over the center pane; a draft opened beneath it would be
    // waiting there when the page closes. Every overlay and takeover counts,
    // not just the center pane's own selection.
    const { store } = makeStore(workspaceOf([]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));
    api.allocateDraftName.mockImplementation(async () => {
      // biome-ignore lint/suspicious/noExplicitAny: partial store seed
      store.setState({ settingsScreenOpen: true } as any);
      return "everest";
    });

    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();

    expect(store.getState().drafts).toEqual([]);
    expect(store.getState().activeDraftId).toBeNull();
    expect(track).not.toHaveBeenCalledWith("project_add_draft_opened", expect.anything());
  });

  it("yields to wherever the user went while it was opening", async () => {
    // The hand-off is automatic, so a selection the user made meanwhile wins:
    // the draft is not opened over it, and a page they opened is not closed.
    const { store, closeProjectScreen } = makeStore(workspaceOf([]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));
    api.allocateDraftName.mockImplementation(async () => {
      store.setState({
        selectedAgentId: "fuji",
        projectScreen: { repoPath: "/repos/app", view: "roadmap" },
      });
      return "everest";
    });

    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();

    expect(store.getState().drafts).toEqual([]);
    expect(store.getState().selectedAgentId).toBe("fuji");
    expect(closeProjectScreen).not.toHaveBeenCalled();
    expect(track).not.toHaveBeenCalledWith("project_add_draft_opened", expect.anything());
  });

  it("does nothing when the folder was already pinned", async () => {
    // The backend answers an already-pinned path with the unchanged workspace:
    // nothing was added, so no activation event and no unasked-for draft.
    const { store } = makeStore(workspaceOf(["/repos/app"]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));

    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();

    expect(store.getState().drafts).toEqual([]);
    expect(track).not.toHaveBeenCalled();
  });

  it("reports a draft that could not open without undoing the add", async () => {
    const { store, setLastError } = makeStore(workspaceOf([]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));
    api.allocateDraftName.mockRejectedValue(new Error("host went away"));

    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();

    expect(store.getState().workspace?.repos).toEqual(["/repos/app"]);
    expect(track).toHaveBeenCalledWith("project_added", { method: "existing", first: true });
    expect(store.getState().drafts).toEqual([]);
    expect(setLastError).toHaveBeenCalledWith(expect.stringContaining("host went away"));
    expect(track).not.toHaveBeenCalledWith("project_add_draft_opened", expect.anything());
  });
});

describe("the draft's spawn event", () => {
  it("reports whether the suggested task was kept", async () => {
    const { store } = makeStore(workspaceOf([]));
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
    store.setState({ skills: [], mcpServers: [], managedLogs: {}, sending: {} } as any);
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));
    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();
    const draft = draftOf(store);
    if (!draft) throw new Error("no draft opened");

    // Only the event matters here; the spawn itself is pinned elsewhere.
    Object.assign(api, {
      spawnAgent: vi.fn().mockResolvedValue({ id: "everest", repos: [], status: "spawning" }),
      getWorkspace: vi.fn().mockResolvedValue(workspaceOf(["/repos/app"])),
      sendUserMessage: vi.fn().mockResolvedValue(false),
    });
    // Whitespace around the seed is still the seed; a different task is not.
    await store.getState().spawnFromDraft(draft.id, `  ${FIRST_TASK_PROMPT}\n`, "claude");

    expect(track).toHaveBeenCalledWith("project_add_draft_spawned", {
      seeded: true,
      kept_seed: true,
    });
  });
});

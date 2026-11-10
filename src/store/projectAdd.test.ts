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
    wfListRuns: vi.fn(),
  },
  track: vi.fn(),
}));
vi.mock("@/api", () => ({ api }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/util/track", () => ({ track }));
// The gate reads the whole store module, which would pull the real slices in
// through a cycle; every add here runs on the local environment, where it is
// open.
vi.mock("./capabilities", () => ({ activeGateReason: () => null }));

import type { Workspace } from "@/api";
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
  const setProjectFocus = vi.fn();
  const store = create<AppState>()(
    (...a) =>
      ({
        ...createReposSlice(...a),
        ...createDraftsSlice(...a),
        ...createWorkspaceTypesSlice(...a),
        ...createComposerSlice(...a),
        closeProjectScreen,
        setLastError,
        setProjectFocus,
        projectFocus: [],
      }) as unknown as AppState,
  );
  // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  store.setState({ workspace, customAgents: [], modelsByAgent: {} } as any);
  return { store, closeProjectScreen, setLastError, setProjectFocus };
};

/** Let the fire-and-forget hand-off settle. */
const settled = () => new Promise((r) => setTimeout(r, 0));

const draftOf = (store: ReturnType<typeof makeStore>["store"]) => {
  const s = store.getState();
  const draft = s.drafts.find((d) => d.id === s.activeDraftId);
  return draft ? { ...draft, text: s.composerDrafts[draft.id] } : null;
};

/** Make `entry` (or the local environment) the active one, as a switch does. */
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
  api.wfListRuns.mockResolvedValue([]);
});

describe("a project added while the sidebar is focused", () => {
  it("joins the focus, so it is listed", async () => {
    const { store, setProjectFocus } = makeStore(workspaceOf(["/repos/app", "/repos/other"]));
    store.setState({ projectFocus: ["id-0"] });
    api.cloneRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/other", "/repos/new"]));

    await store.getState().cloneRepo("x/new", "/repos");
    await settled();

    expect(setProjectFocus).toHaveBeenCalledWith(["id-0", "id-2"]);
  });

  it("joins a focus on a project only a workflow run keeps listed", async () => {
    const { store, setProjectFocus } = makeStore(workspaceOf(["/repos/app"]));
    store.setState({ projectFocus: ["/repos/gone"] });
    api.wfListRuns.mockResolvedValue([
      { id: "r1", project_id: "id-gone", repo_path: "/repos/gone" },
    ]);
    api.cloneRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/new"]));

    await store.getState().cloneRepo("x/new", "/repos");
    await settled();

    expect(setProjectFocus).toHaveBeenCalledWith(["id-1", "/repos/gone"]);
  });

  it("goes back to all projects when the focus then names every one", async () => {
    // Focused on one of two projects, then the other was removed: the saved
    // focus still names the survivor.
    const { store, setProjectFocus } = makeStore(workspaceOf(["/repos/app"]));
    store.setState({ projectFocus: ["id-0"] });
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/other"]));

    await store.getState().addWorkspaceRepo("/repos/other");
    await settled();

    expect(setProjectFocus).toHaveBeenCalledWith([]);
  });

  it("drops the join when the focus was changed while the runs were on their way", async () => {
    const { store, setProjectFocus } = makeStore(workspaceOf(["/repos/app", "/repos/other"]));
    store.setState({ projectFocus: ["id-0"] });
    let answer: (runs: never[]) => void = () => {};
    api.wfListRuns.mockReturnValue(new Promise((resolve) => (answer = resolve)));
    api.cloneRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/other", "/repos/new"]));

    await store.getState().cloneRepo("x/new", "/repos");
    // The user picks another project before the host answers.
    store.setState({ projectFocus: ["id-1"] });
    answer([]);
    await settled();

    expect(setProjectFocus).not.toHaveBeenCalled();
  });

  it("drops the join when the environment was switched meanwhile", async () => {
    const { store, setProjectFocus } = makeStore(workspaceOf(["/repos/app", "/repos/other"]));
    store.setState({ projectFocus: ["id-0"], activeEnvironmentId: LOCAL_ENVIRONMENT_ID });
    let answer: (runs: never[]) => void = () => {};
    api.wfListRuns.mockReturnValue(new Promise((resolve) => (answer = resolve)));
    api.cloneRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/other", "/repos/new"]));

    await store.getState().cloneRepo("x/new", "/repos");
    store.setState({ activeEnvironmentId: "host-1" });
    answer([]);
    await settled();

    expect(setProjectFocus).not.toHaveBeenCalled();
  });

  it("keeps the focus as it stands when the runs cannot be read", async () => {
    // One pinned project in the store; the sidebar may list a run-only one
    // beside it, which a failed read cannot rule out. Tidying against the
    // store alone would read this focus as every project and clear it.
    const { store, setProjectFocus } = makeStore(workspaceOf(["/repos/app"]));
    store.setState({ projectFocus: ["id-0"] });
    api.wfListRuns.mockRejectedValue(new Error("host unreachable"));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/other"]));

    await store.getState().addWorkspaceRepo("/repos/other");
    await settled();

    expect(setProjectFocus).toHaveBeenCalledWith(["id-0", "id-1"]);
  });

  it("leaves all projects showing when no focus is on", async () => {
    const { store, setProjectFocus } = makeStore(workspaceOf(["/repos/app"]));
    // A saved key of a project that is gone is not a focus.
    store.setState({ projectFocus: ["id-gone"] });
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app", "/repos/other"]));

    await store.getState().addWorkspaceRepo("/repos/other");
    await settled();

    expect(setProjectFocus).not.toHaveBeenCalled();
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

  it("is dropped when the user switches environments while it opens", async () => {
    // Drafts are stashed per environment. The add has returned, but the draft's
    // own reads are still in flight when the switch lands — the draft must not
    // be written into, or selected in, the environment the user moved to, and
    // the next read (about this repo) must not go to the other host either.
    const { store, closeProjectScreen } = makeStore(workspaceOf([]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));
    api.allocateDraftName.mockImplementation(async () => {
      activeIs(remote);
      return "everest";
    });

    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();

    expect(api.repoDefaultBranch).not.toHaveBeenCalled();
    expect(api.listRepoBranches).not.toHaveBeenCalled();
    expect(store.getState().drafts).toEqual([]);
    expect(store.getState().activeDraftId).toBeNull();
    expect(closeProjectScreen).not.toHaveBeenCalled();
    expect(track).not.toHaveBeenCalledWith("project_add_draft_opened", expect.anything());
  });

  it("is dropped when the switch lands during the last read too", async () => {
    const { store } = makeStore(workspaceOf([]));
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));
    api.repoDefaultBranch.mockImplementation(async () => {
      activeIs(remote);
      return "main";
    });

    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();

    expect(store.getState().drafts).toEqual([]);
    expect(track).not.toHaveBeenCalledWith("project_add_draft_opened", expect.anything());
  });

  it("does not send the base fallback read to a host the user switched to", async () => {
    // A remembered base branch costs two reads. The switch lands during the
    // first, and the branch turns out to be gone — the fallback read is about
    // the old environment's repo and must not go to the new host.
    const { store } = makeStore(workspaceOf([]));
    store.getState().rememberDraftBase("/repos/app", "develop");
    api.addWorkspaceRepo.mockResolvedValue(workspaceOf(["/repos/app"]));
    api.listRepoBranches.mockImplementation(async () => {
      activeIs(remote);
      return ["main"];
    });

    await store.getState().addWorkspaceRepo("/repos/app");
    await settled();

    expect(api.repoDefaultBranch).not.toHaveBeenCalled();
    expect(store.getState().drafts).toEqual([]);
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

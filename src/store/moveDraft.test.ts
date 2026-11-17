// @vitest-environment happy-dom
// A draft moved to another host: its project is made ready there and the
// draft is opened again in that host's scope, where it then spawns like any
// draft of that host's. These drive the real store with This Mac's engine
// mocked and a paired host behind a fake connection, and pin what each side is
// asked, where the draft lands, and what of This Mac's never crosses.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/** This Mac's engine: each op a spy, answered from `localAnswers`. */
const { localApi, localAnswers } = vi.hoisted(() => {
  const localAnswers: Record<string, (...args: unknown[]) => unknown> = {};
  const calls = new Map<string, ReturnType<typeof vi.fn>>();
  const localApi = new Proxy(
    {},
    {
      get: (_target, op: string) => {
        let spy = calls.get(op);
        if (!spy) {
          spy = vi.fn((...args: unknown[]) =>
            Promise.resolve(localAnswers[op] ? localAnswers[op](...args) : undefined),
          );
          calls.set(op, spy);
        }
        return spy;
      },
    },
  ) as Record<string, ReturnType<typeof vi.fn>>;
  return { localApi, localAnswers };
});
vi.mock("@/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api")>();
  return { ...actual, api: localApi };
});
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import type { AgentRecord, ProjectDefinition, ProjectRef, Workspace } from "@/api";
import type { Transport } from "@/api/transport";
import { hostStore, startHost, stopHost, useAppStore } from "@/store";
import type { DraftAgent } from "./drafts";
import { LOCAL_ENVIRONMENT_ID } from "./environments";

const HOME_PATH = "/Users/me/code/app";
const COPY_PATH = "/Users/studio/.fletch/projects/p1/repos/r1/app";

const DEFINITION: ProjectDefinition = {
  project_id: "p1",
  name: "App",
  home: { device: "this-mac", name: "MacBook" },
  repos: [{ id: "r1", dir: "app", origin: "https://github.com/acme/app.git", label: null }],
};

const ref = (path: string, home?: ProjectRef["home"]): ProjectRef => ({
  path,
  name: "App",
  project_id: "p1",
  label: null,
  home,
});

const agent = (id: string, repoPath: string): AgentRecord =>
  ({
    id,
    name: id,
    project_id: "p1",
    provider: "claude",
    status: "spawning",
    view: "custom",
    task: "",
    created_at: "2026-11-17T10:00:00Z",
    repos: [{ repo_path: repoPath, subdir: "", branch: null, parent_branch: "main" }],
  }) as unknown as AgentRecord;

/** A paired host behind a fake connection, with a copy of the project once
 *  it is asked to make one. `fail` makes one op refuse with the host's words;
 *  `hold` keeps one op unanswered until its promise resolves. */
function fakeHost(
  id: string,
  fail: Record<string, string> = {},
  hold?: { op: string; until: Promise<void> },
) {
  const calls: [string, Record<string, unknown> | undefined][] = [];
  let workspace: Workspace = { repos: [], projects: [], agents: [] } as unknown as Workspace;
  const answer = (op: string, args?: Record<string, unknown>): unknown => {
    switch (op) {
      case "ensure_project":
        workspace = {
          ...workspace,
          repos: [COPY_PATH],
          projects: [ref(COPY_PATH, DEFINITION.home)],
        };
        return workspace;
      case "get_workspace":
        return workspace;
      case "allocate_draft_name":
        return "aconcagua";
      case "list_repo_branches":
        return ["main"];
      case "repo_default_branch":
        return "main";
      case "spawn_agent": {
        const rec = agent("fuji", String(args?.repoPath));
        workspace = { ...workspace, agents: [rec] };
        return rec;
      }
      case "send_user_message":
        return true;
      default:
        return undefined;
    }
  };
  const transport: Transport = {
    call: async <T>(op: string, args?: Record<string, unknown>): Promise<T> => {
      calls.push([op, args]);
      if (fail[op]) throw new Error(fail[op]);
      if (op === hold?.op) await hold.until;
      return answer(op, args) as T;
    },
    on: () => Promise.resolve(() => {}),
  };
  return { id, calls, transport, ops: () => calls.map(([op]) => op) };
}

type FakeHost = ReturnType<typeof fakeHost>;

async function publish(host: FakeHost) {
  useAppStore.getState().upsertEnvironment({
    id: host.id,
    name: "Studio",
    kind: "remote",
    connection: "connected",
    transport: host.transport,
    protocol: {
      version: 9,
      ops: ["ensure_project", "project_definition", "spawn_agent"],
      events: [],
      features: [],
    },
  });
  await startHost(host.id);
}

const DRAFT: DraftAgent = {
  id: "d1",
  kind: "agent",
  repoPath: HOME_PATH,
  name: "chimborazo",
  provider: "claude",
  base: "main",
};

/** This Mac's store with `draft` open on its project. */
function openDraft(draft: DraftAgent) {
  useAppStore.setState({
    workspace: {
      repos: [HOME_PATH],
      projects: [ref(HOME_PATH)],
      agents: [],
    } as unknown as Workspace,
    drafts: [draft],
    activeDraftId: draft.id,
  });
}

const TEXT = "fix the login bug";

/** This Mac's store with `draft` open on its project and `TEXT` typed. */
function drafting(draft: DraftAgent) {
  openDraft(draft);
  useAppStore.getState().setComposerDraft(draft.id, TEXT);
}

const move = () => useAppStore.getState().moveDraft("d1", "studio");

const argsOf = (host: FakeHost, op: string) => host.calls.find(([o]) => o === op)?.[1];

let studio: FakeHost;

async function pair(host: FakeHost) {
  await stopHost(studio.id);
  studio = host;
  await publish(studio);
}

beforeEach(async () => {
  vi.clearAllMocks();
  for (const key of Object.keys(localAnswers)) delete localAnswers[key];
  localAnswers.projectDefinition = () => DEFINITION;
  localAnswers.spawnAgent = (_view, repoPath) => agent("kyoto", String(repoPath));
  localAnswers.sendUserMessage = () => true;
  useAppStore.setState(useAppStore.getInitialState(), true);
  studio = fakeHost("studio");
  await publish(studio);
});

afterEach(async () => {
  await stopHost(studio.id);
  useAppStore.getState().removeEnvironment(studio.id);
});

describe("a draft left on its own host", () => {
  it("spawns exactly as before and asks neither host about the project", async () => {
    openDraft(DRAFT);

    await expect(useAppStore.getState().spawnFromDraft("d1", TEXT, "claude")).resolves.toBe(true);

    expect(localApi.projectDefinition).not.toHaveBeenCalled();
    expect(localApi.spawnAgent).toHaveBeenCalledTimes(1);
    expect(localApi.spawnAgent.mock.calls[0][1]).toBe(HOME_PATH);
    expect(studio.ops()).not.toContain("ensure_project");
    expect(studio.ops()).not.toContain("spawn_agent");
    expect(useAppStore.getState().selectedAgentId).toBe("kyoto");
    expect(useAppStore.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
  });

  it("stays put when it is a roadmap hand-off", async () => {
    drafting({ ...DRAFT, roadmapItemId: "item-1" });

    await move();

    expect(localApi.projectDefinition).not.toHaveBeenCalled();
    expect(useAppStore.getState().drafts.map((d) => d.id)).toEqual(["d1"]);
  });
});

describe("a draft moved to a paired host", () => {
  it("makes the project ready there and opens the draft again in that host's scope", async () => {
    drafting({ ...DRAFT, base: "develop", issueRef: "42" });

    await move();

    // The home describes the project; the host makes its copy, then names
    // the draft itself. Nothing is spawned by a move.
    expect(localApi.projectDefinition).toHaveBeenCalledWith("p1");
    expect(argsOf(studio, "ensure_project")).toEqual({ definition: DEFINITION });
    const ops = studio.ops();
    expect(ops.indexOf("ensure_project")).toBeLessThan(ops.indexOf("allocate_draft_name"));
    expect(ops).not.toContain("spawn_agent");
    expect(localApi.spawnAgent).not.toHaveBeenCalled();

    const s = useAppStore.getState();
    const there = s.hostStates.studio;
    expect(there.drafts).toHaveLength(1);
    const moved = there.drafts[0];
    // The task goes with it: what was typed, and the choices that mean the
    // same on any device.
    expect(moved).toMatchObject({
      kind: "agent",
      repoPath: COPY_PATH,
      provider: "claude",
      base: "develop",
      issueRef: "42",
    });
    expect(there.composerDrafts[moved.id]).toBe(TEXT);
    expect(there.activeDraftId).toBe(moved.id);
    // It has left where it lived, and is what is on screen.
    expect(s.drafts).toEqual([]);
    expect(s.activeDraftId).toBeNull();
    expect(s.composerDrafts.d1).toBeUndefined();
    expect(s.activeEnvironmentId).toBe("studio");
  });

  it("leaves behind everything that names something of its own host", async () => {
    drafting({ ...DRAFT, customAgentId: "ca-1", account: "work", workspaceType: "box" });

    await move();

    const moved = useAppStore.getState().hostStates.studio.drafts[0];
    // Named by the host it now runs on, against that host's own agents.
    expect(moved.name).toBe("aconcagua");
    expect(moved.customAgentId).toBeUndefined();
    expect(moved.account).toBeUndefined();
    expect(moved.workspaceType).toBeUndefined();
  });

  it("then spawns there like any draft of that host's", async () => {
    drafting({ ...DRAFT, base: "develop" });
    await move();
    const there = hostStore("studio");
    const moved = there.getState().drafts[0];

    await expect(there.getState().spawnFromDraft(moved.id, TEXT, "claude")).resolves.toBe(true);

    // The host forks from the branch as its remote has it.
    expect(argsOf(studio, "spawn_agent")).toMatchObject({
      repoPath: COPY_PATH,
      name: "aconcagua",
      forkBase: "develop",
      task: TEXT,
    });
    expect(argsOf(studio, "send_user_message")).toMatchObject({ agentId: "fuji", text: TEXT });
    expect(localApi.spawnAgent).not.toHaveBeenCalled();
    const s = useAppStore.getState();
    expect(s.hostStates.studio.selectedAgentId).toBe("fuji");
    expect(s.hostStates.studio.drafts).toEqual([]);
    expect(s.workspace?.agents).toEqual([]);
  });

  it("keeps the draft and its message, with the host's error, when the copy can't be made", async () => {
    await pair(fakeHost("studio", { ensure_project: "Couldn't clone acme/app: not found" }));
    drafting(DRAFT);

    await move();

    const s = useAppStore.getState();
    expect(s.lastError).toBe("Couldn't clone acme/app: not found");
    expect(s.drafts.map((d) => d.id)).toEqual(["d1"]);
    expect(s.drafts[0].preparing).toBeUndefined();
    expect(s.composerDrafts.d1).toBe(TEXT);
    expect(s.activeDraftId).toBe("d1");
    expect(s.activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
    expect(s.hostStates.studio.drafts).toEqual([]);
  });

  /** Pair a host that holds `op` until `ready()`, with the draft open. */
  let ready: () => void;
  async function holding(op: string) {
    const until = new Promise<void>((resolve) => {
      ready = resolve;
    });
    await pair(fakeHost("studio", {}, { op, until }));
    drafting(DRAFT);
  }

  describe("while its project is being made ready", () => {
    beforeEach(() => holding("ensure_project"));

    it("sends nothing and starts no second move", async () => {
      const moving = move();
      await vi.waitFor(() => expect(studio.ops()).toContain("ensure_project"));
      expect(useAppStore.getState().drafts[0].preparing).toBe("Preparing the project on Studio…");

      await expect(useAppStore.getState().spawnFromDraft("d1", TEXT, "claude")).resolves.toBe(
        false,
      );
      await move();
      expect(localApi.spawnAgent).not.toHaveBeenCalled();
      expect(localApi.projectDefinition).toHaveBeenCalledTimes(1);

      ready();
      await moving;
      expect(useAppStore.getState().hostStates.studio.drafts).toHaveLength(1);
    });

    it("moves what the user typed meanwhile", async () => {
      const moving = move();
      await vi.waitFor(() => expect(studio.ops()).toContain("ensure_project"));
      useAppStore.getState().setComposerDraft("d1", "and the logout one");

      ready();
      await moving;

      const there = useAppStore.getState().hostStates.studio;
      expect(there.composerDrafts[there.drafts[0].id]).toBe("and the logout one");
    });

    it("opens nothing for a draft discarded meanwhile", async () => {
      const moving = move();
      await vi.waitFor(() => expect(studio.ops()).toContain("ensure_project"));
      useAppStore.getState().removeDraft("d1");

      ready();
      await moving;

      expect(useAppStore.getState().hostStates.studio.drafts).toEqual([]);
      expect(useAppStore.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
    });

    it("does not take the screen from where the user has gone", async () => {
      const moving = move();
      await vi.waitFor(() => expect(studio.ops()).toContain("ensure_project"));
      useAppStore.getState().selectDraft(null);

      ready();
      await moving;

      const s = useAppStore.getState();
      expect(s.hostStates.studio.drafts).toHaveLength(1);
      expect(s.drafts).toEqual([]);
      expect(s.activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
    });
  });

  describe("while the host names it, the last thing asked", () => {
    beforeEach(() => holding("allocate_draft_name"));

    it("opens nothing for a draft discarded then", async () => {
      const moving = move();
      await vi.waitFor(() => expect(studio.ops()).toContain("allocate_draft_name"));
      useAppStore.getState().removeDraft("d1");

      ready();
      await moving;

      const s = useAppStore.getState();
      expect(s.hostStates.studio.drafts).toEqual([]);
      expect(s.activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
    });

    it("does not take the screen from where the user went then", async () => {
      const moving = move();
      await vi.waitFor(() => expect(studio.ops()).toContain("allocate_draft_name"));
      useAppStore.getState().selectDraft(null);

      ready();
      await moving;

      const s = useAppStore.getState();
      expect(s.hostStates.studio.drafts).toHaveLength(1);
      expect(s.drafts).toEqual([]);
      expect(s.activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
    });
  });

  it("opens over a project page the host had up, and shows its copy through a refresh", async () => {
    hostStore("studio").getState().openProjectSettings("/some/where");
    drafting(DRAFT);

    await move();

    const there = useAppStore.getState().hostStates.studio;
    expect(there.projectScreen).toBeNull();
    expect(studio.ops()).toContain("get_workspace");
    expect(there.workspace?.projects.map((p) => p.path)).toEqual([COPY_PATH]);
  });
});

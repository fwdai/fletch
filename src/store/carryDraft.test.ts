// @vitest-environment happy-dom
// A draft picked to run on another host: its project is carried there and the
// agent is spawned by that host's own actions, into its partition. These drive
// the real store with This Mac's engine mocked and a paired host behind a fake
// connection, and pin what each side is asked, where the result lands, and what
// of This Mac's never crosses.

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
 *  it is asked to make one. `fail` makes one op refuse with the host's words. */
function fakeHost(id: string, fail: Record<string, string> = {}) {
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
    call: <T>(op: string, args?: Record<string, unknown>): Promise<T> => {
      calls.push([op, args]);
      if (fail[op]) return Promise.reject(new Error(fail[op]));
      return Promise.resolve(answer(op, args) as T);
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

const spawnFromDraft = (folders: { path: string; writable: boolean }[] = []) =>
  useAppStore
    .getState()
    .spawnFromDraft(
      "d1",
      "fix the login bug",
      "claude",
      undefined,
      [],
      undefined,
      undefined,
      folders,
    );

const argsOf = (host: FakeHost, op: string) => host.calls.find(([o]) => o === op)?.[1];

let studio: FakeHost;

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

describe("a draft run on its own host", () => {
  it("spawns exactly as before and asks neither host about the project", async () => {
    openDraft(DRAFT);

    await expect(spawnFromDraft()).resolves.toBe(true);

    expect(localApi.projectDefinition).not.toHaveBeenCalled();
    expect(localApi.spawnAgent).toHaveBeenCalledTimes(1);
    expect(localApi.spawnAgent.mock.calls[0][1]).toBe(HOME_PATH);
    expect(studio.ops()).not.toContain("ensure_project");
    expect(studio.ops()).not.toContain("spawn_agent");
    expect(useAppStore.getState().selectedAgentId).toBe("kyoto");
    expect(useAppStore.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
  });
});

describe("a draft run on a paired host", () => {
  it("carries the project there and spawns into that host's partition", async () => {
    openDraft({ ...DRAFT, runsOn: "studio" });

    await expect(spawnFromDraft()).resolves.toBe(true);

    // The home describes the project; the host makes its copy, then spawns in it.
    expect(localApi.projectDefinition).toHaveBeenCalledWith("p1");
    expect(argsOf(studio, "ensure_project")).toEqual({ definition: DEFINITION });
    const ops = studio.ops();
    expect(ops.indexOf("ensure_project")).toBeLessThan(ops.indexOf("spawn_agent"));
    expect(argsOf(studio, "spawn_agent")).toMatchObject({
      repoPath: COPY_PATH,
      forkBase: "main",
      task: "fix the login bug",
    });
    expect(argsOf(studio, "send_user_message")).toMatchObject({
      agentId: "fuji",
      text: "fix the login bug",
    });
    expect(localApi.spawnAgent).not.toHaveBeenCalled();

    // Everything about the new workspace is the host's…
    const s = useAppStore.getState();
    expect(s.hostStates.studio.selectedAgentId).toBe("fuji");
    expect(s.hostStates.studio.workspace?.agents.map((a) => a.id)).toEqual(["fuji"]);
    expect(s.hostStates.studio.managedLogs.fuji?.[0]).toMatchObject({ text: "fix the login bug" });
    expect(s.hostStates.studio.drafts).toEqual([]);
    // …nothing of it is This Mac's, and the draft is gone from where it lived.
    expect(s.workspace?.agents).toEqual([]);
    expect(s.managedLogs.fuji).toBeUndefined();
    expect(s.drafts).toEqual([]);
    expect(s.activeDraftId).toBeNull();
    // And it is what is on screen.
    expect(s.activeEnvironmentId).toBe("studio");
    expect(hostStore("studio").getState().selectedAgentId).toBe("fuji");
  });

  it("keeps the draft and reports the host's error when the copy can't be made", async () => {
    await stopHost(studio.id);
    studio = fakeHost("studio", { ensure_project: "Couldn't clone acme/app: not found" });
    await publish(studio);
    openDraft({ ...DRAFT, runsOn: "studio" });

    await expect(spawnFromDraft()).resolves.toBe(false);

    expect(studio.ops()).not.toContain("spawn_agent");
    const s = useAppStore.getState();
    expect(s.lastError).toBe("Couldn't clone acme/app: not found");
    expect(s.drafts.map((d) => d.id)).toEqual(["d1"]);
    expect(s.drafts[0].preparing).toBeUndefined();
    expect(s.activeDraftId).toBe("d1");
    expect(s.activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
  });

  it("refuses a base branch the copy does not have rather than forking another", async () => {
    openDraft({ ...DRAFT, base: "wip/local-only", runsOn: "studio" });

    await expect(spawnFromDraft()).resolves.toBe(false);

    expect(studio.ops()).not.toContain("spawn_agent");
    expect(useAppStore.getState().lastError).toBe(
      "wip/local-only isn't on Studio. Push it, or pick a branch it has.",
    );
    expect(useAppStore.getState().drafts.map((d) => d.id)).toEqual(["d1"]);
  });

  it("sends nothing of This Mac's own: no account, workspace type or folder", async () => {
    openDraft({ ...DRAFT, runsOn: "studio", account: "work", workspaceType: "box" });

    await expect(spawnFromDraft([{ path: "/Users/me/Notes", writable: true }])).resolves.toBe(true);

    expect(argsOf(studio, "spawn_agent")).toMatchObject({
      account: null,
      workspaceType: null,
      folderGrants: null,
    });
  });
});

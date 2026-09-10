// The bug this pins: agent ids are landmark names, and the backend recycles the
// names of archived agents. Spawning a draft under a recycled name returned a
// record whose id the store ALREADY held — as the archived predecessor. The
// spawn path selected that id before the workspace refresh landed, so the chat
// mounted against the archived record (its provider/model/effort/custom agent),
// and since the id — and so the React key — never changed, the composer's
// mount-time picker state was never corrected until the user switched chats.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { spawnAgent, sendUserMessage, getWorkspace } = vi.hoisted(() => ({
  spawnAgent: vi.fn(),
  sendUserMessage: vi.fn(),
  getWorkspace: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { spawnAgent, sendUserMessage, getWorkspace } }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/data/slashCommands", () => ({ discoverCommands: vi.fn() }));

import type { AgentRecord, Workspace } from "@/api";
import { adoptSpawnedAgent } from "./adoptSpawnedAgent";
import { createDraftsSlice } from "./drafts";
import type { AppState } from "./types";

const NAME = "chimborazo";

const record = (over: Partial<AgentRecord>): AgentRecord =>
  ({
    id: NAME,
    project_id: "p1",
    name: NAME,
    provider: "claude",
    repos: [{ repo_path: "/repos/app", subdir: "app" }],
    task: "",
    status: "idle",
    view: "custom",
    created_at: "2026-01-01T00:00:00Z",
    ...over,
  }) as AgentRecord;

/** The archived agent that used to own this name — a different provider, model
 *  and custom agent than the one about to be spawned. */
const archived = record({
  provider: "codex",
  model: "gpt-5-codex",
  custom_agent_id: "reviewer",
  effort: "high",
  archive: { archived_at: "2026-01-02T00:00:00Z", repos: [], diff_stats: {} as never },
});

const unrelated = record({ id: "eiger", name: "eiger" });

/** What the backend returns for the fresh spawn under the recycled name. */
const fresh = record({ provider: "claude", model: "claude-opus-5", status: "spawning" });

const workspaceWith = (agents: AgentRecord[]): Workspace => ({
  repos: ["/repos/app"],
  projects: [],
  agents,
});

const makeStore = (workspace: Workspace) => {
  const store = create<AppState>()((...a) => ({ ...createDraftsSlice(...a) }) as AppState);
  store.setState({
    workspace,
    drafts: [{ id: "d1", repoPath: "/repos/app", name: NAME, provider: "claude", base: "main" }],
    composerDrafts: {},
    customAgents: [],
    modelsByAgent: {},
    skills: [],
    managedLogs: {},
    managedBusy: {},
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  } as any);
  return store;
};

describe("adoptSpawnedAgent", () => {
  it("replaces an archived record that still holds the recycled id", () => {
    const next = adoptSpawnedAgent(workspaceWith([unrelated, archived]), fresh);
    expect(next?.agents).toEqual([fresh, unrelated]);
  });

  it("adds the record when the snapshot has never seen the id", () => {
    const next = adoptSpawnedAgent(workspaceWith([unrelated]), fresh);
    expect(next?.agents).toEqual([fresh, unrelated]);
  });

  it("is a no-op before the workspace has loaded", () => {
    expect(adoptSpawnedAgent(null, fresh)).toBeNull();
  });
});

describe("spawning a draft under a recycled landmark name", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    spawnAgent.mockResolvedValue(fresh);
    sendUserMessage.mockResolvedValue(false);
    // The refresh yields nothing, so the store is exactly what the spawn path
    // itself committed — the window the chat mounts in before a snapshot lands.
    getWorkspace.mockResolvedValue(null);
  });

  it("selects the fresh record, not the archived one that shared its id", async () => {
    const store = makeStore(workspaceWith([unrelated, archived]));

    await store.getState().spawnFromDraft("d1", "ship it", "claude", "claude-opus-5");

    const { selectedAgentId, workspace } = store.getState();
    expect(selectedAgentId).toBe(NAME);
    const selected = workspace?.agents.find((a) => a.id === selectedAgentId);
    expect(selected).toEqual(fresh);
    // The archived predecessor is gone entirely — one record per id.
    expect(workspace?.agents.filter((a) => a.id === NAME)).toHaveLength(1);
    expect(workspace?.agents).toContainEqual(unrelated);
  });

  it("makes the new record visible immediately on a never-used name too", async () => {
    const store = makeStore(workspaceWith([unrelated]));

    await store.getState().spawnFromDraft("d1", "ship it", "claude", "claude-opus-5");

    const { selectedAgentId, workspace } = store.getState();
    expect(workspace?.agents.find((a) => a.id === selectedAgentId)).toEqual(fresh);
  });
});

// A checkout whose config Fletch refuses to run git over answers the poll with a
// zero-state carrying `blocked_config`. That zero-state — no files, nothing
// unpushed — must never reach `gitStates`, where delegation and autopilot would
// read it as "committed and pushed"; it goes to `gitBlocked` instead.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { getGitState } = vi.hoisted(() => ({ getGitState: vi.fn() }));
vi.mock("@/api", () => ({ api: { getGitState } }));

import type { GitState } from "@/api";
import { type EnvironmentId, setEnvironmentsSource } from "./environments";
import { createGitSlice } from "./git";
import type { AppState } from "./types";

let activeEnv: EnvironmentId = "local";
setEnvironmentsSource(() => ({ environments: {}, activeEnvironmentId: activeEnv }));

const state = (over: Partial<GitState> = {}): GitState => ({
  branch: "fix/x",
  parent_branch: "main",
  ahead: 1,
  behind: 0,
  unpushed: 1,
  files: [{ path: "a.ts", kind: "modified", staged: false, additions: 1, deletions: 0 }],
  additions: 1,
  deletions: 0,
  has_origin: true,
  ...over,
});

const makeStore = () =>
  create<AppState>()((...a) => ({ ...createGitSlice(...a) }) as unknown as AppState);

describe("fetchGitState on a refused checkout", () => {
  beforeEach(() => {
    getGitState.mockReset();
    activeEnv = "local";
  });

  it("drops an answer that lands after an environment switch", async () => {
    const store = makeStore();
    // Agent ids recur across hosts, so the old host's `a1` answering after the
    // switch must not block (or overwrite) the new host's `a1`.
    getGitState.mockImplementationOnce(async () => {
      activeEnv = "host-2";
      return state({ blocked_config: ["filter.lfs.clean"] });
    });
    await store.getState().fetchGitState("a1");

    expect(store.getState().gitBlocked).toEqual({});
    expect(store.getState().gitStates).toEqual({});
  });

  it("records the blocking keys and keeps the last real state", async () => {
    const store = makeStore();
    getGitState.mockResolvedValueOnce(state());
    await store.getState().fetchGitState("a1");

    getGitState.mockResolvedValueOnce(
      state({ files: [], unpushed: 0, ahead: 0, blocked_config: ["filter.lfs.clean"] }),
    );
    await store.getState().fetchGitState("a1");

    expect(store.getState().gitBlocked).toEqual({ a1: ["filter.lfs.clean"] });
    expect(store.getState().gitStates.a1.files).toHaveLength(1);
    expect(store.getState().gitStates.a1.unpushed).toBe(1);
  });

  it("clears the block once a poll reads the checkout again", async () => {
    const store = makeStore();
    getGitState.mockResolvedValueOnce(state({ blocked_config: ["filter.lfs.clean"] }));
    await store.getState().fetchGitState("a1", "web");
    expect(store.getState().gitBlocked).toEqual({ "a1::web": ["filter.lfs.clean"] });

    getGitState.mockResolvedValueOnce(state({ blocked_config: [] }));
    await store.getState().fetchGitState("a1", "web");

    expect(store.getState().gitBlocked).toEqual({});
    expect(store.getState().gitStates["a1::web"]).toBeDefined();
  });
});

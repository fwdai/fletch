// The new-agent screen's base-branch picker is sticky per project: once the
// user picks a branch there, the next agent they start in that project defaults
// to it instead of the repo's default branch. These pin the store side of that
// — the remembering, the per-project scoping, and the fallback when the
// remembered branch no longer exists.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { allocateDraftName, repoDefaultBranch, listRepoBranches, setSetting } = vi.hoisted(() => ({
  allocateDraftName: vi.fn(),
  repoDefaultBranch: vi.fn(),
  listRepoBranches: vi.fn(),
  setSetting: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { allocateDraftName, repoDefaultBranch, listRepoBranches } }));
vi.mock("@/storage/settings", () => ({ setSetting }));

import { createDraftsSlice } from "./drafts";
import type { AppState } from "./types";

const makeStore = () => {
  const store = create<AppState>()((...a) => ({ ...createDraftsSlice(...a) }) as AppState);
  store.setState({
    customAgents: [],
    modelsByAgent: {},
    composerDrafts: {},
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  } as any);
  return store;
};

/** The base of the draft `createDraft` just pushed to the front of the list. */
const newestBase = (store: ReturnType<typeof makeStore>) => store.getState().drafts[0]?.base;

describe("sticky draft base branch", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    allocateDraftName.mockResolvedValue("everest");
    repoDefaultBranch.mockResolvedValue("main");
    listRepoBranches.mockResolvedValue(["main", "develop"]);
  });

  it("starts a new draft on the repo default when nothing is remembered", async () => {
    const store = makeStore();
    await store.getState().createDraft("/repos/app");
    expect(newestBase(store)).toBe("main");
  });

  it("seeds the next draft with the branch last picked for that project", async () => {
    const store = makeStore();
    store.getState().rememberDraftBase("/repos/app", "develop");

    await store.getState().createDraft("/repos/app");

    expect(newestBase(store)).toBe("develop");
    expect(setSetting).toHaveBeenCalledWith("draftBaseBranches", { "/repos/app": "develop" });
  });

  it("scopes the pick to its project — another project keeps its own default", async () => {
    const store = makeStore();
    store.getState().rememberDraftBase("/repos/app", "develop");

    await store.getState().createDraft("/repos/other");

    expect(newestBase(store)).toBe("main");
  });

  it("falls back to the repo default once the remembered branch is deleted", async () => {
    const store = makeStore();
    store.getState().rememberDraftBase("/repos/app", "develop");
    // "develop" has since been deleted from the repo.
    listRepoBranches.mockResolvedValue(["main"]);

    await store.getState().createDraft("/repos/app");

    expect(newestBase(store)).toBe("main");
  });

  it("keeps the remembered pick when a branch is deleted, so a re-created branch resticks", async () => {
    // Deletion is not a reason to forget the intent — the pick is only ignored
    // while the branch is missing.
    const store = makeStore();
    store.getState().rememberDraftBase("/repos/app", "develop");
    listRepoBranches.mockResolvedValue(["main"]);
    await store.getState().createDraft("/repos/app");

    listRepoBranches.mockResolvedValue(["main", "develop"]);
    await store.getState().createDraft("/repos/app");

    expect(newestBase(store)).toBe("develop");
  });

  it("re-picking a project's branch replaces the previous pick", async () => {
    const store = makeStore();
    store.getState().rememberDraftBase("/repos/app", "develop");
    store.getState().rememberDraftBase("/repos/app", "release/1.x");
    expect(store.getState().draftBaseByRepo).toEqual({ "/repos/app": "release/1.x" });
  });

  it("doesn't re-persist an unchanged pick", () => {
    const store = makeStore();
    store.getState().rememberDraftBase("/repos/app", "develop");
    store.getState().rememberDraftBase("/repos/app", "develop");
    expect(setSetting).toHaveBeenCalledTimes(1);
  });
});

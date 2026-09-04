// Regression tests for the base branch a spawn forks from.
//
// The bug these pin: `spawn(view, repoPath)` passed no `forkBase` at all, so the
// backend fell back to the source repo's *currently-checked-out* branch — a new
// agent silently forked onto the user's in-progress local work. The product rule
// is that the app must never pick that branch implicitly, so every spawn path
// has to supply a base, and that base is the repo's resolved default (not a
// hardcoded "main", which forks the wrong branch on a master/develop repo).

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { spawnAgent, getWorkspace, repoDefaultBranch, listRepoBranches } = vi.hoisted(() => ({
  spawnAgent: vi.fn(),
  getWorkspace: vi.fn(),
  repoDefaultBranch: vi.fn(),
  listRepoBranches: vi.fn(),
}));
vi.mock("@/api", () => ({
  api: { spawnAgent, getWorkspace, repoDefaultBranch, listRepoBranches },
}));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));

import { resolveBaseBranch } from "@/helpers";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

const makeStore = () => {
  const store = create<AppState>()((...a) => ({ ...createWorkspaceSlice(...a) }) as AppState);
  // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  store.setState({ managedLogs: {}, managedBusy: {} } as any);
  return store;
};

/** `forkBase` is the 9th positional arg of `api.spawnAgent`. */
const forkBaseArg = (call: unknown[]) => call[8];

describe("spawn base branch", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    spawnAgent.mockResolvedValue({ id: "a1" });
    getWorkspace.mockResolvedValue(null);
  });

  it("passes the repo's resolved default branch as the fork base", async () => {
    repoDefaultBranch.mockResolvedValue("develop");
    const store = makeStore();

    await store.getState().spawn("custom", "/repos/app");

    expect(repoDefaultBranch).toHaveBeenCalledWith("/repos/app");
    expect(forkBaseArg(spawnAgent.mock.calls[0])).toBe("develop");
  });

  it("never leaves the fork base unset, even when the resolve fails", async () => {
    // An unset base is what let the backend fall back to the checked-out
    // branch, so a failure here must still yield a concrete base.
    repoDefaultBranch.mockRejectedValue(new Error("no such repo"));
    const store = makeStore();

    await store.getState().spawn("custom", "/repos/app");

    expect(forkBaseArg(spawnAgent.mock.calls[0])).toBe("main");
  });

  it("resolves the default branch rather than assuming main", async () => {
    repoDefaultBranch.mockResolvedValue("master");
    await expect(resolveBaseBranch("/repos/legacy")).resolves.toBe("master");
  });
});

// The new-agent screen's branch picker is sticky per project: a remembered pick
// seeds the next draft there. It must never outlive the branch itself, so the
// resolver re-validates it against the repo's live branches before honoring it.
describe("sticky base branch", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    repoDefaultBranch.mockResolvedValue("main");
  });

  it("honors a remembered branch that still exists", async () => {
    listRepoBranches.mockResolvedValue(["main", "develop", "release/1.x"]);
    await expect(resolveBaseBranch("/repos/app", "develop")).resolves.toBe("develop");
    expect(repoDefaultBranch).not.toHaveBeenCalled();
  });

  it("falls back to the repo default when the remembered branch was deleted", async () => {
    listRepoBranches.mockResolvedValue(["main", "release/1.x"]);
    await expect(resolveBaseBranch("/repos/app", "develop")).resolves.toBe("main");
  });

  it("falls back rather than forking from an unverified branch", async () => {
    // Listing branches failed, so "develop" can't be confirmed to exist.
    listRepoBranches.mockRejectedValue(new Error("not a repo"));
    await expect(resolveBaseBranch("/repos/app", "develop")).resolves.toBe("main");
  });

  it("skips the branch listing entirely when nothing is remembered", async () => {
    await expect(resolveBaseBranch("/repos/app")).resolves.toBe("main");
    expect(listRepoBranches).not.toHaveBeenCalled();
  });
});

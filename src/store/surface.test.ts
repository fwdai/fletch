import { describe, expect, it } from "vitest";
import { activeSurface, surfaceAgent, surfaceRepoPath } from "./surface";
import type { AppState } from "./types";

const agent = (id: string, repo: string) =>
  ({
    id,
    status: "idle",
    repos: [{ repo_path: repo }],
    archive: null,
  }) as unknown as AppState["workspace"] extends { agents: (infer A)[] } | null ? A : never;

/** The slice of state the surface reads, everything closed and nothing
 *  selected, with `patch` on top. */
function state(patch: Partial<AppState> = {}): AppState {
  return {
    onboardingOpen: false,
    historyOpen: false,
    settingsScreenOpen: false,
    usageScreenOpen: false,
    projectScreenRepoPath: null,
    activeDraftId: null,
    drafts: [],
    selectedRunId: null,
    selectedAgentId: null,
    lastRepoPath: undefined,
    workspace: {
      projects: [{ path: "/a" }, { path: "/b" }],
      repos: ["/a", "/b"],
      agents: [agent("one", "/a"), agent("two", "/b")],
    },
    ...patch,
  } as unknown as AppState;
}

describe("activeSurface", () => {
  it("is Home with nothing open or selected", () => {
    expect(activeSurface(state())).toEqual({ kind: "home" });
  });

  it("is the selected agent, or the run, or the draft, in the center pane's order", () => {
    expect(activeSurface(state({ selectedAgentId: "two" })).kind).toBe("agent");
    expect(surfaceAgent(state({ selectedAgentId: "two" }))?.id).toBe("two");
    expect(activeSurface(state({ selectedAgentId: "two", selectedRunId: "r1" }))).toEqual({
      kind: "run",
      runId: "r1",
    });
    const drafts = [{ id: "d1", repoPath: "/a" }] as AppState["drafts"];
    expect(activeSurface(state({ selectedAgentId: "two", activeDraftId: "d1", drafts })).kind).toBe(
      "draft",
    );
  });

  it("drops a draft whose project is gone", () => {
    const drafts = [{ id: "d1", repoPath: "/gone" }] as AppState["drafts"];
    expect(activeSurface(state({ activeDraftId: "d1", drafts, selectedAgentId: "one" })).kind).toBe(
      "agent",
    );
  });

  it("lets a full-screen takeover or an overlay cover the selected agent", () => {
    const selected = { selectedAgentId: "one" } as const;
    expect(activeSurface(state({ ...selected, projectScreenRepoPath: "/a" }))).toEqual({
      kind: "project",
      repoPath: "/a",
    });
    expect(activeSurface(state({ ...selected, usageScreenOpen: true })).kind).toBe("usage");
    expect(activeSurface(state({ ...selected, settingsScreenOpen: true })).kind).toBe("settings");
    expect(activeSurface(state({ ...selected, historyOpen: true })).kind).toBe("history");
    expect(activeSurface(state({ ...selected, onboardingOpen: true })).kind).toBe("onboarding");
    // …and none of them leaves the agent actionable behind it.
    for (const patch of [
      { projectScreenRepoPath: "/a" },
      { usageScreenOpen: true },
      { settingsScreenOpen: true },
      { historyOpen: true },
      { onboardingOpen: true },
    ]) {
      expect(surfaceAgent(state({ ...selected, ...patch }))).toBeNull();
    }
  });

  it("ranks History above the settings screen, as the sheet renders over it", () => {
    expect(activeSurface(state({ settingsScreenOpen: true, historyOpen: true })).kind).toBe(
      "history",
    );
  });
});

describe("surfaceRepoPath", () => {
  it("prefers what is in front, then the last-used project, then the first", () => {
    expect(surfaceRepoPath(state({ selectedAgentId: "two" }))).toBe("/b");
    expect(surfaceRepoPath(state({ projectScreenRepoPath: "/b" }))).toBe("/b");
    const drafts = [{ id: "d1", repoPath: "/b" }] as AppState["drafts"];
    expect(surfaceRepoPath(state({ activeDraftId: "d1", drafts }))).toBe("/b");
    expect(surfaceRepoPath(state({ lastRepoPath: "/b" }))).toBe("/b");
    expect(surfaceRepoPath(state({ lastRepoPath: "/removed" }))).toBe("/a");
    expect(surfaceRepoPath(state())).toBe("/a");
  });

  it("is undefined with no projects", () => {
    expect(
      surfaceRepoPath(state({ workspace: { projects: [], repos: [], agents: [] } as never })),
    ).toBeUndefined();
  });
});

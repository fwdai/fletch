import { describe, expect, it } from "vitest";
import { focusedProjectCount, projectCount, surfaceProjectKey } from "./projectFocus";
import type { AppState } from "./types";

type Agent = NonNullable<AppState["workspace"]>["agents"][number];

const agent = (id: string, projectId: string, repo: string) =>
  ({
    id,
    project_id: projectId,
    status: "idle",
    repos: [{ repo_path: repo }],
    archive: null,
  }) as Agent;

const PROJECTS = [
  { path: "/a", name: "a", project_id: "p-a" },
  { path: "/b", name: "b", project_id: "p-b" },
];

/** The slice of state the focus commands read: two pinned projects, nothing
 *  in front, with `patch` on top. */
function state(patch: Partial<AppState> = {}, agents: Agent[] = []): AppState {
  return {
    onboardingOpen: false,
    historyOpen: false,
    settingsScreenOpen: false,
    usageScreenOpen: false,
    projectScreen: null,
    activeDraftId: null,
    drafts: [],
    selectedRunId: null,
    selectedAgentId: null,
    lastRepoPath: "/a",
    projectFocus: [],
    workspace: { projects: PROJECTS, repos: ["/a", "/b"], agents },
    ...patch,
  } as unknown as AppState;
}

describe("surfaceProjectKey", () => {
  it("is the project of the agent in front", () => {
    const s = state({ selectedAgentId: "two" }, [agent("two", "p-b", "/b")]);
    expect(surfaceProjectKey(s)).toBe("p-b");
  });

  it("follows the project id of an agent whose repo was relocated", () => {
    const s = state({ selectedAgentId: "two" }, [agent("two", "p-b", "/old/b")]);
    expect(surfaceProjectKey(s)).toBe("p-b");
  });

  it("is the repo path for an agent in a repo that is no longer pinned", () => {
    const s = state({ selectedAgentId: "x" }, [agent("x", "p-gone", "/gone")]);
    expect(surfaceProjectKey(s)).toBe("/gone");
  });

  it("falls back to the project last used when nothing of a project is in front", () => {
    expect(surfaceProjectKey(state())).toBe("p-a");
  });

  it("names no project while a workflow run is in front", () => {
    expect(surfaceProjectKey(state({ selectedRunId: "run-in-b" }))).toBeUndefined();
  });
});

describe("the projects there are to focus on", () => {
  it("counts an unpinned repo a live agent still works in, as the sidebar lists it", () => {
    const s = state({ workspace: { projects: [PROJECTS[0]], repos: ["/a"], agents: [] } } as never);
    expect(projectCount(s)).toBe(1);
    const detached = state({
      workspace: {
        projects: [PROJECTS[0]],
        repos: ["/a"],
        agents: [agent("x", "p-gone", "/gone")],
      },
      projectFocus: ["/gone"],
    } as never);
    expect(projectCount(detached)).toBe(2);
    expect(focusedProjectCount(detached)).toBe(1);
  });

  it("does not count archived agents or saved keys of projects that are gone", () => {
    const archived = { ...agent("x", "p-gone", "/gone"), archive: {} } as Agent;
    const s = state({ projectFocus: ["p-removed", "p-b"] }, [archived]);
    expect(projectCount(s)).toBe(2);
    expect(focusedProjectCount(s)).toBe(1);
  });
});

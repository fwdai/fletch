import { describe, expect, it } from "vitest";
import type { ProjectRef } from "@/api";
import { newDraftHost } from "./draftHosts";
import { LOCAL_ENVIRONMENT_ID } from "./environments";

const ref = (path: string, projectId: string): ProjectRef => ({
  path,
  name: projectId,
  project_id: projectId,
  label: null,
});

describe("newDraftHost", () => {
  const local = [ref("/Users/me/app", "p1"), ref("/Users/me/app-api", "p1"), ref("/w/x", "p2")];
  const studio = [ref("/copies/p1/app", "p1"), ref("/studio/only", "p3")];

  it("opens a draft for a project This Mac has in This Mac's scope, at its primary repo", () => {
    expect(newDraftHost(local, studio, "studio", "/copies/p1/app")).toEqual({
      hostId: LOCAL_ENVIRONMENT_ID,
      repoPath: "/Users/me/app",
    });
  });

  it("opens one for a project only the paired host has on that host", () => {
    expect(newDraftHost(local, studio, "studio", "/studio/only")).toEqual({
      hostId: "studio",
      repoPath: "/studio/only",
    });
  });

  it("leaves This Mac's own drafts where they are", () => {
    expect(newDraftHost(local, local, LOCAL_ENVIRONMENT_ID, "/w/x")).toEqual({
      hostId: LOCAL_ENVIRONMENT_ID,
      repoPath: "/w/x",
    });
  });
});

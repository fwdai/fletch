// Which host an agent of a project can be started on (src/lib/runsOn.ts): one
// that has the project always, one that lacks it only if it can take a copy and
// a host that has it can describe it — asked of the host that speaks for it
// first.

import type { ProjectRef, Workspace } from "@desktop/api/types/agent";
import { GATES } from "@desktop/store/gates";
import { describe, expect, it } from "vitest";
import { type ProjectEntry, projectEntries } from "../src/lib/projects";
import { definitionSource, type RunsOnHost, runsOnReason } from "../src/lib/runsOn";
import { V2_DEFAULT_OPS } from "../src/remote";

const ref = (path: string, home?: string) =>
  ({
    project_id: "prj-atlas",
    path,
    name: "atlas",
    label: null,
    ...(home ? { home: { device: home, name: home } } : {}),
  }) as ProjectRef;

const ws = (projects: ProjectRef[]) => ({ projects, agents: [], repos: [] }) as Workspace;

/** A host answering what a current one does, or `ops` alone. */
const host = (hostKey: string, ops?: string[]): RunsOnHost => ({
  hostKey,
  name: hostKey,
  protocol: {
    version: 2,
    ops: ops ?? [...V2_DEFAULT_OPS, "project_definition", "ensure_project"],
    events: [],
    features: [],
  },
});

/** `atlas`, owned by the mini and copied to the studio; the air has none. */
const atlas: ProjectEntry = projectEntries([
  { hostKey: "studio", workspace: ws([ref("/s/atlas", "mini")]) },
  { hostKey: "mini", workspace: ws([ref("/m/atlas")]) },
  { hostKey: "air", workspace: ws([]) },
])[0];

const mini = host("mini");
const studio = host("studio");
const air = host("air");
const old = (h: RunsOnHost) => host(h.hostKey, [...V2_DEFAULT_OPS]);

describe("runsOnReason", () => {
  it("lets every host that has the project run it, whatever it answers", () => {
    expect(runsOnReason(atlas, old(mini), [old(mini), old(studio)])).toBeNull();
    expect(runsOnReason(atlas, old(studio), [old(mini), old(studio)])).toBeNull();
  });

  it("lets a host without it take a copy, described by a host that has it", () => {
    expect(runsOnReason(atlas, air, [mini, studio, air])).toBeNull();
  });

  it("refuses a host too old to take a copy, in the desktop's words", () => {
    expect(runsOnReason(atlas, old(air), [mini, studio, old(air)])).toBe(GATES.projectCopy.reason);
  });

  it("refuses a copy no connected host that has the project can describe", () => {
    // Named after the host that speaks for it, as the one asked first.
    expect(runsOnReason(atlas, air, [old(studio), old(mini), air])).toBe(
      "mini is too old to share this project. Update it.",
    );
    expect(runsOnReason(atlas, air, [old(studio), air])).toBe(
      "studio is too old to share this project. Update it.",
    );
    expect(runsOnReason(atlas, air, [air])).toBe("No connected host has this project.");
  });
});

describe("definitionSource", () => {
  it("asks the host that speaks for the project, its home, first", () => {
    expect(definitionSource(atlas, [studio, mini, air])?.hostKey).toBe("mini");
  });

  it("falls back to another host that has it, and never to one that lacks it", () => {
    expect(definitionSource(atlas, [studio, air])?.hostKey).toBe("studio");
    expect(definitionSource(atlas, [old(mini), studio, air])?.hostKey).toBe("studio");
    expect(definitionSource(atlas, [old(mini), air])).toBeUndefined();
  });
});

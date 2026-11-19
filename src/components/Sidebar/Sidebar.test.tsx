// @vitest-environment happy-dom
// The sidebar over every host: This Mac's projects with the work of each
// paired host that runs in one of them, the projects only a paired host has
// after them, a chip on every row that runs on a paired host and on nothing
// else, and one selection for the window that puts its row's host on screen.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// This Mac's engine answers the sidebar's one read (its workflow runs) with
// none, and its event streams with nothing.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (op: string) => Promise.resolve(op === "wf_list_runs" ? [] : undefined),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import type { AgentRecord, ProjectRef, Workspace } from "@/api";
import type { Transport } from "@/api/transport";
import { hostStore, stopHost, useAppStore } from "@/store";
import { LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { act, fireEvent, render, screen } from "@/test/render";
import { Sidebar } from ".";
import { stepSelection } from "./rowNav";

const project = (path: string, project_id: string, name = path.split("/").pop() ?? path) =>
  ({ path, name, project_id, label: null }) as ProjectRef;

const agent = (id: string, project_id: string, repo: string, task: string, created_at: string) =>
  ({
    id,
    project_id,
    name: id,
    provider: "claude",
    repos: [{ repo_path: repo, subdir: "", branch: `feat/${id}` }],
    task,
    status: "idle",
    view: "custom",
    created_at,
  }) as AgentRecord;

/** Recent enough that no row folds away behind "N older". */
const minutesAgo = (n: number) => new Date(Date.now() - n * 60_000).toISOString();

const workspace = (projects: ProjectRef[], agents: AgentRecord[]) =>
  ({ repos: projects.map((p) => p.path), projects, agents }) as Workspace;

/** A paired host that answers every read with nothing. */
const quietHost: Transport = {
  call: <T,>(op: string) => Promise.resolve((op === "wf_list_runs" ? [] : undefined) as T),
  on: () => Promise.resolve(() => {}),
};

function pair(id: string, name: string, ws: Workspace) {
  useAppStore.getState().upsertEnvironment({
    id,
    name,
    kind: "remote",
    connection: "connected",
    transport: quietHost,
  });
  hostStore(id).setState({ workspace: ws });
}

/** This Mac's `fletch`, with one workspace on it. */
function thisMac() {
  useAppStore.setState({
    workspace: workspace(
      [project("/w/fletch", "p1"), project("/w/relay", "p2")],
      [agent("fuji", "p1", "/w/fletch", "Mine", minutesAgo(1))],
    ),
  });
}

/** The studio has its own copy of `fletch` (same id, another path) with a
 *  workspace that has this Mac's one's id, and a project this Mac has not. */
function studio() {
  pair(
    "studio",
    "Studio",
    workspace(
      [project("/srv/fletch", "p1"), project("/srv/atlas", "p9", "Atlas")],
      [
        agent("fuji", "p1", "/srv/fletch", "Theirs", minutesAgo(2)),
        agent("etna", "p9", "/srv/atlas", "Atlas work", minutesAgo(3)),
      ],
    ),
  );
}

const rows = () => [...document.querySelectorAll<HTMLElement>(".agent[role='button']")];
const rowTitled = (title: string) => {
  const row = screen.getByText(title).closest<HTMLElement>(".agent");
  if (!row) throw new Error(`no row ${title}`);
  return row;
};
const groupLabels = () =>
  [...document.querySelectorAll(".proj .pname")].map((el) => el.textContent);
const chipOf = (el: Element) => el.querySelector(".host-chip")?.textContent ?? null;

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState(), true);
  Element.prototype.scrollIntoView ??= () => {};
});

afterEach(async () => {
  await stopHost("studio");
  useAppStore.getState().removeEnvironment("studio");
});

describe("Sidebar", () => {
  it("with no paired host, lists This Mac's projects and rows, with no chip anywhere", () => {
    thisMac();
    render(<Sidebar />);

    expect(groupLabels()).toEqual(["fletch", "relay"]);
    expect(rows().map((r) => r.querySelector(".ag-title")?.textContent)).toEqual(["Mine"]);
    expect(document.querySelectorAll(".host-chip")).toHaveLength(0);
    // Every group is still a drop target for the manual order.
    expect(document.querySelectorAll(".proj[data-repo-path]")).toHaveLength(2);
  });

  it("reads a turn the host opened, still idle, as working, as the chat does", () => {
    studio();
    hostStore("studio").setState({ turnStartedAt: { etna: Date.now() } });
    render(<Sidebar />);

    const row = rowTitled("Atlas work");
    expect(row.querySelector(".ag-rail.run")).not.toBeNull();
    expect(row.querySelector(".ag-loader")).not.toBeNull();
    expect(row.querySelector("[aria-label='Stop']")).not.toBeNull();
    expect(row.querySelector("[aria-label='Archive']")).toBeNull();

    act(() => hostStore("studio").setState({ turnStartedAt: {} }));
    expect(row.querySelector(".ag-rail.idle")).not.toBeNull();
    expect(row.querySelector(".ag-loader")).toBeNull();
    expect(row.querySelector("[aria-label='Archive']")).not.toBeNull();
  });

  it("merges a paired host's work into This Mac's project of the same id, with a chip", () => {
    thisMac();
    studio();
    render(<Sidebar />);

    // This Mac's projects first, then the one only the studio has.
    expect(groupLabels()).toEqual(["fletch", "relay", "Atlas"]);
    // Two hosts' `fuji` are two rows of one project, newest first.
    const fletch = document.querySelector(".proj[data-repo-path='/w/fletch']");
    const titles = [...(fletch?.querySelectorAll(".ag-title") ?? [])].map((el) => el.textContent);
    expect(titles).toEqual(["Mine", "Theirs"]);
    expect(chipOf(rowTitled("Mine"))).toBeNull();
    expect(chipOf(rowTitled("Theirs"))).toBe("Studio");
    expect(chipOf(rowTitled("Atlas work"))).toBe("Studio");

    // A project only the studio has says so on its header; This Mac's don't.
    const headers = [...document.querySelectorAll(".proj-h")];
    expect(headers.map(chipOf)).toEqual([null, null, "Studio"]);
    // …and is no drop target: the manual order is This Mac's.
    expect(document.querySelectorAll(".proj[data-repo-path]")).toHaveLength(2);
  });

  it("selecting a paired host's row puts that host on screen and selects in its scope", () => {
    thisMac();
    studio();
    useAppStore.getState().selectAgent("fuji");
    render(<Sidebar />);
    expect(rowTitled("Mine").classList).toContain("active");

    fireEvent.click(rowTitled("Theirs"));

    expect(useAppStore.getState().activeEnvironmentId).toBe("studio");
    expect(hostStore("studio").getState().selectedAgentId).toBe("fuji");
    // This Mac keeps its own selection, which is simply not on screen.
    expect(useAppStore.getState().selectedAgentId).toBe("fuji");
    // One row is the selection: the same id on This Mac is not it.
    expect(rowTitled("Theirs").classList).toContain("active");
    expect(rowTitled("Mine").classList).not.toContain("active");
  });

  it("steps through the merged rows across hosts, each step selecting on its host", () => {
    thisMac();
    studio();
    render(<Sidebar />);

    fireEvent.click(rowTitled("Mine"));
    expect(useAppStore.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);

    act(() => stepSelection(1));
    expect(useAppStore.getState().activeEnvironmentId).toBe("studio");
    expect(hostStore("studio").getState().selectedAgentId).toBe("fuji");

    act(() => stepSelection(1));
    expect(hostStore("studio").getState().selectedAgentId).toBe("etna");
    expect(rowTitled("Atlas work").classList).toContain("active");

    // And round to the top, back on This Mac.
    act(() => stepSelection(1));
    expect(useAppStore.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
    expect(rowTitled("Mine").classList).toContain("active");
  });

  it("keeps an offline host's last-known rows, its chip saying so", () => {
    thisMac();
    studio();
    useAppStore.getState().setEnvironmentConnection("studio", "disconnected", undefined, false);
    render(<Sidebar />);

    const chip = rowTitled("Theirs").querySelector(".host-chip");
    expect(chip?.textContent).toBe("Studio");
    expect(chip?.getAttribute("data-tip")).toBe("Runs on Studio · offline");
    expect(chip?.classList).toContain("inactive");
    expect(chip?.querySelector(".env-dot")?.getAttribute("data-state")).toBe("inactive");
  });
});

// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProjectRef, Workspace, WorkspaceTypeOption } from "@/api";
import { workspaceTypesKey } from "@/components/Composer/workspaceType";
import { type DraftAgent, useAppStore } from "@/store";
import { render, screen } from "@/test/render";
import { EmptyWorkspace } from "./EmptyWorkspace";

// The composer is its own tree, and listens to the Tauri webview for drops.
// What it was handed is all a test here reads of it.
const { composer } = vi.hoisted(() => ({
  composer: vi.fn((_props: { folders?: boolean }) => null),
}));
vi.mock("@/components/Composer", () => ({ Composer: composer }));
// The pill reads the host's answer on mount; the tests seed it instead.
vi.mock("@/components/Composer/WorkspaceTypePicker", () => ({ WorkspaceTypePicker: () => null }));

const draft: DraftAgent = {
  id: "d1",
  kind: "agent",
  repoPath: "/repos/acme",
  name: "moravia",
  provider: "claude",
  base: "main",
};

describe("EmptyWorkspace", () => {
  beforeEach(() => useAppStore.setState(useAppStore.getInitialState(), true));

  it("renders a draft while the workspace has not loaded", () => {
    useAppStore.setState({ workspace: null });
    render(<EmptyWorkspace draft={draft} />);
    expect(screen.getByText("What should be the first task?")).toBeTruthy();
  });

  it("brings a new draft in, and draws one moved from another host in place", () => {
    const projects = [{ path: "/repos/acme", name: "Acme", project_id: "p1" }] as ProjectRef[];
    useAppStore.setState({ workspace: { projects, agents: [] } as unknown as Workspace });

    const fresh = render(<EmptyWorkspace draft={draft} />);
    expect(fresh.container.querySelector(".empty-wrap")?.classList.contains("fade-in")).toBe(true);
    fresh.unmount();

    const moved = render(<EmptyWorkspace draft={{ ...draft, arrived: true }} />);
    expect(moved.container.querySelector(".fade-in")).toBeNull();
  });

  it("offers the workspace's projects", () => {
    const projects = [
      { path: "/repos/acme", name: "Acme", project_id: "p1" },
      { path: "/repos/other", name: "Other", project_id: "p2" },
    ] as ProjectRef[];
    useAppStore.setState({ workspace: { projects, agents: [] } as unknown as Workspace });
    render(<EmptyWorkspace draft={draft} />);
    expect(screen.getByText("Acme")).toBeTruthy();
  });

  it("says what a copy of another device's project starts from, only on a copy", () => {
    const own = [{ path: "/repos/acme", name: "Acme", project_id: "p1" }] as ProjectRef[];
    useAppStore.setState({ workspace: { projects: own, agents: [] } as unknown as Workspace });
    const note =
      "Starts from the branch as pushed. Commits not pushed from MacBook are not included.";

    const here = render(<EmptyWorkspace draft={draft} />);
    expect(screen.queryByText(note)).toBeNull();
    here.unmount();

    const copy = [{ ...own[0], home: { device: "macbook-key", name: "MacBook" } }] as ProjectRef[];
    useAppStore.setState({ workspace: { projects: copy, agents: [] } as unknown as Workspace });
    render(<EmptyWorkspace draft={draft} />);
    expect(screen.getByText(note)).toBeTruthy();
  });

  it("offers no folder from this Mac to a workspace declared in the cloud", () => {
    const projects = [{ path: "/repos/acme", name: "Acme", project_id: "p1" }] as ProjectRef[];
    const options: WorkspaceTypeOption[] = [
      { kind: "box", label: null, available: true, reason: null, default: true },
      { kind: "host", label: null, available: true, reason: null, default: false },
      { kind: "cloud:fly", label: "Fly.io", available: true, reason: null, default: false },
    ];
    useAppStore.setState({
      workspace: { projects, agents: [] } as unknown as Workspace,
      workspaceTypes: {
        inputs: { repoPath: draft.repoPath, provider: draft.provider },
        answer: { key: workspaceTypesKey(draft.repoPath, draft.provider), options },
        failedKey: null,
      },
    });
    const folders = () => composer.mock.lastCall?.[0].folders;

    const local = render(<EmptyWorkspace draft={draft} />);
    expect(folders()).toBe(true);
    local.unmount();

    render(<EmptyWorkspace draft={{ ...draft, workspaceType: "cloud:fly" }} />);
    expect(folders()).toBe(false);
    expect(screen.getByText(/A checkout will be created on a machine at Fly\.io, at/)).toBeTruthy();
  });

  it("shows only that it is being moved while its project is made ready elsewhere", () => {
    const projects = [{ path: "/repos/acme", name: "Acme", project_id: "p1" }] as ProjectRef[];
    useAppStore.setState({ workspace: { projects, agents: [] } as unknown as Workspace });
    render(<EmptyWorkspace draft={{ ...draft, preparing: "Preparing the project on Studio…" }} />);

    expect(screen.getByRole("status").textContent).toBe("Preparing the project on Studio…");
    // The row's pickers are out of reach until it has moved or failed.
    expect((screen.getByText("Acme").closest(".empty-meta") as HTMLElement).style.display).toBe(
      "none",
    );
  });
});
